# petal-diagram-canvas

Canvas-based diagram visualization tool with a live source code editor. Renders
Petal programs as interactive diagrams on an HTML5 Canvas.

This is a **sample app**: it builds on the
[`petal-web-canvas`](../../integrations/petal-web-canvas/) integration for the
WASM runtime, canvas renderer, and browser-input plumbing, and adds only the
diagram-specific shell (the CodeMirror source editor and the pause/step debug
protocol). It does **not** embed Petal Core directly or ship its own WASM crate.

## Prerequisites

- **Node.js** (v18+)

The WASM runtime is provided prebuilt by `petal-web-canvas` (an npm workspace
package). If you change that integration's Rust, rebuild it with
`npm run build:wasm --workspace integrations/petal-web-canvas` from the repo
root.

## Setup

```bash
npm install          # from the repo root — installs the whole workspace
```

## Development

```bash
npm run dev --workspace sample-apps/diagram-canvas   # Vite dev server (http://localhost:4012)
```

## Production build

```bash
npm run build --workspace sample-apps/diagram-canvas     # output to dist/
npm run preview --workspace sample-apps/diagram-canvas   # preview the build locally
```

## Examples

| File | Description |
|------|-------------|
| `examples/flowchart.ptl` | Flowchart with boxes, edges, and hover highlighting |
| `examples/org-chart.ptl` | Hierarchical org chart with color-coded levels |
| `examples/interactive.ptl` | Draggable, toggleable nodes with dynamic connections |

Select examples from the dropdown in the sidebar. Click **View Source** to open
the CodeMirror editor — changes update the diagram live.

## How it works

1. `petal-web-canvas` provides the WASM `PetalRuntime`, the `PetalCanvas` frame
   loop, the canvas renderer, and browser-input plumbing (all imported from the
   `petal-web-canvas` package)
2. `src/main.ts` wires a `PetalCanvas` up to the example picker and the debug
   controller, gating the shared frame loop for pause/step
3. `src/editor.ts` integrates CodeMirror for live source editing
4. `src/debug.ts` / `src/debug-panel.ts` / `src/debug-ws.ts` add the pause/step
   debug protocol and its WebSocket bridge (dev only)
