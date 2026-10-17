//! Layered hit testing and the target registry.
//!
//! Immediate mode has no z order: a button drawn early cannot know that a
//! panel drawn later covers it. This module is how the `ui` prelude's
//! `interact` / `ui_layer` / `ui_modal` / `pointer_captured` answer that
//! question without the script keeping a list of claimed rectangles itself.
//!
//! # The model
//!
//! Every interactive rect a script declares this frame is **registered**: the
//! prelude emits `[x, y, w, h, layer, flags, id]` (already intersected with
//! the clip in force) into the [`UI_TARGETS_BUFFER`] output buffer. The host
//! never has to drain that buffer itself — [`roll`], which
//! [`crate::input::bind_input`] calls at the top of every frame, does:
//!
//! 1. it drains the registrations of the frame that just ran into the
//!    [`SYM_UI_REGISTRY`] binding (the *registry*: what was on screen last
//!    frame, and where);
//! 2. it works out the **floor** for the frame about to run — the highest
//!    layer that has a registered rect under the pointer *now*, or that
//!    declared itself modal — and binds it as [`SYM_UI_FLOOR`].
//!
//! A rect at layer `L` is then *hot* when the pointer is inside it (and
//! inside its clip), and `L` is not below the floor. The script side adds one
//! refinement the host cannot make: a rect that turns out hot raises a
//! per-run floor of its own (a cell in the prelude, reset at the top of every
//! run), so anything asked *later in the same frame* on a lower layer is
//! already blocked. That is what closes the one-frame leak for
//! `pointer_captured()` called after the UI has drawn: a panel that opened
//! this very frame, under the pointer, captures the click on this frame.
//!
//! What it cannot close: a widget on a lower layer that runs *before* a
//! higher one that first appears this frame sees one frame of hover (and a
//! press, if the press lands on exactly that frame). `interact().click` —
//! released inside, having been pressed inside — does not have that leak,
//! because by the release the registry knows about the new panel.
//!
//! # Who a press belongs to
//!
//! `click` also needs the rect to have been *there* when the button went
//! down. A click that opens a panel would otherwise click whatever row of the
//! new panel sits under the pointer on the release. So on the frame the left
//! button goes down, [`roll`] snapshots the targets under the press point
//! that were open to it ([`SYM_PRESS_TARGETS`]): last frame's registry, plus,
//! on the next roll, what the press frame itself registered (a widget shown
//! on hover that a test moved onto and pressed in one frame). A rect is
//! clicked only if a snapshot row on its layer has its rect or its id. With
//! no snapshot — nothing was registered yet, or the press and the release
//! arrived in the same frame — `click` falls back to the press point alone.
//!
//! Everything here is a pure function of bindings and arguments, or an emit,
//! so the frame gate and memoized scopes stay exact: the floor is a binding
//! (a run that read it re-runs when it moves), registrations are emits (a
//! replayed scope re-registers), and the probes are re-evaluable.
//!
//! # A run that registered nothing
//!
//! An empty buffer means no run happened since the last roll (the gate
//! skipped the frame), so the registry is kept. A run that *ran* and
//! registered nothing (the HUD was hidden) must clear it instead, and says
//! so with a marker: the `ui` module pushes one (`nil`) from its module
//! scope, which executes once per run.

use petal::env::Env;
use petal::heap::{Heap, RecordMap};
use petal::native_fn::{InputClasses, NativeEffects, NativeResult, PetalCxt};
use petal::value::Value;

use crate::input::{InputState, SYM_BUTTONS_DOWN, SYM_BUTTONS_PRESSED, SYM_BUTTONS_RELEASED};
use crate::input::{SYM_MOUSE_X, SYM_MOUSE_Y};
use crate::params::register;

/// Output buffer the prelude registers interactive rects into. Drained by
/// [`roll`] (inside `bind_input`); a host reads the decoded form with
/// [`targets`].
pub const UI_TARGETS_BUFFER: &str = "ui_targets";

/// Binding holding last frame's registry: a list of
/// `[x, y, w, h, layer, flags, id]` rows.
pub const SYM_UI_REGISTRY: &str = "ui_registry";

