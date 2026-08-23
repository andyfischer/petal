//! Audio transport and mixing: the device handle, the PCM sound bank, the
//! realtime Petal DSP bus, and the once-per-frame pump that keeps the queue fed.
//!
//! `petal-sdl` gives us a queued (not callback) device precisely so synthesis
//! can happen on the main thread inside our own frame — which is what makes
//! Petal-authored DSP possible at all. The cost is that nobody tops the queue
//! up for us: [`AudioEngine::pump`] must run every frame, in every run mode,
//! and must keep a lead of a few frames so one long frame does not underrun.
//!
//! **Audio is never fatal.** A machine with no sound device, and every run mode
//! that does not init SDL (headless, screenshot, record — see
//! `Host::on_sdl_init`), leave [`AudioEngine::out`] as `None`. Everything here
//! still runs and still advances the APU's state; the samples are simply
//! dropped. A cart must behave identically with and without a device.
//!
//! ## The two Petal synthesis paths
//!
//! Both call a top-level cart function as `f(start_sample, count, sample_rate)`
//! and accept either an `f64_array` or a list of floats in −1..1. The array is
//! the fast path and the one the prelude shows: `docs/audio-feasibility.md`
//! measured identical instruction counts but 56% more wall time for the boxed
//! list, because `append` drags the collector into the audio slice.
//!
//! - [`AudioEngine::sync_sounds`] is the **ahead-of-time** path behind
//!   `register_sound`. It renders the whole effect in blocks at load time (~10 ms
//!   for a 0.3 s effect) and banks the PCM; `play_sound` then costs a memcpy.
//! - [`AudioEngine::begin_frame`] runs the **realtime** path behind
//!   `enable_dsp`: one block per frame, timed, and faded out if it repeatedly
//!   overruns [`DSP_BUDGET_MS`]. The budget exists because a slow block steals
//!   from the video frame, not from the device — the queue lead absorbs the
//!   jitter, so the failure mode is a dropped frame, which is exactly what the
//!   fade prevents.
//!
//! ## Detecting a hot reload
//!
//! A rendered sound must be thrown away when the cart's synthesis function
//! changes, and a hot reload is invisible from here: it keeps the same program
//! id and the same stack, and the cart re-emits `register_sound` with identical
//! arguments every frame. So the reload is detected by its *effect* instead —
//! once per frame one banked sound is re-asked for a short window of samples
//! (see [`PROBE_SAMPLES`]) and compared against what was cached. A changed
//! function answers differently and the sound is re-rendered. With sixteen
//! effects banked, an edit is picked up within about a quarter of a second, at
//! a cost of one Petal call per frame.

use std::collections::HashMap;
use std::time::Instant;

use sdl2::Sdl;

use petal::env::Env;
use petal::stack::StackKey;
use petal::value::Value;
use petal_sdl::AudioOutput;

use crate::apu::Apu;

/// Requested device format. `AudioOutput` reports what was actually obtained;
/// synthesis must use [`AudioEngine::sample_rate`], not this.
pub const SAMPLE_RATE: i32 = 44_100;
pub const CHANNELS: u8 = 2;
/// SDL's internal device buffer, in frames. ~11ms at 44.1kHz — short enough
/// that a sound effect feels attached to the frame that triggered it.
pub const DEVICE_BUFFER_FRAMES: u16 = 512;

/// How many frames of audio to keep queued ahead of the device. Three frames
/// of slack absorbs an occasional long frame without adding audible latency.
pub const TARGET_LEAD_FRAMES: u32 = 3;

/// Ceiling on how much one pump may generate, in video frames. Only reached
/// while filling the queue from empty (startup, or after a stall cleared it);
/// it bounds the catch-up work — and the DSP call that goes with it — so
/// recovering from a hitch cannot itself cause the next one.
pub const MAX_PUMP_FRAMES: u32 = 2;

/// Nominal video frame rate, used to size one pump's worth of samples.
pub const FRAME_RATE: u32 = 60;

/// Samples asked for per call while rendering a sound ahead of time. Large
/// enough that the per-call overhead is noise (measured: none worth naming),
/// small enough that a cart asking for a 30 s effect does not allocate one
/// enormous array inside the interpreter.
const RENDER_BLOCK: usize = 4096;

/// Hard cap on a registered sound's length. A cart typo (`register_sound(name,
/// 300, ...)`) should cost a moment, not five minutes of synthesis.
const MAX_SOUND_SECONDS: f32 = 30.0;

/// Window compared against the cache to notice a hot-reloaded synthesis
/// function. Sampled from a third of the way in rather than the start, because
/// most effects open on an attack from silence and would compare equal there.
const PROBE_SAMPLES: usize = 32;

/// How far apart two renders of the same sample may be before the synthesis
/// function counts as changed. Deterministic Petal reproduces its own output
/// bit for bit; this only absorbs the last-bit noise of an `f64`->`f32` trip.
const PROBE_EPSILON: f32 = 1e-6;

