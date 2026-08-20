//! System natives: the console version, logging, presentation controls, the
//! two 8-button pads, and the cart browser.
//!
//! The pads are the one place this crate re-reads `petal-ui`'s input bindings
//! rather than adding state of its own. A pad button is a *name for a key*: the
//! SDL layer (including gamepad folding) has already normalized everything into
//! `keys_down` / `keys_pressed` / `keys_released`, so `pad_down(0, "a")` is a
//! lookup of "z" in that same list. Carts get console vocabulary; the host
//! keeps one input path.
//!
//! Presentation (`set_scale`, `set_crt`) and cart launching go through output
//! channels because they act on the window and the loaded program — things the
//! host owns and a native cannot reach.
//!
//! STUB: the cart list is not populated yet, so `cart_count()` is 0 and the
//! launcher has nothing to show. Everything else here is real.

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

use petal_ui::input::{SYM_KEYS_DOWN, SYM_KEYS_PRESSED, SYM_KEYS_RELEASED};

/// Bumped when the native contract changes incompatibly; carts can gate on it.
pub const NES_VERSION: i64 = 1;

/// Host->script cart list, as `[name, path]` pairs (same shape as petal-sdl's
/// example list).
pub const SYM_CARTS: &str = "carts";
/// Script->host: a cart path to load next frame.
pub const LAUNCH_CART_SIGNAL: &str = "launch_cart";
/// Script->host: window presentation requests.
pub const PRESENTATION_CHANNEL: &str = "nes_presentation";

pub fn register_system(env: &mut Env) {
    env.register_native("nes_version", native_nes_version);
    env.register_native("log", native_log);
    env.register_native("set_scale", native_set_scale);
    env.register_native("set_crt", native_set_crt);

    env.register_native("cart_count", native_cart_count);
    env.register_native("cart_name", native_cart_name);
    env.register_native("cart_path", native_cart_path);
    env.register_native("launch_cart", native_launch_cart);

    env.register_native("pad_down", native_pad_down);
    env.register_native("pad_pressed", native_pad_pressed);
    env.register_native("pad_released", native_pad_released);
}

// ── Host-side entry points ────────────────────────────────────────────────

pub struct CartEntry {
    pub name: String,
    pub path: String,
}

/// Publish the cart list the launcher browses.
pub fn bind_carts(env: &mut Env, carts: &[CartEntry]) {
    let mut pairs = Vec::with_capacity(carts.len());
    for c in carts {
        let name = Value::String(env.heap_mut().alloc_string(c.name.clone()));
        let path = Value::String(env.heap_mut().alloc_string(c.path.clone()));
        pairs.push(Value::List(env.heap_mut().alloc_list(vec![name, path])));
    }
    let list = Value::List(env.heap_mut().alloc_list(pairs));
    let sym = env.intern_symbol(SYM_CARTS);
    env.set_binding(sym, list);
}

/// The last cart path this frame asked to launch, if any.
pub fn take_pending_launch(env: &mut Env) -> Option<String> {
    let sym = env.intern_symbol(LAUNCH_CART_SIGNAL);
    let values = env.take_output_buffer(sym);
    values.into_iter().rev().find_map(|v| match v {
        Value::String(id) => Some(env.heap().get_string(id).to_string()),
        _ => None,
    })
}

/// A frame's window-presentation requests. Both fields are `None` when the
/// cart asked for nothing, so the host leaves the CLI's choice alone.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Presentation {
    pub scale: Option<u32>,
    pub crt: Option<bool>,
}

pub fn take_presentation(env: &mut Env) -> Presentation {
    let commands = crate::natives::take_commands(env, PRESENTATION_CHANNEL);
    let mut out = Presentation::default();
    for c in &commands {
        match c.tag.as_str() {
            "set_scale" => out.scale = Some(c.i64(0).clamp(1, 8) as u32),
            "set_crt" => out.crt = Some(c.bool(0)),
            _ => {}
        }
    }
    out
}

// ── Console + logging ─────────────────────────────────────────────────────

fn native_nes_version(cxt: &mut PetalCxt) -> NativeResult {
    cxt.push_int(NES_VERSION);
    Ok(1)
}

fn native_log(cxt: &mut PetalCxt) -> NativeResult {
    let msg = cxt.get_string(1).unwrap_or_default();
    cxt.print(msg);
    cxt.push_nil();
    Ok(1)
}

// ── Presentation ──────────────────────────────────────────────────────────

fn native_set_scale(cxt: &mut PetalCxt) -> NativeResult {
    let args = crate::natives::opt_nums(cxt, 1);
    crate::natives::emit(cxt, PRESENTATION_CHANNEL, "set_scale", args);
    cxt.push_nil();
    Ok(1)
}

