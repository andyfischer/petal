# Writing GPP apps

A GPP app is a subprocess that drives one Garden pane. It pushes a Petal UI
script to Garden once, then answers that script's data requests over stdio
using the Garden Pane Protocol ([gpp.md](gpp.md)).

This guide builds a Rust app on the `petal-query` crate
(`../../petal-query`), which runs the whole protocol loop for you. You write
a handler per query kind and a `.ptl` drawer; nothing else. For Python, see
[writing-gpp-apps-python.md](writing-gpp-apps-python.md). Any other language
implements the loop from the wire spec directly; it is small.

Related docs:

- [gpp.md](gpp.md): the wire protocol.
- [petal-graphical-panels.md](petal-graphical-panels.md): the drawer's draw,
  input, and text vocabulary.
- [petal-query/README.md](../../petal-query/README.md): the provider API.

Reference apps under `gpp-apps/`:

| App | Good for |
|-----|----------|
| `directory-browser` | the smallest complete app: one query kind and a host-owned mutation |
| `git-viewers` (`git-log`, behind `:Git`) | the one this guide tracks: git plumbing in `src/lib.rs`, a `Provider` in `src/bin/git-log.rs`, and a colocated drawer `src/git_panel.ptl`. Copy it to start. |
| `garden-diff` | the only editable app: `edit_view` regions with projections, written back with `mutate("apply", …)`; also network-backed (`gh`) in PR mode |
| `sqlite-browser` | a read-only SQLite and Postgres browser |
| `screens-demo` | multi-screen navigation |
| `main-menu` | the start screen |
| `gpp-test-app` | a fixture that puts a pane into a chosen error state |

## The mental model

Think web browser and web server.

- Garden is the browser. It runs your script every frame, handles all
  interaction locally (scroll, hover, selection, drag), and renders.
- Your app is the server. It ships one page (the Petal script), then answers
  `query(kind, arg)` requests by doing whatever a subprocess can do: shell out
  to `git` or `gh`, read files, call a network service.

Interaction never touches the pipe, so scrolling and selection are instant.
Only data crosses the wire. The same script would run unchanged in an
in-process `panel(...)` pane; the only difference is who answers `query`.

Your app never draws and never handles keys. Only the UI is Petal; the app's
logic can be in any style you like, since it is a JSON-over-pipe data server.

## Step 1: the crate

Add a workspace member under `gpp-apps/` that depends on `petal-query` and
`serde_json`:

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

`petal-query` re-exports everything an app needs from the `gpp` crate, so an
app never links `gpp` directly. For several views from one crate, follow
`gpp-apps/git-viewers/Cargo.toml`: a shared `[lib]` plus one `[[bin]]` per
view.

Add `"gpp-apps/my-app"` to `members` in the root `Cargo.toml`. GPP binaries
are deliberately not dependencies of `garden-app`, so build the whole
workspace (`cargo build`) to get your binary beside `garden`.

## Step 2: declare the app

`Provider` plus `gpp::serve` is the whole loop: the handshake, the script
push, and dispatch of every `query`, `mutate`, `navigate`, and `emit` to the
handler you registered. A single-binary app puts this in `src/main.rs` and
embeds the drawer from a sibling `.ptl`:

```rust
use std::path::PathBuf;
use std::time::Duration;
use petal_query::{Provider, Reply};
use petal_query::gpp::{self, PanelUi};

const UI_SCRIPT: &str = include_str!("ui.ptl");

fn main() -> std::io::Result<()> {
    // State is built once from the handshake (`init.args` / `init.cwd` tell
    // you what to show: a directory, a repo, a PR number) and handed to every
    // handler by `&mut`. Use `Provider::stateless()` if you have none.
    let provider = Provider::new(|init| PathBuf::from(init.repo_arg()))
        .query("log", |repo: &mut PathBuf, _ctx| {
            Reply::from(fetch_log(repo)).max_age(Duration::from_secs(3))
        })
        .query("commit", |repo: &mut PathBuf, ctx| {
            // A commit addressed by hash never changes: cache it forever.
            Reply::from(fetch_commit(repo, ctx.arg_str()))
        })
        // Optional: react to the script's `emit(event, arg)`.
        .on_emit("ui_state", |_repo, ctx| persist(ctx.arg));

    gpp::serve(provider, PanelUi::new("my-app", UI_SCRIPT))
}
```

