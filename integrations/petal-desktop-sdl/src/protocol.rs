//! The JSON-over-stdio agent protocol, shared by every SDL host.
//!
//! Commands arrive on stdin (one JSON object per line) and responses go to
//! stdout. The command/response *shape* is fixed here; the two places a host
//! varies — how a frame's draw output is serialized to JSON and rasterized to
//! pixels — are delegated to the [`Host`] (`draw_commands_json`, `render_image`,
//! `draw_stats`), so `capture_draw_commands`/`screenshot`/`draw_stats` work for
//! any draw vocabulary.

use std::io::{self, BufRead};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

use petal_ui::draw::clear_draw_commands;
use petal_ui::input::{InputState, bind_frame_info, bind_input, bind_time, dimensions};

use crate::game_loop::Host;

// --- Commands (stdin → engine) ---

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Pause,
    Resume,
    Step {
        #[serde(default = "default_step_count")]
        n: u32,
    },
    State,
    CaptureDrawCommands,
    Input {
        #[serde(default)]
        keys_down: Vec<String>,
        #[serde(default)]
        mouse: Option<MouseInput>,
        /// Typed text to deliver to the next stepped frame, read by the
        /// script's `text_input()`.
        #[serde(default)]
        text: String,
        /// Raw relative pointer motion for the next stepped frame, read by
        /// `mouse_dx()`/`mouse_dy()` — drives mouselook over the protocol.
        #[serde(default)]
        mouse_delta: Option<MouseDelta>,
    },
    SetState {
        name: String,
        value: JsonValue,
    },
    /// Render the current frame to a PNG. With `overlay` (or while the timeline
    /// is frozen) the picture is what the window shows: the recorded frame
    /// on screen plus the timeline's trail and scrub bar.
    Screenshot {
        #[serde(default)]
        overlay: bool,
    },
    /// Optional per-frame draw statistics; hosts that don't implement it
    /// respond with an "unsupported" error.
    DrawStats,
    /// Structured per-frame report of every live pending resource (state, age,
    /// origin, absorption count) — the observability query behind the dev
    /// overlay and agent debugging. Replies in the `pending` response field.
    PendingReport,

    // ── Timeline (see `crate::timeline`) ──
    /// Report the recorded history: frame count, cursor, tracked call.
    Timeline,
    /// Stop the game at the latest recorded frame (and pause the live loop).
    Freeze,
    /// Resume from the cursor: the live execution becomes that frame's
    /// snapshot and later frames are discarded.
    Unfreeze,
    /// Move the cursor to a recorded frame (freezing first). Negative
    /// indexes count from the end: `-1` is the latest frame.
    Scrub { to: i64 },
    /// Restore the execution from `frames` frames ago and continue from
    /// there — a rewind without freezing.
    Rewind { frames: u32 },
    /// Re-simulate every recorded frame after the cursor (all of history when
    /// not frozen) through the program loaded now, with the recorded input.
    /// A hot reload while frozen does this automatically.
    Replay,
    /// Follow the `draw_*` call that painted the shape at `(x, y)` on the
    /// frame on screen. Answers with the resolved call in `site`.
    Track { x: i32, y: i32 },
    /// Where the tracked call drew on every recorded frame, in `trail`.
    Trail,
}

fn default_step_count() -> u32 {
    1
}

/// Mouse input accepts both the legacy tuple form `[x, y]` and the canonical
/// object form `{x, y, buttons?}`. The object form matches
/// petal-diagram-canvas so agents can use a single payload across transports.
/// Button ids use the petal-ui standard: 0 = left, 1 = right, 2 = middle.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MouseInput {
    Tuple(i32, i32),
    Object {
        x: i32,
        y: i32,
        #[serde(default)]
        buttons: Vec<u8>,
    },
}

impl MouseInput {
    pub fn position(&self) -> (i32, i32) {
        match *self {
            MouseInput::Tuple(x, y) => (x, y),
            MouseInput::Object { x, y, .. } => (x, y),
        }
    }

