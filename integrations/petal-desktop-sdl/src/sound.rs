//! Script-facing sound effects: the `sfx(name, opts?)` and `synth(params)`
//! natives, a small voice [`Mixer`], and the [`SoundBoard`] a host drives from
//! [`Host::end_frame`](crate::game_loop::Host::end_frame).
//!
//! The data flow is the same one `launch_script` uses: the natives do no host
//! work during the run. They validate their arguments and push the request
//! into the `sfx` output buffer; after the run the host drains the buffer,
//! renders each request through the [`crate::sfx`] synth (cached), and hands
//! the samples to the mixer, which tops up the [`AudioOutput`] queue.
//!
//! **Why that shape, and why the natives are declared `EMITS` and nothing
//! more.** Petal memoizes user-function calls: a helper like
//! `fn land(p) sfx("bounce", {volume: p.vy / 400}) end` called with unchanged
//! arguments is *replayed* from a record instead of run, and a replay
//! re-appends the output-buffer values the call pushed last time without
//! calling any native. Because the request lives in an output buffer, the
//! replay reproduces it exactly — so `NativeEffects::EMITS` is the honest row,
//! and helpers that play sounds stay memoizable. A native that instead queued
//! the sound on a host-side list (cheesecake's design) would be invisible to
//! the replay and would have to declare an `effect`; see "Embedder pitfalls"
//! in `docs/ffi.md`. Variant rotation (so repeated `sfx("jump")` calls don't
//! sound identical) happens here, on the host, for the same reason: a native
//! that advanced a counter would have an effect.
//!
//! Sounds are fire-and-forget events: call `sfx` on an edge
//! (`key_pressed`, a collision), not every frame. A frame the frame gate
//! skips emits nothing, so a sound fired unconditionally each frame would
//! stutter with the gate rather than loop.

use std::rc::Rc;

use petal::env::Env;
use petal::heap::Heap;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::value::Value;
use sdl2::Sdl;

use crate::audio::AudioOutput;
use crate::sfx::{DEFAULT_SAMPLE_RATE, SfxCache, SfxParams, SfxPreset, Wave, f32_to_i16};

/// Output channel the sound natives push requests into.
pub const SFX_BUFFER: &str = "sfx";

/// How many distinct variants `sfx` rotates through when none is given.
pub const VARIANT_ROTATION: u32 = 8;

/// Options `sfx(name, opts)` accepts.
const SFX_OPTIONS: &[&str] = &["volume", "pitch", "pan", "variant"];

/// Playback options shared by `sfx` and `synth`.
const PLAY_OPTIONS: &[&str] = &["pitch", "pan"];

/// `synth` parameter fields, in [`SfxParams`] order (`wave` and `seed` are
/// read separately).
const SYNTH_FLOATS: &[&str] = &[
    "freq",
    "min_freq",
    "slide",
    "delta_slide",
    "vibrato_depth",
    "vibrato_rate",
    "duty",
    "duty_sweep",
    "arp_mult",
    "arp_time",
    "repeat_time",
    "attack",
    "sustain",
    "punch",
    "decay",
    "lpf_cutoff",
    "lpf_resonance",
    "lpf_sweep",
    "hpf_cutoff",
    "hpf_sweep",
    "volume",
];

/// A script's longest allowed envelope stage, in seconds. The synth itself
/// caps a sound at 10 s; this keeps a typo like `decay: 300` from stalling a
/// frame on a long render.
const MAX_STAGE_SECONDS: f32 = 5.0;

// ─── Requests ───────────────────────────────────────────────────────────────

/// What to synthesize.
#[derive(Debug, Clone, PartialEq)]
pub enum SoundSource {
    /// A built-in preset; `variant` `None` = rotate on the host.
    Preset {
        preset: SfxPreset,
        variant: Option<u32>,
    },
    /// Custom parameters.
    Synth(SfxParams),
}

