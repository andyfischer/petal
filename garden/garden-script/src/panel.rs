//! The per-frame Petal runtime behind a `panel(...)` pane.
//!
//! Where [`ScriptHost`](crate::ScriptHost) runs a layout script *once* and
//! captures a tree, a [`PanelHost`] runs its script *every frame* and drains the
//! draw commands the script emitted. The input/draw contract and the widget
//! prelude come from the upstream [`petal_ui`] crate (the standard every Petal
//! embedder shares); this host only adds Garden-specific policy — the animation
//! bookkeeping lives in `garden-app`.
//!
//! Host introspection needs no cooperation from the script: the env runs with
//! Petal's *observation* facility enabled, so every named binding the last frame
//! evaluated is readable by name through
//! [`observed_json`](PanelHost::observed_json).
//!
//! The host↔script channels are `petal-ui`'s: host→script timing/input/dimensions
//! are bound as uniforms (`bind_frame_info`/`bind_input`/`bind_dimensions`);
//! script→host draw commands flow through the `draw_commands` output buffer,
//! drained with [`petal_ui::draw::take_draw_commands`].
//!
//! [`PanelCmd`] and [`PanelInput`] are deliberately plain data (no
//! `garden-render` dependency — the cross-crate rule the [`Theme`](crate::Theme)
//! capture follows): the host speaks `u8` RGB and panel-local `i32`/`u32`
//! pixels, and `garden-app` maps that onto render primitives. `PanelCmd` is
//! Garden's projection of [`petal_ui::draw::DrawCommand`] onto the two render
//! primitives Garden rasterizes (quad + text); the offscreen-canvas commands
//! `petal-ui` also defines are not registered here, so they never appear.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use indexmap::IndexMap;
use petal::direct_manipulation::{self, ManipulationGoal};
use petal::env::Env;
use petal::heap::Heap;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::static_value::StaticValue;
use petal::value::Value;
use petal_ui::draw::DrawCommand;
use petal_ui::host_data;
use petal_ui::input::{
    self, InputState, SYM_BUTTONS_DOWN, SYM_BUTTONS_PRESSED, SYM_BUTTONS_RELEASED, SYM_CLICK_COUNT,
    SYM_DRAG_ACTIVE, SYM_DRAG_START_X, SYM_DRAG_START_Y, SYM_KEYS_DOWN, SYM_KEYS_PRESSED,
    SYM_KEYS_RELEASED, SYM_MODIFIERS, SYM_MOUSE_X, SYM_MOUSE_Y, SYM_SCROLL_X, SYM_SCROLL_Y,
    SYM_TEXT_INPUT,
};

/// Re-exported from [`petal_ui`] so the host (`garden-app`) can translate its
/// native winit/debug-server events into the standard input contract without a
/// direct `petal-ui` dependency — Garden's `garden-app → garden-script`
/// boundary owns the whole panel input/draw vocabulary.
pub use petal_ui::input::{buttons, InputEvent, Modifiers, KEY_NAMES};

/// Re-exported from [`petal_ui::host_data`]: the blessed host→script data-pull
/// channel behind the `host_data(kind, arg)` native. Garden prototyped this
/// mechanism; it was later generalized into `petal-ui`, so the host now
/// *consumes* it rather than carrying a fork. `PanelData` is the local name for
/// the upstream [`petal_ui::host_data::HostData`] value tree the host's
/// [`DataProvider`] returns (kept so `garden-app` code reads unchanged).
pub use petal_ui::host_data::{DataProvider, HostData as PanelData};

use crate::query::{self, QueryProvider};

thread_local! {
    /// Live text of each `edit_view` region on this thread, keyed by region id,
    /// published by the host for the duration of [`PanelHost::frame`] (the same
    /// swap dance as the query provider) and read by `edit_view_text(id)`. The
    /// host owns the editable buffers (the real `EditorView`s), so this is the
    /// only way the script sees the user's edits. Empty → every read returns "".
    static EDIT_VIEW_TEXTS: RefCell<HashMap<i64, String>> = RefCell::new(HashMap::new());
}

/// Install `texts` as this thread's active `edit_view` text map for the duration
/// of a frame, returning the previous map so the host can swap it back after
/// `env.run` (panic-safe, mirroring [`query::swap_query_provider`]).
fn swap_edit_view_texts(texts: HashMap<i64, String>) -> HashMap<i64, String> {
    EDIT_VIEW_TEXTS.with(|t| std::mem::replace(&mut *t.borrow_mut(), texts))
}

thread_local! {
    /// The write-back each projected `edit_view` region currently resolves to,
    /// keyed by region id — published by the host alongside [`EDIT_VIEW_TEXTS`]
    /// and read by `edit_view_edits(id)`. Only regions carrying a projection
    /// appear; everything else reads as an empty list.
    static EDIT_VIEW_EDITS: RefCell<HashMap<i64, PanelData>> = RefCell::new(HashMap::new());
}

/// Swap this thread's resolved-edit map for the duration of a frame, like
/// [`swap_edit_view_texts`].
fn swap_edit_view_edits(edits: HashMap<i64, PanelData>) -> HashMap<i64, PanelData> {
    EDIT_VIEW_EDITS.with(|t| std::mem::replace(&mut *t.borrow_mut(), edits))
}

/// Output-buffer channel carrying the script's `emit(event, arg)` events to the
/// host — the fire-and-forget script→client push channel of panel-mode GPP
/// (see [`PanelHost::take_emitted`]). The host drains it each frame and forwards
/// each event to the pane's subprocess as a GPP `emit` notification.
const EMIT_EVENTS: &str = "emit_events";

/// Output-buffer channel carrying the script's browser-history navigation
/// intents (`navigate`/`navigate_replace`/`navigate_back`/`navigate_forward`)
/// to the host — a side channel distinct from [`EMIT_EVENTS`] so a `navigate`
/// call never mixes with a fire-and-forget `emit` (see [`PanelHost::take_nav`]).
/// The host drains it each frame and drives its per-pane history stack.
const NAV_EVENTS: &str = "nav_events";

/// Output-buffer channel carrying the script's `mutate(name, arg)` requests to
/// the host — a side channel distinct from [`EMIT_EVENTS`] and [`NAV_EVENTS`].
/// Unlike `emit` (a fire-and-forget notification), a mutation is a *request*: the
/// host relays it to the pane's subprocess as a GPP `mutate` (the same transport
/// `navigate` uses under the hood), waits for the `mutateResult`, and surfaces
/// its value/error (e.g. a save's "wrote N files" / write failure) as the pane's
/// status. From the script's side it is still fire-and-forget — the reply is
/// host-surfaced, not returned to the frame. See [`PanelHost::take_mutations`].
const MUTATE_EVENTS: &str = "mutate_events";

/// Output-buffer channel carrying the script's `claim_key(key, mods)` requests —
/// the chords this panel wants delivered to it instead of being consumed by the
/// host's own shortcuts. Drained by [`PanelHost::take_key_claims`] after each
/// frame; a claim is *declarative* (re-stated by every frame it applies to), so
/// the host replaces its claim set wholesale rather than accumulating.
const KEY_CLAIMS: &str = "key_claims";

/// A browser-style history navigation intent a panel script raised this frame,
/// drained by [`PanelHost::take_nav`]. `Push`/`Replace` name the target screen
/// (a `.ptl` script the host resolves against the pane's whitelist) and the
/// argument the caller passed with it; `Back`/`Forward` move the history cursor
/// and carry no target.
///
/// The argument is the answer to "navigate to the detail screen *for this row*".
/// It is plain JSON (like a `mutate` arg, and for the same reason: a navigation
/// may cross a subprocess boundary) and `Null` when the one-argument
/// `navigate(screen)` form was used. The host stores it on the history entry, so
/// it comes back with the screen on *back* and *forward* — a restored entry that
/// lost its argument would redraw a detail screen with no subject.
#[derive(Clone, Debug, PartialEq)]
pub enum NavIntent {
    /// Navigate to `screen`, pushing a new history entry (browser link click).
    Push(String, serde_json::Value),
    /// Navigate to `screen`, replacing the current history entry (browser
    /// `location.replace` — the current entry is not kept for *back*).
    Replace(String, serde_json::Value),
    /// Move the history cursor back one entry (browser *back*); a no-op at the
    /// start of history.
    Back,
    /// Move the history cursor forward one entry (browser *forward*); a no-op at
    /// the end of history.
    Forward,
}

/// Garden's monospace advance as a fraction of the font size (size 14 → 8.4 px):
/// the metric `petal-ui`'s `text_width` native uses until the host publishes
/// measured ones with [`PanelHost::set_font_advance_ratios`]. See
/// [`petal_ui::draw::bind_text_metrics`].
const TEXT_ADVANCE_RATIO: f64 = 0.6;

/// Ring-buffer size for the per-term execution trace a traced panel records, so
/// a drag can solve computed arguments (see
/// [`PanelHost::propose_drag_edits`]). It holds one frame of a sketch — cleared
/// at the top of every frame — so this is a ceiling for a pathological frame,
/// not a working set: Petal's own default (200k events) would let one frame of
/// a busy loop reserve tens of megabytes for a question nobody asked.
const TRACE_CAPACITY: usize = 20_000;

