# Wasm use case feedback — facetedllc.com snowflake background (2026-10-15)

A field report from embedding `petal-web-canvas` in a production website: a
Next.js 16 (Turbopack, `output: "export"`) static site that draws fractal
snowflakes behind the page. The script is
`~/biz/facetedllc.com/frontend/public/petal/snowflakes.ptl`; the vendored host is
`~/biz/facetedllc.com/frontend/src/petal/`.

## The blocker: every web-canvas script trapped on frame 1

A fresh `./build-wasm.sh` at `8a35bd52` produced a runtime where **every**
script, including `examples/starfield.ptl` and a bare `clear(1, 2, 3)`, died on
its first `reset_and_run` with `RuntimeError: unreachable`. The browser then
showed the follow-on error `recursive use of an object detected which would
lead to unsafe aliasing in rust` from the `ResizeObserver`'s
`invalidate_frame`, because the trap leaves the `PetalRuntime` borrow flag set.
That second error is the one you see first, and it misleads.

Cause: `StaticSlots::begin_build` (core/src/static_cache.rs) calls
`std::time::Instant::now()` on every static build, and that panics on
`wasm32-unknown-unknown`. `builtins::initial_seed` already special-cases
`SystemTime` for wasm32; the static cache's timing never got the same
treatment.

Fix (in this session): a cfg'd `Instant` stand-in in static_cache.rs for
wasm32 that reads every instant as infinitely long ago. Live-edit drags never
"continue", so nothing gets deferred, and builds are never timed (`build_ns`
stays 0, which already means "not timed").

Suggestions:

- **Add a CI smoke test that runs a script through the built wasm.** Nothing
  caught this, and every web host was broken. A Node script that
  instantiates the pkg and runs a couple of frames is about 20 lines; see
  "Driving the runtime from Node" below.
- **Install `console_error_panic_hook`** (or an equivalent `set_hook` that
  forwards to `console.error`) in `PetalRuntime::new`. A bare `unreachable`
  gives you nothing to go on. I only found the cause by building with `--dev`
  for a symbolized stack.
- **Lint against bare `std::time` in core**, e.g. a clippy `disallowed-methods`
  entry for `Instant::now` / `SystemTime::now` outside an allow-listed clock
  module. Profiling and bench uses are gated today, but the next ungated use
  breaks web again.

## Pain points, in the order hit

1. **The checked-in `pkg/` was 3 months stale (July 13)**, so "grab a new build"
   meant running `build-wasm.sh`. That's fine, but there's no version or commit
   stamp in the pkg, so a vendoring consumer can't tell which Petal it has.
   Suggestion: have `build-wasm.sh` write `pkg/BUILD_INFO` (git SHA, dirty
   flag, date).
