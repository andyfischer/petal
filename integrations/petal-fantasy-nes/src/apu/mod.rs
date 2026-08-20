//! The APU: two pulse channels, a triangle, a noise channel and a sample
//! player.
//!
//! The model matches the PPU's: the cart writes channel state every frame and
//! the host renders a frame's worth of samples from whatever the state is now.
//! There is no sequencer, no envelope hardware, and no length counter — a cart
//! that wants a decaying note lowers the volume each frame, which is both
//! simpler to explain and strictly more expressive.
//!
//! Channel state is *sticky*: a note keeps sounding until the cart changes or
//! silences it. That is what makes [`Apu::mute`] necessary (a cart that stops
//! writing a channel is not asking for silence — it may simply be holding a
//! note), and it is why [`crate::audio::AudioEngine`] calls [`Apu::render`]
//! every frame whether or not the cart wrote anything.
//!
//! Two properties everything here is built around:
//!
//! * **Continuity.** Oscillator phase, the noise LFSR and the gain slews all
//!   live across `render` calls. A held note re-asserted every frame must
//!   produce one unbroken waveform; if any of that state reset at a block
//!   boundary the output would click at 60Hz, which is the single most
//!   audible thing that can go wrong in a chip emulation.
//! * **Determinism.** The same sequence of writes and the same sample rate
//!   give bit-identical output. Nothing here reads a clock or an RNG, and the
//!   noise channel is a plain shift register.
//!
//! `render` is also allocation-free: it is called from inside the frame, on
//! the same thread as the interpreter.
//!
//! The PCM channel here plays a buffer the *host* hands it. The sound bank,
//! the Petal-synthesized voices and the realtime DSP bus live in
//! [`crate::audio`], which mixes them over this chip output.

pub mod channels;

use channels::{
    GAIN_SLEW_SECONDS, MAX_VOLUME, Noise, Pcm, Pulse, Triangle, note_to_hz, slew_coefficient,
};

pub const PULSE_CHANNELS: usize = 2;

/// Per-channel mix levels, before the soft clip. Roughly the hardware's
/// balance: the triangle carries the bass line so it sits slightly forward,
/// the noise is a percussion bed and sits back, and two pulses at full volume
/// plus a triangle must not on their own reach the clipper.
pub const PULSE_LEVEL: f32 = 0.26;
pub const TRIANGLE_LEVEL: f32 = 0.30;
pub const NOISE_LEVEL: f32 = 0.22;
pub const PCM_LEVEL: f32 = 0.60;

/// Headroom on the summed bus. Everything at once sums past 1.0 on purpose —
/// the soft clip is what makes that sound loud rather than broken.
pub const MASTER_LEVEL: f32 = 0.9;

#[derive(Clone, Debug, Default)]
pub struct Apu {
    pub pulse: [Pulse; PULSE_CHANNELS],
    pub triangle: Triangle,
    pub noise: Noise,
    pub pcm: Pcm,
}

impl Apu {
    pub fn new() -> Self {
        Self::default()
    }

    /// `ch` is 0 or 1, `note` is a (fractional) MIDI semitone, `duty` is 0-3,
    /// `volume` is 0-15 with 0 silencing the channel. Out-of-range arguments
    /// are clamped rather than rejected — a cart is untrusted input and a
    /// wrong note beats a killed frame.
    pub fn write_pulse(&mut self, ch: usize, note_semitones: f32, duty: u8, volume: u8) {
        if ch >= PULSE_CHANNELS {
            return;
        }
        let p = &mut self.pulse[ch];
        p.hz = note_to_hz(note_semitones);
        p.duty = duty & 3;
        p.volume = volume.min(MAX_VOLUME);
    }

    /// The triangle has no volume control on the hardware, so it is a gate:
    /// `on` false silences it, `on` true sounds it at full level.
    pub fn write_triangle(&mut self, note: f32, on: bool) {
        self.triangle.hz = note_to_hz(note);
        self.triangle.on = on;
    }

    /// `period` 0-15 selects one of the chip's noise rates (0 = highest),
    /// `mode` 0 is the long sequence and 1 the short metallic one.
    pub fn write_noise(&mut self, period: u8, volume: u8, mode: u8) {
        self.noise.period = period.min(15);
        self.noise.volume = volume.min(MAX_VOLUME);
        self.noise.mode = mode & 1;
    }

    /// Restart a channel's waveform from the top. Writing a note is *not* a
    /// retrigger — a cart re-asserts held notes every frame — so this is the
    /// explicit way to make a repeated note sound repeated.
    pub fn retrigger_pulse(&mut self, ch: usize) {
        if let Some(p) = self.pulse.get_mut(ch) {
            p.phase = 0.0;
        }
    }

    pub fn retrigger_triangle(&mut self) {
        self.triangle.phase = 0.0;
    }

