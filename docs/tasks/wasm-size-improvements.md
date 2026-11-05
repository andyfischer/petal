# Wasm size improvements

Status: 2026-11-05. Two items done (embedded-source comment stripping, binary
draw commands); the rest are open. Goal: the browser runtime under 1 MB.
Compressed, the default build is under 1 MB with brotli and the `--size`
build is under 1 MB with gzip. Under 1 MB raw needs the player/editor split
(item 10).

All numbers are for **petal-web-canvas**, the runtime the website and the
diagram canvas use, built as `build-wasm.sh` builds it: release, LTO,
`codegen-units = 1`, `wasm-bindgen --target web`, then `wasm-opt -O` with
the feature flags in `rust/Cargo.toml`. gzip is `-9`, brotli is quality 11.

## Current size

| Build | Raw | gzip | brotli |
|---|---|---|---|
| Before this work (`95571d8`) | 3.93 MB | 1.37 MB | 985 KB |
| **Now** (items 1 and 4 done) | **3.75 MB** | **1.31 MB** | **932 KB** |
| `--size` before (`opt-level z`, `wasm-opt -Oz`) | 2.31 MB | 886 KB | 700 KB |
| `--size` + nightly `-Cpanic=immediate-abort -Zfmt-debug=none -Zlocation-detail=none`, before | 2.16 MB | 825 KB | 656 KB |

The 2026-10-23 table in [embedding-web.md](../usage/embedding-web.md)
(3.69 MB) predates two weeks of core growth.

The other wasm build, core with `--features wasm` (petal-web-html), was about
3.25 MB raw / 1.14 MB gzip before this work.

## What the size is made of

From `twiggy top` on the bindgen output before `wasm-opt` (4.29 MB without its
name section), before this work:

| Area | Bytes | Notes |
|---|---|---|
| Compiler front and middle end | ~1.25 MB | `compiler` 427K, `static_cache` 161K, `static_classify` 148K, `parse` 144K, `typecheck` 95K, `fn_introspect` 71K, `cst_project` 66K, `pkgman` 59K, `module` 59K, `ast`/`lexer`/`package` ~100K |
| Bytecode backend and VM | 466K | lowering, escape analysis, copy propagation, the VM |
| `.rodata` | 537K | ~372K embedded Petal source (ui prelude 198K, bloom 95K, glsl 42K, text-layout 19K, core packages 19K), ~165K strings and tables |
| std / core / alloc generics | ~445K | |
| `hashbrown` | 288K | 254 monomorphized instances |
| Drop glue | 230K | `drop::<Env>` alone is 64K |
| `builtins` / `env` | 189K / 154K | |
| `petal_ui` | 183K | draw, text, input, hit testing |
| `serde_json` | 50K | still used by `get_state_json`, `set_state_json`, `register_package`, results |

About three quarters of the binary is the compiler. A page that only plays a
finished program downloads all of it.

## The ideas