/// Binding holding the floor for the frame about to run: an int layer, or
/// `nil` when nothing registered is under the pointer and nothing is modal.
pub const SYM_UI_FLOOR: &str = "ui_floor";

/// Bindings holding where the left button last went down (kept after the
/// release, unlike `drag_start_x/y`), so a release can be matched to its press.
pub const SYM_PRESS_X: &str = "press_x";
pub const SYM_PRESS_Y: &str = "press_y";

/// Binding holding the targets the left button's last press landed on: a list
/// of registry rows, or `nil` when there is nothing to check against. See
/// "Who a press belongs to" above.
pub const SYM_PRESS_TARGETS: &str = "ui_press_targets";

/// Binding: the press frame's own registrations still have to be added to
/// [`SYM_PRESS_TARGETS`] on the next roll.
const SYM_PRESS_SETTLE: &str = "ui_press_settle";

/// `flags` bit: the rect blocks the pointer for lower layers.
pub const FLAG_BLOCKS: i64 = 1;
/// `flags` bit: not a rect at all — the layer declared itself modal, so every
/// lower layer is inert wherever the pointer is.
pub const FLAG_MODAL: i64 = 2;

/// One row of the registry: a rect a script declared, as the host sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The id the script gave it (`interact(r, "save")`), if any. A string id
    /// is itself; a number is its decimal spelling.
    pub id: Option<String>,
    /// The rect in logical pixels, already intersected with the clip that was
    /// in force — so its centre is a point a click would actually land on.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub layer: i64,
    /// Whether it blocks the pointer for lower layers (a named, non-blocking
    /// `ui_target` does not).
    pub blocks: bool,
    /// A modal declaration rather than a rect (`w` and `h` are 0).
    pub modal: bool,
}

impl Target {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// The middle of the rect — where a test or an agent clicks it.
    pub fn center(&self) -> (i32, i32) {
        (
            (self.x + self.w / 2.0).floor() as i32,
            (self.y + self.h / 2.0).floor() as i32,
        )
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(f) => Some(*f),
        Value::Dual { value, .. } => Some(*value),
        _ => None,
    }
}

fn decode_row(v: &Value, heap: &Heap) -> Option<Target> {
    let Value::List(id) = v else { return None };
    let row = heap.get_list(*id);
    if row.len() < 7 {
        return None;
    }
    let flags = num(&row[5])? as i64;
    let id = match &row[6] {
        Value::String(s) => Some(heap.get_string(*s).to_string()),
        Value::Int(n) => Some(n.to_string()),
        _ => None,
    };
    Some(Target {
        id,
        x: num(&row[0])?,
        y: num(&row[1])?,
        w: num(&row[2])?,
        h: num(&row[3])?,
        layer: num(&row[4])? as i64,
        blocks: flags & FLAG_BLOCKS != 0,
        modal: flags & FLAG_MODAL != 0,
    })
}

fn decode_rows(values: &[Value], heap: &Heap) -> Vec<Target> {
    live_rows(values)
        .into_iter()
        .filter_map(|v| decode_row(&v, heap))
        .collect()
}

/// The rows of a frame's buffer that count: not the markers, and not what
/// was registered between a `true` and its `false` — a piece of UI the script
/// ran only to measure (`ui_measure`), which is not on screen.
fn live_rows(values: &[Value]) -> Vec<Value> {
    let mut muted = 0usize;
    let mut out = Vec::with_capacity(values.len());
    for v in values {
        match v {
            Value::Bool(true) => muted += 1,
            Value::Bool(false) => muted = muted.saturating_sub(1),
            Value::List(_) if muted == 0 => out.push(*v),
            _ => {}
        }
    }
    out
}

/// The floor a registry implies for a pointer at (`px`, `py`): the highest
/// layer with a blocking rect under the pointer or a modal declaration.
pub fn floor_of(targets: &[Target], px: f64, py: f64) -> Option<i64> {
    targets
        .iter()
        .filter(|t| t.modal || (t.blocks && t.contains(px, py)))
        .map(|t| t.layer)
        .max()
}

