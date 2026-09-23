//! Procedural sound effects in the spirit of sfxr / jsfxr: a small
//! [`SfxParams`] record — oscillator, pitch envelope, amplitude envelope,
//! filters — rendered once into mono `f32` samples.
//!
//! This module is pure synthesis: parameters in, samples out. It knows nothing
//! about SDL, devices, mixing, or scripts — [`crate::sound`] is the layer that
//! plays renders and exposes them to Petal as `sfx(...)` / `synth(...)`.
//!
//! Unlike sfxr's normalised 0..1 knobs, parameters are in physical units (Hz,
//! seconds, octaves per second) so they read well in a script:
//!
//! ```text
//! synth({wave: "square", freq: 440, slide: 1.5, attack: 0, decay: 0.2})
//! ```
//!
//! Guarantees of [`render_sfx`]:
//!
//! - **Deterministic.** Noise comes from a seeded xorshift32, so equal
//!   parameters always give identical samples (and [`preset_params`] variants
//!   are deterministic too).
//! - **Safe.** Every parameter is clamped into a renderable range; NaN and
//!   infinite values fall back to defaults. Output is always finite.
//! - **Never clips.** The peak is at most 1.0: loud settings are scaled down,
//!   never clipped. Every sound ends with a 3 ms fade so it cannot click.
//! - **Bounded.** At least 10 ms and at most 10 s long.
//!
//! Ported from cheesecake's `engine/audio/src/sfx.cpp`; the presets and the
//! variant scheme are the same, so a sound designed there sounds the same here.

use std::collections::HashMap;
use std::f64::consts::PI as PI64;
use std::rc::Rc;

/// Sample rate used when a caller has no device rate to render against.
pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;

/// Sub-samples per output sample (anti-aliasing for square and saw waves).
const OVERSAMPLE: usize = 8;
/// Noise values per oscillator period, as in sfxr: `freq` sets how bright
/// noise sounds.
const NOISE_STEPS: usize = 32;
/// Hard cap on a rendered sound's length, in seconds.
const MAX_SECONDS: f32 = 10.0;
/// Length of the closing fade, in seconds.
const END_FADE_SECONDS: f32 = 0.003;

// ─── Oscillator shape ───────────────────────────────────────────────────────

/// The oscillator's waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wave {
    Square,
    Saw,
    Sine,
    Triangle,
    Noise,
}

impl Wave {
    pub const ALL: [Wave; 5] = [
        Wave::Square,
        Wave::Saw,
        Wave::Sine,
        Wave::Triangle,
        Wave::Noise,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Wave::Square => "square",
            Wave::Saw => "saw",
            Wave::Sine => "sine",
            Wave::Triangle => "triangle",
            Wave::Noise => "noise",
        }
    }

    /// Parse a wave name; `"sawtooth"` is accepted for [`Wave::Saw`].
    pub fn from_name(name: &str) -> Option<Wave> {
        if name == "sawtooth" {
            return Some(Wave::Saw);
        }
        Wave::ALL.into_iter().find(|w| w.name() == name)
    }
}

// ─── Parameters ─────────────────────────────────────────────────────────────

