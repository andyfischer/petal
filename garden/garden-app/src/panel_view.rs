//! The app-side half of a `panel(...)` pane: wraps a [`PanelHost`] (the Petal
//! runtime in `garden-script`) with the per-pane animation bookkeeping and
//! turns the script's [`PanelCmd`]s into render [`Primitive`]s.
//!
//! ## Animation: sleep/wake
//!
//! Garden has no continuous render loop, so a panel can't just animate forever.
//! Instead it stays *awake* for [`PANEL_WAKE`] after the last activity (any user
//! input, plus spawn / reload / resize), ticking ~60fps while awake and sleeping
//! afterward. [`App::tick_panels`](crate::app::App) drives this; see
//! `docs/petal-graphical-panels.md`.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use garden_core::projection::{
    ChromeRole, Decor, LineOrigin, NewLine, Projection, SourceEdit, Span,
};
use garden_render::{Color, Primitive, Rect, TextStyle, Vertex, FONT_SIZE};
use garden_script::{
    DrawTrace, InputEvent, Modifiers, NavIntent, PanelCmd, PanelData, PanelHost, PanelInput,
    ProjectionSpec,
};

use crate::app::Mods;
use crate::editor_view::{EditorView, ScrollAxis};

use crate::panel_tess as tess;
use crate::process_pane::ProcessPane;
use crate::script_client::{ProcessQueryProvider, Shared};
use crate::theme::Theme;

/// The renderer's real glyph advances (advance ÷ font size, by codepoint),
/// measured once and reused. Measuring builds a throwaway `FontSystem` and
/// shapes the ASCII range, which is far too much work to redo for every panel
/// load and navigation — but the embedded font is fixed at compile time, so the
/// answer never changes. This caches a pure function's result; it is not a
/// place to *configure* the metric. Each [`PanelHost`] is told individually
/// (see [`adopt_font`]), so a host is free to measure differently.
fn measured_advance_ratios() -> &'static [f64] {
    static RATIOS: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    RATIOS.get_or_init(garden_render::ascii_advance_ratios)
}

/// [`measured_advance_ratios`] for the proportional UI face — the table behind
/// a script's `text_width(s, size, "ui")`. Cached the same way and for the same
/// reason: measuring a face is a pure function of the build.
fn measured_ui_advance_ratios() -> &'static [f64] {
    static RATIOS: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    RATIOS.get_or_init(garden_render::ui_ascii_advance_ratios)
}

/// The advance table a run in this role measures with, so the pen in
/// [`push_text_run`] steps exactly as `text_width` says it will.
fn advance_ratios_for(role: garden_render::FontRole) -> &'static [f64] {
    match role {
        garden_render::FontRole::Mono => measured_advance_ratios(),
        garden_render::FontRole::Ui => measured_ui_advance_ratios(),
    }
}

/// Emit a script's text run as scene primitives.
///
/// Normally that is one [`Primitive::Text`]. A run with letter-spacing becomes
/// one primitive **per glyph**, because cosmic-text has no letter-spacing of
/// its own: the pen advances by the glyph's measured advance plus the spacing,
/// which is exactly the sum `text_width` reports for the same style, so a
/// spaced run measures and draws the same width. The cost (one shaped run per
/// character) is why only spaced text pays it.
fn push_text_run(
    prims: &mut Vec<Primitive>,
    (x, y): (f32, f32),
    text: &str,
    color: Color,
    clip: Rect,
    size: f32,
    style: TextStyle,
) {
    if style.spacing == 0.0 || text.is_empty() {
        prims.push(Primitive::Text {
            pos: (x, y),
            text: text.to_string(),
            color,
            clip,
            size,
            style,
        });
        return;
    }
    let ratios = advance_ratios_for(style.font);
    let mut pen = x;
    for ch in text.chars() {
        let advance = ratios
            .get(ch as usize)
            .copied()
            .unwrap_or(FALLBACK_ADVANCE_RATIO) as f32
            * size;
        prims.push(Primitive::Text {
            pos: (pen, y),
            text: ch.to_string(),
            color,
            clip,
            size,
            // The spacing is already in the pen positions; a per-glyph run must
            // not also carry it, or a future renderer would apply it twice.
            style: TextStyle {
                spacing: 0.0,
                ..style
            },
        });
        pen += advance + style.spacing;
    }
}

/// Advance ratio for codepoints outside the measured table — the same
/// monospace estimate `garden-script` falls back to, so the pen and the
/// script's `text_width` stay in step even off the table.
const FALLBACK_ADVANCE_RATIO: f64 = 0.6;

/// Publish this app's font measurement into a freshly built host, so the
/// script's `text_width()` agrees with what glyphon rasterizes instead of
/// assuming a 0.6 monospace ratio. Every path that builds a `PanelHost` for a
/// pane funnels through here — construction, both rebuild paths, and restart —
/// because the ratios live in the host's env and a rebuilt host starts over
/// with the estimate.
fn adopt_font(host: &mut PanelHost) {
    host.set_font_advance_ratios_with_ui(
        measured_advance_ratios().to_vec(),
        measured_ui_advance_ratios().to_vec(),
    );
}

/// A native read-only editor rendered inside one `text_view(...)` region of a
/// panel — the machinery that gives a panel (e.g. git log mode) a
/// buffer-backed, natively-selectable, clipboard-copyable text area instead of
/// script-drawn glyphs. Keyed by the region's stable `id` so it survives the
/// per-frame script rerun; `content_hash` gates the (expensive) buffer rebuild
/// so a diff re-declared unchanged at 60fps is a no-op and selection/scroll
/// persist. `rect` is panel-local (translate by the pane origin to render/hit-test).
struct EmbeddedText {
    view: EditorView,
    content_hash: u64,
    /// Hash of the last-applied per-line style names, so styling is only rebuilt
    /// when it (or the content) actually changes.
    styles_hash: u64,
    rect: Rect,
    /// Whether this region is an `edit_view` (host routes vim keystrokes into it)
    /// or a read-only `text_view` (selection/scroll/copy only). Set each frame
    /// from the declaring [`PanelCmd::TextView`].
    editable: bool,
    /// Hash of the last-applied [`ProjectionSpec`], so the region's projection —
    /// and with it every line's recorded origin, and the user's edits so far —
    /// is rebuilt only when the declared provenance genuinely changes, not on
    /// each of the 60 frames a second that re-declare the same one.
    projection_hash: u64,
    /// The write-back the region last resolved to, with the buffer revision it
    /// was computed at. Folding a large diff is cheap but not free, and an idle
    /// panel re-publishes every tick — so it is recomputed only after an edit.
    edits_cache: Option<(u64, PanelData)>,
}

impl EmbeddedText {
    /// The region's absolute (screen) rect, given the panel's pane rect —
    /// `rect` is stored panel-local. Used for both rendering and hit-testing.
    fn abs_rect(&self, pane_rect: Rect) -> Rect {
        Rect {
            x: pane_rect.x + self.rect.x,
            y: pane_rect.y + self.rect.y,
            w: self.rect.w,
            h: self.rect.h,
        }
    }
}

/// Build a [`Projection`] from the flat spec a drawer declared, reading each
/// ghost line's base text out of `seed` — the projected text itself. That is
/// what keeps the wire shape small: a deleted line's content is already on
/// screen, so only its *kind* has to be transmitted.
fn projection_from_spec(spec: &ProjectionSpec, seed: &str) -> Projection {
    let spans = (0..spec.span_source.len())
        .map(|i| Span {
            source: spec.span_source[i].max(0) as u32,
            target: (
                spec.span_start.get(i).copied().unwrap_or(0).max(0) as usize,
                spec.span_end.get(i).copied().unwrap_or(0).max(0) as usize,
            ),
            group: spec
                .span_group
                .get(i)
                .copied()
                .filter(|g| *g >= 0)
                .map(|g| g as u32),
        })
        .collect();
    let decor = Decor {
        same: (spec.decor.same.clone(), spec.decor.same_style.clone()),
        added: (spec.decor.added.clone(), spec.decor.added_style.clone()),
        removed: (spec.decor.removed.clone(), spec.decor.removed_style.clone()),
        new_line: if spec.decor.diff_markers {
            NewLine::DiffMarker
        } else {
            NewLine::Literal
        },
        gutter: spec.decor.gutter,
    };
    let mut proj = Projection::new(spec.sources.clone(), spans, decor);
    let lines: Vec<&str> = seed.lines().collect();
    for (i, kind) in spec.kinds.chars().enumerate() {
        let chrome = |role, locked| LineOrigin::Chrome { role, locked };
        let origin = match kind {
            '+' => LineOrigin::Live { added: true },
            '-' => LineOrigin::Ghost {
                text: lines
                    .get(i)
                    .map(|l| l.strip_prefix(&spec.decor.removed).unwrap_or(l).to_string())
                    .unwrap_or_default(),
            },
            'l' => chrome(ChromeRole::Plain, true),
            'h' => chrome(ChromeRole::SpanHeader, false),
            'g' => chrome(ChromeRole::GroupHeader, false),
            'c' => chrome(ChromeRole::Plain, false),
            _ => LineOrigin::Live { added: false },
        };
        let style = spec.styles.get(i).map(String::as_str).unwrap_or("");
        let span = spec
            .line_spans
            .get(i)
            .copied()
            .filter(|v| *v >= 0)
            .map(|v| v as u32);
        proj.push(origin, style, span);
    }
    proj
}

/// A region's buffer as the projection reads it — the same line splitting both
/// [`Projection::capture_baseline`] and [`Projection::resolve`] are fed, so a
/// baseline and the folds compared against it can never disagree over what a
/// line is.
fn buffer_lines(view: &EditorView) -> Vec<String> {
    (0..view.buffer.line_count())
        .map(|i| view.buffer.line(i))
        .collect()
}

/// Project the resolved write-backs into the neutral value tree the panel host
/// hands a script (`edit_view_edits(id)` turns it into a Petal list of records).
/// `expected` rides along as the lines the span held when the view loaded, so
/// the writer can notice the source changed underneath it; a projection with no
/// captured baseline sends nil, meaning "no expectation".
fn source_edits_to_data(edits: &[SourceEdit]) -> PanelData {
    PanelData::List(
        edits
            .iter()
            .map(|e| {
                PanelData::Record(vec![
                    ("source".to_string(), PanelData::Str(e.source.clone())),
                    ("start".to_string(), PanelData::Int(e.start as i64)),
                    ("end".to_string(), PanelData::Int(e.end as i64)),
                    (
                        "lines".to_string(),
                        PanelData::List(
                            e.lines.iter().map(|l| PanelData::Str(l.clone())).collect(),
                        ),
                    ),
                    (
                        "expected".to_string(),
                        match &e.expected {
                            Some(lines) => PanelData::List(
                                lines.iter().map(|l| PanelData::Str(l.clone())).collect(),
                            ),
                            None => PanelData::Nil,
                        },
                    ),
                ])
            })
            .collect(),
    )
}

/// Apply a region's soft-wrap flag to its editor, measuring the viewport from
/// the region's own rect (panel-local — only its size matters here). A region
/// starts unwrapped (`set_external_content` turns wrapping off for every
/// externally-supplied buffer), so this is what a script's `text_view_wrap`
/// re-enables, on the same pass that may have just rebuilt the buffer.
fn set_region_wrap(view: &mut EditorView, rect: Rect, cell: (f32, f32), wrap: bool) {
    let visible = EditorView::visible_lines(rect, cell.1);
    let cols = view.visible_cols(rect, cell.0);
    view.set_wrap(wrap, visible, cols);
}

