# Building Apps on Core and Integrations

This doc describes how apps in this repo are layered on top of Petal, and the
concrete local mechanisms for building a new one. It is the reference for
routing the remaining sample apps through their integrations (the "SDL track"
and any future ports).

## The three tiers

Petal is layered so each tier depends only on the tier above it:

```
Petal Core  →  Integrations  →  Sample Apps
```

| Tier | What lives here | Crates / packages |
|------|-----------------|-------------------|
| **Petal Core** | The language (compiler, IR, evaluator, bytecode VM) and the shared interactivity layer (normalized input, the draw-command vocabulary, the `ui` prelude). | [`rust/`](../rust/) (`petal`), [`petal-ui/`](../petal-ui/) (`petal-ui`) |
| **Integrations** | Reusable *hosts* that embed Petal Core for one platform. Own platform *policy* (windowing, event loop, rasterization, file IO). | [`integrations/petal-desktop-sdl`](../integrations/petal-desktop-sdl/) (native SDL2), [`integrations/petal-web-canvas`](../integrations/petal-web-canvas/) (WASM + canvas), [`integrations/petal-web-html`](../integrations/petal-web-html/) (WASM + DOM) |
| **Sample Apps** | Example programs built on top of an integration. | [`sample-apps/`](../sample-apps/) |

**The rule:** a sample app depends on an integration (and, through it, on Core).
It must not re-implement host code that an integration already provides, and it
must not embed Petal Core directly when an integration for its platform exists.

### Why the rule matters

The failure mode we are correcting is **fork-and-drift**: an app starts as a
copy of an integration's host code, then both evolve independently. The
integration gains fixes and features; the copy goes stale and re-introduces
bugs the integration already fixed. Concretely, `diagram-canvas` carried a
609-line copy of `petal-web-canvas`'s pre-`petal-ui` runtime — including a
press-edge input bug that `petal-web-canvas` had already fixed. Depending on the
integration instead of copying it deleted ~1,400 lines and made that class of
drift impossible.

## Two shapes of sample app

Pick the lightest shape that works.

### Shape A — Pure-Petal app (no host code)

The app is only `.ptl` files (plus assets and a launch script). It runs on an
integration's **existing binary/host unchanged**; all app logic is in Petal.
This is the ideal — zero host code to drift.

`sample-apps/side-scroller` is the model. It has no Rust and no TS: `game.ptl`,
`editor.ptl`, level files, and a launch script that points the `petal-sdl`
binary at its script:

```bash
# sample-apps/side-scroller/run-game.sh (abridged)
SDL_DIR="$REPO/integrations/petal-desktop-sdl"
BIN="$SDL_DIR/target/debug/petal-sdl"
[ -x "$BIN" ] || ( cd "$SDL_DIR" && cargo build )   # build the integration if needed
exec "$BIN" side-scroller/game.ptl --width 960 --height 600 --title "Petal Runner"
```

**Choose Shape A when** the integration already exposes every native the app
needs (drawing, input, timing, and whatever app-specific natives it registers).
Most 2D games and canvas sketches fit here.

### Shape B — App that extends an integration (thin host delta)

The app needs host capabilities the integration doesn't provide — a different
renderer, extra native functions, an editor/debug shell. It **depends on the
integration as a library/package** and adds only its delta on top. It never
copies the integration's shared code.

`sample-apps/diagram-canvas` is the model on the web side: it consumes
`petal-web-canvas` for the WASM runtime, canvas renderer, and input plumbing,
and adds only a CodeMirror editor and a pause/step debug protocol.

**Choose Shape B when** you need custom host code, but design it so the *shared*
part stays in the integration and only the *specific* part lives in the app.

## Mechanism: Web (WASM + TypeScript)

Web integrations are a Rust WASM crate (built with `wasm-pack`) plus a
TypeScript host, wired together by Vite. Sample apps consume them through an
**npm workspace**.

### How `diagram-canvas` consumes `petal-web-canvas`

**1. Workspace wiring.** The root `package.json` lists both the integration and
the app as workspaces, so the integration is symlinked into `node_modules` and
importable by name:

```jsonc
// package.json (repo root)
"workspaces": [
  "integrations/petal-web-canvas",
  "sample-apps/diagram-canvas"
]
```

**2. The integration publishes an entry point.** It adds an `exports` map and a
barrel `src/index.ts`. The `pkg/*` subpath exposes the built WASM so consumers
get it transitively:

```jsonc
// integrations/petal-web-canvas/package.json
"exports": {
  ".": "./src/index.ts",
  "./pkg/*": "./pkg/*"
}
```

```ts
// integrations/petal-web-canvas/src/index.ts
export { PetalCanvas } from "./runtime.js";
export { renderCommands } from "./canvas-renderer.js";
export { InputTracker } from "./input.js";
export { default as initRuntime, PetalRuntime } from "../pkg/petal_web_canvas.js";
```

