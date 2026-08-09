# Writing GPP apps

A **GPP app** is a subprocess that drives the content of a Garden pane over a
small JSON-RPC protocol on stdio (the Garden Pane Protocol). This guide is a
practical, build-it walkthrough for the **panel-mode** ("script-push") style of
app — the current, richer model, where your app pushes a **Petal UI script** the
host runs in-process and drives with data over the pipe.

A panel-mode app is built on the **`petal-query`** crate (`../petal-query`):
you declare a handler per `query(kind, arg)` — each returning a `Reply` with the
value (or error) and **how cacheable it is** (`CachePolicy`) — and `petal-query`
runs the whole protocol loop for you. You do **not** hand-roll the stdio
handshake or the `QueryResult` plumbing.

Read `docs/gpp.md` for the wire reference, `../petal-query/README.md` for
the provider API, and `docs/petal-graphical-panels.md` for the panel draw/input
vocabulary. The complete worked example this guide tracks is
`gpp-apps/git-viewers` — the in-use app behind `:Git`. Its `git-log` bin keeps the
git plumbing in `src/lib.rs`, declares its query handlers with a
`petal_query::App`, and `include_str!`s a colocated production drawer
(`src/git_panel.ptl`). Copy it to start. `gpp-apps/session-retro` is the reference
for a **stateful** provider (in-memory caches + `on_emit` persistence);
`gpp-apps/garden-diff` for both an **editable** one (`edit_view` regions carrying
editable projections, written back with `mutate("apply", …)` — the only GPP app
that is not read-only; see [Editable panels](#editable-panels-mutate-write-back)
below) and a
**network-backed** one (the `gh` CLI, in its PR mode).

## The mental model

Think **web browser + web server**:

- The **host** (garden) is the browser. It runs your UI script every frame,
  handles all interaction locally (scroll, hover, selection, drag), and renders.
- Your **app** is the server. It ships a "page" (a Petal script) once, then
  answers **data requests** the page makes (`query(kind, arg)`), by doing
  whatever a subprocess can do — shell out to `git`/`gh`, read files, hit a
  network service.

The win over the old "push text lines every frame" model: the interaction loop
runs host-side with **zero pipe traffic**, so scrolling a diff or moving a
selection is instant, and only real data crosses the wire. The same Petal script
that a built-in panel runs (`:Git`) can run in your app — the only difference is
whether the data provider is local Rust or your subprocess.

### Panel mode vs. Lines mode

GPP has two client modes (`gpp::ClientMode`):

- **Lines** (the original, still the default): you push full-screen **text**
  (`render` with `lines`/`styles`/`backgrounds`) every frame and subscribe to a
  few keys. Good for simple, list-shaped, text-only views. See the "Lines mode"
  parts of `docs/gpp.md`.
- **Panel** (this guide): you push a **Petal UI script** and answer `query`
  requests. Choose this for anything rich — 2-D layout, draggable dividers,
  selectable/copyable text, master-detail, spinners, live data.

Pick Panel mode when you want graphical UI or on-demand data; pick Lines mode for
a plain scrollable/colored list where a per-frame text buffer is enough.

## Anatomy of a panel-mode app

A panel-mode app is a normal Rust binary in the workspace with two parts:

1. **A Petal UI script** (a colocated `src/*.ptl`, embedded with `include_str!`) —
   the "page": it draws, handles input, and calls `query(kind, arg)` for its data.
2. **A stdio loop** (`src/main.rs`, or `src/bin/<name>.rs` for a multi-bin crate)
   — handshake, push the script, then answer the `query` requests that come back,
   plus `shutdown`.

`git-viewers` factors the common loop into a `run(name, script, make_handler)`
helper in `src/lib.rs` and declares two thin bins (`[[bin]]`) that each pass their
own drawer + query handler — the pattern for shipping several related views from
one crate. A single-view app can put the loop straight in `src/main.rs`.

Your app **never draws** and **never handles keys** itself; the host does, by
running your script. Your app only ships the script and answers data requests.
Crucially, your app's *logic* can be in any language-agnostic style (it's a
JSON-over-pipe data server) — only the *UI* is Petal.

## Step 1 — the crate

Add a workspace member depending on `petal-query` (+ `serde_json`). A single-bin
skeleton:

```toml
[package]
name = "my-app"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
petal-query = { workspace = true }
serde_json = "1"
```

(An app depends on `petal-query`, not `gpp` — the query/panel wire it needs lives
in `petal_query::wire`, so it links no Garden crate.)

For several views from one crate, follow `gpp-apps/git-viewers/Cargo.toml`: a
shared `[lib]` plus one `[[bin]]` per view —

```toml
[lib]
name = "git_viewers"
path = "src/lib.rs"

[[bin]]
name = "git-log"
path = "src/bin/git-log.rs"

```

Add `"gpp-apps/my-app"` to the workspace `members` in the root `Cargo.toml`. GPP
client binaries deliberately are **not** dependencies of `garden-app`, so build
the whole workspace (`cargo build`) to produce your binary beside `garden`.