/// Hash a region's text to detect real content changes across frames.
fn hash_text(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Hash a region's per-line style names to detect styling changes across frames.
fn hash_styles(styles: &[String]) -> u64 {
    let mut h = DefaultHasher::new();
    styles.hash(&mut h);
    h.finish()
}

/// Fingerprint of a declared [`ProjectionSpec`], so a drawer re-declaring the
/// same provenance every frame never costs a rebuild (which would discard the
/// user's edits along with the table).
fn hash_projection(spec: &ProjectionSpec) -> u64 {
    let mut h = DefaultHasher::new();
    spec.sources.hash(&mut h);
    spec.span_source.hash(&mut h);
    spec.span_start.hash(&mut h);
    spec.span_end.hash(&mut h);
    spec.span_group.hash(&mut h);
    spec.kinds.hash(&mut h);
    spec.line_spans.hash(&mut h);
    spec.styles.hash(&mut h);
    let d = &spec.decor;
    (
        &d.same,
        &d.added,
        &d.removed,
        &d.same_style,
        &d.added_style,
        &d.removed_style,
        d.diff_markers,
    )
        .hash(&mut h);
    h.finish()
}

/// How long a panel keeps ticking after its last activity before it sleeps.
pub const PANEL_WAKE: Duration = Duration::from_secs(10);

/// The live wake window in milliseconds, defaulting to [`PANEL_WAKE`] and
/// overridable process-wide by `--panel-wake` (see [`set_panel_wake`]). A
/// running game or animation is exactly the case where sleeping after ten idle
/// seconds is wrong, and a headless harness driving one has no user input to
/// keep re-stamping activity with.
static PANEL_WAKE_MS: AtomicU64 = AtomicU64::new(PANEL_WAKE.as_secs() * 1_000);

/// Override the panel wake window process-wide. `None` means "never sleep".
/// Set once from the command line before any frontend starts.
pub fn set_panel_wake(window: Option<Duration>) {
    let ms = match window {
        // ~584 million years: "forever" without a second code path in `is_awake`.
        None => u64::MAX,
        Some(d) => d.as_millis().min(u64::MAX as u128) as u64,
    };
    PANEL_WAKE_MS.store(ms, Ordering::Relaxed);
}

/// The wake window in force (see [`set_panel_wake`]).
pub fn panel_wake() -> Duration {
    Duration::from_millis(PANEL_WAKE_MS.load(Ordering::Relaxed))
}
/// Target frame interval while a panel is awake (~60fps).
pub const PANEL_FRAME: Duration = Duration::from_millis(16);

/// Largest `dt` handed to a script, so a frame after a long sleep doesn't see a
/// huge time step.
const MAX_DT: f64 = 0.1;

/// How many of a panel's most recent `print(...)` lines are kept for the debug
/// channel. A panel that prints every frame would otherwise grow the buffer
/// without bound between reads; the newest lines are the interesting ones.
const OUTPUT_CAP: usize = 200;

pub struct PanelView {
    host: PanelHost,
    frame_count: i64,
    /// When the previous frame ran, for computing `dt`.
    last_frame: Option<Instant>,
    /// When the panel last saw activity; it animates until `last_activity +
    /// PANEL_WAKE`.
    last_activity: Instant,
    /// The most recent frame's draw commands — what [`build_scene`] renders.
    cmds: Vec<PanelCmd>,
    /// Whether the last [`tick`](Self::tick) frame changed the drawn output
    /// (commands or error text) — the steadiness signal
    /// [`App::settle_panels`](crate::app::App::settle_panels) converges on.
    last_tick_changed: bool,
    /// Every named binding of the last **successful** frame, as JSON keyed by
    /// (function-qualified) name — Petal's observation buffer, surfaced to the
    /// debug server so an interactive panel's logical state is assertable.
    ///
    /// Cached here rather than read through on demand, and for the same reason
    /// [`cmds`](Self::cmds) is: a frame that failed leaves the runtime's buffer
    /// holding whatever the aborted run got through, so the values a reader
    /// wants — the ones matching the picture still on screen — are the last good
    /// frame's.
    observed: serde_json::Map<String, serde_json::Value>,
    /// The panel frame number [`observed`](Self::observed) was captured on. A
    /// reader that only ever sees the *last good* map cannot tell a value that
    /// is current from one that is several failed frames stale — and an absent
    /// key reads as "that branch never ran" when it in fact means "the frame
    /// that would have bound it blew up". Stamping the frame makes both
    /// self-diagnosing.
    observed_frame: i64,
    /// The observation buffer of the most recent frame that **errored**, with
    /// the frame number it came from: whatever the aborted run got through
    /// before raising. Kept separate from [`observed`](Self::observed) (which
    /// still matches the picture on screen) so a debugger can see how far the
    /// broken frame got without the good values being overwritten by a partial
    /// set. Cleared by the next successful frame.
    partial_observed: Option<(i64, serde_json::Map<String, serde_json::Value>)>,
    /// Set when a **reload** (disk `poll_reload`, live `reload_from_editor`, or a
    /// GPP `setScript` push) fails to compile the *new* source. The old program
    /// keeps running (so the last good frame stays on screen), so this must NOT be
    /// cleared by that program's successful frames — only by a reload that
    /// compiles. It is what keeps the error banner up while your buffer is broken.
    reload_error: Option<String>,
    /// Set when the last frame of the *running* program raised a runtime error;
    /// cleared by the next successful frame. Distinct from [`reload_error`](Self::reload_error),
    /// which is about compiling new source.
    frame_error: Option<String>,
    /// The script's own `print(...)` lines, drained from the host after every
    /// frame and kept here (newest last, capped at [`OUTPUT_CAP`]) until a
    /// consumer takes them — the debug server's `/state` reports them as
    /// `script.output`, which is the only debug channel a panel author has.
    /// Draining every frame also keeps the host's buffer from growing without
    /// bound in a panel that prints each frame.
    output: VecDeque<String>,
    /// Native read-only editors for the panel's `text_view(...)` regions, keyed
    /// by the script's stable region id. Synced from [`PanelCmd::TextView`] each
    /// frame in [`tick`](Self::tick); rendered in [`build_scene`](Self::build_scene).
    text_views: HashMap<i64, EmbeddedText>,
    /// The key chords the last good frame claimed with `claim_key(name, mods)`,
    /// as `(canonical key name, modifier bits or None for "any chord")`. A claim
    /// is re-declared by every frame, so this is replaced (not merged) after
    /// each successful frame; a frame that *errored* leaves the previous claims
    /// standing, since dropping them would silently return the panel's command
    /// keys to the host mid-debugging. Read by
    /// [`App::panel_key`](crate::app::App) before it applies any host shortcut.
    key_claims: Vec<(String, Option<u8>)>,
    /// The modifier chord last pushed with [`set_modifiers`](Self::set_modifiers),
    /// so a change can be republished as `keys_down` entries — `key_down("shift")`
    /// used to return false forever, which failed silently.
    mods: Mods,
    /// Which `text_view` region (if any) currently owns keyboard focus — set when
    /// the pointer presses inside a region, so Cmd-C copies that region's
    /// selection. `None` means keys go to the script as usual.
    focused_region: Option<i64>,
    /// Hash of the source last applied by [`reload_from_editor`](Self::reload_from_editor)
    /// (the Petal-IDE live binding). Gates the recompile so an unchanged buffer
    /// re-scanned every awake frame is a no-op. `None` until the first live apply.
    live_source_hash: Option<u64>,
    /// For a **panel-mode GPP pane** (the script-push protocol): the subprocess
    /// that pushed this script and answers its `query` requests, plus the shared
    /// query cache the attached [`ProcessQueryProvider`] reads. `None` for an
    /// in-process built-in panel (`:Diff`/`:Git`), whose provider is local Rust.
    /// See [`pump_client`](Self::pump_client) and
    /// `docs/gpp.md`.
    client: Option<ProcessPane>,
    client_shared: Option<Shared>,
    /// The name to rebuild the host under when the client re-pushes its script.
    client_name: String,
    /// Browser-style history: the screens this pane has visited, in order. Seeded
    /// with the initial screen (entry 0) at construction. [`nav_push`](Self::nav_push)
    /// appends, [`nav_replace`](Self::nav_replace) swaps in place, and
    /// [`nav_back`](Self::nav_back)/[`nav_forward`](Self::nav_forward) move
    /// [`cursor`](Self::cursor). Entry 0 is the stable [`origin_script`](Self::origin_script);
    /// the entry at the cursor is the live [`script`](Self::script).
    history: Vec<HistoryEntry>,
    /// Index into [`history`](Self::history) of the currently displayed entry.
    cursor: usize,
    /// `ClientEvent::Navigate`s drained from the host's nav side channel in
    /// [`tick`](Self::tick), surfaced to the app layer via
    /// [`take_nav_events`](Self::take_nav_events).
    nav_events: Vec<ClientEvent>,
    /// Whether this pane traces its drawn shapes back to source
    /// ([`set_trace_origins`](Self::set_trace_origins)) — the Petal-IDE
    /// direct-manipulation mode. Off for every ordinary panel, which therefore
    /// pays nothing for it.
    trace_origins: bool,
    /// The layout-declared explicit navigation allowlist (`panel(script, {
    /// screens: [...] })`). **Empty means not declared** — the implicit
    /// script-directory default applies. When non-empty it *narrows* that
    /// default: a `navigate(...)` target must be a member of this list (in
    /// addition to the directory / traversal / `.ptl` / existence safety checks),
    /// so an off-list screen is refused even though it sits in the directory.
    /// Read by the app layer's navigation resolver ([`screens`](Self::screens)).
    screens: Vec<String>,
}

/// A client-directed signal surfaced by [`PanelView::pump_client`] for the `App`
/// to act on (the panel runtime can't reach the pane set itself).
pub enum ClientEvent {
    /// The script asked the host to open a file in this pane (`openPath`).
    OpenPath(String),
    /// The client set the pane's status text (`setStatus`).
    SetStatus(String),
    /// The script asked to navigate (browser-history API: `navigate` /
    /// `navigate_replace` / `navigate_back` / `navigate_forward`). The app layer
    /// resolves the target screen against the pane's whitelist and drives the
    /// [`PanelView`]'s swap methods. The payload is consumed by the app layer in
    /// the next chunk.
    Navigate(#[allow(dead_code)] NavIntent),
    /// The script called `mutate(name, arg)` — an effectful request for the pane's
    /// subprocess. The app layer relays it to the client
    /// ([`client_mutate`](PanelView::client_mutate)) and surfaces the reply as the
    /// pane's status *and* back to the script under `handle`
    /// ([`resolve_mutation`](PanelView::resolve_mutation)).
    Mutate {
        name: String,
        arg: serde_json::Value,
        /// The handle `mutate(...)` returned to the script, which
        /// `mutate_result(handle)` will read the reply back under.
        handle: i64,
    },
}

/// Render an `on_mutation` reply value as a one-line status string: a JSON string
/// is used verbatim (so `Reply::json("wrote 2 files")` reads cleanly), any other
/// JSON value is compacted to its textual form.
fn json_to_status(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Extract the target screen's source from a `navigate` [`gpp::MutateResult`]: the
/// built-in mutation returns `{ "screen", "source" }`; a handler error (e.g. an
/// undeclared screen) comes back as `error`. Anything else is a malformed reply.
fn mutate_source(m: gpp::MutateResult) -> Result<String, String> {
    if let Some(err) = m.error {
        return Err(err);
    }
    m.value
        .as_ref()
        .and_then(|v| v.get("source"))
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .ok_or_else(|| "navigate: client reply had no source".to_string())
}

/// One visited screen in a panel's browser-style history stack. State is scoped
/// **per entry** (not per screen identity), so revisiting a screen via *back*
/// restores exactly the value that visit held when it was left.
struct HistoryEntry {
    /// The screen identity (the `.ptl` script name/path) as requested.
    screen: String,
    /// The on-disk path this entry rebuilds from, when it is file-backed. Set for
    /// the **seed** entry (entry 0), whose host was loaded from a real `.ptl` file:
    /// revisiting it via *back* rebuilds through [`PanelHost::load`] so the origin
    /// keeps its hot-reload and real path (rebuilding from `source` would strip
    /// both). `None` for navigated entries, which are ephemeral and rebuild from
    /// [`source`](Self::source) instead. Takes precedence over `source`.
    path: Option<PathBuf>,
    /// The resolved source to rebuild the host from when this entry is (re)visited.
    /// `None` for the seed entry (it rebuilds from [`path`](Self::path)) and until
    /// a navigated entry has been created with its source.
    source: Option<String>,
    /// The `state` snapshot captured when navigation last **left** this entry,
    /// re-seeded before its first frame on return. `None` until the entry is left.
    saved_state: Option<serde_json::Map<String, serde_json::Value>>,
    /// The argument the `navigate(screen, arg)` that created this entry carried,
    /// republished to the script as `nav_arg()` every time the entry is
    /// displayed. `Null` for the seed (nothing navigated to it) and for the
    /// one-argument `navigate(screen)` form.
    ///
    /// It lives on the *entry* rather than on the host, which is the whole point:
    /// a detail screen returned to by *back* must come back showing the same
    /// subject, and a host slot would only ever hold the most recent navigation.
    nav_arg: serde_json::Value,
}

impl PanelView {
    /// Wrap a freshly loaded host. A new panel starts awake so its animation
    /// plays immediately on appearing.
    pub fn new(mut host: PanelHost, script: String, now: Instant) -> PanelView {
        adopt_font(&mut host);
        // The seed rebuilds from its real path (not source) so returning home via
        // *back* keeps the origin's hot-reload and file identity.
        let seed = HistoryEntry {
            screen: script,
            path: Some(host.path().to_path_buf()),
            source: None,
            saved_state: None,
            nav_arg: serde_json::Value::Null,
        };
        PanelView {
            host,
            frame_count: 0,
            last_frame: None,
            last_activity: now,
            cmds: Vec::new(),
            last_tick_changed: false,
            observed: serde_json::Map::new(),
            observed_frame: -1,
            partial_observed: None,
            reload_error: None,
            frame_error: None,
            output: VecDeque::new(),
            text_views: HashMap::new(),
            key_claims: Vec::new(),
            mods: Mods::default(),
            focused_region: None,
            live_source_hash: None,
            client: None,
            client_shared: None,
            client_name: String::new(),
            history: vec![seed],
            cursor: 0,
            nav_events: Vec::new(),
            trace_origins: false,
            screens: Vec::new(),
        }
    }

    /// Set the explicit navigation allowlist declared on the panel's layout node
    /// (`panel(script, { screens: [...] })`). Empty leaves the implicit
    /// script-directory default in force. Applied at construction (and carried
    /// with the pane across a rebuild, so a navigated panel keeps its allowlist).
    pub fn set_screens(&mut self, screens: Vec<String>) {
        self.screens = screens;
    }

    /// The explicit navigation allowlist, or an empty slice when none was
    /// declared. The app layer's navigation resolver narrows to these when
    /// non-empty.
    pub fn screens(&self) -> &[String] {
        &self.screens
    }

    /// Attach the pushing subprocess (a panel-mode GPP client) and the shared
    /// query cache its provider reads. After this, [`pump_client`](Self::pump_client)
    /// bridges the script's `query` calls to `query` requests over the pipe.
    /// `name` rebuilds the host if the client re-pushes its script.
    pub fn attach_client(&mut self, client: ProcessPane, shared: Shared, name: String) {
        self.client = Some(client);
        self.client_shared = Some(shared);
        self.client_name = name;
    }

    /// Whether this panel is driven by a pushed-script GPP client.
    pub fn has_client(&self) -> bool {
        self.client.is_some()
    }

    /// Drain the client's pipe and drive the query round-trip for a panel-mode
    /// GPP pane. Applies `queryResult` answers and client-pushed `invalidate`s to
    /// the shared cache, then flushes any queued `query` requests to the client.
    /// A re-pushed `setScript` hot-reloads the host (keeping the query cache).
    ///
    /// With `wait: Some(dur)`, after flushing new requests it blocks up to `dur`
    /// for the client's first answer (and applies whatever arrives) — used to
    /// paint the first frame with data instead of a spinner. `None` is fully
    /// non-blocking, for the steady-state poll tick.
    ///
    /// Returns `(events, changed)`: `events` are `openPath`/`setStatus` for the
    /// `App` to act on; `changed` is true when new data landed (redraw wanted).
    pub fn pump_client(&mut self, wait: Option<Duration>) -> (Vec<ClientEvent>, bool) {
        if self.client.is_none() {
            return (Vec::new(), false);
        }
        let incoming = self.client.as_ref().unwrap().try_drain();
        let (mut events, mut changed) = self.apply_client_envelopes(incoming);

        let sent = self.flush_query_outbox();
        if let (Some(dur), true) = (wait, sent) {
            let more = self.client.as_ref().unwrap().drain_for(dur);
            let (e2, c2) = self.apply_client_envelopes(more);
            events.extend(e2);
            changed |= c2;
            // A second wave of queries (e.g. the diff for the auto-selected row)
            // may have been enqueued while applying the first; flush it too.
            self.flush_query_outbox();
        }
        (events, changed)
    }

    /// Apply a batch of client→host envelopes to the shared cache and collect any
    /// `openPath`/`setStatus` events. Returns `(events, changed)`.
    fn apply_client_envelopes(&mut self, envs: Vec<gpp::Envelope>) -> (Vec<ClientEvent>, bool) {
        let mut events = Vec::new();
        let mut changed = false;
        for env in envs {
            // A `queryResult` is a response (id + result, no method).
            if env.method.is_none() && env.result.is_some() {
                if let (Ok(r), Some(shared)) = (
                    env.result_as::<gpp::QueryResult>(),
                    self.client_shared.as_ref(),
                ) {
                    shared
                        .borrow_mut()
                        .resolve(r.kind, r.arg, r.value, r.error, r.cache_control);
                    changed = true;
                }
                continue;
            }
            match env.method.as_deref() {
                Some(gpp::method::INVALIDATE) => {
                    if let (Ok(p), Some(shared)) = (
                        env.params_as::<gpp::InvalidateParams>(),
                        self.client_shared.as_ref(),
                    ) {
                        shared.borrow_mut().invalidate(&p.kind, &p.arg);
                        changed = true;
                    }
                }
                Some(gpp::method::SET_SCRIPT) => {
                    if let Ok(p) = env.params_as::<gpp::SetScriptParams>() {
                        // A client push (hot-)reloads the CURRENT screen in place;
                        // record it as this history entry's source (dropping any
                        // path) so a later *back* to it rebuilds from the pushed
                        // source, not a bogus file path. See `set_origin_source`.
                        self.history[self.cursor].source = Some(p.source.clone());
                        self.history[self.cursor].path = None;
                        self.reload_from_source(&p.source);
                        changed = true;
                    }
                }
                Some(gpp::method::OPEN_PATH) => {
                    if let Ok(p) = env.params_as::<gpp::OpenPathParams>() {
                        events.push(ClientEvent::OpenPath(p.path));
                    }
                }
                Some(gpp::method::SET_STATUS) => {
                    if let Ok(p) = env.params_as::<gpp::SetStatusParams>() {
                        events.push(ClientEvent::SetStatus(p.text));
                    }
                }
                _ => {}
            }
        }
        (events, changed)
    }

    /// Send every queued `(kind, arg)` request to the client. Returns whether any
    /// were sent this call.
    fn flush_query_outbox(&mut self) -> bool {
        let outbox = self
            .client_shared
            .as_ref()
            .map(|s| s.borrow_mut().take_outbox())
            .unwrap_or_default();
        let sent = !outbox.is_empty();
        if let Some(client) = self.client.as_mut() {
            for (kind, arg) in outbox {
                client.send_query(&kind, &arg);
            }
        }
        sent
    }

    /// Rebuild the host from freshly pushed source, re-attaching a query provider
    /// over the same shared cache (so already-fetched data survives a re-push).
    fn reload_from_source(&mut self, source: &str) {
        match PanelHost::from_source(&self.client_name, source) {
            Ok(host) => self.install_reloaded_host(host),
            Err(err) => self.reload_error = Some(err),
        }
    }

    /// Rebuild the host from a real `.ptl` file (the seed entry's origin), via
    /// [`PanelHost::load`] rather than `from_source` — so the rebuilt host keeps
    /// its hot-reload signature and file path. Used when *back* returns to the
    /// origin screen. A read/compile failure leaves the current host in place with
    /// the error recorded, exactly like [`reload_from_source`](Self::reload_from_source).
    fn reload_from_path(&mut self, path: &Path) {
        match PanelHost::load(path) {
            Ok(host) => self.install_reloaded_host(host),
            Err(err) => self.reload_error = Some(err),
        }
    }

    /// Swap in a freshly built host, re-attaching the query provider and clearing
    /// the departed screen's per-host view state. Shared by the source- and
    /// path-backed rebuild paths.
    fn install_reloaded_host(&mut self, mut host: PanelHost) {
        adopt_font(&mut host);
        if let Some(shared) = self.client_shared.as_ref() {
            host.set_query_provider(Box::new(ProcessQueryProvider::new(shared.clone())));
        }
        self.host = host;
        self.reload_error = None;
        self.frame_error = None;
        self.text_views.clear();
        self.frame_count = 0;
        self.last_frame = None;
        self.observed_frame = -1;
        self.partial_observed = None;
    }

    /// The **live** screen currently displayed — the history entry at the cursor.
    /// After a `navigate(...)` this diverges from
    /// [`origin_script`](Self::origin_script); the titlebar and debug view report
    /// it so the on-screen screen is what shows.
    pub fn script(&self) -> &str {
        &self.history[self.cursor].screen
    }

    /// The **origin** screen: the layout-declared `.ptl` this pane was built with
    /// (history entry 0), stable across navigation. Pane reuse across a rebuild,
    /// layout persistence, and the navigation whitelist all key on this — so a
    /// navigated panel still round-trips to (and is reused as) its declared node,
    /// and a reload never resurrects a navigated screen as the declared one.
    pub fn origin_script(&self) -> &str {
        &self.history[0].screen
    }

    /// The resolved on-disk path of the **origin** screen (history entry 0), when
    /// it is file-backed. The app layer uses its parent directory as the navigation
    /// whitelist root — correct even when the layout declares a bare relative name
    /// like `panel("clock.ptl")`, whose own string has no directory to resolve
    /// siblings within. `None` for a source-only panel (no backing file).
    pub fn origin_path(&self) -> Option<&Path> {
        self.history[0].path.as_deref()
    }

    /// Whether the panel has navigated away from its origin screen (cursor past
    /// entry 0). The live editor→panel binding ([`App::sync_editor_panels`]) applies
    /// only at the origin, so a navigated panel is not driven by the origin's editor.
    pub fn is_navigated(&self) -> bool {
        self.cursor != 0
    }
}

// ── Browser-style history navigation ──────────────────────────────────────
// The swap mechanics behind the `navigate*` script API. These take
// *already-resolved* source (the app layer owns whitelist resolution) and
// preserve true browser semantics: state is scoped per history entry, so *back*
// restores the value a screen held when it was left. Currently exercised only by
// tests; the app layer resolves screens and drives these in the next chunk, so
// the block is `allow(dead_code)` until then.
#[allow(dead_code)]
impl PanelView {
    /// Number of entries in the history stack (>= 1; the seed is entry 0).
    pub(crate) fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Index of the currently displayed history entry.
    pub(crate) fn history_cursor(&self) -> usize {
        self.cursor
    }

    /// The screen identity of the current history entry (its `.ptl` name/path).
    pub(crate) fn current_screen(&self) -> &str {
        &self.history[self.cursor].screen
    }

    /// The current entry's screen and navigation argument, when re-issuing its
    /// `navigate` mutation is meaningful — i.e. a *navigated* entry. `None` at
    /// the seed (entry 0): nothing navigated to it, and its screen name is the
    /// pane's own origin rather than one the client declared, so replaying it
    /// would only ever draw a rejection.
    pub(crate) fn restored_entry(&self) -> Option<(String, serde_json::Value)> {
        if self.cursor == 0 {
            return None;
        }
        let entry = &self.history[self.cursor];
        Some((entry.screen.clone(), entry.nav_arg.clone()))
    }

    /// Swap the current entry's cached source for a freshly fetched one and
    /// redisplay it, keeping the entry's saved `state` and its navigation
    /// argument (both live on the entry, and [`load_entry`](Self::load_entry)
    /// re-applies them).
    ///
    /// Returns whether anything was rebuilt. An identical source is left alone:
    /// the common case of a *back* that re-runs the client's `navigate` handler
    /// purely for its side effects then costs no recompile and no flicker.
    pub(crate) fn refresh_current_source(&mut self, source: String) -> bool {
        // The seed rebuilds from its `path`, not a source; replacing it would
        // strip the origin's file identity and hot-reload.
        if self.cursor == 0 {
            return false;
        }
        if self.history[self.cursor].source.as_deref() == Some(source.as_str()) {
            return false;
        }
        self.history[self.cursor].source = Some(source);
        self.load_entry();
        true
    }

    /// Drain the navigation intents accumulated by [`tick`](Self::tick), for the
    /// app layer to resolve and act on. Empty after a drain until the next frame.
    pub(crate) fn take_nav_events(&mut self) -> Vec<ClientEvent> {
        std::mem::take(&mut self.nav_events)
    }

    /// Mark the panel's origin (seed) screen as **source-backed** — for a
    /// subprocess (pushed-script) pane, whose home screen has no on-disk file, so
    /// returning *back* to origin rebuilds from the client's initial pushed source
    /// instead of the synthetic `gpp:<cmd>` path the seed was created with. Called
    /// once when a script-client pane is built.
    pub(crate) fn set_origin_source(&mut self, source: String) {
        self.history[0].path = None;
        self.history[0].source = Some(source);
    }

    /// Fetch a navigation target screen's source from the attached subprocess
    /// client via the built-in `navigate` **mutation**, blocking up to `wait` for
    /// the answer. This is how a subprocess panel navigates: the client owns the
    /// screen sources, the host owns the history stack, and this round-trip bridges
    /// them. Non-mutate envelopes drained while waiting (renders, `queryResult`s)
    /// are applied normally so nothing is lost. Returns the target's `.ptl` source,
    /// or an error string (no client, timeout, undeclared screen, malformed reply).
    pub(crate) fn client_fetch_screen(
        &mut self,
        screen: &str,
        nav_arg: &serde_json::Value,
        wait: Duration,
    ) -> Result<String, String> {
        // `arg` is the subject `navigate(screen, arg)` carried. A client that
        // registered its own `navigate` handler needs it to prime data for the
        // target screen; the built-in handler ignores it. Absent for the
        // one-argument form, so an app that never passes one sees the shape it
        // always did.
        let mut arg = serde_json::json!({ "screen": screen });
        if !nav_arg.is_null() {
            arg["arg"] = nav_arg.clone();
        }
        let id = match self.client.as_mut() {
            Some(client) => client.send_mutate(petal_query::gpp::NAVIGATE, arg),
            None => return Err("panel has no client to navigate".to_string()),
        };
        let deadline = Instant::now() + wait;
        let mut leftover = Vec::new();
        let mut result: Option<Result<String, String>> = None;
        while result.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let batch = match self.client.as_ref() {
                Some(client) => client.drain_for(remaining),
                None => break,
            };
            if batch.is_empty() {
                break; // nothing arrived before the deadline
            }
            for env in batch {
                // A `mutateResult` is a response (id + result, no method) whose
                // result carries `name` — distinct from a `queryResult` (kind/arg).
                if result.is_none() && env.id == Some(id) && env.method.is_none() {
                    if let Ok(m) = env.result_as::<gpp::MutateResult>() {
                        result = Some(mutate_source(m));
                        continue;
                    }
                }
                leftover.push(env);
            }
        }
        // Fold whatever else we drained back through the normal path.
        let _ = self.apply_client_envelopes(leftover);
        result.unwrap_or_else(|| Err(format!("navigate: no response for screen '{screen}'")))
    }

    /// Relay a script `mutate(name, arg)` to the pane's subprocess as a GPP
    /// `mutate` request and block up to `wait` for its `mutateResult` — the
    /// general form of [`client_fetch_screen`](Self::client_fetch_screen) (which
    /// is just the built-in `navigate` mutation). Returns the reply's value on
    /// success (an app-registered `on_mutation` handler's `Reply`, JSON-stringified
    /// for a status line), or an `Err` on a handler error / no client / timeout.
    /// Non-`mutateResult` envelopes drained while waiting are folded back through
    /// the normal path so nothing is lost.
    /// Report what a mutation resolved to back to the script, under the handle
    /// `mutate(...)` handed it. The next frame's `mutate_result(handle)` answers
    /// with it.
    ///
    /// Every outcome is reported, including the ones the host answered itself and
    /// the ones that failed: a drawer that asked for a save is entitled to know it
    /// did not happen, and a test asserting on the panel's own values is entitled
    /// to read that.
    pub(crate) fn resolve_mutation(&mut self, handle: i64, result: Result<Option<String>, String>) {
        self.host
            .set_mutation_result(handle, garden_script::mutation_reply(result));
    }

    pub(crate) fn client_mutate(
        &mut self,
        name: &str,
        arg: serde_json::Value,
        wait: Duration,
    ) -> Result<Option<String>, String> {
        let id = match self.client.as_mut() {
            Some(client) => client.send_mutate(name, arg),
            None => return Err(format!("panel has no subprocess to handle '{name}'")),
        };
        let deadline = Instant::now() + wait;
        let mut leftover = Vec::new();
        let mut result: Option<Result<Option<String>, String>> = None;
        while result.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let batch = match self.client.as_ref() {
                Some(client) => client.drain_for(remaining),
                None => break,
            };
            if batch.is_empty() {
                break;
            }
            for env in batch {
                if result.is_none() && env.id == Some(id) && env.method.is_none() {
                    if let Ok(m) = env.result_as::<gpp::MutateResult>() {
                        result = Some(match m.error {
                            Some(err) => Err(err),
                            None => Ok(m.value.map(json_to_status)),
                        });
                        continue;
                    }
                }
                leftover.push(env);
            }
        }
        let _ = self.apply_client_envelopes(leftover);
        result.unwrap_or_else(|| Err(format!("mutate '{name}': no response")))
    }

    /// Navigate to `screen` (a browser link click): snapshot the current entry's
    /// `state`, drop any forward entries, push a new entry for `screen`/`source`,
    /// advance the cursor, and load it. `source` is the already-resolved script.
    pub(crate) fn nav_push(&mut self, screen: String, source: String, arg: serde_json::Value) {
        self.history[self.cursor].saved_state = Some(self.host.state_json());
        self.history.truncate(self.cursor + 1);
        self.history.push(HistoryEntry {
            screen,
            path: None,
            source: Some(source),
            saved_state: None,
            nav_arg: arg,
        });
        self.cursor = self.history.len() - 1;
        self.load_entry();
    }

    /// Replace the current screen in place (browser `location.replace`): the
    /// current entry is discarded rather than kept for *back*, so history length
    /// is unchanged. Loads `screen`/`source` fresh (no state carried over).
    ///
    /// v1 limitation: replacing at the origin (cursor 0) overwrites the seed's
    /// screen identity and drops its file `path`, so the panel's [`origin_script`]
    /// no longer matches its layout-declared node. Replace is meant for a
    /// navigated screen redirecting onward, not for redirecting the origin itself.
    pub(crate) fn nav_replace(&mut self, screen: String, source: String, arg: serde_json::Value) {
        self.history[self.cursor] = HistoryEntry {
            screen,
            path: None,
            source: Some(source),
            saved_state: None,
            nav_arg: arg,
        };
        self.load_entry();
    }

    /// Move the history cursor back one entry (browser *back*), restoring that
    /// visit's state. Snapshots the current entry's state before moving so it can
    /// be restored on a later *forward*. Returns false (no-op) at the start.
    pub(crate) fn nav_back(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.history[self.cursor].saved_state = Some(self.host.state_json());
        self.cursor -= 1;
        self.load_entry();
        true
    }

    /// Move the history cursor forward one entry (browser *forward*). Snapshots
    /// the current entry's state before moving. Returns false (no-op) at the end.
    pub(crate) fn nav_forward(&mut self) -> bool {
        if self.cursor + 1 >= self.history.len() {
            return false;
        }
        self.history[self.cursor].saved_state = Some(self.host.state_json());
        self.cursor += 1;
        self.load_entry();
        true
    }

    /// Display the entry at [`cursor`](Self::cursor). Order is load-bearing:
    /// rebuild the host FIRST, then re-seed its saved `state` BEFORE the next frame
    /// runs — so the frame's `state x = …` init block is skipped
    /// ([`Inst::StateInit`] skips a populated slot) and the restored value
    /// survives. Running a frame before the restore would clobber it.
    ///
    /// A file-backed entry (the seed) rebuilds from its `path` via
    /// [`PanelHost::load`], keeping hot-reload and the real path; a navigated entry
    /// rebuilds from its `source`. Rebuilding is unconditional so returning *back*
    /// to a screen restores its program, not just its state.
    fn load_entry(&mut self) {
        let entry = &self.history[self.cursor];
        // The entry's navigation argument, republished before the rebuilt host
        // runs its first frame — so a screen restored by *back* draws its own
        // subject immediately instead of a frame of nothing.
        let nav_arg = entry.nav_arg.clone();
        if let Some(path) = entry.path.clone() {
            self.reload_from_path(&path);
        } else if let Some(source) = entry.source.clone() {
            self.reload_from_source(&source);
        }
        self.host.set_nav_arg(nav_arg);
        if let Some(state) = self.history[self.cursor].saved_state.clone() {
            self.host.restore_state(&state);
        }
        // A new screen has its own regions/focus; drop the departed one's.
        self.text_views.clear();
        self.focused_region = None;
        // Wake the panel so the next tick renders the loaded screen. (Frame
        // bookkeeping — frame_count/last_frame — is reset by reload_from_source
        // whenever a source was present.)
        self.note_activity(Instant::now());
    }
}

impl PanelView {
    /// Re-point the panel at a new script path — used when a Petal-IDE "save as"
    /// renames the scratch to a user-chosen file, so the editor→panel live
    /// binding (matched by path) keeps following the same buffer. Rewrites the
    /// **origin** (history entry 0), since it is the layout-declared identity the
    /// binding and persistence key on; a save-as target is never a navigated
    /// panel, so the current screen (cursor 0) follows.
    pub fn set_script(&mut self, script: String) {
        self.history[0].screen = script;
    }

    pub fn frame_count(&self) -> i64 {
        self.frame_count
    }

    /// Whether the panel is currently animating (within its wake window).
    pub fn is_awake(&self, now: Instant) -> bool {
        now.duration_since(self.last_activity) < panel_wake()
    }

    /// Push the activity stamp far enough into the past that the panel is
    /// asleep — for tests of the paths that must run regardless (`/tick`).
    #[cfg(test)]
    pub(crate) fn sleep_for_test(&mut self) {
        self.last_activity = Instant::now() - PANEL_WAKE - Duration::from_secs(1);
    }

    /// Stamp activity, restarting the wake window.
    pub fn note_activity(&mut self, now: Instant) {
        self.last_activity = now;
    }

    /// Update the pane-local mouse position (a level). Also drives the standard
    /// contract's drag detection while a button is held.
    pub fn set_mouse(&mut self, x: i32, y: i32) {
        self.host.input_event(InputEvent::MouseMove { x, y });
    }

    /// A mouse button went down (0 = left). The host derives the click chain and
    /// drag-start anchor from the paired down/up events, so feed the current
    /// position with [`set_mouse`](Self::set_mouse) first.
    pub fn mouse_down(&mut self, button: u8) {
        self.host.input_event(InputEvent::MouseDown { button });
    }

    /// A mouse button was released (0 = left) — a `mouse_released` edge; ends a drag.
    pub fn mouse_up(&mut self, button: u8) {
        self.host.input_event(InputEvent::MouseUp { button });
    }

    /// A left press that is the `clicks`-th of a chain — what `click_count()`
    /// reports. A real pointer builds the chain out of repeated presses, and so
    /// does this: a host (or `POST /mouse {"clicks": 2}`) that already knows the
    /// count feeds that many presses at the current position, which is the only
    /// thing the standard input contract derives a chain from. Without it a
    /// panel's `click_count()` was permanently 1 and no double-click gesture was
    /// reachable, in a real window or headless.
    pub fn mouse_down_clicks(&mut self, button: u8, clicks: u32) {
        // Only the left button carries a click chain; anything else is one press.
        let repeats = if button == 0 { clicks.max(1) } else { 1 };
        for _ in 0..repeats {
            self.host.input_event(InputEvent::MouseDown { button });
        }
    }

    /// Forward a key (canonical name, e.g. `"j"`, `"down"`, `"space"`) plus its
    /// typed text if any. Garden's frontends don't deliver key-up, so a key is
    /// fed as a paired down+up: scripts see the `key_pressed`/`key_released`
    /// edges but `key_down` stays a within-frame pulse (no phantom held keys).
    /// A printable character is also fed as `text_input`.
    pub fn key(&mut self, name: String, text: Option<String>) {
        if let Some(text) = text {
            self.host.input_event(InputEvent::Text { text });
        }
        self.host
            .input_event(InputEvent::KeyDown { key: name.clone() });
        self.host.input_event(InputEvent::KeyUp { key: name });
    }

    /// Press and **hold** a key: the `key_pressed` edge fires now and the key
    /// stays in `key_down(...)` until [`key_up`](Self::key_up). No frontend
    /// delivers key-up, so this comes from the debug server's
    /// `POST /key {"op":"down"}` — the only way a hold-to-do-X interaction is
    /// drivable headless.
    pub fn key_down(&mut self, name: String, text: Option<String>) {
        if let Some(text) = text {
            self.host.input_event(InputEvent::Text { text });
        }
        self.host.input_event(InputEvent::KeyDown { key: name });
    }

    /// Release a key held by [`key_down`](Self::key_down) — the `key_released`
    /// edge, and the end of `key_down(...)`.
    pub fn key_up(&mut self, name: String) {
        self.host.input_event(InputEvent::KeyUp { key: name });
    }

    /// Whether the script claimed `name` under the modifier bits `mods`
    /// (`1=shift 2=ctrl 4=alt 8=cmd`) — i.e. the host must forward this chord
    /// instead of applying its own shortcut. See [`key_claims`](Self::key_claims).
    pub fn claims_key(&self, name: &str, mods: u8) -> bool {
        self.key_claims.iter().any(|(claim, claim_mods)| {
            claim.eq_ignore_ascii_case(name) && claim_mods.is_none_or(|m| m == mods)
        })
    }

    /// The chords the script currently claims, for tests and introspection.
    pub fn key_claims(&self) -> &[(String, Option<u8>)] {
        &self.key_claims
    }

    /// Feed typed text (IME/paste) as `text_input`, with no accompanying key
    /// edge — for the debug server's `/text` and future IME composition.
    pub fn text(&mut self, text: String) {
        self.host.input_event(InputEvent::Text { text });
    }

    /// Inject the host UI theme read by the script's `panel_theme()` native, so
    /// a drawer paints in the app's colors. Read-only per-frame input: the host
    /// calls this each tick (before [`tick`](Self::tick)) with the current
    /// [`Theme`]'s projection, so a live `POST /theme` is reflected next frame.
    pub fn set_theme(&mut self, theme: garden_script::PanelTheme) {
        self.host.set_theme(theme);
    }

    /// Attach the host-side data source the script reaches through
    /// `host_data(kind, arg)`. The Petal-IDE IR inspector uses this to pull its
    /// rendered stages from the app's shared inspector cache (see
    /// [`crate::petal_ide`]).
    pub fn set_data_provider(&mut self, provider: garden_script::DataProvider) {
        self.host.set_data_provider(provider);
    }

    /// Set the held modifier chord read by
    /// `mod_shift()`/`mod_ctrl()`/`mod_alt()`/`mod_cmd()`.
    ///
    /// The same change is also published as ordinary held keys named `"shift"`,
    /// `"ctrl"`, `"alt"` and `"cmd"`, so `key_down("shift")` answers truthfully.
    /// It is not how modifiers are *meant* to be read (`mod_shift()` is), but it
    /// used to return false forever — a keybinding written that way simply did
    /// nothing, with no error to notice.
    pub fn set_modifiers(&mut self, mods: Mods) {
        self.host.input_event(InputEvent::Modifiers(Modifiers {
            shift: mods.shift,
            ctrl: mods.ctrl,
            alt: mods.alt,
            cmd: mods.cmd,
        }));
        let was = self.mods;
        self.mods = mods;
        for (name, now, before) in [
            ("shift", mods.shift, was.shift),
            ("ctrl", mods.ctrl, was.ctrl),
            ("alt", mods.alt, was.alt),
            ("cmd", mods.cmd, was.cmd),
        ] {
            if now == before {
                continue;
            }
            let key = name.to_string();
            self.host.input_event(if now {
                InputEvent::KeyDown { key }
            } else {
                InputEvent::KeyUp { key }
            });
        }
    }

    /// Accumulate wheel movement in whole lines/columns (positive = down/right),
    /// read as `scroll_x()` / `scroll_y()` next frame.
    pub fn scroll(&mut self, dx: i32, dy: i32) {
        self.host.input_event(InputEvent::Scroll {
            dx: dx as f64,
            dy: dy as f64,
        });
    }

    /// Every named binding of the last successful frame (name → value), for
    /// introspection. Keys are function-qualified — a `let sel` inside
    /// `fn list_row` reads as `list_row.sel` — and a binding that didn't execute
    /// is absent; see [`PanelHost::observed_json`] for the full rule.
    pub fn observed(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.observed
    }

    /// The panel frame [`observed`](Self::observed) was captured on, or `None`
    /// before the first successful frame. Compare against
    /// [`frame_count`](Self::frame_count) to tell current values from stale ones.
    pub fn observed_frame(&self) -> Option<i64> {
        (self.observed_frame >= 0).then_some(self.observed_frame)
    }

    /// The partial observations of the last **failed** frame (frame number and
    /// the bindings it reached before raising), or `None` when the last frame
    /// succeeded. See the [`partial_observed`](Self::partial_observed) field.
    pub fn partial_observed(&self) -> Option<(i64, &serde_json::Map<String, serde_json::Value>)> {
        self.partial_observed.as_ref().map(|(f, m)| (*f, m))
    }

    /// The script's live `state` variables as a JSON map keyed by name — the
    /// data the Petal-IDE state inspector renders.
    pub fn state_json(&self) -> serde_json::Map<String, serde_json::Value> {
        self.host.state_json()
    }

    /// The current script error, if any — the text shown in the pane's error
    /// banner. A **reload** error (the buffer/file doesn't compile) takes
    /// precedence over a runtime **frame** error, since it means the source on
    /// screen is the thing to fix. `None` when the panel is healthy.
    pub fn error(&self) -> Option<&str> {
        self.reload_error.as_deref().or(self.frame_error.as_deref())
    }

    /// Whether the program currently rendering is **older than the source it was
    /// loaded from** — i.e. the last reload failed to compile, so the canvas is
    /// showing the last good frame while the buffer has moved on.
    ///
    /// Anything that maps a rendered pixel back onto the *current* text has to
    /// check this: the running program's spans describe the text that compiled,
    /// so once the buffer drifts they point at whatever now occupies those lines.
    /// A [`frame_error`](Self::frame_error) is deliberately not drift — the
    /// running program is still the source on screen.
    pub fn source_drifted(&self) -> bool {
        self.reload_error.is_some()
    }

    /// Record the compile error of a panel whose script did not load at all, so
    /// the pane reports it (banner, `status_error`, `/state`) while running the
    /// empty [`PanelHost::stub`] program. Cleared like any other reload error —
    /// by the next [`poll_reload`](Self::poll_reload) that compiles — which is
    /// what lets a panel that was broken at startup recover on a fixed save.
    pub fn set_load_error(&mut self, err: impl Into<String>) {
        self.reload_error = Some(err.into());
    }

    /// The exact input delivered to the last frame (for debug-server visibility).
    pub fn input_snapshot(&self) -> &PanelInput {
        self.host.input_snapshot()
    }

    /// Run one frame if awake, caching the resulting commands. Returns whether a
    /// frame ran (i.e. the cached commands changed and a redraw is wanted).
    pub fn tick(&mut self, now: Instant, rect: Rect, cell: (f32, f32)) -> bool {
        if !self.is_awake(now) {
            return false;
        }
        let dt = match self.last_frame {
            Some(prev) => now.duration_since(prev).as_secs_f64().min(MAX_DT),
            None => 0.0,
        };
        self.last_frame = Some(now);
        self.run_frame(dt, rect, cell);
        true
    }

    /// Run one frame with an explicit `dt`, ignoring the sleep/wake window —
    /// the debug server's `POST /tick`. A headless harness driving a game or an
    /// animation otherwise has to fake input to get a frame at all (and then
    /// gets a wall-clock `dt` it did not choose); this advances panel time
    /// deterministically instead. Activity is stamped so the panel also stays
    /// awake for the ordinary tick path afterwards.
    pub fn tick_with_dt(&mut self, now: Instant, dt: f64, rect: Rect, cell: (f32, f32)) {
        self.note_activity(now);
        self.last_frame = Some(now);
        self.run_frame(dt.clamp(0.0, MAX_DT), rect, cell);
    }

    /// The body of one panel frame, shared by the wall-clock [`tick`](Self::tick)
    /// and the deterministic [`tick_with_dt`](Self::tick_with_dt).
    fn run_frame(&mut self, dt: f64, rect: Rect, cell: (f32, f32)) {
        // The host owns the standard input contract: events fed since the last
        // frame (mouse, keys, scroll, modifiers, text) are promoted to this
        // frame's edge/level snapshot by `frame`'s `begin_frame`, so there is no
        // per-frame edge bookkeeping to do here.
        self.host
            .set_dimensions(rect.w.max(0.0) as i32, rect.h.max(0.0) as i32);
        // Publish each editable region's live buffer text so this frame's
        // `edit_view_text(id)` reads the user's edits (handled host-side between
        // frames) back into the script.
        let ev_texts = self.edit_view_texts();
        self.host.set_edit_view_texts(ev_texts);
        // …and what those edits currently fold back to, for `edit_view_edits(id)`.
        let ev_edits = self.edit_view_edits();
        self.host.set_edit_view_edits(ev_edits);
        match self.host.frame(dt, self.frame_count) {
            Ok(cmds) => {
                self.last_tick_changed = cmds != self.cmds || self.frame_error.is_some();
                self.cmds = cmds;
                self.observed = self.host.observed_json();
                self.observed_frame = self.frame_count;
                self.partial_observed = None;
                // Forward the frame's `emit(event, arg)` events to the pane's
                // GPP subprocess as `emit` notifications — fire-and-forget, in
                // call order. An in-process panel (no client) drops them.
                let emitted = self.host.take_emitted();
                if let Some(client) = self.client.as_mut() {
                    for (event, arg) in emitted {
                        client.send_emit(&event, arg);
                    }
                }
                // Navigation intents flow out over the `ClientEvent` path (not the
                // subprocess `emit` pipe): an in-process panel has no client, but
                // the app layer must still act on `navigate(...)` to resolve the
                // target screen and drive this pane's history swap methods.
                for intent in self.host.take_nav() {
                    self.nav_events.push(ClientEvent::Navigate(intent));
                }
                // Mutation requests (`mutate(name, arg)`) ride the same
                // app-drained channel: the app layer relays each to the
                // subprocess and surfaces the reply as status.
                for (name, arg, handle) in self.host.take_mutations() {
                    self.nav_events
                        .push(ClientEvent::Mutate { name, arg, handle });
                }
                // The chords this frame asked the host to hand over. Declarative:
                // whatever the frame claimed *is* the claim set from now on.
                self.key_claims = self.host.take_key_claims();
                // A successful frame clears only the *runtime* error; a reload
                // error (broken buffer) persists — the old program is what just
                // ran, and its health says nothing about the new source.
                self.frame_error = None;
                self.sync_text_views(cell);
            }
            Err(err) => {
                self.last_tick_changed = self.frame_error.as_deref() != Some(err.as_str());
                self.frame_error = Some(err);
                // How far the broken frame got, kept beside (never on top of)
                // the last good values — an absent key otherwise reads as "that
                // branch never ran".
                self.partial_observed = Some((self.frame_count, self.host.observed_json()));
            }
        }

        // Collect this frame's `print(...)` lines whether it succeeded or raised
        // — a print before the failing line is exactly what someone debugging a
        // broken frame needs to see.
        self.collect_output();
        self.frame_count += 1;
    }

    /// Move the host's buffered `print(...)` lines into this pane's capped ring.
    fn collect_output(&mut self) {
        for line in self.host.take_output() {
            if self.output.len() == OUTPUT_CAP {
                self.output.pop_front();
            }
            self.output.push_back(line);
        }
    }

    /// Take the script `print(...)` lines collected since the last call (oldest
    /// first). Drained, so each line is reported once — the debug server's
    /// `/state` is the consumer.
    pub fn take_output(&mut self) -> Vec<String> {
        self.output.drain(..).collect()
    }

    /// Whether the most recent [`tick`](Self::tick) frame actually changed the
    /// drawn output (draw commands or error text). Distinct from `tick`'s
    /// return value, which reports that a frame *ran* (an awake panel reruns
    /// its script every tick even when it draws the same thing).
    pub fn last_tick_changed(&self) -> bool {
        self.last_tick_changed
    }

    // ── Direct manipulation: canvas → source ──────────────────────────────

    /// Trace this pane's drawn shapes back to the code that drew them, so
    /// pointing at the canvas can highlight the source. Petal IDE turns this on
    /// for the canvas it pairs with an editor; every other panel leaves it off
    /// and pays nothing. Idempotent.
    pub fn set_trace_origins(&mut self, on: bool) {
        if self.trace_origins == on {
            return;
        }
        self.trace_origins = on;
        self.host.set_trace_origins(on);
    }

    /// Whether this pane is tracing (see [`set_trace_origins`](Self::set_trace_origins)).
    pub fn traces_origins(&self) -> bool {
        self.trace_origins
    }

    /// The source of the shape drawn at window point (`x`, `y`) within
    /// `pane_rect`: the span of the `draw_*` call that painted it, plus where
    /// each of its arguments came from.
    ///
    /// `None` when the point is bare canvas, when tracing is off, or when the
    /// origin no longer resolves — the last of which is what a live reload
    /// leaves behind between the recompile and the next frame, and is a miss
    /// rather than a wrong answer on purpose.
    pub fn trace_at(&self, pane_rect: Rect, x: f32, y: f32) -> Option<DrawTrace> {
        if !self.trace_origins {
            return None;
        }
        // Draw commands are panel-local; the pointer is in window coordinates.
        let px = (x - pane_rect.x) as i32;
        let py = (y - pane_rect.y) as i32;
        let index = garden_script::hit_test(&self.cmds, px, py)?;
        let origin = self.host.origin_at(index)?;
        self.host.trace_origin(origin)
    }

    /// The shape under (`x`, `y`) as something a pointer can **drag**: its index
    /// in this frame's command list, plus which arguments of the call that drew
    /// it move with the pointer and what they are worth now
    /// ([`DragHandle`](garden_script::DragHandle)).
    ///
    /// The index is the durable half. Term ids go stale the moment a drag's own
    /// edit recompiles the sketch, but the command index does not: the frame is
    /// rebuilt with the same shapes in the same order, so the grabbed shape is
    /// still command *n*. A drag holds the index and re-resolves the call each
    /// move ([`drag_handle_at`](Self::drag_handle_at)).
    pub fn drag_target_at(
        &self,
        pane_rect: Rect,
        x: f32,
        y: f32,
    ) -> Option<(usize, garden_script::DragHandle)> {
        if !self.trace_origins {
            return None;
        }
        let px = (x - pane_rect.x) as i32;
        let py = (y - pane_rect.y) as i32;
        let index = garden_script::hit_test(&self.cmds, px, py)?;
        let handle = self.drag_handle_at(index)?;
        Some((index, handle))
    }

    /// Re-resolve the command at `index` for a drag in progress: the call it
    /// belongs to *in the program running right now*, and its handle.
    ///
    /// `None` once the sketch stops compiling or the shape stops existing —
    /// which ends the drag rather than writing an edit derived from a frame that
    /// no longer describes the buffer.
    pub fn drag_handle_at(&self, index: usize) -> Option<garden_script::DragHandle> {
        let cmd = self.cmds.get(index)?;
        let trace = self.host.trace_origin(self.host.origin_at(index)?)?;
        garden_script::drag_handle(cmd, trace.callee.as_deref()?, trace.args.len())
    }

    /// State a drag's goals against a traced call and get the source edits that
    /// satisfy them — see [`PanelHost::propose_drag_edits`](garden_script::PanelHost::propose_drag_edits).
    pub fn propose_drag_edits(
        &self,
        cmd_index: usize,
        goals: &[(usize, f64)],
    ) -> garden_script::DragOutcome {
        self.host.propose_drag_edits(cmd_index, goals)
    }

    /// Reconcile the embedded editors with the `text_view(...)` regions (and
    /// their `text_view_line_styles(...)`) the last frame declared. For each
    /// declared region: refresh its rect, and rebuild its buffer only when the
    /// text actually changed (content-hash gate) so an unchanged diff re-declared
    /// every frame preserves selection and scroll. Styling is (re)applied only
    /// when it changes or the content was rebuilt (which clears it). Regions not
    /// declared this frame are dropped (their editor is freed).
    fn sync_text_views(&mut self, cell: (f32, f32)) {
        let mut seen: Vec<i64> = Vec::new();
        // Regions whose buffer was (re)built this frame — `set_external_content`
        // clears styling, so their styles must be reapplied even if unchanged.
        let mut rebuilt: Vec<i64> = Vec::new();
        // Which regions asked to soft-wrap this frame. Read up front because a
        // `text_view_wrap` may be declared before or after its region's
        // `text_view`, and the wrap flag has to be applied in the same pass that
        // (re)builds the buffer — `set_external_content` resets it to off.
        let wrap_flags: Vec<(i64, bool)> = self
            .cmds
            .iter()
            .filter_map(|cmd| match cmd {
                PanelCmd::TextViewWrap { id, wrap } => Some((*id, *wrap)),
                _ => None,
            })
            .collect();
        // Last declaration for a region wins.
        let wants_wrap = |id: i64| {
            wrap_flags
                .iter()
                .rfind(|(wid, _)| *wid == id)
                .is_some_and(|(_, wrap)| *wrap)
        };
        for cmd in &self.cmds {
            let PanelCmd::TextView {
                id,
                x,
                y,
                w,
                h,
                text,
                editable,
            } = cmd
            else {
                continue;
            };
            seen.push(*id);
            let rect = Rect {
                x: *x as f32,
                y: *y as f32,
                w: *w as f32,
                h: *h as f32,
            };
            let hash = hash_text(text);
            match self.text_views.get_mut(id) {
                Some(entry) => {
                    // The content-hash gate is what makes an editable region work:
                    // a drawer re-declares the same seed `text` every frame, so
                    // the hash is unchanged and the buffer (with the user's edits)
                    // is left alone. Only a genuine change to the declared text
                    // (new file/view) rebuilds — discarding edits, as intended.
                    if entry.content_hash != hash {
                        entry.view.set_external_content(text, None);
                        entry.content_hash = hash;
                        rebuilt.push(*id);
                    }
                    entry.rect = rect;
                    entry.editable = *editable;
                    set_region_wrap(&mut entry.view, rect, cell, wants_wrap(*id));
                }
                None => {
                    let mut view = EditorView::open(None);
                    view.set_external_content(text, None);
                    set_region_wrap(&mut view, rect, cell, wants_wrap(*id));
                    self.text_views.insert(
                        *id,
                        EmbeddedText {
                            view,
                            content_hash: hash,
                            styles_hash: 0,
                            rect,
                            editable: *editable,
                            projection_hash: 0,
                            edits_cache: None,
                        },
                    );
                    rebuilt.push(*id);
                }
            }
        }
        // Second pass: apply per-line styling. Runs after all content is set (a
        // region's `text_view_line_styles` follows its `text_view` in the same
        // frame), and only when the styling or the content changed.
        for cmd in &self.cmds {
            let PanelCmd::TextViewStyles { id, styles } = cmd else {
                continue;
            };
            let Some(entry) = self.text_views.get_mut(id) else {
                continue;
            };
            let hash = hash_styles(styles);
            if rebuilt.contains(id) || entry.styles_hash != hash {
                entry.view.set_external_line_styles(styles);
                entry.styles_hash = hash;
            }
        }
        // Third pass: projections. After the styles pass, because a projected
        // region's styles come from its origin table — they follow their lines
        // through insertions, which a positional style list cannot do — and so
        // must win over any `text_view_line_styles` the drawer also declared.
        for cmd in &self.cmds {
            let PanelCmd::TextViewProjection { id, spec } = cmd else {
                continue;
            };
            let Some(entry) = self.text_views.get_mut(id) else {
                continue;
            };
            let hash = hash_projection(spec);
            // A rebuilt buffer dropped its projection with the old content, so
            // re-attach even when the spec itself is unchanged.
            if !rebuilt.contains(id) && entry.projection_hash == hash {
                continue;
            }
            entry.projection_hash = hash;
            entry.edits_cache = None;
            let seed = entry.view.buffer.text();
            let mut proj = projection_from_spec(spec, &seed);
            // Record the sources' state as this projection was built from them,
            // so `resolve` reports only what the user then changes. This runs
            // exactly where a projection is *created* — a spec re-declared
            // unchanged never reaches here, so a live edit is never mistaken for
            // the original content (which would make `^S` blind again).
            proj.capture_baseline(&buffer_lines(&entry.view));
            let styles = proj.line_styles();
            entry.view.projection = Some(proj);
            entry.view.set_external_line_styles(&styles);
        }

        // Fourth pass: programmatic scrolls (`text_view_scroll_to`). These are
        // actions, not frame state, so each is applied once and then *removed*
        // from `cmds` — an idle panel re-syncs the same command list every tick,
        // and a surviving scroll command would pin the region there forever.
        let mut jumps: Vec<(i64, i64)> = Vec::new();
        self.cmds.retain(|cmd| match cmd {
            PanelCmd::TextViewScrollTo { id, line } => {
                jumps.push((*id, *line));
                false
            }
            _ => true,
        });
        for (id, line) in jumps {
            let Some(entry) = self.text_views.get_mut(&id) else {
                continue;
            };
            // The region's own rect is panel-local, and the wrap width only needs
            // its size, so the absolute origin is irrelevant here.
            let cols = entry.view.visible_cols(entry.rect, cell.0);
            entry.view.scroll_to_line(line.max(0) as usize, cols);
        }

        self.text_views.retain(|id, _| seen.contains(id));
        if self.focused_region.is_some_and(|id| !seen.contains(&id)) {
            self.focused_region = None;
        }
    }

    // ── Embedded text-view regions ────────────────────────────────────────
    // Selectable read-only text areas declared by the script via `text_view`.
    // The host (`garden-app`'s pointer/key routing) drives selection, scroll,
    // and clipboard copy on these directly; the panel translates between the
    // pane's absolute coordinates and each region's panel-local rect.

    /// Which `text_view` region currently owns keyboard focus (for Cmd-C), or
    /// `None` when keys go to the script as usual.
    pub fn focused_region(&self) -> Option<i64> {
        self.focused_region
    }

    /// Hand keyboard focus back to the script (e.g. Escape, or a press outside
    /// every region).
    pub fn clear_focused_region(&mut self) {
        self.focused_region = None;
    }

    /// The region id whose absolute rect contains `(x, y)`, if any. `pane_rect`
    /// is the panel's absolute pane rect.
    pub fn text_view_at(&self, pane_rect: Rect, x: f32, y: f32) -> Option<i64> {
        self.text_views.iter().find_map(|(id, e)| {
            let r = e.abs_rect(pane_rect);
            (x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h).then_some(*id)
        })
    }

    /// A press inside region `id`: focus it and begin a selection drag. `extend`
    /// is Shift (grow the current selection); `clicks` gives word/line
    /// granularity (2 = word, 3+ = line), matching the editor's own behavior.
    pub fn region_press(
        &mut self,
        id: i64,
        pane_rect: Rect,
        cell: (f32, f32),
        x: f32,
        y: f32,
        extend: bool,
        clicks: u32,
    ) {
        self.focused_region = Some(id);
        if let Some(e) = self.text_views.get_mut(&id) {
            let r = e.abs_rect(pane_rect);
            let p = e.view.position_for_click(r, cell, x, y);
            e.view.begin_drag_with_clicks(p, extend, clicks);
        }
    }

    /// Extend the active selection drag in region `id` to `(x, y)`,
    /// auto-scrolling the region to keep the drag point visible.
    pub fn region_drag_to(&mut self, id: i64, pane_rect: Rect, cell: (f32, f32), x: f32, y: f32) {
        if let Some(e) = self.text_views.get_mut(&id) {
            let r = e.abs_rect(pane_rect);
            let p = e.view.position_for_click(r, cell, x, y);
            e.view.drag_to(p);
            let vis = EditorView::visible_lines(r, cell.1);
            let cols = e.view.visible_cols(r, cell.0);
            e.view.ensure_cursor_visible(vis, cols);
        }
    }

    /// End an active selection drag in region `id`.
    pub fn region_end_drag(&mut self, id: i64) {
        if let Some(e) = self.text_views.get_mut(&id) {
            e.view.end_drag();
        }
    }

    /// Select the whole contents of region `id`.
    pub fn region_select_all(&mut self, id: i64) {
        if let Some(e) = self.text_views.get_mut(&id) {
            e.view.select_all();
        }
    }

    /// The read-only editor backing region `id` (e.g. to read its selection for
    /// a clipboard copy).
    pub fn region_view(&self, id: i64) -> Option<&EditorView> {
        self.text_views.get(&id).map(|e| &e.view)
    }

    /// Whether region `id` is an editable `edit_view` (host routes vim keystrokes
    /// into it) rather than a read-only `text_view`. `None` if there is no such
    /// region.
    pub fn region_editable(&self, id: i64) -> Option<bool> {
        self.text_views.get(&id).map(|e| e.editable)
    }

    /// The live (post-edit) text of every editable region, keyed by id — what the
    /// host publishes into the script each tick so `edit_view_text(id)` reads the
    /// user's edits back. Read-only regions are excluded (their text is the
    /// script's own, nothing to report).
    pub fn edit_view_texts(&self) -> std::collections::HashMap<i64, String> {
        self.text_views
            .iter()
            .filter(|(_, e)| e.editable)
            .map(|(id, e)| (*id, e.view.buffer.text()))
            .collect()
    }

    /// What each projected region's edits currently fold back to, keyed by id —
    /// published into the script each tick so `edit_view_edits(id)` can hand the
    /// payload to `mutate(...)`. Regions with no projection are absent.
    /// Memoized per region on the buffer revision.
    fn edit_view_edits(&mut self) -> std::collections::HashMap<i64, PanelData> {
        let mut out = std::collections::HashMap::new();
        for (id, e) in self.text_views.iter_mut() {
            if e.view.projection.is_none() {
                continue;
            }
            let revision = e.view.buffer.revision();
            if e.edits_cache
                .as_ref()
                .is_none_or(|(rev, _)| *rev != revision)
            {
                let lines = buffer_lines(&e.view);
                let edits = e.view.projection.as_ref().expect("checked").resolve(&lines);
                e.edits_cache = Some((revision, source_edits_to_data(&edits)));
            }
            if let Some((_, data)) = &e.edits_cache {
                out.insert(*id, data.clone());
            }
        }
        out
    }

    /// Route a key into editable region `id`, running the real vim state machine
    /// on its `EditorView` (full editing, motions, visual mode, undo) exactly as
    /// the editor panes do. `pane_rect` is the panel's absolute rect; `cell` the
    /// glyph size — together they give the viewport line/col counts vim needs for
    /// paging and cursor-visibility. No-op (returns false) if the region is
    /// missing or read-only. Returns true when the key was consumed (the caller
    /// then redraws).
    pub fn region_key(
        &mut self,
        id: i64,
        pane_rect: Rect,
        cell: (f32, f32),
        key: crate::vim::Key,
        clipboard: &mut dyn crate::clipboard::Clipboard,
    ) -> Option<crate::vim::Action> {
        let e = self.text_views.get_mut(&id)?;
        if !e.editable {
            return None;
        }
        let r = e.abs_rect(pane_rect);
        let vis = EditorView::visible_lines(r, cell.1);
        let cols = e.view.visible_cols(r, cell.0);
        // The action the region's vim wants the *host* to take — opening the
        // search prompt, say. `Action::None` is the overwhelmingly common
        // answer; a region that returns anything else is asking for chrome it
        // has no way to draw itself.
        let action = crate::vim::handle(&mut e.view, key, vis, cols, clipboard);
        e.view.ensure_cursor_visible(vis, cols);
        Some(action)
    }

    /// Run a search prompt's pattern against region `id`, exactly as
    /// [`App::accept_search`](crate::app::App::accept_search) does for a normal
    /// pane: record it as the region's last search (so `n`/`N` repeat it and
    /// the matches stay highlighted) and move the cursor to the first hit.
    /// Returns false when the pattern is not in the region — the caller says so.
    pub fn region_search(
        &mut self,
        id: i64,
        pane_rect: Rect,
        cell: (f32, f32),
        pattern: &str,
        forward: bool,
    ) -> bool {
        let Some(e) = self.text_views.get_mut(&id) else {
            return false;
        };
        let r = e.abs_rect(pane_rect);
        let view = &mut e.view;
        view.vim.last_search = Some(pattern.to_string());
        view.vim.last_search_forward = forward;
        view.vim.last_search_word = false; // prompt patterns are plain substrings
        view.vim.search_hl = true;
        let Some(p) = crate::search::find_next(&view.buffer, view.cursor, pattern, forward, false)
        else {
            return false;
        };
        crate::vim::collapse_selection_in_normal(view);
        view.cursor = p;
        view.desired_col = None;
        let vis = EditorView::visible_lines(r, cell.1);
        let cols = view.visible_cols(r, cell.0);
        view.ensure_cursor_visible(vis, cols);
        true
    }

    /// Take the reason the focused region's projection refused the last edit,
    /// if it refused one — the caller shows it in the status bar.
    pub fn take_edit_refusal(&mut self, id: i64) -> Option<String> {
        self.text_views
            .get_mut(&id)
            .and_then(|e| e.view.edit_refusal.take())
    }

    /// The vim mode of editable region `id` (so the caller can, e.g., treat
    /// Escape as "leave the region" only once the region is back in Normal mode).
    /// `None` for a missing or read-only region.
    pub fn region_mode(&self, id: i64) -> Option<crate::vim::Mode> {
        self.text_views
            .get(&id)
            .filter(|e| e.editable)
            .map(|e| e.view.vim.mode)
    }

    // Consume-or-forward rule for region scroll input (wheel + nav keys): a
    // region consumes a scroll input iff its content actually overflows its
    // viewport on that axis. This is a property of the *content*, not of the
    // current offset — while a diff overflows, the region owns navigation
    // consistently, so autorepeat-scrolling into the top/bottom boundary never
    // leaks keys into the script mid-gesture (which would, e.g., suddenly move
    // an app's list selection). Content that fits its rect never captures
    // scroll input at all, so every wheel tick and nav key reaches the script
    // (`scroll_y()` / `key_pressed(...)`) and script-side navigation keeps
    // working over selectable text.

    /// Wheel-scroll region `id` vertically by `lines` rows — fractional, so a
    /// trackpad's pixel deltas scroll it smoothly (native editor scroll,
    /// independent of the script's own scroll state). Returns whether the region
    /// consumed the wheel: false when its content fits its viewport (nothing to
    /// scroll), so the caller forwards the wheel to the script instead.
    pub fn region_scroll(
        &mut self,
        id: i64,
        pane_rect: Rect,
        cell: (f32, f32),
        lines: f32,
    ) -> bool {
        let Some(e) = self.text_views.get_mut(&id) else {
            return false;
        };
        let r = e.abs_rect(pane_rect);
        let vis = EditorView::visible_lines(r, cell.1);
        let cols = e.view.visible_cols(r, cell.0);
        if !e.view.can_scroll_v(vis, cols) {
            return false;
        }
        e.view.scroll_by(lines, vis, cols);
        true
    }

    /// Wheel-scroll region `id` horizontally by `cols` display columns
    /// (fractional, like [`region_scroll`](Self::region_scroll)). Returns
    /// whether the region consumed the wheel: false when no line is wider than
    /// the viewport, so the caller forwards the wheel to the script instead.
    pub fn region_scroll_h(
        &mut self,
        id: i64,
        pane_rect: Rect,
        cell: (f32, f32),
        cols: f32,
    ) -> bool {
        let Some(e) = self.text_views.get_mut(&id) else {
            return false;
        };
        let r = e.abs_rect(pane_rect);
        let vis_cols = e.view.visible_cols(r, cell.0);
        if !e.view.can_scroll_h(vis_cols) {
            return false;
        }
        e.view.scroll_h_by(cols);
        true
    }

    /// Scroll region `id` in response to a navigation key when it holds focus
    /// (arrows / `j` / `k` / page / home / end / space). Returns whether the
    /// key was consumed; a non-scroll key — or any key while the content fits
    /// its viewport (see the consume-or-forward rule above) — is left for the
    /// script. This is what makes a focused, overflowing text region navigable
    /// by keyboard like a real text view without stealing navigation keys from
    /// scripts whose regions have nothing to scroll.
    pub fn region_scroll_key(
        &mut self,
        id: i64,
        pane_rect: Rect,
        cell: (f32, f32),
        key: &str,
    ) -> bool {
        let Some(e) = self.text_views.get_mut(&id) else {
            return false;
        };
        let r = e.abs_rect(pane_rect);
        let vis = EditorView::visible_lines(r, cell.1);
        let cols = e.view.visible_cols(r, cell.0);
        // A page is a viewport of lines; home/end use a delta past any row a
        // buffer can have, which `scroll_by` clamps to the first/last line
        // (it stops stepping at the ends rather than walking the whole delta).
        // Keys move whole rows — sub-row steps belong to the wheel.
        let span = 1e9_f32;
        let delta: f32 = match key {
            "down" | "j" => 1.0,
            "up" | "k" => -1.0,
            "pagedown" | "space" => vis as f32,
            "pageup" => -(vis as f32),
            "home" => -span,
            "end" => span,
            _ => return false,
        };
        if !e.view.can_scroll_v(vis, cols) {
            return false;
        }
        e.view.scroll_by(delta, vis, cols);
        true
    }

    /// If `(x, y)` presses region `id`'s own scrollbar, grab it (performing the
    /// initial jump), focus the region, and return the axis + grab offset for
    /// the ensuing drag. `None` if the press isn't on a scrollbar.
    pub fn region_scrollbar_grab(
        &mut self,
        id: i64,
        pane_rect: Rect,
        cell: (f32, f32),
        x: f32,
        y: f32,
    ) -> Option<(ScrollAxis, f32)> {
        let e = self.text_views.get_mut(&id)?;
        let r = e.abs_rect(pane_rect);
        let hit = e.view.scrollbar_hit(r, cell, x, y)?;
        e.view.drag_scroll(hit.axis, hit.grab, r, cell, x, y);
        self.focused_region = Some(id);
        Some((hit.axis, hit.grab))
    }

    /// Continue a scrollbar drag on region `id`.
    pub fn region_scrollbar_drag(
        &mut self,
        id: i64,
        pane_rect: Rect,
        cell: (f32, f32),
        axis: ScrollAxis,
        grab: f32,
        x: f32,
        y: f32,
    ) {
        if let Some(e) = self.text_views.get_mut(&id) {
            let r = e.abs_rect(pane_rect);
            e.view.drag_scroll(axis, grab, r, cell, x, y);
        }
    }

    /// Live-recompile the panel from a paired editor buffer's current text (the
    /// Petal-IDE binding): the editor on the left drives the canvas on the right
    /// without a save round-trip. Hash-gated, so re-scanning the same text every
    /// awake frame is a no-op. On a real change it recompiles preserving Petal
    /// `state` (via [`PanelHost::reload_source`]), wakes the panel, and clears/sets
    /// the error banner. A compile error keeps the last good render and shows the
    /// message. Returns true when it applied a change (redraw wanted).
    pub fn reload_from_editor(&mut self, source: &str, now: Instant) -> bool {
        let hash = hash_text(source);
        if self.live_source_hash == Some(hash) {
            return false;
        }
        self.live_source_hash = Some(hash);
        match self.host.reload_source(source) {
            // A clean compile swaps in the new program; clear both errors.
            Ok(()) => {
                self.reload_error = None;
                self.frame_error = None;
            }
            // A broken buffer: keep the last good program and hold the banner up
            // (a runtime frame of the old program must not clear it — see `tick`).
            Err(err) => self.reload_error = Some(err),
        }
        self.note_activity(now);
        true
    }

    /// Hard-restart the panel from its script file, **discarding** Petal `state`
    /// (the canvas re-runs its `state x = …` initializers from scratch) — the
    /// Petal-IDE toolbar's Reset. Unlike [`reload_from_editor`](Self::reload_from_editor)
    /// / [`poll_reload`](Self::poll_reload), no state is transferred. Resetting
    /// `live_source_hash` makes the next live-binding tick re-apply the editor's
    /// (possibly unsaved) buffer onto the fresh state, so unsaved edits are kept
    /// but their animation clock/counters start over. Keeps running on failure.
    pub fn restart(&mut self, now: Instant) -> bool {
        let path = self.host.path().to_path_buf();
        match PanelHost::load(&path) {
            Ok(mut host) => {
                adopt_font(&mut host);
                self.host = host;
                self.reload_error = None;
                self.frame_error = None;
                self.text_views.clear();
                self.frame_count = 0;
                self.last_frame = None;
                self.observed_frame = -1;
                self.partial_observed = None;
                self.live_source_hash = None;
                self.note_activity(now);
                true
            }
            Err(err) => {
                self.reload_error = Some(err);
                true
            }
        }
    }

    /// Whether this panel runs a script loaded from a real file, rather than
    /// source pushed by a GPP subprocess. Only a file-backed panel can be
    /// restarted from disk ([`restart`](Self::restart)); a pushed one has no
    /// file to reload (its "path" is the client binary).
    pub fn is_file_backed(&self) -> bool {
        self.client.is_none()
    }

    /// Poll the script file and hot-reload on change. Returns true if it
    /// reloaded (the caller should redraw). A reload re-wakes the panel.
    pub fn poll_reload(&mut self, now: Instant) -> bool {
        match self.host.poll_reload() {
            Ok(true) => {
                self.note_activity(now);
                self.reload_error = None;
                self.frame_error = None;
                true
            }
            Ok(false) => false,
            Err(err) => {
                self.reload_error = Some(err);
                true
            }
        }
    }

    /// Render the cached frame into `rect`, plus a tiny sleep/wake indicator dot
    /// in the top-right corner.
    ///
    /// The panel's solid geometry — the base fill, every shape, each
    /// `text_view` region's chrome, and the indicator dot — is tessellated into
    /// [`Primitive::Mesh`] runs in submission order, GPU-scissored to `rect` so
    /// a sketch can't paint over its neighbors. Text and images are separate
    /// primitives.
    ///
    /// Geometry batches into as few meshes as possible, but the batch is
    /// **flushed before any non-geometry primitive** ([`flush_mesh`]), so the
    /// emitted list is in true submission order. The renderer honours
    /// painter's order across primitive kinds, so a `draw_rect` after a
    /// `draw_text` really does cover it — which is what makes an overlay (a
    /// context menu, a modal) possible (see
    /// `docs/petal-graphical-panels.md`).
    pub fn build_scene(
        &self,
        rect: Rect,
        cell: (f32, f32),
        theme: &Theme,
        awake: bool,
        prims: &mut Vec<Primitive>,
    ) {
        let mut verts: Vec<Vertex> = Vec::new();

        // Base fill, so a script that never calls clear() doesn't show garbage.
        tess::rect(&mut verts, rect.x, rect.y, rect.w, rect.h, theme.pane_bg);

        // Panel-local (x, y) → absolute pane pixels.
        let ox = rect.x;
        let oy = rect.y;
        // Active clip for subsequent geometry/text. Geometry accumulates into one
        // mesh per clip region: a `Clip`/`ClipNone` command flushes the current
        // verts as a mesh scissored to the *previous* clip, then switches. Every
        // clip is intersected with the pane so a script can't paint outside it.
        let mut cur_clip = rect;
        for cmd in &self.cmds {
            match cmd {
                PanelCmd::Clip { x, y, w, h } => {
                    flush_mesh(prims, &mut verts, cur_clip);
                    cur_clip = intersect(
                        rect,
                        Rect {
                            x: ox + *x as f32,
                            y: oy + *y as f32,
                            w: *w as f32,
                            h: *h as f32,
                        },
                    );
                }
                PanelCmd::ClipNone => {
                    flush_mesh(prims, &mut verts, cur_clip);
                    cur_clip = rect;
                }
                PanelCmd::TextView { id, .. } => {
                    // Render the region's native editor into this panel's own
                    // stream. The editor emits its solid geometry — region
                    // background, border, cursor line, caret, scrollbars, diff
                    // bands — as `Primitive::Quad`, but the renderer draws
                    // *every* quad beneath *every* mesh, and a panel paints its
                    // whole surface as one mesh. Left as quads they would all be
                    // buried under it (only the text, which composites above
                    // meshes, would survive). So they are tessellated into the
                    // panel's mesh instead, where submission order is honoured;
                    // text passes straight through.
                    if let Some(entry) = self.text_views.get(id) {
                        let abs = entry.abs_rect(rect);
                        let focused = self.focused_region == Some(*id);
                        let mut region = Vec::new();
                        entry
                            .view
                            .build_scene(abs, cell, focused, theme, &mut region);
                        for p in region {
                            match p {
                                Primitive::Quad { rect: r, color } => {
                                    tess::rect(&mut verts, r.x, r.y, r.w, r.h, color)
                                }
                                other => {
                                    // Keep the region's own submission order:
                                    // its text must land after the chrome
                                    // tessellated above it, not before.
                                    flush_mesh(prims, &mut verts, cur_clip);
                                    // …and it obeys the panel's active clip like
                                    // anything else drawn under it: the region
                                    // carries its own clip (its interior), so the
                                    // two are intersected rather than replaced.
                                    prims.push(clip_to(other, cur_clip));
                                }
                            }
                        }
                    }
                }
                // Styling, wrapping and programmatic scrolls are applied to the
                // region's editor during sync, not at render time — nothing to
                // draw here. (A scroll is consumed there, so one never reaches
                // this match.)
                PanelCmd::TextViewStyles { .. }
                | PanelCmd::TextViewProjection { .. }
                | PanelCmd::TextViewScrollTo { .. }
                | PanelCmd::TextViewWrap { .. } => {}
                PanelCmd::Clear { r, g, b } => {
                    tess::rect(
                        &mut verts,
                        rect.x,
                        rect.y,
                        rect.w,
                        rect.h,
                        col(*r, *g, *b, 255),
                    );
                }
                PanelCmd::Image {
                    source,
                    x,
                    y,
                    w,
                    h,
                    a,
                } => {
                    flush_mesh(prims, &mut verts, cur_clip);
                    prims.push(Primitive::Image {
                        rect: Rect {
                            x: ox + *x as f32,
                            y: oy + *y as f32,
                            w: *w as f32,
                            h: *h as f32,
                        },
                        source: source.clone(),
                        alpha: *a as f32 / 255.0,
                        clip: cur_clip,
                    });
                }
                PanelCmd::Rect {
                    x,
                    y,
                    w,
                    h,
                    r,
                    g,
                    b,
                    a,
                    radius,
                } => {
                    let (px, py, pw, ph) = (ox + *x as f32, oy + *y as f32, *w as f32, *h as f32);
                    if *radius > 0 {
                        tess::rect_rounded(
                            &mut verts,
                            px,
                            py,
                            pw,
                            ph,
                            *radius as f32,
                            col(*r, *g, *b, *a),
                        );
                    } else {
                        tess::rect(&mut verts, px, py, pw, ph, col(*r, *g, *b, *a));
                    }
                }
                PanelCmd::RectOutline {
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
                } => {
                    let t = (*width as f32).max(1.0);
                    let (px, py, pw, ph) = (ox + *x as f32, oy + *y as f32, *w as f32, *h as f32);
                    if *radius > 0 {
                        tess::rect_rounded_outline(
                            &mut verts,
                            px,
                            py,
                            pw,
                            ph,
                            *radius as f32,
                            t,
                            col(*r, *g, *b, *a),
                        );
                    } else {
                        tess::rect_outline(&mut verts, px, py, pw, ph, t, col(*r, *g, *b, *a));
                    }
                }
                PanelCmd::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    r,
                    g,
                    b,
                    a,
                    width,
                } => {
                    tess::line(
                        &mut verts,
                        ox + *x1 as f32,
                        oy + *y1 as f32,
                        ox + *x2 as f32,
                        oy + *y2 as f32,
                        *width as f32,
                        col(*r, *g, *b, *a),
                    );
                }
                PanelCmd::Circle {
                    cx,
                    cy,
                    radius,
                    r,
                    g,
                    b,
                    a,
                } => {
                    tess::circle(
                        &mut verts,
                        ox + *cx as f32,
                        oy + *cy as f32,
                        *radius as f32,
                        col(*r, *g, *b, *a),
                    );
                }
                PanelCmd::Triangle {
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
                } => {
                    tess::triangle(
                        &mut verts,
                        (ox + *x1 as f32, oy + *y1 as f32),
                        (ox + *x2 as f32, oy + *y2 as f32),
                        (ox + *x3 as f32, oy + *y3 as f32),
                        col(*r, *g, *b, *a),
                    );
                }
                PanelCmd::Poly { points, r, g, b, a } => {
                    let pts = abs_points(points, ox, oy);
                    tess::poly(&mut verts, &pts, col(*r, *g, *b, *a));
                }
                PanelCmd::Polygon { points, r, g, b, a } => {
                    let pts = abs_points(points, ox, oy);
                    tess::polygon(&mut verts, &pts, col(*r, *g, *b, *a));
                }
                PanelCmd::Fan {
                    cx,
                    cy,
                    points,
                    r,
                    g,
                    b,
                    a,
                } => {
                    let pts = abs_points(points, ox, oy);
                    tess::fan(
                        &mut verts,
                        ox + *cx as f32,
                        oy + *cy as f32,
                        &pts,
                        col(*r, *g, *b, *a),
                    );
                }
                PanelCmd::Polyline {
                    points,
                    r,
                    g,
                    b,
                    a,
                    width,
                } => {
                    let pts = abs_points(points, ox, oy);
                    tess::polyline(&mut verts, &pts, *width as f32, col(*r, *g, *b, *a));
                }
                PanelCmd::Ellipse {
                    cx,
                    cy,
                    rx,
                    ry,
                    r,
                    g,
                    b,
                    a,
                } => {
                    tess::ellipse(
                        &mut verts,
                        ox + *cx as f32,
                        oy + *cy as f32,
                        *rx as f32,
                        *ry as f32,
                        col(*r, *g, *b, *a),
                    );
                }
                PanelCmd::EllipseOutline {
                    cx,
                    cy,
                    rx,
                    ry,
                    r,
                    g,
                    b,
                    a,
                    width,
                } => {
                    tess::ellipse_outline(
                        &mut verts,
                        ox + *cx as f32,
                        oy + *cy as f32,
                        *rx as f32,
                        *ry as f32,
                        *width as f32,
                        col(*r, *g, *b, *a),
                    );
                }
                PanelCmd::Arc {
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
                } => {
                    tess::arc(
                        &mut verts,
                        ox + *cx as f32,
                        oy + *cy as f32,
                        *r_in,
                        *r_out,
                        *a0,
                        *a1,
                        col(*r, *g, *b, *a),
                    );
                }
                PanelCmd::Text {
                    text,
                    x,
                    y,
                    size,
                    r,
                    g,
                    b,
                    a,
                    // `ui` selects the embedded proportional face; every other
                    // name resolves to the monospace one. The measurement side
                    // resolves names the same way (an unbound role falls back to
                    // the default metrics), so the two agree either way.
                    font,
                    weight,
                    italic,
                    spacing,
                } => {
                    // The script's own size — a panel is not locked to the
                    // editor's 14 px, so typographic hierarchy survives.
                    // 0 would render nothing, so treat it as "default".
                    let size = match *size {
                        0 => FONT_SIZE,
                        s => s as f32,
                    };
                    // Close the pending mesh first: the renderer honours
                    // painter's order across primitive kinds, so a text run
                    // pushed while shapes are still queued would sit *under*
                    // them. (Same reason `Image` flushes.)
                    flush_mesh(prims, &mut verts, cur_clip);
                    push_text_run(
                        prims,
                        (ox + *x as f32, oy + *y as f32),
                        text,
                        col(*r, *g, *b, *a),
                        cur_clip,
                        size,
                        TextStyle {
                            weight: *weight,
                            italic: *italic,
                            spacing: *spacing,
                            font: font
                                .as_deref()
                                .map_or(garden_render::FontRole::Mono, garden_render::FontRole::from_name),
                        },
                    );
                }
            }
        }

        // Flush whatever clip region was active when the commands ended.
        flush_mesh(prims, &mut verts, cur_clip);

        // Sleep/wake indicator: a tiny dot, top-right, always over the whole pane
        // (never clipped by the script). Filled green awake; dim when asleep.
        let d = 5.0;
        let color = if awake {
            Color::rgb(0.36, 0.85, 0.47)
        } else {
            theme.text_dim
        };
        tess::rect(
            &mut verts,
            rect.x + rect.w - d - 4.0,
            rect.y + 4.0,
            d,
            d,
            color,
        );

        prims.push(Primitive::Mesh {
            vertices: verts,
            clip: rect,
        });

        if let Some(err) = self.error() {
            build_error_card(err, rect, cell, prims);
        }
    }
}

