//! Inert, deterministic stand-ins for the host natives Garden registers on a
//! panel script (`garden/garden-script/src/panel.rs`), so a panel drawer runs
//! under a bare embedder — chiefly `petal-ui-run` — instead of dying at
//! `Unknown builtin: palette` on frame 0.
//!
//! These are *stubs*, not an implementation of the Garden host:
//!
//! - `palette()` returns the same fallback palette Garden resolves when no
//!   host theme is injected ([`STUB_PALETTE`], a copy of Garden's
//!   `FALLBACK_PALETTE`), so a drawer paints in its real colors.
//!   `panel_theme()` returns `{}` — exactly what Garden answers with no
//!   injected theme.
//! - `query(kind, arg)` answers a loading `Value::Pending` every frame — the
//!   same graceful degradation Garden's native performs when no provider is
//!   attached — so a drawer renders its spinner/loading path forever, which is
//!   deterministic. `invalidate` is a no-op.
//! - The push channels (`emit`, `mutate`, `claim_key`, `request_frame` /
//!   `animating`, the `navigate` family) validate their arguments like
//!   Garden's natives and then go nowhere.
//!   `mutate` still returns a unique handle; `mutate_result(handle)` and
//!   `nav_arg()` answer nil.
//! - `panel_store_get` answers nil (an empty store); `panel_store_set` keeps
//!   Garden's string-or-nil type check but stores nothing, so every run starts
//!   from the same blank slate. Both still report what Garden's do — a host
//!   read, an effect — so a memoized scope is classified the same way here as
//!   in Garden (the effect audit is what caught them not doing so); so do
//!   `invalidate` and the `edit_view_*` readers, for the same reason.
//! - The `text_view`/`edit_view` region natives emit the same `Host` draw
//!   commands Garden's do (tags `text_view`, `edit_view`,
//!   `edit_view_projection`, `text_view_styles`, `text_view_scroll_to`,
//!   `text_view_wrap`), so the regions appear in a headless trace; the
//!   read-back halves answer empty (`edit_view_text` → `""`,
//!   `edit_view_edits` → `[]`) since no editor exists.
//!
//! Everything here is a pure function of the script's own calls, so
//! registering the stubs cannot perturb a script that never uses them — the
//! byte-identical-trace property `petal-ui-run` is built on survives.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};

use indexmap::IndexMap;
use petal::env::Env;
use petal::native_fn::{InputClasses, NativeClass, NativeEffects, NativeResult, PetalCxt};
use petal::value::Value;

use crate::draw;

/// Garden's `FALLBACK_PALETTE` (the palette `palette()` resolves when the
/// host injects no theme), copied so headless runs of Garden drawers paint
/// the colors they would really ship with.
const STUB_PALETTE: &[(&str, [u8; 4])] = &[
    ("window_bg", [0x0d, 0x11, 0x17, 0xff]),
    ("panel", [0x10, 0x15, 0x1c, 0xff]),
    ("panel_focused", [0x10, 0x15, 0x1c, 0xff]),
    ("border", [0x26, 0x2c, 0x34, 0xff]),
    ("border_focused", [0x2f, 0x5a, 0x8f, 0xff]),
    ("text", [0xe6, 0xed, 0xf3, 0xff]),
    ("text_mut", [0xaa, 0xb4, 0xc0, 0xff]),
    ("text_dim", [0x7d, 0x85, 0x90, 0xff]),
    ("text_faint", [0x54, 0x5d, 0x68, 0xff]),
    ("cursor", [0xe6, 0xed, 0xf3, 0xff]),
    ("accent", [0x58, 0xa6, 0xff, 0xff]),
    ("focus", [0x2f, 0x5a, 0x8f, 0xff]),
    ("sel", [0x1e, 0x3d, 0x63, 0xff]),
    ("hover", [0x16, 0x1d, 0x26, 0xff]),
    ("green", [0x3f, 0xb9, 0x50, 0xff]),
    ("orange", [0xd2, 0x99, 0x22, 0xff]),
    ("red", [0xf8, 0x51, 0x49, 0xff]),
    ("purple", [0xbc, 0x8c, 0xff, 0xff]),
    ("blue", [0x58, 0xa6, 0xff, 0xff]),
    ("error", [0xf8, 0x51, 0x49, 0xff]),
    ("added_bg", [0x12, 0x26, 0x1a, 0xff]),
    ("removed_bg", [0x2d, 0x18, 0x1b, 0xff]),
    ("hunk", [0x58, 0xa6, 0xff, 0xff]),
    ("hunk_bg", [0x17, 0x22, 0x34, 0xff]),
    ("hunk_bg_hover", [0x1f, 0x33, 0x50, 0xff]),
    ("scrollbar_thumb", [0x3a, 0x42, 0x4c, 0xff]),
    ("scrollbar_track", [0x21, 0x27, 0x2f, 0xff]),
];