    /// Load and start the sample channel. `samples` are mono, -1..1, already
    /// at the device rate; the copy happens here rather than in `render` so
    /// that rendering stays allocation-free.
    pub fn play_pcm(&mut self, samples: &[f32], volume: f32, looping: bool) {
        self.pcm.data.clear();
        self.pcm.data.extend_from_slice(samples);
        self.pcm.cursor = 0;
        self.pcm.volume = volume.clamp(0.0, 1.0);
        self.pcm.looping = looping;
    }

    /// Stop the sample channel, keeping its buffer so a replay does not
    /// reallocate.
    pub fn stop_pcm(&mut self) {
        self.pcm.cursor = self.pcm.data.len();
        self.pcm.looping = false;
    }

    /// Silence every channel, keeping phases so a resumed note does not click.
    /// Used on cart switch and on hot reload.
    pub fn mute(&mut self) {
        for p in self.pulse.iter_mut() {
            p.volume = 0;
        }
        self.triangle.on = false;
        self.noise.volume = 0;
        self.stop_pcm();
    }

    /// Render into `out`, which is **interleaved stereo** — `[l, r, l, r, ...]`
    /// — so its length must be even. The chip is mono; both channels get the
    /// same signal, and the stereo layout exists only because that is what the
    /// device is opened with.
    pub fn render(&mut self, out: &mut [i16], sample_rate: u32) {
        if sample_rate == 0 {
            out.fill(0);
            return;
        }
        let rate = sample_rate as f32;
        let slew = slew_coefficient(GAIN_SLEW_SECONDS, rate);

        for frame in out.chunks_mut(2) {
            let mut mix = 0.0;
            for p in self.pulse.iter_mut() {
                mix += p.next_sample(rate, slew) * PULSE_LEVEL;
            }
            mix += self.triangle.next_sample(rate, slew) * TRIANGLE_LEVEL;
            mix += self.noise.next_sample(rate, slew) * NOISE_LEVEL;
            mix += self.pcm.next_sample() * PCM_LEVEL;

            let s = to_i16(soft_clip(mix * MASTER_LEVEL));
            for slot in frame.iter_mut() {
                *slot = s;
            }
        }
    }
}

/// A Padé approximation of tanh: unity gain around zero, a smooth knee, and a
/// hard asymptote at ±1 so the sum of five channels compresses instead of
/// wrapping. Cheap enough to run per sample.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    (x * (27.0 + x2) / (27.0 + 9.0 * x2)).clamp(-1.0, 1.0)
}

/// Scale a -1..1 sample to i16. The clamp is load-bearing: an unclamped cast
/// of a value past 1.0 saturates in Rust but the intent — never emit a
/// wrapped, full-scale-opposite-sign sample — is worth stating.
#[inline]
fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

#[cfg(test)]
mod tests {
    use super::channels::{NOISE_LONG_PERIOD, NOISE_SHORT_PERIOD, Noise};
    use super::*;

    const RATE: u32 = 44_100;

    fn render_mono(apu: &mut Apu, frames: usize) -> Vec<f32> {
        let mut buf = vec![0i16; frames * 2];
        apu.render(&mut buf, RATE);
        buf.chunks(2).map(|f| f[0] as f32 / 32768.0).collect()
    }

    /// A4 = note 69 = 440Hz should cross zero 880 times per second.
    #[test]
    fn pulse_has_the_expected_zero_crossing_rate() {
        let mut apu = Apu::new();
        apu.write_pulse(0, 69.0, 2, 15);
        let samples = render_mono(&mut apu, RATE as usize);

        let crossings = samples
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!(
            (crossings as i32 - 880).abs() <= 8,
            "expected ~880 zero crossings for A4, got {crossings}"
        );
    }

    /// Duty is the fraction of the period spent high, so the count of positive
    /// samples should track it.
    #[test]
    fn duty_changes_the_mark_space_ratio() {
        let mut previous = 0.0;
        for (duty, expected) in [(0u8, 0.125f32), (1, 0.25), (2, 0.5), (3, 0.75)] {
            let mut apu = Apu::new();
            apu.write_pulse(0, 45.0, duty, 15);
            // Discard the gain ramp, which is near zero and would blur the
            // sign of the first millisecond.
            render_mono(&mut apu, 1000);
            let samples = render_mono(&mut apu, 20_000);

            let high = samples.iter().filter(|&&s| s > 0.0).count() as f32 / samples.len() as f32;
            assert!(
                (high - expected).abs() < 0.02,
                "duty {duty}: expected ~{expected} high, got {high}"
            );
            assert!(high > previous, "duty {duty} did not widen the mark");
            previous = high;
        }
    }

    #[test]
    fn short_mode_lfsr_repeats_and_long_mode_does_not() {
        let mut short = Noise {
            mode: 1,
            ..Default::default()
        };
        let start = short.lfsr;
        for step in 1..=NOISE_SHORT_PERIOD {
            short.clock();
            if step < NOISE_SHORT_PERIOD {
                assert_ne!(short.lfsr, start, "short mode repeated early at {step}");
            }
        }
        assert_eq!(
            short.lfsr, start,
            "short mode did not repeat after 93 steps"
        );

        let mut long = Noise::default();
        let start = long.lfsr;
        for step in 1..NOISE_LONG_PERIOD {
            long.clock();
            assert_ne!(long.lfsr, start, "long mode repeated early at {step}");
        }
        long.clock();
        assert_eq!(long.lfsr, start);
    }

