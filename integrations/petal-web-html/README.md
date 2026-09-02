# petal-web-html

Web runtime for Petal programs that render JSX element trees as live DOM.
The Petal compiler runs as WebAssembly; the program returns an element tree
each frame and the TypeScript side turns it into real DOM with click
handling.

For canvas graphics in the browser, see [petal-web-canvas](../petal-web-canvas/)
instead.

## Prerequisites

- Node.js 18 or later
- Rust (stable)
- `wasm-pack`: `cargo install wasm-pack`

## Setup and development

```bash
cd integrations/petal-web-html
npm install
npm run build:wasm   # compiles the Petal core (../../rust, feature `wasm`) and copies it into pkg/
npm run dev          # Vite dev server on http://localhost:5173
npm run build        # production build to dist/
npm run preview      # preview the build locally
```

## Examples

| File | Description |
|------|-------------|
| `examples/menu.ptl` | dropdown menus and a modal dialog using JSX and `state` |

The dev server loads `examples/menu.ptl` by default.

## How it works

1. `build-wasm.sh` compiles the Petal core (`../../rust/`) to WASM via
   `wasm-pack` with the `wasm` feature and copies the output into `pkg/`.
2. `src/runtime.ts` loads Petal source and executes it.
3. The program returns an element tree, which `src/renderer.ts` converts to
   DOM.
4. Elements carry an `eid={id}` attribute. A click on one is injected back
   into the runtime, and the program reads it on the next run with
   `clicked(id)`; `next_id()` hands out a unique id per element per frame.
   `examples/menu.ptl` shows the pattern.