## Step 2 — declare the app

`petal_query::App` is the whole loop: it does the handshake, pushes your script,
and dispatches every `query`/`emit` to the handler you registered. You write only
the handlers. A single-bin app puts this in `src/main.rs`, `include_str!`'ing the
drawer from a sibling `.ptl`:

```rust
use std::path::PathBuf;
use std::time::Duration;
use petal_query::{App, CachePolicy, Reply};

const UI_SCRIPT: &str = include_str!("ui.ptl");

fn main() -> std::io::Result<()> {
    // The state is built from the handshake (`init.args` / `init.cwd` are how you
    // learn what to show — a dir, a repo, a PR number) and handed to every
    // handler by `&mut` reference. Use `App::stateless(name, script)` if you
    // don't need any.
    App::new("my-app", UI_SCRIPT, |init| PathBuf::from(init.repo_arg()))
        .query("log", |repo: &mut PathBuf, _ctx| {
            // Answer with a value + how cacheable it is. See Step 3.
            Reply::from(fetch_log(repo)).max_age(Duration::from_secs(3))
        })
        .query("commit", |repo: &mut PathBuf, ctx| {
            // A commit addressed by hash never changes — cache it forever.
            Reply::from(fetch_commit(repo, ctx.arg))
        })
        // Optional: react to the script's `emit(event, arg)` — fire-and-forget
        // (persist UI state, open a path). Omit if your app has no use for it.
        .on_emit("ui_state", |repo: &mut PathBuf, ctx| persist(&ctx.arg))
        .serve()
}
```

