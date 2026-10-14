//! What the drain does to a frame's commands before a host sees them: draw
//! layers, retained layers, and batches.
//!
//! Three things a script can say that a plain painter's-order command list
//! cannot, each resolved here so that a host which knows nothing about them
//! still draws the right picture:
//!
//! - **Draw layers** (`ui_layer(n)` / `with_layer` in the prelude). A
//!   [`DrawCommand::Layer`] marker says "from here on, layer `n`". The drain
//!   sorts the frame by layer — stably, so the order within a layer is the
//!   order drawn — and drops the markers. A tooltip drawn in the middle of a
//!   frame at a high layer comes out on top. No host ever sees a marker.
//! - **Retained layers** (`cached_layer(key, rect, body)`). The body's
//!   commands arrive once, between [`DrawCommand::CacheBegin`] and
//!   [`DrawCommand::CacheEnd`]; on every later frame only a
//!   [`DrawCommand::CacheDraw`] arrives. By default the drain keeps the body
//!   (as a binding on the env, so it lives exactly as long as the script's
//!   state does) and replaces each `CacheDraw` with it, moved to where it is
//!   drawn and clipped to its size. The script stops paying for the body; the
//!   host still rasterizes it. A host with render targets can do better: it
//!   sets [`HostCaps::retained_layers`], receives the three commands as they
//!   are, and keeps a texture.
//! - **Batches** (`draw_rects`). One [`DrawCommand::Rects`] carries many
//!   rects. Lowered to one [`DrawCommand::Rect`] each unless the host sets
//!   [`HostCaps::batch_rects`].
//!
//! A fourth marker, [`DrawCommand::Mute`], brackets what a script drew only to
//! measure it (`ui_measure`); the drain drops the span.
//!
//! The order is fixed: muted spans are dropped, retained layers are
//! resolved, then the frame is sorted by layer, then batches are lowered. A
//! frame that uses none of these is returned untouched.

use petal::env::Env;
use petal::native_fn::{InputClasses, NativeEffects, NativeResult, PetalCxt};
use petal::stack::StackKey;
use petal::value::Value;

use crate::draw::{BatchRect, DRAW, DrawCommand, color_of, emit_draw, num_as_f64};
use crate::params::register;

/// Binding holding the host's capabilities, as bits ([`HostCaps::bits`]).
pub const SYM_HOST_CAPS: &str = "ui_host_caps";

/// Binding holding the retained-layer epoch: a number the host bumps
/// ([`invalidate_caches`]) when it lost what it was keeping, so every
/// `cached_layer` records its body again.
pub const SYM_CACHE_EPOCH: &str = "ui_cache_epoch";

/// Binding listing the retained-layer ids the drain is keeping content for.
const SYM_CACHE_IDS: &str = "ui_cache_ids";

/// How deep a retained layer may draw another before the drain stops
/// following (a layer that draws itself would otherwise never end).
const MAX_CACHE_DEPTH: usize = 8;

/// What a host handles itself rather than having the drain lower for it.
/// The default is nothing: every host gets the classic vocabulary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostCaps {
    /// The host draws [`DrawCommand::Rects`] itself.
    pub batch_rects: bool,
    /// The host keeps retained layers itself (as textures, say): it receives
    /// [`DrawCommand::CacheBegin`] / [`DrawCommand::CacheEnd`] /
    /// [`DrawCommand::CacheDraw`] and the content commands between the first
    /// two, in layer-local coordinates.
    pub retained_layers: bool,
}

impl HostCaps {
    pub const BATCH_RECTS: i64 = 1;
    pub const RETAINED_LAYERS: i64 = 2;

    pub fn bits(self) -> i64 {
        (if self.batch_rects { Self::BATCH_RECTS } else { 0 })
            | (if self.retained_layers { Self::RETAINED_LAYERS } else { 0 })
    }

    pub fn from_bits(bits: i64) -> HostCaps {
        HostCaps {
            batch_rects: bits & Self::BATCH_RECTS != 0,
            retained_layers: bits & Self::RETAINED_LAYERS != 0,
        }
    }
}

