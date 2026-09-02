# petal-diagram-canvas

A browser sample app that renders Petal programs as interactive diagrams on an
HTML5 canvas, with a live source editor beside them.

It is built on the [`petal-web-canvas`](../../../integrations/petal-web-canvas/)
integration, which supplies the WASM runtime, the canvas renderer, and browser
input (all on top of the shared `petal-ui` layer). This app adds only its own
shell: the CodeMirror editor and a pause/step debug protocol. It has no Rust
or WASM code of its own.

## Prerequisites

- Node.js 18 or newer

The WASM runtime comes prebuilt from `petal-web-canvas`, which is an npm
workspace package in this repo. If you change that integration's Rust code,
rebuild it from the repo root:

```bash
npm run build:wasm --workspace integrations/petal-web-canvas
```

## Setup

From the repo root:

```bash
npm install          # installs the whole workspace, including this app
```

## Run

```bash
npm run dev --workspace examples/custom-integrations/diagram-canvas
```

Then open http://localhost:4012.

## Production build

```bash
npm run build --workspace examples/custom-integrations/diagram-canvas     # output to dist/
npm run preview --workspace examples/custom-integrations/diagram-canvas   # serve the build locally
```

## Examples

| File | Description |
|------|-------------|
| `examples/flowchart.ptl` | Flowchart with boxes, edges, and hover highlighting |
| `examples/org-chart.ptl` | Hierarchical org chart with color-coded levels |
| `examples/interactive.ptl` | Draggable, toggleable nodes with dynamic connections |

Pick an example from the dropdown in the sidebar. Click **View Source** to open
the editor; edits update the diagram live.

## How it works

- `petal-web-canvas` provides the WASM `PetalRuntime`, the `PetalCanvas` frame
  loop, the canvas renderer, and browser input handling.
- `src/main.ts` connects a `PetalCanvas` to the example picker and the debug
  controller, which can pause and step the frame loop.
- `src/editor.ts` sets up CodeMirror for live source editing.
- `src/debug.ts`, `src/debug-panel.ts`, and `src/debug-ws.ts` implement the
  pause/step debug protocol and its WebSocket bridge. The bridge is served by
  the Vite dev server at `ws://localhost:4012/debug` (dev only), so external
  tools can send commands to the running page.
