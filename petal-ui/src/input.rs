//! Layer 0: the standard input contract.
//!
//! The host translates its native events (SDL, winit, a debug server…) into
//! [`InputEvent`]s and feeds them to an [`InputState`] as they arrive. Once
//! per script frame it calls [`InputState::begin_frame`], then
//! [`bind_input`], then runs the script. `InputState` owns the semantics:
//! *level* state (`mouse_x`, held keys/buttons) persists across frames, while
//! *edge* state (pressed/released/scroll/text/click-count) is visible to
//! exactly the one frame that follows the events.
//!
//! Button ids are standardized as 0 = left, 1 = right, 2 = middle — hosts
//! translate their platform's numbering at the event boundary.

use std::collections::HashSet;

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

use crate::UI_VERSION;

pub mod buttons {
    pub const LEFT: u8 = 0;
    pub const RIGHT: u8 = 1;
    pub const MIDDLE: u8 = 2;
}

/// Pointer distance (px) a left press must travel before it becomes a drag.
pub const DRAG_THRESHOLD: i32 = 3;
/// Maximum seconds between presses that chain into a double/triple click.
pub const MULTI_CLICK_SECONDS: f64 = 0.4;
/// Maximum pointer travel (px) between presses that chain into a multi-click.
pub const MULTI_CLICK_RADIUS: i32 = 4;

// ── Binding (uniform) names — the host↔script vocabulary ─────────────────

pub const SYM_MOUSE_X: &str = "mouse_x";
pub const SYM_MOUSE_Y: &str = "mouse_y";
pub const SYM_MOUSE_DX: &str = "mouse_dx";
pub const SYM_MOUSE_DY: &str = "mouse_dy";
pub const SYM_KEYS_DOWN: &str = "keys_down";
pub const SYM_KEYS_PRESSED: &str = "keys_pressed";
pub const SYM_KEYS_RELEASED: &str = "keys_released";
pub const SYM_BUTTONS_DOWN: &str = "mouse_buttons_down";
pub const SYM_BUTTONS_PRESSED: &str = "mouse_buttons_pressed";
pub const SYM_BUTTONS_RELEASED: &str = "mouse_buttons_released";
pub const SYM_SCROLL_X: &str = "scroll_x";
pub const SYM_SCROLL_Y: &str = "scroll_y";
pub const SYM_MODIFIERS: &str = "modifiers";
pub const SYM_DRAG_ACTIVE: &str = "drag_active";
pub const SYM_DRAG_START_X: &str = "drag_start_x";
pub const SYM_DRAG_START_Y: &str = "drag_start_y";
pub const SYM_CLICK_COUNT: &str = "click_count";
pub const SYM_TEXT_INPUT: &str = "text_input";
pub const SYM_DT: &str = "dt";
pub const SYM_FRAME_COUNT: &str = "frame_count";
pub const SYM_SCREEN_WIDTH: &str = "screen_width";
pub const SYM_SCREEN_HEIGHT: &str = "screen_height";

/// Output channel: pointer grab/release requests emitted by `grab_mouse()` /
/// `release_mouse()`. Hosts that support pointer lock (native windows) honor
/// it via [`take_mouse_grab`]; hosts that don't may leave it undrained.
pub const MOUSE_GRAB_SIGNAL: &str = "mouse_grab";

const MOD_SHIFT: i64 = 1;
const MOD_CTRL: i64 = 2;
const MOD_ALT: i64 = 4;
const MOD_CMD: i64 = 8;

/// Canonical key names. Hosts map their platform's key events onto these
/// spellings so scripts are portable across embedders.
pub const KEY_NAMES: &[&str] = &[
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "return",
    "escape",
    "backspace",
    "tab",
    "space",
    "up",
    "down",
    "left",
    "right",
    "pageup",
    "pagedown",
    "home",
    "end",
    "delete",
    "insert",
    "shift",
    "ctrl",
    "alt",
    "cmd",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "minus",
    "equals",
    "comma",
    "period",
    "slash",
    "backslash",
    "semicolon",
    "quote",
    "backquote",
    "leftbracket",
    "rightbracket",
];

pub fn is_canonical_key(name: &str) -> bool {
    KEY_NAMES.contains(&name)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub cmd: bool,
}