/// Turn the registrations of the frame that just ran into the registry and
/// floor the next frame hit-tests against. [`crate::input::bind_input`] calls
/// this; a host that binds input some other way calls it itself, once per
/// frame, before the gate is consulted.
pub fn roll(env: &mut Env, input: &InputState) {
    let buf_sym = env.intern_symbol(UI_TARGETS_BUFFER);
    let reg_sym = env.intern_symbol(SYM_UI_REGISTRY);
    let emitted = env.take_output_buffer(buf_sym);
    let registry = if emitted.is_empty() {
        // No run since the last roll (or one that asked nothing): keep.
        match env.binding(reg_sym) {
            Some(Value::List(id)) => {
                let rows = env.heap().get_list(id).to_vec();
                decode_rows(&rows, env.heap())
            }
            _ => Vec::new(),
        }
    } else {
        // The markers (`nil`) are dropped here; the rows are kept as emitted,
        // minus exact repeats (a host that ran the script twice between two
        // rolls registered everything twice).
        let mut rows: Vec<Value> = Vec::with_capacity(emitted.len());
        let mut decoded: Vec<Target> = Vec::with_capacity(emitted.len());
        for v in &live_rows(&emitted) {
            if let Some(t) = decode_row(v, env.heap()) {
                if !decoded.contains(&t) {
                    decoded.push(t);
                    rows.push(*v);
                }
            }
        }
        let list = Value::List(env.heap_mut().alloc_list(rows));
        env.set_binding(reg_sym, list);
        decoded
    };
    let floor = floor_of(&registry, input.mouse_x as f64, input.mouse_y as f64);
    let floor_sym = env.intern_symbol(SYM_UI_FLOOR);
    env.set_binding(floor_sym, floor.map_or(Value::Nil, Value::Int));
    let (px, py) = input.last_press();
    let s = env.intern_symbol(SYM_PRESS_X);
    env.set_binding(s, Value::Int(px as i64));
    let s = env.intern_symbol(SYM_PRESS_Y);
    env.set_binding(s, Value::Int(py as i64));
    roll_press(env, &registry, px as f64, py as f64);
}

/// Whether the int list bound as `name` holds `button`.
fn env_list_has(env: &mut Env, name: &str, button: i64) -> bool {
    let sym = env.intern_symbol(name);
    match env.binding(sym) {
        Some(Value::List(id)) => env
            .heap()
            .get_list(id)
            .iter()
            .any(|v| matches!(v, Value::Int(n) if *n == button)),
        _ => false,
    }
}

/// The rows of `registry` a press at (`px`, `py`) landed on: under the point
/// and not below its floor.
fn press_rows(registry: &[Target], px: f64, py: f64) -> Vec<Target> {
    let floor = floor_of(registry, px, py);
    registry
        .iter()
        .filter(|t| !t.modal && t.contains(px, py) && floor.is_none_or(|f| t.layer >= f))
        .cloned()
        .collect()
}

fn encode_row(env: &mut Env, t: &Target) -> Value {
    let flags = if t.blocks { FLAG_BLOCKS } else { 0 };
    let id = match &t.id {
        Some(s) => Value::String(env.heap_mut().alloc_string(s.clone())),
        None => Value::Nil,
    };
    let row = vec![
        Value::Float(t.x),
        Value::Float(t.y),
        Value::Float(t.w),
        Value::Float(t.h),
        Value::Int(t.layer),
        Value::Int(flags),
        id,
    ];
    Value::List(env.heap_mut().alloc_list(row))
}

