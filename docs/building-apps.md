# Building Apps

Tech outline to build a full application on Petal.

1. **Write a pure Petal app** — just writing `.ptl` scripts. Uses an existing app or integration.
2. **Extend an integration** — depend on an existing host as a library and add
   your app's delta (a custom renderer, extra native functions, an editor shell).
3. **Embed Petal in a new host** — write a new integration for your platform,
   building on the `petal` and `petal-ui` crates directly (see also
   [ffi.md](ffi.md) for the embedding API).

This doc explains the layering that makes those paths work and walks through
the concrete mechanics of each, using the apps in this repo as worked examples.

## The three tiers

Petal is layered so each tier depends only on the tier above it:

```
Petal Core  →  Integrations  →  Sample Apps
```

| Tier | What lives here | Crates / packages |
|------|-----------------|-------------------|
| **Petal Core** | The language (compiler, IR, evaluator, bytecode VM) and the shared interactivity layer (normalized input, the draw-command vocabulary, the `ui` prelude). | [`rust/`](../rust/) (`petal`), [`petal-ui/`](../petal-ui/) (`petal-ui`) |
| **Integrations** | Reusable *hosts* that embed Petal Core for one platform. Own platform *policy* (windowing, event loop, rasterization, file IO). | [`integrations/petal-desktop-sdl`](../integrations/petal-desktop-sdl/) (native SDL2), [`integrations/petal-web-canvas`](../integrations/petal-web-canvas/) (WASM + canvas), [`integrations/petal-web-html`](../integrations/petal-web-html/) (WASM + DOM) |
| **Apps** | Your programs, built on top of an integration. The [`sample-apps/`](../sample-apps/) directory holds worked examples. | [`sample-apps/`](../sample-apps/) |

**The rule:** an app depends on an integration (and, through it, on Core). It
should not re-implement host code an integration already provides, and it
should not embed Petal Core directly when an integration for its platform
exists.

## Two shapes of app

Pick the lightest shape that works.

### Shape A — Pure-Petal app (no host code)

Your app is only `.ptl` files (plus assets and a launch script). It runs on an
integration's **existing binary/host unchanged**; all app logic is in Petal.
This is the ideal — zero host code to maintain.

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

Your app needs host capabilities the integration doesn't provide — a different
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
TypeScript host, wired together by Vite. Apps consume them as an npm package —
in this repo, through an **npm workspace**; an out-of-tree app can depend on
the integration package the same way.

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

### Building (web)

- `npm ci` at the **repo root** installs the whole workspace.
- Build the integration's WASM first, then your app:
  ```bash
  npm run build:wasm --workspace integrations/petal-web-canvas
  npm run build      --workspace integrations/petal-web-canvas
  npm run build      --workspace sample-apps/diagram-canvas
  ```

## Mechanism: Desktop (Rust + SDL) — the SDL track

`petal-desktop-sdl` is a lib + bin crate. The library is the reusable host; the
`petal-sdl` binary is a thin CLI over it. Both sample apps build on it:

- `side-scroller` is **Shape A** — it launches the binary unchanged.
- `petal-fps` is **Shape B** — it depends on the library and adds only its
  delta (a software-framebuffer 3D rasterizer and the `triangle3d` native
  family): one small `Host` impl plus its rasterizer and font.

### The design: one `Host` trait over a generic loop

**1. lib + bin split.** The crate has a `lib.rs`; `main.rs` is a thin CLI:

```toml
# integrations/petal-desktop-sdl/Cargo.toml
[lib]
name = "petal_sdl"
path = "src/lib.rs"

[[bin]]
name = "petal-sdl"
path = "src/main.rs"
```

The library owns everything reusable: `GameConfig`, the run entry points
(`run_game`/`run_agent`/`run_headless`/`run_screenshot`/`run_record`), SDL event
→ `petal_ui` input translation, the agent JSON protocol, PNG encoding, the
hot-reload watcher, and the font ladder + SDL-canvas renderer.

**2. The `Host` trait is the app seam.** Rather than two narrow traits, one
`Host` bundles the axes apps actually vary — the natives a script can call, how
a live frame is painted, and how a frame is captured to pixels/JSON — with inert
defaults for the rest (browser hooks, per-frame prep, draw stats):

```rust
pub trait Host {
    fn register(&mut self, env: &mut Env);                       // natives + prelude
    fn present(&mut self, canvas: &mut Canvas<Window>, env: &mut Env) -> Result<(), String>;
    fn render_image(&mut self, env: &mut Env, stack: StackKey, w: u32, h: u32)
        -> Result<RgbImage, String>;                             // screenshot/record/agent
    // + default-inert hooks: default_source, on_program_loaded, prepare_frame,
    //   draw_commands_json, draw_stats, on_escape, after_frame
}
```