impl Modifiers {
    fn to_bits(self) -> i64 {
        (self.shift as i64) * MOD_SHIFT
            + (self.ctrl as i64) * MOD_CTRL
            + (self.alt as i64) * MOD_ALT
            + (self.cmd as i64) * MOD_CMD
    }
}

/// A normalized host input event. Key names must be canonical (see
/// [`KEY_NAMES`]); button ids use the [`buttons`] numbering.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    MouseMove {
        x: i32,
        y: i32,
    },
    /// Raw relative pointer motion, independent of the absolute position.
    /// Deltas accumulate within a frame and are read via `mouse_dx()` /
    /// `mouse_dy()`. This is what makes mouselook work while the pointer is
    /// grabbed/locked (absolute position stops moving, but deltas keep coming).
    MouseRelative {
        dx: i32,
        dy: i32,
    },
    MouseDown {
        button: u8,
    },
    MouseUp {
        button: u8,
    },
    /// Wheel/trackpad scroll in lines. Fractional deltas accumulate across
    /// frames with carry, so slow trackpad scrolling still moves.
    Scroll {
        dx: f64,
        dy: f64,
    },
    /// A key went down. Feeding OS auto-repeat events here is deliberate:
    /// each one re-fires the `key_pressed` edge (held-key list navigation)
    /// while `key_down` is unaffected.
    KeyDown {
        key: String,
    },
    KeyUp {
        key: String,
    },
    /// Typed text (post-layout, pre-IME). Read by `text_input()`.
    Text {
        text: String,
    },
    /// The current modifier chord. Hosts that deliver modifiers as ordinary
    /// keys ("shift" down/up) may skip this and let scripts use `key_down`.
    Modifiers(Modifiers),
}

/// Accumulates [`InputEvent`]s and derives the per-frame snapshot scripts
/// read through the standard natives.
///
/// Time is advanced only by [`begin_frame`](Self::begin_frame)'s `dt`, so
/// multi-click detection is deterministic and testable — there is no wall
/// clock inside.
#[derive(Default)]
pub struct InputState {
    // Level state (persists).
    pub mouse_x: i32,
    pub mouse_y: i32,
    keys_down: HashSet<String>,
    buttons_down: HashSet<u8>,
    mods: Modifiers,
    drag_press: Option<(i32, i32)>,
    drag_active: bool,

    // Pending edges (since the last begin_frame).
    pending_keys_pressed: HashSet<String>,
    pending_keys_released: HashSet<String>,
    pending_buttons_pressed: HashSet<u8>,
    pending_buttons_released: HashSet<u8>,
    pending_scroll: (f64, f64),
    pending_mouse_dx: i32,
    pending_mouse_dy: i32,
    pending_text: String,
    pending_click_count: i64,

    // The current frame's edges (what bind_input publishes).
    frame_keys_pressed: HashSet<String>,
    frame_keys_released: HashSet<String>,
    frame_buttons_pressed: HashSet<u8>,
    frame_buttons_released: HashSet<u8>,
    frame_scroll: (i64, i64),
    frame_mouse_dx: i32,
    frame_mouse_dy: i32,
    frame_text: String,
    frame_click_count: i64,

    // Multi-click chain: (time, x, y, count) of the last left press.
    last_click: Option<(f64, i32, i32, i64)>,
    now: f64,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one host event. Level state updates immediately; edges
    /// accumulate until the next [`begin_frame`](Self::begin_frame).
    pub fn event(&mut self, ev: InputEvent) {
        match ev {
            InputEvent::MouseMove { x, y } => {
                self.mouse_x = x;
                self.mouse_y = y;
                if let Some((sx, sy)) = self.drag_press {
                    if !self.drag_active
                        && ((x - sx).abs() >= DRAG_THRESHOLD || (y - sy).abs() >= DRAG_THRESHOLD)
                    {
                        self.drag_active = true;
                    }
                }
            }
            InputEvent::MouseDown { button } => {
                self.buttons_down.insert(button);
                self.pending_buttons_pressed.insert(button);
                if button == buttons::LEFT {
                    self.drag_press = Some((self.mouse_x, self.mouse_y));
                    self.drag_active = false;
                    let count = match self.last_click {
                        Some((t, x, y, c))
                            if self.now - t <= MULTI_CLICK_SECONDS
                                && (self.mouse_x - x).abs() <= MULTI_CLICK_RADIUS
                                && (self.mouse_y - y).abs() <= MULTI_CLICK_RADIUS =>
                        {
                            c + 1
                        }
                        _ => 1,
                    };
                    self.last_click = Some((self.now, self.mouse_x, self.mouse_y, count));
                    self.pending_click_count = count;
                }
            }
            InputEvent::MouseUp { button } => {
                self.buttons_down.remove(&button);
                self.pending_buttons_released.insert(button);
                if button == buttons::LEFT {
                    self.drag_press = None;
                    self.drag_active = false;
                }
            }
            InputEvent::Scroll { dx, dy } => {
                self.pending_scroll.0 += dx;
                self.pending_scroll.1 += dy;
            }
            InputEvent::MouseRelative { dx, dy } => {
                self.pending_mouse_dx += dx;
                self.pending_mouse_dy += dy;
            }
            InputEvent::KeyDown { key } => {
                self.pending_keys_pressed.insert(key.clone());
                self.keys_down.insert(key);
            }
            InputEvent::KeyUp { key } => {
                self.keys_down.remove(&key);
                self.pending_keys_released.insert(key);
            }
            InputEvent::Text { text } => self.pending_text.push_str(&text),
            InputEvent::Modifiers(m) => self.mods = m,
        }
    }