What `App` guarantees for you (the rules you'd otherwise hand-roll):

- It replies to `initialize` in panel mode **before** anything else, then sends
  `setScript` immediately — the pane shows a spinner until the script arrives.
- A **`query` is a request**; your handler's `Reply` becomes the `queryResult`
  response (value/error + cache policy). Unregistered kinds answer `null`.
- An **`emit` is a notification** (no reply); `on_emit` handlers are
  fire-and-forget. An app that registers none is unaffected by emits.
- It exits on `shutdown` **or** stdin EOF. **Never panic** in a handler — return
  `Reply::error(..)`; log to stderr (the host inherits it) for diagnostics.

For several views from one crate (like `git-viewers`), keep shared plumbing in
`src/lib.rs` and give each `[[bin]]` its own `App` with its own drawer + handlers.

## Step 3 — answering queries

A handler returns a `Reply`:

- `Reply::json(value)` — a value (anything `Serialize`; a `HostData`-shaped JSON
  tree). `Reply::from(result)` maps a `Result<T, E>` (Ok → value, Err → error).
- `Reply::error(msg)` — the script surfaces it via `error_of` / `??`.
- `Reply::loading()` — neither: the host keeps the script spinning without
  re-requesting (use when a background thread will fill it in later).

…and stamps it with a **cache policy** (default `forever`):

```rust
Reply::json(v)                                    // fresh forever (until invalidate)
Reply::json(v).max_age(Duration::from_secs(3))    // refresh after 3s
Reply::json(v).no_store()                          // live data: always revalidate
Reply::json(v).cache(
    CachePolicy::max_age(Duration::from_secs(3))
        .stale_while_revalidate(Duration::from_secs(60)))  // serve stale while refetching
```

### Cacheability

Because a panel *pulls* `query(kind, arg)` every frame, "caching" is really "how
often do we re-ask you, and do we show the old value while we wait?". The host's
cache honors the policy you attach:

| Policy | When to use |
|---|---|
| `CachePolicy::forever()` / `immutable()` (default) | The value at this key can't change — a commit hash, a content digest, a loaded session snapshot. |
| `CachePolicy::max_age(d)` | Changes over time; a stale value is worse than a brief spinner. Hard-expires at `d`. |
| `…​.stale_while_revalidate(s)` | …but you'd rather show the old value than a spinner: within `s` past `max_age`, the stale value is served *and* re-fetched in the background. |
| `CachePolicy::no_store()` | Live data — always served, always revalidated. |

Pick the policy per `(kind, arg)`: `git-log` gives its `log` a short `max_age`
with a stale window, but a `commit` diff addressed by hash is `immutable()`; the
working-tree diff (`@worktree`) is `no_store()`. `garden-diff` treats its whole
projection as one fresh-forever answer and drops it only on a save (the host
drives editing and scrolling locally). A `forever` policy adds
nothing to the wire, so the default costs nothing.

### The data shape (JSON → Petal)

Whatever JSON you put in `value` becomes a Petal value the script reads:

| JSON | Petal value | read in script as |
|---|---|---|
| object | record | `v.field` |
| array | list | `v[i]`, `len(v)`, `for x in v do … end` |
| string | string | `v ++ "…"` |
| number | int **or** float | `v`, `v * 100` (a whole-number literal → int; anything fractional → float) |
| bool | bool | `if v then …` |
| null | nil | `v ?? fallback` |

**Numbers keep their kind.** JSON writes both integers and reals as `number`, so
the host recovers the split from the literal: `7` arrives as an int, `0.42` and
`3.0` as floats. Send a ratio, rate, or duration as a real and the script gets a
real — `draw_rect(x, y, v.fill * meter_w, h, …)` scales the way you'd expect. A
`serde_json::json!` literal already does the right thing (`0.42` is an `f64`),
but watch integer-typed Rust values you *mean* as fractions: `json!(pct / 100)`
on two integers is integer division in Rust and reaches the script as `0`. Do
the division in floating point (`pct as f64 / 100.0`).

So a `query("log")` answer like

```rust
serde_json::json!({ "commits": [ { "hash": h, "short": s, "subject": subj } ] })
```

is read in the script as `data.commits`, each `c.hash` / `c.short` / `c.subject`.
Design these shapes to match exactly what your script reads.

**Soft errors and nil fields.** A convenient convention is a soft `error` field
baked into an otherwise-successful reply (a non-fatal failure the drawer shows in
place, distinct from a `Reply::error` that fails the whole query). Report **no
error as `null`** (`serde_json::Value::Null`), not `""` — a reply never has to
carry an empty placeholder. But then the drawer **must** read it null-safely,
because a bare `len(nil)` (or any op on nil) aborts the frame and raises the
panel error card:

```petal
// WRONG — crashes the frame when the field is null (no error):
if len(data.error) > 0 then … end
// RIGHT — coalesce nil to "" first (`??` treats both nil and pending as absent):
let err = data.error ?? ""
if len(err) > 0 then … end
```

`gpp-apps/git-viewers` and `garden-diff` follow this; `gpp-apps/gpp-test-app`
reproduces the crash-if-you-don't
(`garden --subprocess gpp-test-app runtime-error`).

## Step 4 — the UI script

The pushed script is an ordinary Petal graphical panel (see
`docs/petal-graphical-panels.md` for the full API). Key pieces you get:

- **Per-frame model.** The whole script runs every frame; there's no retained
  scene. Keep UI state in `state` vars (persist across frames).
- **Drawing**, in panel-local pixels, colors as `#rrggbb` (0–255 sRGB):
  `draw_rect(rect(x,y,w,h), color)`, `draw_text(s, {x, y}, size, color)`,
  `draw_line`/`draw_circle`/`draw_rect_outline`/…, `clip(x,y,w,h)`/`clip_none`
  for scroll regions. `screen_width()`/`screen_height()` give the pane size.
  **All text is one monospace size** — build hierarchy with color, and wrap long
  text rather than shrinking.
- **Host theme** (paint in the app's colors, not a hardcoded set): call
  **`palette()`** once per frame and read colors off it — this is the pattern
  every GPP panel app shares, so they all paint in one consistent per-scheme set.
  It returns the host's *current* UI theme as `{ r, g, b, a }` color records (sRGB
  0–255, ready to drop into `draw_rect`/`draw_text`), and is **read-only per-frame
  input** injected before every frame like `screen_width()` — so a live theme
  switch (`POST /theme`, or the user picking a scheme) recolors your drawer on the
  next frame with no work on your part. Idiom:

  ```petal
  let P = palette()
  let BG = P.window_bg   let TEXT = P.text   let ADDBG = P.added_bg   // …then draw with these
  ```

  `palette()` is **always complete** — it overlays the host theme onto a built-in
  fallback, so every key below resolves even in a non-Garden embedder or a bare
  unit test. Read `P.<key>` directly; no `len(keys(...)) > 0` guard, no hardcoded
  fallback of your own. Keys:

  | key | meaning |
  |-----|---------|
  | `window_bg` | the window background (behind panes) |
  | `panel` | pane/card background — **paint a drawer card, and the backdrop behind a `text_view`, with this** so it matches the embedded editor in every scheme |
  | `panel_focused` | focused-pane background |
  | `border` | hairlines, dividers, value-bar tracks |
  | `border_focused` | focused border |
  | `text` | primary text |
  | `text_mut` | secondary text (a tier brighter than `text_dim`) |
  | `text_dim` | dim text |
  | `text_faint` | faintest text (dim faded toward the background) |
  | `cursor` | caret/cursor accent |
  | `accent` | the theme accent (titles, active tab, focus ring) |
  | `focus` | focused-row fill (a step stronger than `sel`) |
  | `sel` | selected-row fill (opaque, pre-composited over `panel`) |
  | `hover` | hovered-row fill (opaque, pre-composited over `panel`) |
  | `green` `orange` `red` `purple` `blue` | semantic accents, tuned to read on the scheme's background (light *and* dark) — e.g. added/warning/error/special/info |
  | `error` | error text |
  | `added_bg` / `removed_bg` | diff add/remove row fills (opaque, match the built-in editor diff view) |
  | `hunk` / `hunk_bg` / `hunk_bg_hover` | diff hunk-header color and its band fill |
  | `scrollbar_thumb` / `scrollbar_track` | scrollbar parts |

  Each color also carries `.a` (0–255), so `draw_rect(r, c, c.a)` paints with the
  theme's native alpha. **A drawer that puts a `text_view` over a custom
  background must paint that background from `palette().panel`** — otherwise a
  fixed dark card collides with the host-themed editor text in a light scheme (low
  contrast). Any of the git/PR/retro/sqlite drawers under `gpp-apps/` is a worked
  example. (The lower-level `panel_theme()` native returns exactly what the host
  injected — an **empty record** when nothing is — and backs `palette()`; prefer
  `palette()` unless you specifically need to detect the no-theme case.)
- **Input** (the focused pane receives it): `key_pressed("j")`,
  `mouse_x()`/`mouse_y()`, `mouse_pressed(0)`, `drag_active()`, `scroll_y()`,
  `text_input()`, `mod_shift()`… plus the `ui` prelude (`button`, `list_update`,
  focus helpers). The command bar (`:`) and global chords stay with the host.
- **Data**: `query(kind, arg)` returns the value when ready, else a **pending**
  value you inspect with `is_ready` / `is_loading` / `is_error` / `error_of` /
  `??`. `invalidate(kind, arg)` drops a cached answer so the next `query`
  re-requests (how you refresh). The per-frame rerun *is* the retry loop.
- **Push (script → app)**: `emit(event, arg)` sends a fire-and-forget signal to
  your app process — `event` is a string naming the intent, `arg` any
  JSON-serializable value (string/int/float/bool/nil/record/list). It returns
  nil and no reply exists (the app receives a notification, not a request).
  The host drains emits once per frame tick and delivers them in call order,
  so guard the call with an edge (a key press, a drag end) rather than emitting
  unconditionally — an unconditional `emit` re-fires every frame while the
  panel is awake. Example — push a divider position on drag end so the app can
  persist it:

  ```petal
  if divider_drag && mouse_released(0) then
    emit("divider", { pos: divider_x })
  end
  ```

  The app sees `EmitParams { event: "divider", arg: { "pos": … } }`; apps that
  never look at `emit` keep working unchanged.
- **Selectable/copyable text**: `text_view(id, x, y, w, h, text)` embeds a real
  editor region (highlight-to-select, Cmd-C, native scroll) instead of glyphs;
  `text_view_line_styles(id, styles)` adds a per-line semantic color
  (`"added"`/`"removed"`/`"hunk"`/`"title"`/`"dim"`/`"comment"`), and
  `text_view_scroll_to(id, line)` scrolls the region so that 0-based line is at
  the top — programmatic navigation (e.g. a file list that jumps the diff beside
  it). That one is an *action*, not frame state: the host applies it once, so
  emit it only on the frame the navigation happens, never every frame. It is an
  anchor rather than a scroll, so a line near the end still reaches the top row
  and the region shows blank space below it; only the buffer's own bounds clamp
  it (a line number past the end lands on the last line).
  `text_view_wrap(id, wrap)` soft-wraps the region's long lines to its width
  instead of letting them run off the right edge — frame state, so declare it on
  every frame the region should wrap. It is opt-in per region because wrapping
  slides a *row-aligned* pair of regions (a side-by-side before/after diff) out
  of step, while a single full-width body only gains from it. Everything else
  stays wrap-aware: clicks, the caret, per-line styling and scroll-to-line all
  keep addressing buffer lines, not screen rows. Use this for
  any diff/code/log body the user might want to copy. **Input routing**: a
  region only consumes an input it can act on. While its content *overflows*
  its rect, the region owns the wheel over it and — once a click focused it —
  the nav keys (`j`/`k`/arrows/page/space/home/end), even at the top/bottom
  boundary; when the content fits, wheel and keys all fall through to the
  script (`scroll_y()`, `key_pressed(...)`), so script-side navigation keeps
  working over selectable text. Clicks are always *teed*: the region starts a
  native selection **and** the script still sees `mouse_pressed()`/`mouse_x/y()`
  for its own click semantics (row selection, toggles). Escape returns key
  focus from a region to the script; Cmd/Ctrl-C and Cmd/Ctrl-A stay native.
- **Editable text**: `edit_view(id, x, y, w, h, seed)` embeds a fully editable
  region — the host's real vim `EditorView`, seeded once with `seed` — and
  `edit_view_text(id)` reads the current buffer back. Click to focus, edit with
  real vim; the script drives write-back itself (`garden-diff`'s `^S` sends the
  edits its region's projection resolved — see
  [Editable panels](#editable-panels-mutate-write-back)). A region's editor state
  is pruned when the region isn't declared for a frame, so if you hide it (a mode
  switch) snapshot `edit_view_text(id)` into a `state` seed first, or unsaved
  edits are lost on return — but see the gotcha below: a region carrying a
  projection cannot be re-seeded that way.
- **Projected text**: `edit_view_projection(id, spec)` tells the host where each
  of a region's lines *came from*, so edits to it fold back into the documents it
  was built out of — see [Editable projections](#editable-projections) below.
  `edit_view_edits(id)` reads out the resulting write-backs.
- **Testing hook**: nothing to call — the host **observes** every named binding
  the frame made and reports it at the debug server's `/state` →
  `panes[].panel.values`. Naming a value is what exposes it; see
  [Step 6 — verifying it](#step-6--verifying-it) below.

A minimal but complete drawer in the shape of `git-viewers/src/git_panel.ptl`
(simplified — the production drawer adds focus regions, draggable dividers, and
refresh) — load a list once, move a selection with the keyboard, and show a
selectable, styled detail on the right:

```petal
state sel = 0
state loaded = false
state commits = []

let w = screen_width()
let h = screen_height()
draw_rect(rect(0, 0, w, h), #0d1117)

// Load the list once; cache it in `state` after it lands.
if !loaded then
  let rd = query("log", "")
  if is_ready(rd) then commits = rd.commits; loaded = true
  elsif is_error(rd) then draw_text("error: " ++ (error_of(rd) ?? "?"), {x: 8, y: 8}, 14, #f85149)
  else draw_text("loading…", {x: 8, y: 8}, 14, #6e7681) end
end

let n = len(commits)
if key_pressed("j") || key_pressed("down") then sel = sel + 1 end
if key_pressed("k") || key_pressed("up") then sel = sel - 1 end
if sel < 0 then sel = 0 end
if n > 0 && sel >= n then sel = n - 1 end

for i in range(0, n) do
  let c = commits[i]
  let y = 8 + i * 20
  if i == sel then draw_rect(rect(0, y - 2, 380, 20), #1e3d63) end
  draw_text(c.short ++ "  " ++ c.subject, {x: 8, y: y}, 14, #c9d1d9)
end

// A second query, keyed by the selection — the diff for the selected row.
if n > 0 then
  let d = query("commit", commits[sel].hash)
  if is_ready(d) then
    let lines = []
    let styles = []
    for f in d.files do
      for ln in f.lines do
        let marker = " "
        let style = ""
        if ln.kind == "add" then marker = "+"; style = "added"
        elsif ln.kind == "del" then marker = "-"; style = "removed"
        elsif ln.kind == "hunk" then marker = "@"; style = "hunk" end
        lines = append(lines, marker ++ " " ++ ln.text)
        styles = append(styles, style)
      end
    end
    text_view(1, 388, 8, w - 396, h - 16, join(lines, "\n"))
    text_view_line_styles(1, styles)
  end
end
```

Notice the pattern: a selection change re-keys the second `query`; the host
requests the new diff over the pipe, caches it, and the next frame renders it —
all with no per-frame wire traffic.

### Persisting drawer UI state

Some drawer state is neither report *data* (it comes from the user, not the
data source) nor throwaway per-frame state (it should outlive a relaunch) — a
draggable divider's position, a chosen sort order, a collapsed/expanded flag.
The recommended pattern is a small **`emit` → subprocess → state file → `query`**
round-trip. `session-retro` is the reference implementation; it persists the
divider width so it survives both view changes and relaunches of the same
session:

1. **Drawer → app, on the edge.** Guard `emit` with an interaction edge (here,
   drag-END) so it fires **once per adjustment**, not every awake frame. Ints
   cross the wire safely (the JSON→Petal bridge coerces every number to an int),
   so scale a fraction:

   ```petal
   // detect dragging → not-dragging this frame, then push the new width ×1000
   if was_dragging && !dragging then
     emit("ui_state", { left_frac: int(left_frac * 1000.0 + 0.5) })
   end
   was_dragging = dragging
   ```

2. **App persists it, keyed by session id, atomically.** In the loop's `emit`
   arm, merge the pushed fields into a retained object and write it to an
   XDG-ish path (`$XDG_STATE_HOME/garden/<app>/<session-id>.json`, falling back
   to `~/.local/state`). Write a temp file and `rename` it over the target so a
   crash mid-write can't corrupt it. **Persistence failures are non-fatal** —
   log to stderr and keep serving; never break the request/response flow:

   ```rust
   } else if env.is_method(method::EMIT) {
       let p: EmitParams = match env.params_as() {
           Ok(p) => p,
           Err(e) => { eprintln!("bad emit: {e}"); continue; }
       };
       if p.event == "ui_state" {
           if let Some(obj) = p.arg.as_object() {
               // merge obj into `ui_state`, then save_ui_state(&session_key, &ui_state)
               // (temp-write + atomic rename; all errors logged, not fatal)
           }
       }
   }
   ```

3. **App re-serves it via a query; drawer restores once at load.** Load the
   file at startup and answer a dedicated `query("ui_state", "")` with it (or
   `null` when absent). The drawer reads it a single time and seeds its state,
   falling back to its default when the answer is null:

   ```petal
   if !ui_loaded then
     let us = query("ui_state", "")
     if is_ready(us) then
       ui_loaded = true
       if us != nil then left_frac = float(us.left_frac) / 1000.0 end
     end
   end
   ```

**Why subprocess-side, keyed by session id — not host-side panel state?** The
host's panel runtime is deliberately stateless across a pane's relaunches (a
reload/split/restore re-spawns the app, which re-pushes its script — see Step 5),
so there is nowhere host-side for per-session UI state to live. The subprocess,
by contrast, already owns a **stable per-session key** (the transcript's session
id) and a place to write, and it re-serves the value on the next launch's
`query`. Keeping it here also leaves the host's contract untouched: a plain
`emit`/`query` pair, no new protocol. The one caveat is that `emit` is delivered
on the next poll tick, so a test that relaunches to check persistence should
poll for the state file (or the restored value) rather than assume it landed
synchronously.

## Multi-screen navigation (optional)

A panel-mode app can have **more than one screen** and give the user browser-style
back/forward across them. Declare each navigable screen's source on the `PanelUi`
with `.screen(name, source)`; the declared set is also the **allowlist** (a
`navigate` to an undeclared screen is refused):

```rust
const HOME: &str = include_str!("home.ptl");
const DETAIL: &str = include_str!("detail.ptl");

gpp::serve(
    provider,
    PanelUi::new("my-app", HOME).screen("detail.ptl", DETAIL),
)
```

Then a screen navigates with the same script API as an in-process panel — the
browser-history natives `navigate("detail.ptl")` / `navigate_replace(...)` /
`navigate_back()` / `navigate_forward()` (and the host's `Ctrl+[`/`Ctrl+]` +
`:back`/`:forward` drive them too). Under the hood the host fetches the target
screen's source from your app over the built-in **`navigate` mutation** and owns
the history stack + per-entry `state` restore; your app just supplies source.

An app that needs **navigation side effects** (log the visit, prime data for the
target screen) registers its own handler instead — it takes precedence over the
built-in, and must return the target `source` itself:

```rust
provider.on_mutation("navigate", |state: &mut S, ctx| {
    let screen = ctx.arg["screen"].as_str().unwrap_or_default();
    state.active_screen = screen.to_string();          // the effect
    Reply::json(serde_json::json!({ "screen": screen, "source": source_for(screen) }))
})
```

More broadly, `on_mutation(name, handler)` is the general **mutation** primitive:
an effectful, uncached request/response (the fourth quadrant beside `query` and
`emit`). `navigate` is just the first built-in use. **v1 limitation:** back/forward
reuse the host-cached source and don't re-issue the mutation, so if your app tracks
hidden per-screen context, key your `query` data by its `arg` (as the reference
apps do) rather than by that context.

## Editable panels (`mutate` write-back)

Every app above is read-only — the panel *pulls* data. An app can also let the
user **edit** and push changes back, using the same `mutate` primitive.
`gpp-apps/garden-diff` is the reference: an editable diff review whose unified
stream and after column both write to the working-tree files on `^S`. The pattern
is three pieces. What a region sends back is the one real choice — its **text**
(below) when the region is a picture of the thing being edited, or the **edits**
its projection resolved ([Editable projections](#editable-projections)) when it
isn't. garden-diff sends edits from both of its views; the text form is shown here
because it is the simpler case and the rest of the pattern is identical.

1. **Draw an `edit_view` region** for the editable content. Unlike `text_view`
   (selectable but read-only), `edit_view(id, x, y, w, h, seed)` is the host's real
   vim editor, seeded once from a `state` var (the drawer keeps a stable seed so
   the seed isn't recomputed every frame):

   ```petal
   edit_view(1, x, y, col_w, body_h, seed)   // real vim; click to focus, edit
   ```

2. **On an edit-commit edge, read the buffer and `mutate`.** `edit_view_text(id)`
   returns the region's current text; `mutate(name, arg)` is a request whose result
   is never cached (unlike `query`) and which returns a value (unlike `emit`).
   Guard it with an edge — a key chord — not every frame:

   ```petal
   if mod_ctrl() && key_pressed("s") then
     mutate("save", { text: edit_view_text(1) })
     invalidate("doc", "")   // drop the cached data so the panes re-read post-save
     ready = false
   end
   ```

3. **Handle the mutation in the app** with `Provider::on_mutation(name, handler)`
   (the same registration `navigate` uses). Do the effect — here, splice the edited
   text back into the files — and return a `Reply` (a status string, or
   `Reply::error(..)` the drawer can surface). **Validate before writing** — a
   text write-back is only as good as the assumption that the buffer still has the
   shape you projected, so check that before touching a file rather than
   mis-writing one. (A projection removes the assumption entirely: the host knows
   what changed, so there is nothing left to re-derive or to distrust.)

   ```rust
   provider.on_mutation("save", |state: &mut State, ctx| {
       let text = ctx.arg["text"].as_str().unwrap_or_default().to_string();
       match write_back(state, &text) {
           Ok(files) => Reply::json(format!("wrote {} files", files.len())),
           Err(e) => Reply::error(e),
       }
   });
   gpp::serve(provider, PanelUi::new("garden-diff", UI_SCRIPT))
   ```

   (garden-diff drives its provider with the lower-level `Provider` + `gpp::serve`
   rather than `App`, so it can register `on_mutation`; the `.query(...)` handlers
   are declared the same way.)

**Region lifetime gotcha.** A region's editor state is pruned when the region
isn't declared for a frame (e.g. you switch to a mode that hides it). If unsaved
edits must survive that, snapshot `edit_view_text(id)` into your seed `state` var
at the transition and re-seed from it on return. A region carrying a *projection*
(below) **cannot** do this: its origin table describes the lines you projected, so
re-seeding from edited text would leave the two out of step. Declare a projected
region from the projected text every frame — the host rebuilds the buffer only
when that text actually changes — and accept that hiding it discards unsaved
edits, as garden-diff's two editable views do.

## Editable projections

Sending the region's **text** back, as above, works when the region is a picture
of the thing being edited. It stops working when the region *mixes* sources — a
unified diff shows a file's current lines interleaved with the base lines it
dropped, so the text is not a picture of any file. Recovering the user's intent
from such a buffer means diffing it against what you projected and guessing, and
the guess is wrong in exactly the interesting cases.

Declare a **projection** instead: say where each line came from, once, and the
host tracks it through every edit and hands you the resulting write-backs.

```petal
edit_view(5, x, y, w, h, doc.unified.text)
edit_view_projection(5, doc.unified.projection)
```

`spec` is a record of parallel arrays — cheap for a drawer to pass straight
through from its client:

| field | meaning |
|---|---|
| `sources` | opaque names the write-backs are addressed to (file paths, say) |
| `span_source` / `span_start` / `span_end` / `span_group` | per editable span: which source it writes to, the `[start, end)` line range it replaces, and a grouping key (`-1` = none) so several spans revert together |
| `kinds` | **one character per projected line** naming its origin (below) |
| `line_spans` | the span each line belongs to (`-1` = none), parallel to `kinds` |
| `styles` | the style name each line is painted with, parallel to `kinds` |
| `decor` | `{ same, added, removed, same_style, added_style, removed_style, diff_markers, gutter }` |

`decor.gutter` decides **where the three markers live**. With it false (the
default) they are the first character of each projected line's text, and the
host strips them back off when folding. With it true they are *display only*:
the buffer holds the sources' own text and the host draws the glyph in a gutter
column beside it, from the same origin table.

Prefer `gutter: true` for anything a user will edit. With markers in the text
every buffer operation is one character out of step with the content — `J` joins
`+one` and `+two` into `+one +two`, `0` lands the cursor on a `+` instead of the
indent, a column selection takes the marker with it, and a `/` search has to
match past it. With `gutter: true` none of that arises, because there is nothing
in the buffer to step around. It also turns off `diff_markers` (there is no
marker in a typed line to interpret, so a line starting `-` is just code that
starts with `-`) and stops autoindent copying a marker into a newly opened line.

The origin alphabet:

| char | origin | deleting the line means |
|---|---|---|
| `' '` | content the source holds unchanged | remove it from the source |
| `'+'` | content added relative to the base | drop that addition |
| `'-'` | content the base held and the source dropped | **revert** that deletion — the base line comes back |
| `'c'` | chrome (inert decoration) | nothing |
| `'l'` | chrome, locked | refused, with a status message |
| `'h'` | chrome heading its span | revert the whole span |
| `'g'` | chrome heading its group | revert every span in the group |

Two things this buys you. First, **all of vim works**: `dd`, `3dd`, `cc`, `V}d`,
`p`, `.`-repeat, undo and redo all fold back correctly, because the host reports
each buffer mutation to the projection rather than anyone re-deriving intent
afterwards — and undo restores the *origins*, so a `-` line brought back by undo
is a deletion again. Second, **styles stop drifting**: the bands ride the origin
table and follow their lines through insertions, so a projected region needs no
`text_view_line_styles` and no per-frame restyling.

Then save the edits rather than the text:

```petal
if mod_ctrl() && key_pressed("s") then
  mutate("apply", { edits: edit_view_edits(5) })
  invalidate("doc", "")
  ready = false
end
```

`edit_view_edits(id)` is a list of `{source, start, end, lines}` — replace lines
`[start, end)` of `source` with `lines`. The client just applies them:

```rust
provider.on_mutation("apply", |state: &mut State, ctx| {
    match parse_edits(&ctx.arg).and_then(diff_core::apply_edits) {
        Ok(files) => Reply::json(format!("wrote {} files", files.len())),
        Err(e) => Reply::error(e),
    }
});
```

Declare the spec **every frame**, like the region itself. The host fingerprints it
and rebuilds only when it genuinely changes, so re-declaring the same one leaves
the live table — and the user's edits — alone.

## Step 5 — launching it

The host spawns a panel-mode app the same way as any GPP client — a `process`
node in a layout script:

```petal
layout(process("/abs/path/to/target/debug/my-app", ["/some/dir"]))
```

The command is run with `Command::new(command)`, so a **bare name is resolved on
`$PATH`**; during development use an **absolute path** (as above) or put the
binary on `PATH`. The `args` list becomes `InitializeParams::args`, and the pane's
`cwd` is passed too — that's how the app learns what to operate on. (Garden's
own clients — the directory browser, `git-log`, and `garden-diff` — get a
sibling-of-`garden` resolver when launched via `:E`/`:Git`/`:Diff`/`:PR`; a
generic `process(...)` does not, so be explicit.)

The pane **renders and handles input as a panel** but **persists as a
`process(...)` node**: a reload, split, or window restore re-spawns your app,
which re-pushes its script — no stale state, no on-disk marker.

## Step 6 — verifying it

Drive it headlessly over the debug server (no window, no focus stealing — see
`docs/debug-server.md`):

```bash
cargo build
cat > /tmp/init.ptl <<EOF
layout(process("$PWD/target/debug/my-app", ["$PWD"]))
EOF
./target/debug/garden --headless --debug-port 8080 --init /tmp/init.ptl &

curl -s localhost:8080/state | jq '.panes[0].panel.values'  # every value you bound
curl -s -X POST localhost:8080/key -d '{"key":"j"}'         # forwarded to the script
curl -s localhost:8080/screenshot -o /tmp/shot.png          # offscreen render
```

A working app shows `panes[0].kind == "panel"`, an incrementing `frame`, and a
`values` object holding whatever your script bound this frame — including the
data that came back over `query`. That map is Petal's *observation* buffer: the
last value bound to every named term, so the drawer above needs no publishing
step for a test to read its `sel`:

```bash
curl -s localhost:8080/state | jq '.panes[0].panel.values.sel'    # → 0
curl -s -X POST localhost:8080/key -d '{"key":"j"}'
curl -s localhost:8080/state | jq '.panes[0].panel.values.sel'    # → 1
```

Keys are function-qualified — a `let y` inside `fn draw_row` reads as
`draw_row.y`, and reports its *last* call — and values keep their real types
(bools are bools, lists are lists). A binding that never executed is absent
rather than null.

Two habits this rewards. **Name the values a test needs**: a click target you
compute inline into `draw_rect(...)` is invisible, while `let row_y = …` is free
introspection, so a test can click by geometry it reads instead of geometry it
hardcodes. **Don't bind what you don't want dumped**: a name holding a 200 KB
rendered body puts 200 KB into every `/state` response — bind
`let body_len = len(body)` when the length is the assertable part. And because
one-frame edges (`mouse_released`, `click_count`, `scroll_y`, `text_input`) are
cleared by the next idle tick, count them into a `state` var rather than
sampling them; the counter is observed under its own name.

Assert on `values` for deterministic integration tests — the pattern the
built-in git panel uses.

## Gotchas and current limits

- **The pushed script runs in-process in the host.** A runaway script
  (`while true`) hangs the editor — panel-mode apps are **trusted code** (your
  own), not sandboxed downloads, until Petal ships a bounded/interruptible run.
  Your app *process* is still isolated (its logic can't hang the host); the
  *script* is not.
- **The host↔app contract is wide.** Your script targets the host's Petal +
  panel-native version ("browser compatibility"). Fine within this repo; keep it
  in mind before shipping external apps.
- **One text size.** Panels draw all text at the single monospace size; use color
  and wrapping for hierarchy.
- **Selectable text needs `text_view`.** Script-drawn `draw_text` glyphs can't be
  selected or copied — route any body the user might copy through a `text_view`.
- **`emit` needs a subprocess.** `emit(event, arg)` reaches your app only in a
  panel-mode GPP pane; in a plain in-process `panel(script)` pane there is no
  client to deliver to, so the events are silently dropped. `openPath` (turn
  the pane into an editor on a file) and `setStatus` from the app work too.
- **`query` latency is ~one poll tick** in steady state (answers are applied on
  the ~200ms poll before the next frame). The first frame is primed synchronously
  so a freshly opened app paints with data, not a spinner.

## Checklist

1. New workspace-member crate depending on `petal-query` + `serde_json`.
2. `src/ui.ptl` — the Petal drawer, `include_str!`'d.
3. `src/main.rs` — build a `petal_query::App`, register a `.query(kind, …)`
   handler per kind (each returning a `Reply` with a `CachePolicy`), an
   `.on_emit(…)` per event you care about, and call `.serve()`.
4. Define your `(kind, arg)` → JSON shapes to match what the script reads, and
   choose a cache policy per kind.
5. `cargo build`; launch via `process("/abs/path", [args])`; verify over the
   debug server.

Reference implementations: `gpp-apps/git-viewers` (the `git-log` app behind
`:Git`; full cache-policy range), `gpp-apps/garden-diff` (the *editable* diff
review behind `:Diff` / `:Review*` / `:PR` and the `garden diff` / `garden pr`
CLIs — `edit_view` + editable projections + a `mutate("apply", …)` write-back,
and `gh`-backed in PR mode),
`gpp-apps/session-retro` (stateful + `on_emit` persistence),
`gpp-apps/sqlite-browser` (a read-only SQLite browser + visualizer via
`rusqlite` — catalog, schema+data grid, and an Overview bar chart). Provider API:
`../petal-query/README.md`. Protocol reference:
`docs/gpp.md`. Draw/input API: `docs/petal-graphical-panels.md`. Design rationale:
`docs/gpp.md`.