/// Declare what this host handles itself. Call once at startup (the binding
/// persists); the default is [`HostCaps::default`].
pub fn set_host_caps(env: &mut Env, caps: HostCaps) {
    let sym = env.intern_symbol(SYM_HOST_CAPS);
    env.set_binding(sym, Value::Int(caps.bits()));
}

/// The capabilities bound on `env` (none, if the host never said).
pub fn host_caps(env: &mut Env) -> HostCaps {
    let sym = env.intern_symbol(SYM_HOST_CAPS);
    match env.binding(sym) {
        Some(Value::Int(bits)) => HostCaps::from_bits(bits),
        _ => HostCaps::default(),
    }
}

/// Tell every `cached_layer` to record its body again on the next run — for a
/// host with [`HostCaps::retained_layers`] that lost its textures (a resized
/// or recreated device), or that was sent a `CacheDraw` for a layer it does
/// not have. The script re-runs because it read the epoch.
pub fn invalidate_caches(env: &mut Env) {
    let sym = env.intern_symbol(SYM_CACHE_EPOCH);
    let next = match env.binding(sym) {
        Some(Value::Int(n)) => n + 1,
        _ => 1,
    };
    env.set_binding(sym, Value::Int(next));
}

fn cache_symbol(id: u32) -> String {
    format!("ui_cache:{id}")
}

fn is_composed(cmd: &DrawCommand) -> bool {
    matches!(
        cmd,
        DrawCommand::Rects { .. }
            | DrawCommand::Layer { .. }
            | DrawCommand::Mute { .. }
            | DrawCommand::CacheBegin { .. }
            | DrawCommand::CacheEnd { .. }
            | DrawCommand::CacheDraw { .. }
    )
}

/// A command and the index of the emitted command it came from (`None` for
/// one the drain made up, like a clip restored between layers).
pub type Composed = (DrawCommand, Option<usize>);

/// Resolve layers, retained layers and batches in a drained frame. `values`
/// are the raw emitted values, index for index with `cmds`; `stack` is the
/// fork the frame was drained from (`None` for the env's main stack).
pub fn compose(
    env: &mut Env,
    stack: Option<StackKey>,
    cmds: Vec<DrawCommand>,
    values: Vec<Value>,
) -> Vec<Composed> {
    if !cmds.iter().any(is_composed) {
        return cmds
            .into_iter()
            .enumerate()
            .map(|(i, c)| (c, Some(i)))
            .collect();
    }
    let caps = host_caps(env);
    // What was drawn only to be measured is dropped first, so nothing below
    // ever sees it. Indices are taken before the drop: they name the emitted
    // command, which is what a traced drain attributes by.
    let mut muted = 0usize;
    let mut kept: Vec<(usize, DrawCommand, Value)> = Vec::with_capacity(cmds.len());
    for (i, (cmd, value)) in cmds.into_iter().zip(values).enumerate() {
        match cmd {
            DrawCommand::Mute { on: true } => muted += 1,
            DrawCommand::Mute { on: false } => muted = muted.saturating_sub(1),
            other if muted == 0 => kept.push((i, other, value)),
            _ => {}
        }
    }
    let mut out: Vec<Composed> = if caps.retained_layers {
        kept.into_iter().map(|(i, c, _)| (c, Some(i))).collect()
    } else {
        resolve_caches(env, stack, kept)
    };
    if out.iter().any(|(c, _)| matches!(c, DrawCommand::Layer { .. })) {
        out = sort_layers(out);
    }
    if !caps.batch_rects {
        out = lower_batches(out);
    }
    out
}

// ── Retained layers ───────────────────────────────────────────────────────

/// Bindings and heap of the context a frame was drained from.
struct Store<'a> {
    env: &'a mut Env,
    stack: Option<StackKey>,
}