    /// Start a script frame: advance the clock by `dt` seconds and promote
    /// pending edges into the frame snapshot (clearing the previous one).
    /// Every edge is delivered to exactly one frame.
    pub fn begin_frame(&mut self, dt: f64) {
        self.now += dt;
        self.frame_keys_pressed = std::mem::take(&mut self.pending_keys_pressed);
        self.frame_keys_released = std::mem::take(&mut self.pending_keys_released);
        self.frame_buttons_pressed = std::mem::take(&mut self.pending_buttons_pressed);
        self.frame_buttons_released = std::mem::take(&mut self.pending_buttons_released);
        // Integer lines this frame; the fractional remainder carries over.
        let wx = self.pending_scroll.0.trunc();
        let wy = self.pending_scroll.1.trunc();
        self.pending_scroll.0 -= wx;
        self.pending_scroll.1 -= wy;
        self.frame_scroll = (wx as i64, wy as i64);
        self.frame_mouse_dx = std::mem::take(&mut self.pending_mouse_dx);
        self.frame_mouse_dy = std::mem::take(&mut self.pending_mouse_dy);
        self.frame_text = std::mem::take(&mut self.pending_text);
        self.frame_click_count = std::mem::take(&mut self.pending_click_count);
    }

    /// Replace the level state with an absolute snapshot — for
    /// protocol-driven hosts (agent/debug servers) whose messages say "these
    /// keys and buttons are down now". Edges are derived by diffing against
    /// the current level state and fed through [`event`](Self::event), so
    /// drag and click-count semantics still apply.
    pub fn apply_absolute(
        &mut self,
        keys_down: &[String],
        buttons_down: Option<&[u8]>,
        mouse: Option<(i32, i32)>,
    ) {
        if let Some((x, y)) = mouse {
            self.event(InputEvent::MouseMove { x, y });
        }
        let new_keys: HashSet<String> = keys_down.iter().cloned().collect();
        for k in self
            .keys_down
            .difference(&new_keys)
            .cloned()
            .collect::<Vec<_>>()
        {
            self.event(InputEvent::KeyUp { key: k });
        }
        for k in new_keys
            .difference(&self.keys_down)
            .cloned()
            .collect::<Vec<_>>()
        {
            self.event(InputEvent::KeyDown { key: k });
        }
        // `None` leaves the held buttons untouched (a keys-only message);
        // `Some(&[])` explicitly releases everything.
        if let Some(buttons) = buttons_down {
            let new_buttons: HashSet<u8> = buttons.iter().copied().collect();
            for b in self
                .buttons_down
                .difference(&new_buttons)
                .copied()
                .collect::<Vec<_>>()
            {
                self.event(InputEvent::MouseUp { button: b });
            }
            for b in new_buttons
                .difference(&self.buttons_down)
                .copied()
                .collect::<Vec<_>>()
            {
                self.event(InputEvent::MouseDown { button: b });
            }
        }
    }

    /// Queue raw relative pointer motion (e.g. from an SDL `MouseMotion`
    /// xrel/yrel or a protocol `mouse_delta`). Accumulates like the other edge
    /// state and is delivered to the next frame promoted by
    /// [`begin_frame`](Self::begin_frame), read via `mouse_dx()`/`mouse_dy()`.
    pub fn move_relative(&mut self, dx: i32, dy: i32) {
        self.event(InputEvent::MouseRelative { dx, dy });
    }

