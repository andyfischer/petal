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
- `const NAME = expr` is a `let` whose value must be static: a function of
  the source text alone, depending on no `state`, no `var`, no dynamic native
  and no function that reads or prints. A `const` that is not is a compile
  error carrying the chain of reasons (`A calls build`, `build calls print`),
  which `petal check` fails on and `petal run` refuses to start with. It is
  top level only, never assigned, holds data rather than a function, exports
  as `pub const`, and is otherwise evaluated, hoisted and hot-reloaded as the
  `let` it is. Whether a host's native is constant is the host's to say, so
  `petal check` and the language server report nothing for a `const` built
  from natives they do not have, and its host reports the error on load. See
  [Constants](docs/language-guide.md#const--a-value-that-must-be-constant).
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
- Enum variants are members of their enum (`Color.red`,
  `Color.custom(1, 2, 3)`, `when Color.red ->`), may carry payload fields and
  `key: expr` constant data (`park(shape: "area")`, read as `k.shape`), and
  answer `k.name`, `type(k)`, `Kind.all` and `Kind.from("park")`.
- An enum's name is the type of its variants. `fn f(k: Kind) -> Kind` is
  checked like a class-typed signature: `petal check` warns when another
  enum's variant, or a value that is no variant, reaches the slot, and when
  a field is read that no variant of the enum has (`k.shap`,
  `Kind.park.colour`).
- `match` is the control-flow statement as well as an expression: an arm's
  body after `->` is a statement list (to the next `when` or the match's
  `end`), `when a | b ->` matches either alternative, a match over the
  variants of one enum must cover them all or it does not compile, and
  `petal check` warns about an arm that can never run.
- Optional type annotations on `let`, `state`, parameters and return values,
  including `num` (int or float). Programs can be typed, untyped or mixed.
- Native value types for creative coding: `vec2`, `vec3`, colors, and a flat
  unboxed `f64_array`.
- Pending values: a value that is not ready yet (a fetch, a resource) flows
  through operators, conditions, loops and `match` without special handling.
- JSX-like element syntax for hosts that render a tree.
- Modules: a plain `import m` binds every export of the module bare and the
  qualifier `m` (`import m as x` renames it); `import m: a, b` binds only
  those names. Import paths are relative to the importing file (`./level`,
  `../shared/rng`) or start with a package name (`bloom/menu`); `pub` makes
  a symbol visible to importers, `pub import m` re-exports a module's
  surface, and a `petal.toml` makes a directory a package. A module's
  identity is canonical, so one file reached by two spellings is one module.
- Projects and packages: a `petal.toml` names a package, its
  `[dependencies]` (path or git; the key is the import name) and the host
  interfaces it `[requires]`. Every command and the language server find the
  nearest one from the file they are given. See
  [docs/packages.md](docs/packages.md).
- A bare name is resolved in tiers: a declaration in the file, a selective
  import, a plain import, the core builtins and `std`. Two plain imports offering the same
  name are an error only where the name is used bare; qualified access
  always works.

### Runtime

- A bytecode VM is the only execution engine. It has a frame pool, copy
  propagation, record shapes with an inline cache for field reads, and a
  straight-line fast path for the hot instructions.
- A `match` on string literals, int literals or enum variants is entered
  through a jump table, so its cost does not grow with the number of arms
  (a 40-arm match in a loop runs about five times faster).
- Reading an enum variant's constant data costs about what a record field
  read costs: `k.code` went from 51 ns to 3.5 ns over an empty loop and
  `K.v.code` from 71 ns to 6.6 ns, against 1.7 ns for a record field. Each
  declared variant has a slot in a table the running stack keeps, found by
  the variant tag's id, so a read no longer splits and hashes the tag.
  `Kind.all` is one list per stack and no longer allocates per read (228 ns
  to 19 ns for an eight-variant enum), and declaring an enum's data costs a
  run about 40% less (150 ns to 93 ns per variant). Nothing changes for a
  program: the list cannot be written through, a variable that starts as
  `Kind.all` and is rewritten in a loop gets its own copy at the read and
  is then updated in place as before, and a variant a host built or one
  kept in `state` across an edit reads as before. See
  [docs/dev/performance.md](docs/dev/performance.md#enum-data-reads).
- A `state` declaration that finds its slot costs about 40% less at the top
  level (74 ns to 44 ns) and 30% less inside a function (214 ns to 149 ns):
  the state table and the per-run set of touched keys hash with a small
  multiplicative hasher instead of SipHash. Nothing changes for a program.
  For an embedder, `Env::snapshot_state` and `get_all_state` now return a
  `petal::stack::StateMap` (a `HashMap` with that hasher), and
  `restore_state` takes any collection of `(RuntimeStateKey, Value)`, so a
  plain `HashMap` still passes. See
  [docs/dev/performance.md](docs/dev/performance.md#a-state-hit).
- Escape and last-use analyses turn value-semantic updates into in-place writes
  (loop accumulators, `state`-backed containers, `var` cells).
- A generational heap with a mark-and-sweep collector paced by bytes allocated.
- A frozen heap generation for data that lives as long as the program:
  `freeze(value)` makes a value and everything it holds permanent, so the
  collector neither traces nor frees it, and `frozen(value)` asks. With a
  rooted table of 100,000 records the pause per collection goes from 13.5 ms
  to 0.24 ms, and a collection with nothing to free from 2.1 ms to 0.009 ms:
  the sweep skips frozen slots through a bit index instead of reading them. Freezing is one-way (a frozen value is never freed), a value holding
  a function or a `var` cell is refused, and an update of a frozen value
  copies. See [docs/Builtins.md](docs/Builtins.md#frozen-data).
- Static values: a top-level value that depends on no dynamic data (a
  literal table, a table built from one by constant builtins or by
  functions that read no `state`, input or clock) is built on the first run
  and kept on the stack for the next ones, where it used to be rebuilt on
  every run. Nothing changes in the source and nothing changes in what a
  program does: the code that builds the value is still there and is jumped
  over when the value is already held, a value that is written to anywhere
  is left alone, a top-level `assert` and an unused binding that raises
  keep doing so on every run, and a kept value that something tries to
  update in place is copied instead. Over the 55 example panels a frame runs 0% to 30% fewer
  instructions (median 5%) and makes 1% to 46% fewer allocations (median
  19%), and a large `let` table passed to a memoized function no longer
  makes that call miss on every frame. It is part of the `fast` and
  `replay` policies and off under `baseline` and `explain`, while
  observations or the trace are on (so not yet in Garden panels), and for
  `petal run`, which runs a script once. `--policy fast-static` (or
  `Env::set_static_cache(false)`) turns it off; `Env::static_values` lists
  what a stack holds. The one thing a program can tell is `frozen`: a kept
  table passed to `freeze` is frozen once and stays frozen, where the table
  rebuilt on every run was a new unfrozen one each time. See
  [docs/dev/performance.md](docs/dev/performance.md#static-values).
- A reload follows the diff through the static values: `reload_program` and
  `set_config_value` keep the ones the edit cannot have changed and the next
  run rebuilds the rest, where every reload used to rebuild all of them. A
  value is kept when it is not written inside a statement the edit changed,
  nothing it depends on is (a function it calls, a table it is built from,
  every method of a name it calls, an enum it reads), and after a recompile
  the same declaration depends on the same names; so a new import, a
  shadowing declaration, an added overload and an added method rebuild what
  they could affect. A literal inside a function body rebuilds what calls
  the function. A rebuilt value that comes out as it was keeps its old heap
  value. Each value that something outside its own statement reads is
  built and stored on its own: a rebuilt table leaves the tables written
  around it kept, and holds the kept tables it is built from themselves,
  so which objects two values share is the same as after a rebuild of
  everything (`freeze` through one name is seen through the other). A value
  is kept only together with the values it was built from. A table that
  stands alone between two function declarations is kept too.
  Everything is rebuilt when the diff cannot say what changed, when
  top-level statements changed order, and when the set of source files
  changed. Only the stack the reload was asked of keeps anything;
  another stack of the program, or a fork, rebuilds on its next run. `ReloadReport::statics` reports constants kept and rebuilt and
  **stale seeds**, the `state` cells that kept a value although their
  initializer read something the edit changed; the SDL host and Garden
  print it (`constants: 2 rebuilt (TILES, WIDTHS), 41 kept`) and
  `pb_vm_last_reload_json` carries it under `statics`. While
  `set_config_value` is driven continuously, a static value whose last
  build took 4 ms or more waits for a 150 ms pause before it is rebuilt
  (`Env::set_static_debounce`, `Env::flush_static_rebuilds`); the dragged
  value itself never waits, nor does a cheap value written beside a slow
  one, and a slow table that only one other value reads is timed with it. `Env::set_static_follow(false)` is the old
  behaviour. From C: `pb_vm_set_static_debounce`,
  `pb_vm_flush_static_rebuilds`, `pb_vm_set_static_follow`. A reload that follows static values lowers the new program
  itself, which the next frame used to do. On Moses the frame after a
  reload runs 2,642 instructions where it ran 8,353, and 2,681 after an
  edit to one table (2,558 and 2,600 for 35,292 with its constant tables as
  `const`); the recompile is the same half second.
  See [docs/hot-reload.md](docs/hot-reload.md#static-values-across-a-reload).
- An enum's constant data is a static value where its expressions are
  static: the per-variant records are published by the first run after a
  load, or after a reload that reaches the enum, and left in place by the
  runs after it, where every run
  used to build and publish them again (about 100 ns a variant; 9 ns now).
  Data that calls a function reading `state` is published on every run as
  before.
- A static value that cannot be kept (one holding a function, a `var` cell,
  a host handle or a pending result) is built on every run, and the host is
  told once per load: `Env::take_static_warnings` hands out the lines,
  `petal run` and the SDL host print them, and a `const` says it was one. A
  program that declares a `const` is classified once per recompile, for the
  check and the cache together.
- The collector's budget counts live objects plus a small cost per slot, where
  it charged the full trace cost for free slots too: a program allocating at a
  steady rate now reaches a steady heap instead of growing its slot count and
  its pauses (30 ms to 17 ms a collection in the same 100,000-record
  benchmark, before freezing).
- Hot reload: recompile a running program and carry its `state` over.
- Hot reload does only the work an edit calls for. `Env::reload_program` diffs
  each source file's syntax tree against what the program was compiled from:
  a whitespace or comment edit only moves source positions, an edit of literal
  values (`0.35` to `0.4`, also inside records, lists and function bodies)
  writes the constants into the running program, and anything else recompiles
  and transfers state as before. The result is the same on every path, which
  `core/tests/hot_reload.rs` checks against a full recompile. On a
  10,000-line game a value edit takes 0.8 ms where the recompile took 300 ms.
  `Env::set_config_value` sets one value by binding path without the file
  changing. See [docs/hot-reload.md](docs/hot-reload.md).
- An enum can be edited while its variants are held in `state`: an edit to
  constant data is patched in place, and a variant whose enum, variant or
  payload has since changed stays a value that is read through the current
  declaration (a missing field or an unmatched `match` is an ordinary runtime
  error, never a crash).
- A recompile is about 2.7 times cheaper on a large program (300 ms to 110 ms
  on the same game): copy propagation solves per basic block over bit sets
  instead of per instruction over hash maps, and a reload re-parses only the
  files whose text changed.
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
- The everyday builtins are in scope everywhere; the domain sets are
  bundled packages a file imports (`import creative`, `noise`, `color`,
  `vec2`, `autodiff`, `json`, `io`). See
  [docs/Builtins.md](docs/Builtins.md#which-builtins-need-an-import).
- Math: `lerp` on numbers, colors and number lists, `clamp` and `round` that
  keep ints as ints, `asin`/`acos`/`atan`/`hypot`, `safe_div`, `random()` in
  several arities, and `rotate(v, angle)` on `vec2`.
- JSON: `json_parse` and `json_stringify`.
- Colors: `hsv()`/`hsl()` take hue in `[0, 1)`; `hsv_deg`/`hsl_deg` take
  degrees.
- A core prelude written in Petal, loaded only when a program refers to it.
- Every native declares its effects (what it reads and writes), which the
  memoizer and the `--effect-audit` runtime check both use.
- Function introspection (experimental): `fn_info(f)` describes a function
  value (parameter names, annotations, defaults, declared return type,
  captures, and a `code_hash` that survives reloads), and `fn_ast(f)` returns
  its syntax tree as records and lists. See
  [docs/function-introspection.md](docs/function-introspection.md).
- `glsl` (experimental), a library in `core-runtime/`: `to_glsl(lambda)`
  translates a Petal function in the pure numeric subset to GLSL, refuses
  anything else with a line and column, and caches by code hash so a script
  that calls it every frame translates once per change. See
  [docs/petal-to-glsl.md](docs/petal-to-glsl.md).

### CLI

- `petal check` and `petal run` agree on more scripts. `check` now reports a
  call to a builtin or host native with an argument count its `native fn`
  declaration does not take, a call through a plain import with a count the
  module it resolved to does not take, method syntax on a plain value that
  needs a packaged builtin the file has not imported, and every deferred
  compile error (a qualifier read as a value, a member a module does not
  export). `petal run` refuses a script with a name nothing defines before
  the first line runs, instead of failing if and when the line is reached.
  Assigning through a module qualifier (`hud.score = 5`) is a compile error.
- Method syntax reaches a packaged builtin or a host native only in a file
  that has it in scope; another file's import no longer enables it
  program-wide.
- `import ../x` works when the entry file is named with no directory
  (`petal run esc.ptl`); it used to fail with "climbs above the filesystem
  root".
- `petal fix-imports` rewrites `import color` to `import ./color` when a
  `color.ptl` sits beside the file and the file takes names the `color`
  package does not export; `-e` code sees the working directory's project
  dependencies.
- `petal run --ir` links the program (every native its calls name must be
  registered) before any line runs.
- The prelude-shadow warning is skipped only for the bundled copy of a
  first-party package, not for any package that takes a bundled name.
- The web runtimes' TypeScript wrappers (`PetalCanvas`, `PetalWeb`) expose
  `registerModule`, `registerPackage`, `registerBundle`, `provides`,
  `providedInterfaces` and `verifyInterface`.
- `petal fix-imports [--check] [--diff] <paths…>` migrates code written for
  the earlier import rules: a sibling file imported by a bare name becomes
  `./name`, `import m: *` becomes `import m`, a selective import whose
  qualifier is still used gains a plain import, and a bare name that exactly
  one available package exports gets its `import` line. Idempotent, and safe
  on migrated files.
- The language server resolves a document's imports from its own path (and
  from other open documents' unsaved text), and offers completion, hover and
  go-to-definition for imported names, `qualifier.` members and import
  paths.
- `petal run`, with `--trace`, `--observe` (dump every named value the run
  bound), `--profile`, `--policy`, `--seed`/`PETAL_SEED`, and `--ir` to run a
  serialized IR file. `--observe` reports a value-position loop
  (`let xs = for … end`) under its own name.
- `petal run --state-storage <file>` keeps `state` between runs in a JSON
  file. Without it, a script that declares `state` gets a warning on stderr,
  since its state lasts one process.
- `petal check` type-checks and lowers a program without running it. Unknown
  names and impossible calls are errors; `--strict` also fails on type
  warnings. It consults no host: a script for a host imports the host's
  interface package, found bundled, through the project's `petal.toml`, or
  with `-I`.
- `petal check` warns on three programs that compile but are silently wrong: a
  line starting with `-` under the line it was meant to continue, a bare
  operator expression whose value is discarded (`n + 1`), and a call to a
  builtin's name that reaches a plain value.
- Argument errors name the problem: a mistyped command gets a suggestion
  instead of "Error reading file", an unclosed block names its opener, and a
  stack trace that crosses modules names the entry file.
- `petal fmt` formats source. `petal lint` reports rule-based findings and can
  fix them, with `--verify` proving a rewrite left the IR equivalent. The
  `prefer-repeat` rule rewrites a string-repeat loop to `repeat(s, n)`, and
  `prefer-match` turns an `if`/`elsif` chain that compares one subject against
  literals — numbers included, since `when 1` matches what `== 1` does — into
  a `match`.
- `petal lint` rule `state-as-constant`: a plain top-level `state` that
  nothing in the program assigns, and whose initializer is static, is
  reported, and `--fix` rewrites it to `const`. The rule compiles its own
  rewrite and keeps only what the compiler accepts as constant. It is the
  first opt-in rule: it runs only with `--rules-include=state-as-constant`,
  because a cell a host writes by name looks the same to it. A `pub state`
  qualifies (importers cannot write an export) and becomes a `pub const`.
- `petal show-static` lists a `const` under that keyword and says when one is
  not constant or cannot be verified without its host (`const_check` in
  `--json`). The language server shows a top-level declaration's class on
  hover (`// static`, `// dynamic: <the first reason>`, `// function,
  static-pure`), reports a `const` that is not constant as an error, and
  lists a `const` as a constant in the outline. A static declaration the
  static-value cache gives a slot reads `// static, kept between runs`. The
  class is worked out by the first hover after a change, not on every
  keystroke.
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
- The language server knows enums: completion after `Kind.` offers the
  variants, `all` and `from`, and after a variant value its payload fields,
  constant data and `name`; hover and go-to-definition work on `Kind` and on
  `Kind.park`, through an import as well; and it answers
  `textDocument/documentSymbol` with the file's outline. Completion works on
  a line that does not parse yet. Completion item kinds and diagnostic
  severities are sent as the protocol's numbers (they were sent as names).
- `petal lsp` runs the language server. An MCP server under `tools/` exposes
  the same tooling to coding agents (see `docs/dev/mcp-server.md`).
- Per-command help pages (`petal <command> --help`).
- Prebuilt binaries for macOS (arm64, x86_64) and Linux (x86_64, arm64, static
  musl), installed by `curl -fsSL https://petal-lang.org/install.sh | sh`.
  The installer compares the archive's sha256 with the published one itself,
  and refuses a checksum file that does not hold a hash.

### Embedding and integrations

- `Env::resolve_import(path, importer)` (the canonical name an import
  resolves to), `Env::exports_of(path)`, `Env::importable_paths()`, and
  `Env::set_inline_import_dir(dir)` (where `./x` leads for a program loaded
  with no path). `Program::imports` records every module's exports and the
  entry file's imports for tools. `petal::fix_imports::ImportFixer` is the
  migration as a library, for a host that wants its own packages known.
- Packages compiled into the binary are importable by name with nothing
  registered, after everything the host and the search path offer: `core`
  (`import core` binds the qualifier, `core.len(xs)`), the builtin packages
  (`noise`, `color`, `vec2`, `autodiff`, `json`, `io`, `creative`), `ui` (the
  petal-ui interface; `ui/input` is its input half), `bloom`, `text_layout`
  and `glsl`.
  `core/build.rs` embeds them and fails the build if a source directory is
  missing. `petal check` therefore resolves `import ui` and `import bloom`
  with no host and no `-I`.
- Each host binding carries a revision, bumped when `set_binding` binds a
  value other than the very one already bound. `PetalCxt::binding_revision`
  lets a native keep what it decoded from a binding until the binding is set
  again (petal-ui's text natives keep font metrics this way), and the frame
  gate does not fingerprint a binding again that has not been set since the
  last run.
- A `petal.toml` that does not parse is a load error for every program under
  it, whether or not the program imports anything.
- `petal.toml` names dependencies and required host interfaces.
  `[dependencies]` maps an import name to `{ path = "…" }`, `{ git = "…", tag
  = "…" }` / `{ git, rev }` or a registry version string; path dependencies
  and git dependencies resolve, and a registry version is a
  "registries are not supported yet" error where it is imported. `[requires]`
  names host interfaces with a version range (`ui = ">=0.1"`).
- Every command that compiles, and the language server, finds the nearest
  `petal.toml` by walking up from the file: a project needs no `-I`. The
  example trees carry manifests naming the host interface packages they
  import (`garden`, `garden_config`, `sdl`, `web`, `fps`, `nes`).
- Hosts announce what they provide: `Env::provides(name, version)` and
  `Env::provides_package(name)` (`pb_vm_provides`, `pb_vm_provides_package`,
  `Vm::provides`; `provides` on the wasm runtimes). Loading a program whose
  packages require an interface the host lacks, or has at an incompatible
  version, is a load error. `petal check` reports the required interfaces and
  does not fail on them. bloom and text_layout require `ui`.
- A package manager. `petal init`, `add`, `remove`, `install`, `update`,
  `vendor` and `bundle` manage a project's `[dependencies]`. Git dependencies
  are fetched (with `git`) into a content-addressed cache under
  `~/.petal/cache` (`PETAL_HOME`); `petal.lock` pins each package's source,
  commit and tree hash, and fetches are verified against it. A program holds
  exactly one version of each package, chosen by minimal version selection
  (the highest tag anyone asks for; no solver), with a conflict error for
  pins that cannot be ordered. `vendor/<name>` at the project root is used in
  preference to the cache. `petal bundle <entry>` emits the whole graph in
  the in-memory form `register_package` accepts. `check` and `run` install on
  demand when a program imports a git dependency (`PETAL_OFFLINE=1` forbids
  it); hosts and the language server never fetch
  (`Env::set_install_on_demand`).
- `petal packages [path]` shows the project's resolved dependency graph, the
  host interfaces it requires, and the bundled packages, as well as the
  search path.
- `petal fix-imports` adds the import of a `[dependencies]` package of the
  file's project, and the language server completes and resolves them.
- `petal fix-imports` adds `import ui` (and, with a host's package on `-I`,
  `import garden`, `import sdl`, …) for names a script used through a host's
  implicit prelude, picking the package the others are built on when several
  export a name. `// petal-fix-imports-ignore` in a file keeps imports from
  being added to it.
- Host interface packages: each host ships the `native fn` declarations of its
  natives as a package beside its Rust code and registers it in memory —
  petal-ui `ui`, Garden panels `garden`, Garden's config host `garden_config`,
  petal-desktop-sdl `sdl`, the wasm element-tree runtime `web`. The stock CLI
  checks a host's scripts through `-I <the package's directory>`, with no
  knowledge of the host in `core/`. Registering a package gives the natives
  already registered the parameter names it declares.
- `Env::verify_interface(packages) -> Result<(), InterfaceMismatch>`: every
  native the named packages declare is registered and every native registered
  beyond the builtins is declared. Each host's drift test is this one call. A
  name may be a package, one module of it (`ui/input`), or an in-memory module.
- C bridge: `pb_vm_override_file` (`vm.override_file` in C++) compiles as if
  a file on disk held other text, wherever an import resolves to it. A
  relative import is a file, so a registered module cannot replace one.
- C bridge: `pb_vm_register_package` (a package from memory),
  `pb_vm_verify_interface`, `pb_vm_set_check_only`,
  `pb_vm_set_inline_import_dir`, and their `petal::Vm` methods. wasm:
  `register_package(name, modules_json)`, `verify_interface(packages)`,
  `set_check_only(on)`; the web-canvas runtime gained `register_module`,
  `register_package` and `verify_interface`.
- petal-ui: `Headless::with_ui(body)` (a script behind `import ui`),
  `Headless::panel_from_file_with_paths` (the Garden panel stubs and the
  `garden` package registered before the load), `petal_ui::raw_declarations()`
  and `script_for(env, source)` for tests that call the raw natives,
  `package_modules()`, `input_source()`, `INTERFACE_PACKAGES`.

- Native interfaces: `native fn name(a: int, b?, ...rest) -> type = "host_name"`
  declares a function the host implements — its parameter names, arity,
  parameter types and result type — in Petal source. A declared name binds to
  the host native (the same name, or the one after `=`), can be exported with
  `pub`, and can share an overload set with Petal `fn`s of other arities. The
  checker reads its signatures from these declarations; the builtins' live in
  the `core` package compiled into the binary (`core/packages/core`), and
  petal-ui's at the top of its `ui` module. `Env::declare_natives(source)`
  gives a host's registered natives the declared parameter names.
- Linking: loading a program into an `Env` fails before any line runs when a
  declared native the program refers to is not registered there
  (`module 'ui' needs native fns this host does not register: mouse_x, …, +38
  more`). `Env::set_check_only(true)` compiles without linking, which is what
  `petal check`, the LSP and `petal suggest` do.
- A Rust embedding API on `Env`: call a named Petal function, register native
  functions and host classes, pass opaque handles, read and restore state as
  JSON, run with a step budget, and watch source files for changes.
- `petal-c-bridge`: a C ABI and C++20 wrapper for embedding Petal, including
  native `vec3` across the boundary and source reading and editing by path.
- Source edits keep the file's formatting. `Goal::should_set_path`,
  `should_insert`, `should_append` and `should_remove` change one place inside
  a binding's literal (`POST.effects[2].amount`), and `should_set_value`
  rewrites only what differs: unchanged values keep their text and comments, a
  changed number or color keeps its token's spelling habits, and new elements
  are formatted like their siblings. The C bridge exposes them as
  `pb_source_set_value`, `_insert`, `_append` and `_remove`, taking values as
  JSON. See [docs/source-preservation.md](docs/source-preservation.md).
- A config file can hold enum variants: `Mode.dark` and `Shape.circle(2)`, of
  an enum the file declares, read as `StaticValue::Variant`, are written back
  as they were spelled, and cross the C bridge as `{"variant": "Mode.dark"}`.
- `petal-c-bridge`: a variant built with `enum_("Shape.box", …)` is that
  variant of the script's enum (it matches its patterns and reads its data).
- IR import validates the `enums` table, each variant construction and
  pattern against it, and every match arm's blocks. `show-ir` lists a
  program's enums.
- Natives can be declared constant functions: `NativeEffects::CONST`
  (`PB_FX_CONST` in the C bridge, `petal::fx::Const` in C++) says the same
  arguments give the same result for the life of the program and that the
  native reads no dynamic input at all. It is separate from `PURE`, opt-in,
  and off for every native that does not ask for it; `Env::native_constness`
  answers it. The core builtins are audited (arithmetic, strings, collections,
  colours, vectors and formatting are constant; `noise`, `is_valid`,
  `fn_info`, `fn_ast` and the random builtins are not). The static-value
  cache relies on it: a top-level table built by a `CONST` native is kept
  between runs, so a native wrongly declared constant is a stale value
  until the next reload (`Env::clear_static_values` drops them by hand).
  See "Constant natives" in `docs/ffi.md`.
- `petal show-static` classifies every top-level declaration as static data,
  a function (static-pure or not) or dynamic, with the chain of reasons for a
  dynamic one, whether dynamic code reads a static one, and the declarations
  each depends on (`--json` for tools, `--all` for imported modules). It is
  the classifier the static-value cache lowers against, and it shows what a
  host that runs the script every frame may build once and keep. A native
  counts as static only if registered `CONST`, so a host's
  natives are dynamic under the plain CLI. `Env::static_report` is the same
  answer against a host's own natives. See
  [docs/CLI.md](docs/CLI.md#show-static--static-declarations).
- `petal run --runs <n>` runs a script n times on one stack, the way a host
  runs it every frame: `state`, memo records and static values carry from
  each run to the next. Run policies take `+static` / `-static`
  (`--policy fast-static`), and `PETAL_POLICY=fast petal show-bytecode`
  shows the `static_skip` / `static_store` instructions of the static-value
  cache, which a plain `show-bytecode`, like a plain `run`, leaves out.
- `petal run --static-audit` (and `petal-ui-run --static-audit`) checks the
  classifier and the `CONST` registrations against a real run: while a
  top-level term that `show-static` calls static is being evaluated, it
  watches the host bindings, host data, resources, output, effects, the
  random stream and `state`, and any call of a native that is not constant,
  and fails naming the declaration, the input and the call if one is touched.
  `--static-only` evaluates only the static top-level statements and skips
  the rest, which builds a script's constant tables without its host.
  Embedders turn it on with `Env::set_static_audit` and read
  `Env::static_audit_report`. See
  [docs/CLI.md](docs/CLI.md#checking-the-answer---static-audit).
- `petal-c-bridge`: `pb_vm_set_static_cache` / `pb_vm_static_cache` switch
  the static-value cache, `pb_vm_clear_static_values` drops what it holds
  (for a host that changed something a `PB_FX_CONST` native answers from),
  and `pb_vm_static_values_json` returns its counters, the values that could
  not be kept and the warnings not yet handed out. In C++: `set_static_cache`,
  `static_cache`, `clear_static_values`, `static_values_json`.
- `petal-c-bridge`: `pb_vm_reload` takes layout and value edits without
  recompiling, and `pb_vm_last_reload_kind` / `pb_vm_last_reload_json` say
  what a reload did. `pb_vm_set_config` sets one config value in the running
  program without a file write (`PB_ERR_NEEDS_RELOAD` when the type or shape
  differs), and `pb_vm_source_text` returns the running text to write when a
  drag ends.
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
- Editor support: a tree-sitter grammar and Vim syntax highlighting. Both
  know enum variants with payload fields and constant data, qualified variant
  patterns, or-patterns and statement-list `match` arms.

### Changed

Breaking changes made during development, newest first. For the import and
package changes below, `petal fix-imports` does the rewriting and
[docs/packages.md](docs/packages.md#migrating-older-code) is the short guide.

- `petal run` starts with the check `petal check` does and does not run a
  program that fails it. Any check error (an unknown name, an argument count
  no overload takes, a bad named argument, method syntax that needs an import)
  stops the program before its first line, whether or not the line would
  have been reached; before, only unknown names did, and the other errors
  were printed while the program ran. The output is `check`'s, and `--json`
  reports `"phase": "check"`. `bench`, `pending-report` and `propose-edit`
  do the same. The check is `Env::check_program` for embedders, the language
  server reports the same findings, and petal-sdl and `petal-ui-run` print
  them on load and reload.
- `const` is a keyword and can no longer name a binding, a parameter or a
  function. It is still a record key and a field name (`{const: 1}`,
  `r.const`). No script in this repository or the projects built on it used
  the name. `ConstructKind` (the kind of a changed top-level construct in a
  reload report) and the classifier's `DeclKind` gain a `Const` variant, and
  `StmtKind::Let` an `is_const` field, for embedders that match on them.
- `petal check` no longer warns about the `export` spelling of `pub`;
  `petal lint` (rule `prefer-pub`) reports it and `petal fmt` rewrites it.
- `petal lint-fix` is removed; write `petal lint --fix`.
- `petal show-provenance`, `show-dependents` and `show-slice` are removed;
  `petal graph` is the one command (`--direction forward` for dependents,
  several `--term`s or `--slice` for a slice). Its JSON carries
  `target_terms` in place of the aliases' `root`, `ancestors`, `dependents`
  and `slice` keys.
- A `match` whose arms are all variants of one enum is now a compile error
  unless it has an arm for every variant or a catch-all (`when _ ->`). A
  guarded arm, or one whose payload patterns can fail, does not cover its
  variant. Add the missing arms or the catch-all. A single `|` is now a token
  (the or-pattern separator) and a `when … ->` arm with nothing after the
  arrow is a parse error. A `match` in which no arm matches stops the run
  with `No matching pattern for value: …`; it does not evaluate to `nil`, so
  a match that is meant to do nothing for other values needs `when _ -> nil`.
- Enum variants are no longer top-level names. Write `Color.red` where a bare
  `red` used to resolve, in expressions and in `match` patterns
  (`when Color.custom(r, g, b) ->`); a bare name is now undefined, and the
  error says which enum declares it. A bare identifier in a pattern is always
  a binding. `pub enum` exports the enum's name instead of its variants, so
  `import ./shapes: Circle` becomes `import ./shapes: Shape`. A variant prints
  as `Color.red` and `type()` of one is its enum's name (`"Color"`) instead of
  `"enum"`. `enum` is top-level only, an enum name may be declared once in a
  program, and inside a variant's parens `key: expr` is now constant data, not
  a type annotation. The `petal check` warnings about a variant declared by
  two enums, or named like another top-level declaration, are gone with the
  collisions they described. To migrate, run `petal check`: each bare variant
  is an `Undefined variable` error that names the enum to put in front
  (`did you mean Color.red?`), and a pattern written `when Circle(r)` is an
  error that says the same. A pattern that was a bare payload-free variant
  (`when Red ->`) is the one silent case: it now binds the value to `Red` and
  matches everything. `petal check` reports the arms after it as
  unreachable, and reports nothing when it was the last arm, so search for
  those by hand. State saved with `--state-storage` before the change holds
  the old unqualified tags, which equal no variant now.
  See [Enums](docs/language-guide.md#enums) and
  [Pattern Matching](docs/language-guide.md#pattern-matching).
- The domain builtins are behind imports. What reads as language stays global
  (`print`, `len`, `str`, `range`, the casts, the collection and string
  functions, the plain math including `clamp`, `pow`, `fract`, `radians`,
  `random_int`); the rest is declared by packages bundled in the binary, and a
  file names the one it uses:
  - `import creative`: `lerp`, `map_range`, `distance`, `mag`, `smoothstep`,
    `choose`.
  - `import noise`: `noise`, `noise_seed`.
  - `import color`: `hsv`, `hsl`, `hsv_deg`, `hsl_deg`, `color_lerp`.
  - `import vec2`: `vec2`, `vec3`, `normalize`, `dot`, `cross`, `limit`,
    `rotate`.
  - `import autodiff`: `dual`, `value_of`, `deriv_of`.
  - `import json`: `json_stringify`, `json_parse`.
  - `import io`: `symbol`, `push_output`, `binding`.

  After the import the names work bare, qualified (`noise.noise_seed(1)`) and
  with method syntax (`v.normalize()`). Without it a call is an error that
  names the import (`Unknown function: lerp — … add \`import creative\``),
  from `petal check` and at run time alike. `petal fix-imports` adds the
  imports, method-syntax uses included. The natives themselves are registered
  in every Env as before, in the same order; a host needs no change beyond
  its scripts. A host module or package registered under one of these names
  (`color`, `io`, …) replaces the bundled one.
- Hosts have no implicit imports and no global natives. Run
  `petal fix-imports` over old scripts, with `-I` for the directory of any
  host package that is not bundled (`-I garden/garden-script/packages`,
  `-I integrations/petal-desktop-sdl/packages`).
  - A script that uses petal-ui names starts with `import ui`; a Garden
    panel adds `import garden`; `init.ptl` and layout scripts say
    `import garden_config`; petal-desktop-sdl's extras are `import sdl`.
    bloom's and text_layout's modules import `ui` themselves.
  - A native is reachable only through a `native fn` declaration in scope.
    A native a host registers and nothing declares cannot be called, by name
    or by method syntax; an unknown callee is `Unknown function: x` (it used
    to be looked up among the host's natives and fail with
    `Unknown builtin: x`). An embedder that calls `register_native` also
    registers a package (or module) declaring it, and its scripts import it.
  - `Env::set_implicit_imports`, `pb_vm_add_implicit_import`,
    `petal::Vm::add_implicit_import` and the wasm `set_implicit_imports` are
    removed.
  - `petal check --host`, `--native`, and `--host` on `suggest`,
    `apply-change`, `ir-equal` and `bench` are removed, with
    `typecheck::globals::HostProfile` and its name lists. `check` and `run`
    agree on every script: `petal run` of a script that imports `ui` fails at
    load with the natives it lacks and the advice to use a petal-ui runner.
  - `petal_ui::register_prelude` registers the package `ui` (modules `ui` and
    `ui/input`) instead of a module plus an implicit import; the input,
    timing and frame declarations moved from `prelude/ui.ptl` to
    `prelude/input.ptl`, which `ui` re-exports. `Headless::new` takes a
    script with its own `import ui`.
  - Garden: an `init.ptl` without `import garden_config` no longer loads;
    saving a layout or a setting adds the import when it is missing.
  - `petal ir-equal`, `suggest` and `apply-change` compile check-only, so
    they work on a script written for a host this process does not provide.
- Imports changed meaning. Run `petal fix-imports` over old code.
  - A plain `import m` binds every export of `m` bare, as well as the
    qualifier. A file's own declarations and selective imports still win;
    two plain imports that offer one name make it an error to use that name
    bare (`m.name` always works).
  - `import m: a, b` no longer binds the qualifier `m`.
  - `import m: *` is removed; `pub import m` re-exports every export of `m`
    (it used to re-export the qualifier, which nothing does now).
  - A bare first path segment is always a package: `import level` no longer
    finds `level.ptl` beside the importer (write `import ./level`), and
    `-I`/`PETAL_PATH`/`Env::add_module_path` find packages only, not loose
    files.
  - A module's name is canonical. A file under a package's module directory
    is `<package>/<path>` however it was imported, so `state` keys and
    qualified function names of a module that used to be imported flat
    (`motion::x`) are now package-qualified (`bloom/motion::x`).
  - `ModuleResolver::resolve` takes the path as written and an `Importer`
    and returns a `Result`; `ModuleSource.name` is the canonical name.
    `ImportDecl.star` is still parsed, but only so the loader can reject it.

- The hand-kept signature tables are gone: `builtins::BUILTIN_PARAMS`,
  `typecheck::globals::PETAL_UI_NATIVE_PARAMS` and `host_native_signatures`,
  and `typecheck::builtin_types::{builtin_return_type, builtin_param_slots,
  builtin_arg_order}`. Signatures come from `native fn` declarations
  (`petal::native_decl`). The checker no longer knows a petal-ui native by
  name alone: it knows the ones a module of the program declares.
- A script that uses the `ui` prelude no longer loads into an `Env` that
  lacks a petal-ui native the prelude or the script refers to; it used to
  load and fail at the first call (`Unknown builtin`). A host that composes
  its own registration has to register the input natives as well as the draw
  ones before `register_prelude`.
- `native` is a keyword directly before `fn`.

- A color literal reads as `StaticValue::Color`, not as an `{r, g, b}`
  record, and `pb_source_bindings_json` writes it as `{"color": "#ff2e88"}`.
  `Goal::should_set_value` on a list of calls in an existing binding indents
  its elements from the binding's line (two spaces, closing bracket at the
  margin) instead of four and two.
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