/// Draw the panel's error as a wrapped card across the top of the pane, over the
/// last good frame. Petal's error string is multi-line — a headline
/// (`"<message> [line N, column M]"`) followed by a source snippet (a `NNN |
/// code` line and a caret line). The old banner flattened all of that onto one
/// unwrapped line that ran off the pane and overlapped the drawer's own content;
/// this wraps the headline to the pane width, keeps each snippet line on its own
/// (truncated) row, and sizes an opaque card to fit — so a long error stays
/// readable and never bleeds past the pane.
fn build_error_card(err: &str, rect: Rect, cell: (f32, f32), prims: &mut Vec<Primitive>) {
    const MAX_SNIPPET_ROWS: usize = 6;
    let pad = 8.0_f32;
    let accent_h = 3.0_f32;
    let line_h = cell.1.max(12.0);
    let char_w = cell.0.max(1.0);
    let inner_w = (rect.w - 2.0 * pad).max(0.0);
    let cols = ((inner_w / char_w).floor() as usize).max(8);

    // Row list: (text, dim?). The headline (message + position) wraps by word
    // and reads bright; the source snippet lines are dimmer and truncated.
    let mut lines = err.split('\n');
    let headline = lines.next().unwrap_or("");
    let mut rows: Vec<(String, bool)> = Vec::new();
    for l in wrap_words(&format!("⚠ panel error: {headline}"), cols) {
        rows.push((l, false));
    }
    let mut snippet_shown = 0;
    for l in lines {
        if l.trim().is_empty() {
            continue; // drop the blank gutter line petal emits above the code
        }
        if snippet_shown == MAX_SNIPPET_ROWS {
            rows.push(("…".to_string(), true));
            break;
        }
        rows.push((truncate_chars(l, cols), true));
        snippet_shown += 1;
    }

    // A card sized to the rows, clamped to the pane. Nearly opaque so the frame
    // beneath doesn't bleed through, with a bright top accent stripe.
    let card_h = (pad + rows.len() as f32 * line_h + pad).min(rect.h);
    let mut card = Vec::new();
    tess::rect(
        &mut card,
        rect.x,
        rect.y,
        rect.w,
        card_h,
        Color::rgba(0.30, 0.06, 0.07, 0.97),
    );
    tess::rect(
        &mut card,
        rect.x,
        rect.y,
        rect.w,
        accent_h.min(card_h),
        Color::rgb(0.86, 0.28, 0.30),
    );
    prims.push(Primitive::Mesh {
        vertices: card,
        clip: rect,
    });

    let card_clip = Rect {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: card_h,
    };
    let mut y = rect.y + pad + accent_h;
    for (text, dim) in rows {
        let color = if dim {
            Color::rgb(0.93, 0.76, 0.78)
        } else {
            Color::rgb(1.0, 0.92, 0.92)
        };
        prims.push(Primitive::Text {
            pos: (rect.x + pad, y),
            text,
            color,
            clip: card_clip,
            size: FONT_SIZE,
            style: TextStyle::default(),
        });
        y += line_h;
    }
}