    /// Queue typed text (e.g. from a debug-protocol `input` command or an SDL
    /// text event). Like the edge state, it is delivered to the next frame
    /// promoted by [`begin_frame`](Self::begin_frame) and read via
    /// `text_input()`.
    pub fn type_text(&mut self, text: &str) {
        self.event(InputEvent::Text {
            text: text.to_string(),
        });
    }

    /// The typed text delivered to the current frame (what `text_input()`
    /// reads). Empty unless [`type_text`](Self::type_text) / an
    /// [`InputEvent::Text`] arrived before the last `begin_frame`.
    pub fn frame_text(&self) -> &str {
        &self.frame_text
    }

    pub fn is_key_down(&self, key: &str) -> bool {
        self.keys_down.contains(key)
    }

    pub fn is_button_down(&self, button: u8) -> bool {
        self.buttons_down.contains(&button)
    }

    pub fn drag_active(&self) -> bool {
        self.drag_active
    }

    pub fn drag_start(&self) -> Option<(i32, i32)> {
        self.drag_press
    }
}

// ── Host-side: bind the snapshot into the Env before a run ───────────────

/// Bind the per-frame input uniforms. Call after
/// [`InputState::begin_frame`], before running the script.
pub fn bind_input(env: &mut Env, input: &InputState) {
    bind_str_list(env, SYM_KEYS_DOWN, input.keys_down.iter());
    bind_str_list(env, SYM_KEYS_PRESSED, input.frame_keys_pressed.iter());
    bind_str_list(env, SYM_KEYS_RELEASED, input.frame_keys_released.iter());
    bind_int_list(
        env,
        SYM_BUTTONS_DOWN,
        input.buttons_down.iter().map(|b| *b as i64),
    );
    bind_int_list(
        env,
        SYM_BUTTONS_PRESSED,
        input.frame_buttons_pressed.iter().map(|b| *b as i64),
    );
    bind_int_list(
        env,
        SYM_BUTTONS_RELEASED,
        input.frame_buttons_released.iter().map(|b| *b as i64),
    );
    bind_int(env, SYM_MOUSE_X, input.mouse_x as i64);
    bind_int(env, SYM_MOUSE_Y, input.mouse_y as i64);
    bind_int(env, SYM_MOUSE_DX, input.frame_mouse_dx as i64);
    bind_int(env, SYM_MOUSE_DY, input.frame_mouse_dy as i64);
    bind_int(env, SYM_SCROLL_X, input.frame_scroll.0);
    bind_int(env, SYM_SCROLL_Y, input.frame_scroll.1);
    bind_int(env, SYM_MODIFIERS, input.mods.to_bits());
    bind_int(env, SYM_DRAG_ACTIVE, input.drag_active as i64);
    let (sx, sy) = input.drag_press.unwrap_or((input.mouse_x, input.mouse_y));
    bind_int(env, SYM_DRAG_START_X, sx as i64);
    bind_int(env, SYM_DRAG_START_Y, sy as i64);
    bind_int(env, SYM_CLICK_COUNT, input.frame_click_count);
    let text = Value::String(env.heap_mut().alloc_string(input.frame_text.clone()));
    let s = env.intern_symbol(SYM_TEXT_INPUT);
    env.set_binding(s, text);
}

/// Bind the per-frame dt (seconds) + frame_count uniforms.
pub fn bind_frame_info(env: &mut Env, dt: f64, frame_count: i64) {
    let s = env.intern_symbol(SYM_DT);
    env.set_binding(s, Value::Float(dt));
    bind_int(env, SYM_FRAME_COUNT, frame_count);
}

/// Bind the drawable size in logical pixels (set on init/resize; persists).
pub fn bind_dimensions(env: &mut Env, width: i32, height: i32) {
    bind_int(env, SYM_SCREEN_WIDTH, width as i64);
    bind_int(env, SYM_SCREEN_HEIGHT, height as i64);
}