    /// The stair rises for the first half of the period; sampling it finely
    /// must never step backwards.
    #[test]
    fn triangle_ramp_is_monotonic() {
        let mut apu = Apu::new();
        // Low note: many samples per stair step, so the ramp is well resolved.
        apu.write_triangle(24.0, true);
        render_mono(&mut apu, 2000); // let the gate ramp settle

        let period = RATE as f32 / channels::note_to_hz(24.0);
        let half = (period / 2.0) as usize - 2;
        apu.triangle.phase = 0.0;
        let samples = render_mono(&mut apu, half);

        for w in samples.windows(2) {
            assert!(w[1] >= w[0], "triangle ramp went backwards: {w:?}");
        }
        assert!(samples[samples.len() - 1] > samples[0]);
    }

    #[test]
    fn volume_zero_is_exact_silence() {
        let mut apu = Apu::new();
        apu.write_pulse(0, 69.0, 2, 0);
        apu.write_pulse(1, 72.0, 0, 0);
        apu.write_triangle(48.0, false);
        apu.write_noise(4, 0, 0);

        let mut buf = vec![0i16; 4096];
        apu.render(&mut buf, RATE);
        assert!(
            buf.iter().all(|&s| s == 0),
            "silent channels emitted signal"
        );
    }

    /// The seam between two `render` calls must look like any other sample
    /// boundary. The block length is chosen so the seam lands in the middle of
    /// the wave's high plateau: a phase reset there would show up as a
    /// full-amplitude jump, while a continuous oscillator moves by nothing.
    #[test]
    fn held_note_is_continuous_across_render_calls() {
        let mut apu = Apu::new();
        apu.write_pulse(0, 45.0, 2, 15);
        // Exactly 400 samples per period, so the seam position is exact.
        apu.pulse[0].hz = RATE as f32 / 400.0;
        render_mono(&mut apu, 4000); // settle the gain ramp

        let a = render_mono(&mut apu, 2050); // 5 periods + an eighth
        // The cart re-asserts the same note next frame, as carts do.
        apu.write_pulse(0, 45.0, 2, 15);
        apu.pulse[0].hz = RATE as f32 / 400.0;
        let b = render_mono(&mut apu, 2050);

        let max_delta = a
            .windows(2)
            .chain(b.windows(2))
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        let seam = (b[0] - a[a.len() - 1]).abs();
        assert!(
            seam <= max_delta,
            "discontinuity at the block seam: {seam} > in-block max {max_delta}"
        );
        assert!(
            seam < 0.05 * max_delta,
            "the seam looks like a waveform edge ({seam}), i.e. the phase reset"
        );
    }

    /// Everything at once overdrives the bus; the result must compress, not
    /// wrap around to the opposite rail.
    #[test]
    fn mixer_never_wraps() {
        let mut apu = Apu::new();
        apu.write_pulse(0, 45.0, 2, 15);
        apu.write_pulse(1, 45.1, 2, 15);
        apu.write_triangle(33.0, true);
        apu.write_noise(0, 15, 0);
        apu.play_pcm(&[1.0; 8192], 1.0, true);

        let mut buf = vec![0i16; 8192];
        apu.render(&mut buf, RATE);

        // A wrap shows up as adjacent samples at opposite extremes.
        for w in buf.chunks(2).map(|f| f[0]).collect::<Vec<_>>().windows(2) {
            let jump = (w[1] as i32 - w[0] as i32).abs();
            assert!(
                jump < 60_000,
                "sample jumped {jump}, which looks like a wrap"
            );
        }
        assert!(buf.iter().all(|&s| s > i16::MIN));
    }

    /// Same writes, same rate, same bytes — the tests above and the cart smoke
    /// tests both depend on this.
    #[test]
    fn output_is_deterministic() {
        let script = |apu: &mut Apu| {
            apu.write_pulse(0, 60.0, 1, 12);
            apu.write_noise(3, 9, 1);
            apu.write_triangle(36.0, true);
        };
        let mut a = Apu::new();
        let mut b = Apu::new();
        script(&mut a);
        script(&mut b);
        assert_eq!(render_mono(&mut a, 5000), render_mono(&mut b, 5000));
    }

    #[test]
    fn pcm_plays_once_and_stops() {
        let mut apu = Apu::new();
        apu.play_pcm(&[0.5; 64], 1.0, false);
        assert!(apu.pcm.is_playing());
        let samples = render_mono(&mut apu, 128);
        assert!(samples[0] > 0.0);
        assert_eq!(samples[100], 0.0);
        assert!(!apu.pcm.is_playing());
    }
}
