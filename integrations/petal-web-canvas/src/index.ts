/** Public entry point for the petal-web-canvas integration.
 *
 * Sample apps depend on this package rather than re-implementing the WASM
 * runtime, canvas renderer, and browser-input plumbing. The frame loop is
 * `PetalCanvas`; `renderCommands` rasterizes petal-ui draw commands onto a 2D
 * context; `InputTracker` feeds browser events into the runtime. The WASM
 * runtime type/init are re-exported for hosts that need to poke the runtime
 * directly (debug tooling, headless drivers). */

export { PetalCanvas } from "./runtime.js";
export { renderCommands } from "./canvas-renderer.js";
export { InputTracker } from "./input.js";
export { default as initRuntime, PetalRuntime } from "../pkg/petal_web_canvas.js";
