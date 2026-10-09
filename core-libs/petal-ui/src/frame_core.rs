//! The shared panel-frame core: one compiled app on one stack, driven a frame
//! at a time under the standard host contract.
//!
//! Every embedder that runs a Petal UI script per frame does the same work:
//! own an [`Env`] and a stack, accumulate [`InputEvent`]s, publish a clock,
//! bind the frame's inputs, ask the runtime's frame gate whether a run could
//! differ from the last one, and — if so — reset, run with the host's
//! providers swapped in, and hand the draw commands back. [`FrameCore`] is
//! that work, written once. It also owns the counters a tool wants to see
//! about it: frames run versus skipped, why the last one ran, and the memo
//! table's hits and misses.
//!
//! Two embeddings sit on it:
//!
//! - [`crate::harness::Headless`], the test driver, adds a deterministic
//!   fixed-`dt` clock and decodes draw commands verbatim.
//! - Garden's `garden_script::PanelHost` adds its own channels (theme,
//!   navigation, mutations, edit views, query providers, the panel store)
//!   through [`FrameHooks`], and decodes draw commands into its render
//!   vocabulary.
//!
//! What a host adds lives in the hooks and in the caller; what every host
//! shares lives here, so a counter or a clock mode added to the core reaches
//! both at once.

use std::time::Instant;

use petal::env::Env;
use petal::memo::MemoStats;
use petal::policy::RunPolicy;
use petal::program::ProgramId;
use petal::run_deps::RunReason;
use petal::stack::StackKey;
use petal::value::Value;

use crate::draw;
use crate::host_data::{self, DataProvider};
use crate::input::{self, InputEvent, InputState};

/// Where `time()` comes from.
#[derive(Debug, Clone, Copy)]
enum Clock {
    /// Seconds since this instant — the interactive default. Read fresh each
    /// frame rather than accumulated from `dt`, so it does not drift.
    Wall(Instant),
    /// An explicit reading that only [`FrameCore::advance_clock`] and
    /// [`FrameCore::set_clock`] move — for a host driving frames (a harness,
    /// `POST /tick`), where `dt` is an instruction rather than a measurement.
    Virtual(f64),
}

/// Host extension points around [`FrameCore::frame`]. Every method has an
/// empty default, so a host implements only the ones it has something for;
/// `()` implements none.
pub trait FrameHooks {
    /// After the standard frame info, clock and input are bound and before the
    /// gate is consulted: bind the host's own per-frame inputs here, so the
    /// gate sees them.
    fn bind(&mut self, _env: &mut Env) {}
    /// The gate decided to run: clear any host output buffers. Runs after the
    /// draw buffer is cleared and before the stack is reset.
    fn prepare(&mut self, _env: &mut Env) {}
    /// Immediately before `env.run`: swap host channels in.
    fn enter(&mut self) {}
    /// Immediately after `env.run`, whether or not it failed: swap them back
    /// out and collect whatever the run produced on them.
    fn exit(&mut self, _env: &mut Env) {}
}

impl FrameHooks for () {}

/// What [`FrameCore::frame`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRun {
    /// The gate said a run would reproduce the last one; the host serves its
    /// retained output.
    Skipped,
    /// The script ran; the host takes the draw commands off the env.
    Ran,
}

/// The counters a tool reads about a panel's frames — the same ones
/// `bench_panel` and `petal-ui-run --gate-stats --memo-stats` report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameStats {
    /// Frames that ran the script, since creation.
    pub frames_run: u64,
    /// Frames the gate skipped, since creation.
    pub frames_skipped: u64,
    /// Whether the most recent frame was skipped.
    pub last_frame_skipped: bool,
    /// Why the most recent frame ran, by name (`None` after a skip or before
    /// any frame). See [`run_reason_name`].
    pub last_run_reason: Option<String>,
    /// Whether the gate is consulted at all.
    pub gate: bool,
    /// Whether runs memoize user-function calls.
    pub memo: bool,
    /// The app stack's memo counters, cumulative.
    pub memo_stats: MemoStats,
}

/// A [`RunReason`] as a short stable name: `no_record`, `forced`,
/// `binding_changed:<name>`, `host_data_changed`, `state_unsettled`,
/// `rng_consumed`, `resources_changed`.
pub fn run_reason_name(env: &Env, reason: &RunReason) -> String {
    match reason {
        RunReason::NoRecord => "no_record".to_string(),
        RunReason::Forced => "forced".to_string(),
        RunReason::BindingChanged(sym) => {
            format!("binding_changed:{}", env.symbol_name(*sym).unwrap_or("?"))
        }
        RunReason::HostDataChanged => "host_data_changed".to_string(),
        RunReason::StateUnsettled => "state_unsettled".to_string(),
        RunReason::RngConsumed => "rng_consumed".to_string(),
        RunReason::ResourcesChanged => "resources_changed".to_string(),
    }
}