fn native_set_crt(cxt: &mut PetalCxt) -> NativeResult {
    let args = crate::natives::opt_nums(cxt, 1);
    crate::natives::emit(cxt, PRESENTATION_CHANNEL, "set_crt", args);
    cxt.push_nil();
    Ok(1)
}

// ── Cart browser ──────────────────────────────────────────────────────────

fn cart_field(cxt: &mut PetalCxt, i: usize, field: usize) -> String {
    let list_id = match cxt.binding_named(SYM_CARTS) {
        Value::List(id) => id,
        _ => return String::new(),
    };
    let pair = match cxt.heap().get_list(list_id).get(i).copied() {
        Some(Value::List(pid)) => pid,
        _ => return String::new(),
    };
    match cxt.heap().get_list(pair).get(field).copied() {
        Some(Value::String(sid)) => cxt.heap().get_string(sid).to_string(),
        _ => String::new(),
    }
}

fn native_cart_count(cxt: &mut PetalCxt) -> NativeResult {
    let count = match cxt.binding_named(SYM_CARTS) {
        Value::List(id) => cxt.heap().get_list(id).len() as i64,
        _ => 0,
    };
    cxt.push_int(count);
    Ok(1)
}

fn native_cart_name(cxt: &mut PetalCxt) -> NativeResult {
    let i = cxt.get_int(1).unwrap_or(0).max(0) as usize;
    let name = cart_field(cxt, i, 0);
    cxt.push_string(name);
    Ok(1)
}

fn native_cart_path(cxt: &mut PetalCxt) -> NativeResult {
    let i = cxt.get_int(1).unwrap_or(0).max(0) as usize;
    let path = cart_field(cxt, i, 1);
    cxt.push_string(path);
    Ok(1)
}

fn native_launch_cart(cxt: &mut PetalCxt) -> NativeResult {
    let path = cxt.get_string(1)?;
    let pathv = Value::String(cxt.heap_mut().alloc_string(path));
    let sym = cxt.intern_symbol(LAUNCH_CART_SIGNAL);
    cxt.push_output(sym, pathv);
    cxt.push_nil();
    Ok(1)
}

// ── Pads ──────────────────────────────────────────────────────────────────

/// Keyboard keys each pad button answers to. More than one per button where a
/// second layout is conventional (arrows *and* WASD on pad 0); the pad is down
/// if any of them is. Names must match `petal_ui::input::KEY_NAMES`.
///
/// Pad 1 has no select/start: there are only so many comfortable keys on one
/// keyboard, and no two-player cart has needed them yet.
const PAD_KEYS: [[(&str, &[&str]); 8]; 2] = [
    [
        ("up", &["up", "w"]),
        ("down", &["down", "s"]),
        ("left", &["left", "a"]),
        ("right", &["right", "d"]),
        ("a", &["z"]),
        ("b", &["x"]),
        ("select", &["shift"]),
        ("start", &["return"]),
    ],
    [
        ("up", &["i"]),
        ("down", &["k"]),
        ("left", &["j"]),
        ("right", &["l"]),
        ("a", &["n"]),
        ("b", &["m"]),
        ("select", &[]),
        ("start", &[]),
    ],
];

fn keys_for(pad: i64, button: &str) -> &'static [&'static str] {
    let pad = match pad {
        0 | 1 => pad as usize,
        _ => return &[],
    };
    PAD_KEYS[pad]
        .iter()
        .find(|(name, _)| *name == button)
        .map(|(_, keys)| *keys)
        .unwrap_or(&[])
}

/// Is any of `keys` present in the string list bound at `binding`?
fn any_key_in(cxt: &mut PetalCxt, binding: &str, keys: &[&str]) -> bool {
    let list_id = match cxt.binding_named(binding) {
        Value::List(id) => id,
        _ => return false,
    };
    let heap = cxt.heap();
    heap.get_list(list_id).iter().any(|v| match v {
        Value::String(sid) => keys.contains(&heap.get_string(*sid)),
        _ => false,
    })
}

fn pad_query(cxt: &mut PetalCxt, binding: &str) -> NativeResult {
    let pad = cxt.get_int(1).unwrap_or(0);
    let button = cxt.get_string(2).unwrap_or_default();
    let keys = keys_for(pad, &button);
    let down = !keys.is_empty() && any_key_in(cxt, binding, keys);
    cxt.push_bool(down);
    Ok(1)
}

fn native_pad_down(cxt: &mut PetalCxt) -> NativeResult {
    pad_query(cxt, SYM_KEYS_DOWN)
}

fn native_pad_pressed(cxt: &mut PetalCxt) -> NativeResult {
    pad_query(cxt, SYM_KEYS_PRESSED)
}

fn native_pad_released(cxt: &mut PetalCxt) -> NativeResult {
    pad_query(cxt, SYM_KEYS_RELEASED)
}