/// Word-wrap `text` to at most `cols` characters per line, breaking on
/// whitespace and hard-breaking any single word longer than `cols`. Always
/// returns at least one line.
fn wrap_words(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize; // char count of `cur`
    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        if wlen > cols {
            // A single over-long word (e.g. a path or a run of `^`): flush, then
            // hard-break it across as many rows as needed.
            if cur_len > 0 {
                lines.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            for ch in word.chars() {
                if cur_len == cols {
                    lines.push(std::mem::take(&mut cur));
                    cur_len = 0;
                }
                cur.push(ch);
                cur_len += 1;
            }
            continue;
        }
        let sep = usize::from(cur_len > 0);
        if cur_len + sep + wlen > cols {
            lines.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if cur_len > 0 {
            cur.push(' ');
            cur_len += 1;
        }
        cur.push_str(word);
        cur_len += wlen;
    }
    if cur_len > 0 || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Truncate `s` to `cols` characters, marking a cut with a trailing ellipsis.
fn truncate_chars(s: &str, cols: usize) -> String {
    if s.chars().count() <= cols {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cols.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Color from 0..=255 RGBA (sRGB; the renderer linearizes and alpha-blends).
/// Panels get real translucency this way — overlapping selection/hover tints
/// composite instead of being pre-mixed opaque.
fn col(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::rgba(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    )
}

/// Offset a command's panel-local point list into absolute pane pixels — the
/// one translation every point-list primitive (poly, polygon, fan, polyline)
/// needs before it reaches the tessellator.
fn abs_points(points: &[(i32, i32)], ox: f32, oy: f32) -> Vec<(f32, f32)> {
    points
        .iter()
        .map(|(x, y)| (ox + *x as f32, oy + *y as f32))
        .collect()
}

/// Intersection of two rects (empty — zero size — when they don't overlap), so a
/// panel's clip region can never extend past its pane.
fn intersect(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.w).min(b.x + b.w);
    let bottom = (a.y + a.h).min(b.y + b.h);
    Rect {
        x,
        y,
        w: (right - x).max(0.0),
        h: (bottom - y).max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use garden_script::PanelHost;
    use std::io::Write;

    const RECT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 300.0,
        h: 200.0,
    };
    const CELL: (f32, f32) = (8.0, 16.0);

    /// A panel whose only frame declares one `text_view` region with `text`.
    /// The temp file must outlive `load` (which reads it), so it is returned
    /// alongside the panel and dropped by the caller at end of test.
    fn text_view_panel(text: &str) -> (PanelView, tempfile::NamedTempFile) {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(f, "text_view(1, 0, 0, 200, 100, \"{text}\")\n").unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        (pv, f)
    }

    /// Count the translucent rectangles (a diff-row tint or a selection band,
    /// both < 1.0 alpha) a region contributed to the panel's **mesh** stream.
    /// They are tessellated into the panel's mesh rather than left as `Quad`s
    /// so that they sit in the panel's own submission order, interleaved with
    /// the shapes around them, rather than forming a separate batch whose
    /// position relative to that geometry depends on emission accidents.
    fn translucent_bands(prims: &[Primitive]) -> usize {
        let translucent = |c: &Color| c.a > 0.0 && c.a < 0.9;
        assert!(
            !prims
                .iter()
                .any(|p| matches!(p, Primitive::Quad { color, .. } if translucent(color))),
            "a region's geometry must join the panel mesh, not stay a buried Quad"
        );
        prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Mesh { vertices, .. } => Some(vertices),
                _ => None,
            })
            .flat_map(|v| v.chunks(6)) // one `tess::rect` = two triangles
            .filter(|tri| tri.len() == 6 && translucent(&tri[0].color))
            .count()
    }

    #[test]
    fn line_styles_paint_translucent_diff_bands() {
        // A region with two styled diff rows (one added, one removed).
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            "text_view(1, 0, 0, 200, 100, \"+ added line\\n- removed line\")\n\
             text_view_line_styles(1, [\"added\", \"removed\"])\n"
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        let theme = Theme::default();
        let mut prims = Vec::new();
        pv.build_scene(RECT, CELL, &theme, true, &mut prims);
        // No selection is active, so any translucent rects are the styling bands.
        assert!(
            translucent_bands(&prims) >= 2,
            "expected an added and a removed background band"
        );
    }

    /// A focused region has to *show* where the cursor is, and an overflowing one
    /// where it is in the buffer. Both the caret and the scrollbar are solid
    /// geometry, so — like the diff bands above — they only render if they join
    /// the panel's mesh; left as `Quad`s the panel's own surface mesh buries
    /// them, and an editable region reads as a plain text dump.
    #[test]
    fn a_focused_region_paints_its_caret_and_scrollbar_into_the_panel_mesh() {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        let lines = (1..=40).map(|i| format!("line {i}")).collect::<Vec<_>>();
        write!(
            f,
            "edit_view(1, 0, 0, 200, 100, \"{}\")\n",
            lines.join("\\n")
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        pv.region_press(1, RECT, CELL, 10.0, 10.0, false, 1);
        assert_eq!(pv.focused_region(), Some(1));

        let theme = Theme::default();
        let mut prims = Vec::new();
        pv.build_scene(RECT, CELL, &theme, true, &mut prims);
        let mesh_has = |color: Color| {
            prims.iter().any(|p| match p {
                Primitive::Mesh { vertices, .. } => vertices.iter().any(|v| v.color == color),
                _ => false,
            })
        };
        assert!(mesh_has(theme.cursor_block), "no caret in the panel mesh");
        assert!(
            mesh_has(theme.scrollbar_thumb),
            "40 lines in a 100px region overflow — no scrollbar thumb in the mesh"
        );
    }

    // --- projected regions ---------------------------------------------------

    /// A panel whose one `edit_view` is a projection of `a.txt`: the base is
    /// `one / two / three`, the working tree `one / TWO / three`. Declared the
    /// way a drawer does — the same spec every frame, so the host keeps the live
    /// table (and the user's edits) rather than rebuilding it.
    fn projected_panel() -> (PanelView, tempfile::NamedTempFile) {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            r#"
let seed = "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three"
edit_view(1, 0, 0, 400, 200, seed)
edit_view_projection(1, {{
  sources: ["a.txt"],
  span_source: [0], span_start: [0], span_end: [3], span_group: [0],
  kinds: "h -+ ",
  line_spans: [0, 0, 0, 0, 0],
  styles: ["hunk", "", "removed", "added", ""],
  decor: {{ same: " ", added: "+", removed: "-",
           same_style: "", added_style: "added", removed_style: "removed",
           diff_markers: true }}
}})
"#
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        (pv, f)
    }

    /// The write-back region 1 currently resolves to, as `(source, start, end,
    /// lines)`.
    fn resolved(pv: &PanelView) -> Vec<(String, usize, usize, Vec<String>)> {
        let e = pv.text_views.get(&1).expect("region 1");
        let proj = e
            .view
            .projection
            .as_ref()
            .expect("region 1 has a projection");
        proj.resolve(&buffer_lines(&e.view))
            .into_iter()
            .map(|e| (e.source, e.start, e.end, e.lines))
            .collect()
    }

    /// The same write-back as [`resolved`], as the panel data a script receives
    /// — the wire shape `mutate("apply", …)` carries.
    fn resolved_data(pv: &mut PanelView) -> PanelData {
        pv.edit_view_edits().remove(&1).expect("region 1")
    }

    /// The same fixture as [`projected_panel`], but in **gutter** mode: the
    /// seed carries the files' own text and the `+`/`-`/space markers are
    /// declared as display chrome for the host to draw beside it.
    fn gutter_panel() -> (PanelView, tempfile::NamedTempFile) {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            r#"
let seed = "@@ -1,3 +1,3 @@\none\ntwo\nTWO\nthree"
edit_view(1, 0, 0, 400, 200, seed)
edit_view_projection(1, {{
  sources: ["a.txt"],
  span_source: [0], span_start: [0], span_end: [3], span_group: [0],
  kinds: "h -+ ",
  line_spans: [0, 0, 0, 0, 0],
  styles: ["hunk", "", "removed", "added", ""],
  decor: {{ same: " ", added: "+", removed: "-",
           same_style: "", added_style: "added", removed_style: "removed",
           diff_markers: false, gutter: true }}
}})
"#
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        (pv, f)
    }

    /// A gutter projection's buffer holds the files' own text — no line wears a
    /// marker — and the prefixed fixture beside it still does, so this is the
    /// mode changing and not the fixture.
    #[test]
    fn a_gutter_projection_keeps_the_markers_out_of_the_buffer() {
        let (pv, _f) = gutter_panel();
        assert_eq!(
            buffer_lines(&pv.text_views.get(&1).unwrap().view),
            ["@@ -1,3 +1,3 @@", "one", "two", "TWO", "three"]
        );
        // Untouched, so it resolves nothing at all: only a span whose content
        // actually moved is written back. (What it folds *to* once edited is
        // `joining_lines_in_a_gutter_region_writes_no_marker_into_the_file`.)
        assert!(resolved(&pv).is_empty());

        let (plain, _g) = projected_panel();
        assert_eq!(
            buffer_lines(&plain.text_views.get(&1).unwrap().view),
            ["@@ -1,3 +1,3 @@", " one", "-two", "+TWO", " three"]
        );
    }

    /// The markers reach the screen: each content line's glyph is drawn as its
    /// own text primitive, to the left of the line's text and never inside it.
    #[test]
    fn gutter_markers_are_drawn_beside_the_text() {
        let (pv, _f) = gutter_panel();
        let theme = Theme::default();
        let mut prims = Vec::new();
        pv.build_scene(RECT, CELL, &theme, true, &mut prims);
        let runs: Vec<(String, f32)> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Text { text, pos, .. } => Some((text.clone(), pos.0)),
                _ => None,
            })
            .collect();

        // One marker per content line: context, deletion, addition, context.
        // (The hunk header is chrome and gets none.)
        let markers: Vec<&(String, f32)> = runs
            .iter()
            .filter(|(t, _)| t == " " || t == "-" || t == "+")
            .collect();
        assert_eq!(
            markers.len(),
            4,
            "expected a glyph per content line, got {runs:?}"
        );
        assert_eq!(markers.iter().filter(|(t, _)| t == "+").count(), 1);
        assert_eq!(markers.iter().filter(|(t, _)| t == "-").count(), 1);

        // Every marker sits left of every line of text — the column is a gutter,
        // not part of the content.
        let text_x = runs
            .iter()
            .filter(|(t, _)| t == "one" || t == "two" || t == "TWO" || t == "three")
            .map(|(_, x)| *x)
            .fold(f32::INFINITY, f32::min);
        for (glyph, x) in &markers {
            assert!(
                *x < text_x,
                "marker {glyph:?} at x={x} is not left of the text at x={text_x}"
            );
        }

        // And the prefixed fixture draws no gutter at all — its markers are
        // already in the text, so a column would show them twice.
        let (plain, _g) = projected_panel();
        let mut plain_prims = Vec::new();
        plain.build_scene(RECT, CELL, &theme, true, &mut plain_prims);
        let plain_markers = plain_prims
            .iter()
            .filter(|p| matches!(p, Primitive::Text { text, .. } if text == "+" || text == "-"))
            .count();
        assert_eq!(plain_markers, 0);
    }

    /// The point of the exercise, at the level the user hit it: `J` in a gutter
    /// projection joins two lines of code, with no marker to drag into the seam.
    #[test]
    fn joining_lines_in_a_gutter_region_writes_no_marker_into_the_file() {
        let (mut pv, _f) = gutter_panel();
        pv.region_press(1, RECT, CELL, 10.0, 10.0, false, 1);
        // Down to the addition ("TWO"), then join it with the line below.
        press_keys(&mut pv, "jjj");
        press_keys(&mut pv, "J");
        assert_eq!(
            resolved(&pv),
            vec![(
                "a.txt".to_string(),
                0,
                3,
                vec!["one".to_string(), "TWO three".to_string()]
            )],
            "the join is a join — no `+` lands in the file"
        );
    }

    fn press_keys(pv: &mut PanelView, keys: &str) {
        let mut clip = crate::clipboard::InMemoryClipboard::default();
        for ch in keys.chars() {
            pv.region_key(1, RECT, CELL, crate::vim::Key::Char(ch), &mut clip);
        }
    }

    #[test]
    fn a_declared_projection_reaches_the_regions_editor() {
        let (mut pv, _f) = projected_panel();
        // Freshly built and untouched: nothing to write back. `^S` here used to
        // splice this view's idea of the hunk over whatever the file held.
        assert_eq!(resolved(&pv), vec![]);
        // The table is nonetheless live — an edit resolves against the file.
        pv.region_press(1, RECT, CELL, 10.0, 10.0, false, 1);
        press_keys(&mut pv, "4Glx"); // `+TWO` → `+WO`
        assert_eq!(
            resolved(&pv),
            vec![(
                "a.txt".to_string(),
                0,
                3,
                vec!["one".to_string(), "WO".to_string(), "three".to_string()]
            )]
        );
    }

    /// The wire shape a script hands `mutate("apply", …)`: each edit carries the
    /// lines to write **and** the lines the span held when the view loaded, so
    /// the writer can tell the file changed underneath it.
    #[test]
    fn resolved_edits_carry_the_expected_source_lines() {
        let (mut pv, _f) = projected_panel();
        // Clean: the published list is empty, which is what an unsaved-changes
        // indicator counts.
        assert_eq!(resolved_data(&mut pv), PanelData::List(vec![]));

        pv.region_press(1, RECT, CELL, 10.0, 10.0, false, 1);
        press_keys(&mut pv, "3Gdd"); // revert the deletion of `two`
        let strs = |ls: &[&str]| {
            PanelData::List(ls.iter().map(|s| PanelData::Str(s.to_string())).collect())
        };
        assert_eq!(
            resolved_data(&mut pv),
            PanelData::List(vec![PanelData::Record(vec![
                ("source".into(), PanelData::Str("a.txt".into())),
                ("start".into(), PanelData::Int(0)),
                ("end".into(), PanelData::Int(3)),
                ("lines".into(), strs(&["one", "two", "TWO", "three"])),
                ("expected".into(), strs(&["one", "TWO", "three"])),
            ])])
        );
    }

    /// The point of the whole exercise: real vim in the region, and the edit
    /// folds back as intent rather than as text.
    #[test]
    fn vim_in_a_projected_region_folds_back_to_the_source() {
        let (mut pv, _f) = projected_panel();
        pv.region_press(1, RECT, CELL, 10.0, 10.0, false, 1);
        press_keys(&mut pv, "3Gdd"); // dd on `-two` — revert that deletion
        assert_eq!(
            resolved(&pv)[0].3,
            vec![
                "one".to_string(),
                "two".to_string(),
                "TWO".to_string(),
                "three".to_string()
            ]
        );
    }

    /// Deleting the hunk header is a structural request, not a line deletion.
    #[test]
    fn deleting_the_hunk_header_in_a_projected_region_reverts_the_hunk() {
        let (mut pv, _f) = projected_panel();
        pv.region_press(1, RECT, CELL, 10.0, 10.0, false, 1);
        press_keys(&mut pv, "ggdd");
        assert_eq!(
            resolved(&pv)[0].3,
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
    }

    /// Re-declaring the same spec every frame must not rebuild the projection —
    /// that would throw the user's edits away 60 times a second.
    #[test]
    fn a_re_declared_projection_keeps_the_live_edits() {
        let (mut pv, _f) = projected_panel();
        pv.region_press(1, RECT, CELL, 10.0, 10.0, false, 1);
        press_keys(&mut pv, "3Gdd");
        let after_edit = resolved(&pv);
        assert_eq!(after_edit.len(), 1);
        for _ in 0..3 {
            pv.tick(Instant::now(), RECT, CELL);
        }
        // Unchanged in both directions: the table still holds the edit, and the
        // baseline was *not* re-captured (which would quietly adopt the edit as
        // the original content and report the span clean).
        assert_eq!(resolved(&pv), after_edit);
    }

    /// `text_view_wrap` is what makes a long diff line readable: the region's
    /// editor lays the line out over several visual rows instead of running it
    /// off the right edge. Wrapping is opt-in, so the same region without the
    /// call stays on one row (a row-aligned pair of regions depends on that).
    #[test]
    fn text_view_wrap_lays_a_long_line_over_several_rows() {
        // ~30 words in a 200px-wide region at an 8px cell — about 24 columns.
        let long = (1..=30).map(|i| format!("word{i}")).collect::<Vec<_>>();
        let rows_for = |wrap: bool| {
            let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
            let wrap_call = if wrap {
                "text_view_wrap(1, true)\n"
            } else {
                ""
            };
            write!(
                f,
                "text_view(1, 0, 0, 200, 100, \"{}\")\n{wrap_call}",
                long.join(" ")
            )
            .unwrap();
            let host = PanelHost::load(f.path()).unwrap();
            let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
            pv.tick(Instant::now(), RECT, CELL);
            let mut prims = Vec::new();
            pv.build_scene(RECT, CELL, &Theme::default(), true, &mut prims);
            let mut ys: Vec<i32> = prims
                .iter()
                .filter_map(|p| match p {
                    Primitive::Text { pos, .. } => Some(pos.1 as i32),
                    _ => None,
                })
                .collect();
            ys.sort_unstable();
            ys.dedup();
            ys.len()
        };
        assert_eq!(rows_for(false), 1, "unwrapped, the line is one row");
        assert!(
            rows_for(true) > 1,
            "wrapped, the line should span several rows"
        );
    }

    #[test]
    fn text_view_region_is_selectable_and_copyable() {
        let (mut pv, _f) = text_view_panel("line one\\nline two\\nline three");
        // The region was declared and hit-tests inside its rect.
        assert_eq!(pv.text_view_at(RECT, 10.0, 10.0), Some(1));
        assert_eq!(pv.text_view_at(RECT, 250.0, 10.0), None);
        // Select-all then read the selection: exactly what Cmd-C would copy.
        pv.region_select_all(1);
        assert_eq!(
            pv.region_view(1).unwrap().selected_text(),
            "line one\nline two\nline three"
        );
    }

    #[test]
    fn identical_redeclare_preserves_selection() {
        let (mut pv, _f) = text_view_panel("alpha\\nbeta\\ngamma");
        pv.region_select_all(1);
        let before = pv.region_view(1).unwrap().selected_text();
        assert!(!before.is_empty());
        // Re-run the frame: the same text is re-declared. The content-hash gate
        // must skip the buffer rebuild so the selection survives (a rebuild
        // clears the anchor and the selection would vanish).
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(pv.region_view(1).unwrap().selected_text(), before);
    }

    #[test]
    fn text_view_renders_as_editor_text_primitives() {
        let (pv, _f) = text_view_panel("one\\ntwo\\nthree");
        let theme = Theme::default();
        let mut prims = Vec::new();
        pv.build_scene(RECT, CELL, &theme, true, &mut prims);
        let text_lines = prims
            .iter()
            .filter(|p| matches!(p, Primitive::Text { .. }))
            .count();
        assert!(
            text_lines >= 3,
            "expected the region's lines rendered as text primitives, got {text_lines}"
        );
    }

    #[test]
    fn draw_text_honors_the_scripts_font_size() {
        // A panel that draws a heading and a caption at different sizes. Garden
        // used to render every panel run at the editor's 14 px, flattening the
        // typographic hierarchy a script asked for.
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            "draw_text(\"Title\", 4, 4, 28, 255, 255, 255)\n\
             draw_text(\"caption\", 4, 40, 10, 200, 200, 200)\n"
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        let theme = Theme::default();
        let mut prims = Vec::new();
        pv.build_scene(RECT, CELL, &theme, true, &mut prims);

        let sizes: Vec<f32> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Text { text, size, .. } if text == "Title" || text == "caption" => {
                    Some(*size)
                }
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![28.0, 10.0], "each run keeps its own size");
    }

    #[test]
    fn a_styled_run_carries_its_weight_and_slant_into_the_scene() {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            "draw_text(\"heading\", {{x: 4, y: 4}}, {{size: 20, weight: 700}})\n\
             draw_text(\"aside\", {{x: 4, y: 30}}, {{size: 12, italic: true}})\n\
             draw_text(\"plain\", {{x: 4, y: 50}}, {{size: 12}})\n"
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        let mut prims = Vec::new();
        pv.build_scene(RECT, CELL, &Theme::default(), true, &mut prims);

        let styles: Vec<(String, TextStyle)> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Text { text, style, .. } => Some((text.clone(), *style)),
                _ => None,
            })
            .collect();
        assert_eq!(
            styles,
            vec![
                (
                    "heading".to_string(),
                    TextStyle {
                        weight: 700,
                        italic: false,
                        spacing: 0.0,
                        font: garden_render::FontRole::Mono,
                    }
                ),
                (
                    "aside".to_string(),
                    TextStyle {
                        weight: 400,
                        italic: true,
                        spacing: 0.0,
                        font: garden_render::FontRole::Mono,
                    }
                ),
                ("plain".to_string(), TextStyle::default()),
            ]
        );
    }

    #[test]
    fn letter_spacing_places_each_glyph_and_matches_the_measurement() {
        // cosmic-text has no letter-spacing, so a spaced run is drawn glyph by
        // glyph with the pen advancing by advance + spacing. That pen has to
        // agree with `text_width` for the same style, or a script centering a
        // spaced label would be off by the accumulated error.
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            "let S = {{size: 20, spacing: 5}}\n\
             draw_text(\"abc\", {{x: 10, y: 4}}, S)\n\
             let width = text_width(\"abc\", S)\n"
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        let mut prims = Vec::new();
        pv.build_scene(RECT, CELL, &Theme::default(), true, &mut prims);

        let runs: Vec<(String, f32)> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Text {
                    text, pos, style, ..
                } => {
                    assert_eq!(
                        style.spacing, 0.0,
                        "the pen carries the spacing, not the run"
                    );
                    Some((text.clone(), pos.0))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            runs.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"],
            "a spaced run is emitted one glyph at a time"
        );
        // Each step is one glyph advance plus the 5px spacing, evenly.
        let step = runs[1].1 - runs[0].1;
        assert_eq!(step, runs[2].1 - runs[1].1, "even steps");
        let measured = debug_val(&pv, "width").expect("the script's own measurement is observable");
        assert_eq!(
            measured as f32,
            step * 3.0,
            "text_width must equal the pen's total travel for the same style"
        );
    }

    #[test]
    fn every_panel_host_measures_with_the_renderers_font() {
        // The app tells each host its real advances; a host that was skipped
        // would fall back to garden-script's uniform 0.6 estimate. The tab is the
        // marker: the measured table gives it a 0 advance (the renderer expands
        // tabs itself and never shapes one), where the estimate would claim 60 px
        // at size 100. Every rebuild path is checked, because the ratios live in
        // the host's env and a rebuilt host starts over with the estimate.
        assert_eq!(
            measured_advance_ratios().get(b'\t' as usize),
            Some(&0.0),
            "the marker only works while the measured tab advance differs from 0.6"
        );
        let src = "let tab = text_width(\"\\t\", 100)\n";
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(f, "{src}").unwrap();

        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        let measured = |pv: &mut PanelView| {
            pv.tick(Instant::now(), RECT, CELL);
            debug_val(pv, "tab")
        };
        assert_eq!(measured(&mut pv), Some(0), "freshly constructed host");

        pv.reload_from_source(src);
        assert_eq!(
            measured(&mut pv),
            Some(0),
            "host rebuilt from pushed source"
        );

        pv.reload_from_path(f.path());
        assert_eq!(measured(&mut pv), Some(0), "host rebuilt from its file");

        pv.restart(Instant::now());
        assert_eq!(measured(&mut pv), Some(0), "host restarted from scratch");
    }

    // The region in `text_view_panel` is 200×100 at CELL (8, 16): 5 visible
    // lines and 23 visible columns. `TALL` overflows it vertically; `SHORT`
    // fits on both axes.
    const TALL: &str = "l1\\nl2\\nl3\\nl4\\nl5\\nl6\\nl7\\nl8\\nl9\\nl10";
    const SHORT: &str = "a\\nb\\nc";

    /// Consume-or-forward for nav keys: a region that overflows consumes every
    /// scroll key — including at the boundary (position unchanged), so keys
    /// never leak into the script mid-gesture — and always forwards non-scroll
    /// keys.
    #[test]
    fn navigation_keys_scroll_an_overflowing_region() {
        let (mut pv, _f) = text_view_panel(TALL);
        // At the very top, upward keys are still consumed (boundary rule).
        assert!(pv.region_scroll_key(1, RECT, CELL, "up"));
        assert!(pv.region_scroll_key(1, RECT, CELL, "home"));
        for key in [
            "down", "j", "up", "k", "pagedown", "pageup", "space", "home", "end",
        ] {
            assert!(
                pv.region_scroll_key(1, RECT, CELL, key),
                "{key} should scroll"
            );
        }
        // At the very bottom, downward keys are still consumed too.
        assert!(pv.region_scroll_key(1, RECT, CELL, "end"));
        assert!(pv.region_scroll_key(1, RECT, CELL, "down"));
        // A non-navigation key is left for the script.
        assert!(!pv.region_scroll_key(1, RECT, CELL, "x"));
    }

    /// A region whose content fits its rect can't act on scroll keys, so every
    /// one of them is forwarded to the script (keyboard navigation must keep
    /// working after a click on selectable text).
    #[test]
    fn navigation_keys_forward_when_the_region_fits() {
        let (mut pv, _f) = text_view_panel(SHORT);
        for key in [
            "down", "j", "up", "k", "pagedown", "pageup", "space", "home", "end", "x",
        ] {
            assert!(
                !pv.region_scroll_key(1, RECT, CELL, key),
                "{key} should forward to the script"
            );
        }
    }

    /// The wheel follows the same rule: consumed by an overflowing region,
    /// forwarded (as the script's `scroll_y()`) when the content fits.
    #[test]
    fn wheel_consumed_only_by_an_overflowing_region() {
        let (mut pv, _f) = text_view_panel(TALL);
        assert!(pv.region_scroll(1, RECT, CELL, 2.0));
        assert!(pv.region_scroll(1, RECT, CELL, -2.0)); // boundary: still consumed
        let (mut pv, _f) = text_view_panel(SHORT);
        assert!(!pv.region_scroll(1, RECT, CELL, 2.0));
    }

    /// Horizontal wheel: consumed only when a line is wider than the viewport.
    #[test]
    fn horizontal_wheel_consumed_only_when_lines_overflow() {
        let (mut pv, _f) = text_view_panel("a line much wider than the 23-column viewport");
        assert!(pv.region_scroll_h(1, RECT, CELL, 4.0));
        let (mut pv, _f) = text_view_panel(SHORT);
        assert!(!pv.region_scroll_h(1, RECT, CELL, 4.0));
    }

    /// A minimal GPP panel-mode client, as a shell script: it answers the
    /// `initialize` handshake, then watches stdin for the host's `emit`
    /// notification. When it sees `("divider", 42)` it replies with a
    /// `setStatus` notification — so the test can observe, purely through the
    /// public `pump_client` API, that a script's `emit(event, arg)` crossed the
    /// pipe with its event *and* arg intact. Substring matches are
    /// order-independent (serde_json may order object keys either way).
    const EMIT_ECHO_CLIENT: &str = r#"
