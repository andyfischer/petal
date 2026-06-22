# petal-diagram-canvas

Canvas-based diagram visualization tool with a live source code editor. Renders
Petal programs as interactive diagrams on an HTML5 Canvas.

## Prerequisites

- **Node.js** (v18+)
- **Rust** (latest stable)
- **wasm-pack**: `cargo install wasm-pack`

## Setup

```bash
cd petal-diagram-canvas
npm install
npm run build:wasm   # compiles Petal to WASM (requires wasm-pack)
```

## Development

```bash
npm run dev          # starts Vite dev server (http://localhost:4012)
```

## Production build

```bash
npm run build        # output to dist/
npm run preview      # preview the build locally
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

1. `build-wasm.sh` compiles the Petal compiler (`../rust/`) to WASM via `wasm-pack`
2. The runtime (`src/runtime.ts`) captures draw commands during Petal evaluation
3. `src/canvas-renderer.ts` converts draw commands to Canvas API calls
4. `src/input.ts` provides mouse/keyboard state to the Petal program
5. `src/editor.ts` integrates CodeMirror for live source editing