/// One decoded `sfx` / `synth` call.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundRequest {
    pub source: SoundSource,
    /// Playback gain, >= 0 (a `synth`'s own `volume` shapes the render
    /// instead, so this is 1 for it).
    pub volume: f32,
    /// Playback rate: 2 = an octave up. 0.05..20.
    pub pitch: f32,
    /// Stereo balance, −1 (left) .. 1 (right).
    pub pan: f32,
}

fn type_name(v: Value) -> &'static str {
    v.type_name()
}

fn number(
    heap: &Heap,
    fields: &Record,
    key: &str,
    default: f64,
    what: &str,
) -> Result<f64, String> {
    match fields.get(heap, key) {
        None | Some(Value::Nil) => Ok(default),
        Some(v) => match v.as_f64() {
            Some(n) if n.is_finite() => Ok(n),
            Some(_) => Err(format!("{what}: {key} must be finite")),
            None => Err(format!(
                "{what}: {key} must be a number, got {}",
                type_name(v)
            )),
        },
    }
}

/// A record argument, or the empty record for a missing/`nil` one.
struct Record(Option<petal::heap::MapId>);

impl Record {
    fn new(v: Option<Value>, what: &str) -> Result<Record, String> {
        match v {
            None | Some(Value::Nil) => Ok(Record(None)),
            Some(Value::Map(id)) => Ok(Record(Some(id))),
            Some(other) => Err(format!(
                "{what}: options must be a record, got {}",
                type_name(other)
            )),
        }
    }

    fn get(&self, heap: &Heap, key: &str) -> Option<Value> {
        self.0.and_then(|id| heap.get_map(id).get(key).copied())
    }

    fn keys<'h>(&self, heap: &'h Heap) -> Vec<&'h str> {
        match self.0 {
            Some(id) => heap.get_map(id).keys().map(|k| k.as_str()).collect(),
            None => Vec::new(),
        }
    }
}

fn check_keys(heap: &Heap, rec: &Record, what: &str, allowed: &[&[&str]]) -> Result<(), String> {
    for key in rec.keys(heap) {
        if !allowed.iter().any(|set| set.contains(&key)) {
            return Err(format!("{what}: unknown option \"{key}\""));
        }
    }
    Ok(())
}

fn playback(heap: &Heap, rec: &Record, what: &str) -> Result<(f32, f32), String> {
    let pitch = number(heap, rec, "pitch", 1.0, what)?.clamp(0.05, 20.0) as f32;
    let pan = number(heap, rec, "pan", 0.0, what)?.clamp(-1.0, 1.0) as f32;
    Ok((pitch, pan))
}

/// Decode `sfx(name, opts?)`'s arguments.
pub fn parse_sfx(heap: &Heap, args: &[Value]) -> Result<SoundRequest, String> {
    const W: &str = "sfx";
    let name = match args.first() {
        Some(Value::String(id)) => heap.get_string(*id),
        Some(v) => return Err(format!("sfx: name must be a string, got {}", type_name(*v))),
        None => return Err("sfx: expected a sound name".to_string()),
    };
    let preset = SfxPreset::from_name(name).ok_or_else(|| {
        let names: Vec<&str> = SfxPreset::ALL.iter().map(|p| p.name()).collect();
        format!(
            "sfx: unknown sound \"{name}\" (presets: {})",
            names.join(" ")
        )
    })?;
    let rec = Record::new(args.get(1).copied(), W)?;
    check_keys(heap, &rec, W, &[SFX_OPTIONS])?;
    let volume = number(heap, &rec, "volume", 1.0, W)?.max(0.0) as f32;
    let (pitch, pan) = playback(heap, &rec, W)?;
    let variant = match rec.get(heap, "variant") {
        None | Some(Value::Nil) => None,
        Some(_) => Some(number(heap, &rec, "variant", 0.0, W)?.max(0.0) as u32),
    };
    Ok(SoundRequest {
        source: SoundSource::Preset { preset, variant },
        volume,
        pitch,
        pan,
    })
}

