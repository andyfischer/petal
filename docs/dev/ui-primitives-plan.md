# Plan: `petal-ui` — standard interactivity primitives for embedders

Status: Phases 1–2 **landed** (2026-07-01) — the `petal-ui` crate exists with
Layers 0–2, the `ui` prelude, and the headless harness, and petal-sdl runs on
it. Phase 3 (Garden adoption) is next.
Related: [../module-system.md](../module-system.md) (the import system that
carries the Petal-source half of this library — landed; the prelude ships as
a real module via `register_prelude`)

## Problem

Every Petal embedder that renders interactive graphics re-implements the same
two things by hand:

1. **The host contract.** petal-sdl (`apps/petal-sdl/src/native_fns.rs`,
   `input.rs`) and Garden (`~/garden/garden-script/src/panel.rs`,
   `~/garden/garden-app/src/panel_view.rs`) each independently register
   near-identical native fns (`mouse_x/y`, `mouse_pressed`, `key_pressed`,
   `dt`, `frame_count`, `draw_rect`, `draw_text`, `clip`, …) and each
   hand-implement the same frame loop: *bind input uniforms → `env.run(stack)`
   → drain a tagged `draw_commands` output buffer*. The names agree only
   because Garden copied petal-sdl's pattern; nothing in Petal defines or
   enforces the vocabulary, key-name spellings, or edge-vs-level semantics.

2. **Everything above the raw primitives.** Interactive scripts hand-roll the
   same widget logic over and over. Garden's diff viewer
   (`~/garden/garden-app/assets/diff_detail_panel.ptl`) reimplements rect
   hit-testing for list selection, j/k/arrow keyboard nav with manual
   clamping, keep-selection-visible scroll-follow, wheel/PageUp scroll regions,
   `…`-prefix text truncation, a hardcoded `CW = 8.4` monospace advance, and
   `rect`/`text` shims that adapt `{r,g,b}` records to the flat-int natives —
   and nearly all of it is copy-pasted verbatim into its sibling
   `diff_panel.ptl`, because two Petal scripts cannot share code. petal-sdl's
   `examples/browser.ptl` and `examples/paint.ptl` hand-roll the same idioms
   again.

Both hosts also document the same input gaps: no key hold/release distinction
(`key_down` currently mirrors `key_pressed` in Garden), no drag gestures, no
right/middle button or modifier-aware clicks, no per-widget focus, no text
input (see `~/garden/docs/petal-graphical-panels.md` "Not yet wired").

## Proposal

Ship a standard interactivity layer **with Petal**, as a new `petal-ui` crate
in this workspace, in three layers. Each layer is useful alone and each maps
to a delivery phase.

### Layer 0 — standard input contract (Rust)

Promote petal-sdl's `InputState` into `petal-ui` as the canonical
implementation every embedder uses:

- **A normalized event enum** the host feeds:
  `MouseMove / MouseDown / MouseUp / Scroll / KeyDown / KeyUp / Text(char)`,
  carrying button ids and modifier state. `InputState::begin_frame()` derives
  the level/edge split — `*_down` (held) vs `*_pressed` (edge, this frame
  only) vs the currently-missing `*_released`.
- **One canonical key-name table** (`"j"`, `"down"`, `"pageup"`, `"return"`,
  …), so hosts stop maintaining private copies that can drift.
- **Standard native fns**, registered with one call —
  `petal_ui::register_input(&mut env)`. Today's set plus the documented gaps:

  | fn | notes |
  |---|---|
  | `mouse_x() / mouse_y()` | pane/window-local logical px (level) |
  | `mouse_down(b) / mouse_pressed(b) / mouse_released(b)` | b: 0=left 1=right 2=middle |
  | `scroll_y()` (and `scroll_x()`) | wheel lines this frame (edge) |
  | `key_down(name)` | true hold tracking (new) |
  | `key_pressed(name) / key_released(name)` | edges; canonical names |
  | `mod_shift() / mod_ctrl() / mod_alt() / mod_cmd()` | modifier levels |
  | `drag_active() / drag_start_x() / drag_start_y()` | left-drag gesture (new) |
  | `click_count()` | 1/2/3 for double/triple-click (new) |
  | `dt() / frame_count()` | timing |
  | `screen_width() / screen_height()` | drawable size |
  | `ui_version()` | contract version for compat checks |

- **`petal_ui::bind_input(&mut env, &input)`** — the per-frame uniform
  binding both hosts currently hand-write (uses the existing
  `Env::set_binding` / `PetalCxt::binding` data plane; natives stay bare fn
  pointers reading bindings, exactly the pattern that works today).

The standard owns **semantics** (what `key_pressed` means, what a drag is);
the host keeps **policy** (which keys are reserved for the host, when a script
ticks, focus routing between panes). Garden's `classify_panel_key` and
edge-buffering in `panel_view.rs` collapse into calls to this module; its
reserved-chord policy stays in Garden.

### Layer 1 — Petal-source prelude of interaction primitives

A set of `.ptl` files in `petal-ui/prelude/`, embedded via `include_str!` and
exposed as `petal_ui::prelude_source()`. The module system has landed
(../module-system.md), so delivery is real modules from day one:
`env.register_module("ui", petal_ui::prelude_source())` +
`env.set_implicit_imports(&["ui"])` — user scripts call `button(...)` with
zero ceremony, with per-file error attribution and no source concatenation.

