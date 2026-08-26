# Writing GPP apps

A **GPP app** is a subprocess that drives the content of a Garden pane over a
small JSON-RPC protocol on stdio (the Garden Pane Protocol, **v2**). This guide
is a practical, build-it walkthrough: your app pushes a **Petal UI script** the
host runs in-process, then serves that script's data over the pipe.

A Rust app is built on the **`petal-query`** crate (`../petal-query`): you
declare a handler per `query(kind, arg)` — each returning a `Reply` with the
value (or error) and **how cacheable it is** (`CachePolicy`) — and
`petal-query` runs the whole protocol loop for you. You do **not** hand-roll
the stdio handshake or the response plumbing. (An app in another language
implements the loop from the wire spec directly — it is small; see
[`gpp.md`](gpp.md).)

Read `docs/gpp.md` for the wire reference, `../petal-query/README.md` for the
provider API, and `docs/petal-graphical-panels.md` for the panel draw/input
vocabulary. The complete worked example this guide tracks is
`gpp-apps/git-viewers` — the in-use app behind `:Git`. Its `git-log` bin keeps
the git plumbing in `src/lib.rs`, declares its query handlers on a
`petal_query::Provider`, and `include_str!`s a colocated production drawer
(`src/git_panel.ptl`). Copy it to start. `gpp-apps/directory-browser` is the
smallest complete app (one query kind, a host-owned mutation);
`gpp-apps/garden-diff` is the reference for an **editable** app (`edit_view`
regions carrying editable projections, written back with `mutate("apply", …)` —
the only GPP app that is not read-only; see
[Editable panels](#editable-panels-mutate-write-back) below) and a
**network-backed** one (the `gh` CLI, in its PR mode); `gpp-apps/screens-demo`
for multi-screen navigation.

## The mental model

Think **web browser + web server**:

- The **host** (garden) is the browser. It runs your UI script every frame,
  handles all interaction locally (scroll, hover, selection, drag), and renders.
- Your **app** is the server. It ships a "page" (a Petal script) once, then
  answers **data requests** the page makes (`query(kind, arg)`), by doing
  whatever a subprocess can do — shell out to `git`/`gh`, read files, hit a
  network service.

The interaction loop runs host-side with **zero pipe traffic**, so scrolling a
diff or moving a selection is instant, and only real data crosses the wire. The
same Petal script that an in-process `panel(...)` pane runs can run in your app
— the only difference is whether the data provider is local Rust or your
subprocess.

## Anatomy of an app

A GPP app is a normal Rust binary in the workspace with two parts:

1. **A Petal UI script** (a colocated `src/*.ptl`, embedded with `include_str!`)
   — the "page": it draws, handles input, and calls `query(kind, arg)` for its
   data.
2. **A `main.rs`** (or `src/bin/<name>.rs` for a multi-bin crate) that builds a
   `petal_query::Provider` — the handlers — and hands it to `gpp::serve` with a
   `PanelUi` (the pane name + the script).

Your app **never draws** and **never handles keys** itself; the host does, by
running your script. Your app only ships the script and answers data requests.
Crucially, your app's *logic* can be in any language-agnostic style (it's a
JSON-over-pipe data server) — only the *UI* is Petal.

## Step 1 — the crate

Add a workspace member depending on `petal-query` (+ `serde_json`). A
single-bin skeleton:

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

(`petal-query` itself depends on the `gpp` crate — the single wire definition —
and re-exports what an app needs, so an app never links `gpp` directly.)

For several views from one crate, follow `gpp-apps/git-viewers/Cargo.toml`: a
shared `[lib]` plus one `[[bin]]` per view.

Add `"gpp-apps/my-app"` to the workspace `members` in the root `Cargo.toml`. GPP
client binaries deliberately are **not** dependencies of `garden-app`, so build
the whole workspace (`cargo build`) to produce your binary beside `garden`.

## Step 2 — declare the app

`Provider` + `gpp::serve` is the whole loop: the handshake (protocol version
check included), the script push, and the dispatch of every
`query`/`mutate`/`navigate`/`emit` to the handler you registered. You write
only the handlers. A single-bin app puts this in `src/main.rs`,
`include_str!`'ing the drawer from a sibling `.ptl`:

```rust
use std::path::PathBuf;
use std::time::Duration;
use petal_query::{Provider, Reply};
use petal_query::gpp::{self, PanelUi};

const UI_SCRIPT: &str = include_str!("ui.ptl");

fn main() -> std::io::Result<()> {
    // The state is built from the handshake (`init.args` / `init.cwd` are how
    // you learn what to show — a dir, a repo, a PR number) and handed to every
    // handler by `&mut` reference. Use `Provider::stateless()` if you don't
    // need any.
    let provider = Provider::new(|init| PathBuf::from(init.repo_arg()))
        .query("log", |repo: &mut PathBuf, _ctx| {
            // Answer with a value + how cacheable it is. See Step 3.
            Reply::from(fetch_log(repo)).max_age(Duration::from_secs(3))
        })
        .query("commit", |repo: &mut PathBuf, ctx| {
            // A commit addressed by hash never changes — cache it forever.
            Reply::from(fetch_commit(repo, ctx.arg_str()))
        })
        // Optional: react to the script's `emit(event, arg)` — fire-and-forget
        // (persist UI state, kick a refresh). Omit if your app has no use for it.
        .on_emit("ui_state", |_repo, ctx| persist(ctx.arg));

    gpp::serve(provider, PanelUi::new("my-app", UI_SCRIPT))
}
```

What the loop guarantees for you (the rules you'd otherwise hand-roll):

- It answers `initialize` (verifying `protocol: 2` — a mismatched host is
  refused with a clean error) **before** anything else, then sends `setScript`
  immediately.
- A **`query` is a request**; your handler's `Reply` becomes the id-correlated
  response (a `{ value, cache? }` result, or a JSON-RPC error for
  `Reply::error`). Unregistered kinds answer `null`.
- A **`mutate` is a request**; `on_mutation` handlers return a `Reply` the same
  way (never cached). An unregistered mutation is an error.
- A **`navigate` request** is served from your declared screens (or your
  `on_navigate` handler — see [Multi-screen navigation](#multi-screen-navigation-optional)).
- An **`emit` is a notification** (no reply); `on_emit` handlers are
  fire-and-forget. An app that registers none is unaffected by emits.
- An unknown *request* is answered `METHOD_NOT_FOUND`; unknown notifications
  are skipped — forward compatibility.
- It exits on `shutdown` **or** stdin EOF. **Never panic** in a handler —
  return `Reply::error(..)`; log to stderr (the host inherits it) for
  diagnostics.
- `PanelUi::title(|state| …)` derives the pane's display name from the built
  state (e.g. the browsed directory) instead of the static name.

Handler contexts carry the request's `kind`/`name`/`event`/`screen`, its JSON
`arg` (`ctx.arg`, any `serde_json::Value`; `ctx.arg_str()` for the common
string case), and the handshake `InitializeParams` (`ctx.init`).

## Step 3 — answering queries

A handler returns a `Reply`:

- `Reply::json(value)` — a value (anything `Serialize`; a `HostData`-shaped JSON
  tree). `Reply::from(result)` maps a `Result<T, E>` (Ok → value, Err → error).
- `Reply::error(msg)` — a JSON-RPC error response on the wire; the script
  surfaces it via `error_of` / `??`.
- `Reply::loading()` — neither: the host keeps the script spinning without
  re-requesting (use when a background thread will fill it in later, then push
  an `invalidate`).

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
| `CachePolicy::forever()` / `immutable()` (default) | The value at this key can't change — a commit hash, a content digest. |
| `CachePolicy::max_age(d)` | Changes over time; a stale value is worse than a brief spinner. Hard-expires at `d`. |
| `…​.stale_while_revalidate(s)` | …but you'd rather show the old value than a spinner: within `s` past `max_age`, the stale value is served *and* re-fetched in the background. |
| `CachePolicy::no_store()` | Live data — always served, always revalidated. |

Pick the policy per `(kind, arg)`: `git-log` gives its `log` a short `max_age`
with a stale window, but a `commit` diff addressed by hash is `immutable()`; the
working-tree diff (`@worktree`) is `no_store()`. `garden-diff` treats its whole
projection as one fresh-forever answer and drops it only on a save (the host
drives editing and scrolling locally). A `forever` policy adds nothing to the
wire, so the default costs nothing.

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

**Query args are JSON.** The wire carries `arg` verbatim — a string, a record,
a list — so a composite key (`{ "table": "users", "page": 3 }`) needs no string
encoding; the host caches per `(kind, arg)`. Petal's script-side `query`
native currently passes a string arg, which arrives at your handler as a JSON
string (`ctx.arg_str()` reads it); a client in another language, or a future
script-side extension, can key by any JSON value today.

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
  scene. Keep UI state in `state` vars (persist across frames). Declare them at
  the **top level** — that is one cell per name for the whole panel, and it is
  what every drawer in this repo does. A `state` written *inside a function* is
  a different thing: it is keyed by the call path that reached it, so each
  callsite — and each iteration of a `for` around the call — gets its own cell.
  That is exactly what you want for a reusable row/widget helper
  (`fn row(item) state hovered = false … end` gives every row its own hover),
  and exactly what you do *not* want for a value two functions are meant to
  share: hoist that one to a top-level `state var` and reach it with
  `get`/`set`. When a per-item cell must follow the item rather than its
  position in the list, key it — `state(item.id) hovered = false` is absolute,
  ignores the call path, and so survives a reorder. Note that the debug server
  flattens these: `panes[].panel.values` reports an in-function binding once,
  under its function-qualified name (`row.hovered`), holding whichever call
  wrote it last in the frame — top-level names are the ones you can assert on
  cell by cell.
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
  contrast). Any of the git/diff/sqlite drawers under `gpp-apps/` is a worked
  example. (The lower-level `panel_theme()` native returns exactly what the host
  injected — an **empty record** when nothing is — and backs `palette()`; prefer
  `palette()` unless you specifically need to detect the no-theme case.)
- **Input** (the focused pane receives it): `key_pressed("j")`,
  `mouse_x()`/`mouse_y()`, `mouse_pressed(0)`, `click_count()`, `drag_active()`,
  `scroll_y()`, `text_input()`, `mod_shift()`… plus the `ui` prelude (`button`,
  `list_update`, focus helpers). The command bar (`:`) and global chords stay
  with the host. The host spells Return `"return"` (never `"enter"`).
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
- **Asking Garden to act**: `mutate(name, arg)` with one of the **host-owned
  names** (`open_path`, `open_project`, `open_pr`, `open_file_dialog` — see
  `gpp.md`) is answered by the host itself and never reaches your app — this is
  how the directory browser opens a file and the main menu opens a project.
  Every other name is forwarded to your `on_mutation` handler.
- **Selectable/copyable text**: `text_view(id, x, y, w, h, text)` embeds a real
  editor region (highlight-to-select, Cmd-C, native scroll) instead of glyphs;
  `text_view_line_styles(id, styles)` adds a per-line semantic color
  (`"added"`/`"removed"`/`"hunk"`/`"title"`/`"dim"`/`"comment"`), and
  `text_view_scroll_to(id, line)` scrolls the region so that 0-based line is at
  the top — programmatic navigation (e.g. a file list that jumps the diff beside
  it). That one is an *action*, not frame state: the host applies it once, so
  emit it only on the frame the navigation happens, never every frame.
  `text_view_wrap(id, wrap)` soft-wraps the region's long lines to its width —
  frame state, so declare it on every frame the region should wrap. Everything
  else stays wrap-aware: clicks, the caret, per-line styling and scroll-to-line
  all keep addressing buffer lines, not screen rows. Use this for any
  diff/code/log body the user might want to copy. A region declared while a
  `clip(...)` is active is clipped by it — text included. **Input routing**: a
  region only consumes an input it can act on. While its content *overflows*
  its rect, the region owns the wheel over it and — once a click focused it —
  the nav keys, even at the top/bottom boundary; when the content fits, wheel
  and keys all fall through to the script. Clicks are always *teed*: the region
  starts a native selection **and** the script still sees
  `mouse_pressed()`/`mouse_x/y()` for its own click semantics. Escape returns
  key focus from a region to the script; Cmd/Ctrl-C and Cmd/Ctrl-A stay native.
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

Notice too that `sel`, `loaded` and `commits` are declared at the **top level**,
where each is a single cell for the panel: the row loop reads and writes them
across iterations and they mean the same thing on the next frame. That is the
default shape for drawer state, and the reason this drawer needs no keying.
Moving one of those declarations down into a helper would change what it means —
it would become one cell per callsite of that helper, per loop iteration — so
factor a drawer into helpers that take state as parameters and return values,
and keep the cells themselves at the top.

### Persisting drawer UI state

Some drawer state is neither report *data* (it comes from the user, not the
data source) nor throwaway per-frame state (it should outlive a relaunch) — a
draggable divider's position, a chosen sort order, a collapsed/expanded flag.
The recommended pattern is a small **`emit` → subprocess → state file → `query`**
round-trip:

1. **Drawer → app, on the edge.** Guard `emit` with an interaction edge (here,
   drag-END) so it fires **once per adjustment**, not every awake frame:

   ```petal
   // detect dragging → not-dragging this frame, then push the new width ×1000
   if was_dragging && !dragging then
     emit("ui_state", { left_frac: int(left_frac * 1000.0 + 0.5) })
   end
   was_dragging = dragging
   ```

2. **App persists it, atomically.** Register an `on_emit("ui_state", …)`
   handler that merges the pushed fields into a retained object on your state
   and writes it to an XDG-ish path
   (`$XDG_STATE_HOME/garden/<app>/<key>.json`, falling back to
   `~/.local/state`). Write a temp file and `rename` it over the target so a
   crash mid-write can't corrupt it. **Persistence failures are non-fatal** —
   log to stderr and keep serving; never break the request/response flow:

   ```rust
   .on_emit("ui_state", |state: &mut State, ctx| {
       if let Some(obj) = ctx.arg.as_object() {
           state.ui_state.extend(obj.clone());
           if let Err(e) = save_ui_state(&state.key, &state.ui_state) {
               eprintln!("my-app: could not persist ui_state: {e}");
           }
       }
   })
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

**Why subprocess-side — not host-side panel state?** The host's panel runtime
is deliberately stateless across a pane's relaunches (a reload/split/restore
re-spawns the app, which re-pushes its script — see Step 5), so there is
nowhere host-side for per-session UI state to live. The subprocess, by
contrast, owns a stable key and a place to write, and it re-serves the value on
the next launch's `query`. Keeping it there also leaves the host's contract
untouched: a plain `emit`/`query` pair, no new protocol. The one caveat is that
`emit` is delivered on the next poll tick, so a test that relaunches to check
persistence should poll for the state file (or the restored value) rather than
assume it landed synchronously.

## Multi-screen navigation (optional)

An app can have **more than one screen** and give the user browser-style
back/forward across them. Declare each navigable screen's source on the
`PanelUi` with `.screen(name, source)`; the declared set is also the
**allowlist** (a `navigate` to an undeclared screen is refused):

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
screen's source from your app with GPP v2's first-class **`navigate` request**
and owns the history stack + per-entry `state` restore; your app just supplies
source. `gpp-apps/screens-demo` is the worked example.

`navigate("detail.ptl", { id: 7 })` additionally carries the subject the target
screen is for. The target screen reads it back with `nav_arg()`, and it arrives
at your handler as `ctx.arg`. The host stores it on the history entry, so
*back* and *forward* return to that screen with the argument it was opened with
rather than an empty one. See
[petal-graphical-panels.md](petal-graphical-panels.md#navigating-navigatescreen--navigatescreen-arg).

An app that needs **navigation side effects** (log the visit, prime data for the
target screen) registers an `on_navigate` handler instead — it replaces the
built-in declared-screens lookup, and returns the target screen's `source` (or
an `Err` to refuse the navigation):

```rust
provider.on_navigate(|state: &mut S, ctx| {
    state.active_screen = ctx.screen.to_string();       // the effect
    Ok(source_for(ctx.screen)?)                          // the source
})
```

**Back and forward re-issue it.** Restoring a history entry replays the *host's*
record of that visit — its source, its `state` snapshot, its navigation argument
— but your provider holds the data the screen actually draws, so the host asks
again: going back or forward to a navigated entry sends your `navigate` handler
the same screen and `arg` the original push did, and your handler re-runs its
effect before the screen is redrawn. Write it to be **idempotent**: it is called
once per visit, not once per screen. Returning a changed `source` swaps the
running program in; returning the same one costs nothing.

The replay is best effort, because the user's back/forward must not fail: the
cursor has already moved when the request is sent, so if your app is gone, slow
(500 ms), or rejects the screen, the entry stays on its cached source and the
reason appears in the pane's status note. The seed entry is never replayed —
nothing navigated to it, and its screen name is the pane's own origin rather than
one your app declared. In-process `panel(...)` panes have no provider to re-ask.

A script-issued `mutate(name, arg)` hands the drawer a **handle**, and
`mutate_result(handle)` reads back `{ ok, value, error }` once your handler has
replied — the panel-side half of `Reply::json` / `Reply::error`. See
[petal-graphical-panels.md](petal-graphical-panels.md#reading-the-outcome-the-handle-mutate-returns).

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

3. **Handle the mutation in the app** with `Provider::on_mutation(name, handler)`.
   Do the effect — here, splice the edited text back into the files — and return
   a `Reply` (a status string the host shows in the status bar, or
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

The host spawns a GPP app the same way as any client — a `process` node in a
layout script:

```petal
layout(process("/abs/path/to/target/debug/my-app", ["/some/dir"]))
```

The command is run with `Command::new(command)`, so a **bare name is resolved on
`$PATH`**; during development use an **absolute path** (as above) or put the
binary on `PATH`. The `args` list becomes `InitializeParams::args`, and the pane's
`cwd` is passed too — that's how the app learns what to operate on. (Garden's
own clients — the directory browser, `git-log`, `garden-diff`, `main-menu` —
get a sibling-of-`garden` resolver when launched via `:E`/`:Git`/`:Diff`/`:PR`;
a generic `process(...)` does not, so be explicit.) `garden --subprocess
<app> [args…]` runs any client as the whole layout.

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

A working app shows `panes[0].kind == "panel"`, a `panel.client` naming your
spawn command, an incrementing `frame`, and a `values` object holding whatever
your script bound this frame — including the data that came back over `query`.
That map is Petal's *observation* buffer: the last value bound to every named
term, so the drawer above needs no publishing step for a test to read its `sel`:

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

Assert on `values` for deterministic integration tests — the pattern
`tools/integration-test.ts` (the directory browser) and the git/menu/diff
harnesses all use.

## Gotchas and current limits

- **The pushed script runs in-process in the host.** A runaway script
  (`while true`) hangs the editor — GPP apps are **trusted code** (your
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
  GPP pane; in a plain in-process `panel(script)` pane there is no client to
  deliver to, so the events are silently dropped. The host-owned `mutate`
  names work everywhere.
- **`query` latency is ~one poll tick** in steady state (answers are applied on
  the ~200ms poll before the next frame). The first frame is primed synchronously
  so a freshly opened app paints with data, not a spinner.

## Checklist

1. New workspace-member crate depending on `petal-query` + `serde_json`.
2. `src/ui.ptl` — the Petal drawer, `include_str!`'d.
3. `src/main.rs` — build a `petal_query::Provider`, register a `.query(kind, …)`
   handler per kind (each returning a `Reply` with a `CachePolicy`), an
   `.on_mutation(…)` / `.on_emit(…)` / `.on_navigate(…)` per call you care
   about, and hand it to `gpp::serve` with a `PanelUi`.
4. Define your `(kind, arg)` → JSON shapes to match what the script reads, and
   choose a cache policy per kind.
5. `cargo build`; launch via `process("/abs/path", [args])`; verify over the
   debug server.

Reference implementations: `gpp-apps/directory-browser` (the smallest complete
app: one query kind + the host-owned `open_path` mutation),
`gpp-apps/git-viewers` (the `git-log` app behind `:Git`; full cache-policy
range), `gpp-apps/garden-diff` (the *editable* diff review behind `:Diff` /
`:Review*` / `:PR` — `edit_view` + editable projections + a
`mutate("apply", …)` write-back, and `gh`-backed in PR mode),
`gpp-apps/sqlite-browser` (a read-only SQLite/Postgres browser + visualizer),
`gpp-apps/screens-demo` (multi-screen navigation), and `gpp-apps/gpp-test-app`
(the error-state fixture). Provider API: `../petal-query/README.md`. Protocol
reference: `docs/gpp.md`. Draw/input API: `docs/petal-graphical-panels.md`.