impl Store<'_> {
    fn get(&mut self, name: &str) -> Option<Value> {
        let sym = self.env.intern_symbol(name);
        match self.stack {
            Some(stack) => self.env.binding_for(stack, sym),
            None => self.env.binding(sym),
        }
    }

    fn set(&mut self, name: &str, value: Value) {
        let sym = self.env.intern_symbol(name);
        match self.stack {
            Some(stack) => self.env.set_binding_for(stack, sym, value),
            None => self.env.set_binding(sym, value),
        }
    }

    fn list(&mut self, items: Vec<Value>) -> Option<Value> {
        let heap = match self.stack {
            Some(stack) => self.env.heap_for_mut(stack)?,
            None => self.env.heap_mut(),
        };
        Some(Value::List(heap.alloc_list(items)))
    }

    fn items(&self, v: &Value) -> Vec<Value> {
        let heap = match self.stack {
            Some(stack) => self.env.heap_for(stack),
            None => Some(self.env.heap()),
        };
        match (v, heap) {
            (Value::List(id), Some(heap)) => heap.get_list(*id).to_vec(),
            _ => Vec::new(),
        }
    }

    fn decode(&self, v: &Value) -> Option<DrawCommand> {
        let heap = match self.stack {
            Some(stack) => self.env.heap_for(stack)?,
            None => self.env.heap(),
        };
        DrawCommand::from_value(v, heap).ok()
    }
}

/// Keep what each `CacheBegin`…`CacheEnd` recorded, and replace each
/// `CacheDraw` with the kept content.
fn resolve_caches(
    env: &mut Env,
    stack: Option<StackKey>,
    cmds: Vec<(usize, DrawCommand, Value)>,
) -> Vec<Composed> {
    let mut store = Store { env, stack };
    // Recordings in progress: (id, w, h, the raw content so far). They nest —
    // a retained layer may draw another — and the inner one closes first.
    let mut open: Vec<(u32, u32, u32, Vec<Value>)> = Vec::new();
    let mut stream: Vec<Composed> = Vec::with_capacity(cmds.len());
    let mut used: Vec<u32> = Vec::new();
    let close = |store: &mut Store, (id, w, h, content): (u32, u32, u32, Vec<Value>)| {
        let mut items = vec![Value::Int(w as i64), Value::Int(h as i64)];
        items.extend(content);
        if let Some(list) = store.list(items) {
            store.set(&cache_symbol(id), list);
        }
    };
    for (i, cmd, value) in cmds {
        match cmd {
            DrawCommand::CacheBegin { id, w, h } => {
                used.push(id);
                open.push((id, w, h, Vec::new()));
            }
            DrawCommand::CacheEnd { .. } => {
                if let Some(done) = open.pop() {
                    close(&mut store, done);
                }
            }
            other => match open.last_mut() {
                // Inside a recording nothing is drawn where it stands, and a
                // draw-layer marker means nothing: the layer is one object.
                Some((_, _, _, content)) => {
                    if !matches!(other, DrawCommand::Layer { .. }) {
                        content.push(value);
                    }
                }
                None => stream.push((other, Some(i))),
            },
        }
    }
    // A recording the frame never closed (the script errored inside the body)
    // still holds what was drawn before the error.
    while let Some(done) = open.pop() {
        close(&mut store, done);
    }

    let mut out: Vec<Composed> = Vec::with_capacity(stream.len());
    for (cmd, from) in stream {
        match cmd {
            DrawCommand::CacheDraw { id, x, y } => {
                expand(&mut store, id, x, y, 0, from, &mut out, &mut used);
            }
            other => out.push((other, from)),
        }
    }

    // A layer this frame neither recorded nor drew is let go: its script-side
    // state was swept by the same absence, so it would be recorded again.
    // Drawn includes drawn by another layer's content: a nested layer's call
    // site does not run while the layer around it is replayed, and nothing
    // would record it again.
    let known = store.get(SYM_CACHE_IDS);
    let known: Vec<u32> = known
        .map(|v| store.items(&v))
        .unwrap_or_default()
        .iter()
        .filter_map(|v| match v {
            Value::Int(n) => Some(*n as u32),
            _ => None,
        })
        .collect();
    used.sort_unstable();
    used.dedup();
    for id in &known {
        if !used.contains(id) {
            store.set(&cache_symbol(*id), Value::Nil);
        }
    }
    if known != used {
        let ids = used.iter().map(|id| Value::Int(*id as i64)).collect();
        if let Some(list) = store.list(ids) {
            store.set(SYM_CACHE_IDS, list);
        }
    }
    out
}