/// A sound whose probe keeps disagreeing is not being edited — it is using
/// `rand()`. Re-rendering it forever would hitch every frame, so probing gives
/// up on it after this many attempts.
const MAX_PROBE_RERENDERS: u32 = 3;

/// Wall-clock slice the realtime DSP function gets per frame's worth of
/// samples, from `docs/audio-feasibility.md`: about 8 lean Petal voices, 3 full
/// chip voices, or 2 rich ones on an M4. Overrunning it does not glitch the
/// device (the queue lead absorbs that) — it steals from the video frame.
pub const DSP_BUDGET_MS: f64 = 2.0;

/// Smoothing for the cost estimate the budget is judged on. One slow block —
/// a GC pause, a scheduler hiccup — must not silence a cart that is fine on
/// average, so the trip is on the running average, not the instant.
const DSP_COST_SMOOTHING: f64 = 0.2;

/// Consecutive frames the *average* must stay over budget before the bus is
/// faded out. Smoothing alone is not enough: a single 10ms block drags a
/// healthy average over the line for a frame or two, and that is a cart worth
/// keeping audible.
const DSP_OVER_FRAMES: u32 = 6;

/// Fraction of full gain the DSP bus moves per frame when tripping or
/// recovering: ~10 frames, short enough to protect the frame rate and long
/// enough not to click.
const DSP_FADE_STEP: f32 = 0.1;

/// While muted, the bus is re-measured this often (in frames) with a single
/// call mixed at zero gain, so a cart that got slow because of *what it was
/// playing* comes back on its own once the load drops.
const DSP_RETRY_FRAMES: u32 = 120;

/// Host->script binding behind the `dsp_cost_ms()` native.
pub const SYM_DSP_COST: &str = "nes_dsp_cost_ms";

/// A `register_sound` request as the natives decoded it. Re-emitted every frame
/// by a cart that registers at top level, so [`AudioEngine::sync_sounds`] must
/// be idempotent and cheap when nothing changed.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundRequest {
    pub name: String,
    pub seconds: f32,
    pub fn_name: String,
}

/// A decoded sound in the bank: mono samples in -1..1 at the device rate.
pub struct Sound {
    pub samples: Vec<f32>,
    /// The request that produced these samples. A change here re-renders
    /// immediately, without waiting for the probe.
    seconds: f32,
    fn_name: String,
    /// Where in `samples` the hot-reload probe reads.
    probe_at: usize,
    /// Re-renders triggered by the probe rather than by an argument change.
    probe_rerenders: u32,
    /// The last error this sound's function returned, so a broken cart logs
    /// once rather than once per frame.
    last_error: Option<String>,
}

/// One playing instance of a banked sound.
struct Voice {
    name: String,
    cursor: usize,
    volume: f32,
}

/// The realtime Petal synthesis bus and its budget guard.
#[derive(Default)]
struct DspBus {
    fn_name: Option<String>,
    /// Sample index handed to the next call, so a cart can keep phase across
    /// blocks from the argument alone.
    cursor: usize,
    /// The block the last [`AudioEngine::begin_frame`] produced, mixed by the
    /// next pump. Empty while muted.
    block: Vec<f32>,
    /// Gain at the start and end of `block`, interpolated across it so a fade
    /// does not step.
    gain_from: f32,
    gain_to: f32,
    /// Last measured call cost and its running average, both in milliseconds.
    cost_ms: f64,
    average_ms: f64,
    /// Consecutive frames the average has been over budget.
    over_streak: u32,
    muted: bool,
    warned: bool,
    retry_in: u32,
    last_error: Option<String>,
}