/// Decode `synth(params)`'s argument.
pub fn parse_synth(heap: &Heap, args: &[Value]) -> Result<SoundRequest, String> {
    const W: &str = "synth";
    let rec = match args.first() {
        Some(Value::Map(id)) => Record(Some(*id)),
        Some(v) => {
            return Err(format!(
                "synth: parameters must be a record, got {}",
                type_name(*v)
            ));
        }
        None => return Err("synth: expected a parameter record".to_string()),
    };
    check_keys(
        heap,
        &rec,
        W,
        &[&["wave", "seed"], SYNTH_FLOATS, PLAY_OPTIONS],
    )?;
    let mut p = SfxParams::default();
    if let Some(v) = rec.get(heap, "wave").filter(|v| *v != Value::Nil) {
        let name = match v {
            Value::String(id) => heap.get_string(id),
            other => {
                return Err(format!(
                    "synth: wave must be a string, got {}",
                    type_name(other)
                ));
            }
        };
        p.wave = Wave::from_name(name).ok_or_else(|| {
            format!(
                "synth: wave must be \"square\", \"saw\", \"sine\", \"triangle\" or \"noise\", got \"{name}\""
            )
        })?;
    }
    let fields: [&mut f32; 21] = [
        &mut p.freq,
        &mut p.min_freq,
        &mut p.slide,
        &mut p.delta_slide,
        &mut p.vibrato_depth,
        &mut p.vibrato_rate,
        &mut p.duty,
        &mut p.duty_sweep,
        &mut p.arp_mult,
        &mut p.arp_time,
        &mut p.repeat_time,
        &mut p.attack,
        &mut p.sustain,
        &mut p.punch,
        &mut p.decay,
        &mut p.lpf_cutoff,
        &mut p.lpf_resonance,
        &mut p.lpf_sweep,
        &mut p.hpf_cutoff,
        &mut p.hpf_sweep,
        &mut p.volume,
    ];
    for (field, key) in fields.into_iter().zip(SYNTH_FLOATS) {
        *field = number(heap, &rec, key, f64::from(*field), W)? as f32;
    }
    p.seed =
        number(heap, &rec, "seed", f64::from(p.seed), W)?.clamp(0.0, f64::from(u32::MAX)) as u32;
    p.attack = p.attack.clamp(0.0, MAX_STAGE_SECONDS);
    p.sustain = p.sustain.clamp(0.0, MAX_STAGE_SECONDS);
    p.decay = p.decay.clamp(0.0, MAX_STAGE_SECONDS);
    let (pitch, pan) = playback(heap, &rec, W)?;
    Ok(SoundRequest {
        source: SoundSource::Synth(p),
        volume: 1.0,
        pitch,
        pan,
    })
}

// ─── Natives ────────────────────────────────────────────────────────────────

/// Register `sfx` and `synth`. Both are `NativeEffects::EMITS`: they read
/// nothing but their arguments, change nothing on the host, and push their
/// request into the [`SFX_BUFFER`] output buffer — which a memoized replay
/// re-emits — and a `Pending` argument makes them a silent no-op.
pub fn register_natives(env: &mut Env) {
    env.register_native("sfx", native_sfx, NativeEffects::EMITS);
    env.register_native("synth", native_synth, NativeEffects::EMITS);
}

fn args_of(state: &PetalCxt) -> Result<Vec<Value>, String> {
    (1..=state.arg_count())
        .map(|i| state.get_value(i))
        .collect()
}

fn native_sfx(state: &mut PetalCxt) -> NativeResult {
    let args = args_of(state)?;
    parse_sfx(state.heap(), &args)?;
    let sym = state.intern_symbol(SFX_BUFFER);
    state.emit(sym, "sfx", args);
    state.push_nil();
    Ok(1)
}

fn native_synth(state: &mut PetalCxt) -> NativeResult {
    let args = args_of(state)?;
    parse_synth(state.heap(), &args)?;
    let sym = state.intern_symbol(SFX_BUFFER);
    state.emit(sym, "synth", args);
    state.push_nil();
    Ok(1)
}