/// One draw command produced by a panel script for a frame. Plain data: the
/// host renders it, but the values carry no GPU types. Colors are `0..=255` per
/// channel with a separate `a` (alpha, `0..=255`, 255 = opaque); coordinates are
/// panel-local logical pixels. The fields mirror [`petal_ui::draw::DrawCommand`]'s
/// renderable subset faithfully — including the alpha/`radius`/`width` fields —
/// so a Garden panel renders the full blessed draw vocabulary like every other
/// petal-ui host, rather than a truncated one.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelCmd {
    /// Fill the whole pane with one (opaque) color.
    Clear { r: u8, g: u8, b: u8 },
    /// The provenance of a projected `edit_view` region's lines — see
    /// [`ProjectionSpec`]. Declared frame state like the region itself; the host
    /// rebuilds the region's projection only when it genuinely changes.
    TextViewProjection { id: i64, spec: ProjectionSpec },
    /// A host-resolved bitmap scaled into a panel-local destination rectangle.
    Image {
        source: String,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        a: u8,
    },
    /// Filled rectangle. `radius` px rounds the corners (0 = square).
    Rect {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
        radius: u32,
    },
    /// Hollow rectangle, stroked `width` px inside its bounds. `radius` px
    /// rounds the corners (0 = square), so a rounded bordered box is one
    /// stroked frame rather than two stacked rounded fills.
    RectOutline {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
        width: u32,
        radius: u32,
    },
    /// A straight line between two endpoints, `width` px thick (1 = hairline).
    Line {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
        width: u32,
    },
    /// Filled circle centered at (`cx`, `cy`).
    Circle {
        cx: i32,
        cy: i32,
        radius: i32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    /// Filled triangle through three points.
    Triangle {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        x3: i32,
        y3: i32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    /// Filled convex polygon through `points` (panel-local pixels).
    Poly {
        points: Vec<(i32, i32)>,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    /// Filled **simple** polygon — concave allowed. Triangulated properly by
    /// the host (ear clipping), unlike the first-vertex fan of [`Poly`].
    Polygon {
        points: Vec<(i32, i32)>,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    /// Filled triangle fan from an explicit center through `points`.
    Fan {
        cx: i32,
        cy: i32,
        points: Vec<(i32, i32)>,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    /// A stroked open path, `width` px wide, with round joins and caps. One
    /// shape, so a translucent stroke composites evenly instead of darkening
    /// at every join the way N separate [`Line`]s do.
    Polyline {
        points: Vec<(i32, i32)>,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
        width: u32,
    },
    /// Filled axis-aligned ellipse with semi-axes `rx`/`ry`.
    Ellipse {
        cx: i32,
        cy: i32,
        rx: i32,
        ry: i32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    /// Hollow axis-aligned ellipse, stroked `width` px inside `rx`/`ry`.
    /// `draw_circle_outline` arrives here with `rx == ry`.
    EllipseOutline {
        cx: i32,
        cy: i32,
        rx: i32,
        ry: i32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
        width: u32,
    },
    /// A filled annular sector (the donut/pie wedge): radii `r_in`..`r_out`
    /// between angles `a0`..`a1` in radians, clockwise from +x with y down.
    Arc {
        cx: i32,
        cy: i32,
        r_in: f32,
        r_out: f32,
        a0: f32,
        a1: f32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    /// A run of text with its top-left at (`x`, `y`), in the style the script
    /// asked for. `font` names a face by role (`ui`/`mono`/`serif`) or family;
    /// Garden has one embedded face, so every name resolves to it and the
    /// field is carried for hosts that have more. `weight`/`italic` are
    /// requests the shaper answers as best it can; `spacing` (letter-spacing,
    /// px per glyph) the host applies itself.
    Text {
        text: String,
        x: i32,
        y: i32,
        size: u16,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
        font: Option<String>,
        weight: u16,
        italic: bool,
        spacing: f32,
    },
    /// Restrict subsequent draws to this sub-rect (panel-local) until the next
    /// [`ClipNone`](PanelCmd::ClipNone). The host intersects it with the pane so
    /// a script still can't paint outside its own pane. Used for scroll regions.
    Clip { x: i32, y: i32, w: u32, h: u32 },
    /// Clear any active clip; subsequent draws use the whole pane again.
    ClipNone,
    /// Declare a read-only, natively-selectable text region: the host renders a
    /// real [`EditorView`](../../garden_app/editor_view/struct.EditorView.html)
    /// (buffer-backed, with selection + system-clipboard copy) inside this
    /// panel-local rect instead of the script drawing the text by hand. `id` is
    /// stable across frames — it keys the host's per-region editor state so
    /// selection and scroll survive the per-frame script rerun. `text` is the
    /// full region contents (newline-separated lines). Emitted by the
    /// `text_view(id, x, y, w, h, text)` native as a `Host` extension command.
    ///
    /// `editable` distinguishes the read-only `text_view` (false) from the
    /// editable `edit_view` (true): the host routes real vim keystrokes into an
    /// editable region's [`EditorView`] (full editing + undo), whereas a
    /// read-only region only takes selection/scroll/copy. Both share this command
    /// and all the host's per-region state (`text_view`'s original text is the
    /// editable region's *seed* — re-declaring the same text leaves host edits
    /// intact via the content-hash gate).
    TextView {
        id: i64,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        text: String,
        editable: bool,
    },
    /// Per-line styling for the `text_view` region `id`: `styles[i]` names line
    /// `i`'s semantic style (`added`/`removed`/`hunk`/`title`/`dim`/`comment`,
    /// or `""` for plain). The host colors each whole line accordingly (a
    /// translucent band too, for the diff kinds). Emitted by the
    /// `text_view_line_styles(id, styles)` native as a `Host` extension command;
    /// a side channel so a script can add color without owning column spans.
    TextViewStyles { id: i64, styles: Vec<String> },
    /// Scroll the `text_view` region `id` so its 0-based `line` sits at the top
    /// of the viewport — programmatic navigation, as when a file list beside a
    /// diff jumps the diff to the clicked file. Emitted by the
    /// `text_view_scroll_to(id, line)` native. An **action**, not a description
    /// of the frame: the host applies it once and drops it, so a script that
    /// keeps re-declaring the region does not keep yanking the user's scroll
    /// back (emit it only on the frame the navigation happens).
    TextViewScrollTo { id: i64, line: i64 },
    /// Soft-wrap the `text_view` region `id`'s long lines to its width instead
    /// of scrolling horizontally. Emitted by the `text_view_wrap(id, wrap)`
    /// native; frame state, not an action — a region is unwrapped unless the
    /// frame that declares it also asks for wrapping.
    ///
    /// Opt-in per region because wrapping breaks a *row-aligned* pair of
    /// regions (a side-by-side before/after diff, whose projections are padded
    /// so row N means the same thing in both), while a single full-width region
    /// (a unified diff) only gains from it.
    TextViewWrap { id: i64, wrap: bool },
}

impl PanelCmd {
    /// A text command in Garden's own face — regular, upright, unspaced: what
    /// every `draw_text` meant before typography, and what a caller building
    /// one by hand almost always wants.
    pub fn plain_text(
        text: impl Into<String>,
        x: i32,
        y: i32,
        size: u16,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    ) -> PanelCmd {
        PanelCmd::Text {
            text: text.into(),
            x,
            y,
            size,
            r,
            g,
            b,
            a,
            font: None,
            weight: petal_ui::draw::REGULAR_WEIGHT,
            italic: false,
            spacing: 0.0,
        }
    }
}

impl PanelCmd {
    /// Project a `petal-ui` draw command onto Garden's render vocabulary,
    /// carrying the full field set (alpha, corner `radius`, stroke `width`).
    /// Returns `None` for commands Garden doesn't rasterize (the offscreen
    /// canvas ops and host-extension tags — none of which this host emits,
    /// since it doesn't register the canvas natives).
    fn from_draw(cmd: DrawCommand) -> Option<PanelCmd> {
        Some(match cmd {
            DrawCommand::Image {
                source,
                x,
                y,
                w,
                h,
                a,
            } => PanelCmd::Image {
                source,
                x,
                y,
                w,
                h,
                a,
            },
            DrawCommand::Clear { r, g, b } => PanelCmd::Clear { r, g, b },
            DrawCommand::Rect {
                x,
                y,
                w,
                h,
                r,
                g,
                b,
                a,
                radius,
            } => PanelCmd::Rect {
                x,
                y,
                w,
                h,
                r,
                g,
                b,
                a,
                radius,
            },
            DrawCommand::RectOutline {
                x,
                y,
                w,
                h,
                r,
                g,
                b,
                a,
                width,
                radius,
            } => PanelCmd::RectOutline {
                x,
                y,
                w,
                h,
                r,
                g,
                b,
                a,
                width,
                radius,
            },
            DrawCommand::Line {
                x1,
                y1,
                x2,
                y2,
                r,
                g,
                b,
                a,
                width,
            } => PanelCmd::Line {
                x1,
                y1,
                x2,
                y2,
                r,
                g,
                b,
                a,
                width,
            },
            DrawCommand::Circle {
                cx,
                cy,
                radius,
                r,
                g,
                b,
                a,
            } => PanelCmd::Circle {
                cx,
                cy,
                radius,
                r,
                g,
                b,
                a,
            },
            DrawCommand::Triangle {
                x1,
                y1,
                x2,
                y2,
                x3,
                y3,
                r,
                g,
                b,
                a,
            } => PanelCmd::Triangle {
                x1,
                y1,
                x2,
                y2,
                x3,
                y3,
                r,
                g,
                b,
                a,
            },
            DrawCommand::Poly { points, r, g, b, a } => PanelCmd::Poly { points, r, g, b, a },
            DrawCommand::Polygon { points, r, g, b, a } => PanelCmd::Polygon { points, r, g, b, a },
            DrawCommand::Fan {
                cx,
                cy,
                points,
                r,
                g,
                b,
                a,
            } => PanelCmd::Fan {
                cx,
                cy,
                points,
                r,
                g,
                b,
                a,
            },
            DrawCommand::Polyline {
                points,
                r,
                g,
                b,
                a,
                width,
            } => PanelCmd::Polyline {
                points,
                r,
                g,
                b,
                a,
                width,
            },
            DrawCommand::Ellipse {
                cx,
                cy,
                rx,
                ry,
                r,
                g,
                b,
                a,
            } => PanelCmd::Ellipse {
                cx,
                cy,
                rx,
                ry,
                r,
                g,
                b,
                a,
            },
            DrawCommand::EllipseOutline {
                cx,
                cy,
                rx,
                ry,
                r,
                g,
                b,
                a,
                width,
            } => PanelCmd::EllipseOutline {
                cx,
                cy,
                rx,
                ry,
                r,
                g,
                b,
                a,
                width,
            },
            DrawCommand::Arc {
                cx,
                cy,
                r_in,
                r_out,
                a0,
                a1,
                r,
                g,
                b,
                a,
            } => PanelCmd::Arc {
                cx,
                cy,
                r_in,
                r_out,
                a0,
                a1,
                r,
                g,
                b,
                a,
            },
            DrawCommand::Text {
                text,
                x,
                y,
                size,
                r,
                g,
                b,
                a,
                font,
                weight,
                italic,
                spacing,
            } => PanelCmd::Text {
                text,
                x,
                y,
                size,
                r,
                g,
                b,
                a,
                font,
                weight,
                italic,
                spacing,
            },
            DrawCommand::Clip { x, y, w, h } => PanelCmd::Clip { x, y, w, h },
            DrawCommand::ClipNone => PanelCmd::ClipNone,
            DrawCommand::CreateCanvas { .. }
            | DrawCommand::SetTarget { .. }
            | DrawCommand::DrawCanvas { .. }
            | DrawCommand::Host { .. } => return None,
        })
    }
}

/// A read-only snapshot of exactly what a panel script saw for one frame:
/// the input uniforms `petal-ui` bound into the [`Env`] after
/// [`InputState::begin_frame`]. It is *derived from* the [`InputState`] the host
/// feeds events into — a debug/introspection side channel, not an input path —
/// so the debug server can surface a panel's live input (drag, click count,
/// modifiers, pressed/released edges, typed text) at `/state`.
///
/// Level fields (`mouse_*`, `*_down`, `modifiers`, `drag_*`) persist across
/// frames; edge fields (`*_pressed`, `*_released`, `scroll_*`, `click_count`,
/// `text`) reflect just the frame that was bound.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelInput {
    pub mouse_x: i32,
    pub mouse_y: i32,
    pub keys_down: Vec<String>,
    pub keys_pressed: Vec<String>,
    pub keys_released: Vec<String>,
    pub mouse_buttons_down: Vec<i64>,
    pub mouse_buttons_pressed: Vec<i64>,
    pub mouse_buttons_released: Vec<i64>,
    /// Wheel movement this frame in whole lines/columns (positive down/right).
    pub scroll_x: i32,
    pub scroll_y: i32,
    /// Held modifier chord as a bitmask (1=shift 2=ctrl 4=alt 8=cmd).
    pub modifiers: i64,
    pub drag_active: bool,
    pub drag_start_x: i32,
    pub drag_start_y: i32,
    /// 1/2/3… for the current click chain on a left press this frame.
    pub click_count: i64,
    /// Text typed this frame (post-layout characters), read by `text_input()`.
    pub text: String,
}

/// A read-only snapshot of the host UI theme, injected into a panel script
/// each frame so a drawer can paint in the app's colors instead of hardcoding a
/// palette (read through the `panel_theme()` native — see
/// [`register_panel_natives`]).
///
/// Plain data, like [`PanelCmd`]: colors are `u8` sRGB `[r, g, b, a]` (the same
/// per-channel `0..=255` the draw natives consume, so a component drops straight
/// into `draw_rect`/`draw_text`) with no `garden-render` dependency — the host
/// (`garden-app`) owns the mapping from its `Theme` onto these stable semantic
/// keys. Colors are **not** linearized: they are bound as-is, exactly as the
/// draw commands expect (CLAUDE.md: the app speaks sRGB everywhere).
///
/// Built by the host per frame; a panel with no theme injected (a unit test, or
/// a non-Garden embedder) sees an empty record from `panel_theme()`, so scripts
/// read each key with a `?? <fallback>` default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelTheme {
    /// `(key, [r, g, b, a])` pairs (each channel `0..=255`), bound as a record
    /// of `{ r, g, b, a }` color records under `panel_theme()`. Insertion order
    /// is preserved (an `IndexMap`-backed record on the script side).
    colors: Vec<(String, [u8; 4])>,
}

impl PanelTheme {
    /// A theme with no colors — `panel_theme()` returns an empty record.
    pub fn new() -> PanelTheme {
        PanelTheme::default()
    }

    /// Set the sRGB `[r, g, b, a]` (each `0..=255`) for semantic key `key`.
    /// Chainable; a repeated key overwrites in place, keeping its position.
    pub fn set(&mut self, key: &str, rgba: [u8; 4]) -> &mut PanelTheme {
        if let Some(slot) = self.colors.iter_mut().find(|(k, _)| k == key) {
            slot.1 = rgba;
        } else {
            self.colors.push((key.to_string(), rgba));
        }
        self
    }

    /// Whether any color has been set (the host injects a populated theme; an
    /// empty one yields an empty `panel_theme()` record).
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// The sRGB `[r, g, b, a]` set for `key`, or `None` if unset (host
    /// introspection / tests).
    pub fn get(&self, key: &str) -> Option<[u8; 4]> {
        self.colors.iter().find(|(k, _)| k == key).map(|(_, c)| *c)
    }
}

/// The env binding the [`panel_theme`](native_panel_theme) native reads: the
/// current frame's [`PanelTheme`] as a record of `{ r, g, b, a }` color records.
/// Distinct from the native's own name so the two can coexist as bindings.
const PANEL_THEME_BINDING: &str = "panel_theme_data";

/// Counter symbol behind the mutation handle `mutate(name, arg)` returns. Env
/// counters live on the [`Env`], so ids stay unique across frames and across a
/// hot reload — which is exactly what a handle held in `state` needs.
const MUTATE_HANDLES: &str = "mutate_handles";

/// The env binding the [`mutate_result`](native_mutate_result) native reads: a
/// map from mutation handle to the reply the host resolved it with. Republished
/// by [`bind_mutation_results`] before every frame.
const MUTATE_RESULTS_BINDING: &str = "mutate_results_data";

/// The env binding the [`nav_arg`](native_nav_arg) native reads: the argument the
/// navigation that brought this screen up carried (`navigate(screen, arg)`).
/// Distinct from the native's own name so the two can coexist as bindings.
const NAV_ARG_BINDING: &str = "nav_arg_data";

/// File identity for change detection: (modified time, size in bytes).
type FileSig = (SystemTime, u64);

/// Hosts the Petal environment for one `panel(...)` pane.
///
/// Created with [`PanelHost::load`]; driven a frame at a time with
/// [`PanelHost::frame`]; hot-reloaded from disk with [`PanelHost::poll_reload`]
/// (preserving Petal `state` vars via `env.transfer_state`, like the layout
/// host). Not `Send`.
pub struct PanelHost {
    env: Env,
    program_id: ProgramId,
    stack_id: StackKey,
    path: PathBuf,
    /// Whether [`path`](Self::path) is a real script file to hot-reload from.
    /// False for a [`from_source`](Self::from_source) host, whose `path` is a
    /// *virtual* identity (`gpp:<cmd>`, a client binary's path, a `garden:diff?…`
    /// URI) that must never be read — see [`poll_reload`](Self::poll_reload).
    disk_backed: bool,
    last_sig: Option<FileSig>,
    output: Vec<String>,
    /// The standard input contract's accumulator: the host feeds it normalized
    /// [`InputEvent`]s as they arrive ([`input_event`](Self::input_event)) and
    /// [`frame`](Self::frame) promotes them to the per-frame edge/level snapshot
    /// scripts read through the `mouse_*`/`key_*`/`drag_*`/`text_input` natives.
    input: InputState,
    /// The last frame's bound input, for host introspection ([`input_snapshot`](Self::input_snapshot)).
    last_input: PanelInput,
    /// The host UI theme injected into each frame (read by `panel_theme()`).
    /// Set by the host with [`set_theme`](Self::set_theme) before every frame;
    /// empty until then, so `panel_theme()` returns an empty record.
    theme: PanelTheme,
    /// Replies the host has resolved, keyed by the handle `mutate(...)` returned
    /// — read back by the script as `mutate_result(handle)`.
    ///
    /// A mutation is fire-and-forget *from inside the frame* (the round trip to a
    /// subprocess must not block a redraw), so the only way a drawer can tell a
    /// save that worked from one that failed — and the only way a test can assert
    /// on either — is to keep the handle in `state` and read the reply on a later
    /// frame. Entries are kept until [`forget_mutation_result`] is called, so a
    /// script that never asks does not grow the map without bound in the one way
    /// that matters: [`set_mutation_result`] caps it.
    mutation_results: HashMap<i64, serde_json::Value>,
    /// The argument the navigation that opened this screen carried
    /// (`navigate(screen, arg)`), read back by the script as `nav_arg()`.
    /// `Null` for a screen reached without one — including a panel's origin
    /// screen, which nothing navigated to.
    ///
    /// The host owns the history stack, so it is the host that re-publishes this
    /// on *back* and *forward* ([`set_nav_arg`](Self::set_nav_arg)): the value
    /// belongs to the history entry, not to the screen identity, which is what
    /// makes returning to a detail screen show the same subject it did before.
    nav_arg: serde_json::Value,
    /// Host-side data source behind the `host_data(kind, arg)` native; without
    /// one the native answers nil. Installed into [`DATA_PROVIDER`] for the
    /// duration of each [`frame`](Self::frame).
    provider: Option<DataProvider>,
    /// Host-side async data source behind the `query(kind, arg)` /
    /// `invalidate(kind, arg)` natives (Garden's React-Query prototype on Petal's
    /// pending values); without one, `query` answers a loading `Pending`.
    /// Installed into the query channel for the duration of each
    /// [`frame`](Self::frame), the same way `provider` is.
    query_provider: Option<Box<dyn QueryProvider>>,
    /// Live text of each `edit_view` region, keyed by region id — the host
    /// publishes the current buffer contents here each tick
    /// ([`set_edit_view_texts`](Self::set_edit_view_texts)) so `edit_view_text(id)`
    /// can read the user's edits back. Swapped into [`EDIT_VIEW_TEXTS`] for the
    /// duration of each [`frame`](Self::frame), like the providers.
    edit_view_texts: HashMap<i64, String>,
    /// The write-back each projected `edit_view` region resolves to, published
    /// by the host each tick ([`set_edit_view_edits`](Self::set_edit_view_edits))
    /// and read by `edit_view_edits(id)`. Swapped into [`EDIT_VIEW_EDITS`] for
    /// the duration of each [`frame`](Self::frame), like the texts.
    edit_view_edits: HashMap<i64, PanelData>,
    /// Monotonic origin for the `time()`/`elapsed()` clock published each frame.
    /// Read fresh (`start.elapsed()`) rather than accumulated from `dt`, so
    /// `elapsed()` does not drift.
    start: Instant,
    /// Whether this host records which call site drew each command
    /// ([`set_trace_origins`](Self::set_trace_origins)). Off by default, so an
    /// ordinary panel pays nothing; Petal IDE turns it on to map the canvas back
    /// to the source beside it.
    trace_origins: bool,
    /// This panel's persistent key/value store, behind the `panel_store_get` /
    /// `panel_store_set` natives — scoped to the script's own path, so a panel
    /// remembers its todos across a restart without any file API. Installed
    /// into the store channel for the duration of each [`frame`](Self::frame)
    /// (the same swap as the providers) and flushed to disk after it, so a
    /// write costs a rewrite only on the frames that actually change something.
    store: Option<crate::panel_store::PanelStore>,
    /// The call site of each command the last [`frame`](Self::frame) returned,
    /// index-aligned with that command list. Empty while tracing is off.
    ///
    /// Held as raw ids rather than resolved spans because resolving is the
    /// expensive half and almost none of it is ever needed: a frame draws
    /// hundreds of shapes and a mouse asks about one. [`trace_origin`](Self::trace_origin)
    /// does that work on demand.
    frame_origins: Vec<crate::panel_trace::DrawOrigin>,
}

impl std::fmt::Debug for PanelHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelHost")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl PanelHost {
    /// Compile + prepare the panel script at `path`. Does not run a frame yet —
    /// the caller binds dimensions and calls [`frame`](Self::frame).
    pub fn load(path: &Path) -> Result<PanelHost, String> {
        let source = fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;

        let mut env = new_panel_env();
        let program_id = env.load_program(&source)?;
        let stack_id = env.create_stack(program_id)?;

        Ok(PanelHost {
            env,
            program_id,
            stack_id,
            path: path.to_path_buf(),
            disk_backed: true,
            last_sig: stat_sig(path),
            output: Vec::new(),
            input: InputState::new(),
            last_input: PanelInput::default(),
            theme: PanelTheme::default(),
            nav_arg: serde_json::Value::Null,
            mutation_results: HashMap::new(),
            provider: None,
            query_provider: None,
            edit_view_texts: HashMap::new(),
            edit_view_edits: HashMap::new(),
            start: Instant::now(),
            trace_origins: false,
            store: Some(crate::panel_store::PanelStore::for_script(path)),
            frame_origins: Vec::new(),
        })
    }

    /// A **placeholder** host for a script that does not compile: an empty
    /// program that draws nothing, but which keeps `path` as a real disk-backed
    /// script with *no* recorded signature. The next
    /// [`poll_reload`](Self::poll_reload) therefore re-reads the file, so a panel
    /// whose script was broken at load time comes alive by itself the moment the
    /// file is fixed — no restart. Fails only if the empty program itself won't
    /// compile (i.e. never in practice).
    pub fn stub(path: &Path) -> Result<PanelHost, String> {
        let mut host = PanelHost::from_source(&path.to_string_lossy(), "")?;
        host.path = path.to_path_buf();
        host.disk_backed = true;
        host.last_sig = None;
        Ok(host)
    }

    /// Compile + prepare a panel from an **in-memory** source string rather than a
    /// file. `name` is a stable virtual identity (not a real path) used for
    /// `Debug` and, in Garden, as the pane's `script` string — the built-in diff
    /// viewer passes a `garden:diff?…` URI so no throwaway `.ptl` is ever written.
    ///
    /// Because there is no backing file, [`poll_reload`](Self::poll_reload)
    /// permanently no-ops: a source-backed panel never hot-reloads (its source is
    /// re-pushed, or its data refreshed through its query provider, rather than
    /// read from disk). This holds even when `name` happens to name a real file —
    /// a panel-mode GPP pane's `name` is its *client binary*, and reading that as
    /// Petal source is exactly the bug the `disk_backed` flag prevents.
    /// Otherwise identical to [`load`](Self::load) — the
    /// same natives, program, and stack.
    pub fn from_source(name: &str, source: &str) -> Result<PanelHost, String> {
        let mut env = new_panel_env();
        let program_id = env.load_program(source)?;
        let stack_id = env.create_stack(program_id)?;

        Ok(PanelHost {
            env,
            program_id,
            stack_id,
            path: PathBuf::from(name),
            disk_backed: false,
            last_sig: None,
            output: Vec::new(),
            input: InputState::new(),
            last_input: PanelInput::default(),
            theme: PanelTheme::default(),
            nav_arg: serde_json::Value::Null,
            mutation_results: HashMap::new(),
            provider: None,
            query_provider: None,
            edit_view_texts: HashMap::new(),
            edit_view_edits: HashMap::new(),
            start: Instant::now(),
            trace_origins: false,
            // A source-backed panel has no script file, but it does have a
            // stable virtual identity (`gpp:<cmd>`, a `garden:…` URI), which is
            // exactly the scoping key the store wants — so a GPP app's drawer
            // persists under its own name like any other panel.
            store: Some(crate::panel_store::PanelStore::for_script(Path::new(name))),
            frame_origins: Vec::new(),
        })
    }

    /// Publish the advance ratios (glyph advance ÷ font size, indexed by
    /// codepoint) measured from the face this host's panes are actually drawn
    /// with, so the script's `text_width` agrees with what the renderer shapes.
    /// Replaces the [`TEXT_ADVANCE_RATIO`] estimate every host starts with,
    /// which stays the fallback for codepoints past the table's end.
    ///
    /// Per host, not per process: the measurement belongs to whoever will draw
    /// the pane, so an embedder with a different font — or a test with a made-up
    /// table — sets its own without disturbing anyone else's.
    ///
    /// Call before the first [`frame`](Self::frame); the ratios survive
    /// [`poll_reload`](Self::poll_reload) (which recompiles into the same env),
    /// but a host rebuilt from scratch is a new host and must be told again.
    pub fn set_font_advance_ratios(&mut self, ratios: Vec<f64>) {
        let metrics = petal_ui::draw::FontMetrics::proportional(ratios, TEXT_ADVANCE_RATIO);
        bind_font_advances(&mut self.env, &metrics, &metrics);
    }

    /// [`set_font_advance_ratios`](Self::set_font_advance_ratios) for a host
    /// that renders **two** faces: the default/`mono` table and a separate one
    /// for the `ui` role a script selects with `font: "ui"`.
    ///
    /// Publishing both is what keeps `text_width(s, size, "ui")` agreeing with
    /// what the renderer shapes. A host that draws a proportional `ui` face but
    /// publishes only the monospace table will measure every UI run with
    /// monospace advances — centered and right-aligned text then lands visibly
    /// wrong, and nothing about the drawing looks broken, so it is a hard bug
    /// to see. Same contract as the single-face setter: call before the first
    /// [`frame`](Self::frame), and tell every rebuilt host again.
    pub fn set_font_advance_ratios_with_ui(&mut self, mono: Vec<f64>, ui: Vec<f64>) {
        let mono = petal_ui::draw::FontMetrics::proportional(mono, TEXT_ADVANCE_RATIO);
        let ui = petal_ui::draw::FontMetrics::proportional(ui, TEXT_ADVANCE_RATIO);
        bind_font_advances(&mut self.env, &mono, &ui);
    }

    /// Attach the host-side data source the script reaches through
    /// `host_data(kind, arg)`. A panel without one (the common case) sees nil.
    pub fn set_data_provider(&mut self, provider: DataProvider) {
        self.provider = Some(provider);
    }

    /// Publish the current text of each `edit_view` region (id → buffer text) so
    /// the next frame's `edit_view_text(id)` reads the user's edits back. The host
    /// owns the editable buffers, so it calls this each tick before
    /// [`frame`](Self::frame) with the live contents. Cheap (stores the map;
    /// bound into the thread-local for the run in `frame`).
    pub fn set_edit_view_texts(&mut self, texts: HashMap<i64, String>) {
        self.edit_view_texts = texts;
    }

    /// Publish what each projected `edit_view` region currently resolves to
    /// (id → the list of `{source, start, end, lines}` write-backs), so the next
    /// frame's `edit_view_edits(id)` can hand it to `mutate(...)`. The host
    /// computes these from the region's projection; regions without one are
    /// absent.
    pub fn set_edit_view_edits(&mut self, edits: HashMap<i64, PanelData>) {
        self.edit_view_edits = edits;
    }

    /// Whether a data provider is attached (host introspection).
    pub fn has_data_provider(&self) -> bool {
        self.provider.is_some()
    }

    /// Attach the host-side async data source the script reaches through
    /// `query(kind, arg)` / `invalidate(kind, arg)`. A panel without one sees a
    /// perpetual loading `Pending` from every `query`.
    pub fn set_query_provider(&mut self, provider: Box<dyn QueryProvider>) {
        self.query_provider = Some(provider);
    }

    /// Whether a query provider is attached (host introspection).
    pub fn has_query_provider(&self) -> bool {
        self.query_provider.is_some()
    }

    /// Feed one normalized host input event into the standard input contract.
    /// Level state (mouse position, held keys/buttons, modifiers, an in-progress
    /// drag) updates immediately; edges (pressed/released/scroll/text/click
    /// count) accumulate until the next [`frame`](Self::frame). Cheap; the host
    /// calls it as events arrive between ticks.
    pub fn input_event(&mut self, ev: InputEvent) {
        self.input.event(ev);
    }

    /// Path of the watched panel script.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bind the pane's current size (panel-local pixel space). Cheap; the host
    /// calls it each tick so a resize just changes the bound numbers.
    pub fn set_dimensions(&mut self, width: i32, height: i32) {
        petal_ui::input::bind_dimensions(&mut self.env, width, height);
    }

    /// Set the host UI theme injected into each frame — read by the script
    /// through `panel_theme()`. Read-only per-frame input, like the mouse
    /// position: [`frame`](Self::frame) binds it into the env before running, so
    /// a live theme change (a `POST /theme`) is reflected on the next frame. The
    /// host calls this each tick before [`frame`](Self::frame). Cheap (stores the
    /// snapshot; the record is built at frame time).
    pub fn set_theme(&mut self, theme: PanelTheme) {
        self.theme = theme;
    }

    /// Publish the argument this screen was navigated to with, read back by the
    /// script as `nav_arg()`. Set by the host from the live history entry before
    /// the next frame — on the navigation itself and again on every *back* /
    /// *forward* onto that entry.
    pub fn set_nav_arg(&mut self, arg: serde_json::Value) {
        self.nav_arg = arg;
    }

    /// Run one frame: advance the input clock, bind timing + input, re-run the
    /// script from the top (preserving `state`), and return the draw commands it
    /// emitted. On a script runtime error the previous frame's commands are *not*
    /// returned — the caller keeps its last good frame and surfaces the error.
    ///
    /// [`InputState::begin_frame`] promotes the events fed since the last frame
    /// into this frame's edge snapshot; `dt` also advances the multi-click clock,
    /// so double/triple clicks are derived here rather than at the host boundary.
    pub fn frame(&mut self, dt: f64, frame_count: i64) -> Result<Vec<PanelCmd>, String> {
        self.input.begin_frame(dt);
        input::bind_frame_info(&mut self.env, dt, frame_count);
        input::bind_time(&mut self.env, self.start.elapsed().as_secs_f64());
        input::bind_input(&mut self.env, &self.input);
        bind_panel_theme(&mut self.env, &self.theme);
        bind_host_palette(&mut self.env, &self.theme);
        bind_nav_arg(&mut self.env, &self.nav_arg);
        bind_mutation_results(&mut self.env, &self.mutation_results);
        self.last_input = self.snapshot_input();

        // Discard any stale buffered commands + emitted events, then re-run.
        // (The observation buffer needs no clearing here: `env.run` clears it
        // itself, so it always holds exactly this frame's bindings.)
        petal_ui::draw::clear_draw_commands(&mut self.env);
        // The per-term trace answers questions about *this* frame only (what
        // was `bh` when that bar was drawn?), so it starts each frame empty
        // rather than accumulating a 60-per-second history nobody reads.
        if self.trace_origins {
            self.env.trace_mut().clear();
        }
        let sym = self.env.intern_symbol(EMIT_EVENTS);
        self.env.clear_output_buffer(sym);
        let sym = self.env.intern_symbol(NAV_EVENTS);
        self.env.clear_output_buffer(sym);
        let sym = self.env.intern_symbol(MUTATE_EVENTS);
        self.env.clear_output_buffer(sym);
        let sym = self.env.intern_symbol(KEY_CLAIMS);
        self.env.clear_output_buffer(sym);
        self.env.reset_stack(self.stack_id)?;
        // Make the data + query providers reachable from their natives for the
        // duration of the run, then reclaim them (with any cache they updated,
        // even on a script error) by swapping the saved values back in.
        let saved = host_data::swap_data_provider(self.provider.take());
        let saved_q = query::swap_query_provider(self.query_provider.take());
        let saved_e = swap_edit_view_texts(std::mem::take(&mut self.edit_view_texts));
        let saved_ee = swap_edit_view_edits(std::mem::take(&mut self.edit_view_edits));
        // The persistent store is reachable only while this panel's own frame
        // runs, so `panel_store_get`/`_set` can never touch another script's.
        let saved_store = crate::panel_store::swap_store(self.store.take());
        let run_result = self.env.run(self.stack_id);
        self.store = crate::panel_store::swap_store(saved_store);
        // Persist whatever the frame changed. A failure (read-only home, full
        // disk) is reported to the script's output rather than failing the
        // frame: the panel keeps drawing, with its in-memory store intact.
        if let Some(Err(err)) = self.store.as_mut().map(|s| s.flush()) {
            self.output.push(format!("[panel store] {err}"));
        }
        self.edit_view_edits = swap_edit_view_edits(saved_ee);
        self.edit_view_texts = swap_edit_view_texts(saved_e);
        self.query_provider = query::swap_query_provider(saved_q);
        self.provider = host_data::swap_data_provider(saved);
        self.output.append(&mut self.env.take_output());
        run_result?;

        // Decode the frame's draw commands. Most map straight onto Garden's
        // render vocabulary via `from_draw`; the `text_view` host-extension
        // command carries a heap-backed string, so it's decoded here where the
        // Env's heap is still live (`from_draw` is heap-free by design).
        //
        // While tracing is on, each raw command also carries the call site that
        // drew it; the two are pushed together so `frame_origins[i]` describes
        // `cmds[i]` even though several raw commands decode to nothing.
        let raw = petal_ui::draw::take_draw_commands_traced(&mut self.env);
        let heap = self.env.heap();
        let mut cmds = Vec::with_capacity(raw.len());
        let mut origins = Vec::new();
        if self.trace_origins {
            origins.reserve(raw.len());
        }
        for (cmd, origin) in raw {
            let decoded = match cmd {
                DrawCommand::Host { tag, data } if tag == "text_view" => {
                    decode_text_view(&data, heap, false)
                }
                DrawCommand::Host { tag, data } if tag == "edit_view" => {
                    decode_text_view(&data, heap, true)
                }
                DrawCommand::Host { tag, data } if tag == "edit_view_projection" => {
                    decode_projection(&data, heap)
                }
                DrawCommand::Host { tag, data } if tag == "text_view_styles" => {
                    decode_text_view_styles(&data, heap)
                }
                DrawCommand::Host { tag, data } if tag == "text_view_scroll_to" => {
                    match (data.first().and_then(num), data.get(1).and_then(num)) {
                        (Some(id), Some(line)) => Some(PanelCmd::TextViewScrollTo { id, line }),
                        _ => None,
                    }
                }
                DrawCommand::Host { tag, data } if tag == "text_view_wrap" => {
                    match (data.first().and_then(num), data.get(1).and_then(num)) {
                        (Some(id), Some(wrap)) => Some(PanelCmd::TextViewWrap {
                            id,
                            wrap: wrap != 0,
                        }),
                        _ => None,
                    }
                }
                other => PanelCmd::from_draw(other),
            };
            if let Some(pc) = decoded {
                cmds.push(pc);
                if self.trace_origins {
                    origins.push(crate::panel_trace::DrawOrigin(origin));
                }
            }
        }
        self.frame_origins = origins;
        Ok(cmds)
    }

    /// Poll the script file (mtime + size). On change: recompile, hot-reload
    /// (preserving Petal `state` vars), and report `Ok(true)`. On a compile
    /// error the running program is left untouched and `Err` is returned once.
    ///
    /// A [`from_source`](Self::from_source) host has no script file — its `path`
    /// is a virtual identity — so this is an unconditional `Ok(false)` for one:
    /// never a stat, never a read, whatever that name resolves to on disk.
    pub fn poll_reload(&mut self) -> Result<bool, String> {
        if !self.disk_backed {
            return Ok(false);
        }
        let sig = match stat_sig(&self.path) {
            Some(sig) => sig,
            None => {
                if self.last_sig.take().is_some() {
                    return Err(format!("cannot stat {}", self.path.display()));
                }
                return Ok(false);
            }
        };
        if self.last_sig == Some(sig) {
            return Ok(false);
        }
        self.last_sig = Some(sig);

        let source = fs::read_to_string(&self.path)
            .map_err(|e| format!("failed to read {}: {}", self.path.display(), e))?;
        let new_program = self.env.compile_program(self.program_id, &source)?;
        self.env.transfer_state(self.stack_id, new_program)?;
        Ok(true)
    }

    /// Recompile the panel from an **in-memory** source string, preserving Petal
    /// `state` vars (`env.transfer_state`, exactly like [`poll_reload`](Self::poll_reload)
    /// — but the source comes from a live editor buffer rather than disk). This
    /// is the recompile half of the Petal-IDE live binding: the editor pane's
    /// current text drives the paired panel without a save round-trip. On a
    /// compile error the running program is left untouched and `Err` is returned,
    /// so the panel keeps its last good render. Does not touch `last_sig`, so a
    /// later disk [`poll_reload`](Self::poll_reload) of an identical save is a
    /// harmless re-transfer, and a divergent on-disk edit still reloads.
    pub fn reload_source(&mut self, source: &str) -> Result<(), String> {
        let new_program = self.env.compile_program(self.program_id, source)?;
        self.env.transfer_state(self.stack_id, new_program)?;
        Ok(())
    }

    /// Drain collected script `print` output since the last call.
    pub fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.output)
    }

    /// Everything the last frame *bound*, as a JSON map keyed by name — the host
    /// introspection channel (surfaced by the debug server's `/state`, and how
    /// tests assert an interactive panel's logical state — selection, scroll
    /// offset, hit rectangles — without decoding pixels).
    ///
    /// The script does nothing to participate: Petal records the last value
    /// bound to every named term while it runs, so a plain `let sel = …` is
    /// already readable. Three properties worth knowing (the full story is in
    /// Petal's `Env::get_observations_json`):
    ///
    /// - **Keys are function-qualified.** A top-level `let sel` keys as `sel`;
    ///   the same binding inside `fn list_row` keys as `list_row.sel`. Only
    ///   function bodies qualify, so a `let` inside a top-level `if` or loop
    ///   still keys under its plain name.
    /// - **Last write wins.** A name bound in a loop, or in a function called
    ///   repeatedly, reports its *final* value for the frame, not its history.
    /// - **Per frame, not cumulative.** [`frame`](Self::frame)'s `env.run`
    ///   clears the buffer as it starts, so this is exactly the frame just run —
    ///   with one deliberate exception: a frame that never ran (a compile error
    ///   in [`reload_source`](Self::reload_source), say) clears nothing, so the
    ///   last good frame's values remain readable alongside its last good render.
    ///   A name whose binding didn't execute this frame is absent rather than
    ///   null.
    ///
    /// Values are full JSON — ints, strings, bools, lists, records — including
    /// the script's `state` variables, which are named terms like any other.
    /// Unlike [`state_json`](Self::state_json), this is the *live* value from the
    /// run rather than the state committed after it.
    ///
    /// # What this drops
    /// Petal observes every named term the program contains, and a panel program
    /// contains far more than the panel script: `petal-ui`'s prelude is compiled
    /// in as the implicitly-imported `ui` module, so an unfiltered map is mostly
    /// *its* bindings — 109 keys for `examples/panels/sketch.ptl`, of which 14
    /// are the script's. Nobody reading a panel's state wants to page past the
    /// widget library to find `sel`, so three rules prune it here, where every
    /// Garden consumer (`/state`, the `:State` overlay, tests) benefits:
    ///
    /// - **a key containing `::`** — module-qualified, so imported rather than
    ///   bound by this script (`ui::button`, `std::sum`);
    /// - **a key starting with `_`** — the convention for a binding that is
    ///   plumbing, not API: the prelude's `_native_rect` aliases and `_MENU_PAD`
    ///   constants, which land unprefixed because only *functions* qualify a name;
    /// - **a callable value** (`"<function>"` / `"<native>"`) — no reader wants
    ///   "this name holds a function" reported as an observation, and it is what
    ///   removes the prelude's ~20 unprefixed implicit-import aliases
    ///   (`draw_rect`, `list_update`, `rect`, …).
    ///
    /// One prelude binding survives all three by construction: `theme`, its
    /// `export let theme = { … }` palette record. It is left in rather than
    /// name-blocked, because a script that binds its own `theme` overwrites the
    /// key anyway (its term is later in program order, and later wins) — so the
    /// key is never *wrong*, only occasionally not-yours.
    ///
    /// This is Garden policy, not Petal's; the unfiltered map is a call to
    /// `Env::get_observations_json` away.
    pub fn observed_json(&self) -> serde_json::Map<String, serde_json::Value> {
        self.env
            .get_observations_json(self.program_id, self.stack_id)
            .into_iter()
            .filter(|(k, v)| !k.contains("::") && !k.starts_with('_') && !is_callable_json(v))
            .collect()
    }

    /// The script's live `state` variables as a JSON map keyed by name — the
    /// persistent state committed after the last run, for the Petal-IDE state
    /// inspector. Narrower than [`observed_json`](Self::observed_json), which
    /// reports every named binding the last frame evaluated: this is only the
    /// declared `state`, and it survives a frame that never ran at all.
    pub fn state_json(&self) -> serde_json::Map<String, serde_json::Value> {
        self.env.get_state_json(self.program_id, self.stack_id)
    }

    /// Drain the `(event, arg)` events the last frame published through
    /// `emit(event, arg)` — the fire-and-forget script→client push channel of
    /// panel-mode GPP. Call order is preserved; each arg is the script value
    /// converted to a JSON tree (the shape the GPP `EmitParams` `arg` carries).
    /// Call after [`frame`](Self::frame); the buffer is cleared at the start of
    /// the next frame, so untaken events never leak across frames.
    pub fn take_emitted(&mut self) -> Vec<(String, serde_json::Value)> {
        let sym = self.env.intern_symbol(EMIT_EVENTS);
        let values = self.env.take_output_buffer(sym);
        let heap = self.env.heap();
        let mut out = Vec::with_capacity(values.len());
        for v in &values {
            if let Value::EnumVariant { tag, data } = v {
                let event = heap.get_string(*tag).to_string();
                let arg = heap
                    .get_list(*data)
                    .first()
                    .map(|v| value_to_json(heap, v))
                    .unwrap_or(serde_json::Value::Null);
                out.push((event, arg));
            }
        }
        out
    }

    /// Drain the `(name, arg)` mutation requests the last frame published through
    /// `mutate(name, arg)` — the effectful, response-carrying script→subprocess
    /// channel. Same shape as [`take_emitted`](Self::take_emitted) (name + a
    /// JSON arg), but a distinct buffer so a `mutate` never mixes with an `emit`.
    /// The host relays each to the subprocess and surfaces the reply as status.
    /// Call after [`frame`](Self::frame).
    pub fn take_mutations(&mut self) -> Vec<(String, serde_json::Value, i64)> {
        let sym = self.env.intern_symbol(MUTATE_EVENTS);
        let values = self.env.take_output_buffer(sym);
        let heap = self.env.heap();
        let mut out = Vec::with_capacity(values.len());
        for v in &values {
            if let Value::EnumVariant { tag, data } = v {
                let name = heap.get_string(*tag).to_string();
                let items = heap.get_list(*data);
                let arg = items
                    .first()
                    .map(|v| value_to_json(heap, v))
                    .unwrap_or(serde_json::Value::Null);
                // The handle the native returned to the script, so the host can
                // report this request's reply back under the same number.
                let handle = match items.get(1) {
                    Some(Value::Int(n)) => *n,
                    _ => 0,
                };
                out.push((name, arg, handle));
            }
        }
        out
    }

    /// Record the reply a mutation resolved to, under the handle `mutate(...)`
    /// returned to the script. Read back by `mutate_result(handle)` on the next
    /// frame.
    ///
    /// `result` is the handle record the script sees verbatim — see
    /// [`mutation_reply`] for the shape the host builds.
    pub fn set_mutation_result(&mut self, handle: i64, result: serde_json::Value) {
        // A drawer that fires mutations and never reads their replies (the
        // ordinary fire-and-forget use) must not accumulate them forever. The
        // cap is generous enough that no realistic in-flight set is evicted.
        const MAX_KEPT: usize = 256;
        if self.mutation_results.len() >= MAX_KEPT {
            if let Some(&oldest) = self.mutation_results.keys().min() {
                self.mutation_results.remove(&oldest);
            }
        }
        self.mutation_results.insert(handle, result);
    }

    /// The reply recorded for `handle`, if any — the host-side read of what
    /// `mutate_result(handle)` would answer.
    pub fn mutation_result(&self, handle: i64) -> Option<&serde_json::Value> {
        self.mutation_results.get(&handle)
    }

    /// Drain the key chords the last frame claimed with `claim_key(key, mods)`,
    /// as `(canonical key name, modifier bits)` — `1=shift 2=ctrl 4=alt 8=cmd`,
    /// the same encoding `modifiers` uses. `None` for the bits means "this key
    /// under **any** chord" (the one-argument `claim_key("z")` form).
    ///
    /// The host consults this before applying its own shortcut to a key, which
    /// is the only way a panel gets a command keyspace of its own: a panel whose
    /// bare letters are content (a spreadsheet, a console) otherwise has none,
    /// because every Cmd/Ctrl chord belongs to the editor around it.
    /// Call after [`frame`](Self::frame).
    pub fn take_key_claims(&mut self) -> Vec<(String, Option<u8>)> {
        let sym = self.env.intern_symbol(KEY_CLAIMS);
        let values = self.env.take_output_buffer(sym);
        let heap = self.env.heap();
        let mut out = Vec::with_capacity(values.len());
        for v in &values {
            if let Value::EnumVariant { tag, data } = v {
                let key = heap.get_string(*tag).to_ascii_lowercase();
                let mods = match heap.get_list(*data).first() {
                    Some(Value::Int(bits)) => Some(*bits as u8),
                    _ => None,
                };
                out.push((key, mods));
            }
        }
        out
    }

    /// Drain the browser-history navigation intents the last frame published
    /// through `navigate`/`navigate_replace`/`navigate_back`/`navigate_forward`.
    /// Call order is preserved; each decodes back into a [`NavIntent`]. A
    /// separate channel from [`take_emitted`](Self::take_emitted), so draining
    /// nav intents never swallows `emit` events (and vice versa).
    /// Call after [`frame`](Self::frame); the buffer is cleared at the start of
    /// the next frame, so untaken intents never leak across frames.
    pub fn take_nav(&mut self) -> Vec<NavIntent> {
        let sym = self.env.intern_symbol(NAV_EVENTS);
        let values = self.env.take_output_buffer(sym);
        let heap = self.env.heap();
        let mut out = Vec::with_capacity(values.len());
        for v in &values {
            if let Value::EnumVariant { tag, data } = v {
                let kind = heap.get_string(*tag);
                let screen = || {
                    heap.get_list(*data).first().and_then(|v| match v {
                        Value::String(sid) => Some(heap.get_string(*sid).to_string()),
                        _ => None,
                    })
                };
                // The optional second element is `navigate(screen, arg)`'s
                // argument; absent for the one-argument form.
                let arg = || {
                    heap.get_list(*data)
                        .get(1)
                        .map(|v| value_to_json(heap, v))
                        .unwrap_or(serde_json::Value::Null)
                };
                let intent = match kind {
                    "push" => screen().map(|s| NavIntent::Push(s, arg())),
                    "replace" => screen().map(|s| NavIntent::Replace(s, arg())),
                    "back" => Some(NavIntent::Back),
                    "forward" => Some(NavIntent::Forward),
                    _ => None,
                };
                if let Some(intent) = intent {
                    out.push(intent);
                }
            }
        }
        out
    }

    /// Restore a batch of `state` variables from a JSON map (as produced by
    /// [`state_json`](Self::state_json)) into this host's running stack, keyed by
    /// top-level variable name. Returns the number of keys successfully applied;
    /// unknown or unrepresentable keys are skipped so a partially-compatible
    /// screen still restores what it can (the browser-history restore path).
    ///
    /// Must be called **before the first frame** of the target screen: pre-seeding
    /// the state slot makes `Inst::StateInit` skip the `state x = 0` init block, so
    /// the first frame observes the restored value. Running a frame first would let
    /// the init clobber it.
    pub fn restore_state(&mut self, map: &serde_json::Map<String, serde_json::Value>) -> usize {
        self.env
            .set_state_map_from_json(self.program_id, self.stack_id, map)
    }

    /// The input snapshot delivered to the last [`frame`](Self::frame) — exactly
    /// the uniforms the script read — for host introspection (the debug server
    /// surfaces it so an interactive panel's input is assertable in tests).
    pub fn input_snapshot(&self) -> &PanelInput {
        &self.last_input
    }

    // ── Source tracing (direct manipulation) ──────────────────────────────

    /// Record which call site drew each command, so the rendered canvas can be
    /// traced back to the source that produced it. See [`crate::panel_trace`].
    ///
    /// Off by default: a panel that nobody is going to point at should not pay
    /// even the one id per draw call that this costs. Petal IDE turns it on for
    /// the canvas it pairs with an editor.
    pub fn set_trace_origins(&mut self, on: bool) {
        self.trace_origins = on;
        self.env.enable_emit_trace(on);
        // The per-term trace is the other half of direct manipulation: solving a
        // *computed* argument (`base_y - bh`) for one of its leaves needs the
        // value the other leaf actually had, and only the run knows that. It is
        // bounded to one frame's worth of events (cleared at the top of every
        // frame), so this costs a fixed-size ring, not a growing log.
        self.env.trace_mut().enabled = on;
        self.env.trace_mut().set_capacity(TRACE_CAPACITY);
        if !on {
            self.frame_origins.clear();
            self.env.trace_mut().clear();
        }
    }

    /// The call site recorded for the command at `index` of the last frame, or
    /// `None` if tracing is off, the index is out of range, or the runtime could
    /// not attribute that command.
    pub fn origin_at(&self, index: usize) -> Option<&crate::panel_trace::DrawOrigin> {
        self.frame_origins.get(index).filter(|o| !o.is_empty())
    }

    /// Resolve a recorded call site to source: the span of the drawing call and
    /// of each of its arguments (see [`DrawTrace`](crate::DrawTrace)).
    ///
    /// `None` when the origin does not belong to the program now loaded — which
    /// is what a live reload leaves behind, and the case that must not be
    /// guessed at: term ids are indices, so a stale one would resolve happily to
    /// unrelated code.
    pub fn trace_origin(
        &self,
        origin: &crate::panel_trace::DrawOrigin,
    ) -> Option<crate::panel_trace::DrawTrace> {
        let program = self.env.get_program(self.program_id)?;
        crate::panel_trace::DrawTrace::resolve(program, origin)
    }

    /// **The write-back half of direct manipulation.** State what the shape at
    /// `cmd_index` of the last frame *should* look like — "argument 0 of the
    /// call that drew it should evaluate to 148" — and get the source edits
    /// that make it so.
    ///
    /// `goals` are `(argument index, new value)` pairs, resolved as one batch so
    /// a gesture that moves x and y together can't produce two edits that
    /// contradict each other. The caller never says *what text to change*:
    /// which literal moves is the runtime's answer, and for a computed argument
    /// it is solved (`x + offset` reaching 42 by moving `offset`) against the
    /// values the traced run actually saw.
    ///
    /// Ambiguity is narrowed by the **source itself**: a sketch that declares
    /// its knobs with `config let` pins everything else, so a bare drag comes
    /// back with one proposal per goal. When a goal still resolves several ways
    /// this takes the first — most direct first — and reports the rest of the
    /// story through `shared` / `variable`, which is what a host renders as
    /// "this moved `edge`, and three shapes read it".
    ///
    /// The gesture is addressed by *command index* rather than by call, because
    /// one call can draw many shapes and the difference matters: see
    /// [`DragOutcome::Refused`](crate::DragOutcome::Refused) and the loop rule
    /// below.
    pub fn propose_drag_edits(
        &self,
        cmd_index: usize,
        goals: &[(usize, f64)],
    ) -> crate::panel_trace::DragOutcome {
        use crate::panel_trace::{ArgSource, DragOutcome};

        let Some(program) = self.env.get_program(self.program_id) else {
            return DragOutcome::Stale;
        };
        let Some(origin) = self.origin_at(cmd_index) else {
            return DragOutcome::Stale;
        };
        let Some(trace) = crate::panel_trace::DrawTrace::resolve(program, origin) else {
            return DragOutcome::Stale;
        };

        // Solving a *computed* argument inverts against the values the run
        // recorded — and the trace answers with the **last** value each term
        // took. For a call inside a loop that is the last iteration's value, so
        // the numbers only describe the shape that iteration drew: solving the
        // third of four bars against the fourth's loop counter produces a
        // confident, wrong edit. The last shape a call emitted is exactly the
        // one whose values are still current, so that one is solvable and the
        // rest say why they are not.
        let solving = goals.iter().any(|&(i, _)| {
            trace
                .args
                .get(i)
                .is_some_and(|a| a.source == ArgSource::Computed)
        });
        if solving {
            let siblings = self.frame_origins.iter().filter(|o| *o == origin).count();
            let last = self.frame_origins.iter().rposition(|o| o == origin);
            if siblings > 1 && last != Some(cmd_index) {
                return DragOutcome::Refused(format!(
                    "one of {siblings} shapes drawn by this call, and its position is computed — only the last one can be solved",
                ));
            }
        }

        let goals: Vec<ManipulationGoal> = goals
            .iter()
            .map(|&(arg_index, value)| ManipulationGoal {
                term: trace.call_ref.0,
                arg_index,
                // A drag lands on a whole pixel, and `Float` with no fractional
                // part still renders as `12` where the source wrote `12` —
                // Petal preserves the spelling it found.
                new_value: StaticValue::Float(value.round()),
            })
            .collect();
        let per_goal = match direct_manipulation::propose_edits_batch(
            program,
            &goals,
            Some(self.env.trace()),
            &HashMap::new(),
        ) {
            Ok(p) => p,
            // Addressing a program that isn't running: the same "discard it"
            // case a stale origin is.
            Err(_) => return DragOutcome::Stale,
        };

        let mut out = Vec::new();
        for proposals in per_goal {
            // Most direct first: the batch has already dropped anything that
            // can't hold together with the rest of the gesture.
            let Some(p) = proposals.into_iter().next() else {
                continue;
            };
            let Some(span) = crate::panel_trace::CodeSpan::from_petal(&p.edit.span) else {
                continue;
            };
            out.push(crate::panel_trace::SourceRewrite {
                span,
                new_text: p.edit.new_text,
                variable: p.variable,
                shared: p.shared,
                config: p.config,
                description: p.description,
            });
        }
        if out.is_empty() {
            return DragOutcome::Refused(
                "this shape's position isn't directly editable — nothing to move".to_string(),
            );
        }
        DragOutcome::Edits(out)
    }

    /// Read the input uniforms `petal-ui` just bound back out of the [`Env`] into
    /// a plain [`PanelInput`]. Reading the bound values (rather than the
    /// [`InputState`]'s private fields) keeps the snapshot faithful to what the
    /// script actually saw and needs no accessors upstream.
    fn snapshot_input(&mut self) -> PanelInput {
        PanelInput {
            mouse_x: read_int(&mut self.env, SYM_MOUSE_X) as i32,
            mouse_y: read_int(&mut self.env, SYM_MOUSE_Y) as i32,
            keys_down: read_str_list(&mut self.env, SYM_KEYS_DOWN),
            keys_pressed: read_str_list(&mut self.env, SYM_KEYS_PRESSED),
            keys_released: read_str_list(&mut self.env, SYM_KEYS_RELEASED),
            mouse_buttons_down: read_int_list(&mut self.env, SYM_BUTTONS_DOWN),
            mouse_buttons_pressed: read_int_list(&mut self.env, SYM_BUTTONS_PRESSED),
            mouse_buttons_released: read_int_list(&mut self.env, SYM_BUTTONS_RELEASED),
            scroll_x: read_int(&mut self.env, SYM_SCROLL_X) as i32,
            scroll_y: read_int(&mut self.env, SYM_SCROLL_Y) as i32,
            modifiers: read_int(&mut self.env, SYM_MODIFIERS),
            drag_active: read_int(&mut self.env, SYM_DRAG_ACTIVE) != 0,
            drag_start_x: read_int(&mut self.env, SYM_DRAG_START_X) as i32,
            drag_start_y: read_int(&mut self.env, SYM_DRAG_START_Y) as i32,
            click_count: read_int(&mut self.env, SYM_CLICK_COUNT),
            text: read_str(&mut self.env, SYM_TEXT_INPUT),
        }
    }
}

/// Read an int (or float, truncated) binding back out of the env; 0 if unbound.
fn read_int(env: &mut Env, name: &str) -> i64 {
    let s = env.intern_symbol(name);
    match env.binding(s) {
        Some(Value::Int(n)) => n,
        Some(Value::Float(f)) => f as i64,
        _ => 0,
    }
}

/// Read a string binding back out of the env; empty if unbound.
fn read_str(env: &mut Env, name: &str) -> String {
    let s = env.intern_symbol(name);
    match env.binding(s) {
        Some(Value::String(id)) => env.heap().get_string(id).to_string(),
        _ => String::new(),
    }
}

/// Read a `List` of strings back out of the env; empty if unbound.
fn read_str_list(env: &mut Env, name: &str) -> Vec<String> {
    let s = env.intern_symbol(name);
    match env.binding(s) {
        Some(Value::List(id)) => {
            let heap = env.heap();
            heap.get_list(id)
                .iter()
                .filter_map(|v| match v {
                    Value::String(sid) => Some(heap.get_string(*sid).to_string()),
                    _ => None,
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Read a `List` of ints back out of the env; empty if unbound.
fn read_int_list(env: &mut Env, name: &str) -> Vec<i64> {
    let s = env.intern_symbol(name);
    match env.binding(s) {
        Some(Value::List(id)) => env
            .heap()
            .get_list(id)
            .iter()
            .filter_map(|v| match v {
                Value::Int(n) => Some(*n),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Decode a `text_view(id, x, y, w, h, text)` host-extension command's raw args
/// (`[Int id, Int x, Int y, Int w, Int h, String text]`) into a
/// [`PanelCmd::TextView`]. The trailing `text` is a heap string id, so this
/// runs where the Env's heap is borrowable. Returns `None` on a malformed arg
/// list (wrong arity or a non-string text arg).
fn decode_text_view(data: &[Value], heap: &Heap, editable: bool) -> Option<PanelCmd> {
    let id = num(data.first()?)?;
    let x = num(data.get(1)?)? as i32;
    let y = num(data.get(2)?)? as i32;
    let w = num(data.get(3)?)? as u32;
    let h = num(data.get(4)?)? as u32;
    let text = match data.get(5)? {
        Value::String(sid) => heap.get_string(*sid).to_string(),
        _ => return None,
    };
    Some(PanelCmd::TextView {
        id,
        x,
        y,
        w,
        h,
        text,
        editable,
    })
}

/// Decode a `text_view_line_styles(id, styles)` host-extension command's raw
/// args (`[Int id, List<String> styles]`) into a [`PanelCmd::TextViewStyles`].
/// Non-string list entries decode as `""` (plain), so a stray value styles its
/// line plainly rather than dropping the whole command.
fn decode_text_view_styles(data: &[Value], heap: &Heap) -> Option<PanelCmd> {
    let id = num(data.first()?)?;
    let list_id = match data.get(1)? {
        Value::List(id) => *id,
        _ => return None,
    };
    let styles = heap
        .get_list(list_id)
        .iter()
        .map(|v| match v {
            Value::String(sid) => heap.get_string(*sid).to_string(),
            _ => String::new(),
        })
        .collect();
    Some(PanelCmd::TextViewStyles { id, styles })
}

/// Convert a Petal [`Value`] into the JSON tree a GPP `emit` notification
/// carries — the reverse of [`crate::query`]'s `data_to_value` projection.
/// Nil/bool/int/float/string map directly; lists become arrays and records
/// (maps) become objects, recursively. Values with no JSON projection
/// (closures, handles, pending values, …) degrade to `null` rather than
/// failing the frame.
fn value_to_json(heap: &Heap, v: &Value) -> serde_json::Value {
    match v {
        Value::Nil => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(n) => serde_json::Value::from(*n),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(id) => serde_json::Value::String(heap.get_string(*id).to_string()),
        Value::List(id) => serde_json::Value::Array(
            heap.get_list(*id)
                .iter()
                .map(|v| value_to_json(heap, v))
                .collect(),
        ),
        Value::Map(id) => serde_json::Value::Object(
            heap.get_map(*id)
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json(heap, v)))
                .collect(),
        ),
        _ => serde_json::Value::Null,
    }
}

/// Read a numeric `Value` (int or float) as i64.
/// Decode an `edit_view_projection` host command: `[id, spec]`, where `spec` is
/// the Petal record described by [`ProjectionSpec`]. Routed through the JSON
/// projection rather than walked by hand — the shape is a tree of lists and
/// strings, and `value_to_json` already knows how to flatten one.
fn decode_projection(data: &[Value], heap: &Heap) -> Option<PanelCmd> {
    let id = num(data.first()?)?;
    let json = value_to_json(heap, data.get(1)?);
    let ints = |key: &str| -> Vec<i64> {
        json[key]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_i64().unwrap_or(-1)).collect())
            .unwrap_or_default()
    };
    let strings = |key: &str| -> Vec<String> {
        json[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    let d = &json["decor"];
    let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    Some(PanelCmd::TextViewProjection {
        id,
        spec: ProjectionSpec {
            sources: strings("sources"),
            span_source: ints("span_source"),
            span_start: ints("span_start"),
            span_end: ints("span_end"),
            span_group: ints("span_group"),
            kinds: text(&json["kinds"]),
            line_spans: ints("line_spans"),
            styles: strings("styles"),
            decor: DecorSpec {
                same: text(&d["same"]),
                added: text(&d["added"]),
                removed: text(&d["removed"]),
                same_style: text(&d["same_style"]),
                added_style: text(&d["added_style"]),
                removed_style: text(&d["removed_style"]),
                diff_markers: d["diff_markers"].as_bool().unwrap_or(false),
                gutter: d["gutter"].as_bool().unwrap_or(false),
            },
        },
    })
}

fn num(v: &Value) -> Option<i64> {
    match v {
        Value::Int(n) => Some(*n),
        Value::Float(f) => Some(*f as i64),
        _ => None,
    }
}

/// Where each line of a projected `edit_view` region came from, in the flat,
/// parallel-array form a Petal drawer can cheaply build — decoded by `garden-app`
/// into a `garden_core::projection::Projection`.
///
/// Plain data with no dependency on `garden-core` (`garden-script` sits beside
/// it, not above it — see the crate map in `docs/architecture.md`), so this is a
/// wire shape rather than the model itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectionSpec {
    /// Opaque source names the write-backs are addressed to (file paths, for the
    /// diff reviewer). Never interpreted here.
    pub sources: Vec<String>,
    /// Per span: which source it writes to, the `[start, end)` line range of
    /// that source it replaces, and an optional grouping key (`-1` = none) so
    /// several spans can be reverted together.
    pub span_source: Vec<i64>,
    pub span_start: Vec<i64>,
    pub span_end: Vec<i64>,
    pub span_group: Vec<i64>,
    /// One character per projected line, naming its origin:
    ///
    /// | char | origin |
    /// |---|---|
    /// | `' '` | content the source holds unchanged |
    /// | `'+'` | content added relative to the base |
    /// | `'-'` | content the base held and the source dropped |
    /// | `'c'` | chrome — inert decoration |
    /// | `'l'` | chrome, locked: deleting it is refused |
    /// | `'h'` | chrome heading its span: deleting it reverts the span |
    /// | `'g'` | chrome heading its group: deleting it reverts every span in it |
    pub kinds: String,
    /// The span each line belongs to (`-1` = none), parallel to `kinds`.
    pub line_spans: Vec<i64>,
    /// The semantic style name each line is painted with, parallel to `kinds`.
    pub styles: Vec<String>,
    pub decor: DecorSpec,
}

/// How a projection's lines are decorated, so the host can strip the decoration
/// when folding back and restore it when a reverted line changes meaning.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecorSpec {
    pub same: String,
    pub added: String,
    pub removed: String,
    pub same_style: String,
    pub added_style: String,
    pub removed_style: String,
    /// Read a freshly typed line as a diff line (a leading `+`/`-`/space is a
    /// marker) rather than literally. Ignored under `gutter`, where no line's
    /// text carries a marker to read.
    pub diff_markers: bool,
    /// Draw the three markers in a **gutter** beside the text instead of at the
    /// head of each line, leaving the buffer holding the sources' own text.
    /// This is what makes a projected diff edit like a file: no buffer
    /// operation — a join, a column selection, a search — has to step around a
    /// marker, because there is no marker in the text to step around.
    pub gutter: bool,
}

/// Best-effort (mtime, size) signature for change detection.
fn stat_sig(path: &Path) -> Option<FileSig> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Bind `metrics` as the text measurement every `text_width` call in this env
/// reads: the uniform ratio (the fallback for codepoints past the table), the
/// per-codepoint advance table when there is one, and both font roles a script
/// can name. `mono` and `ui` resolve to the same entry because Garden renders
/// everything in its one embedded face — so `text_width(s, size, "ui")`
/// measures the font the pane will actually get, and an unknown name degrades
/// to the same default. All four are plain rebindable bindings, so calling this
/// again on a live env replaces the measurement wholesale.
fn bind_font_advances(
    env: &mut Env,
    mono: &petal_ui::draw::FontMetrics,
    ui: &petal_ui::draw::FontMetrics,
) {
    // The *unnamed* default metrics stay the mono face: a script that measures
    // without naming a role is measuring what Garden draws by default.
    petal_ui::draw::bind_text_metrics(env, mono.advance);
    if !mono.advances.is_empty() {
        petal_ui::draw::bind_text_advance_table(env, &mono.advances);
    }
    petal_ui::draw::bind_font_metrics(env, "mono", mono);
    // `ui` is a genuinely different face (proportional Inter) whenever the host
    // has measured one. A host that hasn't passes the mono table for both, which
    // is the old single-face behavior — measuring stays consistent with drawing
    // either way, which is the property that matters.
    petal_ui::draw::bind_font_metrics(env, "ui", ui);
    // Garden's default font *is* the mono role, so a style naming no face
    // resolves that role's variants.
    petal_ui::draw::bind_default_font_name(env, "mono");
}

/// Does this observed value hold a function? Petal's JSON dump has no callable
/// type, so a closure, an overload set, and a native all render as the same two
/// placeholder strings — matching them is the only test available, and a script
/// binding one of those exact strings as data is a miss worth accepting.
fn is_callable_json(v: &serde_json::Value) -> bool {
    matches!(v.as_str(), Some("<function>") | Some("<native>"))
}

/// A fresh env prepared for panel work: the panel natives, plus observation
/// switched on.
///
/// Every [`PanelHost`] construction path goes through here, because a host whose
/// env forgot to `enable()` looks identical to a script that bound nothing —
/// [`observed_json`](PanelHost::observed_json) would silently answer an empty
/// map rather than fail. Observation is cheap enough to leave on for the whole
/// session (one bool check per instruction when recording, one slot per named
/// term), which is why the host pays for it unconditionally instead of asking
/// the embedder to opt in.
fn new_panel_env() -> Env {
    let mut env = Env::new();
    register_panel_natives(&mut env);
    env.observations_mut().enable();
    env
}

// ── Panel natives ─────────────────────────────────────────────────────────
// The drawing surface + input a panel script sees comes wholesale from
// `petal-ui`: the standard input/timing natives, the draw-command vocabulary,
// and the `ui` prelude module (widgets, hit-testing, `text_width`) as an
// implicit import. Garden adds only its own host channels (theme/palette,
// `emit`/`mutate`/`navigate`, the text-view regions) and binds its monospace
// text metric; introspection needs no native at all, since the host reads the
// script's bindings straight out of the observation buffer. Garden does not
// register the offscreen canvas natives — it has no render targets — so those
// commands never appear.

fn register_panel_natives(env: &mut Env) {
    petal_ui::input::register_input(env);
    petal_ui::draw::register_draw(env);
    // Text metrics: the uniform ratio is the floor every host starts from. A
    // host that can measure its real face replaces it per instance with
    // [`PanelHost::set_font_advance_ratios`], which rebinds these same symbols.
    let floor = petal_ui::draw::FontMetrics::monospace(TEXT_ADVANCE_RATIO);
    bind_font_advances(env, &floor, &floor);
    // Garden-only: the host UI theme, injected read-only each frame (see
    // [`bind_panel_theme`]) so a drawer paints in the app's colors instead of a
    // hardcoded palette. Its record is bound before the run; the native returns
    // it (or an empty record when no theme was injected).
    env.register_native("panel_theme", native_panel_theme);
    // The always-complete companion: the full active palette (host theme overlaid
    // on a built-in fallback), the shared pattern every panel-mode GPP app reads.
    env.register_native("palette", native_palette);
    // The fire-and-forget script→client push channel of panel-mode GPP: the
    // host drains each frame's events ([`PanelHost::take_emitted`]) and
    // forwards them to the pane's subprocess as `emit` notifications.
    env.register_native("emit", native_emit);
    // Garden-only: request an effectful action from the pane's subprocess
    // (`on_mutation`), with the reply host-surfaced as status. See [`native_mutate`].
    env.register_native("mutate", native_mutate);
    // The reading half: what the mutation identified by a handle resolved to.
    env.register_native("mutate_result", native_mutate_result);
    // The browser-history navigation API: each raises a typed `NavIntent` into
    // the `nav_events` side channel that the host drains ([`PanelHost::take_nav`])
    // to drive its per-pane history stack.
    // `claim_key(key, mods)` — the panel's own command keyspace: chords the host
    // must forward instead of consuming (drained by `take_key_claims`).
    env.register_native("claim_key", native_claim_key);
    // The panel's own persistent key/value store, scoped to its script path —
    // the answer to "a todo app remembers your todos" without handing a sketch
    // a file API. See [`crate::panel_store`].
    crate::panel_store::register_store(env);
    env.register_native("navigate", native_navigate);
    // Read back the argument the navigation that opened this screen carried.
    env.register_native("nav_arg", native_nav_arg);
    env.register_native("navigate_replace", native_navigate_replace);
    env.register_native("navigate_back", native_navigate_back);
    env.register_native("navigate_forward", native_navigate_forward);
    // Garden-only: declare a natively-selectable read-only text region. Emitted
    // as a `Host` extension command; the host (`garden-app`) renders a real
    // `EditorView` there. See [`PanelCmd::TextView`].
    env.register_native("text_view", native_text_view);
    // Garden-only: the editable sibling of `text_view` — the host routes vim
    // keystrokes into its `EditorView`. See [`PanelCmd::TextView`] (`editable`).
    env.register_native("edit_view", native_edit_view);
    // Read an `edit_view` region's live (post-edit) buffer text back into the
    // script, so a drawer can assemble a save payload. Host-bound each frame.
    env.register_native("edit_view_text", native_edit_view_text);
    // Declare an `edit_view` region's projection — where each of its lines came
    // from — so the host can fold the user's edits back into the sources.
    env.register_native("edit_view_projection", native_edit_view_projection);
    // Read back what those edits currently resolve to, ready to hand a subprocess.
    env.register_native("edit_view_edits", native_edit_view_edits);
    // The line-styling side channel for a `text_view` region.
    env.register_native("text_view_line_styles", native_text_view_line_styles);
    env.register_native("text_view_scroll_to", native_text_view_scroll_to);
    // Per-region soft-wrap opt-in. See [`PanelCmd::TextViewWrap`].
    env.register_native("text_view_wrap", native_text_view_wrap);
    // The `host_data(kind, arg)` pull channel is petal-ui's blessed contract
    // (Garden's prototype, generalized upstream) — register it, don't fork it.
    host_data::register_host_data(env);
    // The `query`/`invalidate` async channel — Garden's React-Query prototype on
    // Petal's pending values (a future upstream `petal-query`), see [`crate::query`].
    query::register_query(env);
    petal_ui::register_prelude(env);
}

/// Bind the current host [`PanelTheme`] into the env as the record
/// `panel_theme()` returns: `{ key: { r, g, b, a }, … }`, each channel an int
/// `0..=255` (sRGB, unlinearized — the draw natives consume the same units).
/// Called by [`PanelHost::frame`] before the run, so a live theme change lands
/// on the next frame like any other per-frame input.
fn bind_panel_theme(env: &mut Env, theme: &PanelTheme) {
    let mut record: IndexMap<String, Value> = IndexMap::with_capacity(theme.colors.len());
    for (key, [r, g, b, a]) in &theme.colors {
        let mut color: IndexMap<String, Value> = IndexMap::with_capacity(4);
        color.insert("r".to_string(), Value::Int(*r as i64));
        color.insert("g".to_string(), Value::Int(*g as i64));
        color.insert("b".to_string(), Value::Int(*b as i64));
        color.insert("a".to_string(), Value::Int(*a as i64));
        let color_id = env.heap_mut().alloc_map(color);
        record.insert(key.clone(), Value::Map(color_id));
    }
    let record_id = env.heap_mut().alloc_map(record);
    let sym = env.intern_symbol(PANEL_THEME_BINDING);
    env.set_binding(sym, Value::Map(record_id));
}

/// Publish the *resolved* palette (host theme overlaid on
/// [`FALLBACK_PALETTE`], extra host keys carried through — the same resolution
/// [`native_palette`] performs) through petal-ui's host-palette binding, so
/// `ui_theme()` — and with it every prelude widget — paints in Garden's colors
/// without the drawer calling `theme_set` at all. Bound each frame beside
/// [`bind_panel_theme`], so a live theme change repaints prelude widgets on
/// the next frame like everything else.
fn bind_host_palette(env: &mut Env, theme: &PanelTheme) {
    let mut colors: Vec<(&str, [u8; 4])> =
        Vec::with_capacity(FALLBACK_PALETTE.len() + theme.colors.len());
    for (key, rgba) in FALLBACK_PALETTE {
        colors.push((key, theme.get(key).unwrap_or(*rgba)));
    }
    for (key, rgba) in &theme.colors {
        if !FALLBACK_PALETTE.iter().any(|(k, _)| *k == key.as_str()) {
            colors.push((key.as_str(), *rgba));
        }
    }
    petal_ui::input::bind_host_palette(env, &colors);
}

/// Bind the resolved mutation replies for `mutate_result(handle)` to read, as a
/// record keyed by the handle's decimal spelling (Petal record keys are strings).
///
/// Rebuilt each frame from the host's table rather than mutated in place, so a
/// reply that arrived between two frames is visible on the very next one.
fn bind_mutation_results(env: &mut Env, results: &HashMap<i64, serde_json::Value>) {
    let mut record: IndexMap<String, Value> = IndexMap::with_capacity(results.len());
    for (handle, reply) in results {
        let value = petal::value::json_to_value(reply, env.heap_mut()).unwrap_or(Value::Nil);
        record.insert(handle.to_string(), value);
    }
    let id = env.heap_mut().alloc_map(record);
    let sym = env.intern_symbol(MUTATE_RESULTS_BINDING);
    env.set_binding(sym, Value::Map(id));
}

/// The handle record a resolved mutation reads back as: `ok` plus the reply
/// `value`, or `ok: false` and the `error` that refused it. One shape for both
/// outcomes so a drawer can branch on `ok` without testing for absent keys.
pub fn mutation_reply(result: Result<Option<String>, String>) -> serde_json::Value {
    match result {
        Ok(value) => serde_json::json!({
            "ok": true,
            "value": value,
            "error": serde_json::Value::Null,
        }),
        Err(error) => serde_json::json!({
            "ok": false,
            "value": serde_json::Value::Null,
            "error": error,
        }),
    }
}

/// Bind this frame's navigation argument for `nav_arg()` to read.
///
/// Called by [`PanelHost::frame`] alongside [`bind_panel_theme`], so a screen
/// restored by *back* sees its own argument on its very first frame rather than
/// one frame late — a detail screen that read nil first would draw an empty
/// subject before correcting itself.
///
/// A value that will not convert (nothing in practice — it arrived as JSON)
/// binds as nil rather than failing the frame.
fn bind_nav_arg(env: &mut Env, arg: &serde_json::Value) {
    let value = petal::value::json_to_value(arg, env.heap_mut()).unwrap_or(Value::Nil);
    let sym = env.intern_symbol(NAV_ARG_BINDING);
    env.set_binding(sym, value);
}

/// `nav_arg()` — the argument the navigation that opened this screen carried
/// (`navigate("detail.ptl", { id: 7 })` → `{ id: 7 }` here).
///
/// Returns nil for a screen reached without one, including a panel's origin
/// screen, so the idiomatic read is `nav_arg() ?? <default>`. The value is stored
/// on the history entry, so it is still here after *back* and *forward*.
fn native_nav_arg(cxt: &mut PetalCxt) -> NativeResult {
    let v = cxt.binding_named(NAV_ARG_BINDING);
    cxt.push_value(v);
    Ok(1)
}

/// `panel_theme()` — the host UI theme for this frame, as a record of `{ r, g,
/// b, a }` color records keyed by semantic name (`bg`, `panel`, `text`,
/// `text_dim`, `accent`, `green`, …). Read-only per-frame input the host
/// injects (see [`bind_panel_theme`]); returns an empty record when no theme was
/// injected (a non-Garden embedder or a bare unit test), so scripts read each
/// key with a `?? <fallback>` default. Components are sRGB `0..=255`, ready to
/// drop into `draw_rect`/`draw_text`.
fn native_panel_theme(cxt: &mut PetalCxt) -> NativeResult {
    match cxt.binding_named(PANEL_THEME_BINDING) {
        v @ Value::Map(_) => cxt.push_value(v),
        _ => {
            let id = cxt.heap_mut().alloc_map(IndexMap::new());
            cxt.push_value(Value::Map(id));
        }
    }
    Ok(1)
}

/// The complete canonical palette, as an sRGB fallback used when the host
/// injects no theme (a non-Garden embedder or a bare unit test). A Garden host
/// projects every one of these keys from its live [`Theme`]
/// (`garden_app::theme::Theme::to_panel_theme`); this table is the coherent
/// GitHub-dark default that stands in when it doesn't, so [`palette`] always
/// returns a complete, readable set. Order defines the returned record's order.
const FALLBACK_PALETTE: &[(&str, [u8; 4])] = &[
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

/// Allocate one `{ r, g, b, a }` color record (each channel an int `0..=255`) —
/// the same shape [`bind_panel_theme`] binds and the draw natives consume.
fn alloc_color(heap: &mut Heap, [r, g, b, a]: [u8; 4]) -> Value {
    let mut color: IndexMap<String, Value> = IndexMap::with_capacity(4);
    color.insert("r".to_string(), Value::Int(r as i64));
    color.insert("g".to_string(), Value::Int(g as i64));
    color.insert("b".to_string(), Value::Int(b as i64));
    color.insert("a".to_string(), Value::Int(a as i64));
    Value::Map(heap.alloc_map(color))
}

/// `palette()` — the always-complete companion to [`panel_theme`](native_panel_theme):
/// the full active color scheme as a record of `{ r, g, b, a }` sRGB color
/// records, keyed by semantic name (`window_bg`, `panel`, `text`, `text_dim`,
/// `accent`, `green`, `added_bg`, `hunk_bg`, …). Unlike `panel_theme()` — which
/// returns exactly what the host injected, empty for a bare embedder — `palette()`
/// starts from the built-in [`FALLBACK_PALETTE`] and overlays whatever the host
/// injected on top, so **every canonical key always resolves**. A drawer reads
/// `palette().<key>` unconditionally, with no `?? default` guard and no hardcoded
/// fallback of its own. This is the pattern every GPP panel app shares, so they
/// all paint in one consistent per-scheme palette (see `docs/writing-gpp-apps.md`).
fn native_palette(cxt: &mut PetalCxt) -> NativeResult {
    // The host's injected theme (a Garden host supplies a full palette; a bare
    // embedder supplies nothing, leaving the fallback to stand for every key).
    let injected: IndexMap<String, Value> = match cxt.binding_named(PANEL_THEME_BINDING) {
        Value::Map(id) => cxt.heap().get_map(id).clone(),
        _ => IndexMap::new(),
    };
    let mut out: IndexMap<String, Value> = IndexMap::with_capacity(FALLBACK_PALETTE.len());
    for (key, rgba) in FALLBACK_PALETTE {
        let color = injected
            .get(*key)
            .copied()
            .unwrap_or_else(|| alloc_color(cxt.heap_mut(), *rgba));
        out.insert((*key).to_string(), color);
    }
    // Carry through any host-injected keys the fallback doesn't name, so the
    // host can add semantic colors without this table gating them out.
    for (key, v) in &injected {
        out.entry(key.clone()).or_insert(*v);
    }
    let id = cxt.heap_mut().alloc_map(out);
    cxt.push_value(Value::Map(id));
    Ok(1)
}

/// `emit(event, arg)` — publish a fire-and-forget `(event, arg)` signal into the
/// [`EMIT_EVENTS`] side channel for the host to forward to the pane's GPP
/// subprocess (see [`PanelHost::take_emitted`]). `event` is a string naming the
/// intent; `arg` is any JSON-serializable value (string/int/float/bool/nil/
/// list/record). Emits no draw command, expects no reply, and returns nil;
/// multiple calls per frame are delivered in order. In a panel with no attached
/// subprocess (an in-process `panel(...)` script) the events are dropped.
fn native_emit(cxt: &mut PetalCxt) -> NativeResult {
    let event = cxt.get_string(1)?;
    let arg = cxt.get_value(2)?;
    let sym = cxt.intern_symbol(EMIT_EVENTS);
    cxt.emit(sym, &event, vec![arg]);
    cxt.push_nil();
    Ok(1)
}

/// `mutate(name, arg)` — request an effectful, response-carrying action from the
/// pane's subprocess (its `on_mutation(name, …)` handler): e.g. `mutate("save",
/// {text: edit_view_text(1)})` to write edited files. `arg` is any
/// JSON-serializable value. The host relays it over GPP `mutate`, awaits the
/// `mutateResult`, and surfaces its value (success) or error as the pane's
/// status — so the reply is *not* returned to the frame (fire-and-forget from the
/// script). Emits no draw command, returns nil. Dropped in an in-process
/// `panel(...)` with no subprocess. See [`PanelHost::take_mutations`].
fn native_mutate(cxt: &mut PetalCxt) -> NativeResult {
    let name = cxt.get_string(1)?;
    let arg = cxt.get_value(2)?;
    // The handle: unique for the life of the env, so a script may keep it in
    // `state` across frames and ask for the reply later.
    let counter = cxt.intern_symbol(MUTATE_HANDLES);
    let handle = cxt.next_counter(counter) as i64 + 1;
    let sym = cxt.intern_symbol(MUTATE_EVENTS);
    cxt.emit(sym, &name, vec![arg, Value::Int(handle)]);
    cxt.push_int(handle);
    Ok(1)
}

/// `mutate_result(handle)` — the reply the mutation `handle` identifies resolved
/// to, or nil while it is still in flight (and for a handle that was never
/// issued).
///
/// A resolved reply is a record: `{ ok: bool, value: <reply>, error: <string> }`
/// — `ok` false with `error` set when the host or the client refused it. The
/// idiomatic use keeps the handle in `state`, since the frame that *makes* the
/// request cannot also see its answer:
///
/// ```text
/// state saving = 0
/// if pressed then
///   saving = mutate("save", {text: cur})
/// end
/// let saved = mutate_result(saving)
/// ```
fn native_mutate_result(cxt: &mut PetalCxt) -> NativeResult {
    let handle = cxt.get_int(1)?;
    let results = cxt.binding_named(MUTATE_RESULTS_BINDING);
    let found = match results {
        Value::Map(id) => cxt.heap().get_map(id).get(&handle.to_string()).copied(),
        _ => None,
    };
    cxt.push_value(found.unwrap_or(Value::Nil));
    Ok(1)
}

/// `claim_key(key)` / `claim_key(key, mods)` — ask the host to deliver a chord
/// to this panel instead of applying its own shortcut to it.
///
/// `key` is a canonical key name (`"z"`, `"space"`, `"return"`, `"left"`, …).
/// `mods` is the chord, either a string like `"cmd"` / `"cmd+shift"` /
/// `"alt"` (`option`, `super`, `meta`, `control` are accepted spellings) or the
/// integer bitmask `1=shift 2=ctrl 4=alt 8=cmd`. With no `mods`, the key is
/// claimed under **every** chord, including no modifier at all.
///
/// A claim lasts for the frame that made it, so declare it unconditionally at
/// the top of the script rather than inside a branch. Quit (`Cmd`/`Ctrl`+`Q`)
/// cannot be claimed. Emits no draw command, returns nil; in a host that does
/// not honor claims it is simply ignored.
fn native_claim_key(cxt: &mut PetalCxt) -> NativeResult {
    let key = cxt.get_string(1)?;
    let mods = if cxt.arg_count() >= 2 {
        let v = cxt.get_value(2)?;
        Some(match v {
            Value::Int(bits) => bits,
            Value::String(id) => parse_mod_bits(cxt.heap().get_string(id))? as i64,
            other => {
                return Err(format!(
                    "claim_key() expects a modifier string or bitmask, got {}",
                    other.type_name()
                ));
            }
        })
    } else {
        None
    };
    let sym = cxt.intern_symbol(KEY_CLAIMS);
    let data = mods.map(Value::Int).into_iter().collect();
    cxt.emit(sym, &key, data);
    cxt.push_nil();
    Ok(1)
}

/// Parse `claim_key`'s modifier spelling (`"cmd"`, `"cmd+shift"`, `"alt"`, …)
/// into the `1=shift 2=ctrl 4=alt 8=cmd` bitmask. An unknown name is an error
/// rather than a silently-ignored zero — a typo'd claim that quietly never
/// fires is exactly the failure this whole API exists to end.
fn parse_mod_bits(spec: &str) -> Result<u8, String> {
    let mut bits = 0u8;
    for part in spec.split(['+', '-', ' ']).filter(|p| !p.is_empty()) {
        bits |= match part.to_ascii_lowercase().as_str() {
            "shift" => 1,
            "ctrl" | "control" => 2,
            "alt" | "option" | "opt" => 4,
            "cmd" | "command" | "super" | "meta" => 8,
            other => {
                return Err(format!(
                    "claim_key(): unknown modifier {other:?} \
                     (want shift/ctrl/alt/cmd, e.g. \"cmd+shift\")"
                ));
            }
        };
    }
    Ok(bits)
}

/// `navigate(screen)` / `navigate(screen, arg)` — raise a browser-style *push*
/// navigation intent to `screen` (a `.ptl` script the host resolves against the
/// pane's whitelist), like clicking a link: the current screen's `state` is saved
/// and a new history entry is pushed.
///
/// `arg` is the subject the target screen is *for* — the row that was clicked,
/// the commit to show. The target reads it back with `nav_arg()`, and it is
/// stored on the history entry, so *back* and *forward* return to that screen
/// with the same argument rather than an empty one. Any value that survives the
/// JSON round trip may be passed (see [`NavIntent`]).
///
/// Fire-and-forget; the host acts on the drained [`NavIntent::Push`] (see
/// [`PanelHost::take_nav`]). Emits no draw command, returns nil. In a panel with
/// no host history (a bare unit test) it is dropped.
fn native_navigate(cxt: &mut PetalCxt) -> NativeResult {
    nav_emit(cxt, "push")
}

/// `navigate_replace(screen)` / `navigate_replace(screen, arg)` — raise a
/// browser-style *replace* navigation intent to `screen` (browser
/// `location.replace`): the current history entry is overwritten rather than kept,
/// so *back* skips it. `arg` behaves exactly as in [`native_navigate`].
/// Fire-and-forget; the host acts on the drained [`NavIntent::Replace`]. Emits no
/// draw command, returns nil.
fn native_navigate_replace(cxt: &mut PetalCxt) -> NativeResult {
    nav_emit(cxt, "replace")
}

/// The shared body of `navigate` / `navigate_replace`: emit `kind` into the nav
/// side channel carrying the screen name and, when the two-argument form was
/// used, the caller's argument.
fn nav_emit(cxt: &mut PetalCxt, kind: &str) -> NativeResult {
    let screen = cxt.get_string(1)?;
    let arg = if cxt.arg_count() >= 2 {
        Some(cxt.get_value(2)?)
    } else {
        None
    };
    let screen_id = cxt.heap_mut().alloc_string(screen);
    let sym = cxt.intern_symbol(NAV_EVENTS);
    let mut data = vec![Value::String(screen_id)];
    data.extend(arg);
    cxt.emit(sym, kind, data);
    cxt.push_nil();
    Ok(1)
}

/// `navigate_back()` — raise a browser *back* intent, moving the host's history
/// cursor to the previous entry (a no-op at the start of history). Takes no
/// argument. Fire-and-forget; the host acts on the drained [`NavIntent::Back`].
/// Emits no draw command, returns nil.
fn native_navigate_back(cxt: &mut PetalCxt) -> NativeResult {
    let sym = cxt.intern_symbol(NAV_EVENTS);
    cxt.emit(sym, "back", vec![]);
    cxt.push_nil();
    Ok(1)
}

/// `navigate_forward()` — raise a browser *forward* intent, moving the host's
/// history cursor to the next entry (a no-op at the end of history). Takes no
/// argument. Fire-and-forget; the host acts on the drained [`NavIntent::Forward`].
/// Emits no draw command, returns nil.
fn native_navigate_forward(cxt: &mut PetalCxt) -> NativeResult {
    let sym = cxt.intern_symbol(NAV_EVENTS);
    cxt.emit(sym, "forward", vec![]);
    cxt.push_nil();
    Ok(1)
}

/// `text_view(id, x, y, w, h, text)` — declare a read-only, natively-selectable
/// text region. Emitted as a `text_view` host-extension draw command that
/// `garden-app` fulfills with a real `EditorView` (see [`PanelCmd::TextView`]).
/// `id` is a stable per-region key; `x,y,w,h` are panel-local pixels; `text` is
/// the full region contents. Returns nil.
fn native_text_view(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let x = cxt.get_int(2)?;
    let y = cxt.get_int(3)?;
    let w = cxt.get_int(4)?;
    let h = cxt.get_int(5)?;
    let text = cxt.get_string(6)?;
    let text_id = cxt.heap_mut().alloc_string(text);
    petal_ui::draw::emit_draw(
        cxt,
        "text_view",
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

/// `edit_view(id, x, y, w, h, text)` — declare an **editable** text region: the
/// same host-rendered [`EditorView`] as [`text_view`](native_text_view), but the
/// host routes real vim keystrokes into it (full editing, undo, visual mode), so
/// a panel can host a live editor surface. `text` seeds the buffer; because the
/// host gates buffer rebuilds on a content hash, re-declaring the *same* `text`
/// every frame preserves the user's edits — pass the seed unchanged until the
/// underlying data genuinely changes. Read the edited contents back with
/// `edit_view_text(id)`. Emitted as an `edit_view` host-extension command
/// (decoded into [`PanelCmd::TextView`] with `editable: true`). Returns nil.
fn native_edit_view(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let x = cxt.get_int(2)?;
    let y = cxt.get_int(3)?;
    let w = cxt.get_int(4)?;
    let h = cxt.get_int(5)?;
    let text = cxt.get_string(6)?;
    let text_id = cxt.heap_mut().alloc_string(text);
    petal_ui::draw::emit_draw(
        cxt,
        "edit_view",
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

/// `edit_view_text(id)` — read region `id`'s live (post-edit) buffer text back
/// into the script. Returns the editable region's current contents as a string,
/// or `""` when the region does not exist (or is not an `edit_view`). The host
/// publishes these each frame via [`PanelHost::set_edit_view_texts`]; a drawer
/// uses this to build the payload it hands the subprocess (`emit`/`mutate`) on a
/// save.
fn native_edit_view_text(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let text = EDIT_VIEW_TEXTS.with(|t| t.borrow().get(&id).cloned().unwrap_or_default());
    cxt.push_string(text);
    Ok(1)
}

/// `edit_view_projection(id, spec)` — declare where region `id`'s lines came
/// from, so the host can fold edits made to the region back into the documents
/// it was projected out of (see `garden_core::projection`). `spec` is the record
/// [`ProjectionSpec`] describes.
///
/// Declared frame state, like the region itself: re-declare the same spec every
/// frame and the host leaves the live projection (and the user's edits) alone —
/// it rebuilds only when the spec genuinely changes. A projected region needs no
/// `text_view_line_styles`; its styles come from the projection and follow their
/// lines through insertions instead of drifting. Returns nil.
fn native_edit_view_projection(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let spec = cxt.get_value(2)?;
    if !matches!(spec, Value::Map(_)) {
        return Err(format!(
            "edit_view_projection() expects a projection record, got {}",
            spec.type_name()
        ));
    }
    petal_ui::draw::emit_draw(cxt, "edit_view_projection", vec![Value::Int(id), spec]);
    cxt.push_nil();
    Ok(1)
}

/// `edit_view_edits(id)` — what the user's edits to projected region `id`
/// currently resolve to: a list of `{source, start, end, lines}` records, each
/// asking for source lines `[start, end)` to be replaced by `lines`.
///
/// This is the save payload — hand it straight to the subprocess with
/// `mutate("apply", edit_view_edits(id))`. Unlike sending the region's text, it
/// carries *intent*: the host resolved it from the line origins it has been
/// tracking, so the client applies edits rather than re-deriving them. An empty
/// list for a region with no projection.
fn native_edit_view_edits(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let data = EDIT_VIEW_EDITS.with(|t| t.borrow().get(&id).cloned());
    let value = match data {
        Some(data) => crate::query::data_to_value(cxt, &data),
        None => {
            let empty = cxt.heap_mut().alloc_list(Vec::new());
            Value::List(empty)
        }
    };
    cxt.push_value(value);
    Ok(1)
}

/// `text_view_scroll_to(id, line)` — scroll region `id` so the 0-based `line`
/// is the first visible one (clamped to the region's scroll range). Emitted as
/// a `text_view_scroll_to` host-extension command the host applies once — call
/// it on the frame the navigation is decided, not every frame, or the user can
/// never scroll away from it. Returns nil.
fn native_text_view_scroll_to(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let line = cxt.get_int(2)?;
    petal_ui::draw::emit_draw(
        cxt,
        "text_view_scroll_to",
        vec![Value::Int(id), Value::Int(line)],
    );
    cxt.push_nil();
    Ok(1)
}

/// `text_view_wrap(id, wrap)` — soft-wrap region `id`'s long lines to its
/// width instead of scrolling horizontally. Frame state, unlike
/// `text_view_scroll_to`: declare it on every frame the region should wrap
/// (dropping it unwraps the region again). Returns nil.
fn native_text_view_wrap(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let wrap = cxt.get_bool(2)?;
    petal_ui::draw::emit_draw(
        cxt,
        "text_view_wrap",
        vec![Value::Int(id), Value::Int(wrap as i64)],
    );
    cxt.push_nil();
    Ok(1)
}

/// `text_view_line_styles(id, styles)` — attach per-line semantic styling to
/// the `text_view` region `id`. `styles` is a list of style names, one per line
/// (`added`/`removed`/`hunk`/`title`/`dim`/`comment`, or `""` for plain). A side
/// channel: the region's text and its styling are declared independently.
/// Returns nil.
fn native_text_view_line_styles(cxt: &mut PetalCxt) -> NativeResult {
    let id = cxt.get_int(1)?;
    let styles = cxt.get_value(2)?;
    if !matches!(styles, Value::List(_)) {
        return Err(format!(
            "text_view_line_styles() expects a list of style names, got {}",
            styles.type_name()
        ));
    }
    petal_ui::draw::emit_draw(cxt, "text_view_styles", vec![Value::Int(id), styles]);
    cxt.push_nil();
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;

    fn write_script(src: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(f, "{src}").unwrap();
        f
    }

    #[test]
    fn frame_emits_rect_and_text() {
        let f = write_script(
            "clear(10, 20, 30)\n\
             draw_rect(1, 2, 3, 4, 200, 100, 50)\n\
             draw_text(\"hi\", 5, 6, 14, 255, 255, 255)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 80);
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            cmds,
            vec![
                PanelCmd::Clear {
                    r: 10,
                    g: 20,
                    b: 30
                },
                PanelCmd::Rect {
                    x: 1,
                    y: 2,
                    w: 3,
                    h: 4,
                    r: 200,
                    g: 100,
                    b: 50,
                    a: 255,
                    radius: 0
                },
                PanelCmd::plain_text("hi", 5, 6, 14, 255, 255, 255, 255),
            ]
        );
    }

    #[test]
    fn frame_emits_text_view() {
        let f = write_script("text_view(7, 8, 9, 200, 300, \"a\\nb\")\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 80);
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::TextView {
                id: 7,
                x: 8,
                y: 9,
                w: 200,
                h: 300,
                text: "a\nb".into(),
                editable: false,
            }]
        );
    }

    #[test]
    fn frame_emits_edit_view_as_editable_text_view() {
        let f = write_script("edit_view(5, 1, 2, 120, 90, \"x\\ny\")\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(120, 90);
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::TextView {
                id: 5,
                x: 1,
                y: 2,
                w: 120,
                h: 90,
                text: "x\ny".into(),
                editable: true,
            }]
        );
    }

    #[test]
    fn edit_view_text_reads_the_host_published_map() {
        // With no host-published text, `edit_view_text` reads "" (len 0); the
        // host binds the live buffer each frame in production.
        let f = write_script("let n = len(edit_view_text(9))\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(80, 40);
        host.frame(0.016, 0).unwrap();
        assert_eq!(host.observed_json()["n"], 0);

        host.set_edit_view_texts(std::collections::HashMap::from([(9, "abcd".to_string())]));
        host.frame(0.016, 1).unwrap();
        assert_eq!(host.observed_json()["n"], 4);
    }

    /// `text_view_scroll_to` reaches the host as its own command, so the host
    /// can apply it as a one-shot jump rather than frame state.
    #[test]
    fn frame_emits_text_view_scroll_to() {
        let f = write_script(
            "text_view(3, 0, 0, 100, 100, \"a\\nb\")\n\
             text_view_scroll_to(3, 12)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 80);
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            cmds.last(),
            Some(&PanelCmd::TextViewScrollTo { id: 3, line: 12 })
        );
    }

    /// `text_view_wrap` reaches the host as frame state carrying the script's
    /// boolean, so a region can be wrapped on one frame and not the next.
    #[test]
    fn frame_emits_text_view_wrap() {
        let f = write_script(
            "text_view(3, 0, 0, 100, 100, \"a\\nb\")\n\
             text_view_wrap(3, true)\n\
             text_view_wrap(4, false)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 80);
        let cmds = host.frame(0.016, 0).unwrap();
        let wraps: Vec<&PanelCmd> = cmds
            .iter()
            .filter(|c| matches!(c, PanelCmd::TextViewWrap { .. }))
            .collect();
        assert_eq!(
            wraps,
            vec![
                &PanelCmd::TextViewWrap { id: 3, wrap: true },
                &PanelCmd::TextViewWrap { id: 4, wrap: false },
            ]
        );
    }

    #[test]
    fn frame_emits_text_view_line_styles() {
        let f = write_script(
            "text_view(3, 0, 0, 100, 100, \"a\\nb\")\n\
             text_view_line_styles(3, [\"added\", \"removed\"])\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 80);
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            cmds,
            vec![
                PanelCmd::TextView {
                    id: 3,
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 100,
                    text: "a\nb".into(),
                    editable: false,
                },
                PanelCmd::TextViewStyles {
                    id: 3,
                    styles: vec!["added".into(), "removed".into()],
                },
            ]
        );
    }

    #[test]
    fn panel_theme_reaches_the_script_and_updates() {
        // The script reads panel_theme().text and paints a rect with its sRGB
        // components, so the frame's command carries the injected color verbatim.
        let f = write_script(
            "let th = panel_theme()\n\
             draw_rect(0, 0, 1, 1, th.text.r, th.text.g, th.text.b)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);

        let mut theme = PanelTheme::new();
        theme.set("text", [10, 20, 30, 255]);
        host.set_theme(theme);
        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
                r: 10,
                g: 20,
                b: 30,
                a: 255,
                radius: 0
            }]
        );

        // Changing the injected theme changes what the next frame's script sees
        // (a live `POST /theme` reflected on the following frame).
        let mut theme2 = PanelTheme::new();
        theme2.set("text", [200, 100, 50, 255]);
        host.set_theme(theme2);
        let cmds = host.frame(0.0, 1).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
                r: 200,
                g: 100,
                b: 50,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn host_palette_defaults_ui_theme_for_prelude_widgets() {
        // The palette bridge: `ui_theme()` resolves from the host palette bound
        // each frame (petal-ui's `bind_host_palette`), so a drawer built on
        // prelude widgets paints in Garden's colors without calling `theme_set`.
        // With no theme injected the resolved palette is FALLBACK_PALETTE, so
        // the ui accent is its `accent` (0x58a6ff)…
        let f = write_script(
            "let t = ui_theme()\n\
             draw_rect(0, 0, 1, 1, t.accent.r, t.accent.g, t.accent.b)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let cmds = host.frame(0.016, 1).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
                r: 0x58,
                g: 0xa6,
                b: 0xff,
                a: 255,
                radius: 0
            }]
        );

        // …and an injected host theme overrides it on the next frame, like
        // every other per-frame input.
        let mut theme = PanelTheme::new();
        theme.set("accent", [9, 8, 7, 255]);
        host.set_theme(theme);
        let cmds = host.frame(0.016, 2).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
                r: 9,
                g: 8,
                b: 7,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn panel_theme_alpha_is_readable() {
        // Each color record carries its sRGB alpha too, so a script can paint a
        // translucent fill via the `draw_rect(rect, color, a)` prelude overload.
        let f = write_script(
            "let th = panel_theme()\n\
             draw_rect(0, 0, 1, 1, th.sel.r, th.sel.g, th.sel.b, th.sel.a)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let mut theme = PanelTheme::new();
        theme.set("sel", [40, 80, 120, 128]);
        host.set_theme(theme);
        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
                r: 40,
                g: 80,
                b: 120,
                a: 128,
                radius: 0
            }]
        );
    }

    #[test]
    fn panel_theme_without_injection_is_empty_record() {
        // No theme injected: panel_theme() is an empty record, so `keys(th)` is
        // empty and a script gates its palette on that to fall back to defaults
        // (here it draws the guard rect).
        let f = write_script(
            "let th = panel_theme()\n\
             if len(keys(th)) == 0 then draw_rect(1, 1, 1, 1, 0, 0, 0) end\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 1,
                y: 1,
                w: 1,
                h: 1,
                r: 0,
                g: 0,
                b: 0,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn palette_falls_back_when_no_theme_is_injected() {
        // `palette()` is the always-complete companion to `panel_theme()`: with
        // no theme injected it returns the built-in fallback, so a bare embedder
        // still paints a coherent palette without any `?? default` guards.
        let f = write_script(
            "let p = palette()\n\
             draw_rect(0, 0, 1, 1, p.text.r, p.text.g, p.text.b)\n\
             draw_rect(1, 0, 1, 1, p.added_bg.r, p.added_bg.g, p.added_bg.b)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![
                // fallback `text` = #e6edf3
                PanelCmd::Rect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                    r: 230,
                    g: 237,
                    b: 243,
                    a: 255,
                    radius: 0
                },
                // fallback `added_bg` = #12261a (a derived key, present in the fallback)
                PanelCmd::Rect {
                    x: 1,
                    y: 0,
                    w: 1,
                    h: 1,
                    r: 18,
                    g: 38,
                    b: 26,
                    a: 255,
                    radius: 0
                },
            ]
        );
    }

    #[test]
    fn palette_overlays_injected_theme_onto_the_fallback() {
        // An injected key wins; a key the host did not inject still resolves from
        // the fallback — so a drawer can read any canonical key unconditionally.
        let f = write_script(
            "let p = palette()\n\
             draw_rect(0, 0, 1, 1, p.text.r, p.text.g, p.text.b)\n\
             draw_rect(1, 0, 1, 1, p.green.r, p.green.g, p.green.b)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let mut theme = PanelTheme::new();
        theme.set("text", [10, 20, 30, 255]);
        host.set_theme(theme);
        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![
                // injected `text` wins
                PanelCmd::Rect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                    r: 10,
                    g: 20,
                    b: 30,
                    a: 255,
                    radius: 0
                },
                // `green` was not injected → fallback #3fb950
                PanelCmd::Rect {
                    x: 1,
                    y: 0,
                    w: 1,
                    h: 1,
                    r: 63,
                    g: 185,
                    b: 80,
                    a: 255,
                    radius: 0
                },
            ]
        );
    }

    #[test]
    fn frame_emits_geometric_primitives() {
        let f = write_script(
            "draw_line(0, 1, 2, 3, 10, 20, 30)\n\
             draw_circle(40, 50, 12, 200, 100, 50)\n\
             fill_triangle(0, 0, 10, 0, 5, 8, 1, 2, 3)\n\
             fill_poly([[0, 0], [10, 0], [10, 10]], 4, 5, 6)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 80);
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            cmds,
            vec![
                PanelCmd::Line {
                    x1: 0,
                    y1: 1,
                    x2: 2,
                    y2: 3,
                    r: 10,
                    g: 20,
                    b: 30,
                    a: 255,
                    width: 1
                },
                PanelCmd::Circle {
                    cx: 40,
                    cy: 50,
                    radius: 12,
                    r: 200,
                    g: 100,
                    b: 50,
                    a: 255
                },
                PanelCmd::Triangle {
                    x1: 0,
                    y1: 0,
                    x2: 10,
                    y2: 0,
                    x3: 5,
                    y3: 8,
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 255
                },
                PanelCmd::Poly {
                    points: vec![(0, 0), (10, 0), (10, 10)],
                    r: 4,
                    g: 5,
                    b: 6,
                    a: 255
                },
            ]
        );
    }

    #[test]
    fn fill_poly_rejects_too_few_points() {
        let f = write_script("fill_poly([[0, 0], [10, 0]], 1, 2, 3)\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        assert!(host.frame(0.0, 0).is_err());
    }

    #[test]
    fn dimensions_and_timing_are_bound() {
        let f =
            write_script("draw_rect(0, 0, screen_width(), screen_height(), frame_count(), 0, 0)\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(64, 48);
        let cmds = host.frame(0.5, 7).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 0,
                y: 0,
                w: 64,
                h: 48,
                r: 7,
                g: 0,
                b: 0,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn image_commands_reach_the_panel_with_source_and_alpha() {
        let f = write_script("draw_image(\"assets/dial.png\", 4, 5, 20, 30, 128)\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(64, 48);
        assert_eq!(
            host.frame(0.0, 0).unwrap(),
            vec![PanelCmd::Image {
                source: "assets/dial.png".into(),
                x: 4,
                y: 5,
                w: 20,
                h: 30,
                a: 128,
            }]
        );
    }

    #[test]
    fn state_persists_across_frames() {
        let f = write_script(
            "state n = 0\n\
             n = n + 1\n\
             draw_rect(n, 0, 1, 1, 0, 0, 0)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let c1 = host.frame(0.0, 0).unwrap();
        let c2 = host.frame(0.0, 1).unwrap();
        assert_eq!(
            c1,
            vec![PanelCmd::Rect {
                x: 1,
                y: 0,
                w: 1,
                h: 1,
                r: 0,
                g: 0,
                b: 0,
                a: 255,
                radius: 0
            }]
        );
        assert_eq!(
            c2,
            vec![PanelCmd::Rect {
                x: 2,
                y: 0,
                w: 1,
                h: 1,
                r: 0,
                g: 0,
                b: 0,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn key_pressed_reads_input() {
        let f = write_script("if key_pressed(\"space\") then draw_rect(0,0,1,1,1,1,1) end\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let none = host.frame(0.0, 0).unwrap();
        assert!(none.is_empty());
        host.input_event(InputEvent::KeyDown {
            key: "space".into(),
        });
        let hit = host.frame(0.0, 1).unwrap();
        assert_eq!(hit.len(), 1);
        // The pressed edge lasts exactly one frame.
        let gone = host.frame(0.0, 2).unwrap();
        assert!(gone.is_empty());
    }

    #[test]
    fn frame_emits_clip_commands() {
        let f = write_script(
            "clip(4, 8, 20, 30)\n\
             draw_rect(0, 0, 5, 5, 1, 2, 3)\n\
             clip_none()\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 80);
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            cmds,
            vec![
                PanelCmd::Clip {
                    x: 4,
                    y: 8,
                    w: 20,
                    h: 30
                },
                PanelCmd::Rect {
                    x: 0,
                    y: 0,
                    w: 5,
                    h: 5,
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 255,
                    radius: 0
                },
                PanelCmd::ClipNone,
            ]
        );
    }

    #[test]
    fn scroll_y_reads_input() {
        let f = write_script("draw_rect(scroll_y(), 0, 1, 1, 0, 0, 0)\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        host.input_event(InputEvent::Scroll { dx: 0.0, dy: 3.0 });
        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 3,
                y: 0,
                w: 1,
                h: 1,
                r: 0,
                g: 0,
                b: 0,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn text_width_accepts_garden_font_roles() {
        // Both role names a portable script may ask for resolve to Garden's one
        // embedded face, so measuring by role agrees with measuring by default
        // — and an unknown face degrades to the default rather than erroring.
        let f = write_script(
            "print(str(text_width(\"abcd\", 14)))\n\
             print(str(text_width(\"abcd\", 14, \"mono\")))\n\
             print(str(text_width(\"abcd\", 14, \"ui\")))\n\
             print(str(text_width(\"abcd\", 14, \"Comic Sans\")))\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 80);
        host.frame(0.0, 0).unwrap();
        let widths = host.take_output();
        assert_eq!(widths.len(), 4, "one width per measurement: {widths:?}");
        assert!(
            widths.iter().all(|w| w == &widths[0]),
            "every role measures Garden's one face: {widths:?}"
        );
        // Sanity: 4 chars at size 14 is ~34 px (0.6 ratio), not zero.
        assert_eq!(widths[0].trim(), "34");
    }

    #[test]
    fn measured_advances_are_per_host() {
        // The host that was told its real advances measures with them — through
        // the default *and* both role names — while a sibling host built from the
        // same source keeps the 0.6 estimate. Two hosts in one process disagreeing
        // is the point: the measurement belongs to the pane's renderer, not to
        // the process.
        const SRC: &str = "print(str(text_width(\"aa\", 100)))\n\
                           print(str(text_width(\"aa\", 100, \"mono\")))\n\
                           print(str(text_width(\"aa\", 100, \"ui\")))\n";
        let f = write_script(SRC);

        // 'a' (codepoint 97) advances 0.25× the size; everything else falls back.
        let mut ratios = vec![0.6; 128];
        ratios[b'a' as usize] = 0.25;

        let mut measured = PanelHost::load(f.path()).unwrap();
        measured.set_font_advance_ratios(ratios);
        measured.set_dimensions(100, 80);
        measured.frame(0.0, 0).unwrap();
        let widths = measured.take_output();
        assert_eq!(
            widths.iter().map(|w| w.trim()).collect::<Vec<_>>(),
            ["50", "50", "50"],
            "the measured table drives the default and both roles: {widths:?}"
        );

        let mut estimated = PanelHost::load(f.path()).unwrap();
        estimated.set_dimensions(100, 80);
        estimated.frame(0.0, 0).unwrap();
        let widths = estimated.take_output();
        assert_eq!(
            widths.iter().map(|w| w.trim()).collect::<Vec<_>>(),
            ["120", "120", "120"],
            "an untold host keeps the 0.6 estimate: {widths:?}"
        );
    }

    #[test]
    fn measured_advances_survive_hot_reload() {
        // `poll_reload` recompiles into the same env, so the host keeps the
        // advances it was told — a script edit must not silently drop a pane back
        // to the 0.6 estimate.
        let f = write_script("print(str(text_width(\"a\", 100)))\n");
        let mut ratios = vec![0.6; 128];
        ratios[b'a' as usize] = 0.25;

        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_font_advance_ratios(ratios);
        host.set_dimensions(100, 80);
        host.frame(0.0, 0).unwrap();
        assert_eq!(host.take_output()[0].trim(), "25");

        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(f.path(), "print(str(text_width(\"aa\", 100)))\n").unwrap();
        assert!(host.poll_reload().unwrap(), "the edit should reload");
        host.frame(0.0, 0).unwrap();
        assert_eq!(host.take_output()[0].trim(), "50");
    }

    #[test]
    fn source_backed_host_never_reads_its_name_from_disk() {
        // A panel-mode GPP pane names its host after the *client binary*, so a
        // `from_source` host's path can point at a real, non-UTF-8 file. Polling
        // it must not stat-and-read that file (which used to surface as
        // "failed to read <binary>: stream did not contain valid UTF-8").
        let mut bin = tempfile::NamedTempFile::new().unwrap();
        bin.write_all(&[0xfe, 0xff, 0x00, 0x80]).unwrap();
        bin.flush().unwrap();

        let name = bin.path().to_str().unwrap().to_string();
        let mut host = PanelHost::from_source(&name, "draw_rect(0, 0, 1, 1, 1, 1, 1)\n").unwrap();
        assert_eq!(
            host.poll_reload(),
            Ok(false),
            "no disk poll on the first tick"
        );

        // ...and not after the named file changes, either.
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(bin.path(), [0x00, 0x01, 0x02]).unwrap();
        assert_eq!(
            host.poll_reload(),
            Ok(false),
            "still no disk poll after a change"
        );
    }

    #[test]
    fn prelude_widgets_are_available() {
        // The `ui` prelude ships as an implicit import: hit-testing, the record
        // draw overloads, and `text_width` resolve with no ceremony.
        let f = write_script(
            "let r = rect(0, 0, 10, 10)\n\
             if point_in(5, 5, r) then draw_rect(r, {r: 1, g: 2, b: 3}) end\n\
             draw_text_right(\"ab\", 100, 0, 14, {r: 4, g: 5, b: 6})\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 80);
        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![
                PanelCmd::Rect {
                    x: 0,
                    y: 0,
                    w: 10,
                    h: 10,
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 255,
                    radius: 0
                },
                // "ab" at size 14, ratio 0.6 → 17px wide, right edge at 100.
                PanelCmd::plain_text("ab", 83, 0, 14, 4, 5, 6, 255),
            ]
        );
    }

    #[test]
    fn rect_methods_and_pixel_text_helpers_are_available() {
        // `rect(...)` is the built-in `Rect` class, so a rect carries geometry
        // methods; `ellipsize` and `draw_text_center` are the pixel-measured
        // text helpers. Both arrived with the upstream prelude, and panel
        // scripts here lean on them instead of estimating a character width.
        let f = write_script(
            "let r = rect(10, 20, 40, 30)\n\
             draw_rect(r.right(), r.bottom(), 1, 1, 0, 0, 0)\n\
             draw_rect(r.center_x(), r.center_y(), 1, 1, 0, 0, 0)\n\
             let i = r.inset(5)\n\
             draw_rect(i.x, i.y, i.w, i.h, 1, 1, 1)\n\
             draw_text_center(\"ab\", 50, 0, 14, {r: 4, g: 5, b: 6})\n\
             draw_text(ellipsize(\"abcdefghij\", 30, 14), {x: 0, y: 40}, 14, \
             {r: 7, g: 8, b: 9})\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(200, 80);
        let cmds = host.frame(0.0, 0).unwrap();

        // right/bottom = x+w, y+h; center = x + w/2, y + h/2.
        assert!(cmds.contains(&PanelCmd::Rect {
            x: 50,
            y: 50,
            w: 1,
            h: 1,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            radius: 0
        }));
        assert!(cmds.contains(&PanelCmd::Rect {
            x: 30,
            y: 35,
            w: 1,
            h: 1,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            radius: 0
        }));
        // inset(5) pulls every edge in: 10,20,40,30 → 15,25,30,20.
        assert!(cmds.contains(&PanelCmd::Rect {
            x: 15,
            y: 25,
            w: 30,
            h: 20,
            r: 1,
            g: 1,
            b: 1,
            a: 255,
            radius: 0
        }));
        // "ab" is 17px wide at size 14, so centering on 50 starts it at 42.
        assert!(cmds.contains(&PanelCmd::plain_text("ab", 42, 0, 14, 4, 5, 6, 255)));
        // The ellipsized run measures no wider than the 30px it was given.
        let shortened = cmds
            .iter()
            .find_map(|c| match c {
                PanelCmd::Text { text, y: 40, .. } => Some(text.clone()),
                _ => None,
            })
            .expect("the ellipsized text command");
        assert!(
            shortened.ends_with('…') && shortened.chars().count() < 10,
            "expected a clipped run ending in an ellipsis, got {shortened:?}"
        );
    }

    #[test]
    fn release_drag_click_and_text_edges_reach_the_script() {
        // Phase 4: the full input contract is fed end-to-end. A script reads
        // released edges, an in-progress drag, the click chain, and typed text —
        // each drawn as a rect so the frame's commands assert the values.
        let f = write_script(
            "if mouse_released(0) then draw_rect(1, 0, 1, 1, 0, 0, 0) end\n\
             if key_released(\"a\") then draw_rect(2, 0, 1, 1, 0, 0, 0) end\n\
             if drag_active() then draw_rect(drag_start_x(), drag_start_y(), 1, 1, 0, 0, 0) end\n\
             if click_count() > 0 then draw_rect(click_count(), 5, 1, 1, 0, 0, 0) end\n\
             if text_input() != \"\" then draw_rect(9, 9, 1, 1, 0, 0, 0) end\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(50, 50);

        // Frame 1: press left at (10, 10), press+release "a", type "a".
        host.input_event(InputEvent::MouseMove { x: 10, y: 10 });
        host.input_event(InputEvent::MouseDown {
            button: buttons::LEFT,
        });
        host.input_event(InputEvent::KeyDown { key: "a".into() });
        host.input_event(InputEvent::KeyUp { key: "a".into() });
        host.input_event(InputEvent::Text { text: "a".into() });
        let f1 = host.frame(0.016, 0).unwrap();
        // key_released("a") edge + typed text this frame; no release/drag yet.
        assert!(f1.contains(&PanelCmd::Rect {
            x: 2,
            y: 0,
            w: 1,
            h: 1,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            radius: 0
        }));
        assert!(f1.contains(&PanelCmd::Rect {
            x: 9,
            y: 9,
            w: 1,
            h: 1,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            radius: 0
        }));
        // First left press → click_count 1.
        assert!(f1.contains(&PanelCmd::Rect {
            x: 1,
            y: 5,
            w: 1,
            h: 1,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            radius: 0
        }));

        // Frame 2: move past the drag threshold → drag_active with the press as start.
        host.input_event(InputEvent::MouseMove { x: 20, y: 14 });
        let f2 = host.frame(0.016, 1).unwrap();
        assert!(f2.contains(&PanelCmd::Rect {
            x: 10,
            y: 10,
            w: 1,
            h: 1,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            radius: 0
        }));

        // Frame 3: release the button → mouse_released edge, drag ends.
        host.input_event(InputEvent::MouseUp {
            button: buttons::LEFT,
        });
        let f3 = host.frame(0.016, 2).unwrap();
        assert!(f3.contains(&PanelCmd::Rect {
            x: 1,
            y: 0,
            w: 1,
            h: 1,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            radius: 0
        }));
        // Drag is over: no drag rect.
        assert_eq!(host.input_snapshot().drag_active, false);
    }

    #[test]
    fn click_count_chains_into_a_double_click() {
        // Two quick press/release pairs at the same spot chain to click_count 2,
        // derived from the dt-advanced clock inside the input contract.
        let f = write_script("let cc = click_count()\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(20, 20);
        host.input_event(InputEvent::MouseMove { x: 5, y: 5 });
        host.input_event(InputEvent::MouseDown {
            button: buttons::LEFT,
        });
        host.input_event(InputEvent::MouseUp {
            button: buttons::LEFT,
        });
        host.frame(0.05, 0).unwrap();
        assert_eq!(host.observed_json()["cc"], 1);
        host.input_event(InputEvent::MouseDown {
            button: buttons::LEFT,
        });
        host.input_event(InputEvent::MouseUp {
            button: buttons::LEFT,
        });
        host.frame(0.05, 1).unwrap(); // 0.1s total — inside the multi-click window
        assert_eq!(host.observed_json()["cc"], 2);
    }

    #[test]
    fn input_snapshot_reflects_bound_uniforms() {
        let f = write_script("clear(0, 0, 0)\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        host.input_event(InputEvent::MouseMove { x: 3, y: 4 });
        host.input_event(InputEvent::KeyDown { key: "j".into() });
        host.input_event(InputEvent::Modifiers(Modifiers {
            shift: true,
            ..Default::default()
        }));
        host.frame(0.016, 0).unwrap();
        let snap = host.input_snapshot();
        assert_eq!((snap.mouse_x, snap.mouse_y), (3, 4));
        assert_eq!(snap.keys_pressed, vec!["j".to_string()]);
        assert_eq!(snap.keys_down, vec!["j".to_string()]);
        assert_eq!(snap.modifiers, 1); // shift bit
    }

    /// The host introspection channel: a panel's logical state is readable by
    /// name with no cooperation from the script, and each frame reports that
    /// frame's bindings — nothing stale carried over.
    #[test]
    fn bindings_are_observable_by_name_and_refresh_each_frame() {
        let f = write_script(
            "state n = 0\n\
             n = n + 5\n\
             let selected = n\n\
             let scroll = 42\n\
             let label = \"rows\"\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        // Observation is free of draw commands — reading state can't move pixels.
        let cmds = host.frame(0.0, 0).unwrap();
        assert!(cmds.is_empty());
        let obs = host.observed_json();
        assert_eq!(obs["selected"], 5);
        assert_eq!(obs["scroll"], 42);
        // Not int-only, unlike the debug_state channel this replaced.
        assert_eq!(obs["label"], "rows");
        // `state` vars are named terms too, so they're observable as themselves.
        assert_eq!(obs["n"], 5);

        // Frame 2 reports frame 2: the buffer is cleared by `env.run`, so a
        // value can only be this frame's, never a survivor of the last.
        host.frame(0.0, 1).unwrap();
        let obs = host.observed_json();
        assert_eq!(obs["selected"], 10);
        assert_eq!(obs["scroll"], 42);
        assert_eq!(obs["n"], 10);
    }

    /// Panel scripts are mostly helper functions, so the function-qualified key
    /// is what a host actually reads: a helper's local is namespaced under the
    /// helper (reporting its *last* call), and a same-named top-level binding
    /// stays a separate key rather than colliding with it.
    #[test]
    fn observed_keys_are_qualified_by_the_enclosing_function() {
        let f = write_script(
            "fn list_row(i)\n\
            \x20   let sel = i * 2\n\
            \x20   sel\n\
             end\n\
             let rows = [list_row(1), list_row(3)]\n\
             let sel = 99\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        host.frame(0.0, 0).unwrap();
        let obs = host.observed_json();
        // Last write wins: the second call's `sel`, not the first's.
        assert_eq!(obs["list_row.sel"], 6);
        // ...and the top-level `sel` is its own key, unshadowed.
        assert_eq!(obs["sel"], 99);
        assert_eq!(obs["rows"], serde_json::json!([2, 6]));
    }

    /// The filter policy of [`PanelHost::observed_json`]. A panel program is
    /// mostly the `petal-ui` prelude, so what a reader gets back has to be the
    /// script's own bindings and not the widget library's — a real panel
    /// otherwise reports ~110 keys for a dozen of its own.
    #[test]
    fn observed_json_reports_only_the_scripts_own_bindings() {
        // A script that both binds its own values and *uses* the prelude, so the
        // prelude's bindings are genuinely in the observation buffer.
        let f = write_script(
            "let sel = 3\n\
             let name = \"rows\"\n\
             let mine = draw_rect\n\
             draw_rect(rect(0, 0, 5, 5), theme.panel)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        host.frame(0.0, 0).unwrap();

        let raw = host
            .env
            .get_observations_json(host.program_id, host.stack_id);
        let obs = host.observed_json();
        assert!(
            raw.len() > 50,
            "the prelude really is in there ({} keys)",
            raw.len()
        );

        assert_eq!(obs["sel"], 3);
        assert_eq!(obs["name"], "rows");
        // Rule 1: module-qualified keys are imports, not this script's.
        assert!(raw.contains_key("ui::button") && raw.contains_key("std::sum"));
        assert!(!obs.keys().any(|k| k.contains("::")));
        // Rule 2: the `_`-prefixed plumbing convention — including the prelude's
        // *non-callable* `_MENU_*` constants, which no other rule would catch.
        assert!(raw.contains_key("_MENU_PAD"));
        assert!(!obs.keys().any(|k| k.starts_with('_')));
        // Rule 3: callables are dropped whoever bound them — the prelude's
        // unprefixed implicit-import aliases, and the script's own `mine` alias.
        assert_eq!(raw["draw_rect"], "<function>");
        assert_eq!(raw["rect"], "<native>");
        assert!(!obs.contains_key("draw_rect") && !obs.contains_key("rect"));
        assert!(!obs.contains_key("mine"));

        // What survives is the script's own, plus the one documented leftover:
        // the prelude's exported `theme` record.
        let mut keys: Vec<&str> = obs.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["name", "sel", "theme"]);
    }

    /// The `theme` leftover is harmless because a script that wants the name
    /// takes it: its term is later in program order, and later wins the key.
    #[test]
    fn a_scripts_own_binding_wins_a_name_the_prelude_exported() {
        let f = write_script("let theme = \"mine\"\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        host.frame(0.0, 0).unwrap();
        assert_eq!(host.observed_json()["theme"], "mine");
    }

    #[test]
    fn mutate_is_published_on_its_own_channel_and_drained() {
        // `mutate(name, arg)` draws nothing and rides a channel distinct from
        // `emit`, so an emit and a mutate in the same frame never mix.
        let f = write_script(
            "emit(\"e\", 1)\n\
             mutate(\"save\", { text: \"hi\" })\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        assert!(host.frame(0.0, 0).unwrap().is_empty());
        assert_eq!(
            host.take_mutations(),
            vec![("save".to_string(), serde_json::json!({ "text": "hi" }), 1)]
        );
        // The emit stayed on its own channel, untouched by the mutation drain.
        assert_eq!(
            host.take_emitted(),
            vec![("e".to_string(), serde_json::json!(1))]
        );
        // Drained: a second take is empty.
        assert!(host.take_mutations().is_empty());
    }

    #[test]
    fn emit_is_published_in_order_and_drained() {
        // `emit(event, arg)` is fire-and-forget: it draws nothing, returns nil,
        // and multiple emits in one frame surface in call order.
        let f = write_script(
            "emit(\"select\", 3)\n\
             emit(\"divider\", { pos: 240, axis: \"x\" })\n\
             emit(\"tags\", [\"a\", \"b\"])\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let cmds = host.frame(0.0, 0).unwrap();
        assert!(cmds.is_empty()); // emit produces no draw command
        assert_eq!(
            host.take_emitted(),
            vec![
                ("select".to_string(), serde_json::json!(3)),
                (
                    "divider".to_string(),
                    serde_json::json!({ "pos": 240, "axis": "x" })
                ),
                ("tags".to_string(), serde_json::json!(["a", "b"])),
            ]
        );
        // Drained: a second take is empty…
        assert!(host.take_emitted().is_empty());
        // …and the next frame republishes fresh values only (no accumulation).
        host.frame(0.0, 1).unwrap();
        assert_eq!(host.take_emitted().len(), 3);
    }

    #[test]
    fn emit_converts_scalars_and_nested_values() {
        let f = write_script(
            "emit(\"s\", \"hi\")\n\
             emit(\"b\", true)\n\
             emit(\"f\", 1.5)\n\
             emit(\"n\", nil)\n\
             emit(\"nested\", { items: [1, { deep: \"yes\" }] })\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        host.frame(0.0, 0).unwrap();
        assert_eq!(
            host.take_emitted(),
            vec![
                ("s".to_string(), serde_json::json!("hi")),
                ("b".to_string(), serde_json::json!(true)),
                ("f".to_string(), serde_json::json!(1.5)),
                ("n".to_string(), serde_json::Value::Null),
                (
                    "nested".to_string(),
                    serde_json::json!({ "items": [1, { "deep": "yes" }] })
                ),
            ]
        );
    }

    /// `navigate(screen, arg)` — the two-argument form carries the subject the
    /// target screen is for. The argument rides the same side channel as the
    /// screen name, and `nav_arg()` reads back what the host republished.
    #[test]
    fn a_navigation_carries_its_argument_and_reads_it_back() {
        let f = write_script(
            "navigate(\"detail.ptl\", {id: 7, name: \"row\"})\n\
             navigate_replace(\"login.ptl\", 3)\n\
             navigate(\"plain.ptl\")\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        host.frame(0.0, 0).unwrap();
        assert_eq!(
            host.take_nav(),
            vec![
                NavIntent::Push(
                    "detail.ptl".to_string(),
                    serde_json::json!({"id": 7, "name": "row"})
                ),
                NavIntent::Replace("login.ptl".to_string(), serde_json::json!(3)),
                // The one-argument form is unchanged and carries nothing.
                NavIntent::Push("plain.ptl".to_string(), serde_json::Value::Null),
            ]
        );

        // The reading half: whatever the host publishes is what `nav_arg()` sees,
        // and a screen nothing navigated to reads nil.
        let g = write_script("let got = nav_arg()\n");
        let mut target = PanelHost::load(g.path()).unwrap();
        target.set_dimensions(10, 10);
        target.frame(0.0, 0).unwrap();
        assert_eq!(
            target.observed_json().get("got"),
            Some(&serde_json::Value::Null),
            "a screen nothing navigated to reads nil"
        );
        target.set_nav_arg(serde_json::json!({"id": 7}));
        target.frame(0.0, 1).unwrap();
        assert_eq!(
            target.observed_json().get("got"),
            Some(&serde_json::json!({"id": 7}))
        );
    }

    #[test]
    fn key_claims_are_published_and_drained() {
        // `claim_key` names the chords the host must forward to this panel
        // instead of applying its own shortcut. Bits are petal-ui's:
        // 1=shift 2=ctrl 4=alt 8=cmd.
        let f = write_script(
            "claim_key(\"z\", \"cmd\")\n             claim_key(\"s\", \"cmd+shift\")\n             claim_key(\"Escape\")\n             claim_key(\"c\", 2)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let cmds = host.frame(0.0, 0).unwrap();
        assert!(cmds.is_empty(), "a claim draws nothing");
        assert_eq!(
            host.take_key_claims(),
            vec![
                ("z".to_string(), Some(8)),
                ("s".to_string(), Some(9)),
                // No modifier argument: the key under *any* chord.
                ("escape".to_string(), None),
                // A raw bitmask is accepted as well as a spelling.
                ("c".to_string(), Some(2)),
            ]
        );
        // Drained, and re-declared by the next frame rather than accumulating.
        assert!(host.take_key_claims().is_empty());
        host.frame(0.0, 1).unwrap();
        assert_eq!(host.take_key_claims().len(), 4);
    }

    #[test]
    fn an_unknown_claim_modifier_is_an_error() {
        // A typo'd claim that silently never fires is exactly the failure this
        // API exists to end, so it raises instead of claiming nothing.
        let f = write_script("claim_key(\"z\", \"komand\")\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let err = host.frame(0.0, 0).unwrap_err();
        assert!(err.contains("komand"), "unhelpful error: {err}");
    }

    #[test]
    fn nav_intents_are_published_and_drained() {
        // Each of the four navigation natives raises a typed `NavIntent`,
        // surfaced in call order and drained by `take_nav`.
        let f = write_script(
            "navigate(\"detail\")\n\
             navigate_replace(\"login\")\n\
             navigate_back()\n\
             navigate_forward()\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let cmds = host.frame(0.0, 0).unwrap();
        assert!(cmds.is_empty()); // navigation produces no draw command
        assert_eq!(
            host.take_nav(),
            vec![
                NavIntent::Push("detail".to_string(), serde_json::Value::Null),
                NavIntent::Replace("login".to_string(), serde_json::Value::Null),
                NavIntent::Back,
                NavIntent::Forward,
            ]
        );
        // Drained: a second take is empty…
        assert!(host.take_nav().is_empty());
        // …and the next frame republishes fresh intents only (no accumulation
        // of the prior frame's — the buffer is cleared at frame start).
        host.frame(0.0, 1).unwrap();
        assert_eq!(host.take_nav().len(), 4);
    }

    #[test]
    fn nav_channel_is_separate_from_emit() {
        // A frame that both emits and navigates keeps the two side channels
        // distinct: draining nav intents leaves emit events intact and vice versa.
        let f = write_script(
            "emit(\"select\", 3)\n\
             navigate(\"detail\")\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        host.frame(0.0, 0).unwrap();
        assert_eq!(
            host.take_nav(),
            vec![NavIntent::Push(
                "detail".to_string(),
                serde_json::Value::Null
            )]
        );
        assert_eq!(
            host.take_emitted(),
            vec![("select".to_string(), serde_json::json!(3))]
        );
    }

    #[test]
    fn restore_state_seeds_before_first_frame() {
        // `state x = 0` normally initializes to 0, but a restore_state before the
        // first frame pre-seeds the slot so `Inst::StateInit` skips the init and
        // the first frame observes the restored value.
        let f = write_script(
            "state x = 0\n\
             draw_rect(x, 0, 1, 1, 0, 0, 0)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);

        let mut map = serde_json::Map::new();
        map.insert("x".to_string(), serde_json::json!(5));
        let applied = host.restore_state(&map);
        assert_eq!(applied, 1);

        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 5,
                y: 0,
                w: 1,
                h: 1,
                r: 0,
                g: 0,
                b: 0,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn restore_state_skips_unknown_keys() {
        // An unknown top-level name is skipped (not an error); the known key still
        // applies, so a partially-compatible screen restores what it can.
        let f = write_script(
            "state x = 0\n\
             draw_rect(x, 0, 1, 1, 0, 0, 0)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);

        let mut map = serde_json::Map::new();
        map.insert("x".to_string(), serde_json::json!(9));
        map.insert("nope".to_string(), serde_json::json!(1));
        let applied = host.restore_state(&map);
        assert_eq!(applied, 1); // only `x` applied; `nope` skipped

        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 9,
                y: 0,
                w: 1,
                h: 1,
                r: 0,
                g: 0,
                b: 0,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn host_data_reaches_the_provider_and_converts() {
        // The script asks the provider for a record and draws from its fields;
        // the provider sees the (kind, arg) pair the script passed.
        let f = write_script(
            "state d = { x: 0, label: \"\", flag: false, items: [] }\n\
             if frame_count() == 0 then d = host_data(\"commit\", \"abc\") end\n\
             if d.flag then\n\
               draw_rect(d.x, len(d.items), 1, 1, 0, 0, 0)\n\
               draw_text(d.label, 0, 0, 14, 1, 1, 1)\n\
             end\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(50, 50);
        let seen: std::rc::Rc<RefCell<Vec<(String, String)>>> = Default::default();
        let seen2 = seen.clone();
        host.set_data_provider(Box::new(move |kind, arg| {
            seen2.borrow_mut().push((kind.to_string(), arg.to_string()));
            PanelData::Record(vec![
                ("x".into(), PanelData::Int(7)),
                ("label".into(), PanelData::Str("hi".into())),
                ("flag".into(), PanelData::Bool(true)),
                (
                    "items".into(),
                    PanelData::List(vec![PanelData::Int(1), PanelData::Int(2)]),
                ),
            ])
        }));
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            *seen.borrow(),
            vec![("commit".to_string(), "abc".to_string())]
        );
        assert_eq!(
            cmds,
            vec![
                PanelCmd::Rect {
                    x: 7,
                    y: 2,
                    w: 1,
                    h: 1,
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                    radius: 0
                },
                PanelCmd::plain_text("hi", 0, 0, 14, 1, 1, 1, 255),
            ]
        );
        // The fetched record persists in `state`; no second provider call.
        let cmds2 = host.frame(0.016, 1).unwrap();
        assert_eq!(cmds2.len(), 2);
        assert_eq!(seen.borrow().len(), 1);
    }

    #[test]
    fn host_data_carries_floats_end_to_end() {
        // The failure this arrangement exists to catch: a client hands the
        // panel a fractional value, the data channel rounds it to an int, and
        // the drawer renders an empty meter. Engine-side tests all pass because
        // the client's own model is fine — only the crossing loses the fraction.
        // A 0.42 fill must scale to a 42px bar, not a 0px one.
        let f = write_script(
            "state fill = 0.0\n\
             if frame_count() == 0 then fill = host_data(\"hud\", \"cpu\") end\n\
             draw_rect(0, 0, fill * 100, 4, 0, 0, 0)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(100, 10);
        host.set_data_provider(Box::new(|_, _| PanelData::Float(0.42)));
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 0,
                y: 0,
                w: 42,
                h: 4,
                r: 0,
                g: 0,
                b: 0,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn host_data_without_provider_is_nil() {
        let f = write_script(
            "let d = host_data(\"anything\", \"\")\n\
             if d == nil then draw_rect(1, 1, 1, 1, 0, 0, 0) end\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        let cmds = host.frame(0.0, 0).unwrap();
        assert_eq!(
            cmds,
            vec![PanelCmd::Rect {
                x: 1,
                y: 1,
                w: 1,
                h: 1,
                r: 0,
                g: 0,
                b: 0,
                a: 255,
                radius: 0
            }]
        );
    }

    #[test]
    fn provider_survives_a_script_error_frame() {
        // A runtime error mid-frame must not leak the provider into the
        // thread-local (it would be lost for the next frame).
        let f = write_script(
            "let d = host_data(\"k\", \"a\")\n\
             boom_undefined()\n",
        );
        // Compile may fail at load (unknown symbol); if it loads, the frame errors.
        if let Ok(mut host) = PanelHost::load(f.path()) {
            host.set_dimensions(10, 10);
            host.set_data_provider(Box::new(|_, _| PanelData::Int(1)));
            let _ = host.frame(0.0, 0);
            assert!(host.has_data_provider());
        }
    }

    #[test]
    fn runtime_error_is_reported() {
        // Calling an unregistered fn is a runtime/compile failure.
        let f = write_script("draw_hexagon(1,2,3,4,5,6)\n");
        let host = PanelHost::load(f.path());
        // Either load (compile) or the first frame surfaces the error; both are
        // acceptable. Here the unknown symbol fails at load.
        if let Ok(mut host) = host {
            host.set_dimensions(10, 10);
            assert!(host.frame(0.0, 0).is_err());
        }
    }

    #[test]
    fn reload_source_swaps_program_and_preserves_state() {
        // A panel with a persisted counter that draws a rect at width `n`.
        let f = write_script(
            "state n = 0\n\
             n = n + 1\n\
             draw_rect(n, 0, 1, 1, 0, 0, 0)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        // Advance state to n = 2.
        host.frame(0.0, 0).unwrap();
        let c = host.frame(0.0, 1).unwrap();
        assert_eq!(width_of(&c), 2);

        // Live-reload from a NEW source (draws the counter at height instead of
        // width, and doubles it). `state n` must survive the swap.
        host.reload_source(
            "state n = 0\n\
             n = n + 1\n\
             draw_rect(0, 0, 1, n * 10, 0, 0, 0)\n",
        )
        .unwrap();
        let c = host.frame(0.0, 2).unwrap();
        // n continued from 2 → 3, and the new program drew height = n*10 = 30.
        match &c[0] {
            PanelCmd::Rect { h, .. } => assert_eq!(*h, 30),
            other => panic!("expected a rect, got {other:?}"),
        }
    }

    #[test]
    fn reload_source_rejects_bad_source_and_keeps_running_program() {
        let f = write_script("draw_rect(5, 0, 1, 1, 0, 0, 0)\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(10, 10);
        assert_eq!(width_of(&host.frame(0.0, 0).unwrap()), 5);

        // A source that fails to compile is rejected; the old program stays live.
        assert!(host.reload_source("draw_hexagon(nope\n").is_err());
        assert_eq!(width_of(&host.frame(0.0, 1).unwrap()), 5);
    }

    /// The `x`/width of the first `Rect` in a frame's commands (test helper).
    fn width_of(cmds: &[PanelCmd]) -> i32 {
        match cmds.first() {
            Some(PanelCmd::Rect { x, .. }) => *x,
            other => panic!("expected a rect first, got {other:?}"),
        }
    }
    /// The persistence contract end to end: a script writes a key, and a
    /// *fresh host over the same script file* reads it back. This is the whole
    /// point ("a todo app remembers your todos"), and it is the assertion that
    /// would catch the store being scoped to the wrong thing.
    #[test]
    fn a_panel_reads_back_what_an_earlier_run_stored() {
        let _guard = crate::panel_store::lock_store_env();
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: guarded by STORE_ENV_LOCK — see `panel_store`.
        unsafe { std::env::set_var("GARDEN_PANEL_STORE_DIR", dir.path()) };

        let writer = write_script(
            "let seen = panel_store_get(\"visits\") ?? \"0\"\n\
             panel_store_set(\"visits\", str(int(seen) + 1))\n\
             draw_text(seen, 0, 0, 12, 255, 255, 255)\n",
        );

        // Three separate hosts over the same file: each sees the last one's
        // write, so the count climbs across "restarts".
        let mut seen = Vec::new();
        for _ in 0..3 {
            let mut host = PanelHost::load(writer.path()).unwrap();
            host.set_dimensions(50, 50);
            let cmds = host.frame(0.016, 0).unwrap();
            match &cmds[0] {
                PanelCmd::Text { text, .. } => seen.push(text.clone()),
                other => panic!("expected the stored value drawn, got {other:?}"),
            }
        }
        assert_eq!(seen, vec!["0", "1", "2"]);

        // A different script does not see those keys — the store is scoped to
        // the script's own path, not shared across the process.
        let other =
            write_script("draw_text(panel_store_get(\"visits\") ?? \"none\", 0, 0, 12, 1, 2, 3)\n");
        let mut host = PanelHost::load(other.path()).unwrap();
        host.set_dimensions(50, 50);
        match &host.frame(0.016, 0).unwrap()[0] {
            PanelCmd::Text { text, .. } => assert_eq!(text, "none"),
            other => panic!("expected nil-defaulted text, got {other:?}"),
        }

        unsafe { std::env::remove_var("GARDEN_PANEL_STORE_DIR") };
    }

    /// A store write is only visible to the panel whose frame is running, so a
    /// script that errors — or a host with no store at all — cannot reach into
    /// another's. Deleting a key round-trips too.
    #[test]
    fn a_deleted_key_reads_back_as_nil() {
        let _guard = crate::panel_store::lock_store_env();
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: guarded by STORE_ENV_LOCK — see `panel_store`.
        unsafe { std::env::set_var("GARDEN_PANEL_STORE_DIR", dir.path()) };
        let f = write_script(
            "state step = 0\n\
             step = step + 1\n\
             if step == 1 then\n\
             \x20 panel_store_set(\"k\", \"v\")\n\
             else\n\
             \x20 panel_store_set(\"k\", nil)\n\
             end\n\
             draw_text(panel_store_get(\"k\") ?? \"gone\", 0, 0, 12, 1, 2, 3)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(50, 50);
        let first = host.frame(0.016, 0).unwrap();
        assert!(matches!(&first[0], PanelCmd::Text { text, .. } if text == "v"));
        let second = host.frame(0.016, 1).unwrap();
        assert!(matches!(&second[0], PanelCmd::Text { text, .. } if text == "gone"));
        unsafe { std::env::remove_var("GARDEN_PANEL_STORE_DIR") };
    }

    /// A non-string value is refused at the call site with a message that says
    /// what to do, rather than being stringified into something unparseable.
    #[test]
    fn storing_a_non_string_is_an_error() {
        let _guard = crate::panel_store::lock_store_env();
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: guarded by STORE_ENV_LOCK — see `panel_store`.
        unsafe { std::env::set_var("GARDEN_PANEL_STORE_DIR", dir.path()) };
        let f = write_script("panel_store_set(\"k\", [1, 2, 3])\n");
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(50, 50);
        let err = host.frame(0.016, 0).unwrap_err();
        assert!(err.contains("must be a string"), "{err}");
        unsafe { std::env::remove_var("GARDEN_PANEL_STORE_DIR") };
    }

    /// Every new primitive survives the round trip from the script's call to
    /// the host's render vocabulary — the seam where a missing `from_draw` arm
    /// would silently drop a shape.
    #[test]
    fn the_new_primitives_reach_the_render_vocabulary() {
        let f = write_script(
            "draw_polyline([[0, 0], [10, 5]], 1, 2, 3, 128, 4)\n\
             draw_ellipse(5, 6, 7, 8, 9, 10, 11)\n\
             draw_circle_outline(1, 2, 3, 4, 5, 6, 255, 2)\n\
             fill_arc(1, 2, 3.0, 9.0, 0.0, 1.5, 7, 8, 9)\n\
             fill_polygon([[0, 0], [10, 0], [10, 10], [0, 10]], 1, 2, 3)\n\
             fill_fan(5, 5, [[0, 0], [10, 0]], 1, 2, 3)\n\
             draw_rect_rounded_outline(0, 0, 20, 20, 4, 1, 2, 3, 255, 2)\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(50, 50);
        let cmds = host.frame(0.016, 0).unwrap();
        assert_eq!(
            cmds,
            vec![
                PanelCmd::Polyline {
                    points: vec![(0, 0), (10, 5)],
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 128,
                    width: 4,
                },
                PanelCmd::Ellipse {
                    cx: 5,
                    cy: 6,
                    rx: 7,
                    ry: 8,
                    r: 9,
                    g: 10,
                    b: 11,
                    a: 255,
                },
                PanelCmd::EllipseOutline {
                    cx: 1,
                    cy: 2,
                    rx: 3,
                    ry: 3,
                    r: 4,
                    g: 5,
                    b: 6,
                    a: 255,
                    width: 2,
                },
                PanelCmd::Arc {
                    cx: 1,
                    cy: 2,
                    r_in: 3.0,
                    r_out: 9.0,
                    a0: 0.0,
                    a1: 1.5,
                    r: 7,
                    g: 8,
                    b: 9,
                    a: 255,
                },
                PanelCmd::Polygon {
                    points: vec![(0, 0), (10, 0), (10, 10), (0, 10)],
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 255,
                },
                PanelCmd::Fan {
                    cx: 5,
                    cy: 5,
                    points: vec![(0, 0), (10, 0)],
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 255,
                },
                PanelCmd::RectOutline {
                    x: 0,
                    y: 0,
                    w: 20,
                    h: 20,
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 255,
                    width: 2,
                    radius: 4,
                },
            ]
        );
    }

    #[test]
    fn frames_do_not_accumulate_closures() {
        // A panel re-runs its whole script every frame, so every frame
        // re-executes its `fn` declarations and allocates a fresh closure for
        // each. Those from earlier frames are garbage; if the runtime keeps
        // them (it did until petal's closures became collectable) a panel leaks
        // for as long as its window is open — a few hundred KB a second at
        // 60fps, gigabytes over an afternoon.
        let f = write_script(
            "fn box_at(i)\n\
               let hue = i * 7\n\
               let shade = fn(c) -> c + hue\n\
               draw_rect(i * 3, 4, 3, 4, shade(10), shade(20), shade(30))\n\
             end\n\
             for i in range(0, 30) do\n\
               box_at(i)\n\
             end\n",
        );
        let mut host = PanelHost::load(f.path()).unwrap();
        host.set_dimensions(200, 120);
        host.frame(0.016, 0).unwrap();
        let after_first = host.env.closures().closure_count();
        for i in 1..1000 {
            host.frame(0.016, i).unwrap();
        }
        let after_many = host.env.closures().closure_count();
        assert!(
            after_many < after_first * 100,
            "closures grew from {after_first} to {after_many} over 1000 frames"
        );
    }
}

/// End-to-end source tracing: a panel script draws shapes, the host traces a
/// point on the canvas back to the `draw_*` call that put a shape there, and on
/// to the argument literals that positioned it.
#[cfg(test)]
mod trace_tests {
    use super::*;
    use crate::panel_trace::{hit_test, ArgSource};

    /// Line 1 clears, line 2 draws a circle on the left, line 3 a rect on the
    /// right. Line numbers are what the assertions below check, so keep them.
    const SKETCH: &str = "clear(10, 10, 10)\n\
                          draw_circle(100, 100, 40, 200, 80, 80)\n\
                          draw_rect(300, 50, 80, 60, 80, 200, 120)\n";

    fn traced_host(source: &str) -> PanelHost {
        let mut host = PanelHost::from_source("trace-test", source).unwrap();
        host.set_trace_origins(true);
        host.set_dimensions(600, 400);
        host
    }

    /// The whole feature in one assertion: a point on the canvas resolves to the
    /// line of source that drew what is there.
    #[test]
    fn a_point_on_the_canvas_traces_to_the_line_that_drew_it() {
        let mut host = traced_host(SKETCH);
        let cmds = host.frame(0.016, 1).unwrap();

        // Inside the circle → line 2.
        let i = hit_test(&cmds, 100, 100).expect("the circle is hit");
        let origin = host.origin_at(i).expect("an attributed command");
        let trace = host.trace_origin(origin).expect("resolves to source");
        assert_eq!(trace.callee.as_deref(), Some("draw_circle"));
        assert_eq!(trace.call.expect("a span").start_line, 1, "0-based line 2");

        // Inside the rect → line 3.
        let i = hit_test(&cmds, 320, 70).expect("the rect is hit");
        let origin = host.origin_at(i).expect("an attributed command");
        let trace = host.trace_origin(origin).expect("resolves to source");
        assert_eq!(trace.callee.as_deref(), Some("draw_rect"));
        assert_eq!(trace.call.expect("a span").start_line, 2, "0-based line 3");
    }

    /// The traced call carries each argument's literal value, which is what a
    /// drag mode needs in order to write a new one back.
    #[test]
    fn a_traced_call_exposes_its_argument_literals() {
        let mut host = traced_host(SKETCH);
        let cmds = host.frame(0.016, 1).unwrap();
        let i = hit_test(&cmds, 100, 100).unwrap();
        let trace = host.trace_origin(host.origin_at(i).unwrap()).unwrap();

        assert_eq!(trace.args.len(), 6, "cx, cy, r, and three color channels");
        let cx = &trace.args[0];
        assert_eq!(cx.source, ArgSource::Literal);
        assert_eq!(cx.value, Some(100.0));
        assert!(cx.is_int);
        // The span to rewrite is the `100` itself, on the line of the call.
        assert_eq!(cx.editable_span.expect("a span").start_line, 1);
    }

    /// An argument that comes from a `let` is flagged as a binding, and points
    /// at the *definition* — a drag there changes every shape that reads it, and
    /// the editor needs to know that before it writes.
    #[test]
    fn an_argument_from_a_binding_points_at_its_definition() {
        let mut host = traced_host(
            "let radius = 40\n\
             draw_circle(100, 100, radius, 200, 80, 80)\n",
        );
        let cmds = host.frame(0.016, 1).unwrap();
        let i = hit_test(&cmds, 100, 100).unwrap();
        let trace = host.trace_origin(host.origin_at(i).unwrap()).unwrap();

        let r = &trace.args[2];
        assert_eq!(r.source, ArgSource::Binding);
        assert_eq!(r.value, Some(40.0));
        assert_eq!(
            r.editable_span.expect("a span").start_line,
            0,
            "the `40` on line 1, not the `radius` on line 2"
        );
    }

    /// Tracing is opt-in: an ordinary panel records nothing, and the miss is
    /// reported as a miss rather than as a wrong answer.
    #[test]
    fn an_untraced_host_records_no_origins() {
        let mut host = PanelHost::from_source("untraced", SKETCH).unwrap();
        host.set_dimensions(600, 400);
        let cmds = host.frame(0.016, 1).unwrap();
        assert!(!cmds.is_empty(), "it still draws");
        assert!(host.origin_at(0).is_none(), "but attributes nothing");
    }

    /// Origins are rebuilt per frame and stay aligned with that frame's command
    /// list — a frame whose shape count changes must not leave the previous
    /// frame's attribution behind to be read against the new indices.
    #[test]
    fn origins_are_rebuilt_each_frame() {
        let mut host = traced_host(SKETCH);
        let first = host.frame(0.016, 1).unwrap();
        let second = host.frame(0.016, 2).unwrap();
        assert_eq!(first.len(), second.len());
        for i in 0..second.len() {
            assert!(host.origin_at(i).is_some(), "command {i} is attributed");
        }
        assert!(
            host.origin_at(second.len()).is_none(),
            "and nothing past the end of this frame's list"
        );
    }
}