/// Keep [`SYM_PRESS_TARGETS`] up to date: snapshot it on the frame the left
/// button goes down, and add the press frame's own rows on the roll after.
/// Reads the button bindings, so it must run after they are bound.
fn roll_press(env: &mut Env, registry: &[Target], px: f64, py: f64) {
    let targets_sym = env.intern_symbol(SYM_PRESS_TARGETS);
    let settle_sym = env.intern_symbol(SYM_PRESS_SETTLE);
    if env_list_has(env, SYM_BUTTONS_PRESSED, 0) {
        let same_frame = env_list_has(env, SYM_BUTTONS_RELEASED, 0);
        if same_frame || registry.is_empty() {
            env.set_binding(targets_sym, Value::Nil);
            env.set_binding(settle_sym, Value::Bool(false));
        } else {
            let rows: Vec<Value> = press_rows(registry, px, py)
                .iter()
                .map(|t| encode_row(env, t))
                .collect();
            let list = Value::List(env.heap_mut().alloc_list(rows));
            env.set_binding(targets_sym, list);
            env.set_binding(settle_sym, Value::Bool(true));
        }
    } else if matches!(env.binding(settle_sym), Some(Value::Bool(true))) {
        env.set_binding(settle_sym, Value::Bool(false));
        if let Some(Value::List(id)) = env.binding(targets_sym) {
            let mut rows = env.heap().get_list(id).to_vec();
            let have = decode_rows(&rows, env.heap());
            for t in press_rows(registry, px, py) {
                if !have.contains(&t) {
                    rows.push(encode_row(env, &t));
                }
            }
            let list = Value::List(env.heap_mut().alloc_list(rows));
            env.set_binding(targets_sym, list);
        }
    }
}

/// The registry as the host reads it: every rect the most recent run
/// registered. Straight after a run this is that run's own registrations
/// (still in the buffer); once the next frame's input is bound it is the
/// rolled registry — the same rows either way.
pub fn targets(env: &mut Env) -> Vec<Target> {
    let buf_sym = env.intern_symbol(UI_TARGETS_BUFFER);
    let pending = env.output_buffer(buf_sym).to_vec();
    if !pending.is_empty() {
        return decode_rows(&pending, env.heap());
    }
    let reg_sym = env.intern_symbol(SYM_UI_REGISTRY);
    match env.binding(reg_sym) {
        Some(Value::List(id)) => {
            let rows = env.heap().get_list(id).to_vec();
            decode_rows(&rows, env.heap())
        }
        _ => Vec::new(),
    }
}

/// The target registered under `id` by the most recent run — the last one, if
/// a script registered the id more than once.
pub fn target(env: &mut Env, id: &str) -> Option<Target> {
    targets(env)
        .into_iter()
        .rev()
        .find(|t| !t.modal && t.id.as_deref() == Some(id))
}

// ── Script side ───────────────────────────────────────────────────────────

/// Register the hit-testing natives the `ui` prelude builds `interact` and
/// friends on. Part of [`crate::input::register_input`].
pub fn register_hit(env: &mut Env) {
    // A probe: a pure function of its arguments, the pointer, and the floor
    // binding — re-evaluated at validation, so a memoized widget re-runs only
    // when its own answer changes.
    const HIT: NativeEffects =
        NativeEffects::probe(InputClasses::POINTER.union(InputClasses::BINDINGS));
    const READS: NativeEffects = NativeEffects::probe(InputClasses::BINDINGS);
    const EMITS: NativeEffects = NativeEffects::PURE.with_emits();
    register(env, "ui_register", native_ui_register, EMITS);
    // Declared an effect on purpose: the `ui` module calls it from module
    // scope, and that scope must *run* every frame rather than be replayed
    // from a memo record — it is where the run's layer, clip and floor are
    // reset, and a replay does not re-apply a scope's writes to cells the
    // scope itself created (which a module's own `state var`s are).
    register(env, "ui_mark", native_ui_mark, EMITS.with_effect());
    register(env, "ui_mute", native_ui_mute, EMITS);
    register(env, "ui_probe", native_ui_probe, HIT);
    register(env, "ui_hot", native_ui_hot, HIT);
    register(env, "ui_floor", native_ui_floor, READS);
    register(env, "ui_target_rect", native_ui_target_rect, READS);
}

/// A rect argument: an `{x, y, w, h}` record.
fn rect_arg(state: &PetalCxt, index: usize, who: &str) -> Result<[f64; 4], String> {
    let v = state.get_value(index)?;
    let Value::Map(id) = v else {
        return Err(format!(
            "{who}() expects a rect record, got {}",
            v.type_name()
        ));
    };
    let map = state.heap().get_map(id);
    let field = |name: &str| -> Result<f64, String> {
        match map.get(name) {
            Some(v) => num(v).ok_or_else(|| {
                format!(
                    "{who}() expects a number for rect field '{name}', got {}",
                    v.type_name()
                )
            }),
            None => Err(format!("{who}() expects a rect with field '{name}'")),
        }
    };
    Ok([field("x")?, field("y")?, field("w")?, field("h")?])
}

