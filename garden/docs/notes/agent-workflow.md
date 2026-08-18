# Working in this repo

Conventions that aren't visible from the code itself.

## Verifying a change

Prefer **headless + the debug server** (`--headless --debug-port <port>`, protocol
in `../debug-server.md`): no window opens, nothing steals desktop focus, no
osascript or screen-recording permission needed. The same endpoints exist in
windowed mode, but there real input interleaves with injected input — re-read
`/state` after acting instead of assuming earlier state holds.

**Launch it with an idle timeout, and kill it when you are done:**

```bash
GARDEN_HEADLESS_IDLE_TIMEOUT=1800 garden --headless --debug-port 8080 &
```

A headless app has no window and no terminal, so one you forget about — or one
whose session is killed out from under it — keeps running and holding its port
indefinitely. Garden exits when it is reparented to pid 1, which catches most of
it, but not a launcher that exited before that pid could be sampled (backgrounding
from a `sh -c` is enough), so the timeout is the backstop. It is off by default,
because an idle session is not necessarily an abandoned one. `ps -eo
pid,ppid,command | grep '[g]arden --headless'` lists what is still up; a ppid of
1 means nobody owns it. The test harness sets the timeout for you
(`tools/lib/app.ts`).

Layered strategy (what is unit-tested vs. driven end-to-end) is in
`../testing.md`. The pure modules listed there must stay unit-tested.

## Where new behavior goes

The app runs behind three pluggable frontends (`garden-app/src/frontend/`:
windowed, `--term`, `--headless`). Behavior belongs in the frontend-independent
core (`garden-app/src/app/`), not in a frontend.

## When you add a Petal native fn

Document it in `../architecture.md` **and** the README example. The Petal-side
API surface lives in `garden-script`.

## Traps that have cost real time

- **Color space.** `Color` is sRGB everywhere in the app; `garden-render`
  converts to linear internally because the surface is sRGB. Writing raw
  components into GPU buffers without `Color::to_linear` washes dark colors out
  ~5x lighter. Details: `../architecture.md` ("Color space").
- **Layout is editable state, and the live panes are its source of truth.**
  Runtime rearrangements and out-of-band content changes are persisted by
  rebuilding the tree from the live panes and rewriting the `layout(...)` call.
  Details: `../architecture.md` ("Layout as editable state").
- Paths written as `../<name>` in this repo's docs and comments mean *siblings
  of the `garden/` directory* — the Petal crates and docs at the repo root.

## Upstream

Garden and Petal live in one repo, so embedding friction in Petal itself is
fixed in `../../rust` / `../../petal-ui` rather than filed and deferred.
