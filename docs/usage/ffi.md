# Petal FFI and embedding

How a host application talks to the Petal runtime: the API on `Env`, native
functions, the value model, the host channels, and retained state. For
task-oriented patterns built on these primitives, see
[embedding-guide.md](embedding-guide.md); to embed from C or C++ rather than
Rust, see [embedding-c.md](embedding-c.md). Whether to embed at all, or to
build on a host that already exists, is in [building-apps.md](building-apps.md).

The design is Lua-inspired. Native functions are registered by name against a
stack-style calling convention (`core/src/native_fn.rs`). Everything crosses
the boundary **by value**, except for [handles](#handles), which are opaque
references to host-owned objects.

## The embedding lifecycle

Every embedder (petal-sdl, the wasm runtimes, the headless test harness)
follows the same shape:

```rust
let mut env = Env::new();
env.register_native("spawn_particle", native_spawn_particle, NativeEffects::EMITS); // host functions
env.register_package("game", [("game", include_str!("game.ptl"))])?; // the interface package: declares the natives
let pid = env.load_program_at(&source, &path)?;               // compile (walks imports)
let stack = env.create_stack(pid)?;

loop {                                                        // per frame / tick
    env.set_binding(dt_sym, Value::Float(dt));                // host → script uniforms
    env.reset_counter(canvas_id_sym, 0);
    env.reset_stack(stack);
    env.run(stack)?;                                          // re-run the whole program
    let cmds = env.take_output_buffer(draw_sym);              // script → host commands
}
```

Entry points, all on `Env` (`core/src/env/`):

| Concern | API |
|---|---|
| Native functions | `register_native(name, func, effects) -> NativeFnId`, `register_native_boxed(name, closure, effects) -> NativeFnId`, `native_effects`, `set_native_class`, `declare_natives(source)`, `declare_native_params(id, spec)`, `declare_native_params_by_name(name, spec)`, `set_native_signatures(id, sigs)`, `native_signatures(name)` |
| Linking | `set_check_only(bool)`, `is_check_only()`, `link_program(&program)` |
| Handles | `register_handle_class`, `make_handle` |
| Modules and packages | `register_module`, `add_package`, `register_package`, `add_module_path` (a directory of packages), `set_inline_import_dir`, `set_install_on_demand`, `override_file_source`, `packages`, `project_of`, `project_dependencies`, `resolve_import`, `package_errors`, `verify_interface` (see [Modules and packages](#modules-and-packages)) |
| Host interfaces | `provides(name, version)`, `provides_package(name)`, `provided_interfaces`, `check_requires(&program)` |
| Programs | `load_program`, `load_program_at`, `load_program_diag`, `compile_program`, `compile_program_at`, `compile_program_diag`, `load_program_ir` |
| Execution | `create_stack`, `run`, `run_bounded`, `reset_stack`, `call_function`, `call_function_diag`, `has_function` |
| Host→script data | `intern_symbol`, `set_binding`, `clear_binding` |
| Script→host data | `take_output_buffer`, `output_buffer`, `take_output` (print lines) |
| Id allocation | `reset_counter`, `next_counter` |
| State tooling | `get_state_json`, `set_state_from_json`, `snapshot_state`, `restore_state`, `diff_state` |
| Observation | `observations_mut().enable()`, `get_observations_json` — the last value bound to every named term (see [embedding-guide.md](embedding-guide.md#reading-arbitrary-named-values-observation)) |
| Emit tracing | `enable_emit_trace`, `take_output_origins` (see [../features/direct-manipulation.md](../features/direct-manipulation.md)) |
| Speculation and rewind | `fork_execution`, `run_speculative`, `restore_execution`, `drop_fork`, `diff_state`; to drive a fork by hand, the per-stack `set_binding_for`, `take_output_buffer_for` and `take_output_for` (see [../features/time-travel.md](../features/time-travel.md#what-the-runtime-provides)) |
| Hot reload | `reload_program`, `set_config_value`, `program_source`, `override_file_source`, `diff_program`, `module_manifest`, `program_source_paths`, `watch_program_sources` (→ `source_watch::SourceWatch`), `transfer_state` (see [Hot reload](#hot-reload)) |
| Static values | `static_values`, `clear_static_values`, `set_static_cache`, `set_static_follow`, `set_static_debounce`, `flush_static_rebuilds`, `take_static_warnings`, `take_all_static_warnings`, `native_constness` (see [Constant natives](#constant-natives)) |
| Diagnostics | `check_program`, `diagnostics_text`, `error_diagnostics_text` (see [embedding-guide.md](embedding-guide.md#showing-what-petal-check-would-say)) |
| Reactive layers | `set_policy`, `run_needed`, `note_host_data_changed`, `set_effect_audit`, `effect_audit_report`, `set_static_audit`, `static_audit_report` (see [Embedder pitfalls](#embedder-pitfalls)) |

`run_bounded` returns `RunOutcome::Done | Yielded`, so a 60fps host can slice a
long computation across frames. `call_function(stack, "name", args)` calls a
top-level Petal function by (possibly module-qualified) name after at least one
`run`; this is the host-to-script call direction.

**Typed errors.** The `String`-returning entry points each have a `_diag`
twin with a typed error, and the string is always that error's `Display`:

- `load_program_diag(source, origin)` / `compile_program_diag(pid, source,
  origin)` → `Result<_, error::LoadError>`: the phase that rejected the
  program (`Lex`, `Parse`, `Module`, `Compile`, `Lower`) and each diagnostic
  with its message, span (line/column) and file. `compile_program_diag` is the
  recompile half of a hot reload — compile, then `transfer_state`.
- `call_function_diag(stack, name, args)` → `Result<Value,
  error::CallError>`: `StackNotFound`, `FunctionNotFound { name }` (not
  defined, or the program has not been `run`), or `Runtime(msg)` (it ran and
  failed — including an arity mismatch). A host calling an optional hook
  matches `FunctionNotFound` (or asks `has_function` first) instead of
  sniffing the message. `CallError` converts into `String`, so `?` still
  works in a `Result<_, String>` host.

**Watching source files.** `program_source_paths(pid, entry)` lists every file
on disk a loaded program was compiled from — the entry path the host loaded
it from (if any) first, then each manifest module with a file origin, deduped
by filesystem identity; in-memory modules are skipped.
`watch_program_sources(pid, entry)` stamps them (mtime + length) into a
`SourceWatch`, whose `changed()` / `changed_paths()` a host polls each frame;
after a reload, take a fresh watch from the new program (its imports may have
changed). A host that prefers OS notifications watches the parent directories
of `program_source_paths` instead (petal-desktop-sdl's `watcher.rs`).

## Native functions

```rust
pub type NativeFn = fn(&mut PetalCxt) -> NativeResult;   // core/src/native_fn.rs
pub type NativeResult = Result<u32, String>;             // Ok(count of pushed results)
```

A native is a plain, non-capturing `fn` pointer. `Env::register_native`
appends it to the native table (the id is the table index) and **must be
called before the load that compiles a call to it** (a reload that recompiles
sees natives registered since the first load). Registering makes the native *exist*; it does
not put its name in scope. A script reaches it through a `native fn`
declaration ([below](#native-interfaces-native-fn)) — in practice, by
importing the interface package the host registers beside it — and from there
the name resolves through ordinary scope lookup and can be shadowed.

**Natives with captured state.** `Env::register_native_boxed(name, closure,
effects)` registers a closure (`impl Fn(&mut PetalCxt) -> NativeResult +
'static`, stored as `native_fn::BoxedNativeFn`) instead: the host's userdata
rides in its captures, so one Rust function can serve any number of natives —
a C bridge forwarding each to its own callback, a host registering one native
per command — with no trampoline pool or global dispatch table. It shares the
id space, the effect row, `set_native_class`, the `Pending` interception and
`--effect-audit` with bare natives; a bare `fn` native still dispatches
through its pointer. Three rules:

- It is `Fn`, not `FnMut`: mutate captures through `Cell`/`RefCell`. It need
  not be `Send` (an `Env` is not).
- The runtime cannot see what the closure does to its captures. Mutating them
  is an `effect`; answering from captured state the host changes is a read
  (`HOST_DATA`, and `note_host_data_changed` when it moves). The effect audit
  observes only what the call does through its `PetalCxt`.
- Captures belong to the `Env`, not to an execution: a forked or speculative
  run calls the same closure. They are dropped with the `Env`, which is the
  hook for freeing host userdata.

**Parameter names are opt-in.** A native reads its arguments by index and, by
default, takes them by position only: a script that writes
`spawn(kind: "spark")` gets `builtin 'spawn' does not accept named arguments`.
Declaring the names lifts that, without touching the native:

```rust
let id = env.register_native("spawn", native_spawn, NativeEffects::EMITS);
env.declare_native_params(id, "kind, x, y, speed?")?;
// script: spawn(kind: "spark", x: 10, y: 20)   → the native sees ("spark", 10, 20)
```

The spec lists the names in the order the native reads its arguments,
comma-separated. A trailing `?` marks an optional parameter — one the native
checks `arg_count()` for — and optional ones must come last. Named arguments
are permuted into that order before the call (at compile time for a direct
call, in the VM when the callee is a value, a method-syntax call or a pipe), so
the native is handed exactly the list the positional call would have passed and
the all-positional path is untouched. What the binding rejects, in the words a
Petal `fn` uses: an unknown name, a parameter given twice, a required
parameter left out, and an optional one skipped to reach a later one
(`spawn() is missing a value for parameter 'x'`).

An optional parameter is the native counterpart of a Petal `fn`'s
[default value](../syntax/function-overloading.md#default-parameter-values), minus the
expression: the native itself supplies whatever a missing argument means. Too
many arguments reads as it does for a `fn` with defaults
(`spawn() expects 3-4 arguments, got 5`).

A native that dispatches on its argument count declares each form with its own
call, and a named call takes the first form it fits. A call that fits none is
reported the way an overloaded `fn` reports it — by the form with exactly that
many parameters when there is one; else by listing the forms, when the count
suits one and the names suit none (`range() has no variant that accepts 2
arguments with some named 'start', 'stop' (variants: range(end), range(start,
end, step?))`); else by the counts (`distance() expects 4 or 2 arguments, got
3`):

```rust
env.declare_native_params(id, "x1, y1, x2, y2")?;   // distance(x1: …, …)
env.declare_native_params(id, "v1, v2")?;           // distance(v1: a, v2: b)
```

`declare_native_params_by_name(name, spec)` does the same by registered name,
and `native_signatures(name)` reads the declarations back. Declare before
`load_program`. A genuinely variadic native (`print`) has no roles to name and
should declare nothing.

These calls are the low-level form. The usual way to give a host's natives
their parameter names — and the only way `petal check` learns them, since it
never sees the host's `Env` — is to *declare* the natives in Petal source, as
the next section describes, and hand that source to `Env::declare_natives`.

## Native interfaces: `native fn`

A native registered in the table is a bare function pointer: a name and an
effect row. What it *takes* is written in a declaration, in Petal source:

```petal ignore
native fn sqrt(x: num) -> float
native fn slice(collection, start, end?)
native fn print(...values)
pub native fn mouse_x() -> int
native fn _native_rect(x: num, y: num, w: num, h: num, r: num, g: num, b: num, a?: num) = "draw_rect"
```

`native fn name(params) -> type` has no body and no `end`. It binds `name`,
in the module that declares it, to the host native of the same name — or, with
a trailing `= "host_name"`, to the native registered under that other name.
`pub native fn` exports the name like any other declaration, so importers
reach it bare, selectively (`import ui: mouse_x`) or qualified (`ui.mouse_x()`).
`native` is a contextual keyword: it is one only directly before `fn`, and an
ordinary name everywhere else. A declaration belongs at the top level of a
file and is in scope throughout it, whatever line it is on.

**Parameters.** Each is `name` or `name: type`, with the type vocabulary of
any other annotation. `name?` (`name?: type`) marks a trailing parameter a
call may leave off — the native counterpart of a default value, minus the
expression, since the host fills the gap itself. The list may end in a rest
parameter, `...name`, for a native that takes any number of further arguments
(`print`, `format(template, ...args)`); such a native takes arguments by
position only. A parameter name is only ever a call's argument label, so a
keyword works (`end`).

**Several declarations of one name are call forms.** A native that reads its
arguments differently by count or by kind declares each reading:

```petal ignore
native fn random() -> float
native fn random(max: num) -> float
native fn random(min: num, max: num) -> float
native fn text_width(text: string, style) -> int
native fn text_width(text: string, size, font?) -> int
```

Unlike the overloads of a Petal `fn`, the forms may take the same number of
arguments: they are one native, and a named call uses the first form its names
fit (the rule above). Where forms overlap, the checker holds an argument only
to the type every form taking that count agrees on. All declarations of a name
must bind the same host native.

**A native can share an overload set with Petal `fn`s** of other arities,
which is how a module wraps a native in more convenient shapes:

```petal ignore
native fn clear(r: num, g: num, b: num)
fn clear(c) clear(c.r, c.g, c.b) end
```

`clear(0, 0, 0)` reaches the native and `clear(color)` the `fn`. The two may
not take the same number of arguments, and a rest-parameter native cannot be
part of such a set (an overload is chosen by argument count). Under the hood
the native becomes forwarding `fn`s of that name, so the set behaves exactly
as any overloaded function does — hoisting, exports, values, named arguments.

**A declaration is the only way to a native.** A script reaches a native
through a `native fn` in scope — one written in the file, or one an imported
module exports — and through nothing else. Registering a native in the `Env`
does not put its name in any script's scope: a call to an undeclared name is
`Unknown function: x`, a read of one `Undefined variable: x`, whether or not
the host registers a native called `x`. The core builtins are the one set in
scope everywhere, because the bundled `core` package declares them and is
imported implicitly; the domain builtins (`lerp`, `vec2`, `noise`, …) follow
the general rule and are in scope where their package is imported
([Builtins](../Builtins.md#which-builtins-need-an-import)). So a host ships
its declarations as an **interface package**, registers it, and its scripts
import it ([../module-system.md](../module-system.md#host-interface-packages)).

**What a declaration is used for.**

- *Binding.* A bare call through a declared name compiles to a static call of
  the host native, whatever the script calls it.
- *Checking.* The checker takes parameter names, arity, parameter types and
  the result type from the declaration: `sqrt("x")` warns that argument 1
  expects a number, `let s: string = mouse_x()` that an `int` is not a
  `string`, `mouse_down(btn: 0)` that there is no parameter `btn`. Annotate
  only certainties — a parameter type is a claim that the native fails on
  anything else. A `num` parameter also takes a dual number, a `record` one a
  class instance. A handful of builtins whose *result* depends on their
  arguments (`abs`, `floor`, `ceil`, `round`, `sign`, `min`, `max`, `clamp`,
  the float functions on a dual, the vector natives, `lerp`) are worked out by
  the checker itself and declare no result type; so is `len`'s argument (a
  list, a string or an `f64_array`, which no one type name says).
- *Named arguments at run time.* Registering an interface — `register_module`,
  `register_package`, `add_package` — gives every native already registered
  the parameter lists the interface declares for it, replacing any it had. So
  a host registers its natives first and its interface package after:

  ```rust
  host::register_natives(&mut env);
  env.register_package("myhost", [("myhost", INTERFACE_PTL)])?;
  env.verify_interface(&["myhost"])?;   // the drift test: declared == registered
  ```

  `Env::declare_natives(source)` is the same step by hand, and returns the
  declared natives that are *not* registered.

  (A direct call is put in positional order at compile time from the
  declaration alone; this is what makes `let f = spawn` then `f(kind: "x")`,
  and method-call syntax, bind by the same names.)

**Linking.** Loading a program into an `Env` — `load_program*`,
`compile_program*`, a hot reload — checks that every native the program
*refers to* through a `native fn` declaration is registered there. If one is
not, the load fails, before any line runs, with a `Phase::Link` error naming
each declaring module and its missing natives (a few, then a count):

```text
module 'game' needs native fns this host does not register: spawn, despawn, raycast,
set_camera, +9 more — the program cannot run here; run it with a host that provides them
```

For the `ui` package the message also says which kind of host does:
`petal run` registers the builtins only, so a script that imports `ui` is
refused there and told to use a petal-ui runner.

"Refers to" is static: a call or read anywhere in the program's modules
counts, inside a function that never runs included. A declaration nothing
refers to links nothing, so a host may provide part of an interface to
scripts that use only that part.

A tool that only *checks* a program must not need the natives:
`Env::set_check_only(true)` compiles without linking. `petal check`, the LSP,
`petal suggest`, `petal apply-change` and the static `show-*` tools do this;
a declared native they do not register is still a known name with known
parameters. (`Env::link_program(&program)` runs the check by hand, and
`Program::native_links` lists what a program needs.)

**Providing an interface.** A package states the host interfaces it needs in
its manifest (`[requires] myhost = ">=0.2"`,
[../packages.md](../packages.md#requires)); the host
announces what it provides where it registers the natives —
`env.provides("myhost", "0.2.0")`, or `env.provides_package("myhost")` to take
the version from the interface package's own `petal.toml` (registered with the
package as a source named `petal.toml`). Loading a program to run checks each
requirement before linking: an interface the host does not provide, or
provides at a version the requirement rejects, is a load error (phase
`link`). A check-only `Env` skips it, and `Program::requires` lists what the
program needs; `Env::check_requires(&program)` runs the check by hand. Every
`Env` provides `core`.

**Verifying an interface.** `Env::verify_interface(&["ui", "myhost"])` checks
the whole table against the packages that declare it, both ways — every
declared native is registered, every native registered beyond the builtins is
declared — and returns an `InterfaceMismatch { unregistered, undeclared,
problems }` otherwise. Name every interface package the `Env` answers for in
the one call; a name may also be one module of a package (`ui/input`). It is
stricter than linking on purpose: linking asks what one program needs, this
asks whether the host and its interface have drifted apart. Each host in this
repository asserts it in a test (`register_all_agrees_with_the_ui_interface_package`
in petal-ui, and the like in garden-script, petal-desktop-sdl and the example
hosts).

**The builtins are declared the same way.** The global ones are declared by
the `core` package (`core/packages/core/src/*.ptl`: `basics`, `collections`,
`math`, `format`, `introspect`, `handle`, `pending`, `classes`), whose
declarations are in scope in every file with no import. The domain builtins
are declared by one package each — `noise`, `color`, `vec2`, `autodiff`,
`json`, `io`, `creative` (`core/packages/<name>/<name>.ptl`) — and are in
scope where the package is imported. All of them are compiled into the binary
by `core/build.rs`, which fails the build if a directory is missing, and every
`Env` registers every one of these natives, so importing a builtin package
always links. A test (`core/src/builtins/decls.rs`) holds the packages to
exactly the natives `register_builtins` registers, and every declared form to the argument counts
the native really takes. petal-ui's natives are declared by its `ui` package
(`core-libs/petal-ui/prelude/ui.ptl` and `input.ptl`), which is bundled in the
binary too, so `petal check` reads it with no host.

**Every native declares what it does.** The third argument is its
`NativeEffects` row: what it reads (`InputClasses`: `POINTER`, `KEYBOARD`,
`CLOCK`, `VIEWPORT`, `HOST_DATA`, `RESOURCES`, `RNG`, `BINDINGS`), whether the
call is a `probe` (a pure function of its arguments and those reads, safe to
re-evaluate), whether it `emits` into an output buffer, whether it has an
`effect` no replay could reproduce, and its `Pending` policy. The reactive
layers — the frame gate, memoized scopes — take the row at its word, so a
native that reaches host state without saying so looks pure and every
memoized scope that calls it replays stale. The row is the union over every
path through the native. Start from `NativeEffects::PURE`, `EMITS` or
`EFFECT`, or `NativeEffects::probe(class)` / `reads(class)`, and add
`.with_effect()` / `.with_emits()` / `.with_pending(..)`:

```rust
env.register_native("mouse_x", native_mouse_x, NativeEffects::probe(InputClasses::POINTER));
env.register_native("draw_rect", native_draw_rect, NativeEffects::PURE.with_emits());
env.register_native("query", native_query, NativeEffects::reads(InputClasses::HOST_DATA));
env.register_native("save_file", native_save_file, NativeEffects::EFFECT);
```

**`CONST` is a separate, opt-in declaration.** `NativeEffects::CONST` is
`PURE` plus one claim: the native is a *constant function* — the same
arguments give the same result for the life of the program, it reads no
dynamic input of any kind, and it does nothing but return. Petal uses it to
decide which values are *static values*
([features/static-values.md](../features/static-values.md)), computed once
and kept until the source changes. `PURE` does not imply it, and a native without it is always
treated as dynamic. Read [Constant natives](#constant-natives) before
declaring one.

A native that answers from host state outside the binding table
(`HOST_DATA`) must also call `PetalCxt::note_host_read()` — the row is what
the memo consults, the call is what the frame gate records — and one with an
effect the `PetalCxt` methods do not already report (`print`, the counters,
the mutable resource table do) calls `note_effect()`. `petal run
--effect-audit` and `petal-ui-run --effect-audit` hold what every native was
observed doing against its row and name any facet the row lacks. What
"honest" means for a native that touches host state, and what the audit
cannot see, is under [Embedder pitfalls](#embedder-pitfalls).

`PetalCxt` is the per-call context. Argument readers are 1-indexed like Lua
(`get_int(1)`, `get_string(2)`, `get_value`, `get_symbol`, `get_handle`, …);
results are pushed (`push_int`, `push_value`, `push_nil`, …). It also exposes
the host channels (`binding_named`, `push_output`, `emit`, `next_counter`),
`heap`/`heap_mut`, the call's origin term (`origin()`, used by emit tracing),
and the `in_place` flag (see [Immutability](#immutability-and-the-in-place-gate)).

**Method-call syntax reaches natives.** `obj.method(args)` resolves, in order:
a callable field on a record receiver; a handle class's own `call_method`
dispatcher for a handle receiver (which also rejects stale handles); otherwise
**UFCS fallback**, where the method name names a native that is called with
the receiver prepended. The fallback follows the same rule as a bare call: it
reaches a core builtin anywhere, and a domain builtin or a host native where a `native fn` of that name
(declared under the native's own name) is in scope in the file that writes
the call. So with `set_location` registered and declared, a script that
imports the interface may write `set_location(obj, p)` or
`obj.set_location(p)`. The namespace is flat: one native table for all
receiver types.

**Native classes.** The row's `pending` field (`NativeClass`) says how the
native treats a `Value::Pending` argument: `Strict` (the default) absorbs and
returns the pending value, `Effectful` makes the call a no-op that emits
nothing, and `AllowPending` runs normally. Emitters should be `Effectful`
(`NativeEffects::EMITS` is). `env.set_native_class(id, ..)` changes it after
registration.

The compiled-in builtins (`core/src/builtins/`) go through the same table.
`map`/`filter`/`reduce`/`forEach` register placeholders and are dispatched
specially by the VM because they call back into closures; ordinary natives
**cannot call Petal closures**.

## Modules and packages

What a host must register for its scripts' imports to resolve is in
[../module-system.md](../module-system.md#embedding). These calls ask what
is there:

```rust
env.packages();                       // Vec<PackageInfo>: every registered package
Env::bundled_packages();              // the packages compiled into the binary
env.project_of(&file)?;               // Option<PackageInfo>: the package whose petal.toml
                                      // is nearest at or above `file`; Err if it does not load
env.project_dependencies(&file);      // that manifest's [dependencies] as they resolve:
                                      // Vec<(import name, Result<PackageInfo, String>)>
env.resolve_import("./level", &file)?; // the canonical module name `import ./level`,
                                      // written in `file`, resolves to; Err says why not
env.resolve_module_file("./level", &file); // the file it resolves to, or None for an
                                      // in-memory module
env.package_errors();                 // manifests add_module_path could not load
```

Each runs the same resolution a load does, so the answers are what the next
`load_program_at` of that file would see.

**A custom resolver.** `Env` always resolves through its own
`ModuleRegistry`. A tool that resolves imports some other way (from a
database, over the network) implements the `ModuleResolver` trait in
`core/src/module.rs` and calls `petal::module::load_modules(entry_source,
entry_path, &resolver, gated_imports)` itself. `resolve(path, importer)` is
handed the path as written and the importing module's name and origin, and
returns the source with the module's canonical name; an `Err` is the reason
nothing was found. The trait's other methods (`parse_cache`, `begin_load`,
`project_error`, `requires_of`) have defaults.

**Providing `ui` piecemeal.** `petal_ui::register_all` registers the input
and draw natives and the `ui` package and provides the `ui` interface. A host
that registers those pieces itself calls `petal_ui::provide_interface(env)`
once it has registered all of them, so a package whose manifest says
`[requires] ui = ">=…"` (bloom does) loads. A host that registers only the
input half, for `import ui/input`, does not call it.

## Values and the heap

`Value` (`core/src/value.rs`) is a `Copy` enum of 24 bytes; anything bigger
lives in the `Heap` behind a typed id (a slot index plus a generation):

```
Nil, Bool, Int(i64), Float(f64), Vec2(f64, f64), Dual { value, derivative },
String(StringId), List(ListId), F64Array(F64ArrayId), Vec3(Vec3Id), Map(MapId),
Dict(DictId), Closure(ClosureId), OverloadSet(..), NativeFunction(NativeFnId),
EnumVariant { tag: StringId, data: ListId }, Element(ElementId),
Symbol(SymbolId), Cell(CellId), Handle(HandleVal), Pending(PendingId)
```

An `EnumVariant` is its tag and its positional payload, nothing else. A
variant of a script's `enum` has the tag `Enum.variant` (`"Shape.circle"`);
the names of its payload fields and its constant data belong to the
declaration and are looked up in the running program. A tag the host makes up
for an output buffer (`"rect"`, `"spawn"`) has no enum in front and belongs to
no declared enum, so a script cannot write a pattern for it.

Notes for embedders:

- `Vec3` is heap-allocated (three f64s would widen every `Value`): build one
  with `heap.vec3_value(x, y, z)` and read it with `heap.get_vec3(id)` →
  `[f64; 3]`. It is immutable, so the id behaves as a value. See
  [../dev/vec3.md](../dev/vec3.md).
- `Map` is a **record** (`{a: 1}`, `type()` `"record"`); `Dict` is a **map**
  (`Map()`, `type()` `"map"`), whose keys are any hashable value compared by
  content. Build one with `heap.dict_from_pairs(pairs)` (a `Vec<(Value,
  Value)>`; it fails on an unhashable key: a function, a pending value, NaN),
  read it with `heap.get_dict(id)` (entries in insertion order: `.iter()`,
  `.keys()`, `.values()`, `.len()`) or `heap.dict_get(id, key)`, and change
  it with `heap.dict_set` / `heap.dict_remove`, which return a new id like the
  other collection ops. `heap.as_dict(value)` is the checked accessor.
- `Cell` is the box behind a `var`; it never reaches host code, because every
  read dereferences it (see [../syntax/var.md](../syntax/var.md#containment)).
- `Pending` is an unresolved resource (loading or errored). Ordinary operations
  absorb it and return it.
- The heap (`core/src/heap.rs`) is mark-and-sweep, triggered by allocation
  count. GC roots include stack registers, persistent state, closure captures,
  **bindings and output buffers**, so values parked in the host channels stay
  alive.
- Heap collection ops are copy-on-write: `list_append`, `map_set`, etc. return
  a new id. `Heap::fork` deep-clones the whole heap for speculative execution,
  which is sound because objects are immutable by construction.

## The three host channels

The script's side of these channels is [host-io.md](host-io.md).

All host↔script data flows through symbols. `env.intern_symbol(name)` returns
a `SymbolId`; the host and the script share an id by interning the same name.

1. **Bindings** — host→script values, like GLSL uniforms. The host calls
   `env.set_binding(sym, value)` before a run; scripts read them with the
   `binding` builtin (`import io`; `binding(symbol("dt"))`), natives with `PetalCxt::binding_named`. Used for input
   snapshots, `dt`, `frame_count`, screen dimensions.
2. **Output buffers** — script→host command streams. A native calls
   `cxt.emit(sym, tag, data)`, which appends `EnumVariant { tag, data }` to the
   buffer for `sym` (or `cxt.push_output(sym, value)` for an untagged value).
   The host drains it after the run with `env.take_output_buffer(sym)`. This is
   how all rendering works: draw natives do not draw, they emit.
3. **Counters** — per-run monotonic id allocators (`reset_counter(sym, start)`
   / `next_counter(sym)`), used to hand scripts fresh integer ids (offscreen
   canvas ids, element ids).

## Referencing host resources

There are two ways for a script to refer to something the host owns.

### Integer ids

Allocate an id from a counter, pass it as `Value::Int`, and keep the
id→resource table host-side. This suits frame-scoped resources:

- Offscreen canvases (petal-ui): `create_canvas()` returns an int from a
  per-frame counter; `draw_to(id)`, `draw_canvas(id, …)`, `snapshot_to(id, …)`
  reference it. The host materializes render targets from the command stream;
  the id is an index into command order and resets every frame.
- DOM elements (petal-web-html): `next_id()` ids round-trip through `data-eid`
  attributes and come back via a `clicked_id` binding.

Nothing detects a stale integer id; safety is the host table's discipline.

### Handles

For retained, host-owned objects (a game entity, an actor, a file), use a
**handle**: `Value::Handle(HandleVal { class, slot, serial })`. The host owns
the object; the handle is an opaque address into the host's own storage,
typically a slot map with generation counters. A script calls methods on it,
which go to the class's `call_method`, and a handle whose object is gone is
rejected as stale.
[game-engine-features/handles.md](../game-engine-features/handles.md) is the
full treatment: the model, what a script can do with a handle, where one can
be kept (`state`, state JSON, `const`, hot reload, memoized scopes), effect
rows, the C bridge and a worked example.

```rust
use petal::{HandleClass, HandleClassId, HandleVal};

let actor_class: HandleClassId = env.register_handle_class(HandleClass {
    name: "Actor".into(),
    is_valid: Box::new(|slot, serial| world.is_live(slot, serial)),
    describe: Box::new(|slot, serial| format!("Actor {slot}#{serial}")),
    call_method: Box::new(|cxt, method| { /* dispatch on `method`, receiver is arg 1 */ }),
});

// Hand one to a script (e.g. push it from a native, or set it as a binding):
let h: Value = env.make_handle(actor_class, slot, serial);

// Read one back inside a native, checked for class and liveness:
let h: HandleVal = cxt.get_handle(1, actor_class)?;
```

## Retained state

Scripts keep state across runs with the `state` keyword
([../features/inline-state.md](../features/inline-state.md) is the script
side: declaring, keying, lifetime and reset):

```petal
state score = 0            // top level: one slot, initialized on first run only
state var hp = Map()       // one slot holding a map: one entry per entity id
```

What a host needs to know about it:

- Storage is `Stack::state`, a map on the stack that survives `reset_stack` +
  `run`. One map per stack.
- A runtime key is `RuntimeStateKey { base, path }`. `base` is the
  **declaration id**: a hash of the declaration's full name path (module
  qualifier, enclosing function chain, variable name: `"score"`,
  `"ui::scroll"`, `"ui::draw/row"`), so declaration order does not matter.
  `path` is the chain of callsites and loop iterations that reached the
  declaration ([state is nested](../features/inline-state.md#state-is-nested));
  it is empty for a top-level declaration. The deprecated `state(key)` keys by
  the value and ignores it. The design is in
  [../dev/state-call-paths.md](../dev/state-call-paths.md).
- After each run, `sweep_untouched_state` drops keys the run did not touch, so
  state for deleted code, removed list items, or paths an edit no longer
  produces does not leak.
- `get_state_json` / `set_state_from_json` serialize state for tooling, and
  `snapshot_state` / `restore_state` / `diff_state` copy and compare the whole
  map ([time-travel.md](../features/time-travel.md#what-the-runtime-provides)
  has when to use a snapshot and when a fork). `Env::get_state`/`set_state` and the JSON setters address **top-level
  slots only**; a pathed slot has no name to address it by and renders in
  `get_state_json` under its path (`[3]/row/hovered`).

**Host-invoked functions get a root path of their own.** `Env::call_function`
runs with no caller frame, so it starts a path derived from the function's
qualified name. Repeated host calls of the same function share slots with each
other, but not with in-program calls of that function. To share a value across
both, put it in a top-level `state var` and read it with `get`.

## Hot reload

One call brings a running program up to date with its source:

```rust
let report = env.reload_program(stack, &source, Some(&path))?;
// report.outcome: Unchanged | Relocated | Patched | Recompiled
```

`reload_program` diffs each source file against what the program was compiled
from and does only the work the edit calls for; whichever path it takes, the
program and its state afterwards are what a full recompile would have left.
What each outcome means, keeps and costs is in
[../features/hot-reload.md](../features/hot-reload.md). On the host side:

- **Knowing when to call it.** `watch_program_sources` (polling) or
  `program_source_paths` (for OS notifications) cover the entry file and
  every imported module that came from disk, as described under
  [The embedding lifecycle](#the-embedding-lifecycle); the loop is in
  [embedding-guide.md](embedding-guide.md#hot-reload). `module_manifest(pid)`
  lists every source of a program, in-memory ones included (name, origin,
  content hash).
- **Errors.** A failed reload returns a `LoadError` and leaves the old
  program running.
- **The report.** `ReloadReport` says which path the reload took, what
  changed, what state and static values it kept, and what the edit
  introduced; its fields are listed in
  [The reload report](../features/hot-reload.md#the-reload-report). Print
  `report.static_lines()` after a reload, as the SDL host does.
- **A value without a file write.** `set_config_value(stack, file, path,
  &value)` sets one literal of a top-level binding in the running program (a
  slider drag), and `program_source(pid, file)` returns the edited text to
  save when the drag ends
  ([Setting a value without writing the file](../features/hot-reload.md#setting-a-value-without-writing-the-file)).

The recompile is also available on its own:

```rust
let new_program = env.compile_program_at(pid, &source, &path)?;
let result = env.transfer_state(stack, new_program)?;  // { state_preserved, state_dropped }
```

`transfer_state` keeps every state value whose declaration id still exists in
the new program and drops the rest. It clears closures and the cached function
table (they reference old code and are recaptured on the next run), empties
every static value (called directly it follows nothing: `reload_program` is
what keeps them) and invalidates cached bytecode. Because the id is a name hash, reordering
declarations preserves state; renaming a variable, or moving it between
functions or modules, drops it. See
[../features/program-modification.md](../features/program-modification.md#state-preserving-hot-reload-transfer_state)
for the call-structure caveats.

## Immutability and the in-place gate

Petal values are immutable **by construction**, not by runtime check:

- Ordinary bindings are not mutable variables. Reassignment emits a new term
  and rebinds the name.
- Collections are value types; `append`/`set_at`/`remove` return new
  collections (the `@` rebind operator is sugar: `append(@nums, 4)`).
- The one mutable slot is a `var`, and it is contained: no expression ever
  evaluates to a cell, and the cell slab lives in the heap, so `Heap::fork`
  isolates it like any other object. A `set` replaces the cell's contents; it
  does not edit a value in place.
- The one exception is an optimization. When the VM's escape analysis proves a
  container uniquely owned and non-escaping, it sets `PetalCxt::in_place` and
  builtins mutate the backing store directly. With `--no-opt` the flag is
  never set.

`Heap::fork`, speculative runs, cheap state snapshots, and the
"re-run the whole program every frame" model all depend on this.

## Embedder pitfalls

### Declaring native effects honestly

The frame gate ([frame-gate.md](../dev/frame-gate.md)) skips a whole run whose
inputs did not move; memoized scopes ([memo-scopes.md](../dev/memo-scopes.md))
skip a single user-function call whose arguments and reads did not move, and
**replay** it instead. Both decide from the native's `NativeEffects` row and
nothing else, so the row has to describe what the native does *to the host*,
not what it looks like from the script.

What a replay does, exactly: it appends the output-buffer values the call
pushed last time, re-applies its `state` and `var` writes, re-marks the state
keys it touched, and returns the cached result. It does **not** call any
native. So:

- **`emits` is only honest for a push into an output buffer** —
  `PetalCxt::push_output` / `emit`, drained by the host after the run. That is
  the one kind of output a replay reproduces. A native that writes to a
  host-side list, a scene graph, a physics world, a mixer or a counter is
  *not* an emitter, however much it feels like one: that write is an
  `effect`.
- **A host-data read is only honest if the host bumps the revision.** A
  `HOST_DATA` read is recorded as "valid while the host-data revision is
  unchanged", and the revision moves only when the host calls
  `Env::note_host_data_changed(stack)`. A native answering from state that
  changes every step (a physics query, a clock the host keeps itself) either
  gets that call every time the state moves, or declares the read as a
  `probe` of an input class (when re-calling it with the same arguments at
  validation is harmless and cheap), or declares an `effect`.
- **Top-level code hides the bug.** The root frame is not a scope and always
  runs, and a call too small to be worth a record is folded into its caller.
  An under-declared native works perfectly until someone moves the call into
  a helper function — the ordinary refactor — and then silently stops
  reaching the host from the second frame on.

| The native… | Row |
|---|---|
| computes from its arguments only | `PURE` |
| pushes into an output buffer the host drains after the run | `EMITS` |
| writes host state directly (declares a body, queues a sound, spawns, draws into a host-owned list) | `EFFECT`, and call `note_effect()` in it |
| reads host state that the host reports changes of with `note_host_data_changed` | `reads(HOST_DATA)`, and call `note_host_read()` in it |
| reads host state that moves without such a report (a live simulation) | `EFFECT` (plus the read), or bump the revision whenever it moves |
| reads a bound input and answers a pure function of it | `probe(class)` |

**The cheesecake case.** Cheesecake, a C++ engine scripted in Petal through a
C bridge, registered its scene natives (`body`, `draw_mesh`, `point_light`,
`camera`, …) as `Emits` and its queries (`raycast`, `contacts`,
`body_state`, `world_to_screen`) as plain `ReadsHostData`. The scene natives
did not push into a Petal buffer at all: they wrote the engine's per-frame
declaration lists. The script

```petal
import vec2
fn crate(id, at)
  body(id, {size: 1.0, pos: at})
end
fn height_of(id)
  let hit = raycast(vec3(0.0, 20.0, 0.0), vec3(0.0, -1.0, 0.0), 50.0, {static: false})
  if hit != nil then hit.point.y else -1.0 end
end
for i in range(0, 3) do
  crate("crate{i}", vec3(float(i) * 3.0, 6.0, 0.0))
end
```

declared its crates on the first frame only: from the second frame on each
`crate(...)` call had unchanged arguments and a record with no reads to
invalidate it, so it was replayed and `body()` never ran — and the engine reaped the crates as undeclared. A
helper in an imported module lost its meshes and lights the same way.
`height_of` recorded a host-data read, but the engine never called
`note_host_data_changed` as the physics stepped, so the record stayed valid
and the helper returned the first frame's hit forever. The fix (cheesecake
`4753d07`) registered all of them as `Effect` (`Effect | ReadsHostData` for
the queries), which makes any scope calling them unrecordable, and added a
regression test that calls each native from a helper with the same arguments
for 30 frames.

The other honest fix, where it fits, is to make the native a real emitter:
validate the arguments, push the request into an output buffer, and apply it
host-side after the run. Helpers calling it then stay memoizable, because a
replay re-emits the request. petal-desktop-sdl's `sfx`/`synth` natives work
this way (`integrations/petal-desktop-sdl/src/sound.rs`, whose tests pin both
the replay and the trap).

**A bridge makes the declared facets self-fulfilling.** A C/C++ bridge that
forwards registration flags into the row usually also calls `note_effect()` /
`note_host_read()` on the native's behalf when the flags say so
([petal-c-bridge](embedding-c.md)'s `HostCallback::call` does). The row and the activity counters then agree by construction, and
neither the memo nor the audit can see what the C++ callback did behind
them. Only the embedder knows what a callback touches; a bridge's emitter flag
in particular should mean "this call's output goes through the bridge's
buffer-emitter path", never "this callback writes something somewhere".
petal-c-bridge follows this: only `pb_vm_register_emitter` pushes into a
buffer, a C callback has no way to, and so `PB_FX_EMITS` on a callback is
always an over-declaration (use `PB_FX_EFFECT`).

**The frame gate has the same shape at frame granularity.** On a skipped frame
no native runs at all. A host whose natives must reach the host every frame
(declarations reaped when not re-declared, audio pumped from `end_frame`)
either keeps the last frame's declarations in force on a skipped frame, as
Garden's key claims do, or answers "don't gate" for as long as it needs
frames — petal-desktop-sdl's `Host::frame_gating` does this while a sound is
playing.

### Constant natives

`PURE` and `CONST` answer different questions, and the difference is the
cost of being wrong.

- **`PURE`** means "reads no *tracked* input, emits nothing, has no effect".
  It is what the memo needs: a scope that calls only such natives can be
  replayed while its arguments and reads are unchanged. A wrong `PURE` costs
  a stale replay until the next invalidation.
- **`CONST`** means "a constant function": same arguments, same result, for
  as long as the program lives, whatever the host is doing. No time, input,
  viewport, random stream, theme, font provider, host or game state, handle
  liveness, or anything about the program's source — whether or not an
  `InputClasses` bit exists for it. A call with constant arguments is then
  itself a constant, so a table built from such calls is a
  [static value](../features/static-values.md): evaluated once and kept on
  the stack until an edit reaches it. **A wrong `CONST` is a stale value
  until an edit to the declaration or to something it depends on**: a reload
  keeps the static values the edit cannot have changed, so reloading does
  not flush it. There is no revision to bump and no frame on which it is
  recomputed.

The calls a host has for the values a stack keeps:

- `Env::clear_static_values(stack)` drops what a stack holds, for a host
  that knows it changed something such a native answers from.
- `Env::static_values(stack)` lists the values and names any that could not
  be kept. A value holding a function, a `var` cell, a handle or a pending
  result is rebuilt every run instead
  ([Values that cannot be kept](../features/static-values.md#values-that-cannot-be-kept)).
- `Env::take_static_warnings(stack)` (or `take_all_static_warnings()`) hands
  out one line per value that could not be kept, each once per load and once
  more after a reload. Call it after a run and print what it returns, as
  `petal run` and the SDL host do. The cache prints nothing by itself.
- `Env::set_static_cache(false)` turns the cache off.

So `CONST` is opt-in and the default is dynamic:

```rust
// A lookup into a table compiled into the host: constant.
env.register_native("tile_code", native_tile_code, NativeEffects::CONST);
// Pure for the memo, but the host's fonts decide the answer: not constant.
env.register_native("font", native_font, NativeEffects::PURE);
```

To ask: `NativeEffects::is_const()` on a row, or, for a registered native,
`Env::native_constness(id)` / `native_constness_of_name(name)` (the same on
`NativeFnTable`), which answer a `NativeConstness`: `Const`, `Dynamic`, or
`ConstIfCallbacksAre` for the higher-order builtins (`map`, `filter`,
`reduce`, `sort`, `sort_by`), which are constant exactly when the functions
they are handed are. A name nothing registered — a `native fn` compiled
without its host, as under `petal check` and the LSP — is `Dynamic`, so those
tools may call less static than a host does and never more. A row that says
`CONST` and also reads, probes, emits or has an effect contradicts itself and
is treated as dynamic.

Declare `CONST` only for a native whose body you have read, and check each
of these:

| The native… | `CONST`? |
|---|---|
| computes from its arguments only (arithmetic, string and list work, a record built from the arguments) | yes |
| looks up a table compiled into the host that never changes while it runs | yes |
| raises on bad arguments and otherwise the above | yes — the same arguments raise the same way every time |
| answers from anything the host loads, configures or can swap at run time (fonts, themes, assets, settings, a palette) | no |
| answers from a seed, a clock, a counter, or anything a script or the host can set | no (`noise` reads the noise seed) |
| takes or returns a handle, or asks about one (`is_valid`) | no |
| reports where something is in the source (`fn_info`, `fn_ast`) | no |
| stands in for a native that is dynamic on another host (a stub) | no |
| you are not sure | no |

In the core table every arithmetic, string, collection, colour, vector and
formatting builtin is `CONST`, along with `assert`/`assert_eq`, `symbol` and
the `Rect` class; `noise`, `is_valid`, `fn_info`, `fn_ast`, the random
builtins and everything that reads, emits or has an effect are not. The
exact list is pinned by a test (`const_builtins_are_exactly` in
`core/src/builtins/mod.rs`); petal-ui's only constant native is
`ui_version`.

From C the flag is `PB_FX_CONST` (`petal::fx::Const` in the C++ wrapper).
`PB_FX_PURE` is `0` and stays dynamic, so a host written before the flag
existed has no constant natives until it opts in.

**Check a `CONST` row with the static audit.** `petal run --static-audit`
and `petal-ui-run --static-audit` watch every dynamic input while a static
top-level term is being evaluated and name the native that touched one
(`core/src/static_audit.rs`; see
[../CLI.md](../CLI.md#checking-the-answer---static-audit)). An embedder runs it
over its own natives:

```rust
use petal::static_audit::StaticAuditMode;
env.set_static_audit(StaticAuditMode::Watch);   // between runs; re-lowers
// … run the frames you want audited, with memoization off …
let report = env.static_audit_report();          // Display: one line per finding
assert!(!report.has_failures(), "{report}");
```

It sees what moves a counter: a native that answers from host state without
calling `note_host_read()` or reading a binding is as invisible here as it is
to the effect audit, which is why the table above is still the rule.

### Catching it with the effect audit

The audits, the static scan and the out-of-repo oracle are described for
contributors in [dev/native-effects.md](../dev/native-effects.md).

The effect audit brackets every native call with activity snapshots and holds
what each native was observed doing against its row
(`core/src/effect_audit.rs`). `petal run --effect-audit` and
`petal-ui-run --effect-audit` cover the builtins and the `petal-ui` natives;
an embedder audits its own natives by turning it on in the host:

```rust
env.set_effect_audit(true);                  // clears what was observed so far
for _ in 0..frames { /* bind inputs, reset_stack, run */ }
eprint!("{}", env.effect_audit_report());    // Display: one line per finding
assert!(!env.effect_audit_report().has_under_declared());
```

Read the report knowing what the audit can see: only what goes through the
`PetalCxt` — output pushes, `note_host_read`, `note_effect` (and `print`, the
counters, the resource table, which note it themselves), binding reads. A
C++ or Rust callback that writes host state and says nothing looks *silent*,
so it is never reported as under-declared. What does give it away is the
other direction:

- **`over-declared … emit`** — a native declared `EMITS` that never pushed
  output. That is exactly the cheesecake bug's signature, and usually means
  the row should be `EFFECT`.
- **`over-declared … effect`** — a native declared `EFFECT` that never called
  `note_effect()`. If the native does write host state the row is right;
  add the `note_effect()` call so the report is clean and the next reader of
  the native sees the effect where it happens.
- **A call count far below the frame count.** The audit counts calls that
  reached the native; replayed calls do not. A native called from a helper
  that ran for 60 frames but shows `1 calls` is being replayed.

An audit of the cheesecake shape (a helper calling a host-queueing native
declared `EMITS`, four frames):

```text
effect audit: 1 natives called, 0 under-declared, 1 over-declared
  over-declared queue_on_host                      1 calls  emit
```

To confirm a suspicion, run the same frames with memoization off
(`env.set_policy(RunPolicy::FAST.with_memo(false))`, or `--no-memo` on
`petal-ui-run`): a native whose host-side behavior
changes with the policy is declared wrong. A regression test of the form
"call every native from a helper with unchanged arguments for N frames and
check the host saw every call" catches the next one.

## Existing embedders

Each of these is the place to read one part of the API in use.
[building-apps.md](building-apps.md) describes how they layer, and the
top-level [README](../../README.md#building-a-full-app-with-petal) lists every
integration and app.

- **petal-ui** (`core-libs/petal-ui/`) — the reusable layer: the input vocabulary
  (`InputEvent`, `InputState`), the `DrawCommand` enum (with a
  `Host { tag, data }` pass-through so embedder-specific natives keep their
  place in the command stream), the `ui` interface package (scripts write
  `import ui`), and a `Headless` harness that mirrors the frame contract
  for tests.
- **petal-sdl** (`integrations/petal-desktop-sdl/`) — the reference native
  embedder. Per frame: translate SDL events into `petal_ui::InputState`, bind
  the input snapshot and frame info, ask `run_needed`, `reset_stack` + `run`,
  drain `draw_commands`, rasterize. Hot reload is an OS file watcher over
  `program_source_paths` that calls `reload_program` (`src/watcher.rs`). Its
  JSON protocol (pause/step/state/screenshot over stdin/stdout) drives the
  same contract headlessly for agents and tests. Its `Host` trait is the seam
  the `petal-fps` and `petal-fantasy-nes` example hosts build on.
- **petal-c-bridge** (`integrations/petal-c-bridge/`) — the C ABI and C++
  wrapper ([embedding-c.md](embedding-c.md)). One `Env` + program + stack per
  `pb_vm`; every C callback and emitter is a `register_native_boxed` closure
  owning its userdata; hot reload via `watch_program_sources` +
  `reload_program`; values cross as decoded `pb_value` trees rather than
  JSON.
- **petal-web-canvas** (`integrations/petal-web-canvas/rust/`) and
  **petal-web-html** (`core/src/wasm.rs`, built with the `wasm` feature) —
  wasm-bindgen `PetalRuntime` structs owning an `Env`, with the same channels
  marshalled as JSON strings across the wasm boundary. petal-web-canvas
  returns petal-ui draw commands and exposes `run_speculative` for isolated
  what-if frames (the `diagram-canvas` example app uses it); petal-web-html
  returns a retained element tree instead of draw commands.
- **Garden** (`garden/garden-script/`) — a config host and a panel host over
  petal-ui: output buffers read back as layout and theme
  ([embedding-guide.md](embedding-guide.md#observing-function-calls)), a
  system font source, emit tracing for its editor, and goal-based writes back
  to the user's script.

## Current limitations

1. **Captured native state is invisible to the runtime.** A native can be a
   closure (`register_native_boxed`), but the effect audit and the memo see
   only what it does through `PetalCxt`; its row has to declare what the
   captures read and whether it mutates them.
2. **A native must be registered before the load that refers to it.** A
   program already loaded reaches a native registered later only after a
   reload that recompiles. Handle classes can be added at any time.
3. **Natives cannot call Petal closures.** Host-driven callbacks must be
   inverted into data (command buffers) or go through `Env::call_function`
   between runs.
4. **Everything but handles is by value.** Fine for commands and snapshots;
   large or intrinsically mutable host objects belong behind a handle.