/// Read back the bound drawable size (e.g. for screenshot sizing).
pub fn dimensions(env: &mut Env) -> (u32, u32) {
    let read = |env: &mut Env, name: &str| -> i64 {
        let s = env.intern_symbol(name);
        match env.binding(s) {
            Some(Value::Int(n)) => n,
            Some(Value::Float(f)) => f as i64,
            _ => 0,
        }
    };
    (
        read(env, SYM_SCREEN_WIDTH).max(0) as u32,
        read(env, SYM_SCREEN_HEIGHT).max(0) as u32,
    )
}

/// Drain the pointer grab/release request channel, returning the last request
/// the script made this frame — `Some(true)` to grab/lock the pointer,
/// `Some(false)` to release, `None` if it made no request. Hosts that support
/// pointer lock call this after running a frame and reconcile it with their
/// window; hosts that don't can ignore it (the buffer is harmless if undrained,
/// but draining keeps it from growing).
pub fn take_mouse_grab(env: &mut Env) -> Option<bool> {
    let s = env.intern_symbol(MOUSE_GRAB_SIGNAL);
    let vals = env.take_output_buffer(s);
    vals.into_iter().rev().find_map(|v| match v {
        Value::Bool(b) => Some(b),
        _ => None,
    })
}

fn bind_int(env: &mut Env, name: &str, v: i64) {
    let s = env.intern_symbol(name);
    env.set_binding(s, Value::Int(v));
}

fn bind_str_list<'a>(env: &mut Env, name: &str, items: impl Iterator<Item = &'a String>) {
    let vals: Vec<Value> = items
        .map(|k| Value::String(env.heap_mut().alloc_string(k.clone())))
        .collect();
    let list = Value::List(env.heap_mut().alloc_list(vals));
    let s = env.intern_symbol(name);
    env.set_binding(s, list);
}

fn bind_int_list(env: &mut Env, name: &str, items: impl Iterator<Item = i64>) {
    let vals: Vec<Value> = items.map(Value::Int).collect();
    let list = Value::List(env.heap_mut().alloc_list(vals));
    let s = env.intern_symbol(name);
    env.set_binding(s, list);
}

// ── Script-side: the standard input natives ──────────────────────────────

/// Register the standard input/timing natives. The host must also call
/// [`bind_input`] / [`bind_frame_info`] / [`bind_dimensions`] each frame —
/// the natives only read those bindings.
pub fn register_input(env: &mut Env) {
    env.register_native("mouse_x", native_mouse_x);
    env.register_native("mouse_y", native_mouse_y);
    env.register_native("mouse_dx", native_mouse_dx);
    env.register_native("mouse_dy", native_mouse_dy);
    env.register_native("mouse_down", native_mouse_down);
    env.register_native("mouse_pressed", native_mouse_pressed);
    env.register_native("mouse_released", native_mouse_released);
    env.register_native("scroll_x", native_scroll_x);
    env.register_native("scroll_y", native_scroll_y);
    env.register_native("key_down", native_key_down);
    env.register_native("key_pressed", native_key_pressed);
    env.register_native("key_released", native_key_released);
    env.register_native("mod_shift", native_mod_shift);
    env.register_native("mod_ctrl", native_mod_ctrl);
    env.register_native("mod_alt", native_mod_alt);
    env.register_native("mod_cmd", native_mod_cmd);
    env.register_native("drag_active", native_drag_active);
    env.register_native("drag_start_x", native_drag_start_x);
    env.register_native("drag_start_y", native_drag_start_y);
    env.register_native("click_count", native_click_count);
    env.register_native("text_input", native_text_input);
    env.register_native("grab_mouse", native_grab_mouse);
    env.register_native("release_mouse", native_release_mouse);
    env.register_native("dt", native_dt);
    env.register_native("frame_count", native_frame_count);
    env.register_native("screen_width", native_screen_width);
    env.register_native("screen_height", native_screen_height);
    env.register_native("ui_version", native_ui_version);
}

fn binding_int(state: &mut PetalCxt, name: &str) -> i64 {
    match state.binding_named(name) {
        Value::Int(n) => n,
        Value::Float(f) => f as i64,
        _ => 0,
    }
}

fn binding_float(state: &mut PetalCxt, name: &str) -> f64 {
    match state.binding_named(name) {
        Value::Float(f) => f,
        Value::Int(n) => n as f64,
        _ => 0.0,
    }
}

fn binding_has_str(state: &mut PetalCxt, name: &str, needle: &str) -> bool {
    let list_id = match state.binding_named(name) {
        Value::List(id) => id,
        _ => return false,
    };
    state
        .heap()
        .get_list(list_id)
        .iter()
        .any(|v| matches!(v, Value::String(s) if state.heap().get_string(*s) == needle))
}

