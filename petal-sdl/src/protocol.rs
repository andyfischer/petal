use std::io::{self, BufRead};
use std::sync::mpsc;
use std::thread;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::value;

use crate::commands::DrawCommand;
use crate::native_fns::{DRAW_COMMANDS, FRAME_INFO, INPUT_STATE};

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
        mouse: Option<(i32, i32)>,
    },
    SetState {
        name: String,
        value: JsonValue,
    },
}

fn default_step_count() -> u32 {
    1
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
    let names = env.state_key_names(program_id);
    let heap = env.heap();
    let mut map = serde_json::Map::new();
    if let Some(state) = env.get_all_state(stack_id) {
        for (key, val) in state {
            let name = names
                .get(key)
                .cloned()
                .unwrap_or_else(|| format!("unknown_{}", key.0));
            map.insert(name, value::value_to_json(val, heap));
        }
    }
    map
}

pub fn capture_draw_commands(
    env: &mut Env,
    stack_id: StackKey,
) -> Result<(Vec<DrawCommand>, Vec<String>), String> {
    // Clear the draw buffer before speculative run
    DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().clear());

    // Run speculatively (state is snapshot/restored internally)
    env.run_speculative(stack_id)?;

    // Collect results
    let commands = DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().drain(..).collect::<Vec<_>>());
    let output = env.take_output();

    Ok((commands, output))
}

/// Run one frame: reset_stack + run, update frame_count.
/// Returns the new frame count.
pub fn run_one_frame(env: &mut Env, stack_id: StackKey) -> Result<i64, String> {
    DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().clear());

    let frame_count = FRAME_INFO.with(|f| {
        let mut info = f.borrow_mut();
        info.frame_count += 1;
        info.dt = 1.0 / 60.0; // Fixed dt in agent mode
        info.frame_count
    });

    env.reset_stack(stack_id)?;
    env.run(stack_id)?;

    Ok(frame_count)
}

pub fn apply_input(keys_down: &[String], mouse: Option<(i32, i32)>) {
    INPUT_STATE.with(|s| {
        let mut state = s.borrow_mut();
        state.begin_frame();
        state.keys_down.clear();
        for key in keys_down {
            state.keys_down.insert(key.clone());
        }
        if let Some((x, y)) = mouse {
            state.mouse_x = x;
            state.mouse_y = y;
        }
    });
}

pub fn set_state_from_json(
    env: &mut Env,
    program_id: ProgramId,
    stack_id: StackKey,
    name: &str,
    json_val: &JsonValue,
) -> Result<(), String> {
    let names = env.state_key_names(program_id);
    let key = names
        .iter()
        .find(|(_, n)| n.as_str() == name)
        .map(|(k, _)| *k)
        .ok_or_else(|| format!("No state variable named '{}'", name))?;

    let val = json_to_value(json_val, env)?;
    env.set_state(stack_id, key, val);
    Ok(())
}

fn json_to_value(json: &JsonValue, env: &mut Env) -> Result<petal::value::Value, String> {
    match json {
        JsonValue::Null => Ok(petal::value::Value::Nil),
        JsonValue::Bool(b) => Ok(petal::value::Value::Bool(*b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(petal::value::Value::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(petal::value::Value::Float(f))
            } else {
                Err("Invalid number".to_string())
            }
        }
        JsonValue::String(s) => {
            let id = env.heap_mut().alloc_string(s.clone());
            Ok(petal::value::Value::String(id))
        }
        _ => Err("Only null, bool, number, and string values are supported".to_string()),
    }
}