What the loop does for you:

- Answers `initialize` first (refusing a host that is not `protocol: 2`),
  then sends `setScript` immediately.
- `query` is a request. Your handler's `Reply` becomes the id-correlated
  response: a `{ value, cache? }` result, or a JSON-RPC error for
  `Reply::error`. Unregistered kinds answer `null`.
- `mutate` is a request. `on_mutation` handlers return a `Reply` the same way,
  never cached. An unregistered mutation is an error.
- `navigate` is served from your declared screens, or from an `on_navigate`
  handler (see [Multi-screen navigation](#multi-screen-navigation)).
- `emit` is a notification with no reply; `on_emit` handlers are
  fire-and-forget.
- An unknown request gets `METHOD_NOT_FOUND`; unknown notifications are
  skipped.
- It exits on `shutdown` or stdin EOF.

Never panic in a handler; return `Reply::error(..)` instead. Log to stderr
(Garden inherits it). `PanelUi::title(|state| …)` derives the pane's display
name from the state, for example the browsed directory.

Handler contexts carry the request's `kind`, `name`, `event`, or `screen`, its
JSON `arg` (`ctx.arg` as a `serde_json::Value`, or `ctx.arg_str()` for the
common string case), and the handshake params (`ctx.init`).

## Step 3: answering queries

A handler returns a `Reply`:

- `Reply::json(value)`: any `Serialize` value. `Reply::from(result)` maps a
  `Result<T, E>` to a value or an error.
- `Reply::error(msg)`: a JSON-RPC error; the script sees it through
  `is_error`, `error_of`, or `??`.
- `Reply::loading()`: neither. The host keeps the script waiting without
  re-requesting. Use it when a background thread will fill the value in later
  and push an `invalidate`.

Each reply carries a cache policy (default: forever):

```rust
Reply::json(v)                                    // fresh forever (until invalidate)
Reply::json(v).max_age(Duration::from_secs(3))    // refresh after 3 s
Reply::json(v).no_store()                         // live data: always revalidate
Reply::json(v).cache(
    CachePolicy::max_age(Duration::from_secs(3))
        .stale_while_revalidate(Duration::from_secs(60)))  // serve stale while refetching
```

### Cacheability

The drawer pulls `query(kind, arg)` every frame, so "caching" really means
"how often does Garden re-ask, and does it show the old value while waiting".

| Policy | When to use |
|---|---|
| `CachePolicy::forever()` / `immutable()` (default) | the value at this key cannot change: a commit hash, a content digest |
| `CachePolicy::max_age(d)` | changes over time, and a stale value is worse than a brief spinner |
| `….stale_while_revalidate(s)` | show the old value for up to `s` past `max_age` while re-fetching in the background |
| `CachePolicy::no_store()` | live data; always revalidated |

Pick the policy per `(kind, arg)`. `git-log` gives `log` a short `max_age`
with a stale window, a `commit` diff addressed by hash is immutable, and the
working-tree diff is `no_store()`. `garden-diff` treats its whole projection
as fresh forever and drops it only on save. The default `forever` policy adds
nothing to the wire.

### The data shape (JSON to Petal)

Whatever JSON you put in `value` becomes a Petal value:

| JSON | Petal | read in the script as |
|---|---|---|
| object | record | `v.field` |
| array | list | `v[i]`, `len(v)`, `for x in v do … end` |
| string | string | `v ++ "…"` |
| number | int or float | `v * 100` (a whole-number literal is an int; anything fractional is a float) |
| bool | bool | `if v then … end` |
| null | nil | `v ?? fallback` |

Numbers keep their kind: `7` arrives as an int, `0.42` and `3.0` as floats.
Send a ratio or rate as a real and the script gets a real. Watch integer
division in Rust: `json!(pct / 100)` on two integers reaches the script as
`0`. Divide in floating point (`pct as f64 / 100.0`).

A `query("log")` answer like

```rust
serde_json::json!({ "commits": [ { "hash": h, "short": s, "subject": subj } ] })
```

is read as `data.commits`, each with `c.hash`, `c.short`, `c.subject`. Design
the shapes to match exactly what the script reads.

Query args are JSON. The wire carries `arg` verbatim, so a composite key
(`{ "table": "users", "page": 3 }`) needs no string encoding and Garden caches
per `(kind, arg)`. The script-side `query` native currently passes a string,
which reaches your handler as a JSON string (`ctx.arg_str()`).

**Soft errors and nil fields.** A common convention is a soft `error` field in
an otherwise successful reply, for a non-fatal failure the drawer shows in
place. Report "no error" as `null`, not `""`, and read it null-safely in the
drawer, because any operation on nil aborts the frame and raises the error
card:

```petal
// WRONG: crashes the frame when the field is null
if len(data.error) > 0 then … end
// RIGHT: coalesce first (`??` treats nil and pending as absent)
let err = data.error ?? ""
if len(err) > 0 then … end
```

`garden --subprocess gpp-test-app runtime-error` reproduces the crash.

## Step 4: the UI script

The pushed script is an ordinary Petal graphical panel. The full draw, input,
and text API is in [petal-graphical-panels.md](petal-graphical-panels.md);
this section covers what a GPP drawer uses most.

**Per-frame model.** The whole script runs every frame; there is no retained
scene. Keep UI state in `state` variables, declared at the top level so each
is one cell for the whole panel. A `state` declared inside a function is keyed
by the call path that reached it, so each call site (and each iteration of a
loop around the call) gets its own cell. That is what you want for a reusable
row helper (`fn row(item) state hovered = false … end`), and not what you want
for a value two functions share. Key a per-item cell with `state(item.id)` if
it must follow the item through a reorder.

**Drawing** in panel-local pixels, colors as `#rrggbb`: `draw_rect(rect(x, y,
w, h), color)`, `draw_text(s, {x, y}, size, color)`, `draw_line`,
`draw_circle`, `clip(...)` for scroll regions. `screen_width()` and
`screen_height()` give the pane size.

**Host theme.** Call `palette()` once per frame and read colors off it. It
returns the host's current theme as `{ r, g, b, a }` records and is complete
even outside Garden (it overlays the host theme on a built-in fallback), so
read `P.<key>` directly with no guard:

```petal
let P = palette()
let BG = P.window_bg   let TEXT = P.text   let ADDBG = P.added_bg
```

| Key | Meaning |
|-----|---------|
| `window_bg` | window background (behind panes) |
| `panel`, `panel_focused` | pane/card background; paint a drawer card, and the backdrop behind a `text_view`, with this |
| `border`, `border_focused` | hairlines, dividers |
| `text`, `text_mut`, `text_dim`, `text_faint` | text, brightest to faintest |
| `cursor`, `accent` | caret; titles, active tab, focus ring |
| `focus`, `sel`, `hover` | focused, selected, hovered row fills (opaque) |
| `green`, `orange`, `red`, `purple`, `blue` | semantic accents tuned for the scheme's background |
| `error` | error text |
| `added_bg`, `removed_bg` | diff row fills |
| `hunk`, `hunk_bg`, `hunk_bg_hover` | diff hunk header and its band |
| `scrollbar_thumb`, `scrollbar_track` | scrollbar parts |

Each color carries `.a`, so `draw_rect(r, c, c.a)` paints with the theme's
alpha. A live theme switch recolors the drawer on the next frame. The
lower-level `panel_theme()` returns exactly what the host injected (an empty
record when nothing is); prefer `palette()`.

**Input**, delivered to the focused pane: `key_pressed("j")`, `mouse_x()`,
`mouse_pressed(0)`, `click_count()`, `drag_active()`, `scroll_y()`,
`text_input()`, `mod_shift()`, plus the `ui` prelude (`button`,
`list_update`, focus helpers). The `:` command line and global chords stay
with Garden. Return is spelled `"return"`, never `"enter"`.

**Data.** `query(kind, arg)` returns the value when ready, otherwise a pending
value you inspect with `is_ready`, `is_loading`, `is_error`, `error_of`, or
`??`. `invalidate(kind, arg)` drops a cached answer so the next `query`
re-requests; that is how you refresh. The per-frame rerun is the retry loop.

**Push (script to app).** `emit(event, arg)` sends a fire-and-forget
notification to your app. Garden drains emits once per frame tick, so guard
the call with an edge (a key press, a drag end); an unconditional `emit`
re-fires every frame:

```petal
if divider_drag && mouse_released(0) then
  emit("divider", { pos: divider_x })
end
```

**Asking Garden to act.** `mutate(name, arg)` with a host-owned name
(`open_path`, `open_project`, `open_pr`, `open_file_dialog`; see
[gpp.md](gpp.md)) is answered by Garden and never reaches your app. This is
how the directory browser opens a file. Every other name is forwarded to your
`on_mutation` handler, and `mutate_result(handle)` reads the outcome back.

**Selectable text.** `text_view(id, x, y, w, h, text)` embeds a real editor
region (highlight to select, Cmd-C, native scroll) instead of glyphs.
`text_view_line_styles(id, styles)` adds per-line colors (`"added"`,
`"removed"`, `"hunk"`, `"title"`, `"dim"`, `"comment"`);
`text_view_scroll_to(id, line)` is a one-shot action, so call it only on the
frame the navigation happens; `text_view_wrap(id, wrap)` is frame state, so
declare it every frame. Script-drawn `draw_text` cannot be selected or
copied, so route any body the user might copy through a `text_view`.

A region only consumes input it can act on. While its content overflows, it
owns the wheel over it and, once clicked, the nav keys; when the content fits,
both fall through to the script. Clicks are always teed: the region starts a
selection and the script still sees `mouse_pressed()`. Escape returns key
focus from a region to the script.

**Editable text.** `edit_view(id, x, y, w, h, seed)` embeds Garden's real vim
editor, seeded once; `edit_view_text(id)` reads the buffer back. See
[Editable panels](#editable-panels) below.

**Testing hook.** Nothing to call. Garden observes every named binding the
frame made and reports it at `/state` under `panes[].panel.values`. See
[Step 6](#step-6-verifying-it).

### A minimal drawer

A simplified version of `git-viewers/src/git_panel.ptl`: load a list once,
move a selection with the keyboard, and show a selectable, styled detail on
the right.

```petal
state sel = 0
state loaded = false
state commits = []

let P = palette()
let w = screen_width()
let h = screen_height()
draw_rect(rect(0, 0, w, h), P.panel)

// Load the list once; keep it in `state` after it lands.
if !loaded then
  let rd = query("log", "")
  if is_ready(rd) then commits = rd.commits; loaded = true
  elsif is_error(rd) then draw_text("error: " ++ (error_of(rd) ?? "?"), {x: 8, y: 8}, 14, P.error)
  else draw_text("loading…", {x: 8, y: 8}, 14, P.text_dim) end
end

let n = len(commits)
if key_pressed("j") || key_pressed("down") then sel = sel + 1 end
if key_pressed("k") || key_pressed("up") then sel = sel - 1 end
if sel < 0 then sel = 0 end
if n > 0 && sel >= n then sel = n - 1 end

for i in range(0, n) do
  let c = commits[i]
  let y = 8 + i * 20
  if i == sel then draw_rect(rect(0, y - 2, 380, 20), P.sel) end
  draw_text(c.short ++ "  " ++ c.subject, {x: 8, y: y}, 14, P.text)
end

// A second query, keyed by the selection: the diff for the selected row.
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

A selection change re-keys the second `query`; Garden requests the new diff,
caches it, and the next frame renders it. Nothing crosses the pipe per frame.

`sel`, `loaded`, and `commits` are top-level, so each is one cell for the
panel. Factor a drawer into helpers that take state as parameters and return
values, and keep the cells at the top.

### Persisting drawer UI state

Some drawer state should outlive a relaunch: a divider position, a sort
order, a collapsed flag. Two options:

- **`panel_store_get` / `panel_store_set`** keep small string values in
  Garden's per-panel store, keyed by the app's name. This needs no app code.
  See [petal-graphical-panels.md](petal-graphical-panels.md#persistence-panel_store_get--panel_store_set).
- **An `emit` round-trip** when the app should own the value: the drawer
  emits on an interaction edge (drag end, not every frame), an
  `on_emit` handler writes it to a file (temp file plus rename; failures
  are non-fatal, log and keep serving), and the app re-serves it through a
  dedicated `query("ui_state", "")` the drawer reads once at load.

`emit` is delivered on the next poll tick, so a test that relaunches to check
persistence should poll for the value rather than assume it landed
synchronously.

## Multi-screen navigation

An app can have more than one screen with browser-style back and forward.
Declare each navigable screen's source on the `PanelUi`; the declared set is
also the allowlist, so a `navigate` to an undeclared screen is refused:

```rust
const HOME: &str = include_str!("home.ptl");
const DETAIL: &str = include_str!("detail.ptl");

gpp::serve(
    provider,
    PanelUi::new("my-app", HOME).screen("detail.ptl", DETAIL),
)
```

A screen then navigates with the same natives as an in-process panel:
`navigate("detail.ptl")`, `navigate_replace(...)`, `navigate_back()`,
`navigate_forward()`. Garden's `Ctrl+[` / `Ctrl+]` and `:back` / `:forward`
drive the same history. Garden fetches the target's source through the
`navigate` request and owns the history stack and per-entry `state` restore.
`gpp-apps/screens-demo` is the worked example.

`navigate("detail.ptl", { id: 7 })` carries an argument. The target reads it
with `nav_arg()`, your handler sees it as `ctx.arg`, and back/forward restore
it with the entry. See
[petal-graphical-panels.md](petal-graphical-panels.md#navigating-navigatescreen--navigatescreen-arg).

An app that needs navigation side effects (log the visit, prime data for the
target) registers `on_navigate` instead. It replaces the declared-screens
lookup and returns the target's source, or an `Err` to refuse:

```rust
provider.on_navigate(|state: &mut S, ctx| {
    state.active_screen = ctx.screen.to_string();
    Ok(source_for(ctx.screen)?)
})
```

Back and forward re-issue the request with the same screen and `arg` the
original push used, so write the handler to be idempotent. The replay is best
effort: if your app is gone, slow (500 ms), or refuses, the entry stays on its
cached source and the reason appears in the pane's status note.

## Editable panels

Everything above is read-only. An app can also let the user edit and push
changes back with `mutate`. `gpp-apps/garden-diff` is the reference. The
pattern has three pieces. The one real choice is what a region sends back:
its text (shown here, the simpler case) when the region is a picture of the
thing being edited, or the edits its projection resolved
([Editable projections](#editable-projections)) when it is not.

1. **Draw an `edit_view` region**, seeded once from a `state` variable so the
   seed is not recomputed every frame:

   ```petal
   edit_view(1, x, y, col_w, body_h, seed)   // real vim; click to focus, edit
   ```

2. **On an edit-commit edge, read the buffer and `mutate`.** `mutate` is a
   request whose result is never cached and which, unlike `emit`, returns a
   value. Guard it with an edge such as a key chord:

   ```petal
   if mod_ctrl() && key_pressed("s") then
     mutate("save", { text: edit_view_text(1) })
     invalidate("doc", "")   // drop the cached data so the panes re-read post-save
     ready = false
   end
   ```

3. **Handle the mutation in the app** with `on_mutation`. Do the effect and
   return a `Reply`: a status string Garden shows in the status bar, or
   `Reply::error(..)`. Validate before writing: a text write-back assumes the
   buffer still has the shape you projected, so check that before touching a
   file.

   ```rust
   provider.on_mutation("save", |state: &mut State, ctx| {
       let text = ctx.arg["text"].as_str().unwrap_or_default().to_string();
       match write_back(state, &text) {
           Ok(files) => Reply::json(format!("wrote {} files", files.len())),
           Err(e) => Reply::error(e),
       }
   });
   ```

**Region lifetime.** A region's editor state is pruned when it is not declared
for a frame (for example, a mode switch hides it). To keep unsaved edits
across that, snapshot `edit_view_text(id)` into your seed `state` at the
transition. A region carrying a projection cannot do this: re-seeding from
edited text would leave the origin table out of step. Declare a projected
region from the projected text every frame (Garden rebuilds the buffer only
when that text changes) and accept that hiding it discards unsaved edits, as
garden-diff's two editable views do.

## Editable projections

Sending the region's text back works when the region is a picture of the
thing being edited. It stops working when the region mixes sources: a unified
diff shows a file's current lines interleaved with base lines it dropped, so
the text is not a picture of any file, and recovering the user's intent from
it means guessing.

Declare a projection instead: say where each line came from, once, and Garden
tracks it through every edit and hands you the resulting write-backs.

```petal
edit_view(5, x, y, w, h, doc.unified.text)
edit_view_projection(5, doc.unified.projection)
```

The spec is a record of parallel arrays, cheap for a drawer to pass straight
through from its app:

| Field | Meaning |
|---|---|
| `sources` | opaque names the write-backs are addressed to (file paths, say) |
| `span_source`, `span_start`, `span_end`, `span_group` | per editable span: which source it writes to, the `[start, end)` line range it replaces, and a grouping key (`-1` = none) so several spans revert together |
| `kinds` | one character per projected line naming its origin (below) |
| `line_spans` | the span each line belongs to (`-1` = none), parallel to `kinds` |
| `styles` | the style name each line is painted with, parallel to `kinds` |
| `decor` | `{ same, added, removed, same_style, added_style, removed_style, diff_markers, gutter }` |

`decor.gutter` decides where the `+`/`-`/` ` markers live. With it false (the
default) they are the first character of each line's text, and Garden strips
them when folding back. With it true they are display only: the buffer holds
the sources' own text and Garden draws the glyph in a gutter column.

Prefer `gutter: true` for anything a user will edit. With markers in the text,
every buffer operation is one character out of step with the content (`J`
joins `+one` and `+two` into `+one +two`; `0` lands on a `+`; a search has to
match past it). With `gutter: true` there is nothing in the buffer to step
around. It also turns off `diff_markers` and stops autoindent copying a marker
into a new line.

The origin alphabet:

| Char | Origin | Deleting the line means |
|---|---|---|
| `' '` | content the source holds unchanged | remove it from the source |
| `'+'` | content added relative to the base | drop that addition |
| `'-'` | content the base held and the source dropped | revert that deletion; the base line comes back |
| `'c'` | chrome (inert decoration) | nothing |
| `'l'` | chrome, locked | refused, with a status message |
| `'h'` | chrome heading its span | revert the whole span |
| `'g'` | chrome heading its group | revert every span in the group |

Two things this buys you. All of vim works (`dd`, `3dd`, `cc`, `V}d`, `p`,
`.`, undo, redo) because Garden reports each buffer mutation to the projection
rather than re-deriving intent afterwards, and undo restores the origins too.
And styles stop drifting: the bands ride the origin table, so a projected
region needs no `text_view_line_styles`.

Then save the edits rather than the text:

```petal
if mod_ctrl() && key_pressed("s") then
  mutate("apply", { edits: edit_view_edits(5) })
  invalidate("doc", "")
  ready = false
end
```

`edit_view_edits(id)` is a list of `{source, start, end, lines}`: replace
lines `[start, end)` of `source` with `lines`. The app applies them:

```rust
provider.on_mutation("apply", |state: &mut State, ctx| {
    match parse_edits(&ctx.arg).and_then(diff_core::apply_edits) {
        Ok(files) => Reply::json(format!("wrote {} files", files.len())),
        Err(e) => Reply::error(e),
    }
});
```

Declare the spec every frame, like the region itself. Garden fingerprints it
and rebuilds only when it genuinely changes.

## Step 5: launching it

A `process` node in a layout script spawns the app:

```petal
layout(process("/abs/path/to/target/debug/my-app", ["/some/dir"]))
```

A bare command name is resolved on `$PATH`; during development use an
absolute path. The `args` list becomes `InitializeParams::args`, and the
pane's `cwd` is passed too. (Garden's own clients get a sibling-of-`garden`
resolver when launched via `:E`, `:Git`, `:Diff`, `:PR`; a generic
`process(...)` does not.) `garden --subprocess <app> [args…]` runs any client
as the whole layout.

The pane renders and handles input as a panel but persists as a
`process(...)` node: a reload, split, or window restore re-spawns your app,
which re-pushes its script.

## Step 6: verifying it

Drive it headlessly over the [debug server](debug-server.md):

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
command, an incrementing `frame`, and a `values` object holding whatever the
script bound this frame, including data that came back over `query`. The
drawer above needs no publishing step for a test to read its `sel`:

```bash
curl -s localhost:8080/state | jq '.panes[0].panel.values.sel'    # 0
curl -s -X POST localhost:8080/key -d '{"key":"j"}'
curl -s localhost:8080/state | jq '.panes[0].panel.values.sel'    # 1
```

Keys are function-qualified (a `let y` inside `fn draw_row` reads as
`draw_row.y`, reporting its last call) and values keep their real types. A
binding that never executed is absent rather than null.

Two habits this rewards:

- **Name the values a test needs.** A click target computed inline into
  `draw_rect(...)` is invisible; `let row_y = …` lets a test click by geometry
  it reads instead of hardcodes.
- **Don't bind what you don't want dumped.** A name holding a 200 KB body puts
  200 KB into every `/state` response; bind `let body_len = len(body)` when
  the length is the assertable part.

One-frame edges (`mouse_released`, `click_count`, `scroll_y`, `text_input`)
are cleared by the next idle tick, so count them into a `state` variable
rather than sampling them. The integration scripts under `tools/` all assert
on `values` this way; see [testing.md](testing.md).

## Gotchas

- **The pushed script runs in-process in Garden.** A runaway script
  (`while true`) hangs the editor. GPP apps are trusted code, not sandboxed
  downloads. The app process is isolated; the script is not.
- **The host/app contract is wide.** Your script targets Garden's Petal and
  panel-native version, like browser compatibility. Fine within this repo;
  keep it in mind before shipping external apps.
- **`emit` needs a subprocess.** In a plain in-process `panel(script)` pane
  there is no client to deliver to, so emits are silently dropped. The
  host-owned `mutate` names work everywhere.
- **`query` latency is about one poll tick** (~200 ms) in steady state. The
  first frame is primed synchronously, so a freshly opened app paints with
  data rather than a spinner.

## Checklist

1. A workspace-member crate depending on `petal-query` and `serde_json`.
2. `src/ui.ptl`: the drawer, embedded with `include_str!`.
3. `src/main.rs`: build a `Provider`, register a `.query(kind, …)` per kind
   (each returning a `Reply` with a cache policy), plus `.on_mutation`,
   `.on_emit`, `.on_navigate` as needed, and hand it to `gpp::serve` with a
   `PanelUi`.
4. Define the `(kind, arg)` to JSON shapes to match what the script reads.
5. `cargo build`; launch via `process("/abs/path", [args])`; verify over the
   debug server.