/// Counter symbol behind the stub `mutate` handles — unique for the life of
/// the env, like Garden's, so a drawer that keeps a handle in `state` and
/// polls `mutate_result` later behaves sanely (the reply is just always nil).
const STUB_MUTATE_HANDLES: &str = "__stub_mutate_handles";

/// Register the whole stubbed Garden-panel vocabulary. Call after building
/// the env (natives resolve by name at call time, so order relative to
/// program loading does not matter).
pub fn register_panel_stubs(env: &mut Env) {
    // Each stub declares what *it* does, which is at most what Garden's
    // native does. Where Garden's reaches host state (the query cache, the
    // panel store, an editor buffer) the stub reports the same read or effect,
    // so a panel is classified the same way here as in Garden; a stub that
    // only validates its arguments and drops them is pure.
    const HOST_READ: NativeEffects = NativeEffects::reads(InputClasses::HOST_DATA);
    const EMITS: NativeEffects = NativeEffects::PURE.with_emits();
    env.register_native("palette", native_palette, NativeEffects::PURE);
    env.register_native("panel_theme", native_panel_theme, NativeEffects::PURE);
    // The loading path creates a resource, an effect the audit sets aside
    // because the call answers `Pending` (see `petal::effect_audit`).
    env.register_native("query", native_query, HOST_READ);
    env.register_native("invalidate", native_invalidate, NativeEffects::EFFECT);
    env.register_native("emit", native_emit, NativeEffects::PURE);
    env.register_native("mutate", native_mutate, NativeEffects::EFFECT);
    env.register_native("mutate_result", native_mutate_result, NativeEffects::PURE);
    env.register_native("claim_key", native_claim_key, NativeEffects::PURE);
    env.register_native("request_frame", native_request_frame, EMITS);
    env.register_native("animating", native_request_frame, EMITS);
    env.register_native("navigate", native_navigate, NativeEffects::PURE);
    env.register_native("navigate_replace", native_navigate, NativeEffects::PURE);
    env.register_native("navigate_back", native_nop_nil, NativeEffects::PURE);
    env.register_native("navigate_forward", native_nop_nil, NativeEffects::PURE);
    env.register_native("nav_arg", native_nop_nil, NativeEffects::PURE);
    env.register_native("panel_store_get", native_store_get, HOST_READ);
    env.register_native(
        "panel_store_set",
        native_store_set,
        NativeEffects::EFFECT.with_pending(NativeClass::Effectful),
    );
    env.register_native("text_view", native_text_view, EMITS);
    env.register_native("edit_view", native_edit_view, EMITS);
    env.register_native("edit_view_text", native_edit_view_text, HOST_READ);
    env.register_native("edit_view_edits", native_edit_view_edits, HOST_READ);
    env.register_native("edit_view_projection", native_edit_view_projection, EMITS);
    env.register_native("text_view_line_styles", native_text_view_line_styles, EMITS);
    env.register_native("text_view_scroll_to", native_text_view_scroll_to, EMITS);
    env.register_native("text_view_wrap", native_text_view_wrap, EMITS);
}

fn native_palette(cxt: &mut PetalCxt) -> NativeResult {
    let mut out: IndexMap<String, Value> = IndexMap::with_capacity(STUB_PALETTE.len());
    for (key, [r, g, b, a]) in STUB_PALETTE {
        let mut color: IndexMap<String, Value> = IndexMap::with_capacity(4);
        color.insert("r".to_string(), Value::Int(*r as i64));
        color.insert("g".to_string(), Value::Int(*g as i64));
        color.insert("b".to_string(), Value::Int(*b as i64));
        color.insert("a".to_string(), Value::Int(*a as i64));
        let id = cxt.heap_mut().alloc_map(color);
        out.insert((*key).to_string(), Value::Map(id));
    }
    let id = cxt.heap_mut().alloc_map(out);
    cxt.push_value(Value::Map(id));
    Ok(1)
}

fn native_panel_theme(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.heap_mut().alloc_map(IndexMap::new());
    cxt.push_value(Value::Map(id));
    Ok(1)
}

/// Ready answers for `query(kind, arg)`, keyed by `(kind, arg-as-JSON)`.
///
/// Empty by default, which is the provider-less Garden behavior every existing
/// caller gets: a loading `Pending`, forever. A host that wants a drawer to
/// render its *loaded* path — the differential oracle over the Garden and
/// worlds-fair corpora, chiefly — installs a table with
/// [`set_query_fixtures`].
type QueryFixtures = Vec<((String, String), serde_json::Value)>;