/// One app on one stack, run a frame at a time. See the module docs.
pub struct FrameCore {
    pub env: Env,
    /// The standard input accumulator: events fed between frames, promoted to
    /// the edge/level snapshot by [`frame`](Self::frame).
    pub input: InputState,
    program_id: ProgramId,
    stack_id: StackKey,
    clock: Clock,
    /// Value returned by the most recent run (`Nil` until one completes).
    pub result: Value,
    /// Host data source for the `host_data` native, swapped into the
    /// thread-local channel around each run.
    provider: Option<DataProvider>,
    /// Font source for `font` / `fonts` / on-demand `text_width`, swapped in
    /// around each run the same way.
    fonts: Option<draw::FontProvider>,
    /// Whether the most recent frame skipped its run.
    pub last_frame_skipped: bool,
    /// Why the most recent frame ran, when it ran (`None` after a skip).
    pub last_run_reason: Option<RunReason>,
    /// Frames that ran the script / frames the gate skipped, since creation.
    pub frames_run: u64,
    pub frames_skipped: u64,
}

impl FrameCore {
    /// Wrap an env that already has `program_id` loaded on `stack_id`. The
    /// clock starts as the wall clock.
    pub fn new(env: Env, program_id: ProgramId, stack_id: StackKey) -> FrameCore {
        FrameCore {
            env,
            input: InputState::new(),
            program_id,
            stack_id,
            clock: Clock::Wall(Instant::now()),
            result: Value::Nil,
            provider: None,
            fonts: None,
            last_frame_skipped: false,
            last_run_reason: None,
            frames_run: 0,
            frames_skipped: 0,
        }
    }

    /// The compiled app's program id (module programs have their own).
    pub fn program_id(&self) -> ProgramId {
        self.program_id
    }

    /// The stack the app runs on.
    pub fn stack_id(&self) -> StackKey {
        self.stack_id
    }

    // ── Input ────────────────────────────────────────────────────────────

    /// Feed one input event (applied to the *next* frame's snapshot).
    pub fn event(&mut self, ev: InputEvent) {
        self.input.event(ev);
    }

    // ── Clock ────────────────────────────────────────────────────────────

    /// Switch `time()` to a virtual clock starting where the clock stands now,
    /// so nothing jumps backwards. Idempotent.
    pub fn use_virtual_clock(&mut self) {
        if let Clock::Wall(_) = self.clock {
            self.clock = Clock::Virtual(self.clock());
        }
    }

    /// Advance the virtual clock by `dt` seconds; a no-op on the wall clock.
    pub fn advance_clock(&mut self, dt: f64) {
        if let Clock::Virtual(t) = &mut self.clock {
            *t += dt;
        }
    }

    /// Set the clock to exactly `t` seconds, making it virtual.
    pub fn set_clock(&mut self, t: f64) {
        self.clock = Clock::Virtual(t);
    }

    /// Whether `time()` is currently the virtual clock.
    pub fn is_virtual_clock(&self) -> bool {
        matches!(self.clock, Clock::Virtual(_))
    }

    /// The value the next frame will publish as `time()`, in seconds.
    pub fn clock(&self) -> f64 {
        match self.clock {
            Clock::Wall(start) => start.elapsed().as_secs_f64(),
            Clock::Virtual(t) => t,
        }
    }

    /// Reseed the app's `random()` stream. Applies from the next frame on.
    pub fn set_seed(&mut self, seed: u64) {
        self.env.set_seed(seed);
    }

    // ── Providers and the gate ───────────────────────────────────────────

    /// Attach a host data source for `host_data(kind, arg)`. Answers may differ
    /// from the previous source's, which the gate cannot see, so a frame that
    /// read host data runs again.
    pub fn set_data_provider(&mut self, provider: DataProvider) {
        self.provider = Some(provider);
        self.env.note_host_data_changed(self.stack_id);
    }

    /// Whether a data provider is attached.
    pub fn has_data_provider(&self) -> bool {
        self.provider.is_some()
    }

    /// Attach a font source for `font(name)` / `fonts()` / on-demand
    /// `text_width`.
    pub fn set_font_source(&mut self, fonts: draw::FontProvider) {
        self.fonts = Some(fonts);
    }

    /// Tell the gate that data behind a host-read native changed.
    pub fn note_host_data_changed(&mut self) {
        self.env.note_host_data_changed(self.stack_id);
    }

    /// Force the next frame to run whatever the gate would say.
    pub fn invalidate(&mut self) {
        self.env.invalidate_run(self.stack_id);
    }

