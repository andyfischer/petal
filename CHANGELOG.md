# Changelog

Notable changes to the Petal language, the `petal` CLI, the standard libraries
and the host integrations. Garden has its own releases (`garden-v*` tags) and
is not tracked here.

Petal has not had a tagged release yet, so everything so far sits under
"Unreleased". When a release is cut, rename that heading to the version and
date, and start a new "Unreleased" above it. See
[docs/dev/release-checklist.md](docs/dev/release-checklist.md).

## Unreleased

What the first release contains, written from the git history (January to
October 2026). The "Changed" section lists the breaking changes made along the
way, for code written against an earlier checkout.

### Language

- Block syntax with keywords and `end` (`fn … end`, `if … then … end`,
  `for … do … end`, `match`), optional semicolons, string interpolation
  (`"hello {name}"`), and triple-quoted raw strings.
- Bindings: `let` for dataflow values, `var` for a mutable slot (written with
  `set`, read with `get`), and `state` for values kept across runs and hot
  reloads.
- `state` slots are keyed by the call path that reaches them, so two calls to
  the same function keep separate state. Loops get per-iteration state, with
  optional explicit keys.
- Value semantics: lists, records and arrays are immutable values. Appending,
  index assignment and field assignment produce a new value, and the compiler
  mutates in place where it can prove the old value is dead.
- Functions: multi-arity overloading, default parameter values, named call
  arguments (for user functions, builtins and host natives), argless
  `fn … end` lambdas, the `@` in-out argument (`f(@x)` means `x = f(x)`), and
  `-> nil` to turn off a function's implicit return.