pub struct AudioEngine {
    /// `None` when no device could be opened, or when this run mode never
    /// initialized SDL. Everything degrades to "render and discard".
    out: Option<AudioOutput>,
    bank: HashMap<String, Sound>,
    /// Bank keys in registration order, so the hot-reload probe visits every
    /// sound in turn instead of following `HashMap`'s randomized iteration.
    order: Vec<String>,
    probe_cursor: usize,
    voices: Vec<Voice>,
    dsp: DspBus,
    /// Sample count [`AudioEngine::begin_frame`] committed to, consumed by the
    /// pump. The two must agree: the DSP block was rendered at this length.
    pending: Option<usize>,
    /// Scratch mix buffer, reused across frames so the pump does not allocate.
    scratch: Vec<i16>,
    /// Mono accumulator for the PCM voices and the DSP bus.
    mix: Vec<f32>,
    /// The cart's stack, discovered lazily. See [`AudioEngine::resolve_stack`].
    stack: Option<StackKey>,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        Self {
            out: None,
            bank: HashMap::new(),
            order: Vec::new(),
            probe_cursor: 0,
            voices: Vec::new(),
            dsp: DspBus::default(),
            pending: None,
            scratch: Vec::new(),
            mix: Vec::new(),
            stack: None,
        }
    }

    /// Open the playback device. Called from `Host::on_sdl_init`, so only in
    /// the windowed run modes. A failure is reported once and then ignored.
    pub fn open(&mut self, sdl: &Sdl) {
        match AudioOutput::open(sdl, SAMPLE_RATE, CHANNELS, DEVICE_BUFFER_FRAMES) {
            Ok(out) => {
                out.resume();
                self.out = Some(out);
            }
            Err(e) => eprintln!("[fantasy-nes audio] no sound device: {}", e),
        }
    }

    pub fn is_open(&self) -> bool {
        self.out.is_some()
    }

    /// The rate synthesis must use — what the device actually gave us, falling
    /// back to the requested rate when there is no device.
    pub fn sample_rate(&self) -> u32 {
        self.out
            .as_ref()
            .map(|o| o.sample_rate() as u32)
            .unwrap_or(SAMPLE_RATE as u32)
    }

    pub fn channels(&self) -> u8 {
        self.out.as_ref().map(|o| o.channels()).unwrap_or(CHANNELS)
    }

    /// Install (or replace) a rendered sound. Replacing is the hot-reload path:
    /// a cart that changes its synthesis function re-registers under the same
    /// name and playing voices pick up the new samples.
    pub fn register_sound(&mut self, name: &str, samples: Vec<f32>) {
        let probe_at = samples.len() / 3;
        let previous = self.bank.insert(
            name.to_string(),
            Sound {
                samples,
                seconds: 0.0,
                fn_name: String::new(),
                probe_at,
                probe_rerenders: 0,
                last_error: None,
            },
        );
        if previous.is_none() {
            self.order.push(name.to_string());
        }
    }

    pub fn has_sound(&self, name: &str) -> bool {
        self.bank.contains_key(name)
    }

    pub fn sound_len(&self, name: &str) -> usize {
        self.bank.get(name).map(|s| s.samples.len()).unwrap_or(0)
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// Start a voice. Unknown names are ignored: a cart may call `play_sound`
    /// before the frame that registers it, and warning every frame would be
    /// worse than the silence.
    pub fn play_sound(&mut self, name: &str, volume: f32) {
        if !self.bank.contains_key(name) {
            return;
        }
        self.voices.push(Voice {
            name: name.to_string(),
            cursor: 0,
            volume: volume.clamp(0.0, 1.0),
        });
    }

    /// Stop every voice of a sound.
    pub fn stop_sound(&mut self, name: &str) {
        self.voices.retain(|v| v.name != name);
    }

    /// Drop every playing voice and discard queued audio — a hard cut, for
    /// switching carts.
    ///
    /// The bank goes with it: a new cart's `register_sound("jump", ...)` must
    /// not inherit the old cart's "jump". The discovered stack is forgotten for
    /// the same reason — a switch creates a new one.
    pub fn reset(&mut self) {
        self.voices.clear();
        self.bank.clear();
        self.order.clear();
        self.probe_cursor = 0;
        self.dsp = DspBus::default();
        self.pending = None;
        self.stack = None;
        if let Some(out) = &self.out {
            out.clear();
        }
    }

    // ── Ahead-of-time Petal synthesis ─────────────────────────────────────

    /// Reconcile the bank with this frame's `register_sound` calls, rendering
    /// anything new or changed, and probe one existing sound for a hot reload.
    ///
    /// Rendering happens here — after the cart's frame has run, before the pump
    /// — and not inside the pump, because a 0.3 s effect costs ~10 ms: fine as
    /// a load-time hitch, fatal inside an audio deadline. A cart that registers
    /// its whole sound set at top level pays that once, then nothing.
    pub fn sync_sounds(&mut self, env: &mut Env, requests: &[SoundRequest]) {
        let Some(stack) = self.resolve_stack(env) else {
            return;
        };
        for req in requests {
            let unchanged = self
                .bank
                .get(&req.name)
                .is_some_and(|s| s.seconds == req.seconds && s.fn_name == req.fn_name);
            if !unchanged {
                self.render_sound(env, stack, req);
            }
        }
        self.probe_one(env, stack);
    }

    /// Render `req` in blocks and bank the result, keeping the request that
    /// produced it so the next frame can tell "unchanged" from "edited".
    fn render_sound(&mut self, env: &mut Env, stack: StackKey, req: &SoundRequest) {
        let rate = self.sample_rate();
        let seconds = req.seconds.clamp(0.0, MAX_SOUND_SECONDS);
        let total = (seconds * rate as f32) as usize;

        let mut samples = Vec::with_capacity(total);
        let mut error = None;
        let mut start = 0;
        while start < total {
            let count = RENDER_BLOCK.min(total - start);
            match call_block(env, stack, &req.fn_name, start, count, rate) {
                Ok(block) => {
                    // A short answer is padded rather than rejected: the sound
                    // is the length the cart asked for, whatever the function
                    // felt like returning.
                    samples.extend_from_slice(&block[..block.len().min(count)]);
                    samples.resize(start + count, 0.0);
                }
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
            start += count;
        }

        if let Some(e) = &error {
            // Log only when the message changes, so a cart with a broken
            // synthesis function does not print sixty lines a second while the
            // author fixes it.
            let previously = self.bank.get(&req.name).and_then(|s| s.last_error.clone());
            if previously.as_deref() != Some(e.as_str()) {
                eprintln!(
                    "[fantasy-nes audio] register_sound(\"{}\"): {}",
                    req.name, e
                );
            }
            samples.clear();
        }

        let probe_at = samples.len() / 3;
        let previous = self.bank.insert(
            req.name.clone(),
            Sound {
                samples,
                seconds: req.seconds,
                fn_name: req.fn_name.clone(),
                probe_at,
                probe_rerenders: 0,
                last_error: error,
            },
        );
        if previous.is_none() {
            self.order.push(req.name.clone());
        }
    }

    /// Ask one banked sound for the window it was rendered with and compare.
    /// A mismatch means the cart's function changed under us — the only signal
    /// available for a hot reload (see the module docs).
    fn probe_one(&mut self, env: &mut Env, stack: StackKey) {
        if self.order.is_empty() {
            return;
        }
        self.probe_cursor = (self.probe_cursor + 1) % self.order.len();
        let name = self.order[self.probe_cursor].clone();
        let Some(sound) = self.bank.get(&name) else {
            return;
        };
        if sound.probe_rerenders >= MAX_PROBE_RERENDERS {
            return;
        }
        let req = SoundRequest {
            name: name.clone(),
            seconds: sound.seconds,
            fn_name: sound.fn_name.clone(),
        };
        // A function that has never been callable (or errored last time) is
        // retried whole: the round robin is what rate-limits it.
        if req.fn_name.is_empty() {
            return;
        }
        if sound.samples.is_empty() || sound.last_error.is_some() {
            self.render_sound(env, stack, &req);
            return;
        }

        let at = sound.probe_at;
        let count = PROBE_SAMPLES.min(sound.samples.len() - at);
        let Ok(block) = call_block(env, stack, &req.fn_name, at, count, self.sample_rate()) else {
            return;
        };
        let expected = &self.bank[&name].samples[at..at + count];
        let changed = block.len() < count
            || expected
                .iter()
                .zip(&block)
                .any(|(a, b)| (a - b).abs() > PROBE_EPSILON);
        if !changed {
            return;
        }

        let rerenders = self.bank[&name].probe_rerenders + 1;
        self.render_sound(env, stack, &req);
        if let Some(s) = self.bank.get_mut(&name) {
            s.probe_rerenders = rerenders;
        }
        if rerenders >= MAX_PROBE_RERENDERS {
            eprintln!(
                "[fantasy-nes audio] sound \"{}\" renders differently every time \
                 (is it using rand?); no longer watching it for edits",
                name
            );
        }
    }

    // ── Realtime Petal synthesis ──────────────────────────────────────────

    /// Point the realtime bus at a cart function, or clear it with an empty
    /// name. Re-enabling resets the budget guard, so an edited cart gets a
    /// fresh chance after having been faded out.
    pub fn set_dsp(&mut self, fn_name: &str) {
        if fn_name.is_empty() {
            self.dsp = DspBus::default();
            return;
        }
        if self.dsp.fn_name.as_deref() == Some(fn_name) {
            return;
        }
        let cursor = self.dsp.cursor;
        self.dsp = DspBus {
            fn_name: Some(fn_name.to_string()),
            cursor,
            gain_to: 1.0,
            ..DspBus::default()
        };
    }

    /// Milliseconds the last realtime DSP call took, including marshalling its
    /// answer. Published to the cart as `dsp_cost_ms()`.
    pub fn dsp_cost_ms(&self) -> f64 {
        self.dsp.cost_ms
    }

    pub fn dsp_gain(&self) -> f32 {
        self.dsp.gain_to
    }

    /// Decide how much audio this frame owes the device and, if the realtime
    /// bus is on, synthesize that block now.
    ///
    /// Split from [`pump`](Self::pump) only because this half needs the `Env`
    /// and the pump does not: the host drains its command buffers (which is
    /// where `enable_dsp` arrives) with an `Env` in hand, then pumps.
    pub fn begin_frame(&mut self, env: &mut Env) {
        let want = self.want_samples();
        self.pending = Some(want);
        self.run_dsp(env, want);

        let ms = self.dsp.cost_ms;
        let sym = env.intern_symbol(SYM_DSP_COST);
        env.set_binding(sym, Value::Float(ms));
    }

    /// One frame of the realtime bus: call, time, mix-or-fade.
    ///
    /// The budget is expressed per frame's worth of samples, so a catch-up pump
    /// asking for two frames is allowed two slices — otherwise recovering from
    /// a hitch would look like an overrun and trip the guard.
    fn run_dsp(&mut self, env: &mut Env, want: usize) {
        self.dsp.block.clear();
        self.dsp.gain_from = self.dsp.gain_to;

        let Some(fn_name) = self.dsp.fn_name.clone() else {
            return;
        };
        if want == 0 {
            return;
        }
        if self.dsp.muted {
            // Muted: pay for one measurement call every DSP_RETRY_FRAMES and
            // mix nothing, so a bus that got slow can come back.
            if self.dsp.retry_in > 0 {
                self.dsp.retry_in -= 1;
                return;
            }
            self.dsp.retry_in = DSP_RETRY_FRAMES;
        }
        let Some(stack) = self.resolve_stack(env) else {
            return;
        };

        let rate = self.sample_rate();
        let start = self.dsp.cursor;
        let began = Instant::now();
        let result = call_block(env, stack, &fn_name, start, want, rate);
        let cost = began.elapsed().as_secs_f64() * 1000.0;
        self.dsp.cursor = start.wrapping_add(want);

        let block = match result {
            Ok(block) => block,
            Err(e) => {
                // A missing or failing function silences the bus outright —
                // there is nothing to fade, and nothing to retry.
                if self.dsp.last_error.as_deref() != Some(e.as_str()) {
                    eprintln!("[fantasy-nes audio] enable_dsp(\"{}\"): {}", fn_name, e);
                    self.dsp.last_error = Some(e);
                }
                self.dsp.fn_name = None;
                self.dsp.gain_to = 0.0;
                return;
            }
        };

        let slices = (want as f64 / self.frame_samples() as f64).max(1.0);
        self.charge_budget(cost, slices);

        if self.dsp.gain_from > 0.0 || self.dsp.gain_to > 0.0 {
            self.dsp.block = block;
            self.dsp.block.resize(want, 0.0);
        }
    }

    /// Charge one DSP call against the budget and move the bus's gain.
    ///
    /// `slices` is how many frames' worth of audio the call produced, so the
    /// budget scales with a catch-up block. Split out from
    /// [`run_dsp`](Self::run_dsp) so the guard can be tested without needing a
    /// Petal function that is reliably slow on every machine.
    fn charge_budget(&mut self, cost_ms: f64, slices: f64) {
        self.dsp.cost_ms = cost_ms;
        self.dsp.average_ms =
            self.dsp.average_ms * (1.0 - DSP_COST_SMOOTHING) + cost_ms * DSP_COST_SMOOTHING;

        if self.dsp.average_ms > DSP_BUDGET_MS * slices {
            self.dsp.over_streak += 1;
        } else {
            self.dsp.over_streak = 0;
        }

        if self.dsp.over_streak >= DSP_OVER_FRAMES {
            if !self.dsp.warned {
                let name = self.dsp.fn_name.clone().unwrap_or_default();
                eprintln!(
                    "[fantasy-nes audio] dsp function \"{}\" averages {:.2}ms per frame \
                     (budget {:.2}ms); fading the dsp bus out to protect the frame rate",
                    name, self.dsp.average_ms, DSP_BUDGET_MS
                );
                self.dsp.warned = true;
            }
            self.dsp.gain_to = (self.dsp.gain_from - DSP_FADE_STEP).max(0.0);
            self.dsp.muted = self.dsp.gain_to == 0.0;
        } else {
            self.dsp.muted = false;
            self.dsp.gain_to = (self.dsp.gain_from + DSP_FADE_STEP).min(1.0);
        }
    }

    // ── The pump ──────────────────────────────────────────────────────────

    /// Samples per channel one video frame is worth at the device's rate.
    fn frame_samples(&self) -> usize {
        (self.sample_rate() / FRAME_RATE).max(1) as usize
    }

    /// How much to generate this frame: enough to restore the target lead,
    /// capped so a catch-up cannot cost several frames of synthesis at once.
    ///
    /// With no device there is no queue to read, so we generate exactly one
    /// frame's worth. That keeps the APU's phase advancing at the same rate a
    /// windowed run would, which is what makes a headless run comparable.
    fn want_samples(&self) -> usize {
        let frame_samples = self.frame_samples();
        let Some(out) = &self.out else {
            return frame_samples;
        };
        let lead = out.queued_frames() as usize;
        let target = TARGET_LEAD_FRAMES as usize * frame_samples;
        target
            .saturating_sub(lead)
            .min(MAX_PUMP_FRAMES as usize * frame_samples)
    }

    /// Top the device queue up toward [`TARGET_LEAD_FRAMES`] frames of lead.
    /// Called once per frame from `Host::end_frame`, in every run mode.
    ///
    /// The APU is advanced even with no device so that a headless run and a
    /// windowed run see the same channel state; only the queueing is skipped.
    pub fn pump(&mut self, apu: &mut Apu) {
        let rate = self.sample_rate();
        let channels = self.channels() as usize;
        // `begin_frame` normally decides this (it had to, to size the DSP
        // block); falling back keeps a host that only pumps still audible.
        let want = self.pending.take().unwrap_or_else(|| self.want_samples());
        if want == 0 {
            return;
        }

        self.scratch.clear();
        self.scratch.resize(want * channels, 0);
        apu.render(&mut self.scratch, rate);

        self.mix.clear();
        self.mix.resize(want, 0.0);
        mix_voices(&self.bank, &mut self.voices, &mut self.mix);
        mix_dsp(&self.dsp, &mut self.mix);

        // The chip is mono and so is everything mixed over it, so both output
        // channels get the same signal. Saturating rather than wrapping is the
        // whole point of going through f32: two loud voices should clip, which
        // sounds bad, not wrap, which sounds like an explosion.
        for (i, sample) in self.mix.iter().enumerate() {
            let extra = sample * i16::MAX as f32;
            for c in 0..channels {
                let slot = &mut self.scratch[i * channels + c];
                *slot = (*slot as f32 + extra).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            }
        }

        if let Some(out) = &self.out
            && let Err(e) = out.queue_samples(&self.scratch)
        {
            eprintln!("[fantasy-nes audio] {}", e);
        }
    }

    // ── Finding the cart's stack ──────────────────────────────────────────

    /// The stack the cart is running on, which both Petal synthesis paths must
    /// call into.
    ///
    /// `Host::end_frame` is handed an `Env` but not a `StackKey`, and `Env`
    /// exposes no way to enumerate its stacks, so the cart's stack is found by
    /// probing: keys are handed out in increasing order and never reused, and
    /// speculative forks are dropped as soon as they are used, so the highest
    /// live key is the most recently loaded program — the running cart. The
    /// result is cached until the next [`reset`](Self::reset) (i.e. until a
    /// cart switch), and rechecked cheaply in case a fork took ids in between.
    ///
    /// This wants replacing with a `StackKey` passed down from the host; see
    /// the note in this crate's audio report.
    fn resolve_stack(&mut self, env: &Env) -> Option<StackKey> {
        const LOOKAHEAD: u32 = 64;
        let start = self.stack.map(|s| s.0).unwrap_or(0);
        let mut found = self.stack.filter(|s| env.heap_for(*s).is_some());
        for id in start..start + LOOKAHEAD {
            let key = StackKey(id);
            if env.heap_for(key).is_some() {
                found = Some(key);
            }
        }
        self.stack = found;
        found
    }
}

/// Call `fn_name(start, count, rate)` on the cart's stack and marshal its
/// answer into mono `f32`.
///
/// Both buffer shapes `design.md` and the feasibility spike name are accepted:
/// an `f64_array`, which is the fast path a cart should write, and a list of
/// floats, which is what the design documented first. `nil` is an empty block,
/// so a cart can return early without erroring.
fn call_block(
    env: &mut Env,
    stack: StackKey,
    fn_name: &str,
    start: usize,
    count: usize,
    rate: u32,
) -> Result<Vec<f32>, String> {
    let args = [
        Value::Int(start as i64),
        Value::Int(count as i64),
        Value::Int(rate as i64),
    ];
    let result = env.call_function(stack, fn_name, &args)?;
    let heap = env.heap();
    match result {
        Value::F64Array(id) => Ok(heap.get_f64_array(id).iter().map(|v| *v as f32).collect()),
        Value::List(id) => Ok(heap.get_list(id).iter().map(number).collect()),
        Value::Nil => Ok(Vec::new()),
        other => Err(format!(
            "expected an f64_array or a list of floats, got {}",
            other.type_name()
        )),
    }
}

/// A list element as a sample. Non-numbers are silence rather than an error:
/// one `nil` in a block should not throw away the whole sound.
fn number(v: &Value) -> f32 {
    match v {
        Value::Float(f) => *f as f32,
        Value::Int(n) => *n as f32,
        _ => 0.0,
    }
}

/// Advance every playing voice over `out`, dropping the ones that ran out.
///
/// Per-voice rather than per-sound: several instances of the same effect can
/// overlap (a machine gun is one `play_sound` per shot), each with its own
/// cursor and volume.
fn mix_voices(bank: &HashMap<String, Sound>, voices: &mut Vec<Voice>, out: &mut [f32]) {
    for voice in voices.iter_mut() {
        let Some(sound) = bank.get(&voice.name) else {
            // Re-rendered out from under us, or the bank was cleared.
            voice.cursor = usize::MAX;
            continue;
        };
        let available = sound.samples.len().saturating_sub(voice.cursor);
        let n = available.min(out.len());
        for (o, s) in out[..n].iter_mut().zip(&sound.samples[voice.cursor..]) {
            *o += s * voice.volume;
        }
        voice.cursor += n;
    }
    voices.retain(|v| {
        bank.get(&v.name)
            .is_some_and(|s| v.cursor < s.samples.len())
    });
}

/// Mix the realtime bus in, ramping its gain across the block so trips and
/// recoveries are inaudible as anything but a fade.
fn mix_dsp(dsp: &DspBus, out: &mut [f32]) {
    if dsp.block.is_empty() || (dsp.gain_from == 0.0 && dsp.gain_to == 0.0) {
        return;
    }
    let n = dsp.block.len().min(out.len());
    for (i, (o, b)) in out.iter_mut().zip(&dsp.block).take(n).enumerate() {
        let t = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            1.0
        };
        *o += b * (dsp.gain_from + (dsp.gain_to - dsp.gain_from) * t);
    }
}