/// Everything that describes one sound effect. Units are physical; the
/// defaults are a plain 440 Hz square beep of 0.3 s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SfxParams {
    // Oscillator
    pub wave: Wave,
    /// Start frequency, Hz.
    pub freq: f32,
    /// The sound ends if a slide takes it below this (Hz); 0 = never.
    pub min_freq: f32,
    /// Frequency slide, octaves per second (negative = falling).
    pub slide: f32,
    /// Change of `slide`, octaves per second².
    pub delta_slide: f32,
    /// Vibrato amount as a fraction of frequency (0.1 = ±10 %).
    pub vibrato_depth: f32,
    /// Vibrato speed, Hz.
    pub vibrato_rate: f32,
    /// Square-wave duty cycle, 0..1.
    pub duty: f32,
    /// Duty change per second.
    pub duty_sweep: f32,

    /// Arpeggio: after `arp_time` seconds the frequency jumps by `arp_mult`
    /// (1.5 = a fifth up, 0.5 = an octave down). `arp_time` 0 = off.
    pub arp_mult: f32,
    pub arp_time: f32,

    /// Every `repeat_time` seconds the pitch envelope, duty and arpeggio
    /// restart (the amplitude envelope does not). 0 = off.
    pub repeat_time: f32,

    // Amplitude envelope: linear attack, sustain held at 1 + punch fading to
    // 1, then linear decay to silence.
    /// Seconds.
    pub attack: f32,
    /// Seconds.
    pub sustain: f32,
    /// Extra loudness at the start of sustain, 0..1.
    pub punch: f32,
    /// Seconds.
    pub decay: f32,

    // Filters, applied low-pass then high-pass. A cutoff <= 0 disables one.
    /// Low-pass cutoff, Hz; 0 = off.
    pub lpf_cutoff: f32,
    /// 0..1 (1 = a strong peak at the cutoff).
    pub lpf_resonance: f32,
    /// Cutoff change, octaves per second.
    pub lpf_sweep: f32,
    /// High-pass cutoff, Hz; 0 = off.
    pub hpf_cutoff: f32,
    /// Cutoff change, octaves per second.
    pub hpf_sweep: f32,

    /// Output gain, 0..1.
    pub volume: f32,
    /// Noise seed.
    pub seed: u32,
}

impl Default for SfxParams {
    fn default() -> Self {
        SfxParams {
            wave: Wave::Square,
            freq: 440.0,
            min_freq: 0.0,
            slide: 0.0,
            delta_slide: 0.0,
            vibrato_depth: 0.0,
            vibrato_rate: 0.0,
            duty: 0.5,
            duty_sweep: 0.0,
            arp_mult: 1.0,
            arp_time: 0.0,
            repeat_time: 0.0,
            attack: 0.0,
            sustain: 0.1,
            punch: 0.0,
            decay: 0.2,
            lpf_cutoff: 0.0,
            lpf_resonance: 0.0,
            lpf_sweep: 0.0,
            hpf_cutoff: 0.0,
            hpf_sweep: 0.0,
            volume: 0.5,
            seed: 1,
        }
    }
}

impl SfxParams {
    /// Envelope length in seconds (attack + sustain + decay).
    pub fn duration(&self) -> f32 {
        self.attack + self.sustain + self.decay
    }

    /// Every numeric field in a fixed order — the canonical encoding
    /// [`hash_params`] hashes. `wave` and `seed` are hashed separately.
    fn floats(&self) -> [f32; 21] {
        [
            self.freq,
            self.min_freq,
            self.slide,
            self.delta_slide,
            self.vibrato_depth,
            self.vibrato_rate,
            self.duty,
            self.duty_sweep,
            self.arp_mult,
            self.arp_time,
            self.repeat_time,
            self.attack,
            self.sustain,
            self.punch,
            self.decay,
            self.lpf_cutoff,
            self.lpf_resonance,
            self.lpf_sweep,
            self.hpf_cutoff,
            self.hpf_sweep,
            self.volume,
        ]
    }

    /// Clamp every parameter into a range the synth can render safely.
    fn sanitized(mut self) -> SfxParams {
        fn f(v: f32, fallback: f32) -> f32 {
            if v.is_finite() { v } else { fallback }
        }
        self.freq = f(self.freq, 440.0).clamp(1.0, 20_000.0);
        self.min_freq = f(self.min_freq, 0.0).clamp(0.0, 20_000.0);
        self.slide = f(self.slide, 0.0).clamp(-64.0, 64.0);
        self.delta_slide = f(self.delta_slide, 0.0).clamp(-512.0, 512.0);
        self.vibrato_depth = f(self.vibrato_depth, 0.0).clamp(0.0, 1.0);
        self.vibrato_rate = f(self.vibrato_rate, 0.0).clamp(0.0, 200.0);
        self.duty = f(self.duty, 0.5).clamp(0.0, 1.0);
        self.duty_sweep = f(self.duty_sweep, 0.0).clamp(-100.0, 100.0);
        self.arp_mult = f(self.arp_mult, 1.0).clamp(1.0 / 16.0, 16.0);
        self.arp_time = f(self.arp_time, 0.0).max(0.0);
        self.repeat_time = f(self.repeat_time, 0.0).max(0.0);
        self.attack = f(self.attack, 0.0).clamp(0.0, MAX_SECONDS);
        self.sustain = f(self.sustain, 0.0).clamp(0.0, MAX_SECONDS);
        self.punch = f(self.punch, 0.0).clamp(0.0, 1.0);
        self.decay = f(self.decay, 0.0).clamp(0.0, MAX_SECONDS);
        self.lpf_cutoff = f(self.lpf_cutoff, 0.0).max(0.0);
        self.lpf_resonance = f(self.lpf_resonance, 0.0).clamp(0.0, 1.0);
        self.lpf_sweep = f(self.lpf_sweep, 0.0).clamp(-64.0, 64.0);
        self.hpf_cutoff = f(self.hpf_cutoff, 0.0).max(0.0);
        self.hpf_sweep = f(self.hpf_sweep, 0.0).clamp(-64.0, 64.0);
        self.volume = f(self.volume, 0.5).clamp(0.0, 1.0);
        self
    }
}