/// Append retained layer `id`'s content, moved to (`x`, `y`) and clipped to
/// the layer's size. Every layer drawn, the ones drawn by its content
/// included, goes into `used`.
#[allow(clippy::too_many_arguments)]
fn expand(
    store: &mut Store,
    id: u32,
    x: i32,
    y: i32,
    depth: usize,
    from: Option<usize>,
    out: &mut Vec<Composed>,
    used: &mut Vec<u32>,
) {
    used.push(id);
    let Some(kept) = store.get(&cache_symbol(id)) else {
        return;
    };
    let items = store.items(&kept);
    if items.len() < 2 || depth >= MAX_CACHE_DEPTH {
        return;
    }
    let size = |v: &Value| match v {
        Value::Int(n) => (*n).max(0) as u32,
        _ => 0,
    };
    out.push((
        DrawCommand::ClipPush {
            x,
            y,
            w: size(&items[0]),
            h: size(&items[1]),
            radius: 0,
        },
        from,
    ));
    for value in &items[2..] {
        match store.decode(value) {
            Some(DrawCommand::CacheDraw { id: inner, x: ix, y: iy }) => {
                expand(store, inner, x + ix, y + iy, depth + 1, from, out, used);
            }
            Some(
                DrawCommand::Layer { .. }
                | DrawCommand::Mute { .. }
                | DrawCommand::CacheBegin { .. }
                | DrawCommand::CacheEnd { .. },
            )
            | None => {}
            Some(cmd) => out.push((translate(cmd, x, y), from)),
        }
    }
    out.push((DrawCommand::ClipPop, from));
}

/// `cmd` moved by (`dx`, `dy`).
pub fn translate(mut cmd: DrawCommand, dx: i32, dy: i32) -> DrawCommand {
    use DrawCommand::*;
    match &mut cmd {
        Image { x, y, .. }
        | Rect { x, y, .. }
        | RectOutline { x, y, .. }
        | Text { x, y, .. }
        | RectGradient { x, y, .. }
        | Shadow { x, y, .. }
        | Clip { x, y, .. }
        | ClipPush { x, y, .. }
        | DrawCanvas { x, y, .. }
        | Snapshot { x, y, .. }
        | CacheDraw { x, y, .. } => {
            *x += dx;
            *y += dy;
        }
        Line { x1, y1, x2, y2, .. } => {
            *x1 += dx;
            *y1 += dy;
            *x2 += dx;
            *y2 += dy;
        }
        Triangle {
            x1,
            y1,
            x2,
            y2,
            x3,
            y3,
            ..
        } => {
            *x1 += dx;
            *y1 += dy;
            *x2 += dx;
            *y2 += dy;
            *x3 += dx;
            *y3 += dy;
        }
        Circle { cx, cy, .. }
        | Ellipse { cx, cy, .. }
        | EllipseOutline { cx, cy, .. }
        | Arc { cx, cy, .. }
        | CircleGradient { cx, cy, .. } => {
            *cx += dx;
            *cy += dy;
        }
        Poly { points, .. } | Polygon { points, .. } | Polyline { points, .. } => {
            for p in points.iter_mut() {
                p.0 += dx;
                p.1 += dy;
            }
        }
        Fan { cx, cy, points, .. } => {
            *cx += dx;
            *cy += dy;
            for p in points.iter_mut() {
                p.0 += dx;
                p.1 += dy;
            }
        }
        Rects { rects } => {
            for r in rects.iter_mut() {
                r.x += dx;
                r.y += dy;
            }
        }
        Clear { .. }
        | ClipNone
        | ClipPop
        | CreateCanvas { .. }
        | SetTarget { .. }
        | BlurCanvas { .. }
        | Layer { .. }
        | Mute { .. }
        | CacheBegin { .. }
        | CacheEnd { .. }
        | Host { .. } => {}
    }
    cmd
}

// ── Draw layers ───────────────────────────────────────────────────────────

/// The clip in force at some point of a frame, as the commands that made it.
#[derive(Clone, PartialEq, Default)]
struct ClipState {
    /// The last `Clip` (replace), if `ClipNone` has not cleared it.
    base: Option<DrawCommand>,
    /// The `ClipPush`es not yet popped, outermost first.
    pushes: Vec<DrawCommand>,
}