/// Everything here runs without SDL: the device is never opened, so the engine
/// stays in its degraded "render and discard" mode and the tests read the mix
/// buffer directly. That is the same code path a headless run takes, which is
/// the point — the mixing and the sound bank are not allowed to depend on a
/// device existing.
#[cfg(test)]
mod tests {
    use super::*;

    /// A cart whose `synth` returns a constant block, so a render is trivially
    /// checkable and an "edit" is one character.
    fn cart(level: &str) -> String {
        format!(
            "fn synth(start, count, rate)
  let out = f64_array(count)
  var i = 0
  while i < count do
    out[i] = {}
    set i = i + 1
  end
  out
end
",
            level
        )
    }

    /// Load a program, create its stack and run it once — the state
    /// `call_function` needs, and the state the engine finds by probing.
    fn boot(source: &str) -> (Env, petal::program::ProgramId, StackKey) {
        let mut env = Env::new();
        let pid = env.load_program(source).expect("compiles");
        let stack = env.create_stack(pid).expect("stack");
        env.run(stack).expect("runs");
        (env, pid, stack)
    }

    fn request(seconds: f32) -> SoundRequest {
        SoundRequest {
            name: "blip".to_string(),
            seconds,
            fn_name: "synth".to_string(),
        }
    }

    #[test]
    fn renders_a_registered_sound_and_caches_it() {
        let (mut env, _, _) = boot(&cart("0.5"));
        let mut engine = AudioEngine::new();
        let reqs = vec![request(0.01)];

        engine.sync_sounds(&mut env, &reqs);
        let expected = (0.01 * SAMPLE_RATE as f32) as usize;
        assert_eq!(engine.sound_len("blip"), expected);
        assert!(engine.bank["blip"].samples.iter().all(|s| *s == 0.5));

        // Re-registering with identical arguments must not re-render: the
        // probe may run, but the cached samples are the same object's worth.
        engine.bank.get_mut("blip").unwrap().samples[0] = -1.0;
        engine.sync_sounds(&mut env, &reqs);
        assert_eq!(engine.bank["blip"].samples[0], -1.0);

        // A changed duration is a changed request, so it re-renders now.
        engine.sync_sounds(&mut env, &[request(0.02)]);
        assert_eq!(
            engine.sound_len("blip"),
            (0.02 * SAMPLE_RATE as f32) as usize
        );
        assert_eq!(engine.bank["blip"].samples[0], 0.5);
    }

