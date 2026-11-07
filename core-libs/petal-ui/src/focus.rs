//! Who has the keyboard, and which keys are theirs.
//!
//! Focus used to be a value every app threaded by hand (`fc` in, `.focus`
//! out of every field and panel). A panel that forgot to pass it back left
//! shortcuts firing while the user typed, and an app's "is anyone typing"
//! flag, computed before its panels ran, was a frame behind its own fields.
//! Focus is now the runtime's. [`crate::hit::roll`] works it out once per
//! frame, before the script runs, and binds it as [`SYM_UI_FOCUS`]:
//!
//! 1. The last `focus_request(id)` the frame that just ran made applies
//!    (`""` or `nil` clears). The prelude's `focus_set` and `focus_clear`
//!    make one too, so code that still threads `fc` keeps the runtime's
//!    focus in step.
//! 2. Then a left press, judged against last frame's registry like every
//!    press (`crate::hit`, "One owner"). If the rect in front under it takes
//!    focus (registered `{text: true}` or `{focus: true}` with an id), it
//!    gets the keyboard on this very frame. Otherwise the focus is cleared,
//!    unless the press landed on a layer above the focused widget's: a menu
//!    opened from a field leaves the field focused.
//!
//! # Claims
//!
//! While a widget has the keyboard, some keys are its own, and a
//! `shortcut(chord)` does not fire on them. They are bound as
//! [`SYM_UI_CLAIMS`]:
//!
//! - a rect that takes text claims every key pressed without Cmd/Ctrl
//!   (letters, arrows, Backspace, Escape, Tab…) and the editing chords in
//!   [`TEXT_CHORDS`];
//! - any rect claims the chords it listed (`{keys: ["down", "up"]}`).
//!
//! So Cmd+S still saves while a field has the keyboard, and Cmd+Z undoes
//! the field's typing instead of the document. `shortcut(chord, id)` asks
//! for a chord on behalf of `id` instead: it fires only while `id` has the
//! keyboard, whatever it claims.
//!
//! # Chords
//!
//! A chord is modifiers and one key name, joined with `+`: `"cmd+shift+z"`,
//! `"escape"`, `"alt+left"`. `cmd` is the primary modifier, Cmd or Ctrl, as
//! the text field already treats them. A chord matches only its exact
//! modifiers: `"cmd+z"` does not fire with Shift held.
//!
//! Everything here is a binding read, a probe over bindings, or an emit, so
//! the frame gate and the memo stay exact.

use petal::env::Env;
use petal::native_fn::{InputClasses, NativeEffects, NativeResult, PetalCxt};
use petal::value::Value;

use crate::hit::{Target, front, press_rows};
use crate::input::{SYM_BUTTONS_PRESSED, SYM_KEYS_PRESSED, SYM_MODIFIERS};
use crate::params::register;

/// Binding: the id that has the keyboard for the frame about to run, a
/// string, `""` for nobody.
pub const SYM_UI_FOCUS: &str = "ui_focus";

/// Binding: what the focused widget claims, a list of canonical chords,
/// with `"text"` for a widget that takes text. See "Claims".
pub const SYM_UI_CLAIMS: &str = "ui_focus_claims";

/// Output buffer `focus_request(id)` emits into, drained by the next roll.
pub const UI_FOCUS_BUFFER: &str = "ui_focus_requests";

/// The chords a widget that takes text claims besides every key without
/// Cmd/Ctrl: the ones the prelude's text field and text area act on.
pub const TEXT_CHORDS: &[&str] = &[
    "cmd+a",
    "cmd+c",
    "cmd+x",
    "cmd+v",
    "cmd+z",
    "cmd+shift+z",
    "cmd+y",
    "cmd+left",
    "cmd+right",
    "cmd+up",
    "cmd+down",
    "cmd+shift+left",
    "cmd+shift+right",
    "cmd+shift+up",
    "cmd+shift+down",
    "cmd+backspace",
    "cmd+delete",
];

/// A parsed chord.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Chord<'a> {
    cmd: bool,
    alt: bool,
    shift: bool,
    key: &'a str,
}

fn parse(s: &str) -> Option<Chord<'_>> {
    let mut c = Chord { cmd: false, alt: false, shift: false, key: "" };
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let (key, mods) = parts.split_last()?;
    if key.is_empty() {
        return None;
    }
    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "cmd" | "ctrl" | "control" | "meta" => c.cmd = true,
            "alt" | "option" | "opt" => c.alt = true,
            "shift" => c.shift = true,
            _ => return None,
        }
    }
    c.key = key;
    Some(c)
}

fn spell(c: &Chord) -> String {
    let mut s = String::new();
    if c.cmd {
        s.push_str("cmd+");
    }
    if c.alt {
        s.push_str("alt+");
    }
    if c.shift {
        s.push_str("shift+");
    }
    s.push_str(&c.key.to_ascii_lowercase());
    s
}

