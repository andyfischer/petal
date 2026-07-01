# Plan: a module / import system for Petal

Status: proposed (2026-07-01)
Related: [ui-primitives-plan.md](ui-primitives-plan.md) (first customer — the
`petal-ui` prelude becomes the first standard modules), `docs/goals.md:145`
(modules previously "aspirational, not yet scoped"), `docs/ir-as-target.md`
(Schema v0 — module distribution as IR builds on it later).

## Motivating requirements

1. **`petal-ui` prelude** (immediate): embedders ship Petal-source libraries
   (`button`, `list_update`, palettes) that user scripts pull in without
   source concatenation — with correct per-file error lines and without the
   prelude polluting the user's namespace unless asked.
2. **User code sharing** (Garden's concrete pain): two panel scripts
   (`diff_panel.ptl`, `diff_detail_panel.ptl`) copy-paste their palette,
   draw shims, and truncation helpers because two scripts cannot share code.
   Users need `import` for their own files, resolved relative to the
   importing script.
3. **Embedder-provided modules without a filesystem**: petal-sdl embeds
   `browser.ptl` via `include_str!`; petal-web fetches a single source string.
   Module resolution must be pluggable — in-memory registration, not just
   disk paths — so wasm and embedded hosts work.
4. **Hot reload must keep working** (goals.md pillar: state-preserving live
   editing). Editing a module mid-session should transfer `state` exactly as
   single-file editing does today.
5. **Future**: separate compilation / distributing modules as IR
   (Schema v0+), visibility control, versioned packages. v1 must not paint
   these out, but does not build them.

## Current architecture (what the design must respect)

Findings from a survey of `rust/src` — these shape every decision below:

- **Pipeline is lex → parse → compile with no link step.** `Env::load_program`
  (`src/env/mod.rs:161`) runs `Lexer` → `Parser::parse_program()` →
  `Compiler::compile` (`src/compiler/mod.rs:132`) producing one
  self-contained `Program` (`src/program.rs:305`).
- **All ids are program-local array indices.** `TermId`/`BlockId`/
  `ConstantId`/`FunctionId` index that one program's vecs; `validate`
  (`src/program.rs:383`) enforces id == position. A `Stack` binds exactly one
  `program_id` (`src/stack.rs:70`); no cross-program call path exists
  (`GlobalTermId` at `src/program.rs:33` is dormant). ⇒ v1 must merge modules
  into one `Program` at compile time, not link separate programs at runtime.
- **Name resolution is compile-time and lexical.** `Compiler.scopes:
  Vec<HashMap<String, TermId>>` (`src/compiler/mod.rs:49`), innermost-out
  lookup, silent shadowing. There is no global function table — a top-level
  `fn` is a `MakeClosure` term bound in the global scope
  (`src/compiler/function.rs:11`). Forward references work only because
  `prescan_declarations` (`src/compiler/mod.rs:386-421`) pre-binds phantoms
  before statement compilation. ⇒ imports must be resolved *before* the
  prescan of the importing file.
- **Builtins are the late-bound exception.** Natives are registered as
  phantom terms in the same global scope (`src/compiler/mod.rs:152-160`);
  unshadowed call sites compile to `BuiltinCall(name_const)` resolved by
  string at runtime (`src/backend/graph/call.rs:43`). The unresolved-name
  hint table already says `"require" | "import" => "Petal does not have a
  module system yet"` (`src/compiler/expr.rs:211`).
- **Overloading is by arity, detected per-compile.** Prescan groups same-name
  decls; variants compile as `"name#arity"` joined by `MakeOverloadSet`
  (`src/compiler/function.rs:18-37`).
- **`state` keys are bare-name hashes in one flat namespace.**
  `hash_state_name` (`src/compiler/mod.rs:199`) hashes only the variable
  name; `transfer_state` (`src/transfer_state.rs:24`) preserves state whose
  `StateKey` survives recompile. ⇒ two modules declaring `state scroll = 0`
  would silently share one slot unless keys become module-qualified.
- **Spans have no file identity.** `SourcePosition { line, column, offset }`
  (`src/source_map.rs:12-23`) is relative to the single source string; error
  strings carry `[line N, column M]` only; the CLI re-parses them
  positionally (`src/cli.rs:865`).
- **The whole program body is the root block**, executed top-to-bottom; after
  the root frame completes, named callables are harvested into
  `stack.functions` (`src/backend/graph/mod.rs:386-404`) for
  `Env::call_function`. There is no definition/execution split — "importing"
  must define what happens to a module's top-level statements.
- **No search path exists anywhere.** The CLI reads one file
  (`src/cli.rs:367-388`); petal-sdl `include_str!`s + file-watches one script
  (`apps/petal-sdl/src/game_loop.rs:43,618-669`).

## Design

### Surface syntax

Two forms, both **only allowed before any other statement** in a file (keeps
resolution strictly ahead of the prescan and makes execution order obvious):

```petal
import ui                       // qualified: ui.button(...), ui.palette
import ui: button, clicked      // selective: bare button(...), clicked(...)
import ui as u                  // alias: u.button(...)
```

- Module names are identifiers. What they resolve *to* is the resolver's
  business (§ resolution) — the language does not know about files.
- `import` becomes a keyword (`src/lexer.rs:741-760`). Breakage risk is ~nil:
  the compiler already special-cases `import` as an unresolvable name with a
  "no module system yet" hint, so no working program uses it as an
  identifier.
- **No new expression syntax.** `ui.button(rect)` parses today as dot/method
  sugar; the compiler intercepts it when the receiver identifier is bound to
  a *module alias* (a new compile-time binding kind alongside term bindings)
  and resolves `button` in that module's export scope statically. Bare
  `ui.palette` (field position) resolves the same way. A module alias is not
  a runtime value — using `ui` where a value is expected is a compile error
  ("`ui` is a module").
- **Selective-import collisions are compile errors.** `import a: draw` when
  `draw` is already bound (by another import or a local decl below — caught
  at prescan) errors with both provenances. Silent shadowing stays as-is for
  ordinary lexical bindings; imports are explicit requests and deserve loud
  conflicts. Same-name-different-arity across an import boundary does *not*
  merge into one overload set in v1 (overload sets stay per-module); it is a
  collision.

### Exports

v1: every top-level `fn`, `enum`, and `let`/`state` in a module is exported.
Names starting with `_` are module-private (not resolvable from outside).
This matches Petal's minimalism; a `pub` keyword can tighten it later without
breaking v1 programs.

### Resolution: pluggable providers on `Env`

```rust
pub trait ModuleResolver {
    fn resolve(&self, name: &str, importer: Option<&ModuleOrigin>)
        -> Option<ModuleSource>;   // { name, source: String, origin }
}
impl Env {
    pub fn register_module(&mut self, name: &str, source: &str);  // in-memory
    pub fn add_module_path(&mut self, dir: PathBuf);              // filesystem
    pub fn set_implicit_imports(&mut self, names: &[&str]);       // see below
}
```

Resolution order: (1) embedder-registered in-memory modules — how `petal-ui`
ships its prelude (`env.register_module("ui", include_str!(...))`) and how
wasm hosts work with no filesystem; (2) the importing file's directory
(enables Garden panel scripts sharing a sibling `palette.ptl`); (3) registered
search paths / `PETAL_PATH`; CLI gains `-I <dir>`. First hit wins; a module
name resolves at most once per `load_program` (diamond imports dedupe).

**Implicit imports** replace the ui-primitives plan's
`load_program_with_prelude`: an embedder can declare modules that every
loaded program imports selectively-by-default (Garden marks `ui` implicit so
panel scripts call `button(...)` with zero ceremony; a script's own bindings
still win, and an explicit `import ui` is a no-op). This subsumes
concatenation entirely — same ergonomics, real file attribution.

### Compilation: merge-at-compile-time (v1)

`Env::load_program` becomes: parse the entry file; walk its imports
depth-first (cycle ⇒ compile error listing the cycle path), lexing/parsing
each module **independently** (so spans stay file-local); then run one
`Compiler` pass over the modules in **dependency post-order, then the entry
file**, into a single `Program`:

- Each module compiles inside its own **module scope frame**: its top-level
  bindings land in a per-module export map (`HashMap<String, TermId>`), not
  the entry file's global scope. Prescan runs per module, so in-module
  forward references and overloads behave exactly as today.
- The importer's scope gains *module alias* bindings (for `import m` /
  `as u`) and direct term bindings (for `import m: f, g`) pointing into the
  export maps. Since everything lives in one `Program`, imported bindings are
  ordinary `TermId`s — no relocation, no new backend work; **both backends
  run unchanged** (bytecode lowering is per-Program and sees one merged
  program).