    pub fn buttons(&self) -> &[u8] {
        match self {
            MouseInput::Tuple(..) => &[],
            MouseInput::Object { buttons, .. } => buttons,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct MouseDelta {
    pub dx: i32,
    pub dy: i32,
}

// --- Responses (engine → stdout) ---

#[derive(Serialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<serde_json::Map<String, JsonValue>>,
    /// Draw commands as JSON — the shape is the host's own draw vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draw_commands: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
    /// Optional host-defined per-frame statistics (see [`Host::draw_stats`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<JsonValue>,
    /// The frame pending report — an array of live-resource summaries (see
    /// [`petal_ui::pending::pending_report`]). Present on a `pending_report`
    /// command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<JsonValue>,
    /// Timeline status (see `Timeline::status_json`); on every timeline command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<JsonValue>,
    /// The call a `track` command resolved to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<JsonValue>,
    /// The tracked call's positions across history, from `trail`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trail: Option<JsonValue>,
    /// How many frames a `replay` re-simulated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replayed: Option<usize>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            paused: None,
            frame: None,
            state: None,
            draw_commands: None,
            output: None,
            screenshot: None,
            stats: None,
            pending: None,
            timeline: None,
            site: None,
            trail: None,
            replayed: None,
        }
    }

    pub fn err(msg: String) -> Self {
        Self {
            ok: false,
            error: Some(msg),
            ..Response::ok()
        }
    }
}

// --- Stdin reader thread ---

pub fn spawn_stdin_reader() -> mpsc::Receiver<Command> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Command>(&line) {
                Ok(cmd) => {
                    if tx.send(cmd).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let resp = Response::err(format!("Invalid command: {}", e));
                    send_response(&resp);
                }
            }
        }
    });
    rx
}

pub fn send_response(resp: &Response) {
    if let Ok(json) = serde_json::to_string(resp) {
        println!("{}", json);
    }
}

// --- Frame driving ---

/// Which monotonic clock a frame binds into `time()`. A session must use ONE
/// source for its whole lifetime: mixing them lets `time()` jump backward and
/// `elapsed()` go negative, breaking the monotonicity scripts rely on.
#[derive(Clone, Copy)]
pub enum ClockSource {
    /// The real monotonic clock (`start.elapsed()`). Used by the windowed agent
    /// loop, whose live frames already advance real time — a `Step` interleaved
    /// with the live loop must stay on that same clock, so it threads the loop's
    /// `start: Instant` here.
    Wall(Instant),
    /// Deterministic `frame_count / 60` seconds per frame. Used by the
    /// fully-scripted modes (headless / screenshot / record) that never run the
    /// real-clock loop, so stepped runs are reproducible.
    Fixed,
}

impl ClockSource {
    /// The `time()` value for the frame just about to run.
    pub fn now(&self, frame_count: i64) -> f64 {
        match self {
            ClockSource::Wall(start) => start.elapsed().as_secs_f64(),
            ClockSource::Fixed => frame_count as f64 / 60.0,
        }
    }
}

/// Run one frame under the standard contract at a fixed agent-mode dt (so
/// frame-stepping is deterministic). `clock` selects the `time()` source (see
/// [`ClockSource`]). Returns the new frame count.
pub fn run_one_frame<H: Host>(
    env: &mut Env,
    stack_id: StackKey,
    input: &mut InputState,
    frame_count: &mut i64,
    clock: ClockSource,
    host: &mut H,
) -> Result<i64, String> {
    clear_draw_commands(env);
    host.prepare_frame(env);

    *frame_count += 1;
    // Advance the ExecutionContext frame so pending-resource ages grow (see
    // `env.pending_report` / the `pending_report` command).
    env.advance_frame(stack_id);
    input.begin_frame(1.0 / 60.0);
    bind_frame_info(env, 1.0 / 60.0, *frame_count);
    // Bind `time()` from the session's single clock source: the real monotonic
    // clock when stepping is interleaved with a live real-clock loop (agent
    // mode), or a deterministic 1/60 s per frame for fully-scripted runs.
    bind_time(env, clock.now(*frame_count));
    bind_input(env, input);

    env.reset_stack(stack_id)?;
    env.run(stack_id)?;
    Ok(*frame_count)
}