/// Stable 64-bit hash of every field (FNV-1a over the canonical encoding).
/// `-0.0` and `+0.0` hash alike: they are the same sound.
pub fn hash_params(p: &SfxParams) -> u64 {
    let mut h = Fnv1a::new();
    h.u32(p.wave as u32);
    for v in p.floats() {
        h.u32(if v == 0.0 { 0 } else { v.to_bits() });
    }
    h.u32(p.seed);
    h.0
}

struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Fnv1a(0xcbf2_9ce4_8422_2325)
    }

    fn u32(&mut self, v: u32) {
        for i in 0..4 {
            self.0 ^= u64::from((v >> (8 * i)) & 0xff);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// Small deterministic PRNG (xorshift32) for noise and preset variants.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Rng(if seed == 0 { 0x9e37_79b9 } else { seed })
    }

    fn next(&mut self) -> u32 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        s
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform in [-1, 1).
    fn signed_unit(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

// ─── Filters ────────────────────────────────────────────────────────────────

/// Topology-preserving-transform state-variable low-pass filter (Zavalishin).
/// Stable for any cutoff below Nyquist and cheap to retune every sample.
#[derive(Default)]
struct SvfLowpass {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfLowpass {
    fn tune(&mut self, cutoff: f32, resonance: f32, rate: f32) {
        let g = (std::f32::consts::PI * cutoff / rate).tan();
        let k = 2.0 - 1.9 * resonance; // resonance 0 → Q 0.5, 1 → Q 10
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    fn process(&mut self, x: f32) -> f32 {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }
}

/// One-pole high-pass filter.
struct OnePoleHighpass {
    prev_in: f32,
    prev_out: f32,
    a: f32,
}

impl OnePoleHighpass {
    fn new() -> Self {
        OnePoleHighpass {
            prev_in: 0.0,
            prev_out: 0.0,
            a: 1.0,
        }
    }

    fn tune(&mut self, cutoff: f32, rate: f32) {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        let dt = 1.0 / rate;
        self.a = rc / (rc + dt);
    }

    fn process(&mut self, x: f32) -> f32 {
        self.prev_out = self.a * (self.prev_out + x - self.prev_in);
        self.prev_in = x;
        self.prev_out
    }
}

// ─── Synthesis ──────────────────────────────────────────────────────────────

/// Render `raw` to mono samples in −1..1 at `sample_rate` (0 = the default
/// rate). See the module docs for the guarantees.
pub fn render_sfx(raw: &SfxParams, sample_rate: u32) -> Vec<f32> {
    let p = raw.sanitized();
    let rate = if sample_rate > 0 {
        sample_rate
    } else {
        DEFAULT_SAMPLE_RATE
    };
    let frate = rate as f32;
    let os_rate = frate * OVERSAMPLE as f32;

    let total_seconds = p.duration().clamp(0.01, MAX_SECONDS);
    let total = (total_seconds * frate).ceil() as usize;
    let attack_n = (p.attack * frate) as usize;
    let sustain_n = (p.sustain * frate) as usize;
    let decay_n = total.saturating_sub(attack_n + sustain_n);

    let mut out = Vec::with_capacity(total);

    let mut rng = Rng::new(p.seed);
    let mut noise = [0.0f32; NOISE_STEPS];
    for n in &mut noise {
        *n = rng.signed_unit();
    }

    // Per-sample multipliers for the exponential filter sweeps.
    let nyquist_guard = 0.45 * os_rate;
    let lpf_step = (p.lpf_sweep / frate).exp2();
    let hpf_step = (p.hpf_sweep / frate).exp2();
    let use_lpf = p.lpf_cutoff > 0.0;
    let use_hpf = p.hpf_cutoff > 0.0;
    let mut lpf_cut = p.lpf_cutoff;
    let mut hpf_cut = p.hpf_cutoff;
    let mut lpf = SvfLowpass::default();
    let mut hpf = OnePoleHighpass::new();

    // Pitch state, restarted by `repeat`.
    let mut freq = f64::from(p.freq);
    let mut slide = f64::from(p.slide);
    let mut duty = f64::from(p.duty);
    let mut arp_done = false;
    let mut since_restart = 0usize;
    let repeat_n = if p.repeat_time > 0.0 {
        ((p.repeat_time * frate) as usize).max(1)
    } else {
        0
    };
    let arp_n = (p.arp_time * frate) as usize;
    let fr = f64::from(frate);
    let freq_cap = 0.45 * f64::from(os_rate);
    let freq_floor = f64::from(p.min_freq.max(1.0));

    let mut phase = 0.0f64;
    for i in 0..total {
        if repeat_n != 0 && since_restart >= repeat_n {
            freq = f64::from(p.freq);
            slide = f64::from(p.slide);
            duty = f64::from(p.duty);
            arp_done = false;
            since_restart = 0;
        }

        // Pitch envelope.
        if !arp_done && p.arp_time > 0.0 && since_restart >= arp_n {
            freq *= f64::from(p.arp_mult);
            arp_done = true;
        }
        slide += f64::from(p.delta_slide) / fr;
        freq *= (slide / fr).exp2();
        freq = freq.min(freq_cap);
        if freq < freq_floor {
            break; // slid out of range: the sound ends
        }

        let t = i as f64 / fr;
        let mut f = freq;
        if p.vibrato_depth > 0.0 && p.vibrato_rate > 0.0 {
            f *= 1.0
                + f64::from(p.vibrato_depth) * (2.0 * PI64 * f64::from(p.vibrato_rate) * t).sin();
        }
        duty = (duty + f64::from(p.duty_sweep) / fr).clamp(0.02, 0.98);

        // Amplitude envelope.
        let env = if i < attack_n {
            i as f32 / attack_n as f32
        } else if i < attack_n + sustain_n {
            let s = (i - attack_n) as f32 / sustain_n.max(1) as f32;
            1.0 + (1.0 - s) * 2.0 * p.punch
        } else if decay_n > 0 {
            1.0 - (i - attack_n - sustain_n) as f32 / decay_n as f32
        } else {
            0.0
        };

        // Filter sweeps.
        if use_lpf {
            lpf_cut = (lpf_cut * lpf_step).clamp(10.0, nyquist_guard);
            lpf.tune(lpf_cut, p.lpf_resonance, os_rate);
        }
        if use_hpf {
            hpf_cut = (hpf_cut * hpf_step).clamp(10.0, nyquist_guard);
            hpf.tune(hpf_cut, os_rate);
        }

        // Oscillator, oversampled and box-averaged back down.
        let phase_step = f / f64::from(os_rate);
        let duty32 = duty as f32;
        let mut acc = 0.0f32;
        for _ in 0..OVERSAMPLE {
            phase += phase_step;
            if phase >= 1.0 {
                phase -= phase.floor();
                if p.wave == Wave::Noise {
                    for n in &mut noise {
                        *n = rng.signed_unit();
                    }
                }
            }
            let ph = phase as f32;
            let mut s = match p.wave {
                Wave::Square => {
                    if ph < duty32 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                Wave::Saw => 1.0 - 2.0 * ph,
                Wave::Sine => (2.0 * std::f32::consts::PI * ph).sin(),
                Wave::Triangle => {
                    if ph < 0.5 {
                        4.0 * ph - 1.0
                    } else {
                        3.0 - 4.0 * ph
                    }
                }
                Wave::Noise => noise[((ph * NOISE_STEPS as f32) as usize).min(NOISE_STEPS - 1)],
            };
            if use_lpf {
                s = lpf.process(s);
            }
            if use_hpf {
                s = hpf.process(s);
            }
            acc += s;
        }
        out.push(acc / OVERSAMPLE as f32 * env * p.volume);
        since_restart += 1;
    }

    // Clean-up: finite, a click-free ending, peak <= 1.
    for s in &mut out {
        if !s.is_finite() {
            *s = 0.0;
        }
    }
    let fade = out.len().min((END_FADE_SECONDS * frate) as usize);
    let len = out.len();
    for j in 0..fade {
        out[len - 1 - j] *= j as f32 / fade as f32;
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 1.0 {
        let scale = 1.0 / peak;
        for s in &mut out {
            *s *= scale;
        }
    }
    out
}

/// Convert samples in −1..1 to signed 16-bit PCM (values outside the range
/// saturate rather than wrap).
pub fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&s| f32_to_i16(s)).collect()
}

/// One sample in −1..1 to signed 16-bit PCM, saturating.
pub fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

// ─── Presets ────────────────────────────────────────────────────────────────

/// The built-in effect library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SfxPreset {
    Jump,
    Hit,
    Pickup,
    Explosion,
    Blip,
    Laser,
    Powerup,
    Bounce,
}

impl SfxPreset {
    pub const ALL: [SfxPreset; 8] = [
        SfxPreset::Jump,
        SfxPreset::Hit,
        SfxPreset::Pickup,
        SfxPreset::Explosion,
        SfxPreset::Blip,
        SfxPreset::Laser,
        SfxPreset::Powerup,
        SfxPreset::Bounce,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SfxPreset::Jump => "jump",
            SfxPreset::Hit => "hit",
            SfxPreset::Pickup => "pickup",
            SfxPreset::Explosion => "explosion",
            SfxPreset::Blip => "blip",
            SfxPreset::Laser => "laser",
            SfxPreset::Powerup => "powerup",
            SfxPreset::Bounce => "bounce",
        }
    }

    /// Parse a preset name; `"coin"` is accepted for [`SfxPreset::Pickup`].
    pub fn from_name(name: &str) -> Option<SfxPreset> {
        if name == "coin" {
            return Some(SfxPreset::Pickup);
        }
        SfxPreset::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The canonical (variant 0) parameters.
    fn canonical(self) -> SfxParams {
        let d = SfxParams::default();
        match self {
            // Rising square chirp.
            SfxPreset::Jump => SfxParams {
                wave: Wave::Square,
                duty: 0.35,
                freq: 300.0,
                slide: 2.4,
                sustain: 0.07,
                decay: 0.17,
                hpf_cutoff: 120.0,
                volume: 0.4,
                ..d
            },
            // Short falling noise crunch.
            SfxPreset::Hit => SfxParams {
                wave: Wave::Noise,
                freq: 700.0,
                slide: -3.5,
                sustain: 0.02,
                punch: 0.4,
                decay: 0.16,
                lpf_cutoff: 7000.0,
                lpf_sweep: -3.0,
                volume: 0.5,
                ..d
            },
            // Two-note coin: B5 → E6.
            SfxPreset::Pickup => SfxParams {
                wave: Wave::Square,
                duty: 0.5,
                freq: 987.8,
                arp_mult: 1.3348,
                arp_time: 0.06,
                sustain: 0.08,
                punch: 0.45,
                decay: 0.28,
                volume: 0.35,
                ..d
            },
            // Low rumbling noise burst with a closing filter.
            SfxPreset::Explosion => SfxParams {
                wave: Wave::Noise,
                freq: 110.0,
                slide: -0.7,
                sustain: 0.22,
                punch: 0.6,
                decay: 0.75,
                lpf_cutoff: 4000.0,
                lpf_sweep: -1.8,
                volume: 0.55,
                ..d
            },
            // UI tick.
            SfxPreset::Blip => SfxParams {
                wave: Wave::Square,
                duty: 0.5,
                freq: 880.0,
                sustain: 0.035,
                decay: 0.045,
                hpf_cutoff: 100.0,
                volume: 0.3,
                ..d
            },
            // Fast falling zap.
            SfxPreset::Laser => SfxParams {
                wave: Wave::Square,
                duty: 0.3,
                duty_sweep: 1.2,
                freq: 1500.0,
                min_freq: 120.0,
                slide: -7.0,
                sustain: 0.1,
                decay: 0.14,
                hpf_cutoff: 200.0,
                volume: 0.3,
                ..d
            },
            // Repeated rising sweep with vibrato.
            SfxPreset::Powerup => SfxParams {
                wave: Wave::Square,
                duty: 0.45,
                freq: 300.0,
                slide: 3.0,
                vibrato_depth: 0.04,
                vibrato_rate: 16.0,
                repeat_time: 0.11,
                sustain: 0.32,
                decay: 0.3,
                volume: 0.3,
                ..d
            },
            // Soft low thud for physics contacts.
            SfxPreset::Bounce => SfxParams {
                wave: Wave::Triangle,
                freq: 150.0,
                slide: -2.2,
                attack: 0.002,
                sustain: 0.012,
                punch: 0.35,
                decay: 0.15,
                lpf_cutoff: 800.0,
                lpf_sweep: -2.0,
                volume: 0.55,
                ..d
            },
        }
    }
}

/// Parameters of a preset. `variant` 0 is the canonical sound; other values
/// give deterministic small variations (±1 semitone, ±10 % timing, a new
/// noise seed) so repeated sounds don't all sound identical.
pub fn preset_params(preset: SfxPreset, variant: u32) -> SfxParams {
    let mut p = preset.canonical();
    if variant == 0 {
        return p;
    }
    let mut rng = Rng::new(
        variant
            .wrapping_mul(0x9e37_79b1)
            .wrapping_add((preset as u32).wrapping_mul(0x85eb_ca6b))
            .wrapping_add(1),
    );
    rng.next();
    p.freq *= (rng.signed_unit() / 12.0).exp2();
    p.sustain *= 1.0 + 0.1 * rng.signed_unit();
    p.decay *= 1.0 + 0.1 * rng.signed_unit();
    p.seed = variant.wrapping_add(1);
    p
}

/// How a `bounce` triggered by a collision should be played.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpactVoice {
    /// 0 means "too soft to be heard: don't play".
    pub volume: f32,
    pub pitch: f32,
}

/// Map a collision impulse to how the bounce thud should be played.
/// `reference_impulse` is the impulse that gives full volume. Volume grows
/// with a perceptual (square-root) curve and saturates at 1; hits below 3 %
/// of the reference are silent. Harder hits play slightly lower and fuller,
/// soft taps slightly higher and lighter (pitch 0.85 .. 1.25).
pub fn impact_voice(impulse: f32, reference_impulse: f32) -> ImpactVoice {
    let silent = ImpactVoice {
        volume: 0.0,
        pitch: 1.0,
    };
    if !(reference_impulse > 0.0) || !impulse.is_finite() {
        return silent;
    }
    let x = impulse.abs() / reference_impulse;
    if !(x >= 0.03) {
        return silent;
    }
    let strength = x.min(1.0);
    ImpactVoice {
        volume: strength.sqrt(),
        pitch: 1.25 - 0.4 * strength,
    }
}

// ─── Cache ──────────────────────────────────────────────────────────────────

/// Renders keyed by [`hash_params`], so asking for "the same" sound again
/// costs a lookup. Single-threaded, like the host that owns it.
///
/// Bounded: a script sweeping a `synth` parameter continuously would
/// otherwise grow it without limit, so past [`SfxCache::MAX_ENTRIES`] it
/// starts over (voices already playing hold their own `Rc`).
pub struct SfxCache {
    sample_rate: u32,
    sounds: HashMap<u64, Rc<[f32]>>,
}

impl SfxCache {
    pub const MAX_ENTRIES: usize = 256;

    pub fn new(sample_rate: u32) -> Self {
        SfxCache {
            sample_rate,
            sounds: HashMap::new(),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Change the render rate (e.g. once a device reports what it obtained).
    /// Drops every cached render, since they are at the old rate.
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.sounds.clear();
        }
    }

    pub fn get(&mut self, params: &SfxParams) -> Rc<[f32]> {
        let key = hash_params(params);
        if let Some(s) = self.sounds.get(&key) {
            return s.clone();
        }
        if self.sounds.len() >= Self::MAX_ENTRIES {
            self.sounds.clear();
        }
        let sound: Rc<[f32]> = render_sfx(params, self.sample_rate).into();
        self.sounds.insert(key, sound.clone());
        sound
    }

    pub fn len(&self) -> usize {
        self.sounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sounds.is_empty()
    }

    pub fn clear(&mut self) {
        self.sounds.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 22_050;

    fn peak(s: &[f32]) -> f32 {
        s.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    #[test]
    fn every_preset_is_audible_bounded_and_finite() {
        for preset in SfxPreset::ALL {
            for variant in [0, 1, 7] {
                let s = render_sfx(&preset_params(preset, variant), RATE);
                assert!(!s.is_empty(), "{} v{variant}: empty", preset.name());
                assert!(
                    s.iter().all(|v| v.is_finite()),
                    "{}: non-finite",
                    preset.name()
                );
                let p = peak(&s);
                assert!(p <= 1.0, "{} v{variant}: peak {p} clips", preset.name());
                assert!(
                    p > 0.05,
                    "{} v{variant}: peak {p} is near-silent",
                    preset.name()
                );
                assert!(rms(&s) > 0.01, "{} v{variant}: rms too low", preset.name());
            }
        }
    }

    #[test]
    fn rendering_is_deterministic() {
        for preset in SfxPreset::ALL {
            let p = preset_params(preset, 3);
            assert_eq!(
                render_sfx(&p, RATE),
                render_sfx(&p, RATE),
                "{}",
                preset.name()
            );
        }
    }

    #[test]
    fn variants_are_deterministic_and_distinct() {
        for preset in SfxPreset::ALL {
            assert_eq!(preset_params(preset, 5), preset_params(preset, 5));
            assert_ne!(preset_params(preset, 1), preset_params(preset, 2));
            assert_ne!(preset_params(preset, 0), preset_params(preset, 1));
        }
        // The variation is small: within a semitone and 10 % timing.
        let base = preset_params(SfxPreset::Jump, 0);
        for v in 1..32 {
            let p = preset_params(SfxPreset::Jump, v);
            let semis = 12.0 * (p.freq / base.freq).log2();
            assert!(semis.abs() <= 1.0 + 1e-4, "variant {v}: {semis} semitones");
            assert!((p.decay / base.decay - 1.0).abs() <= 0.1 + 1e-4);
        }
    }

    #[test]
    fn different_seeds_give_different_noise() {
        let a = SfxParams {
            wave: Wave::Noise,
            seed: 1,
            ..SfxParams::default()
        };
        let b = SfxParams { seed: 2, ..a };
        assert_ne!(render_sfx(&a, RATE), render_sfx(&b, RATE));
    }

    #[test]
    fn length_follows_the_envelope() {
        let p = SfxParams {
            attack: 0.05,
            sustain: 0.1,
            decay: 0.15,
            ..SfxParams::default()
        };
        let s = render_sfx(&p, RATE);
        assert_eq!(s.len(), (0.3 * RATE as f32).ceil() as usize);
        // A zero-length envelope still renders the 10 ms minimum.
        let z = SfxParams {
            sustain: 0.0,
            decay: 0.0,
            ..SfxParams::default()
        };
        assert_eq!(
            render_sfx(&z, RATE).len(),
            (0.01 * RATE as f32).ceil() as usize
        );
    }

    #[test]
    fn a_slide_below_min_freq_ends_the_sound_early() {
        let full = render_sfx(&SfxParams::default(), RATE).len();
        let p = SfxParams {
            slide: -20.0,
            min_freq: 200.0,
            ..SfxParams::default()
        };
        assert!(render_sfx(&p, RATE).len() < full);
    }

    #[test]
    fn loud_settings_are_scaled_not_clipped() {
        let p = SfxParams {
            punch: 1.0,
            volume: 1.0,
            lpf_cutoff: 500.0,
            lpf_resonance: 1.0,
            wave: Wave::Saw,
            ..SfxParams::default()
        };
        let s = render_sfx(&p, RATE);
        assert!(peak(&s) <= 1.0);
        assert!(peak(&s) > 0.5);
    }

    #[test]
    fn garbage_parameters_are_sanitized() {
        let p = SfxParams {
            freq: f32::NAN,
            slide: f32::INFINITY,
            decay: 1e9,
            duty: -3.0,
            lpf_cutoff: f32::NEG_INFINITY,
            volume: f32::NAN,
            ..SfxParams::default()
        };
        let s = render_sfx(&p, RATE);
        assert!(s.iter().all(|v| v.is_finite()));
        assert!(s.len() <= (MAX_SECONDS * RATE as f32).ceil() as usize);
    }

    #[test]
    fn every_sound_ends_in_a_fade() {
        for preset in SfxPreset::ALL {
            let s = render_sfx(&preset_params(preset, 0), RATE);
            assert_eq!(*s.last().unwrap(), 0.0, "{}", preset.name());
        }
    }

    #[test]
    fn hash_ignores_zero_sign_and_sees_every_field() {
        let a = SfxParams {
            slide: 0.0,
            ..SfxParams::default()
        };
        let b = SfxParams { slide: -0.0, ..a };
        assert_eq!(hash_params(&a), hash_params(&b));
        assert_ne!(hash_params(&a), hash_params(&SfxParams { seed: 9, ..a }));
        assert_ne!(
            hash_params(&a),
            hash_params(&SfxParams {
                wave: Wave::Sine,
                ..a
            })
        );
        assert_ne!(
            hash_params(&a),
            hash_params(&SfxParams {
                hpf_sweep: 1.0,
                ..a
            })
        );
    }

    #[test]
    fn names_round_trip() {
        for p in SfxPreset::ALL {
            assert_eq!(SfxPreset::from_name(p.name()), Some(p));
        }
        assert_eq!(SfxPreset::from_name("coin"), Some(SfxPreset::Pickup));
        assert_eq!(SfxPreset::from_name("boing"), None);
        for w in Wave::ALL {
            assert_eq!(Wave::from_name(w.name()), Some(w));
        }
        assert_eq!(Wave::from_name("sawtooth"), Some(Wave::Saw));
    }

    #[test]
    fn impact_voice_curve() {
        assert_eq!(impact_voice(0.1, 10.0).volume, 0.0);
        assert_eq!(impact_voice(f32::NAN, 10.0).volume, 0.0);
        assert_eq!(impact_voice(5.0, 0.0).volume, 0.0);
        let full = impact_voice(20.0, 10.0);
        assert_eq!(full.volume, 1.0);
        assert!((full.pitch - 0.85).abs() < 1e-6);
        let soft = impact_voice(1.0, 10.0);
        assert!(soft.volume > 0.0 && soft.volume < 1.0 && soft.pitch > full.pitch);
    }

    #[test]
    fn i16_conversion_saturates() {
        assert_eq!(
            to_i16(&[0.0, 1.0, -1.0, 2.0, -2.0]),
            vec![0, 32767, -32767, 32767, -32767]
        );
    }

    #[test]
    fn cache_shares_renders_and_is_bounded() {
        let mut c = SfxCache::new(RATE);
        let a = c.get(&preset_params(SfxPreset::Blip, 0));
        let b = c.get(&preset_params(SfxPreset::Blip, 0));
        assert!(Rc::ptr_eq(&a, &b));
        assert_eq!(c.len(), 1);
        for i in 0..(SfxCache::MAX_ENTRIES as u32 + 10) {
            c.get(&SfxParams {
                seed: i + 100,
                sustain: 0.0,
                decay: 0.0,
                ..SfxParams::default()
            });
        }
        assert!(c.len() <= SfxCache::MAX_ENTRIES);
        c.set_sample_rate(RATE * 2);
        assert!(c.is_empty());
    }
}