thread_local! {
    static QUERY_FIXTURES: RefCell<QueryFixtures> = const { RefCell::new(Vec::new()) };
}

/// Install ready `query` answers for this thread, returning the previous table.
///
/// The answers are fixed for the life of the table: a fixture is a constant
/// function of `(kind, arg)`, so a drawer sees the same model on every frame
/// and the run stays deterministic. A `(kind, arg)` with no fixture keeps the
/// loading `Pending`, so a table is additive — it can only move a drawer from
/// its spinner onto its content path, never the other way.
pub fn set_query_fixtures(fixtures: QueryFixtures) -> QueryFixtures {
    QUERY_FIXTURES.with(|f| std::mem::replace(&mut *f.borrow_mut(), fixtures))
}

/// Parse the `--query-fixtures` file: a JSON array of
/// `{"kind": ..., "arg": ..., "value": ...}`, the same shape
/// [`crate::host_data::fixture_provider`] takes.
///
/// `arg` is matched against the argument's JSON spelling, so a string arg is
/// written as a string (`"flight"`) and a record arg as that record.
pub fn parse_query_fixtures(json: &serde_json::Value) -> Result<QueryFixtures, String> {
    let entries = json
        .as_array()
        .ok_or_else(|| "query fixtures must be a JSON array".to_string())?;
    let mut table = QueryFixtures::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        let obj = e
            .as_object()
            .ok_or_else(|| format!("query fixture {i} must be an object"))?;
        let kind = match obj.get("kind") {
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => return Err(format!("query fixture {i} needs a string \"kind\"")),
        };
        let arg = obj
            .get("arg")
            .unwrap_or(&serde_json::Value::Null)
            .to_string();
        let value = obj.get("value").cloned().unwrap_or(serde_json::Value::Null);
        table.push(((kind, arg), value));
    }
    Ok(table)
}