    #[test]
    fn re_renders_when_the_cart_hot_reloads() {
        let (mut env, pid, stack) = boot(&cart("0.5"));
        let mut engine = AudioEngine::new();
        engine.sync_sounds(&mut env, &[request(0.01)]);
        assert_eq!(engine.bank["blip"].samples[0], 0.5);

        // Exactly what `petal-sdl`'s watcher does on an edit: recompile under
        // the same program id and transfer state into it. Nothing about the
        // request changes — only the function body.
        let reloaded = env.compile_program(pid, &cart("0.25")).expect("compiles");
        env.transfer_state(stack, reloaded).expect("transfers");
        env.reset_stack(stack).expect("reset");
        env.run(stack).expect("runs");

        engine.sync_sounds(&mut env, &[request(0.01)]);
        assert!(
            engine.bank["blip"].samples.iter().all(|s| *s == 0.25),
            "the probe should have noticed the new function and re-rendered"
        );
    }

    #[test]
    fn overlapping_voices_clip_instead_of_wrapping() {
        let mut engine = AudioEngine::new();
        engine.register_sound("hit", vec![0.9; 1024]);
        for _ in 0..4 {
            engine.play_sound("hit", 1.0);
        }
        assert_eq!(engine.voice_count(), 4);

        let mut apu = Apu::new();
        engine.pump(&mut apu);

        // 3.6 of full scale, saturated. The failure this pins is the wrap: an
        // `as i16` on the sum would give a large *negative* sample, which is a
        // detonation rather than distortion.
        assert!(engine.scratch.iter().all(|s| *s == i16::MAX));
        assert!(engine.mix.iter().all(|m| *m > 3.0));
    }

