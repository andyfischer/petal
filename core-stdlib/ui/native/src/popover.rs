//! Which popover is open: at most one, decided once per frame.
//!
//! A menu used to keep its own open flag and resolve its own dismissal. The
//! press that opened it was still a live edge when the menu ran later in the
//! same frame, so every menu skipped its opening frame (`at:
//! frame_count()`), and a menu re-opened every frame never went live
//! (petal-design's toolbar, Phase 0 of `docs/proposals/input-dispatch.md`).
//! Two menus could be open at once, and Escape both closed the menu and
//! reached the app's own Escape.
//!
//! Now [`crate::hit::roll`] keeps the open popover's id, bound as
//! [`SYM_UI_POPOVER`]:
//!
//! 1. The requests the frame that just ran made apply in order:
//!    `popover_open(id)` opens `id` (closing any other), `popover_close(id)`
//!    closes `id` if it is the one open.
//! 2. Then, if one is open: Escape closes it, and so does a left or right
//!    press outside the rect it registered under its id last frame. The id
//!    it closed is bound as [`SYM_UI_DISMISSED`] for this frame.
//!
//! An open popover registers itself modal, so the press that dismisses it
//! reaches nothing underneath. While one is open, `shortcut(chord)` fires
//! only for that popover (`shortcut(chord, id)`), and an Escape that
//! dismissed one is not also the app's.
//!
//! The opening press can no longer close what it opened: the request it
//! makes applies on the next roll, after that press.

use petal::env::Env;
use petal::native_fn::{InputClasses, NativeEffects, NativeResult, PetalCxt};
use petal::value::Value;

use crate::hit::Target;
use crate::input::{SYM_BUTTONS_PRESSED, SYM_KEYS_PRESSED};
use crate::params::register;

/// Binding: the id of the open popover, `""` for none.
pub const SYM_UI_POPOVER: &str = "ui_popover";

/// Binding: the id this frame's roll closed (Escape, a press outside), `""`
/// when it closed nothing.
pub const SYM_UI_DISMISSED: &str = "ui_popover_dismissed";

/// Output buffer of `[open, id]` requests, drained by the next roll.
pub const UI_POPOVER_BUFFER: &str = "ui_popover_requests";

fn string_binding(env: &mut Env, name: &str) -> String {
    let sym = env.intern_symbol(name);
    match env.binding(sym) {
        Some(Value::String(s)) => env.heap().get_string(s).to_string(),
        _ => String::new(),
    }
}

fn set_string(env: &mut Env, name: &str, value: String, prev: &str) {
    let sym = env.intern_symbol(name);
    if value != prev || env.binding(sym).is_none() {
        let v = Value::String(env.heap_mut().alloc_string(value));
        env.set_binding(sym, v);
    }
}

fn list_has(env: &mut Env, name: &str, pred: impl Fn(&Value, &Env) -> bool) -> bool {
    let sym = env.intern_symbol(name);
    match env.binding(sym) {
        Some(Value::List(id)) => env.heap().get_list(id).iter().any(|v| pred(v, env)),
        _ => false,
    }
}

/// The open popover for the frame about to run. Called by
/// [`crate::hit::roll`] once the button and key bindings are bound.
pub(crate) fn roll_popover(env: &mut Env, registry: &[Target], px: f64, py: f64) {
    let prev = string_binding(env, SYM_UI_POPOVER);
    let prev_dismissed = string_binding(env, SYM_UI_DISMISSED);
    let mut open = prev.clone();
    let buf = env.intern_symbol(UI_POPOVER_BUFFER);
    for req in env.take_output_buffer(buf) {
        let Value::List(l) = req else { continue };
        let row = env.heap().get_list(l).to_vec();
        let (Some(Value::Bool(opening)), Some(Value::String(s))) = (row.first(), row.get(1)) else {
            continue;
        };
        let id = env.heap().get_string(*s).to_string();
        if *opening {
            open = id;
        } else if open == id {
            open.clear();
        }
    }
    let mut dismissed = String::new();
    if !open.is_empty() {
        let escape = list_has(env, SYM_KEYS_PRESSED, |v, env| {
            matches!(v, Value::String(s) if env.heap().get_string(*s) == "escape")
        });
        let pressed = list_has(env, SYM_BUTTONS_PRESSED, |v, _| {
            matches!(v, Value::Int(0) | Value::Int(1))
        });
        let inside = registry
            .iter()
            .any(|t| !t.modal && t.id.as_deref() == Some(open.as_str()) && t.contains(px, py));
        if escape || (pressed && !inside) {
            dismissed = std::mem::take(&mut open);
        }
    }
    set_string(env, SYM_UI_POPOVER, open, &prev);
    set_string(env, SYM_UI_DISMISSED, dismissed, &prev_dismissed);
}

/// The open popover for the frame about to run (`""`: none).
pub fn open_popover(env: &mut Env) -> String {
    string_binding(env, SYM_UI_POPOVER)
}

// ── Script side ───────────────────────────────────────────────────────────

pub(crate) fn register_popover(env: &mut Env) {
    const READS: NativeEffects = NativeEffects::probe(InputClasses::BINDINGS);
    const EMITS: NativeEffects = NativeEffects::PURE.with_emits();
    register(env, "ui_popover", native_ui_popover, READS);
    register(env, "ui_popover_dismissed", native_ui_popover_dismissed, READS);
    register(env, "ui_popover_request", native_ui_popover_request, EMITS);
}

fn cxt_string(state: &mut PetalCxt, name: &str) -> String {
    match state.binding_named(name) {
        Value::String(s) => state.heap().get_string(s).to_string(),
        _ => String::new(),
    }
}

/// `ui_popover()` → the open popover's id, `""` for none.
fn native_ui_popover(state: &mut PetalCxt) -> NativeResult {
    let s = cxt_string(state, SYM_UI_POPOVER);
    state.push_string(s);
    Ok(1)
}

/// `ui_popover_dismissed()` → the id this frame's roll closed, or `""`.
fn native_ui_popover_dismissed(state: &mut PetalCxt) -> NativeResult {
    let s = cxt_string(state, SYM_UI_DISMISSED);
    state.push_string(s);
    Ok(1)
}

/// `ui_popover_request(open, id)` — open `id` (`true`) or close it
/// (`false`) at the next roll.
fn native_ui_popover_request(state: &mut PetalCxt) -> NativeResult {
    let opening = state.get_bool(1)?;
    let id = match state.get_value(2)? {
        Value::String(s) => state.heap().get_string(s).to_string(),
        Value::Int(n) => n.to_string(),
        other => {
            return Err(format!(
                "popover_open() and popover_close() expect a string or int id, got {}",
                other.type_name()
            ));
        }
    };
    let id = Value::String(state.heap_mut().alloc_string(id));
    let row = Value::List(state.heap_mut().alloc_list(vec![Value::Bool(opening), id]));
    let sym = state.intern_symbol(UI_POPOVER_BUFFER);
    state.push_output(sym, row);
    state.push_nil();
    Ok(1)
}