/// Run this frame *speculatively* — fork so live state is untouched — and hand
/// the fork to `decode` to drain/rasterize with the host's vocabulary. Returns
/// `decode`'s value plus the fork's print output. The fork is always dropped.
///
/// The caller must set up the frame's bindings (`prepare_frame` + `bind_input`)
/// on the source *before* calling this, so the fork inherits them.
///
/// (We drive the fork by hand rather than `run_speculative`, which forks → runs
/// → drops and discards the fork's output before it can be read.)
pub fn with_speculative_frame<T>(
    env: &mut Env,
    stack_id: StackKey,
    decode: impl FnOnce(&mut Env, StackKey) -> T,
) -> Result<(T, Vec<String>), String> {
    let fork = env.fork_execution(stack_id)?;
    env.reset_stack(fork)?;
    let run = env.run(fork);

    let result = run.map(|_| {
        let value = decode(env, fork);
        let output = env.take_output_for(fork);
        (value, output)
    });
    env.drop_fork(fork);
    result
}

// --- Command dispatch ---

#[allow(clippy::too_many_arguments)]
pub fn handle_command<H: Host>(
    cmd: Command,
    env: &mut Env,
    program_id: ProgramId,
    stack_id: StackKey,
    paused: &mut bool,
    input: &mut InputState,
    frame_count: &mut i64,
    clock: ClockSource,
    host: &mut H,
) {
    match cmd {
        Command::Pause => {
            *paused = true;
            send_response(&Response {
                paused: Some(true),
                ..Response::ok()
            });
        }
        Command::Resume => {
            *paused = false;
            send_response(&Response {
                paused: Some(false),
                ..Response::ok()
            });
        }
        Command::Step { n } => {
            let mut last_frame = 0i64;
            for _ in 0..n {
                match run_one_frame(env, stack_id, input, frame_count, clock, host) {
                    Ok(fc) => last_frame = fc,
                    Err(e) => {
                        send_response(&Response::err(e));
                        return;
                    }
                }
            }
            let output = env.take_output();
            send_response(&Response {
                frame: Some(last_frame),
                output: if output.is_empty() {
                    None
                } else {
                    Some(output)
                },
                ..Response::ok()
            });
        }
        Command::State => {
            let state = env.get_state_json(program_id, stack_id);
            send_response(&Response {
                state: Some(state),
                ..Response::ok()
            });
        }
        Command::CaptureDrawCommands => {
            host.prepare_frame(env);
            bind_input(env, input);
            match with_speculative_frame(env, stack_id, |env, fork| {
                host.draw_commands_json(env, fork)
            }) {
                Ok((commands, output)) => send_response(&Response {
                    draw_commands: Some(commands),
                    output: if output.is_empty() {
                        None
                    } else {
                        Some(output)
                    },
                    ..Response::ok()
                }),
                Err(e) => send_response(&Response::err(e)),
            }
        }
        Command::Input {
            keys_down,
            mouse,
            text,
            mouse_delta,
        } => {
            apply_input(
                input,
                &keys_down,
                mouse.as_ref(),
                &text,
                mouse_delta.as_ref(),
            );
            send_response(&Response::ok());
        }
        Command::SetState { name, value } => {
            match env.set_state_from_json(program_id, stack_id, &name, &value) {
                Ok(()) => send_response(&Response::ok()),
                Err(e) => send_response(&Response::err(e)),
            }
        }
        Command::Screenshot { .. } => {
            let (w, h) = dimensions(env);
            host.prepare_frame(env);
            bind_input(env, input);
            match with_speculative_frame(env, stack_id, |env, fork| {
                host.render_image(env, fork, w, h)
            }) {
                Ok((Ok(img), _output)) => {
                    let b64 = crate::screenshot::to_base64(&img);
                    send_response(&Response {
                        screenshot: Some(b64),
                        ..Response::ok()
                    });
                }
                Ok((Err(e), _)) => send_response(&Response::err(e)),
                Err(e) => send_response(&Response::err(e)),
            }
        }
        Command::DrawStats => {
            host.prepare_frame(env);
            bind_input(env, input);
            match with_speculative_frame(env, stack_id, |env, fork| host.draw_stats(env, fork)) {
                Ok((Some(stats), _)) => send_response(&Response {
                    stats: Some(stats),
                    ..Response::ok()
                }),
                Ok((None, _)) => send_response(&Response::err(
                    "draw_stats is not supported by this host".to_string(),
                )),
                Err(e) => send_response(&Response::err(e)),
            }
        }
        Command::Timeline
        | Command::Freeze
        | Command::Unfreeze
        | Command::Scrub { .. }
        | Command::Rewind { .. }
        | Command::Replay
        | Command::Track { .. }
        | Command::Trail => {
            // Owned by the game loop, which holds the timeline; a host that
            // dispatches here directly has none.
            send_response(&Response::err("timeline commands need the game loop".to_string()));
        }
        Command::PendingReport => {
            // The report is core-lib (the resource table lives on the
            // ExecutionContext), so — unlike `draw_stats` — it needs no host
            // method and has no unsupported-host fallback: it reads the live
            // resource table directly, no speculative frame required.
            let report = petal_ui::pending::pending_report(env, program_id, stack_id);
            send_response(&Response {
                pending: Some(report),
                ..Response::ok()
            });
        }
    }
}

