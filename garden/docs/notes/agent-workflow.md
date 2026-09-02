# Working in this repo

Conventions that aren't visible from the code itself.

## Verifying a change

Prefer headless plus the debug server (`--headless --debug-port <port>`,
protocol in [`../debug-server.md`](../debug-server.md)): no window opens,
nothing steals desktop focus, no screen-recording permission needed. The same
endpoints exist in windowed mode, but there real input interleaves with
injected input, so re-read `/state` after acting instead of assuming earlier
state holds.

**Launch it with an idle timeout, and kill it when you are done:**

```bash
GARDEN_HEADLESS_IDLE_TIMEOUT=1800 garden --headless --debug-port 8080 &
```

A headless app has no window and no terminal, so a forgotten one keeps running
and holding its port. The timeout is the backstop; see
[debug-server.md](../debug-server.md#when-a-headless-run-stops-by-itself) for
the details. `ps -eo pid,ppid,command | grep '[g]arden --headless'` lists what
is still up; a ppid of 1 means nobody owns it. The test harness sets the
timeout for you (`tools/lib/app.ts`).

The layered strategy (what is unit-tested vs. driven end-to-end) is in
[`../testing.md`](../testing.md). The pure modules listed there must stay
unit-tested.

## Where new behavior goes

The app runs behind three pluggable frontends (`garden-app/src/frontend/`:
windowed, `--term`, `--headless`). Behavior belongs in the frontend-independent
core (`garden-app/src/app/`), not in a frontend.

## When you add a Petal native fn

Document it in `../architecture.md` (layout natives) or
`../petal-graphical-panels.md` (panel natives). The Petal-side API surface
lives in `garden-script`.

## Traps that have cost real time

- **Color space.** `Color` is sRGB everywhere, including in the GPU buffers:
  the render target deliberately has no transfer function, so blending happens
  in the gamma-encoded space CSS blends in. Reintroducing a linearization step,
  or picking an `…Srgb` target format, silently lightens every translucent fill
  and pulls glyph color away from shape color. Details:
  [`../architecture.md`](../architecture.md#garden-render--gpu-renderer).
- **Layout is editable state, and the live panes are its source of truth.**
  Runtime rearrangements and out-of-band content changes are persisted by
  rebuilding the tree from the live panes and rewriting the `layout(...)` call.
  Details: [`../architecture.md`](../architecture.md#layout-as-editable-state).
- Paths written as `../<name>` in Garden's Rust comments mean siblings of the
  `garden/` directory: the Petal crates and docs at the repo root.

## Upstream

Garden and Petal live in one repo, so embedding friction in Petal itself is
fixed in `../../rust` / `../../petal-ui` rather than filed and deferred.