impl ClipState {
    fn apply(&mut self, cmd: &DrawCommand) {
        match cmd {
            DrawCommand::Clip { .. } => self.base = Some(cmd.clone()),
            DrawCommand::ClipNone => self.base = None,
            DrawCommand::ClipPush { .. } => self.pushes.push(cmd.clone()),
            DrawCommand::ClipPop => {
                self.pushes.pop();
            }
            _ => {}
        }
    }
}

struct Segment {
    layer: i32,
    start: ClipState,
    end: ClipState,
    cmds: Vec<Composed>,
}

/// Sort a frame by draw layer and drop the markers.
///
/// A run of commands between two markers moves as a unit, and takes its clip
/// with it: where sorting puts a run after one that left a different clip in
/// force, the clip the run started under is re-established first (pops, then
/// the replace-clip, then the pushes). A marker inside an offscreen canvas or
/// a retained layer's recording is ignored — those are one object, drawn where
/// their composite is.
fn sort_layers(cmds: Vec<Composed>) -> Vec<Composed> {
    let mut clip = ClipState::default();
    let mut segments = vec![Segment {
        layer: 0,
        start: clip.clone(),
        end: clip.clone(),
        cmds: Vec::new(),
    }];
    let mut target = 0u32;
    let mut recording = 0usize;
    for (cmd, from) in cmds {
        match &cmd {
            DrawCommand::Layer { n } => {
                if target == 0 && recording == 0 {
                    let last = segments.last_mut().unwrap();
                    last.end = clip.clone();
                    if last.layer != *n {
                        segments.push(Segment {
                            layer: *n,
                            start: clip.clone(),
                            end: clip.clone(),
                            cmds: Vec::new(),
                        });
                    }
                }
                continue;
            }
            DrawCommand::SetTarget { id } => target = *id,
            DrawCommand::CacheBegin { .. } => recording += 1,
            DrawCommand::CacheEnd { .. } => recording = recording.saturating_sub(1),
            _ => {}
        }
        if target == 0 && recording == 0 {
            clip.apply(&cmd);
        }
        segments.last_mut().unwrap().cmds.push((cmd, from));
    }
    segments.last_mut().unwrap().end = clip;

    segments.sort_by_key(|s| s.layer);
    let mut out = Vec::new();
    let mut state = ClipState::default();
    for seg in segments {
        if seg.cmds.is_empty() {
            continue;
        }
        if state != seg.start {
            for _ in 0..state.pushes.len() {
                out.push((DrawCommand::ClipPop, None));
            }
            if state.base != seg.start.base {
                out.push((seg.start.base.clone().unwrap_or(DrawCommand::ClipNone), None));
            }
            for push in &seg.start.pushes {
                out.push((push.clone(), None));
            }
        }
        out.extend(seg.cmds);
        state = seg.end;
    }
    out
}

// ── Batches ───────────────────────────────────────────────────────────────

fn lower_batches(cmds: Vec<Composed>) -> Vec<Composed> {
    let mut out = Vec::with_capacity(cmds.len());
    for (cmd, from) in cmds {
        match cmd {
            DrawCommand::Rects { rects } => {
                out.extend(rects.into_iter().map(|r| {
                    (
                        DrawCommand::Rect {
                            x: r.x,
                            y: r.y,
                            w: r.w,
                            h: r.h,
                            r: r.r,
                            g: r.g,
                            b: r.b,
                            a: r.a,
                            radius: 0,
                        },
                        from,
                    )
                }));
            }
            other => out.push((other, from)),
        }
    }
    out
}

// ── Script side ───────────────────────────────────────────────────────────

/// Register the natives behind `ui_layer`, `cached_layer` and `draw_rects`.
/// Part of [`crate::draw::register_draw`].
pub fn register_compose(env: &mut Env) {
    const READS: NativeEffects = NativeEffects::probe(InputClasses::BINDINGS);
    register(env, "draw_rects", native_draw_rects, DRAW);
    register(env, "draw_layer", native_draw_layer, DRAW);
    register(env, "cache_begin", native_cache_begin, DRAW);
    register(env, "cache_end", native_cache_end, DRAW);
    register(env, "cache_draw", native_cache_draw, DRAW);
    register(env, "cache_valid", native_cache_valid, READS);
}