    #[test]
    fn a_voice_ends_and_is_dropped() {
        let mut engine = AudioEngine::new();
        // Shorter than one frame's worth, so one pump consumes all of it.
        engine.register_sound("tick", vec![1.0; 100]);
        engine.play_sound("tick", 0.5);

        let mut apu = Apu::new();
        engine.pump(&mut apu);
        assert_eq!(engine.voice_count(), 0);
        assert!((engine.mix[0] - 0.5).abs() < 1e-6);
        assert_eq!(engine.mix[200], 0.0);
    }

    #[test]
    fn stop_sound_silences_every_instance() {
        let mut engine = AudioEngine::new();
        engine.register_sound("loop", vec![1.0; 44100]);
        engine.play_sound("loop", 1.0);
        engine.play_sound("loop", 0.5);
        engine.stop_sound("loop");

        let mut apu = Apu::new();
        engine.pump(&mut apu);
        assert_eq!(engine.voice_count(), 0);
        assert!(engine.mix.iter().all(|m| *m == 0.0));
    }

    #[test]
    fn the_dsp_budget_trips_and_recovers() {
        let mut engine = AudioEngine::new();
        engine.set_dsp("voices");
        assert_eq!(engine.dsp_gain(), 1.0);

        // Sustained overrun: the average crosses the budget, the gain fades to
        // zero over a handful of frames, and the bus mutes.
        for _ in 0..64 {
            engine.dsp.gain_from = engine.dsp.gain_to;
            engine.charge_budget(DSP_BUDGET_MS * 4.0, 1.0);
        }
        assert!(engine.dsp.muted);
        assert_eq!(engine.dsp_gain(), 0.0);
        assert!(engine.dsp.warned);

        // Back under budget — the retry call finds it cheap again and the bus
        // fades back in rather than staying dead for the rest of the session.
        for _ in 0..64 {
            engine.dsp.gain_from = engine.dsp.gain_to;
            engine.charge_budget(DSP_BUDGET_MS * 0.1, 1.0);
        }
        assert!(!engine.dsp.muted);
        assert_eq!(engine.dsp_gain(), 1.0);
    }