**3. The app depends on it and imports by name.** No relative reach across the
tree:

```jsonc
// sample-apps/diagram-canvas/package.json
"dependencies": { "petal-web-canvas": "*" }
```

```ts
// sample-apps/diagram-canvas/src/main.ts
import { PetalCanvas } from "petal-web-canvas";
```

The app keeps **no** WASM crate, renderer, or input code of its own. Vite
(with `vite-plugin-wasm`) transpiles the linked workspace source and bundles the
integration's `.wasm` transitively — no extra config was needed.

### Extension hooks, not forks

When the app needs to influence the integration's frame loop, add small,
**backward-compatible, default-inert hooks** to the integration rather than
letting the app fork the loop. `diagram-canvas` needs pause/step and a
per-frame callback, so `PetalCanvas` grew:

```ts
// on the shared PetalCanvas — no-ops unless a host sets them
frameGate: ((realDt: number) => number | null) | null = null;  // return null to skip a frame
onFrameComplete: (() => void) | null = null;
runOneFrame(dt: number): string { /* drive one frame, bypassing the gate */ }
stop(): void { /* halt the rAF loop */ }
```

The app wires them from the outside; `petal-web-canvas`'s own app leaves them
unset and behaves exactly as before:

```ts
const debug = new DebugController();
const petal = new PetalCanvas();
petal.frameGate = (dt) => debug.shouldRunFrame(dt);   // pause/step gating
petal.onFrameComplete = () => panel.refreshIfVisible();
```

Guideline: a hook must be a no-op when unset, and must not change the
integration app's behavior. If you can't express the app's need as an inert
hook, that's a signal the capability belongs *in* the integration for everyone.

### Build & CI (web)

- `npm ci` at the **repo root** installs the whole workspace.
- Build the integration's WASM first, then the apps:
  ```bash
  npm run build:wasm --workspace integrations/petal-web-canvas
  npm run build      --workspace integrations/petal-web-canvas
  npm run build      --workspace sample-apps/diagram-canvas
  ```
- In CI, do a single root `npm ci` (not per-subdir) and drive builds with
  `--workspace`. See `.github/workflows/ci.yml` (`web-builds` job).
- An integration that is *not* consumed by any workspace app (e.g.
  `petal-web-html` today) can stay outside the workspace list and keep its own
  lockfile + per-dir `npm ci`.

## Mechanism: Desktop (Rust + SDL) — the SDL track

`petal-desktop-sdl` ships the `petal-sdl` **binary**. Today:

- `side-scroller` is **Shape A** — it uses the binary unchanged. Done, correct.
- `petal-fps` is **Shape B done wrong** — it depends on Petal Core (`petal`)
  directly and carries its own copy of the host (`game_loop`, `input`,
  `protocol`, `screenshot`, `font`, `main`). It is genuinely different (a
  software-framebuffer 3D rasterizer with `triangle3d`/`sky_gradient`/z-buffer
  natives and relative-mouse input — it doesn't even use `petal-ui`), but it
  still duplicates all the *scaffolding* it doesn't customize.

The fix is the same principle as the web track: make the integration a
**library** the app can depend on, and reduce the app to its delta.

### Recommended design

**1. Split `petal-desktop-sdl` into lib + bin.** Give the crate a `lib.rs` and
keep `main.rs` as a thin CLI over it:

```toml
# integrations/petal-desktop-sdl/Cargo.toml
[lib]
name = "petal_sdl"
path = "src/lib.rs"

[[bin]]
name = "petal-sdl"
path = "src/main.rs"
```

The library exposes the reusable host: `GameConfig`, the run entry points
(`run_game`/`run_agent`/`run_headless`/`run_screenshot`), SDL event → `petal_ui`
input translation, the agent JSON protocol, screenshot/PNG encoding, the file
watcher (hot reload), and font/text rendering. `side-scroller` is unaffected —
it still launches the binary.

**2. Give the host an extension seam for the parts apps customize.** The two
axes `petal-fps` varies are the **renderer** and the **native-function set**.
Model them as traits the host is generic over (default impls = today's behavior):

```rust
/// What the game loop calls to paint a frame. The default SDL-canvas renderer
/// consumes petal-ui draw commands; petal-fps supplies a framebuffer renderer.
pub trait FrameRenderer {
    fn present(&mut self, env: &mut Env) -> Result<(), String>;
}

/// Natives + prelude a host registers. The default registers the petal-ui set;
/// petal-fps registers its 3D natives instead of (or alongside) it.
pub trait HostNatives {
    fn register(&self, env: &mut Env);
}
```

`run_game(source, config, renderer, natives)` (or a `HostApp` struct bundling
them) then drives the shared loop — event poll, `input.begin_frame(dt)`, bind
uniforms, `env.run`, `renderer.present` — for any host. `petal-fps` becomes a
small crate that depends on `petal-sdl`, implements `FrameRenderer` (its
framebuffer + streaming texture) and `HostNatives` (its `triangle3d` family),
and keeps its 3D math — but drops the copied `game_loop`/`protocol`/
`screenshot`/`input`/`main` scaffolding.

**3. Keep the frame contract identical to petal-ui's.** Even a custom renderer
should follow the same order the web hosts use, so behavior is portable:

```text
poll events → input.begin_frame(dt) → bind frame_info/input → env.run → renderer.present
```

### What is shared vs. custom in petal-fps (scope guide)

| Concern | Shared (move to `petal-sdl` lib) | Custom (stays in `petal-fps`) |
|---------|----------------------------------|-------------------------------|
| Window + event loop | ✅ | |
| SDL event → input translation | ✅ (relative-mouse can be a config flag) | |
| Agent protocol / headless / screenshot / record | ✅ | |
| Hot-reload file watcher | ✅ | |
| Renderer | default SDL-canvas impl | framebuffer + 3D rasterizer (`FrameRenderer`) |
| Native functions | default petal-ui set | `triangle3d`/`sky_gradient`/… (`HostNatives`) |
| Camera / projection / scene | | ✅ (already in Petal) |

### Build & CI (desktop)

- The crates are standalone (`cargo build --manifest-path <crate>/Cargo.toml`),
  not a Cargo workspace. `petal-fps` gains `petal-sdl = { path = "../../integrations/petal-desktop-sdl" }`.
- Building `petal-desktop-sdl` needs SDL2; on this machine set
  `LIBRARY_PATH=/opt/homebrew/lib` for the linker (see the petal-sdl notes).
- CI: `petal-fps` builds under the `rust-subprojects` job; keep it there.

## Choosing an approach for a new port — checklist

1. **Does an integration for this platform already exist?**
   - Yes, and it exposes every native you need → **Shape A**: write `.ptl` + a
     launch script. Stop.
   - Yes, but you need custom host code → **Shape B**: depend on the integration
     (workspace package for web; `path` lib dep for Rust) and add only the delta.
   - No integration for the platform → you are writing a *new integration*, not a
     sample app. Put it in `integrations/`, build on `petal` + `petal-ui`, and
     model it on an existing integration.
2. **Is a capability you need generally useful?** Add it to the integration
   (or to `petal-ui`, if it's cross-platform — e.g. a new draw command belongs
   in `petal-ui/src/draw.rs`, not one host). Don't special-case it in the app.
3. **Can the app's influence on the host be an inert hook?** If yes, add the
   hook to the integration. If no, the capability probably belongs in the
   integration for all consumers.
4. **Delete, don't leave stubs.** When routing an app through an integration,
   remove the duplicated crate/files outright and fix every dangling reference
   (build scripts, CI cache paths and steps, READMEs, `.gitignore`).

## Verifying a port

Build proves compilation; it does not prove the app still *renders*. Exercise
the real runtime after any routing change:

- **petal-web-canvas / diagram-canvas** — `npm run dev --workspace <app>`, open
  the page, and drive it. `diagram-canvas` exposes a debug WebSocket that the
  `petal-diagram-canvas` MCP tools speak (`DiagramStep`, `DiagramScreenshot`,
  `DiagramCaptureDrawCommands`, `DiagramState`); use them to confirm live
  frames, speculative capture, and state introspection. Note: `requestAnimationFrame`
  is throttled in a backgrounded tab, so a live frame counter may sit at 0 in
  headless checks — drive frames with `DiagramStep` to bypass it.
- **petal-desktop-sdl / SDL apps** — run headless/agent mode over the stdin
  JSON protocol, or `--screenshot out.png` to render N frames and write a PNG.
  See [agent-protocol.md](../integrations/petal-desktop-sdl/docs/agent-protocol.md).

A subtle regression to watch for: capabilities that only a sample app exercised.
Routing `diagram-canvas` through `petal-web-canvas` surfaced that
`PetalRuntime.run_speculative` returned no draw commands — it used core
`Env::run_speculative`, which drops the fork before its draw buffer can be read.
The fix (drain the fork with `petal_ui::draw::take_draw_commands_for` before
dropping it) belonged in the integration, and now benefits every consumer. Fold
such fixes into the integration, not the app.

## Related docs

- [Architecture](dev/Architecture.md) — Core internals (IR, evaluator, state).
- [petal-ui](../petal-ui/) — the shared input/draw/prelude contract every host implements.
- [petal-desktop-sdl agent protocol](../integrations/petal-desktop-sdl/docs/agent-protocol.md) and [game-dev guide](../integrations/petal-desktop-sdl/docs/game-dev-guide.md).
- [Debug protocol](dev/debug-protocol.md) — shared by petal-sdl and petal-diagram-canvas.