    /// How frames run: whether [`frame`](Self::frame) skips a run the gate
    /// says would reproduce the last one, whether calls are memoized, and the
    /// optimizer passes. The env's own policy — there is no second copy here.
    pub fn policy(&self) -> RunPolicy {
        self.env.policy()
    }

    /// Set how subsequent frames run (see [`RunPolicy`]).
    pub fn set_policy(&mut self, policy: RunPolicy) {
        self.env.set_policy(policy);
    }

    // ── The frame ────────────────────────────────────────────────────────

    /// Run one frame under the standard contract: promote input edges, bind
    /// frame info, clock and input, let the host bind its own inputs, consult
    /// the gate, and — unless it skips — clear, reset and run with the
    /// providers swapped in.
    ///
    /// On [`FrameRun::Ran`] the draw commands are still in the env's buffer
    /// for the caller to take (verbatim or traced). A script error is returned
    /// after the providers and hooks have been swapped back.
    pub fn frame(
        &mut self,
        dt: f64,
        frame_count: i64,
        hooks: &mut dyn FrameHooks,
    ) -> Result<FrameRun, String> {
        self.input.begin_frame(dt);
        input::bind_frame_info(&mut self.env, dt, frame_count);
        let now = self.clock();
        input::bind_time(&mut self.env, now);
        input::bind_input(&mut self.env, &self.input);
        hooks.bind(&mut self.env);

        // The gate: with every input bound, ask whether a run could differ
        // from the last one. If not, the host's retained output *is* this
        // frame.
        let reason = self.env.run_needed_reason(self.stack_id);
        if self.env.policy().gate && reason.is_none() {
            self.last_frame_skipped = true;
            self.last_run_reason = None;
            self.frames_skipped += 1;
            return Ok(FrameRun::Skipped);
        }
        self.last_frame_skipped = false;
        self.last_run_reason = reason;
        self.frames_run += 1;

        draw::clear_draw_commands(&mut self.env);
        // Canvas ids restart at 1 each frame, so a layer drawn every frame
        // keeps the same id and a renderer keeps its texture.
        draw::reset_canvas_ids(&mut self.env);
        hooks.prepare(&mut self.env);
        self.env.reset_stack(self.stack_id)?;
        // Make the providers reachable from their natives for this run, then
        // take them back (with any cache they updated, even on a script error).
        let saved = host_data::swap_data_provider(self.provider.take());
        let saved_fonts = draw::swap_font_provider(self.fonts.take());
        hooks.enter();
        let run = self.env.run(self.stack_id);
        hooks.exit(&mut self.env);
        self.fonts = draw::swap_font_provider(saved_fonts);
        self.provider = host_data::swap_data_provider(saved);
        self.result = run?;
        Ok(FrameRun::Ran)
    }

    // ── Readout ──────────────────────────────────────────────────────────

    /// The memo counters of the app's stack, see [`petal::memo::MemoStats`].
    pub fn memo_stats(&self) -> MemoStats {
        self.env.memo_stats(self.stack_id).unwrap_or_default()
    }

    /// Gate and memo counters together, with the run reason named.
    pub fn frame_stats(&self) -> FrameStats {
        FrameStats {
            frames_run: self.frames_run,
            frames_skipped: self.frames_skipped,
            last_frame_skipped: self.last_frame_skipped,
            last_run_reason: self
                .last_run_reason
                .as_ref()
                .map(|r| run_reason_name(&self.env, r)),
            gate: self.env.policy().gate,
            memo: self.env.policy().memo,
            memo_stats: self.memo_stats(),
        }
    }

    /// All `state` variables as a JSON map keyed by (module-qualified) name.
    pub fn state(&self) -> serde_json::Map<String, serde_json::Value> {
        self.env.get_state_json(self.program_id, self.stack_id)
    }

    /// Every value the last run bound, unfiltered (requires observations to
    /// be enabled on the env).
    pub fn observations(&self) -> serde_json::Map<String, serde_json::Value> {
        self.env
            .get_observations_json(self.program_id, self.stack_id)
    }
}

impl FrameStats {
    /// The counters as JSON — the shape Garden's `/state` reports per panel.
    pub fn to_json(&self) -> serde_json::Value {
        let m = &self.memo_stats;
        serde_json::json!({
            "gate": self.gate,
            "frames_run": self.frames_run,
            "frames_skipped": self.frames_skipped,
            "last_frame_skipped": self.last_frame_skipped,
            "last_run_reason": self.last_run_reason,
            "memo": {
                "enabled": self.memo,
                "hits": m.hits,
                "misses": m.misses,
                "records": m.records,
                "inlined": m.inlined,
                "effectful": m.effectful,
                "reexecs": m.reexecs,
                "cutoffs": m.cutoffs,
                "evicted": m.evicted,
                "cold": m.cold,
            },
        })
    }
}