/// Drain the [`SFX_BUFFER`] output buffer and decode every request, in call
/// order. The natives validated each one mid-run, so a request that fails to
/// decode here is only possible for a value pushed by something else; it is
/// skipped.
pub fn take_requests(env: &mut Env) -> Vec<SoundRequest> {
    let sym = env.intern_symbol(SFX_BUFFER);
    let values = env.take_output_buffer(sym);
    let heap = env.heap();
    values
        .into_iter()
        .filter_map(|v| match v {
            Value::EnumVariant { tag, data } => {
                let args = heap.get_list(data);
                match heap.get_string(tag) {
                    "sfx" => parse_sfx(heap, args).ok(),
                    "synth" => parse_synth(heap, args).ok(),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

// ─── Mixer ──────────────────────────────────────────────────────────────────

struct Voice {
    sound: Rc<[f32]>,
    /// Read position in source samples (fractional under a `pitch`).
    pos: f64,
    step: f64,
    gain_l: f32,
    gain_r: f32,
}

/// Sums playing voices into interleaved 16-bit frames. Pure (no SDL), so it
/// is testable and a host with its own transport can reuse it.
///
/// Sounds are mono at the mixer's rate; `pitch` resamples with linear
/// interpolation, `pan` is a linear balance (centre = full level in both
/// channels). The sum is clamped to the 16-bit range: a single voice never
/// clips (the synth guarantees a peak <= 1), several loud ones can.
#[derive(Default)]
pub struct Mixer {
    voices: Vec<Voice>,
}

impl Mixer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a voice. A silent or empty one is dropped.
    pub fn play(&mut self, sound: Rc<[f32]>, volume: f32, pitch: f32, pan: f32) {
        if sound.is_empty() || !(volume > 0.0) {
            return;
        }
        let pan = pan.clamp(-1.0, 1.0);
        self.voices.push(Voice {
            sound,
            pos: 0.0,
            step: f64::from(pitch.clamp(0.05, 20.0)),
            gain_l: volume * (1.0 - pan).min(1.0),
            gain_r: volume * (1.0 + pan).min(1.0),
        });
    }

    /// Voices still playing.
    pub fn active(&self) -> usize {
        self.voices.len()
    }

    pub fn stop_all(&mut self) {
        self.voices.clear();
    }

    /// Mix the next `frames` frames for `channels` (1 = mono, 2+ = the first
    /// two carry left/right, the rest are silent), advancing every voice and
    /// retiring the finished ones.
    pub fn mix(&mut self, frames: usize, channels: u8) -> Vec<i16> {
        let ch = channels.max(1) as usize;
        let mut acc = vec![0.0f32; frames * ch];
        for v in &mut self.voices {
            let last = v.sound.len() - 1;
            for f in 0..frames {
                let i = v.pos as usize;
                if i > last {
                    break;
                }
                let frac = (v.pos - i as f64) as f32;
                let a = v.sound[i];
                let b = if i < last { v.sound[i + 1] } else { 0.0 };
                let s = a + (b - a) * frac;
                if ch == 1 {
                    acc[f] += s * 0.5 * (v.gain_l + v.gain_r);
                } else {
                    acc[f * ch] += s * v.gain_l;
                    acc[f * ch + 1] += s * v.gain_r;
                }
                v.pos += v.step;
            }
        }
        self.voices.retain(|v| (v.pos as usize) < v.sound.len());
        acc.into_iter().map(f32_to_i16).collect()
    }
}

// ─── The board a host drives ────────────────────────────────────────────────

/// Everything a host needs to give its scripts `sfx` / `synth`: the render
/// cache, the variant rotation, the mixer and (when there is a device) the
/// audio queue. Call [`open`](Self::open) from `Host::on_sdl_init`,
/// [`end_frame`](Self::end_frame) from `Host::end_frame`, and answer
/// `Host::frame_gating` with `!board.is_playing()` — the queue is only topped
/// up from `end_frame`, which a gated (skipped) frame does not reach.
///
/// **Audio is never fatal.** With no device (headless, screenshot and record
/// modes never init SDL; a machine may have no sound card) requests are
/// still drained, decoded and counted, variants still rotate, and nothing is
/// rendered: a script behaves identically with and without sound.
pub struct SoundBoard {
    cache: SfxCache,
    mixer: Mixer,
    out: Option<AudioOutput>,
    rotation: [u32; SfxPreset::ALL.len()],
    played: u64,
}

impl Default for SoundBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl SoundBoard {
    /// Frames of lead to keep queued while a sound plays, in seconds: enough
    /// to ride out a long frame without underrunning, short enough to feel
    /// immediate.
    pub const LEAD_SECONDS: f32 = 0.05;

    pub fn new() -> Self {
        SoundBoard {
            cache: SfxCache::new(DEFAULT_SAMPLE_RATE),
            mixer: Mixer::new(),
            out: None,
            rotation: [0; SfxPreset::ALL.len()],
            played: 0,
        }
    }

    /// Open the default playback device (stereo, 44.1 kHz requested). A
    /// failure is reported and leaves the board silent.
    pub fn open(&mut self, sdl: &Sdl) {
        match AudioOutput::open(sdl, DEFAULT_SAMPLE_RATE as i32, 2, 512) {
            Ok(out) => {
                self.cache.set_sample_rate(out.sample_rate().max(1) as u32);
                out.resume();
                self.out = Some(out);
            }
            Err(e) => eprintln!("[audio] no sound: {e}"),
        }
    }

    /// Whether a device is open.
    pub fn has_output(&self) -> bool {
        self.out.is_some()
    }

    /// Whether a voice is still being fed to the device.
    pub fn is_playing(&self) -> bool {
        self.mixer.active() > 0
    }

    /// Requests played since the board was made.
    pub fn played(&self) -> u64 {
        self.played
    }

    /// The parameters a request resolves to, advancing the variant rotation
    /// for a preset that named none.
    pub fn resolve(&mut self, req: &SoundRequest) -> SfxParams {
        match &req.source {
            SoundSource::Preset { preset, variant } => {
                let slot = &mut self.rotation[*preset as usize];
                let v = variant.unwrap_or_else(|| {
                    let v = *slot % VARIANT_ROTATION;
                    *slot = slot.wrapping_add(1);
                    v
                });
                crate::sfx::preset_params(*preset, v)
            }
            SoundSource::Synth(p) => *p,
        }
    }

    /// Start one request.
    pub fn play(&mut self, req: &SoundRequest) {
        let params = self.resolve(req);
        self.played += 1;
        if self.out.is_none() {
            return;
        }
        let sound = self.cache.get(&params);
        self.mixer.play(sound, req.volume, req.pitch, req.pan);
    }

    /// Drain this frame's requests, start them, and top the device queue up.
    pub fn end_frame(&mut self, env: &mut Env) {
        for req in take_requests(env) {
            self.play(&req);
        }
        self.pump();
    }

    /// Top the device queue up to [`LEAD_SECONDS`](Self::LEAD_SECONDS) of
    /// audio while any voice plays.
    pub fn pump(&mut self) {
        let Some(out) = &self.out else {
            return;
        };
        if self.mixer.active() == 0 {
            return;
        }
        let lead = (out.sample_rate().max(1) as f32 * Self::LEAD_SECONDS) as u32;
        let queued = out.queued_frames();
        if queued >= lead {
            return;
        }
        let block = self.mixer.mix((lead - queued) as usize, out.channels());
        if let Err(e) = out.queue_samples(&block) {
            eprintln!("[audio] {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sfx::{preset_params, render_sfx};

    fn run_script(src: &str) -> (Env, Result<(), String>) {
        let mut env = Env::new();
        register_natives(&mut env);
        let result = (|| {
            let pid = env.load_program(src)?;
            let stack = env.create_stack(pid)?;
            env.run(stack).map(|_| ())
        })();
        (env, result)
    }

    #[test]
    fn sfx_and_synth_calls_arrive_as_requests_in_order() {
        let (mut env, r) = run_script(
            r#"
sfx("jump")
sfx("coin", {volume: 0.5, pitch: 2, pan: -1, variant: 3})
synth({wave: "sine", freq: 880.0, slide: -2.0, decay: 0.15, volume: 0.4, pan: 0.5})
"#,
        );
        r.unwrap();
        let reqs = take_requests(&mut env);
        assert_eq!(reqs.len(), 3);
        assert_eq!(
            reqs[0].source,
            SoundSource::Preset {
                preset: SfxPreset::Jump,
                variant: None
            }
        );
        assert_eq!(
            reqs[1],
            SoundRequest {
                source: SoundSource::Preset {
                    preset: SfxPreset::Pickup,
                    variant: Some(3)
                },
                volume: 0.5,
                pitch: 2.0,
                pan: -1.0,
            }
        );
        match &reqs[2].source {
            SoundSource::Synth(p) => {
                assert_eq!(p.wave, Wave::Sine);
                assert_eq!(p.freq, 880.0);
                assert_eq!(p.slide, -2.0);
                assert_eq!(p.volume, 0.4);
                assert_eq!(p.sustain, SfxParams::default().sustain);
            }
            other => panic!("expected synth, got {other:?}"),
        }
        assert_eq!(reqs[2].pan, 0.5);
        // Drained: a second take is empty.
        assert!(take_requests(&mut env).is_empty());
    }

    #[test]
    fn bad_calls_fail_the_run_with_a_useful_message() {
        for (src, needle) in [
            (r#"sfx("boing")"#, "unknown sound \"boing\""),
            (r#"sfx("jump", {volum: 1})"#, "unknown option \"volum\""),
            (
                r#"sfx("jump", {volume: "loud"})"#,
                "volume must be a number",
            ),
            (r#"synth({wave: "organ"})"#, "wave must be"),
            (r#"synth(3)"#, "must be a record"),
        ] {
            let (_, r) = run_script(src);
            let err = r.expect_err(src);
            assert!(err.contains(needle), "{src}: {err}");
        }
    }

    #[test]
    fn a_helper_replayed_by_the_memo_still_plays_its_sound() {
        // `EMITS` is the honest row: a memoized helper's replay re-appends
        // the request. Run the same frame twice with memoization on and check
        // both frames carry the sound.
        let src = r#"
fn land(strength)
  var v = 0.0
  for i in range(0, 20) do
    set v = v + strength / 200.0
  end
  sfx("bounce", {volume: v, pitch: 1.2})
end
land(7.0)
"#;
        let mut env = Env::new();
        register_natives(&mut env);
        let pid = env.load_program(src).unwrap();
        let stack = env.create_stack(pid).unwrap();
        for frame in 0..3 {
            env.reset_stack(stack).unwrap();
            env.run(stack).unwrap();
            let reqs = take_requests(&mut env);
            assert_eq!(reqs.len(), 1, "frame {frame}");
            assert!((reqs[0].volume - 0.7).abs() < 1e-5);
        }
        let stats = env.memo_stats(stack).unwrap();
        assert!(stats.hits >= 2, "the helper was not replayed: {stats:?}");
    }

    // The trap `EMITS` avoids, pinned: a native that queues on the *host*
    // (not an output buffer) but is declared `EMITS` is skipped by a replay,
    // so a memoized helper that calls it only reaches the host once. Declared
    // with an `effect`, it runs every frame. The effect audit flags the
    // dishonest row as an over-declared `emit` (it never pushed output).
    thread_local! {
        static HOST_QUEUE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    fn native_queue_on_host(state: &mut PetalCxt) -> NativeResult {
        HOST_QUEUE.with(|q| q.set(q.get() + 1));
        state.push_nil();
        Ok(1)
    }

    fn host_calls_over_frames(row: NativeEffects) -> (u32, String) {
        HOST_QUEUE.with(|q| q.set(0));
        let src = r#"
fn helper(n)
  var acc = 0
  for i in range(0, 20) do
    set acc = acc + n
  end
  queue_on_host(acc)
end
helper(2)
"#;
        let mut env = Env::new();
        env.register_native("queue_on_host", native_queue_on_host, row);
        env.set_effect_audit(true);
        let pid = env.load_program(src).unwrap();
        let stack = env.create_stack(pid).unwrap();
        for _ in 0..4 {
            env.reset_stack(stack).unwrap();
            env.run(stack).unwrap();
        }
        let report = env.effect_audit_report().to_string();
        (HOST_QUEUE.with(|q| q.get()), report)
    }

    #[test]
    fn a_host_side_queue_declared_emits_is_skipped_by_replay() {
        let (calls, report) = host_calls_over_frames(NativeEffects::EMITS);
        assert_eq!(calls, 1, "replayed frames should not have reached the host");
        assert!(
            report.contains("over-declared")
                && report.contains("queue_on_host")
                && report.contains("emit"),
            "{report}"
        );
        let (calls, _) = host_calls_over_frames(NativeEffects::EFFECT);
        assert_eq!(calls, 4, "an effect must run on every frame");
    }

    #[test]
    fn the_board_rotates_variants_deterministically() {
        let mut board = SoundBoard::new();
        let req = SoundRequest {
            source: SoundSource::Preset {
                preset: SfxPreset::Laser,
                variant: None,
            },
            volume: 1.0,
            pitch: 1.0,
            pan: 0.0,
        };
        let seq: Vec<SfxParams> = (0..VARIANT_ROTATION + 1)
            .map(|_| board.resolve(&req))
            .collect();
        for (i, p) in seq.iter().enumerate() {
            assert_eq!(
                *p,
                preset_params(SfxPreset::Laser, i as u32 % VARIANT_ROTATION)
            );
        }
        // Headless (no device): counted, not rendered.
        board.play(&req);
        assert_eq!(board.played(), 1);
        assert!(!board.is_playing());
    }

    #[test]
    fn the_mixer_plays_voices_to_completion_without_clipping() {
        let sound: Rc<[f32]> = render_sfx(&preset_params(SfxPreset::Pickup, 0), 22_050).into();
        let mut m = Mixer::new();
        m.play(sound.clone(), 1.0, 1.0, 0.0);
        m.play(sound.clone(), 0.5, 2.0, -1.0);
        assert_eq!(m.active(), 2);
        let mut out = Vec::new();
        while m.active() > 0 {
            out.extend(m.mix(512, 2));
            assert!(out.len() < 22_050 * 4, "voices never finished");
        }
        assert!(out.iter().any(|&s| s.unsigned_abs() > 1000), "silent");
        // The hard-left voice only reaches the left channel.
        let (l, r): (Vec<i16>, Vec<i16>) = out.chunks(2).map(|c| (c[0], c[1])).unzip();
        assert_ne!(l, r, "pan had no effect");
        // Silence and empties are dropped.
        m.play(Rc::from(Vec::<f32>::new()), 1.0, 1.0, 0.0);
        m.play(sound, 0.0, 1.0, 0.0);
        assert_eq!(m.active(), 0);
    }

    #[test]
    fn the_mixer_mono_path_and_pitch_length() {
        let sound: Rc<[f32]> = Rc::from(vec![0.5f32; 1000]);
        let mut m = Mixer::new();
        m.play(sound, 1.0, 2.0, 0.0);
        let out = m.mix(600, 1);
        // Octave up: 1000 source samples take 500 frames.
        assert_eq!(m.active(), 0);
        assert!(out[..499].iter().all(|&s| s > 16000));
        assert!(out[501..].iter().all(|&s| s == 0));
    }
}
