# Wasm size improvements

Status: 2026-11-08. Finished items (comment stripping in embedded sources,
binary draw commands, `pkgman` out of the library) have been removed from
this doc; see the log. Everything below is open.

Goal: the browser runtime under 1 MB. Compressed, the default build is under 1 MB with brotli and the `--size`
build is under 1 MB with gzip. Live editing is a core feature, so the
compiler always ships; raw size comes down only by trimming everything.

All numbers are for **petal-web-canvas**, the runtime the website and the
diagram canvas use, built as `build-wasm.sh` builds it: release, LTO,
`codegen-units = 1`, `wasm-bindgen --target web`, then `wasm-opt -O` with
the feature flags in `rust/Cargo.toml`. gzip is `-9`, brotli is quality 11.

## Current size

| Build | Raw | gzip | brotli |
|---|---|---|---|
| Before this work (`95571d8`) | 3.93 MB | 1.37 MB | 985 KB |
| **Now** | **3.75 MB** | **1.31 MB** | **932 KB** |
| `--size` before (`opt-level z`, `wasm-opt -Oz`) | 2.31 MB | 886 KB | 700 KB |
| `--size` + nightly `-Cpanic=immediate-abort -Zfmt-debug=none -Zlocation-detail=none`, before | 2.16 MB | 825 KB | 656 KB |

The 2026-10-23 table in [embedding-web.md](../usage/embedding-web.md)
(3.69 MB) predates two weeks of core growth.

The other wasm build, core with `--features wasm` (petal-web-html), was about
3.25 MB raw / 1.14 MB gzip before this work.

## What the size is made of

From `twiggy top` on the bindgen output before `wasm-opt` (4.29 MB without its
name section), before this work. Since then `.rodata` is 367 KB (comments
stripped), `pkgman` is gone and serde_json is a little smaller:

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
| 2 | Bloom, glsl and text_layout opt-in | ~−100 KB raw (comment-stripped source) | pages that use them load them | Not started |
| 3 | Leave `fn_introspect` out of wasm | ~−70 KB raw | `fn_info`/`fn_ast` unavailable in the browser | Not started (needs 2) |
| 5 | Serve brotli | 1.31 MB → 932 KB transferred | hosting config | Not started (outside this repo) |
| 6 | Size profile by default (`opt-level z` + `-Oz`) | −1.6 MB / −480 KB | frames ~1.5x slower | Available as `--size`; not the default |
| 7 | Nightly `immediate-abort` + `fmt-debug=none` | another −155 KB / −60 KB on top of 6 | no panic messages; nightly toolchain | Not started |
| 8 | Build the AST once (drop the parser's direct AST or `cst_project`) | ~−66 KB | refactor of the parse path | Not started |
| 9 | Cut generic instantiations (hashbrown, drop glue) | ~−100–200 KB (guess) | broad refactor | Not started |

Items 2 and 3 have no cost at run time and should land.

### 2. Make bloom, glsl and text_layout opt-in

They are pure Petal libraries that every Env carries (`BUNDLED_PACKAGES` in
`core/build.rs`). Most canvas pages import none of them. Either put each
behind a cargo feature of `petal` (on for the CLI, off for petal-web-canvas
by default), or leave them out of the wasm build and have a page that needs
one call the existing `register_package` with a `petal bundle` of it.

What the code looks like (checked 2026-11-08):

- All three are pure Petal: no `native fn`, no Rust natives, and nothing in
  `core/src` refers to them by name except tests and help text.
- The one seam is the `BUNDLED_PACKAGES` table in `core/build.rs`. Everything
  else (`bundled::BUNDLED_PACKAGES`, the module registry's
  `bundled_packages()`, lint, `strip_comments_check`) iterates that table,
  so a package left out of it is simply absent and `import bloom` fails like
  any unknown package.
- `bloom` depends on `text_layout`, so they go together or `text_layout`
  stays when `bloom` is in.
- Every embedder (garden, petal-web-canvas, petal-web-html, petal-desktop-sdl,
  petal-c-bridge, ui-corpus) uses `default-features = false`, so a
  default-on feature would turn the libraries off for all of them. Garden's
  gpp-apps import `bloom`. So the features must be named explicitly by the
  hosts that want them (`features = ["bloom"]`), not added to `default`.
- No web page imports them: the petal-lang.org snippets that mention them
  are `ignore` blocks.

Plan: cargo features `bloom` (implies `text_layout`), `text_layout`, `glsl`
on `petal`; `cli/` and garden enable them; `build.rs` reads
`CARGO_FEATURE_*` and leaves the rest out of the table. Gate the tests that
`expect("bloom is bundled")` on the feature. Make the unknown-package error
name the missing feature when the name is one of the three.

### 3. Leave `fn_introspect` out of wasm

It backs the experimental `fn_info` / `fn_ast` and `Env::function_params`
(the C bridge). The bundled `glsl` library calls `fn_info`/`fn_ast` to
translate lambdas, so it can only leave the wasm build together with `glsl`
(item 2). cheesecake also calls them (`engine/script/.../petal_fx.ptl`,
`games/rivet/bg.ptl`), natively.

It is less separable than item 2. The 679-line module is mostly the
AST-to-record walker, but its `Cache` is a field of every `Program`
(`program.introspect`, built in `compiler` and `program_analysis`, cleared in
`env/reload.rs`), `Env::function_params` reads its parameter metadata, the
VM dispatches the two intrinsics (`vm/introspect.rs`, `vm/native.rs`), and
`core/packages/core/src/introspect.ptl` declares them in every file's scope.

Plan: a feature `introspection` (enabled by `cli/`, the C bridge, and with
`glsl`). Split the module: parameter metadata (name, annotation, default
source), which `function_params` needs, stays; the `fn_ast` walker and the
JSON value building go behind the feature. Without it the two intrinsics stay
registered and declared (so programs type-check the same everywhere) but
return an error saying the build has no introspection.


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
any user-visible message built with `Debug`.

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
- 2026-11-05: moved `pkgman` into its own crate, `core-libs/petal-pkgman`,
  linked only by the CLI (#23).
- 2026-11-08: removed the finished items (1, 4, `pkgman` half of 3) from
  this doc. Remaining items keep their numbers.
- 2026-11-08: dropped item 10 (a player build without the compiler): live
  editing is a core feature. Recorded the modularity findings for 2 and 3.