/// `"Shift+Cmd+Z"` → `"cmd+shift+z"`; `None` if it is not a chord. The
/// modifiers are spelled `cmd`, `alt`, `shift`, in that order.
pub fn canonical_chord(s: &str) -> Option<String> {
    parse(s).map(|c| spell(&c))
}

/// Whether a widget claiming `claims` has `chord` (canonical) to itself.
pub fn claimed(claims: &[String], chord: &str) -> bool {
    let Some(c) = parse(chord) else { return false };
    let text = claims.iter().any(|k| k == "text");
    (text && (!c.cmd || TEXT_CHORDS.contains(&chord))) || claims.iter().any(|k| k == chord)
}

fn string_binding(env: &mut Env, name: &str) -> String {
    let sym = env.intern_symbol(name);
    match env.binding(sym) {
        Some(Value::String(s)) => env.heap().get_string(s).to_string(),
        _ => String::new(),
    }
}

fn strings_binding(env: &mut Env, name: &str) -> Option<Vec<String>> {
    let sym = env.intern_symbol(name);
    match env.binding(sym) {
        Some(Value::List(id)) => Some(
            env.heap()
                .get_list(id)
                .iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(env.heap().get_string(*s).to_string()),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

fn pressed_left(env: &mut Env) -> bool {
    let sym = env.intern_symbol(SYM_BUTTONS_PRESSED);
    match env.binding(sym) {
        Some(Value::List(id)) => env
            .heap()
            .get_list(id)
            .iter()
            .any(|v| matches!(v, Value::Int(0))),
        _ => false,
    }
}

/// The row registered under `id` (the last one), not a modal marker.
fn row_of<'a>(registry: &'a [Target], id: &str) -> Option<&'a Target> {
    registry
        .iter()
        .rev()
        .find(|t| !t.modal && t.id.as_deref() == Some(id))
}

/// The focus for the frame about to run, from the one before, the requests
/// the last run made, and a press at (`px`, `py`) on this frame. Called by
/// [`crate::hit::roll`] once the button bindings are bound.
pub(crate) fn roll_focus(env: &mut Env, registry: &[Target], px: f64, py: f64) {
    let prev = string_binding(env, SYM_UI_FOCUS);
    let mut focus = prev.clone();
    let buf = env.intern_symbol(UI_FOCUS_BUFFER);
    let requests = env.take_output_buffer(buf);
    if let Some(last) = requests.last() {
        focus = match last {
            Value::String(s) => env.heap().get_string(*s).to_string(),
            Value::Int(n) => n.to_string(),
            _ => String::new(),
        };
    }
    if pressed_left(env) {
        let rows = press_rows(registry, px, py);
        match front(&rows) {
            Some(t) if t.takes_focus() && t.id.is_some() => {
                focus = t.id.clone().unwrap_or_default();
            }
            owner => {
                if !focus.is_empty() {
                    let held = row_of(registry, &focus).map_or(0, |t| t.layer);
                    if !owner.is_some_and(|o| o.layer > held) {
                        focus.clear();
                    }
                }
            }
        }
    }
    // Bind only what moved: an unchanged binding keeps its revision, so a
    // run that read the focus is not re-run for nothing.
    let claims: Vec<String> = match row_of(registry, &focus) {
        Some(t) if !focus.is_empty() => {
            let mut c = Vec::with_capacity(t.keys.len() + 1);
            if t.text {
                c.push("text".to_string());
            }
            c.extend(t.keys.iter().cloned());
            c
        }
        _ => Vec::new(),
    };
    let focus_sym = env.intern_symbol(SYM_UI_FOCUS);
    if focus != prev || env.binding(focus_sym).is_none() {
        let v = Value::String(env.heap_mut().alloc_string(focus));
        env.set_binding(focus_sym, v);
    }
    if strings_binding(env, SYM_UI_CLAIMS).as_ref() != Some(&claims) {
        let items: Vec<Value> = claims
            .into_iter()
            .map(|k| Value::String(env.heap_mut().alloc_string(k)))
            .collect();
        let list = Value::List(env.heap_mut().alloc_list(items));
        let sym = env.intern_symbol(SYM_UI_CLAIMS);
        env.set_binding(sym, list);
    }
}

/// Who has the keyboard for the frame about to run (`""`: nobody).
pub fn focus(env: &mut Env) -> String {
    string_binding(env, SYM_UI_FOCUS)
}

// ── Script side ───────────────────────────────────────────────────────────

pub(crate) fn register_focus(env: &mut Env) {
    const READS: NativeEffects = NativeEffects::probe(InputClasses::BINDINGS);
    const KEYS: NativeEffects =
        NativeEffects::probe(InputClasses::KEYBOARD.union(InputClasses::BINDINGS));
    const EMITS: NativeEffects = NativeEffects::PURE.with_emits();
    register(env, "ui_focus", native_ui_focus, READS);
    register(env, "ui_focus_request", native_ui_focus_request, EMITS);
    register(env, "ui_shortcut", native_ui_shortcut, KEYS);
}

fn id_arg(state: &PetalCxt, index: usize, who: &str) -> Result<Option<String>, String> {
    match state.get_value(index)? {
        Value::Nil => Ok(None),
        Value::String(s) => Ok(Some(state.heap().get_string(s).to_string())),
        Value::Int(n) => Ok(Some(n.to_string())),
        other => Err(format!(
            "{who}() expects a string or int id, got {}",
            other.type_name()
        )),
    }
}

fn cxt_string(state: &mut PetalCxt, name: &str) -> String {
    match state.binding_named(name) {
        Value::String(s) => state.heap().get_string(s).to_string(),
        _ => String::new(),
    }
}

/// `ui_focus()` → the id that has the keyboard, `""` for nobody.
fn native_ui_focus(state: &mut PetalCxt) -> NativeResult {
    let f = cxt_string(state, SYM_UI_FOCUS);
    state.push_string(f);
    Ok(1)
}

/// `ui_focus_request(id)` — give `id` the keyboard at the next roll (`nil`
/// or `""`: nobody). The last request a run makes wins.
fn native_ui_focus_request(state: &mut PetalCxt) -> NativeResult {
    // An id that is not a string or an int (a record, from code that still
    // threads `fc` with ids of its own) cannot be named here: no request.
    let v = match state.get_value(1)? {
        Value::Nil => Value::Nil,
        Value::String(s) if state.heap().get_string(s).is_empty() => Value::Nil,
        v @ Value::String(_) => v,
        Value::Int(n) => Value::String(state.heap_mut().alloc_string(n.to_string())),
        _ => {
            state.push_nil();
            return Ok(1);
        }
    };
    let sym = state.intern_symbol(UI_FOCUS_BUFFER);
    state.push_output(sym, v);
    state.push_nil();
    Ok(1)
}

/// `ui_shortcut(chord, [scope])` → whether `chord` was pressed this frame
/// and is the app's: nothing focused claims it, or (with `scope`) `scope`
/// has the keyboard.
fn native_ui_shortcut(state: &mut PetalCxt) -> NativeResult {
    let raw = match state.get_value(1)? {
        Value::String(s) => state.heap().get_string(s).to_string(),
        other => {
            return Err(format!(
                "shortcut() expects a chord string like \"cmd+z\", got {}",
                other.type_name()
            ));
        }
    };
    let chord = parse(&raw)
        .ok_or_else(|| format!("shortcut(): '{raw}' is not a chord (like \"cmd+shift+z\")"))?;
    let scope = if state.arg_count() >= 2 {
        id_arg(state, 2, "shortcut")?
    } else {
        None
    };
    let key = chord.key.to_string();
    let down = match state.binding_named(SYM_KEYS_PRESSED) {
        Value::List(id) => state
            .heap()
            .get_list(id)
            .iter()
            .any(|v| matches!(v, Value::String(s) if state.heap().get_string(*s) == key)),
        _ => false,
    };
    let fires = down && {
        let mods = match state.binding_named(SYM_MODIFIERS) {
            Value::Int(n) => n,
            _ => 0,
        };
        let (shift, cmd, alt) = (mods & 1 != 0, mods & (2 | 8) != 0, mods & 4 != 0);
        cmd == chord.cmd && alt == chord.alt && shift == chord.shift
    } && {
        let focus = cxt_string(state, SYM_UI_FOCUS);
        match scope {
            Some(id) => focus == id,
            None if focus.is_empty() => true,
            None => {
                let claims: Vec<String> = match state.binding_named(SYM_UI_CLAIMS) {
                    Value::List(id) => state
                        .heap()
                        .get_list(id)
                        .iter()
                        .filter_map(|v| match v {
                            Value::String(s) => Some(state.heap().get_string(*s).to_string()),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                !claimed(&claims, &spell(&chord))
            }
        }
    };
    state.push_bool(fires);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_are_spelled_one_way() {
        assert_eq!(canonical_chord("Shift+Cmd+Z").as_deref(), Some("cmd+shift+z"));
        assert_eq!(canonical_chord("ctrl+alt+k").as_deref(), Some("cmd+alt+k"));
        assert_eq!(canonical_chord("escape").as_deref(), Some("escape"));
        assert_eq!(canonical_chord("cmd+"), None);
        assert_eq!(canonical_chord("hyper+k"), None);
    }

    #[test]
    fn text_claims_plain_keys_and_editing_chords_but_not_save() {
        let text = vec!["text".to_string()];
        assert!(claimed(&text, "v"));
        assert!(claimed(&text, "shift+tab"));
        assert!(claimed(&text, "escape"));
        assert!(claimed(&text, "cmd+z"));
        assert!(!claimed(&text, "cmd+s"));
        let list = vec!["down".to_string(), "up".to_string()];
        assert!(claimed(&list, "down"));
        assert!(!claimed(&list, "v"));
        assert!(!claimed(&[], "v"));
    }
}