/// The clip argument: `nil` (no clip) or the prelude's clip record
/// `{x, y, w, h, prev}`.
fn clip_arg(state: &PetalCxt, index: usize, who: &str) -> Result<Option<[f64; 4]>, String> {
    match state.get_value(index)? {
        Value::Nil => Ok(None),
        Value::Map(_) => rect_arg(state, index, who).map(Some),
        other => Err(format!(
            "{who}() expects a clip rect or nil, got {}",
            other.type_name()
        )),
    }
}

fn intersect(r: [f64; 4], clip: Option<[f64; 4]>) -> [f64; 4] {
    let Some(c) = clip else { return r };
    let x0 = r[0].max(c[0]);
    let y0 = r[1].max(c[1]);
    let x1 = (r[0] + r[2]).min(c[0] + c[2]);
    let y1 = (r[1] + r[3]).min(c[1] + c[3]);
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

fn inside(r: &[f64; 4], px: f64, py: f64) -> bool {
    px >= r[0] && px < r[0] + r[2] && py >= r[1] && py < r[1] + r[3]
}

fn opt_layer(state: &PetalCxt, index: usize) -> Result<Option<i64>, String> {
    match state.get_value(index)? {
        Value::Nil => Ok(None),
        v => num(&v)
            .map(|n| Some(n as i64))
            .ok_or_else(|| format!("expected a layer number, got {}", v.type_name())),
    }
}

fn binding_num(state: &mut PetalCxt, name: &str) -> Option<f64> {
    num(&state.binding_named(name))
}

fn binding_has(state: &mut PetalCxt, name: &str, button: i64) -> bool {
    match state.binding_named(name) {
        Value::List(id) => state
            .heap()
            .get_list(id)
            .iter()
            .any(|v| matches!(v, Value::Int(n) if *n == button)),
        _ => false,
    }
}

/// `ui_register(rect, layer, clip, [id], [flags])` — record `rect ∩ clip` in
/// this frame's registry. `flags` defaults to [`FLAG_BLOCKS`].
fn native_ui_register(state: &mut PetalCxt) -> NativeResult {
    let flags = if state.arg_count() >= 5 {
        state.get_int(5)?
    } else {
        FLAG_BLOCKS
    };
    let layer = Value::Int(opt_layer(state, 2)?.unwrap_or(0));
    let clip = clip_arg(state, 3, "ui_register")?;
    let r = if flags & FLAG_MODAL != 0 && matches!(state.get_value(1)?, Value::Nil) {
        [0.0; 4]
    } else {
        intersect(rect_arg(state, 1, "ui_register")?, clip)
    };
    let id = if state.arg_count() >= 4 {
        match state.get_value(4)? {
            v @ (Value::String(_) | Value::Int(_)) => v,
            _ => Value::Nil,
        }
    } else {
        Value::Nil
    };
    let row = vec![
        Value::Float(r[0]),
        Value::Float(r[1]),
        Value::Float(r[2]),
        Value::Float(r[3]),
        layer,
        Value::Int(flags),
        id,
    ];
    let row = Value::List(state.heap_mut().alloc_list(row));
    let sym = state.intern_symbol(UI_TARGETS_BUFFER);
    state.push_output(sym, row);
    state.push_nil();
    Ok(1)
}

/// `ui_mark()` — say "a run asked about the pointer this frame", so a run
/// that registers nothing still replaces last frame's registry.
fn native_ui_mark(state: &mut PetalCxt) -> NativeResult {
    state.note_effect();
    let sym = state.intern_symbol(UI_TARGETS_BUFFER);
    state.push_output(sym, Value::Nil);
    state.push_nil();
    Ok(1)
}

/// `ui_mute(on)` — bracket a piece of UI the script runs only to measure:
/// nothing drawn or registered between `ui_mute(true)` and `ui_mute(false)`
/// reaches the host. One marker goes into the draw commands (the drain drops
/// the span, see `crate::compose`) and one into the registrations.
fn native_ui_mute(state: &mut PetalCxt) -> NativeResult {
    let on = state.get_bool(1)?;
    let sym = state.intern_symbol(UI_TARGETS_BUFFER);
    state.push_output(sym, Value::Bool(on));
    crate::draw::emit_draw(state, "mute", vec![Value::Bool(on)]);
    state.push_nil();
    Ok(1)
}

/// What the pointer is doing to one rect.
struct Probe {
    hover: bool,
    down: bool,
    pressed: bool,
    released: bool,
    click: bool,
}

/// Whether the press snapshot holds this rect: a row on `layer` with the same
/// rect (to half a pixel) or, when the rect has one, the same id. No
/// snapshot: every rect does.
fn held_at_press(state: &mut PetalCxt, r: &[f64; 4], layer: i64, id: Option<&str>) -> bool {
    let rows = match state.binding_named(SYM_PRESS_TARGETS) {
        Value::List(list) => state.heap().get_list(list).to_vec(),
        _ => return true,
    };
    let near = |a: f64, b: f64| (a - b).abs() <= 0.5;
    decode_rows(&rows, state.heap()).iter().any(|t| {
        t.layer == layer
            && ((near(t.x, r[0]) && near(t.y, r[1]) && near(t.w, r[2]) && near(t.h, r[3]))
                || (id.is_some() && t.id.as_deref() == id))
    })
}

/// The shared arguments `(rect, layer, clip, floor, [id])` → the answers.
fn probe(state: &mut PetalCxt, who: &str) -> Result<Probe, String> {
    let r = intersect(rect_arg(state, 1, who)?, clip_arg(state, 3, who)?);
    let layer = opt_layer(state, 2)?.unwrap_or(0);
    let run_floor = opt_layer(state, 4)?;
    let frame_floor = binding_num(state, SYM_UI_FLOOR).map(|f| f as i64);
    let open = match (run_floor, frame_floor) {
        (None, None) => true,
        (a, b) => layer >= a.unwrap_or(i64::MIN).max(b.unwrap_or(i64::MIN)),
    };
    let px = binding_num(state, SYM_MOUSE_X).unwrap_or(0.0);
    let py = binding_num(state, SYM_MOUSE_Y).unwrap_or(0.0);
    let hover = open && inside(&r, px, py);
    let pressed = hover && binding_has(state, SYM_BUTTONS_PRESSED, 0);
    // Where the press landed decides who holds it — so a held button stays
    // `down` when the pointer slides off, and a release elsewhere is no click.
    let press_in = match (
        binding_num(state, SYM_PRESS_X),
        binding_num(state, SYM_PRESS_Y),
    ) {
        (Some(x), Some(y)) => inside(&r, x, y),
        // A host that binds no press point: fall back to where the pointer is.
        _ => inside(&r, px, py),
    };
    let down = open && press_in && binding_has(state, SYM_BUTTONS_DOWN, 0);
    let released = hover && binding_has(state, SYM_BUTTONS_RELEASED, 0);
    // A click is the release of a press this rect held: it was there, under
    // the press point, when the button went down — not a row that appeared
    // there since (the panel the press opened).
    let click = released && press_in && {
        let id = if state.arg_count() >= 5 {
            match state.get_value(5)? {
                Value::String(s) => Some(state.heap().get_string(s).to_string()),
                Value::Int(n) => Some(n.to_string()),
                _ => None,
            }
        } else {
            None
        };
        held_at_press(state, &r, layer, id.as_deref())
    };
    Ok(Probe {
        hover,
        down,
        pressed,
        released,
        click,
    })
}

/// `ui_probe(rect, layer, clip, floor, [id])` → `{hover, down, pressed,
/// released, click}`.
fn native_ui_probe(state: &mut PetalCxt) -> NativeResult {
    let p = probe(state, "interact")?;
    let mut map: RecordMap = petal::heap::record_map_with_capacity(5);
    map.insert("hover".to_string(), Value::Bool(p.hover));
    map.insert("down".to_string(), Value::Bool(p.down));
    map.insert("pressed".to_string(), Value::Bool(p.pressed));
    map.insert("released".to_string(), Value::Bool(p.released));
    map.insert("click".to_string(), Value::Bool(p.click));
    let id = state.heap_mut().alloc_map(map);
    state.push_value(Value::Map(id));
    Ok(1)
}

/// `ui_hot(rect, layer, clip, floor)` → whether the pointer is over the rect
/// and nothing above it has the pointer.
fn native_ui_hot(state: &mut PetalCxt) -> NativeResult {
    let p = probe(state, "ui_hot")?;
    state.push_bool(p.hover);
    Ok(1)
}

/// `ui_floor()` → the frame's floor layer, or `nil`.
fn native_ui_floor(state: &mut PetalCxt) -> NativeResult {
    let v = match state.binding_named(SYM_UI_FLOOR) {
        v @ Value::Int(_) => v,
        _ => Value::Nil,
    };
    state.push_value(v);
    Ok(1)
}

/// `ui_target_rect(id)` → `{x, y, w, h, layer}` of the rect registered under
/// `id` last frame, or `nil`.
fn native_ui_target_rect(state: &mut PetalCxt) -> NativeResult {
    let want = match state.get_value(1)? {
        Value::String(s) => state.heap().get_string(s).to_string(),
        Value::Int(n) => n.to_string(),
        other => {
            return Err(format!(
                "ui_target_rect() expects a string or int id, got {}",
                other.type_name()
            ));
        }
    };
    let rows = match state.binding_named(SYM_UI_REGISTRY) {
        Value::List(id) => state.heap().get_list(id).to_vec(),
        _ => Vec::new(),
    };
    let found = decode_rows(&rows, state.heap())
        .into_iter()
        .rev()
        .find(|t| !t.modal && t.id.as_deref() == Some(want.as_str()));
    match found {
        Some(t) => {
            let whole = |f: f64| {
                if f.fract() == 0.0 {
                    Value::Int(f as i64)
                } else {
                    Value::Float(f)
                }
            };
            let mut map: RecordMap = petal::heap::record_map_with_capacity(5);
            map.insert("x".to_string(), whole(t.x));
            map.insert("y".to_string(), whole(t.y));
            map.insert("w".to_string(), whole(t.w));
            map.insert("h".to_string(), whole(t.h));
            map.insert("layer".to_string(), Value::Int(t.layer));
            let id = state.heap_mut().alloc_map(map);
            state.push_value(Value::Map(id));
        }
        None => state.push_nil(),
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(x: f64, y: f64, w: f64, h: f64, layer: i64) -> Target {
        Target {
            id: None,
            x,
            y,
            w,
            h,
            layer,
            blocks: true,
            modal: false,
        }
    }

    #[test]
    fn the_floor_is_the_highest_layer_under_the_pointer() {
        let reg = vec![t(0.0, 0.0, 100.0, 100.0, 0), t(10.0, 10.0, 20.0, 20.0, 5)];
        assert_eq!(floor_of(&reg, 50.0, 50.0), Some(0));
        assert_eq!(floor_of(&reg, 15.0, 15.0), Some(5));
        assert_eq!(floor_of(&reg, 500.0, 500.0), None);
    }

    #[test]
    fn a_modal_is_a_floor_wherever_the_pointer_is() {
        let mut m = t(0.0, 0.0, 0.0, 0.0, 9);
        m.modal = true;
        m.blocks = false;
        let reg = vec![t(0.0, 0.0, 10.0, 10.0, 0), m];
        assert_eq!(floor_of(&reg, 500.0, 500.0), Some(9));
    }

    #[test]
    fn a_non_blocking_target_is_no_floor() {
        let mut named = t(0.0, 0.0, 10.0, 10.0, 3);
        named.blocks = false;
        assert_eq!(floor_of(&[named], 5.0, 5.0), None);
    }

    #[test]
    fn clipping_shrinks_a_rect_to_nothing_rather_than_negative() {
        let r = intersect([0.0, 0.0, 10.0, 10.0], Some([20.0, 20.0, 5.0, 5.0]));
        assert_eq!(r[2], 0.0);
        assert_eq!(r[3], 0.0);
        assert!(!inside(&r, 20.0, 20.0));
    }
}