2. **The wasm is 3.8 MB raw / 1.3 MB gzipped** (it was 0.98 MB in July).
   `wasm-opt` wasn't on PATH. The `[package.metadata.wasm-pack.profile.release]
   wasm-opt` flags may or may not have applied; the build log doesn't make that
   obvious. For a decorative background that's a heavy download, so the host
   lazy-loads it on idle. Worth checking whether opt ran, and whether a
   size-focused profile (`opt-level = "z"`, `lto`, `codegen-units = 1`,
   `panic = "abort"`) is viable for embedding.
3. **The host TS isn't consumable by Turbopack as-is.** `runtime.ts` imports
   `./canvas-renderer.js` / `./input.js` / `./text-metrics.js`, the TS-ESM
   `.js`-means-`.ts` convention. Vite and tsc accept it; Next 16's Turbopack
   fails with `Module not found`. I dropped the extensions in the vendored
   copy. Extensionless or `.ts` specifiers (with `allowImportingTsExtensions`)
   would work everywhere.
4. **The host TS isn't a package you can vendor cleanly.** I copied four files
   and patched three things:
   - `pixelRatio` support. `resizeCanvas` sizes the backing store to CSS
     pixels, so hairline strokes are blurry on retina. A first-class
     `pixelRatio` option (plus a `dpr()` native, or exposing it in frame info)
     would make it unnecessary for scripts to take `dpr` as a prop and scale
     every coordinate by hand.
   - Errors with no `errorEl` were swallowed silently. `showError` returned
     early, so a broken script was invisible. It now `console.error`s.
   - The host owns the `ResizeObserver`, but `stop()` never disconnects it.
   A `petal-web-canvas` npm build (dist JS + .d.ts + wasm) would remove the
   copy-and-patch step.
5. **`list + list` isn't allowed** (`Cannot add list and list`), but
   `petal check` and `petal lint` both passed it. Then `petal lint --fix`
   rewrote `segs = segs + branch(...)` into `segs += branch(...)` via
   `prefer-compound-assign`, which looked plausible and was equally wrong. I
   found it only by running the script. Suggestions: have `check` flag `+` on
   two list-typed operands, since the types are known here (`[...]` literal
   and a fn returning one), and say "use concat()" in the runtime error.
6. **Testing a `ui` script outside a browser is awkward.** `petal run` doesn't
   know `screen_width`/`time`/`draw_line`, so I stubbed them as top-level fns,
   and they had to go **above** the script body. Stubs appended at the end
   failed with `Cannot call nil` because they read a `var` and can't be
   hoisted. `petal-ui`'s `Headless` is presumably the intended tool, but it
   isn't reachable from the CLI. A `petal run --host ui-headless script.ptl
   --frames 3 --size 1440x900` that reports command counts and per-frame time
   would cover both the "does it run" and the "is it fast enough" questions.
7. **`show-static` reported `let NAVY` / `let LIME` as "not read"**, but they
   are read via `let c = if f.accent then LIME else NAVY end` inside a `for`
   body. This looks like a false negative in the reader analysis.

## What worked well

- **`const` + static caching is exactly right for generative art.** The
  fractal arm geometry (recursive `branch()`, ~5 crystal habits) and the
  34-flake trait table are `const`. The first frame costs ~88 ms to build them,
  and later frames are ~8.5 ms of script for ~2,450 draw commands, a steady
  60 fps in Chromium at 1440×900. `petal show-static` confirmed that the
  tables were static and that the helper fns were static-pure.
- **The prop feed (`setProp` → `state`) is a great host API.** Scroll
  parallax, a cursor "wake" that parts the flakes, and `dpr` are each one line
  on the host and one `state` declaration in the script. Driving the pointer
  from a `window` listener means the canvas can stay `pointer-events: none`.
- **`petal check` + `petal lint` are fast and the messages are good**
  (`no-redundant-cast`, the hoisting warning). Apart from the list-`+` hole,
  the edit, check, run loop was smooth.
- **wasm-bindgen's `new URL('…_bg.wasm', import.meta.url)` worked unmodified
  in Next 16.** Turbopack emitted it as a hashed `_next/static/media/*.wasm`
  asset, which is ideal for cache-busting a vendored runtime.

## Driving the runtime from Node (for the smoke test)

```js
import fs from "node:fs";
const mod = await import("./pkg/petal_web_canvas.js");
await mod.default({ module_or_path: fs.readFileSync("./pkg/petal_web_canvas_bg.wasm") });
const rt = new mod.PetalRuntime();
const stack = rt.create_stack(rt.load_program(fs.readFileSync(process.argv[2], "utf8")));
for (let i = 0; i < 3; i++) {
  rt.set_frame_info(0.016, 1 + i * 0.016, i, 1440, 900);
  rt.begin_frame();
  rt.frame_needed(stack);
  rt.reset_and_run(stack);
  console.log("frame", i, JSON.parse(rt.take_draw_commands()).length, "cmds");
}
```

Note that `set_state_json` throws (a plain string, not an `Error`) for a prop
with no matching `state` declaration, so a harness pushing a fixed prop set
needs a try/catch per prop.