fn binding_has_int(state: &mut PetalCxt, name: &str, needle: i64) -> bool {
    let list_id = match state.binding_named(name) {
        Value::List(id) => id,
        _ => return false,
    };
    state
        .heap()
        .get_list(list_id)
        .iter()
        .any(|v| matches!(v, Value::Int(n) if *n == needle))
}

fn push_binding_int(state: &mut PetalCxt, name: &str) -> NativeResult {
    let v = binding_int(state, name);
    state.push_int(v);
    Ok(1)
}

fn push_binding_bool(state: &mut PetalCxt, name: &str) -> NativeResult {
    let v = binding_int(state, name) != 0;
    state.push_bool(v);
    Ok(1)
}

fn push_mod(state: &mut PetalCxt, bit: i64) -> NativeResult {
    let mods = binding_int(state, SYM_MODIFIERS);
    state.push_bool(mods & bit != 0);
    Ok(1)
}

fn native_mouse_x(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_MOUSE_X)
}

fn native_mouse_y(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_MOUSE_Y)
}

fn native_mouse_dx(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_MOUSE_DX)
}

fn native_mouse_dy(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_MOUSE_DY)
}

fn native_mouse_down(state: &mut PetalCxt) -> NativeResult {
    let b = state.get_int(1)?;
    let down = binding_has_int(state, SYM_BUTTONS_DOWN, b);
    state.push_bool(down);
    Ok(1)
}

fn native_mouse_pressed(state: &mut PetalCxt) -> NativeResult {
    let b = state.get_int(1)?;
    let v = binding_has_int(state, SYM_BUTTONS_PRESSED, b);
    state.push_bool(v);
    Ok(1)
}

fn native_mouse_released(state: &mut PetalCxt) -> NativeResult {
    let b = state.get_int(1)?;
    let v = binding_has_int(state, SYM_BUTTONS_RELEASED, b);
    state.push_bool(v);
    Ok(1)
}

fn native_scroll_x(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_SCROLL_X)
}

fn native_scroll_y(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_SCROLL_Y)
}

fn native_key_down(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    let v = binding_has_str(state, SYM_KEYS_DOWN, &name);
    state.push_bool(v);
    Ok(1)
}

fn native_key_pressed(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    let v = binding_has_str(state, SYM_KEYS_PRESSED, &name);
    state.push_bool(v);
    Ok(1)
}

fn native_key_released(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    let v = binding_has_str(state, SYM_KEYS_RELEASED, &name);
    state.push_bool(v);
    Ok(1)
}

fn native_mod_shift(state: &mut PetalCxt) -> NativeResult {
    push_mod(state, MOD_SHIFT)
}

fn native_mod_ctrl(state: &mut PetalCxt) -> NativeResult {
    push_mod(state, MOD_CTRL)
}

fn native_mod_alt(state: &mut PetalCxt) -> NativeResult {
    push_mod(state, MOD_ALT)
}

fn native_mod_cmd(state: &mut PetalCxt) -> NativeResult {
    push_mod(state, MOD_CMD)
}

fn native_drag_active(state: &mut PetalCxt) -> NativeResult {
    push_binding_bool(state, SYM_DRAG_ACTIVE)
}

fn native_drag_start_x(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_DRAG_START_X)
}

fn native_drag_start_y(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_DRAG_START_Y)
}

fn native_click_count(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_CLICK_COUNT)
}

fn native_text_input(state: &mut PetalCxt) -> NativeResult {
    let text = match state.binding_named(SYM_TEXT_INPUT) {
        Value::String(id) => state.heap().get_string(id).to_string(),
        _ => String::new(),
    };
    state.push_string(text);
    Ok(1)
}

fn native_grab_mouse(state: &mut PetalCxt) -> NativeResult {
    let sym = state.intern_symbol(MOUSE_GRAB_SIGNAL);
    state.push_output(sym, Value::Bool(true));
    state.push_nil();
    Ok(1)
}

fn native_release_mouse(state: &mut PetalCxt) -> NativeResult {
    let sym = state.intern_symbol(MOUSE_GRAB_SIGNAL);
    state.push_output(sym, Value::Bool(false));
    state.push_nil();
    Ok(1)
}