    #[test]
    fn one_slow_frame_does_not_trip_the_budget() {
        let mut engine = AudioEngine::new();
        engine.set_dsp("voices");
        for _ in 0..16 {
            engine.dsp.gain_from = engine.dsp.gain_to;
            engine.charge_budget(DSP_BUDGET_MS * 0.2, 1.0);
        }
        engine.dsp.gain_from = engine.dsp.gain_to;
        engine.charge_budget(DSP_BUDGET_MS * 5.0, 1.0);
        assert!(!engine.dsp.warned, "a single spike is not an overrun");
        assert_eq!(engine.dsp_gain(), 1.0);
    }

    #[test]
    fn the_dsp_bus_is_mixed_and_faded() {
        let (mut env, _, _) = boot(&cart("0.25"));
        let mut engine = AudioEngine::new();
        engine.set_dsp("synth");
        engine.begin_frame(&mut env);

        let want = engine.pending.expect("begin_frame committed to a length");
        assert_eq!(want, engine.frame_samples());
        assert_eq!(engine.dsp.block.len(), want);

        let mut apu = Apu::new();
        engine.pump(&mut apu);
        // Gain ramps 1.0 -> 1.0 here (healthy), so the block arrives intact.
        assert!((engine.mix[0] - 0.25).abs() < 1e-6);

        // The cursor advances so a cart can derive phase from `start_sample`.
        engine.begin_frame(&mut env);
        assert_eq!(engine.dsp.cursor, want * 2);
    }