read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"name":"echo","mode":"panel"}}'
while IFS= read -r line; do
  case "$line" in
    *'"method":"emit"'*)
      case "$line" in
        *'"event":"divider"'*)
          case "$line" in
            *'"arg":42'*)
              printf '%s\n' '{"jsonrpc":"2.0","method":"setStatus","params":{"text":"divider=42"}}'
              ;;
          esac ;;
      esac ;;
  esac
done
"#;

    /// End-to-end over a real pipe: a panel script's `emit("divider", 42)` is
    /// drained by [`PanelView::tick`] and written to the attached subprocess as
    /// a GPP `emit` notification; the stub client answers with `setStatus`,
    /// which surfaces back through `pump_client` as a [`ClientEvent`].
    #[test]
    fn script_emit_reaches_the_client_over_the_pipe() {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(f, "emit(\"divider\", 42)\n").unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());

        let client = ProcessPane::spawn(
            "bash",
            &["-c".to_string(), EMIT_ECHO_CLIENT.to_string()],
            1,
            24,
            80,
        )
        .expect("spawn stub client");
        pv.attach_client(client, crate::script_client::new_shared(), "echo".into());

        // One frame: the script emits, tick drains + forwards over the pipe.
        assert!(pv.tick(Instant::now(), RECT, CELL));

        // The stub's `setStatus` answer arrives asynchronously; poll for it.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (events, _) = pv.pump_client(None);
            if events
                .iter()
                .any(|e| matches!(e, ClientEvent::SetStatus(s) if s == "divider=42"))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "emit never reached the client (no setStatus reply)"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A stub panel-mode client that answers the built-in `navigate` mutation:
    /// `b.ptl` resolves to a source, any other screen errors. Echoes the request
    /// `id` so the host correlates the response.
    const NAV_STUB_CLIENT: &str = r#"