| # | Idea | Est. saving (raw / gzip) | Cost | Status |
|---|---|---|---|---|
| 1 | Strip comments from embedded `.ptl` | −170 KB / −59 KB | none at run time | **Done** 2026-11-05 |
| 2 | Bloom, glsl and text_layout opt-in | −156 KB / −45 KB of source (before 1; ~−100 KB raw after) | pages that use them load them | Not started |
| 3 | Leave `pkgman` and `fn_introspect` out of wasm | ~−130 KB raw | `fn_info`/`fn_ast` unavailable in the browser | Not started |
| 4 | Draw commands as binary, not JSON | −13 KB code; no per-frame JSON | API change for JS embedders | **Done** 2026-11-05 |
| 5 | Serve brotli | 1.31 MB → 932 KB transferred | hosting config | Not started (outside this repo) |
| 6 | Size profile by default (`opt-level z` + `-Oz`) | −1.6 MB / −480 KB | frames ~1.5x slower | Available as `--size`; not the default |
| 7 | Nightly `immediate-abort` + `fmt-debug=none` | another −155 KB / −60 KB on top of 6 | no panic messages; nightly toolchain | Not started |
| 8 | Build the AST once (drop the parser's direct AST or `cst_project`) | ~−66 KB | refactor of the parse path | Not started |
| 9 | Cut generic instantiations (hashbrown, drop glue) | ~−100–200 KB (guess) | broad refactor | Not started |
| 10 | Separate "player" wasm: VM + builtins + petal_ui, no compiler | ~−1.3–1.5 MB | needs a bytecode file format; two builds | Not started |

Items 1–4 have no cost at run time and should all land. Item 10 is the only
way under 1 MB raw.

### 1. Strip comments from embedded sources — done

`core/build.rs` copies every embedded `.ptl` (the bundled packages, the std
prelude, the `web` interface) to `$OUT_DIR/embedded/`, and the crate's
`include_str!`s read the copies. For a wasm32 target the copy has every
whole-line `//` comment blanked and trailing whitespace trimmed
(`core/build_support/strip_comments.rs`). Native builds keep comments
because the LSP and `petal lint` show them. `PETAL_EMBED_COMMENTS=keep|strip`
overrides the default either way, and `petal::bundled::COMMENTS_STRIPPED`
says which happened.

- Lines are blanked, not removed, so every token keeps its line and column,
  and error positions inside a bundled module match the repository file.
- `src/strip_comments_check.rs` lexes every embedded file before and after
  stripping and requires the same tokens at the same positions. The core lib
  suite also passes with `PETAL_EMBED_COMMENTS=strip` on native.
- petal-ui's `prelude_source()` / `input_source()` now read the bundled `ui`
  package instead of their own `include_str!`. Otherwise the binary would
  carry a stripped and an unstripped copy of the 198 KB prelude; LTO had
  only been merging the two because they were identical.
- Trailing `//` comments after code stay. Removing them safely needs the
  real lexer; they are a small share.
- Measured: `.rodata` 537 KB → 367 KB.

### 2. Make bloom, glsl and text_layout opt-in

They are pure Petal libraries that every Env carries (`BUNDLED_PACKAGES` in
`core/build.rs`). Most canvas pages import none of them. Either put each
behind a cargo feature of `petal` (on for the CLI, off for petal-web-canvas
by default), or leave them out of the wasm build and have a page that needs
one call the existing `register_package` with a `petal bundle` of it.

### 3. Leave `pkgman` and `fn_introspect` out of wasm

`pkgman` (59K) installs packages from the network and the filesystem; it is
reachable through `ModuleRegistry::dependency_package` but cannot do anything
in a browser. `fn_introspect` (71K) backs the experimental `fn_info` /
`fn_ast`. Gate both on `not(target_arch = "wasm32")` or a cargo feature.

### 4. Draw commands as binary — done

`petal_ui::draw_wire::DrawWire` encodes a frame's commands as a flat
`Vec<f64>` plus a string table. petal-web-canvas exports `take_draw_buffer()`
(a `Float64Array`) and `draw_strings()` in place of the JSON
`take_draw_commands()`, and `run_speculative()` returns a buffer too.
`decodeDrawCommands` (`integrations/petal-web-canvas/src/draw-wire.ts`)
rebuilds the same `{ op, ...fields }` objects, so `renderCommands` did not
change.

- `test/vitest/fixtures/draw-wire.json` holds one sample of every command, in
  both encodings. The Rust test `draw_wire::tests::fixture_is_current` writes
  it (`PETAL_UPDATE_FIXTURES=1`) and checks it is current;
  `test/vitest/draw-wire.test.ts` decodes it and compares it with the JSON.
- End to end, the old JSON build and the new build agreed on every command
  of five frames of each `petal-web-canvas/examples/*.ptl` (25,176 commands).
- Size: only −13 KB of code (`DrawCommand`'s serializer and part of
  serde_json). serde_json stays for the other JSON exports. The bigger win is
  per frame: no JSON string is built in wasm and parsed in JS.
- Next step, if frame time matters: render straight from the buffer instead
  of decoding to objects.

### 5. Serve brotli

The default build is 932 KB brotli and 1.31 MB gzip. This is the cheapest way
to get under 1 MB transferred, and it is set where the site is hosted.

### 6. Size profile by default

`build-wasm.sh --size` builds `opt-level = "z"`. Making it the default for
pages that are not animation-heavy is a product call: frames are ~1.5x slower
(flow_field 6.0 → 9.3 ms in the 2026-10-23 measurement). The build should
also pass `-Oz` to `wasm-opt` instead of `-O` under `--size`; it currently
inherits `-O` from `Cargo.toml`.

### 7. Nightly panic and formatting flags

`-Zbuild-std=std,panic_abort` with `-Zunstable-options -Cpanic=immediate-abort
-Zfmt-debug=none -Zlocation-detail=none` saved another 155 KB raw on top of
`opt-level z`. Costs: a nightly toolchain in CI, no panic message for
`console_error_panic_hook` to log, and `{:?}` printing nothing, which breaks
any user-visible message built with `Debug`. Worth it only for a "player"
build.

### 8. Build the AST once

`cst::driver::parse_source_phased` has the parser build an AST directly *and*
record CST events. It then builds the green tree and projects a second AST
from it (`cst_project`). The direct AST is used only for a debug-build
differential check, but both paths ship in release. Dropping one saves
`cst_project` (66K) or the parser's AST building, and makes parsing faster.

### 9. Fewer generic instantiations

hashbrown is 288K over 254 instances, and drop glue is 230K. Fewer distinct
map key/value types (or type-erased maps on cold paths), and boxing rarely
used large `Env` fields, would cut both. Measure with `twiggy top` and
`twiggy dominators` per change.

### 10. A "player" build without the compiler

Compile programs to bytecode when the site builds, and ship a wasm with only
the VM, builtins and petal_ui. The compiler, typecheck, static passes and
lowering are ~1.3–1.5 MB of the 3.75 MB. Needs a serialized bytecode format
(there is none today; `ir_serialize.rs` only has JSON helpers). Live editing,
including the petal-lang.org snippet editor, still needs the full build, so
this means two builds: player and editor.

## How to measure

```bash
cd integrations/petal-web-canvas/rust
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir /tmp/bg target/wasm32-unknown-unknown/release/petal_web_canvas.wasm
wasm-opt -O --enable-bulk-memory --enable-multivalue --enable-mutable-globals \
  --enable-nontrapping-float-to-int --enable-reference-types --enable-sign-ext \
  /tmp/bg/petal_web_canvas_bg.wasm -o /tmp/opt.wasm
ls -l /tmp/opt.wasm; gzip -9c /tmp/opt.wasm | wc -c
twiggy top -n 40 /tmp/bg/petal_web_canvas_bg.wasm   # before wasm-opt, names intact
```

The `wasm-bindgen` CLI version must match the `wasm-bindgen` crate in the
build's lock file (`cargo install wasm-bindgen-cli --version <x>`). For a
per-module breakdown, run `twiggy top -n 100000 --format csv` and sum
`ShallowSize` by the path prefix of each name.

## Log

- 2026-11-05: measured and broke down the size, and wrote down items 1–10.
  Did item 1 (−170 KB of `.rodata`) and item 4 (draw commands binary, −13 KB
  of code). Together: 3.93 → 3.75 MB raw, 1.37 → 1.31 MB gzip, 985 → 932 KB
  brotli.