/// `draw_layer(n)` — everything drawn from here on is on draw layer `n`.
fn native_draw_layer(state: &mut PetalCxt) -> NativeResult {
    let n = state.get_int(1)?;
    emit_draw(state, "layer", vec![Value::Int(n)]);
    state.push_nil();
    Ok(1)
}

fn native_cache_begin(state: &mut PetalCxt) -> NativeResult {
    let args = vec![
        Value::Int(state.get_int(1)?),
        Value::Int(state.get_int(2)?.max(0)),
        Value::Int(state.get_int(3)?.max(0)),
    ];
    emit_draw(state, "cache_begin", args);
    state.push_nil();
    Ok(1)
}

fn native_cache_end(state: &mut PetalCxt) -> NativeResult {
    let id = state.get_int(1)?;
    emit_draw(state, "cache_end", vec![Value::Int(id)]);
    state.push_nil();
    Ok(1)
}

fn native_cache_draw(state: &mut PetalCxt) -> NativeResult {
    let args = vec![
        Value::Int(state.get_int(1)?),
        Value::Int(state.get_int(2)?),
        Value::Int(state.get_int(3)?),
    ];
    emit_draw(state, "cache_draw", args);
    state.push_nil();
    Ok(1)
}

/// `cache_valid(id)` → the retained-layer epoch `e` if layer `id`'s content
/// is still held, `-e - 1` if it has to be recorded again. A host that keeps
/// the layers itself always holds them, as far as the script can tell; it
/// says otherwise by bumping the epoch ([`invalidate_caches`]).
fn native_cache_valid(state: &mut PetalCxt) -> NativeResult {
    let id = state.get_int(1)?;
    let epoch = match state.binding_named(SYM_CACHE_EPOCH) {
        Value::Int(n) => n,
        _ => 0,
    };
    let host_keeps = match state.binding_named(SYM_HOST_CAPS) {
        Value::Int(bits) => HostCaps::from_bits(bits).retained_layers,
        _ => false,
    };
    let held = host_keeps
        || matches!(
            state.binding_named(&cache_symbol(id.max(0) as u32)),
            Value::List(_)
        );
    state.push_int(if held { epoch } else { -epoch - 1 });
    Ok(1)
}