fn native_dt(state: &mut PetalCxt) -> NativeResult {
    let v = binding_float(state, SYM_DT);
    state.push_float(v);
    Ok(1)
}

fn native_frame_count(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_FRAME_COUNT)
}

fn native_screen_width(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_SCREEN_WIDTH)
}

fn native_screen_height(state: &mut PetalCxt) -> NativeResult {
    push_binding_int(state, SYM_SCREEN_HEIGHT)
}

fn native_ui_version(state: &mut PetalCxt) -> NativeResult {
    state.push_int(UI_VERSION);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(input: &mut InputState, key: &str) {
        input.event(InputEvent::KeyDown {
            key: key.to_string(),
        });
    }

    #[test]
    fn edges_are_delivered_to_exactly_one_frame() {
        let mut input = InputState::new();
        press(&mut input, "j");
        input.begin_frame(0.016);
        assert!(input.frame_keys_pressed.contains("j"));
        assert!(input.is_key_down("j"));
        input.begin_frame(0.016);
        assert!(!input.frame_keys_pressed.contains("j"));
        assert!(input.is_key_down("j"), "level state persists");
        input.event(InputEvent::KeyUp {
            key: "j".to_string(),
        });
        input.begin_frame(0.016);
        assert!(input.frame_keys_released.contains("j"));
        assert!(!input.is_key_down("j"));
    }

    #[test]
    fn relative_motion_accumulates_then_resets_each_frame() {
        let mut input = InputState::new();
        input.move_relative(3, -2);
        input.move_relative(1, 5);
        input.begin_frame(0.016);
        assert_eq!((input.frame_mouse_dx, input.frame_mouse_dy), (4, 3));
        // No motion this frame → deltas fall back to zero (edge, not level).
        input.begin_frame(0.016);
        assert_eq!((input.frame_mouse_dx, input.frame_mouse_dy), (0, 0));
    }

    #[test]
    fn scroll_fraction_carries_across_frames() {
        let mut input = InputState::new();
        for _ in 0..3 {
            input.event(InputEvent::Scroll { dx: 0.0, dy: 0.4 });
            input.begin_frame(0.016);
        }
        // 0.4 + 0.4 + 0.4 = 1.2 → one whole line by the third frame.
        assert_eq!(input.frame_scroll.1, 1);
    }

    #[test]
    fn drag_requires_movement_past_threshold() {
        let mut input = InputState::new();
        input.event(InputEvent::MouseMove { x: 100, y: 100 });
        input.event(InputEvent::MouseDown {
            button: buttons::LEFT,
        });
        input.begin_frame(0.016);
        assert!(!input.drag_active());
        input.event(InputEvent::MouseMove { x: 102, y: 100 });
        assert!(!input.drag_active(), "2px is under the threshold");
        input.event(InputEvent::MouseMove { x: 104, y: 100 });
        assert!(input.drag_active());
        assert_eq!(input.drag_start(), Some((100, 100)));
        input.event(InputEvent::MouseUp {
            button: buttons::LEFT,
        });
        assert!(!input.drag_active());
    }

    #[test]
    fn click_count_chains_within_window_and_radius() {
        let mut input = InputState::new();
        input.event(InputEvent::MouseMove { x: 50, y: 50 });
        input.event(InputEvent::MouseDown {
            button: buttons::LEFT,
        });
        input.event(InputEvent::MouseUp {
            button: buttons::LEFT,
        });
        input.begin_frame(0.1);
        assert_eq!(input.frame_click_count, 1);

        input.event(InputEvent::MouseDown {
            button: buttons::LEFT,
        });
        input.event(InputEvent::MouseUp {
            button: buttons::LEFT,
        });
        input.begin_frame(0.1);
        assert_eq!(input.frame_click_count, 2, "double click");

        // Too far away: chain resets.
        input.event(InputEvent::MouseMove { x: 90, y: 50 });
        input.event(InputEvent::MouseDown {
            button: buttons::LEFT,
        });
        input.begin_frame(0.1);
        assert_eq!(input.frame_click_count, 1);

        // Too late: chain resets.
        input.event(InputEvent::MouseUp {
            button: buttons::LEFT,
        });
        input.begin_frame(1.0);
        input.event(InputEvent::MouseDown {
            button: buttons::LEFT,
        });
        input.begin_frame(0.016);
        assert_eq!(input.frame_click_count, 1);
    }
}