/// Apply an absolute input snapshot from the agent protocol ("these keys and
/// buttons are down now"); press/release edges are derived by diffing and reach
/// the next stepped frame. `text` is queued as typed input; `mouse_delta` is
/// queued as relative pointer motion for the next frame.
pub fn apply_input(
    input: &mut InputState,
    keys_down: &[String],
    mouse: Option<&MouseInput>,
    text: &str,
    mouse_delta: Option<&MouseDelta>,
) {
    // Only the object form carries an authoritative buttons list; the tuple
    // form (and a keys-only message) leaves held buttons untouched.
    let (buttons, position) = match mouse {
        Some(m @ MouseInput::Object { .. }) => (Some(m.buttons().to_vec()), Some(m.position())),
        Some(m) => (None, Some(m.position())),
        None => (None, None),
    };
    input.apply_absolute(keys_down, buttons.as_deref(), position);
    if !text.is_empty() {
        input.type_text(text);
    }
    if let Some(d) = mouse_delta {
        input.move_relative(d.dx, d.dy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petal_ui::input::InputState;

    #[test]
    fn input_command_parses_text_field() {
        let cmd: Command = serde_json::from_str(r#"{"cmd":"input","text":"hi"}"#).unwrap();
        match cmd {
            Command::Input { text, .. } => assert_eq!(text, "hi"),
            _ => panic!("expected an Input command"),
        }
    }

    #[test]
    fn input_command_parses_mouse_delta() {
        let cmd: Command =
            serde_json::from_str(r#"{"cmd":"input","mouse_delta":{"dx":3,"dy":-2}}"#).unwrap();
        match cmd {
            Command::Input {
                mouse_delta: Some(d),
                ..
            } => assert_eq!((d.dx, d.dy), (3, -2)),
            _ => panic!("expected an Input command with a mouse_delta"),
        }
    }

    #[test]
    fn pending_report_command_parses() {
        let cmd: Command = serde_json::from_str(r#"{"cmd":"pending_report"}"#).unwrap();
        assert!(matches!(cmd, Command::PendingReport));
    }

    #[test]
    fn apply_input_delivers_typed_text_to_next_frame() {
        let mut input = InputState::new();
        apply_input(&mut input, &[], None, "hi", None);
        input.begin_frame(1.0 / 60.0);
        assert_eq!(input.frame_text(), "hi");
    }
}