/// `draw_rects(rects, [palette], [dx], [dy])` — many filled rects in one
/// call.
///
/// `rects` is a flat run of numbers, five per rect — `x, y, w, h, c` — as a
/// list or a packed `f64` array, or a list of `[x, y, w, h, c]` lists. With a
/// `palette` (a list of colours) `c` is an index into it; without one `c` is
/// the colour itself. A colour is a `{r, g, b, [a]}` record or a packed
/// `0xRRGGBB` int. `dx`, `dy` move the whole batch, so a baked list can be
/// built once in its own coordinates and drawn anywhere.
fn native_draw_rects(state: &mut PetalCxt) -> NativeResult {
    let n = state.arg_count();
    let palette: Option<Vec<(u8, u8, u8, u8)>> = if n >= 2 {
        match state.get_value(2)? {
            Value::Nil => None,
            Value::List(id) => {
                let items = state.heap().get_list(id);
                let mut out = Vec::with_capacity(items.len());
                for (i, v) in items.iter().enumerate() {
                    out.push(color_of(v, state.heap()).ok_or_else(|| {
                        format!(
                            "draw_rects() palette entry {i} is not a colour, got {}",
                            v.type_name()
                        )
                    })?);
                }
                Some(out)
            }
            other => {
                return Err(format!(
                    "draw_rects() expects a list of colours for the palette, got {}",
                    other.type_name()
                ));
            }
        }
    } else {
        None
    };
    let dx = if n >= 3 { state.get_float(3)? } else { 0.0 };
    let dy = if n >= 4 { state.get_float(4)? } else { 0.0 };

    let pack = |(r, g, b, a): (u8, u8, u8, u8)| -> f64 {
        (((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32) as f64
    };
    let colour = |state: &PetalCxt, v: &Value, at: usize| -> Result<f64, String> {
        match &palette {
            Some(p) => {
                let i = num_as_f64(v)
                    .ok_or_else(|| format!("draw_rects() rect {at}: palette index must be a number"))?
                    as i64;
                usize::try_from(i)
                    .ok()
                    .and_then(|i| p.get(i))
                    .map(|c| pack(*c))
                    .ok_or_else(|| {
                        format!(
                            "draw_rects() rect {at}: palette index {i} is outside the palette (0..{})",
                            p.len()
                        )
                    })
            }
            None => color_of(v, state.heap()).map(pack).ok_or_else(|| {
                format!(
                    "draw_rects() rect {at}: expected a colour, got {}",
                    v.type_name()
                )
            }),
        }
    };
    let coord = |v: &Value, at: usize| -> Result<f64, String> {
        num_as_f64(v).ok_or_else(|| {
            format!(
                "draw_rects() rect {at}: expected a number, got {}",
                v.type_name()
            )
        })
    };

    let mut packed: Vec<f64> = Vec::new();
    match state.get_value(1)? {
        Value::F64Array(id) => {
            let data = state.heap().get_f64_array(id).to_vec();
            if data.len() % 5 != 0 {
                return Err(format!(
                    "draw_rects() expects five numbers per rect (x, y, w, h, c); got {} numbers",
                    data.len()
                ));
            }
            packed.reserve(data.len());
            for (at, c) in data.chunks_exact(5).enumerate() {
                let col = match &palette {
                    Some(_) => colour(state, &Value::Float(c[4]), at)?,
                    None => pack(((c[4] as i64 >> 16) as u8, (c[4] as i64 >> 8) as u8, c[4] as i64 as u8, 255)),
                };
                packed.extend([c[0] + dx, c[1] + dy, c[2], c[3], col]);
            }
        }
        Value::List(id) => {
            let items = state.heap().get_list(id).to_vec();
            let nested = matches!(items.first(), Some(Value::List(_)));
            if nested {
                packed.reserve(items.len() * 5);
                for (at, item) in items.iter().enumerate() {
                    let row = match item {
                        Value::List(rid) => state.heap().get_list(*rid).to_vec(),
                        other => {
                            return Err(format!(
                                "draw_rects() rect {at}: expected [x, y, w, h, c], got {}",
                                other.type_name()
                            ));
                        }
                    };
                    if row.len() != 5 {
                        return Err(format!(
                            "draw_rects() rect {at}: expected [x, y, w, h, c], got {} values",
                            row.len()
                        ));
                    }
                    let col = colour(state, &row[4], at)?;
                    packed.extend([
                        coord(&row[0], at)? + dx,
                        coord(&row[1], at)? + dy,
                        coord(&row[2], at)?,
                        coord(&row[3], at)?,
                        col,
                    ]);
                }
            } else {
                if items.len() % 5 != 0 {
                    return Err(format!(
                        "draw_rects() expects five values per rect (x, y, w, h, c); got {} values",
                        items.len()
                    ));
                }
                packed.reserve(items.len());
                for (at, c) in items.chunks_exact(5).enumerate() {
                    let col = colour(state, &c[4], at)?;
                    packed.extend([
                        coord(&c[0], at)? + dx,
                        coord(&c[1], at)? + dy,
                        coord(&c[2], at)?,
                        coord(&c[3], at)?,
                        col,
                    ]);
                }
            }
        }
        other => {
            return Err(format!(
                "draw_rects() expects a list or packed array of rects, got {}",
                other.type_name()
            ));
        }
    }
    if !packed.is_empty() {
        let data = Value::F64Array(state.heap_mut().alloc_f64_array(packed));
        emit_draw(state, "rects", vec![data]);
    }
    state.push_nil();
    Ok(1)
}

/// The rects of a batch, for a host that draws [`DrawCommand::Rects`] itself
/// and wants them as a flat `x, y, w, h, rgba` run.
pub fn batch_as_ints(rects: &[BatchRect]) -> Vec<i32> {
    let mut out = Vec::with_capacity(rects.len() * 5);
    for r in rects {
        out.extend([
            r.x,
            r.y,
            r.w as i32,
            r.h as i32,
            i32::from_be_bytes([r.r, r.g, r.b, r.a]),
        ]);
    }
    out
}