read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"name":"nav-stub","mode":"panel"}}'
while IFS= read -r line; do
  # The host serializes params through a serde_json::Value, so wire keys are
  # alphabetically ordered — match on the screen substring, not field order.
  case "$line" in
    *'"screen":"b.ptl"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"name":"navigate","value":{"screen":"b.ptl","source":"state marker = 2"}}}\n' "$id"
      ;;
    *'"method":"mutate"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"name":"navigate","error":"no such screen"}}\n' "$id"
      ;;
  esac
done
"#;

    /// End-to-end over a real pipe (GPP Phase 4): a subprocess panel navigates by
    /// fetching the target screen's source from the client via the built-in
    /// `navigate` **mutation**; the host then drives its own history stack with it,
    /// keeping the client attached across the source swap.
    #[test]
    fn subprocess_navigation_fetches_source_via_mutation() {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(f, "state marker = 1\n").unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "home".into(), Instant::now());
        let client = ProcessPane::spawn(
            "bash",
            &["-c".to_string(), NAV_STUB_CLIENT.to_string()],
            2,
            24,
            80,
        )
        .expect("spawn stub client");
        pv.attach_client(
            client,
            crate::script_client::new_shared(),
            "nav-stub".into(),
        );
        pv.set_origin_source("state marker = 1".into());

        // An undeclared screen surfaces the client's error (no swap).
        let err = pv.client_fetch_screen(
            "missing.ptl",
            &serde_json::Value::Null,
            Duration::from_secs(5),
        );
        assert!(err.unwrap_err().contains("no such screen"));
        assert_eq!(pv.history_len(), 1, "a rejected navigate pushes no entry");

        // A declared screen resolves to its source over the mutation round-trip;
        // driving it through the history stack swaps the screen and keeps the
        // client, and *back* returns to the origin's own (source-backed) home.
        let src = pv
            .client_fetch_screen("b.ptl", &serde_json::Value::Null, Duration::from_secs(5))
            .expect("navigate fetch");
        assert_eq!(src, "state marker = 2");
        pv.nav_push("b.ptl".into(), src, serde_json::Value::Null);
        assert_eq!(pv.current_screen(), "b.ptl");
        assert_eq!(pv.history_len(), 2);
        assert!(
            pv.has_client(),
            "the client stays attached across a source swap"
        );
        assert!(pv.nav_back(), "back moves the cursor");
        assert_eq!(
            pv.current_screen(),
            "home",
            "back returns to the origin screen"
        );
    }

    /// The integer the panel's last good frame bound to `name`.
    fn debug_val(pv: &PanelView, name: &str) -> Option<i64> {
        pv.observed().get(name).and_then(|v| v.as_i64())
    }

    /// A left press that the host already counted as the n-th of a chain must
    /// reach the script as `click_count() == n`. The count was dropped at the
    /// boundary, so a panel could never see a double click.
    #[test]
    fn a_multi_click_press_reaches_the_script_as_its_count() {
        let host = PanelHost::from_source("p", "let cc = click_count()\n").unwrap();
        let mut pv = PanelView::new(host, "p".into(), Instant::now());
        pv.set_mouse(5, 5);
        pv.mouse_down_clicks(0, 2);
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "cc"), Some(2));

        // A single press is still 1 (and the chain does not keep growing from
        // the earlier double click once the pointer has moved away).
        pv.mouse_up(0);
        pv.set_mouse(300, 300);
        pv.mouse_down_clicks(0, 1);
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "cc"), Some(1));
    }

    /// Modifiers are published as held keys as well as `mod_*()` flags:
    /// `key_down("shift")` used to answer false forever, silently.
    #[test]
    fn modifiers_are_also_readable_as_held_keys() {
        let host = PanelHost::from_source(
            "p",
            "let s = if key_down(\"shift\") then 1 else 0 end\n             let a = if key_down(\"alt\") then 1 else 0 end\n",
        )
        .unwrap();
        let mut pv = PanelView::new(host, "p".into(), Instant::now());
        pv.set_modifiers(Mods {
            shift: true,
            alt: true,
            ..Default::default()
        });
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "s"), Some(1));
        assert_eq!(debug_val(&pv, "a"), Some(1));

        // Releasing them takes the keys back down with it.
        pv.set_modifiers(Mods::default());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "s"), Some(0));
        assert_eq!(debug_val(&pv, "a"), Some(0));
    }

    /// The chords a frame claimed are what the host matches keys against —
    /// including the "any modifier" form, and matched case-insensitively.
    #[test]
    fn claimed_chords_are_matched_by_key_and_modifier_bits() {
        let host =
            PanelHost::from_source("p", "claim_key(\"z\", \"cmd\")\nclaim_key(\"escape\")\n")
                .unwrap();
        let mut pv = PanelView::new(host, "p".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        assert!(pv.claims_key("z", 8), "Cmd+Z was claimed");
        assert!(pv.claims_key("Z", 8), "key names match case-insensitively");
        assert!(!pv.claims_key("z", 0), "a bare z was not claimed");
        assert!(!pv.claims_key("y", 8), "another chord was not claimed");
        // No modifier argument claims the key under every chord.
        assert!(pv.claims_key("escape", 0));
        assert!(pv.claims_key("escape", 8));
    }

    /// `text_view_scroll_to` moves the region's viewport on the frame it is
    /// emitted, and only then: a later frame that emits no jump leaves the
    /// user's own scrolling alone.
    #[test]
    fn text_view_scroll_to_jumps_only_on_the_frame_it_is_emitted() {
        let body: String = (0..60).map(|i| format!("line {i}\n")).collect();
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            "state n = 0\nn = n + 1\ntext_view(1, 0, 0, 200, 100, \"{}\")\n\
             if n == 1 then text_view_scroll_to(1, 30) end\n",
            body.trim_end().replace('\n', "\\n")
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());

        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(pv.region_view(1).map(|v| v.scroll.top), Some(30));

        // The user scrolls away; the next frame (which emits no jump) leaves it.
        pv.region_scroll(1, RECT, CELL, -5.0);
        let after_user = pv.region_view(1).map(|v| v.scroll.top);
        assert_eq!(after_user, Some(25));
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(pv.region_view(1).map(|v| v.scroll.top), after_user);
    }

    /// A jump lands on its line even when there is less than a screenful below
    /// it — the case that made a diff's file column look broken, since the last
    /// few files could never reach the top row. The region shows blank space
    /// under the last line rather than sliding the target down the viewport.
    #[test]
    fn text_view_scroll_to_anchors_a_target_near_the_end() {
        let body: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            "state n = 0\nn = n + 1\ntext_view(1, 0, 0, 200, 100, \"{}\")\n\
             if n == 1 then text_view_scroll_to(1, 18) end\n",
            body.trim_end().replace('\n', "\\n")
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());

        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(
            pv.region_view(1).map(|v| v.scroll.top),
            Some(18),
            "the asked-for line is the top one, blank rows below and all"
        );
        // Idle ticks re-sync the same command list; the anchor must survive them.
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(pv.region_view(1).map(|v| v.scroll.top), Some(18));
    }

    #[test]
    fn reload_from_editor_applies_buffer_text_and_gates_on_change() {
        // A panel that binds a "size", plus a live `state` counter — both
        // observable by name without the script doing anything about it.
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(f, "state n = 0\nn = n + 1\nlet size = 1\n").unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "size"), Some(1));
        assert_eq!(debug_val(&pv, "n"), Some(1));

        // Applying new source recompiles and reports a change...
        let new_src = "state n = 0\nn = n + 1\nlet size = 5\n";
        assert!(pv.reload_from_editor(new_src, Instant::now()));
        // ...and re-applying the SAME source is a no-op (hash gate).
        assert!(!pv.reload_from_editor(new_src, Instant::now()));

        pv.tick(Instant::now(), RECT, CELL);
        // The new program renders (size = 5) and Petal `state` survived (n → 2).
        assert_eq!(debug_val(&pv, "size"), Some(5));
        assert_eq!(debug_val(&pv, "n"), Some(2));
    }

    #[test]
    fn reload_from_editor_keeps_last_render_on_compile_error() {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(f, "let size = 5\n").unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "size"), Some(5));

        // A broken buffer sets the error banner but leaves the program running,
        // so the last good frame's observed values still read 5.
        assert!(pv.reload_from_editor("let size = (oops\n", Instant::now()));
        assert!(
            pv.error().is_some(),
            "a compile error should raise the banner"
        );
        // The banner must PERSIST across the running program's successful frames
        // (a frame of the old, still-valid program must not clear a reload error).
        for _ in 0..3 {
            pv.tick(Instant::now(), RECT, CELL);
            assert!(
                pv.error().is_some(),
                "error banner should stay up while the buffer is broken"
            );
            assert_eq!(debug_val(&pv, "size"), Some(5));
        }
        // Fixing the buffer clears the banner and swaps the new program in.
        assert!(pv.reload_from_editor("let size = 9\n", Instant::now()));
        assert!(pv.error().is_none());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "size"), Some(9));
    }

    /// Push/back/forward across two screens, each with its own `state n`, proving
    /// per-history-entry state save/restore (true browser semantics: *back*
    /// restores the value this screen held when it was left) and that a rebuilt
    /// host's restored state survives `state n = 0` re-init on the next frame.
    #[test]
    fn history_push_back_forward_round_trips_per_entry_state() {
        const A: &str = "state n = 0\nn = n + 1\nlet screen = 1\n";
        const B: &str = "state n = 0\nn = n + 10\nlet screen = 2\n";
        let host = PanelHost::from_source("a", A).unwrap();
        let mut pv = PanelView::new(host, "a".into(), Instant::now());
        assert_eq!(pv.history_len(), 1);
        assert_eq!(pv.history_cursor(), 0);
        assert_eq!(pv.current_screen(), "a");

        // Advance screen A to n = 3.
        for _ in 0..3 {
            pv.tick(Instant::now(), RECT, CELL);
        }
        assert_eq!(debug_val(&pv, "n"), Some(3));

        // Push screen B: a new entry, cursor advances, host swaps to B.
        pv.nav_push("b".into(), B.into(), serde_json::Value::Null);
        assert_eq!(pv.history_len(), 2);
        assert_eq!(pv.history_cursor(), 1);
        assert_eq!(pv.current_screen(), "b");

        // Screen B runs its own fresh state.
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "screen"), Some(2));
        assert_eq!(debug_val(&pv, "n"), Some(10));

        // Back restores screen A's captured n = 3 (per-entry state round-trip).
        assert!(pv.nav_back());
        assert_eq!(pv.history_cursor(), 0);
        assert_eq!(pv.current_screen(), "a");
        assert_eq!(pv.state_json().get("n").and_then(|v| v.as_i64()), Some(3));

        // Forward restores screen B's captured n = 10, rebuilding the host; the
        // next frame must see the restored value survive `state n = 0` re-init
        // (the restore-before-first-frame ordering trap).
        assert!(pv.nav_forward());
        assert_eq!(pv.history_cursor(), 1);
        assert_eq!(pv.current_screen(), "b");
        assert_eq!(pv.state_json().get("n").and_then(|v| v.as_i64()), Some(10));
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "screen"), Some(2));
        assert_eq!(debug_val(&pv, "n"), Some(20));
    }

    /// Back past the start of history and forward past its end are no-ops (they
    /// return false and leave the cursor unmoved).
    #[test]
    fn history_back_past_start_and_forward_past_end_are_no_ops() {
        let host = PanelHost::from_source("a", "state n = 0\n").unwrap();
        let mut pv = PanelView::new(host, "a".into(), Instant::now());
        // At the seed with no forward entries, both directions are no-ops.
        assert!(!pv.nav_back());
        assert!(!pv.nav_forward());
        assert_eq!(pv.history_len(), 1);
        assert_eq!(pv.history_cursor(), 0);

        pv.nav_push("b".into(), "state n = 0\n".into(), serde_json::Value::Null);
        assert_eq!(pv.history_cursor(), 1);
        // At the end of history: forward is a no-op, back works, then back again
        // (now at the start) is a no-op.
        assert!(!pv.nav_forward());
        assert!(pv.nav_back());
        assert_eq!(pv.history_cursor(), 0);
        assert!(!pv.nav_back());
    }

    /// A `navigate(screen, arg)` argument belongs to the *history entry*, not to
    /// the host — so a detail screen returned to by *back* comes back showing the
    /// same subject it was opened with, and *forward* onto a different entry
    /// shows that entry's own subject. A host-level slot would have collapsed all
    /// three of these onto whichever navigation happened last.
    #[test]
    fn a_navigation_argument_survives_back_and_forward() {
        // Each screen simply reports the id it was navigated with (nil at home).
        const SCREEN: &str = "let subject = nav_arg() ?? {id: 0}\nlet id = subject.id\n";
        let host = PanelHost::from_source("home", SCREEN).unwrap();
        let mut pv = PanelView::new(host, "home".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(
            debug_val(&pv, "id"),
            Some(0),
            "home was navigated to by nobody"
        );

        pv.nav_push("detail".into(), SCREEN.into(), serde_json::json!({"id": 7}));
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "id"), Some(7));

        pv.nav_push("detail".into(), SCREEN.into(), serde_json::json!({"id": 9}));
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "id"), Some(9));

        // Back onto the first detail visit: its own argument, not the latest one.
        assert!(pv.nav_back());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(
            debug_val(&pv, "id"),
            Some(7),
            "back restored the entry's argument"
        );

        // Back to home, which never had one.
        assert!(pv.nav_back());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "id"), Some(0));

        // And forward walks the arguments again in order.
        assert!(pv.nav_forward());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "id"), Some(7));
        assert!(pv.nav_forward());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "id"), Some(9));
    }

    /// `nav_replace` swaps the current screen in place without growing history.
    #[test]
    fn history_replace_keeps_length() {
        let host = PanelHost::from_source("a", "state n = 0\n").unwrap();
        let mut pv = PanelView::new(host, "a".into(), Instant::now());
        pv.nav_replace("b".into(), "state n = 0\n".into(), serde_json::Value::Null);
        assert_eq!(pv.history_len(), 1);
        assert_eq!(pv.history_cursor(), 0);
        assert_eq!(pv.current_screen(), "b");
    }

    /// Returning to the origin via *back* must rebuild the **home program**, not
    /// merely restore home's state onto whatever screen is currently running. The
    /// seed rebuilds from its real file path, so `screen` (the program's own
    /// identity) reverts to home's while home's captured `n` is restored. Guards
    /// the seed-entry `source: None` latent bug where back kept the departed
    /// screen's program.
    #[test]
    fn back_to_origin_rebuilds_the_home_program_not_just_state() {
        // Home is file-backed, like a real `panel(...)` layout node.
        let mut home = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(home, "state n = 0\nn = n + 1\nlet screen = 1\n").unwrap();
        let host = PanelHost::load(home.path()).unwrap();
        let mut pv = PanelView::new(host, "home.ptl".into(), Instant::now());

        // Run home to n = 2 (two frames of n = n + 1).
        pv.tick(Instant::now(), RECT, CELL);
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(debug_val(&pv, "screen"), Some(1));
        assert_eq!(debug_val(&pv, "n"), Some(2));

        // Navigate to a DIFFERENT program (screen 2).
        const DETAIL: &str = "state n = 0\nn = n + 100\nlet screen = 2\n";
        pv.nav_push("detail.ptl".into(), DETAIL.into(), serde_json::Value::Null);
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(
            debug_val(&pv, "screen"),
            Some(2),
            "detail program should run"
        );

        // Back home: the HOME program must run again (screen 1) — a rebuild, not
        // just a state restore onto the detail program. Home's n (2) is restored,
        // then this frame's n = n + 1 makes it 3.
        assert!(pv.nav_back());
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(
            debug_val(&pv, "screen"),
            Some(1),
            "back to origin must rebuild the home program, not keep detail's"
        );
        assert_eq!(debug_val(&pv, "n"), Some(3));
    }

    /// `is_navigated` is false at the origin and true after a push, so the live
    /// editor→panel binding can skip a navigated panel.
    #[test]
    fn is_navigated_tracks_the_cursor() {
        let host = PanelHost::from_source("a", "state n = 0\n").unwrap();
        let mut pv = PanelView::new(host, "a".into(), Instant::now());
        assert!(!pv.is_navigated());
        pv.nav_push("b".into(), "state n = 0\n".into(), serde_json::Value::Null);
        assert!(pv.is_navigated());
        assert!(pv.nav_back());
        assert!(!pv.is_navigated());
    }

    /// A script calling `navigate(...)` surfaces a `ClientEvent::Navigate` out of
    /// `tick`, drainable once via `take_nav_events`.
    #[test]
    fn navigate_native_surfaces_client_event() {
        let host = PanelHost::from_source("a", "navigate(\"detail\")\n").unwrap();
        let mut pv = PanelView::new(host, "a".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        let events = pv.take_nav_events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                ClientEvent::Navigate(garden_script::NavIntent::Push(s, _)) if s == "detail"
            )),
            "expected a Navigate(Push(\"detail\")) event"
        );
        // Drained: a second take is empty.
        assert!(pv.take_nav_events().is_empty());
    }

    #[test]
    fn undeclared_region_is_dropped_and_unfocuses() {
        // First frame declares region 1; focus it.
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            "if frame_count() == 0 then text_view(1, 0, 0, 200, 100, \"hi\") end\n"
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        pv.region_press(1, RECT, CELL, 5.0, 5.0, false, 1);
        assert_eq!(pv.focused_region(), Some(1));
        // Next frame declares no region: it is dropped and focus is released.
        pv.tick(Instant::now(), RECT, CELL);
        assert_eq!(pv.text_view_at(RECT, 5.0, 5.0), None);
        assert_eq!(pv.focused_region(), None);
    }

    // ── error card ──────────────────────────────────────────────────────────

    /// The text of every `Text` primitive, in draw order.
    fn text_rows(prims: &[Primitive]) -> Vec<String> {
        prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn wrap_words_breaks_on_spaces_and_respects_width() {
        let out = wrap_words("the quick brown fox jumps", 9);
        assert!(out.iter().all(|l| l.chars().count() <= 9), "{out:?}");
        // Reassembling the words round-trips the content.
        assert_eq!(
            out.join(" ").split_whitespace().collect::<Vec<_>>(),
            vec!["the", "quick", "brown", "fox", "jumps"]
        );
    }

    #[test]
    fn wrap_words_hard_breaks_an_overlong_token() {
        // A single word longer than the width is split across rows, not dropped.
        let out = wrap_words("^^^^^^^^^^^^^^^^^^^^", 6);
        assert!(out.len() > 1, "{out:?}");
        assert!(out.iter().all(|l| l.chars().count() <= 6), "{out:?}");
        assert_eq!(out.concat(), "^^^^^^^^^^^^^^^^^^^^");
    }

    #[test]
    fn truncate_chars_marks_a_cut_with_an_ellipsis() {
        assert_eq!(truncate_chars("short", 10), "short");
        let t = truncate_chars("a very long line of code", 8);
        assert_eq!(t.chars().count(), 8);
        assert!(t.ends_with('…'));
    }

    /// A multi-line petal error wraps within the pane: no drawn text row exceeds
    /// the pane's column budget, and the source snippet survives (truncated).
    #[test]
    fn error_card_wraps_long_message_within_pane() {
        let err = "Cannot get length of nil [line 243, column 46]\n243 |\n\
                   243 | if len(pr_err) > 0 || len(detail_err) > 0 || len(pr.error) > 0 then has_error = 1 end\n\
                       |                                              ^^^^^^^^^^^^";
        let mut prims = Vec::new();
        build_error_card(err, RECT, CELL, &mut prims);
        let rows = text_rows(&prims);

        // Every row fits the pane width (RECT.w / CELL.0 = ~37 cols, plus padding).
        let cols = ((RECT.w - 16.0) / CELL.0) as usize;
        for r in &rows {
            assert!(
                r.chars().count() <= cols,
                "row too wide ({}): {r:?}",
                r.chars().count()
            );
        }
        // The headline is present and wrapped (more than one row total).
        assert!(rows[0].starts_with("⚠ panel error:"), "{rows:?}");
        assert!(
            rows.len() > 1,
            "a long error should wrap to several rows: {rows:?}"
        );
        // The offending source line is still shown (its line number survives).
        assert!(
            rows.iter().any(|r| r.contains("243")),
            "snippet dropped: {rows:?}"
        );
    }

    /// The card is opaque enough to cover the frame beneath and never taller than
    /// the pane, even when the error has many snippet lines.
    #[test]
    fn error_card_is_opaque_and_bounded_by_the_pane() {
        let err = (0..40)
            .map(|i| format!("line {i} of a very tall error trace"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut prims = Vec::new();
        build_error_card(&err, RECT, CELL, &mut prims);

        // The card mesh is (near) opaque so the last good frame doesn't bleed.
        let has_opaque_mesh = prims.iter().any(|p| {
            matches!(
                p,
                Primitive::Mesh { vertices, .. } if vertices.iter().any(|v| v.color.a >= 0.9)
            )
        });
        assert!(
            has_opaque_mesh,
            "error card should have an opaque backing mesh"
        );

        // No drawn text starts below the pane bottom (the card is bounded).
        for p in &prims {
            if let Primitive::Text { pos, .. } = p {
                assert!(
                    pos.1 <= RECT.y + RECT.h,
                    "text drawn past pane bottom: {pos:?}"
                );
            }
        }
    }

    /// Every text run a panel emits while a `clip(...)` is active carries that
    /// clip, so the renderer cuts a run that straddles the region's bottom edge
    /// instead of drawing it whole. A drawer must not have to cull the half row
    /// itself — that is exactly what a scroll viewport wants to show.
    #[test]
    fn draw_text_straddling_the_clip_is_cut_not_dropped() {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(
            f,
            "clip(0, 0, 300, 40)\n\
             draw_text(\"inside\", 0, 4, 16, 255, 255, 255)\n\
             draw_text(\"straddle\", 0, 32, 16, 255, 255, 255)\n\
             draw_text(\"below\", 0, 60, 16, 255, 255, 255)\n\
             clip_none()\n\
             draw_text(\"unclipped\", 0, 120, 16, 255, 255, 255)\n"
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        let mut prims = Vec::new();
        pv.build_scene(RECT, CELL, &Theme::default(), true, &mut prims);

        let run = |want: &str| -> Rect {
            prims
                .iter()
                .find_map(|p| match p {
                    Primitive::Text { text, clip, .. } if text == want => Some(*clip),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no text run {want:?}"))
        };
        // The straddling run is still emitted — cutting is the renderer's job,
        // via the clip the run carries.
        for name in ["inside", "straddle", "below"] {
            let clip = run(name);
            assert_eq!(
                (clip.y, clip.y + clip.h),
                (0.0, 40.0),
                "{name} must carry the active clip"
            );
        }
        let clip = run("unclipped");
        assert_eq!(clip.h, RECT.h, "clip_none() restores the pane rect");
    }

    /// A `text_view` region's text is drawn by the embedded editor, but it is
    /// still "drawn while the clip is active" — so it clips like everything
    /// else. Before this, only the region's tessellated chrome clipped and its
    /// glyphs spilled past the viewport.
    #[test]
    fn a_text_view_region_clips_its_text_to_the_active_clip() {
        let mut f = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        let lines = (1..=20).map(|i| format!("line {i}")).collect::<Vec<_>>();
        write!(
            f,
            "clip(0, 0, 300, 40)\n\
             text_view(1, 0, 0, 200, 300, \"{}\")\n",
            lines.join("\\n")
        )
        .unwrap();
        let host = PanelHost::load(f.path()).unwrap();
        let mut pv = PanelView::new(host, "test.ptl".into(), Instant::now());
        pv.tick(Instant::now(), RECT, CELL);
        let mut prims = Vec::new();
        pv.build_scene(RECT, CELL, &Theme::default(), true, &mut prims);

        let clipped: Vec<Rect> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Text { clip, .. } => Some(*clip),
                _ => None,
            })
            .collect();
        assert!(!clipped.is_empty(), "the region should emit text runs");
        for clip in clipped {
            assert!(
                clip.y >= 0.0 && clip.y + clip.h <= 40.0 + f32::EPSILON,
                "region text escaped the panel clip: {clip:?}"
            );
        }
    }
}

/// Close the pending geometry batch so the next primitive lands *after* it.
///
/// Panel shapes accumulate into one [`Primitive::Mesh`] to keep the draw-call
/// count down, but the renderer composites primitives in list order. Anything
/// that is not geometry — a text run, an image — therefore has to flush the
/// batch first, or it would be drawn underneath shapes that were submitted
/// before it. A no-op when nothing is queued.
/// Narrow a primitive's own clip to `to`.
///
/// Primitives that arrive already clipped — everything an embedded `text_view`
/// region renders — still have to obey whatever `clip(...)` was active when the
/// region was declared. Intersecting (rather than overwriting) keeps both: the
/// region's interior *and* the panel's viewport. A [`Primitive::Quad`] carries
/// no clip at all; panel code tessellates those into the mesh, which is
/// scissored when it is flushed.
fn clip_to(p: Primitive, to: Rect) -> Primitive {
    match p {
        Primitive::Text {
            pos,
            text,
            color,
            clip,
            size,
            style,
        } => Primitive::Text {
            pos,
            text,
            color,
            clip: intersect(clip, to),
            size,
            style,
        },
        Primitive::Mesh { vertices, clip } => Primitive::Mesh {
            vertices,
            clip: intersect(clip, to),
        },
        Primitive::Image {
            rect,
            source,
            alpha,
            clip,
        } => Primitive::Image {
            rect,
            source,
            alpha,
            clip: intersect(clip, to),
        },
        Primitive::Quad { .. } => p,
    }
}

fn flush_mesh(prims: &mut Vec<Primitive>, verts: &mut Vec<Vertex>, clip: Rect) {
    if !verts.is_empty() {
        prims.push(Primitive::Mesh {
            vertices: std::mem::take(verts),
            clip,
        });
    }
}