- Operators: the pipe `|>`, method call syntax `obj.method(args)`, `++` for
  concatenation, compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`, `++=`,
  `??=`), the coalescing `??`, optional chaining `?.`, and scalar-over-list
  broadcasting for `+ - * /`.
- `for` loops are expressions that collect their body's values.
  `for x in range(a, b)` runs without allocating the range.
- Enums and pattern matching, records with spread syntax, and classes with
  user-defined methods (plus a built-in `Rect`).
- Optional type annotations on `let`, `state`, parameters and return values,
  including `num` (int or float). Programs can be typed, untyped or mixed.
- Native value types for creative coding: `vec2`, `vec3`, colors, and a flat
  unboxed `f64_array`.
- Pending values: a value that is not ready yet (a fetch, a resource) flows
  through operators, conditions, loops and `match` without special handling.
- JSX-like element syntax for hosts that render a tree.
- Modules: `import`, namespaced paths (`import bloom/menu`), selective import
  lists, `pub` to make a symbol visible to importers, re-exports
  (`pub import m: *`), and `petal.toml` packages.

### Runtime

- A bytecode VM is the only execution engine. It has a frame pool, copy
  propagation, record shapes with an inline cache for field reads, and a
  straight-line fast path for the hot instructions.
- Escape and last-use analyses turn value-semantic updates into in-place writes
  (loop accumulators, `state`-backed containers, `var` cells).
- A generational heap with a mark-and-sweep collector paced by bytes allocated.
- Hot reload: recompile a running program and carry its `state` over.
- Forked and speculative execution: run an isolated copy of a program side by
  side, with a step budget.
- The frame gate skips a frame whose inputs did not change, and memoized scopes
  replay a call whose inputs did not change. Named run policies (`fast`,
  `baseline`, `explain`, `replay`) choose which of these are on.
- Forward-mode automatic differentiation with dual numbers, through the
  arithmetic operators and the math builtins.
- Runaway recursion stops with a `Stack overflow` error and a collapsed stack
  trace instead of exhausting memory. The call depth limit is 20,000 frames
  (5,000 for a function that declares path-keyed `state`; `state(key)` is not
  limited).
- Memoization is bounded: no memo scope opens deeper than 96 frames, and the
  table is capped at 96 MB of records, so recursion no longer fills memory
  with records (naive `fib(27)` went from 541 MB to under 5 MB).
- The frame gate settles when a loop stores the values a `state` list or
  record already holds (`xs[i] = v` with an equal `v`), as it already did for
  the same stores outside a loop.
- Integer overflow and modulo by zero are reported as errors.
- Runtime errors print the source line with a caret.

### Builtins and standard library

- Collections: `map`, `filter`, `reduce`, `sort`, `sort_by`, `reverse`,
  `enumerate`, `zip`, `slice`, `flat`, `prepend`, `concat`, `last`,
  `drop_last`, and `range(a, b, step)` with negative steps.
- Strings: `split`, `join`, `upper`, `lower`, `repeat`, `starts_with`,
  `ends_with`, `trim`, formatting helpers, and failable number parsing.
  `len`, `slice` and `s[i]` count characters (code points); `byte_len` and
  `byte_slice` work in UTF-8 bytes.
- Math: `lerp` on numbers, colors and number lists, `clamp` and `round` that
  keep ints as ints, `asin`/`acos`/`atan`/`hypot`, `safe_div`, `random()` in
  several arities, and `rotate(v, angle)` on `vec2`.
- JSON: `json_parse` and `json_stringify`.
- Colors: `hsv()`/`hsl()` take hue in `[0, 1)`; `hsv_deg`/`hsl_deg` take
  degrees.
- A core prelude written in Petal, loaded only when a program refers to it.
- Every native declares its effects (what it reads and writes), which the
  memoizer and the `--effect-audit` runtime check both use.

### CLI

- `petal run`, with `--trace`, `--observe` (dump every named value the run
  bound), `--profile`, `--policy`, `--seed`/`PETAL_SEED`, and `--ir` to run a
  serialized IR file. `--observe` reports a value-position loop
  (`let xs = for … end`) under its own name.
- `petal run --state-storage <file>` keeps `state` between runs in a JSON
  file. Without it, a script that declares `state` gets a warning on stderr,
  since its state lasts one process.
- `petal check` type-checks and lowers a program without running it. Unknown
  names and impossible calls are errors; `--strict` also fails on type
  warnings. `--host garden` resolves the packages Garden registers.
- `petal check` warns on four programs that compile but are silently wrong: a
  line starting with `-` under the line it was meant to continue, a bare
  operator expression whose value is discarded (`n + 1`), an enum variant
  declared twice or sharing a top-level name, and a call to a builtin's name
  that reaches a plain value. An enum's name is accepted as a type name.
- Argument errors name the problem: a mistyped command gets a suggestion
  instead of "Error reading file", an unclosed block names its opener, and a
  stack trace that crosses modules names the entry file.
- `petal fmt` formats source. `petal lint` reports rule-based findings and can
  fix them, with `--verify` proving a rewrite left the IR equivalent. The
  `prefer-repeat` rule rewrites a string-repeat loop to `repeat(s, n)`.
- `petal suggest` proposes the type annotations and safe refactors a program
  implies.
- `petal bench` reports the per-call cost of named functions.
- `petal graph` walks the dataflow graph forward or backward from a term,
  `petal show-graph` exports it as DOT, and `petal explain` traces a value
  back to the code that produced it.
- `petal propose-edit` and `petal apply-change` rewrite source toward a goal
  while preserving formatting and comments.
- Stage dumps: `show-tokens`, `show-ast`, `show-ir`, `show-bytecode`, each with
  a JSON form. `petal ir-equal` compares two programs' IR.
- `petal lsp` runs the language server. An MCP server under `tools/` exposes
  the same tooling to coding agents (see `docs/dev/mcp-server.md`).
- Per-command help pages (`petal <command> --help`).
- Prebuilt binaries for macOS (arm64, x86_64) and Linux (x86_64, arm64, static
  musl), installed by `curl -fsSL https://petal-lang.org/install.sh | sh`.
  The installer compares the archive's sha256 with the published one itself,
  and refuses a checksum file that does not hold a hash.

### Embedding and integrations

- A Rust embedding API on `Env`: call a named Petal function, register native
  functions and host classes, pass opaque handles, read and restore state as
  JSON, run with a step budget, and watch source files for changes.
- `petal-c-bridge`: a C ABI and C++20 wrapper for embedding Petal, including
  native `vec3` across the boundary and source reading and editing by path.
