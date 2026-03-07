# petal-web

Web runtime for Petal programs. Compiles Petal to WebAssembly and renders JSX
element trees as live DOM.

## Prerequisites

- **Node.js** (v18+)
- **Rust** (latest stable)
- **wasm-pack**: `cargo install wasm-pack`

## Setup

```bash
cd petal-web
npm install
npm run build:wasm   # compiles Petal to WASM (requires wasm-pack)
```

## Development

```bash
npm run dev          # starts Vite dev server (http://localhost:5173)
```

## Production build

```bash
npm run build        # output to dist/
npm run preview      # preview the build locally
```

## Examples

| File | Description |
|------|-------------|
| `examples/menu.ptl` | Dropdown menus + modal dialog using JSX and state |

The dev server loads `examples/menu.ptl` by default. Petal programs return JSX
element trees that are rendered as real DOM elements with click event handling.

## How it works

1. `build-wasm.sh` compiles the Petal compiler (`../rust/`) to WASM via `wasm-pack`
2. The TypeScript runtime (`src/runtime.ts`) loads Petal source and executes it
3. Programs return element trees (JSX) which `src/renderer.ts` converts to DOM
4. Click events are injected back via `clicked(id)` for stateful UI components