The generic loop drives it: poll events → `input.begin_frame(dt)` → bind
`frame_info`/`input` → `env.run` → `host.present`. `DefaultHost` (the binary)
renders `petal-ui` draw commands to an SDL canvas and adds the example browser +
file I/O; `FpsHost` renders its framebuffer through a streaming texture and
registers its 3D natives. Both leave the loop untouched.

**3. The frame contract is identical to the web hosts'**, so behavior is
portable:

```text
poll events → input.begin_frame(dt) → bind frame_info/input → env.run → host.present
```

### Extend the shared layer, don't special-case the app

Example: `petal-fps` needed relative-mouse deltas (mouselook) and pointer grab —
neither existed in `petal-ui`. Because those are generally useful (any
pointer-locked game wants them), they went **into `petal-ui`**, not into the
app: an `InputEvent::MouseRelative`, `mouse_dx()`/`mouse_dy()` natives, and
`grab_mouse()`/`release_mouse()` with a `take_mouse_grab` drain the loop honors
via SDL relative-mouse mode. Every host — web included — now gets them for free.
The general rule: a capability an app needs belongs in the layer below it
(§"Extension hooks, not forks").

### What is shared vs. custom in petal-fps (scope guide)

| Concern | Shared (`petal-sdl` lib / `petal-ui`) | Custom (stays in `petal-fps`) |
|---------|----------------------------------|-------------------------------|
| Window + event loop | ✅ | |
| SDL event → input translation (incl. relative mouse) | ✅ | |
| Agent protocol / headless / screenshot / record | ✅ | |
| Hot-reload file watcher | ✅ | |
| Input / timing / grab natives | ✅ (`petal-ui`) | |
| Renderer | default SDL-canvas impl (`DefaultHost`) | framebuffer + 3D rasterizer (`FpsHost::present`/`render_image`) |
| Draw native functions | default `petal-ui` set | `triangle3d`/`sky_gradient`/… (`native_fns`) |
| Camera / projection / scene | | ✅ (in Petal) |

### Building (desktop)

- The crates are standalone (`cargo build --manifest-path <crate>/Cargo.toml`),
  not a Cargo workspace. A Shape B app carries a path (or git) dependency on
  the integration — e.g. `petal-fps` declares
  `petal-sdl = { path = "../../integrations/petal-desktop-sdl" }` and
  `petal-ui = { path = "../../petal-ui" }` — and building the app builds the
  library transitively.
- Building either crate needs SDL2; on Homebrew macOS, set
  `LIBRARY_PATH=/opt/homebrew/lib` for the linker (see the petal-sdl notes).

## Choosing an approach for a new app — checklist

1. **Does an integration for this platform already exist?**
   - Yes, and it exposes every native you need → **Shape A**: write `.ptl` + a
     launch script. Stop.
   - Yes, but you need custom host code → **Shape B**: depend on the integration
     (npm package for web; `path`/git lib dep for Rust) and add only the delta.
   - No integration for the platform → you are writing a *new integration*, not
     an app. Build on the `petal` + `petal-ui` crates, model it on an existing
     integration, and see [ffi.md](ffi.md) for the embedding API (natives,
     values, host channels, the per-frame contract).
2. **Is a capability you need generally useful?** Add it to the integration
   (or to `petal-ui`, if it's cross-platform — e.g. a new draw command belongs
   in `petal-ui/src/draw.rs`, not one host). Don't special-case it in the app.
3. **Can the app's influence on the host be an inert hook?** If yes, add the
   hook to the integration. If no, the capability probably belongs in the
   integration for all consumers.

## Verifying your app

Build proves compilation; it does not prove the app still *renders*. Exercise
the real runtime after any structural change:

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

A subtle regression to watch for: capabilities that only your app exercises.
If a code path in the integration only ever runs when your app drives it, test
it from your app — and when you find a bug there, fix it in the integration
(where every consumer benefits), not with a workaround in the app.

## Related docs

- [FFI / Embedding](ffi.md) — the Rust embedding API: registering natives, the value model, host channels.
- [Architecture](dev/Architecture.md) — Core internals (IR, evaluator, state).
- [petal-ui](../petal-ui/) — the shared input/draw/prelude contract every host implements.
- [petal-desktop-sdl agent protocol](../integrations/petal-desktop-sdl/docs/agent-protocol.md) and [game-dev guide](../integrations/petal-desktop-sdl/docs/game-dev-guide.md).
- [Debug protocol](dev/debug-protocol.md) — shared by petal-sdl and petal-diagram-canvas.