Contents — everything the diff viewer and browser.ptl hand-roll, written once
in pure Petal on top of Layer 0:

```petal
// geometry + hit testing (rects are {x, y, w, h} records)
fn point_in(px, py, rect) ... end
fn hovered(rect)  point_in(mouse_x(), mouse_y(), rect) end
fn clicked(rect)  hovered(rect) && mouse_pressed(0) end

// immediate-mode button: draws, returns true on click this frame
fn button(rect, label, style) -> bool

// list widget: keyboard nav + click-to-select + scroll-follow in one call.
// lst is a state record: { selected, scroll, ... }
fn list_update(lst, item_count, visible_rows, list_rect) -> lst
  // j/k/up/down/home/end/pageup/pagedown, click hit-test on rows,
  // wheel scroll with clamping, ensure-selected-visible
end

// scroll region with clamping
fn scroll_update(sc, content_h, view_h) -> sc

// text helpers
fn truncate_tail(s, max_chars) -> string   // "…" prefix when clipped

// style: a default palette record + helpers, overridable per script
```

Retained widget state (selection, scroll offsets) lives in `state` records, so
it survives hot reload for free via `transfer_state`.

The module system handles error attribution (module-file-named spans) and
delivery for free; what remains is **naming discipline**: the prelude claims
short names (`button`, `clicked`, `list_update`) as *implicit* imports, which
are weak like builtins — a script's own declarations shadow them silently,
and `import ui` / `import ui: button` are available when a script wants to be
explicit. `_`-prefixed helpers stay module-private.

**Testing:** `petal-ui` ships a headless harness — a Rust helper that runs a
script in a bare `Env` with synthetic input bindings and asserts on state /
output buffers. Widget logic gets unit tests in this repo with no renderer
attached (mirrors Garden's pure-core testing ethos and its
`debug_state`-driven integration tests).

### Layer 2 — shared draw-command vocabulary + missing draw primitives

The tagged `draw_commands` protocol is duplicated today (encode in each host's
natives, decode in `apps/petal-sdl/src/commands.rs` and Garden's
`garden-script/src/panel.rs`). Promote into `petal_ui::draw`:

- The `DrawCommand` enum (`Clear`, `Rect`, `RectOutline`, `Line`, `Circle`,
  `Triangle`, `Poly`, `Text`, `Clip`, `ClipNone`), the draw natives, and the
  decoder: `register_draw(&mut env)` +
  `drain_draw_commands(&mut env) -> Vec<DrawCommand>`. Hosts implement only
  rasterization.
- **Extensible by design**: hosts can register extra tagged commands alongside
  the standard set (Garden may want host-specific commands; petal-sdl has
  canvas ops the standard doesn't mandate). The standard vocabulary is a
  default, not a ceiling.
- Fix the gaps both codebases show while we're here:
  - `text_width(s, size)` — host-provided metrics via a registration hook
    (the one place the contract calls back into the host). Kills `CW = 8.4`.
  - Record overloads via Petal's arity/shape overloading:
    `draw_rect(rect, color)` accepting `{x,y,w,h}` / `{r,g,b}` records, ending
    the per-script `fn rect(...)` shims. (Overload defined in the prelude,
    delegating to the flat-int natives — no ABI change.)

### Packaging

- New crate **`petal-ui`** at `~/petal/petal-ui`, depending on `petal`
  (`path = "../rust"`, the same shape as petal-sdl). The core language crate
  stays UI-agnostic; embedders opt in with one dependency line.
- `petal-ui/prelude/*.ptl` in-tree, embedded with `include_str!` and
  registered via `petal_ui::register_prelude(&mut env)` (module `ui` +
  implicit import), versioned together with the input contract. `UI_VERSION`
  constant + `ui_version()` native.
- Scope decision: the library is **immediate-mode**. The `state`-record
  pattern already covers retention; a reducer/component framework (à la
  `examples/reactive_ui.ptl`) can layer on later without changing this
  contract.

## Delivery phases

1. **Phase 1 (this repo):** create `petal-ui` with Layer 0; port petal-sdl
   onto it. Same-repo proof that the contract is complete — petal-sdl deletes
   its `input.rs` edge logic and the input half of `native_fns.rs`.
2. **Phase 2 (this repo):** Layer 1 prelude + headless test harness. Rewrite
   `browser.ptl`'s menu on `list_update`/`button` as the dogfood.
3. **Phase 3 (Garden):** `PanelHost` switches to `petal_ui::register_input` +
   `register_draw` + `register_prelude` (implicit `ui` import), keeping
   Garden-specific extras (`debug_state`, reserved-key policy). Rewrite `diff_panel.ptl` and
   `diff_detail_panel.ptl` on the prelude. Acceptance: the copy-pasted halves
   of the two scripts disappear and Garden's
   `scripts/panel-integration-test.sh` still passes.
4. **Phase 4 (later):** per-widget focus helpers and text-input/IME once
   Layer 0 carries text events. (The module system has landed —
   ../module-system.md — so the prelude ships as real modules from day one.)

## Non-goals

- A retained-mode / component framework (future layer, not this).
- Mandating the draw vocabulary — hosts may extend or replace it.
- Anti-aliasing, canvases, layering fixes in any host's renderer — host
  concerns, tracked separately (see Garden's `docs/petal-graphical-panels.md`).