    #[test]
    fn a_missing_dsp_function_silences_the_bus_without_panicking() {
        let (mut env, _, _) = boot(&cart("0.25"));
        let mut engine = AudioEngine::new();
        engine.set_dsp("no_such_function");
        engine.begin_frame(&mut env);
        assert!(engine.dsp.block.is_empty());
        assert_eq!(engine.dsp_gain(), 0.0);

        let mut apu = Apu::new();
        engine.pump(&mut apu);
        assert!(engine.mix.iter().all(|m| *m == 0.0));
    }

    #[test]
    fn a_boxed_list_is_accepted_as_well_as_an_array() {
        let source = "fn synth(start, count, rate)
  var out = []
  var i = 0
  while i < count do
    set out = append(out, 0.5)
    set i = i + 1
  end
  out
end
";
        let (mut env, _, _) = boot(source);
        let mut engine = AudioEngine::new();
        engine.sync_sounds(&mut env, &[request(0.001)]);
        assert!(engine.sound_len("blip") > 0);
        assert!(engine.bank["blip"].samples.iter().all(|s| *s == 0.5));
    }

    #[test]
    fn a_cart_switch_empties_the_bank() {
        let mut engine = AudioEngine::new();
        engine.register_sound("hit", vec![1.0; 64]);
        engine.play_sound("hit", 1.0);
        engine.reset();
        assert!(!engine.has_sound("hit"));
        assert_eq!(engine.voice_count(), 0);
        assert!(engine.stack.is_none());
    }
}
