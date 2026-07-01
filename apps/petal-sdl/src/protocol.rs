use std::io::{self, BufRead};
use std::sync::mpsc;
use std::thread;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

use crate::commands::{clear_draw_commands, take_draw_commands_for, DrawCommand};
use crate::input::InputState;
use crate::native_fns::{bind_frame_info, bind_input, reset_canvas_ids};

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
    },
    SetState {
        name: String,
        value: JsonValue,
    },
    Screenshot,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draw_commands: Option<Vec<DrawCommand>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
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
        }
    }

    pub fn err(msg: String) -> Self {
        Self {
            ok: false,
            error: Some(msg),
            paused: None,
            frame: None,
            state: None,
            draw_commands: None,
            output: None,
            screenshot: None,
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

// --- Command handlers ---

pub fn get_state_json(
    env: &Env,
    program_id: ProgramId,
    stack_id: StackKey,
) -> serde_json::Map<String, JsonValue> {
    env.get_state_json(program_id, stack_id)
}

pub fn capture_draw_commands(
    env: &mut Env,
    stack_id: StackKey,
    input: &InputState,
) -> Result<(Vec<DrawCommand>, Vec<String>), String> {
    // Capture must run the frame *speculatively* — produce its draw commands
    // without advancing the live state. We do that by forking: bind this frame's
    // inputs / reset the canvas counter on the source, then fork. The fork
    // inherits those and starts with empty output sinks, so its draw commands and
    // prints accumulate in the fork's own context, fully isolated from the source.
    //
    // (We drive the fork by hand rather than calling `run_speculative`, which
    // forks → runs → drops and *discards* the fork's output before we could read
    // it — exactly the side effects we need here.)
    reset_canvas_ids(env);
    bind_input(env, input);

    let fork = env.fork_execution(stack_id)?;
    env.reset_stack(fork)?;
    let run = env.run(fork);

    // Drain the fork's own draw buffer + print output (decoded against the
    // fork's heap), then release the fork. Drop it whether or not the run erred.
    let result = run.map(|_| {
        let commands = take_draw_commands_for(env, fork);
        let output = env.take_output_for(fork);
        (commands, output)
    });
    env.drop_fork(fork);
    result
}

/// Run one frame under the standard contract: promote pending input edges,
/// bind, reset_stack + run. Returns the new frame count.
pub fn run_one_frame(
    env: &mut Env,
    stack_id: StackKey,
    input: &mut InputState,
    frame_count: &mut i64,
) -> Result<i64, String> {
    clear_draw_commands(env);
    reset_canvas_ids(env);

    *frame_count += 1;
    input.begin_frame(1.0 / 60.0); // Fixed dt in agent mode
    bind_frame_info(env, 1.0 / 60.0, *frame_count);
    bind_input(env, input);

    env.reset_stack(stack_id)?;
    env.run(stack_id)?;

    Ok(*frame_count)
}

/// Apply an absolute input snapshot from the agent protocol ("these keys and
/// buttons are down now"); press/release edges are derived by diffing and
/// reach the next stepped frame.
pub fn apply_input(input: &mut InputState, keys_down: &[String], mouse: Option<&MouseInput>) {
    // Only the object form carries an authoritative buttons list; the tuple
    // form (and a keys-only message) leaves held buttons untouched.
    let (buttons, position) = match mouse {
        Some(m @ MouseInput::Object { .. }) => (Some(m.buttons().to_vec()), Some(m.position())),
        Some(m) => (None, Some(m.position())),
        None => (None, None),
    };
    input.apply_absolute(keys_down, buttons.as_deref(), position);
}

pub fn set_state_from_json(
    env: &mut Env,
    program_id: ProgramId,
    stack_id: StackKey,
    name: &str,
    json_val: &JsonValue,
) -> Result<(), String> {
    env.set_state_from_json(program_id, stack_id, name, json_val)
}
