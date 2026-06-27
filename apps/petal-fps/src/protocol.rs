//! JSON-over-stdio agent protocol. Ported from petal-sdl/src/protocol.rs with
//! minor additions for 3D inspection (depth statistics, camera dump).

use std::io::{self, BufRead};
use std::sync::mpsc;
use std::thread;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

use crate::commands::{clear_draw_commands, take_draw_commands, DrawCommand};
use crate::input::InputState;
use crate::native_fns::{bind_frame_info, bind_input};

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
        #[serde(default)]
        mouse_delta: Option<MouseDelta>,
    },
    SetState {
        name: String,
        value: JsonValue,
    },
    Screenshot,
    DrawStats,
}

fn default_step_count() -> u32 {
    1
}

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

#[derive(Serialize, Default)]
pub struct DrawStats {
    pub total: usize,
    pub triangles: usize,
    pub lines: usize,
    pub rects: usize,
    pub circles: usize,
    pub texts: usize,
    /// Approximate Z range across all 3D vertices (min_z, max_z).
    pub z_min: Option<f32>,
    pub z_max: Option<f32>,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<DrawStats>,
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
            stats: None,
        }
    }
}

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
    clear_draw_commands(env);
    bind_input(env, input);
    env.run_speculative(stack_id)?;
    let commands = take_draw_commands(env);
    let output = env.take_output();
    Ok((commands, output))
}

pub fn run_one_frame(
    env: &mut Env,
    stack_id: StackKey,
    input: &InputState,
    frame_count: &mut i64,
) -> Result<i64, String> {
    clear_draw_commands(env);
    *frame_count += 1;
    bind_frame_info(env, 1.0 / 60.0, *frame_count);
    bind_input(env, input);
    env.reset_stack(stack_id)?;
    env.run(stack_id)?;
    Ok(*frame_count)
}

pub fn apply_input(
    input: &mut InputState,
    keys_down: &[String],
    mouse: Option<&MouseInput>,
    mouse_delta: Option<&MouseDelta>,
) {
    input.begin_frame();
    // Keys: treat "newly seen" as pressed for the frame.
    let old_keys = input.keys_down.clone();
    input.keys_down.clear();
    for key in keys_down {
        if !old_keys.contains(key) {
            input.keys_pressed.insert(key.clone());
        }
        input.keys_down.insert(key.clone());
    }
    if let Some(m) = mouse {
        let (x, y) = m.position();
        input.mouse_x = x;
        input.mouse_y = y;
        let old_buttons = input.mouse_buttons.clone();
        input.mouse_buttons.clear();
        for btn in m.buttons() {
            if !old_buttons.contains(btn) {
                input.mouse_buttons_pressed.insert(*btn);
            }
            input.mouse_buttons.insert(*btn);
        }
    }
    if let Some(d) = mouse_delta {
        input.mouse_dx = d.dx;
        input.mouse_dy = d.dy;
    }
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

pub fn compute_stats(commands: &[DrawCommand]) -> DrawStats {
    let mut s = DrawStats::default();
    s.total = commands.len();
    let mut min_z: Option<f32> = None;
    let mut max_z: Option<f32> = None;
    let mut track = |z: f32| {
        min_z = Some(min_z.map_or(z, |v| v.min(z)));
        max_z = Some(max_z.map_or(z, |v| v.max(z)));
    };
    for c in commands {
        match c {
            DrawCommand::Triangle3d { z1, z2, z3, .. } => {
                s.triangles += 1;
                track(*z1); track(*z2); track(*z3);
            }
            DrawCommand::Triangle3dShaded { z1, z2, z3, .. } => {
                s.triangles += 1;
                track(*z1); track(*z2); track(*z3);
            }
            DrawCommand::Line3d { z1, z2, .. } => {
                s.lines += 1;
                track(*z1); track(*z2);
            }
            DrawCommand::Line2d { .. } => s.lines += 1,
            DrawCommand::Rect2d { .. } => s.rects += 1,
            DrawCommand::Circle2d { .. } => s.circles += 1,
            DrawCommand::Text2d { .. } => s.texts += 1,
            _ => {}
        }
    }
    s.z_min = min_z;
    s.z_max = max_z;
    s
}