- `petal-desktop-sdl`: an SDL2 desktop host with hot reload, antialiased
  drawing, audio, gamepads, procedural sound effects, and a debug protocol for
  agents (screenshots, typed input, state queries).
- `petal-web-canvas` and `petal-web-html`: WebAssembly hosts that draw to a
  canvas or patch the DOM.
- `petal-ui`: the shared input and draw contract, a widget prelude, a component
  library, text measurement from real font metrics, gradients, shadows, clips,
  offscreen canvases, and a headless driver (`petal-ui-run`) with scenario
  files.
- `petal-ui` text editing: `text_field` and `text_field_update` have a
  selection (shift+arrows, drag, double- and triple-click, select all),
  cut/copy/paste through `clipboard_get()` / `clipboard_set(text)`, undo and
  redo, word and line movement, and horizontal scrolling.
  `text_wrap_rows(s, style, w)` returns each wrapped row with its character
  range, and `text_range_rects(s, style, a, b)` the rects covering a range.
  Garden and the SDL host connect the system clipboard.
- Headless Garden panels: `POST /tick` puts a panel on a virtual clock, so
  `dt()` and `time()` advance by the ticks sent rather than by wall time (and
  stay virtual across `/panel/reset`); `POST /mouse` takes `"pane": <n>` for
  coordinates local to a pane.
- `petal-query`: a data-fetching layer for panels (queries, mutations,
  navigation).
- `bloom`, a UI component library written in Petal, and `text_layout`, text
  placement by font metrics.
- Editor support: a tree-sitter grammar and Vim syntax highlighting.

### Changed

Breaking changes made during development, newest first:

- `petal run` no longer memoizes calls by default (its policy
  is `fast-memo`): a script that runs once has no later run to replay a
  record in. Output is unchanged; pass `--policy fast` for the old behavior.
  Hosts that run a script every frame still default to `fast`.
- `petal run`, `bench`, `check` and the `show-*` and query
  commands reject an unknown option or a second source file instead of taking
  it as the file to run.
- Path-keyed `state` declared more than 5,000 calls deep is an
  error with its own message. Plain recursion may now go 20,000 deep (was
  5,000).
- `petal check --strict` fails on the four new warnings listed
  under CLI, and no longer fails on the `export` deprecation alone.
- petal-ui's `text_wrap` no longer collapses a run of spaces
  inside a row, so every row is a contiguous run of its source.
- The prelude's `wrap`, `preview` and `truncate_*` measure
  `max_chars` in characters, following `len` and `slice` (next item), so text
  with multibyte characters breaks at a different column than before.
- String offsets count characters (Unicode code points) instead of UTF-8
  bytes: `len("Óscar")` is 5 and `slice("Óscar", 0, 1)` is `"Ó"`. `s[i]` now
  indexes a string. `byte_len` and `byte_slice` keep the byte unit. See
  [docs/tasks/code-point-migration.md](docs/tasks/code-point-migration.md).
- `pub` replaces `export`. `export` still parses as a deprecated spelling.
- Assigning to a binding from an outer function is a compile error. Use `var`
  and `set` for a slot that several functions write.
- `state` slots are keyed by call path instead of by name alone, so functions
  that used to share one slot through several call sites no longer do.
- `petal check` fails on unknown names and impossible calls without
  `--strict`.
- Commas are required between the elements of every delimited list.
- Modules must mark a symbol `pub` for importers to see it. The `_` prefix no
  longer makes a name private.
- The read-a-field builtin `get` was removed; `get` now reads a `var` cell.
  The `f64_array` builtin `set` was renamed `set_at`.
- `let` shadowing is lexical: a local that reuses an outer name is a new
  binding.
- List and record mutation builtins (`append`, `pop`, `set`, `swap`, `remove`)
  return a new value instead of changing their argument.
- `hsv()`/`hsl()` take hue in `[0, 1)` instead of degrees.
- The surface syntax moved from braces to keyword-and-`end` blocks.
- The graph evaluator was removed in favor of the bytecode VM.
- The top-level `petal -e` shorthand was removed; use `petal run -e`.

### Known issues

The open items are ranked in [docs/tasks/todo.md](docs/tasks/todo.md).