- Natives stay global: builtin phantoms are registered once and visible in
  every module scope, same as today.
- **Top-level statements of a module execute once**, in dependency order,
  before the importer's body — they are simply compiled into the root block
  ahead of it. This is what makes `let palette = {...}` in a module work,
  and it is the exact semantics concatenation has today, minus namespace
  pollution. Modules should keep top-level side effects minimal by
  convention (the docs say so; the language doesn't police it in v1).
- Host-visible function names: `capture_root_functions` harvests module fns
  under their qualified name (`"ui::button"`), so `Env::call_function` can
  target them explicitly.

### `state` keys: module-qualified, entry file unchanged

`compile_state_decl` (`src/compiler/stmt.rs:142`) switches to
`hash_state_name(&format!("{module}::{name}"))` for module code; the **entry
file keeps bare-name hashing**, so every existing program's hot-reload state
survives this feature unchanged. Consequences, documented loudly:

- Two modules with `state scroll` no longer collide (the current flat
  namespace would silently share the slot).
- Moving a `state` decl between modules (or renaming a module) changes its
  key and drops that state on reload — same class of event as renaming the
  variable today. A future migration hook can use `diff_state`
  (`src/env/mod.rs:731-793`); v1 just documents it.
- `transfer_state` itself needs no change — it already operates on the
  `StateKey` space.

### Diagnostics: file identity in spans

- `SourceMap` gains a file table: `files: Vec<SourceFile { name, source }>`,
  and `SourceSpan` gains `file: FileId(u16)` with `#[serde(default)]` (file 0
  = entry), keeping Schema v0 IR readable and bumping the schema note in
  `docs/ir-as-target.md` to v0.1. Because each module is lexed independently,
  line/column are already file-local — only the tag is new.
- Error strings become `ui.ptl [line 3, column 5]: ...` (entry-file errors
  keep today's format so the CLI's positional `parse_line_column` and
  existing tooling don't break; a structured-diagnostics pass can follow).
- `petal::rewrite` (`src/rewrite.rs`) is unaffected: it splices individual
  source files before compilation, which is exactly the granularity modules
  preserve. Garden's `layout(...)` rewriting keeps working file-by-file.

### Hot reload across modules

`transfer_state` already re-hosts a stack onto a recompiled program under the
same `ProgramId` (`src/transfer_state.rs:31-45`); with merge-at-compile-time,
"recompile" just means re-running the module walk. Two host-side additions:

- `Env` records each program's **module manifest** (name → origin/mtime or
  content hash) so hosts can ask "does editing file X invalidate program P?".
- Watchers (petal-sdl `setup_watcher`, Garden's panel reload) watch the
  manifest's files, not just the entry file. Editing `palette.ptl` hot-reloads
  every panel that imports it — this is the payoff for requirement 2.

### What v1 explicitly defers

- **Separate compilation / IR linking.** Distributing a module as Schema-v0
  IR requires relocating program-local ids and merging constant tables, or
  real cross-program addressing through both backends' call paths
  (`GlobalTermId` is the seed). Design keeps the door open — module scopes
  and qualified state keys are the same shape a linker would need — but v1
  always compiles from source.
- **`pub` / visibility beyond `_`-prefix.**
- **Packages, versions, registries.** `ui_version()` (ui-primitives plan)
  covers the one immediate compat need.
- **Overload-set merging across modules.**
- **Conditional / non-top imports.**

## Delivery phases

1. **Front end + resolution**: `import` keyword and `StmtKind::Import`
   (parser enforces imports-first), `ModuleResolver` + `register_module` /
   `add_module_path` / implicit imports on `Env`, DFS module walk with cycle
   and not-found errors (reusing the existing hint at
   `src/compiler/expr.rs:211` for the not-found case). CLI `-I`.
2. **Compile-time merge**: per-module scope frames + export maps, module
   alias bindings and the dot-resolution intercept in `compile_expr`
   (`src/compiler/expr.rs:96-121` area), selective-import collision errors,
   module-qualified state keys, qualified `capture_root_functions` names.
   Both backends should pass the existing suites untouched.
3. **Diagnostics**: `FileId` in `SourceSpan`/`SourceMap`, file-prefixed
   errors, IR schema v0.1 note. Multi-file cases in `test/` (the harness
   gains its first multi-`.ptl` directories).
4. **Hot reload + hosts**: module manifest on `Env`, petal-sdl watcher
   watches manifest files; migrate `browser.ptl`'s launcher and the
   `petal-ui` prelude from concatenation to `register_module` + implicit
   import (retiring `load_program_with_prelude` before it's ever built —
   if ui-primitives Phase 2 lands first, its concatenation path is the
   temporary bridge).
5. **Garden adoption** (downstream): `PanelHost` registers `ui` as implicit,
   adds the panel script's directory to the module path; the diff panels
   split their shared palette/helpers into an imported module. Future:
   IR-distributed modules once separate compilation is scoped.

## Test plan

- Unit: resolver ordering, cycle/not-found/collision errors, module-qualified
  state-key stability (reorder decls, rename module, move state between
  files), alias vs selective binding resolution, `_`-private enforcement.
- Golden: multi-file `test/*/` cases through both backends (`PETAL_BACKEND`),
  asserting identical results graph vs bytecode.
- Hot reload: scripted edit of an imported file → `transfer_state` preserves
  qualified state; edit that renames a module → state documented-dropped.
- Embedding: an `Env` with only in-memory modules (wasm-shaped, no
  filesystem) compiles a program importing them.