/// `query(kind, arg)` — a fixture answer if one is installed for this
/// `(kind, arg)`, else a loading `Value::Pending`, forever, which is Garden's
/// own provider-less answer. The resource key hashes the arg's JSON form so any
/// arg shape (string, record, list) keys stably.
///
/// `note_host_read()` is what Garden's real `query`
/// (`garden-script/src/query.rs`) does for the same reason: the answer comes
/// from a host-owned cache the binding table does not cover, so the frame gate
/// must not treat a run that consulted it as a pure function of its bindings.
/// The stub owes the gate the same declaration — its answers are constant, so
/// today it only costs a re-run, but a stub that under-declares relative to the
/// native it stands in for would hide exactly the staleness this corpus exists
/// to find.
fn native_query(cxt: &mut PetalCxt) -> NativeResult {
    let kind = cxt.get_string(1)?;
    let arg = cxt.get_value(2)?;
    let arg_json = petal::value::value_to_json(&arg, cxt.heap()).to_string();
    cxt.note_host_read();

    let fixture = QUERY_FIXTURES.with(|f| {
        f.borrow()
            .iter()
            .find(|((k, a), _)| *k == kind && *a == arg_json)
            .map(|(_, v)| v.clone())
    });
    if let Some(value) = fixture {
        let v = petal::value::json_to_value(&value, cxt.heap_mut())?;
        cxt.push_value(v);
        return Ok(1);
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    kind.hash(&mut hasher);
    0u8.hash(&mut hasher);
    arg_json.hash(&mut hasher);
    let key = hasher.finish();
    let origin = cxt.origin();
    let frame = cxt.frame();
    let id = cxt
        .resources_mut()
        .get_or_create_loading(key, origin, frame);
    cxt.push_value(Value::Pending(id));
    Ok(1)
}

/// `request_frame()` / `animating()`: push the same `animating` marker
/// Garden's do (an emit, so the memo classifies the call the same way); no
/// host drains it here.
fn native_request_frame(cxt: &mut PetalCxt) -> NativeResult {
    let sym = cxt.intern_symbol("animating");
    cxt.push_output(sym, Value::Int(1));
    cxt.push_nil();
    Ok(1)
}

fn native_invalidate(cxt: &mut PetalCxt) -> NativeResult {
    let _kind = cxt.get_string(1)?;
    let _arg = cxt.get_value(2)?;
    // Garden's drops a cache entry, which no replay reproduces.
    cxt.note_effect();
    cxt.push_nil();
    Ok(1)
}

fn native_emit(cxt: &mut PetalCxt) -> NativeResult {
    let _event = cxt.get_string(1)?;
    let _arg = cxt.get_value(2)?;
    cxt.push_nil();
    Ok(1)
}

fn native_mutate(cxt: &mut PetalCxt) -> NativeResult {
    let _name = cxt.get_string(1)?;
    let _arg = cxt.get_value(2)?;
    let counter = cxt.intern_symbol(STUB_MUTATE_HANDLES);
    let handle = cxt.next_counter(counter) as i64 + 1;
    cxt.push_int(handle);
    Ok(1)
}

fn native_mutate_result(cxt: &mut PetalCxt) -> NativeResult {
    let _handle = cxt.get_int(1)?;
    cxt.push_nil();
    Ok(1)
}

fn native_claim_key(cxt: &mut PetalCxt) -> NativeResult {
    let _key = cxt.get_string(1)?;
    // Optional modifier argument (string or bitmask); accepted, unchecked.
    cxt.push_nil();
    Ok(1)
}

/// `navigate(screen[, arg])` / `navigate_replace(...)`: validated, dropped.
fn native_navigate(cxt: &mut PetalCxt) -> NativeResult {
    let _screen = cxt.get_string(1)?;
    cxt.push_nil();
    Ok(1)
}

/// Zero-argument nil-returners: `navigate_back`, `navigate_forward`,
/// `nav_arg` (no navigation ever happened, so there is no argument to read).
fn native_nop_nil(cxt: &mut PetalCxt) -> NativeResult {
    cxt.push_nil();
    Ok(1)
}

fn native_store_get(cxt: &mut PetalCxt) -> NativeResult {
    let _key = cxt.get_string(1)?;
    cxt.note_host_read();
    cxt.push_nil();
    Ok(1)
}

fn native_store_set(cxt: &mut PetalCxt) -> NativeResult {
    let _key = cxt.get_string(1)?;
    match cxt.get_value(2)? {
        Value::Nil | Value::String(_) => {}
        other => {
            return Err(format!(
                "panel_store_set() value must be a string or nil, got {} — encode it first (e.g. json_stringify)",
                other.type_name()
            ));
        }
    }
    cxt.note_effect();
    cxt.push_nil();
    Ok(1)
}

/// Emit the same `Host` draw command Garden's `text_view` does, so the region
/// shows up in a headless trace.
fn native_text_view(cxt: &mut PetalCxt) -> NativeResult {
    emit_region(cxt, "text_view")
}

fn native_edit_view(cxt: &mut PetalCxt) -> NativeResult {
    emit_region(cxt, "edit_view")
}

fn emit_region(cxt: &mut PetalCxt, tag: &str) -> NativeResult {
    let id = cxt.get_int(1)?;
    let x = cxt.get_int(2)?;
    let y = cxt.get_int(3)?;
    let w = cxt.get_int(4)?;
    let h = cxt.get_int(5)?;
    let text = cxt.get_string(6)?;
    let text_id = cxt.heap_mut().alloc_string(text);
    draw::emit_draw(
        cxt,
        tag,
        vec![
            Value::Int(id),
            Value::Int(x),
            Value::Int(y),
            Value::Int(w),
            Value::Int(h),
            Value::String(text_id),
        ],
    );
    cxt.push_nil();
    Ok(1)
}

fn native_edit_view_text(cxt: &mut PetalCxt) -> NativeResult {
    let _id = cxt.get_int(1)?;
    // Garden's answers from the editor buffer: a host read.
    cxt.note_host_read();
    cxt.push_string(String::new());
    Ok(1)
}

fn native_edit_view_edits(cxt: &mut PetalCxt) -> NativeResult {
    let _id = cxt.get_int(1)?;
    cxt.note_host_read();
    cxt.push_list(Vec::new());
    Ok(1)
}

fn native_edit_view_projection(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let spec = cxt.get_value(2)?;
    if !matches!(spec, Value::Map(_)) {
        return Err(format!(
            "edit_view_projection() expects a projection record, got {}",
            spec.type_name()
        ));
    }
    draw::emit_draw(cxt, "edit_view_projection", vec![Value::Int(id), spec]);
    cxt.push_nil();
    Ok(1)
}

fn native_text_view_line_styles(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let styles = cxt.get_value(2)?;
    if !matches!(styles, Value::List(_)) {
        return Err(format!(
            "text_view_line_styles() expects a list of style names, got {}",
            styles.type_name()
        ));
    }
    draw::emit_draw(cxt, "text_view_styles", vec![Value::Int(id), styles]);
    cxt.push_nil();
    Ok(1)
}

fn native_text_view_scroll_to(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let line = cxt.get_int(2)?;
    draw::emit_draw(
        cxt,
        "text_view_scroll_to",
        vec![Value::Int(id), Value::Int(line)],
    );
    cxt.push_nil();
    Ok(1)
}

fn native_text_view_wrap(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let wrap = cxt.get_bool(2)?;
    draw::emit_draw(
        cxt,
        "text_view_wrap",
        vec![Value::Int(id), Value::Int(wrap as i64)],
    );
    cxt.push_nil();
    Ok(1)
}
