# Proposal: effect annotations on `native fn`

Status: proposed, 2026-11-08

## Summary

Let a `native fn` declaration in an interface package say what kind of
native it is, with a trailing clause in the vocabulary the effect rows
already use:

```petal ignore
pub native fn raycast(from, dir, max_distance?, opts?) query(PHYSICS)
pub native fn apply_impulse(id, impulse, at?) command
pub native fn mesh_exists(name: string) -> bool reads(HOST_DATA)
```

The annotation is a **claim the host's registration is checked against,
never a replacement for it**. At run time the registered row stays the only
row. The declared row is used in two places: by static tools when the
native is not registered (`petal lint --rules-include never-replays` and
`dep_classes` in the stock CLI, the LSP), and by `verify_interface`, which
starts reporting a registered row that disagrees with its declaration. A
declaration with no clause means "unknown", exactly as today, so no existing
declaration starts lying.

**Recommendation: adopt a smaller version.** Ship the clause for the replay
semantics only (the kind, the read classes, `ticks`), not the whole row:
`contained`, `no_feedback`, `const` and the `Pending` policy stay
host-only. Build it in two steps so the language part can be dropped
without losing the payoff: first a host-exported row table that `petal
lint` can load (tooling, no syntax), then the clause, which reads that
table's vocabulary from source. If the first step turns out to be enough for
Cheesecake (the only host that has asked), defer the syntax.

## Motivation

Cheesecake's `docs/tasks/immediate-mode-upstreaming.md`, step 4: `petal lint
--rules-include never-replays` cannot lint a game. The rule
(`core/src/lint/never_replays.rs`) judges each function by the rows of the
natives it calls, and its own header says why it is blind to a host: "a
native of a host that is not here is unknown and claims nothing". In
`core/src/static_classify/dep_classes.rs:743-747` an unregistered native
resolves to `Deps::UNKNOWN`, which no finding trusts. So the stock CLI has
nothing to say about a script whose natives are declared but not
registered. Checked: `LintSnippet` on a script that declares
`native fn raycast` / `native fn apply_impulse` and calls them from helpers,
plus a helper that prints, returns no findings and no warnings (the bundled
rule set does not run `never-replays`, and with it on, only the `print`
helper could be judged). Cheesecake works around it with its own
`--replay-report` (`engine/app/src/headless.cpp:196`), which calls
`pb_vm_dep_classes_report` from inside the engine after registering its
natives.

How much this covers, counted (top-level `native fn` lines, build dirs and
vendored copies excluded):

| Where | Declarations | Rows known to the stock CLI? |
|---|---|---|
| `core/packages/**` | 193 | yes, registered in Rust |
| `core-libs/petal-ui/prelude/**` | 106 | yes (the CLI registers petal-ui) |
| `~/cheesecake/engine/script/packages/cheesecake/*.ptl` | 163 | no |
| `~/petal-creative/**` | 227 | no |
| `garden/garden-script/packages/**` | 33 | no (only `garden-panel-run`) |
| `~/worlds-fair/ui/ptl/**` | 7 | no |
| `integrations/petal-desktop-sdl/packages/sdl/sdl.ptl` | 9 | no |

So the change matters for about 440 declarations in five host packages and
for none of the 300 the CLI already knows.

The second motivation is that the row is subtle and lives far from where a
script author reads. Cheesecake's registrations
(`engine/script/src/*.cpp`, grouped by the constant passed) are: 42
`kHostEffect`, 28 `kHostQuery`, 19 `fx::Effect`, 12 `kPhysicsQuery`, 6
`kAssetQuery`, 6 `fx::Pure`, 6 `fx::Command`, 5 `kInputQuery`, and a tail.
Its interface package says none of this: `physics.ptl:7`, `pub native fn
body(id, opts)`, says in a comment that it "declares the body", but it is
registered as `kPhysicsCommand` (`natives_physics.cpp:367`), a command that
is re-issued on replay, not a declaration. A writer cannot learn from the
package that `body` re-runs on every replay, or that 28 `kHostQuery`
natives, which only read, are flagged `Effect` and so make every helper that
calls them unreplayable (`host_impl.hpp:60-63`).

## Design

### Syntax

After the return type and before a `= "host_name"` binding, a declaration
may carry one **kind** and any number of **qualifiers**, separated by
spaces:

```
native fn NAME(PARAMS) [-> TYPE] [KIND] [QUALIFIER...] [= "host_name"]

KIND      := pure | emit | declare | command | effect
           | query(CLASS, ...)        -- a revisioned read: PHYSICS, NET, CAMERA
           | reads(CLASS, ...)        -- any other read: HOST_DATA, POINTER, ...
           | probe(CLASS, ...)        -- a read that is re-evaluated at validation
QUALIFIER := ticks | keyed
CLASS     := POINTER | KEYBOARD | INPUT | CLOCK | VIEWPORT | HOST_DATA
           | RESOURCES | RNG | BINDINGS | PHYSICS | NET | CAMERA
```

The words are contextual, as `native` is: only in this position. They are
the names `NativeEffects` already uses (`docs/dev/native-effects.md`, "The
four kinds of engine native"), and the class names are the ones
`pb_vm_dep_classes_report` prints ("class PHYSICS"). `pure` is spelled out
because an omitted clause is not pure: it is unknown.

Edge cases:

- **No clause**: unknown. Static tools treat the native as they do now
  (`Deps::UNKNOWN`), `verify_interface` checks nothing about its row.
- **Several kinds** (`command effect`): a parse error, "a native has one
  kind; `command` and `effect` are both given". A read class combines with
  `emit`/`declare` (`declare reads(HOST_DATA)`, a checked emitter whose
  check reads the asset table) by allowing `reads(...)` / `probe(...)`
  after another kind; `query(...)` is itself a kind.
- **`command` with a non-nil return type** (`-> int command`): a warning
  from the type checker, "a command returns nil". At run time a command
  that returns non-nil already makes its scope effectful
  (`native-effects.md`, command row), so the warning is the static half of
  an existing rule. Without a declared return type the checker infers `nil`
  for the call.
- **An unknown word** in the clause: an error naming the vocabulary.

### Semantics

1. **Run time: nothing changes.** The registered row is what the memo, the
   frame gate and DM use, as now. A declared row is never installed, so a
   wrong annotation cannot make a scope replay stale.
2. **Static tools, native not registered** (stock CLI, LSP, check-only
   VMs): the declared row stands in for the missing registration in
   `dep_classes`, so `never-replays` and the dependency report work on a
   host's scripts. Findings computed this way say so ("by its declaration").
3. **`verify_interface`** (Rust `Env::verify_interface`, C
   `pb_vm_verify_interface`, which hosts already run as a drift test) gains
   a third list, `mismatched`: natives whose registered row does not
   project onto the declared one. Projection compares only what the clause
   can say: the kind, the read classes, `ticks`, `keyed` (on an emitter).
   A declaration with no clause matches anything.
4. **Mismatch is a report, not a failure to load.** The host decides
   whether its test fails. Loading a script never depends on the clause.

### Error messages (proposed)

```
physics.ptl:12: `apply_impulse` is declared `command` but registered as `effect`
  (the host's row: EFFECT | READS(HOST_DATA)); a replay will not re-issue it
assets.ptl:20: `mesh_exists` is declared `reads(HOST_DATA)` but registered
  `effect reads(HOST_DATA)`: every helper that calls it is never replayed
game.ptl:31: `aim` is rarely replayed: it calls `raycast` on every call,
  declared `query(PHYSICS)` (by its declaration; the host's row was not seen)
```

## Before and after

All "before" lines are quoted from the files named. "After" is the proposed
syntax and has not been run; no prototype was built.

**1. A query (the common case).** `~/cheesecake/engine/script/packages/cheesecake/physics.ptl:23`,
registered `kPhysicsQuery` = `Probe | ReadsPhysics` (`natives_physics.cpp:608`):

```petal ignore
# before
pub native fn raycast(from, dir, max_distance?, opts?)
# after (proposed)
pub native fn raycast(from, dir, max_distance?, opts?) probe(PHYSICS)
```

The reader learns the call is cheap to keep in a helper while the world
holds still; the stock lint can now report "rarely replayed" for a helper
that raycasts on every call. One more word per line, no new name.

**2. The case the change is for: a comment that says the opposite of the
row.** `physics.ptl:5-7`, registered `kPhysicsCommand` (`natives_physics.cpp:367`):

```petal ignore
# before
# body(id, opts): declares the body; returns nothing.
# Declarative: keep calling it every frame for as long as the body exists.
pub native fn body(id, opts)
# after (proposed)
# body(id, opts): the body, re-issued with these options by every replay.
pub native fn body(id, opts) command
```

Mistake no longer possible: the package and the engine disagreeing about
`body` without a test noticing (`verify_interface` reports it). Mistake
newly possible: none at run time; a stale clause is a report.

**3. A row the clause cannot say (the old form is still the honest one).**
`kHostQuery` = `Effect | ReadsHostData | Contained` (`host_impl.hpp:63`),
28 natives. The clause can write `effect reads(HOST_DATA)` but not
`contained`, which is a DM concern:

```petal ignore
# before
pub native fn editing() -> bool
# after (proposed)
pub native fn editing() -> bool effect reads(HOST_DATA)
```

The annotation is true but reads as alarming for a function that only
reads; the real fix is the host's row (these are `kAssetQuery`-shaped
reads), and the annotation's value here is that writing it down makes the
wrong row visible. Leaving such declarations unannotated is equally valid.

**4. A core builtin (no gain).** `core/packages/core/src/math.ptl:9`:

```petal ignore
pub native fn sqrt(x: num)
```

Its row is registered in Rust and the stock CLI has it. Annotating the 300
core and petal-ui declarations would duplicate the Rust rows, so they stay
as they are; `verify_interface` would check them if they were annotated,
which is a possible later drift test, not part of this proposal.

**5. A command with a return value (the type-checker interaction).**
`~/cheesecake/engine/script/packages/cheesecake/audio.ptl:4`, registered
`fx::Command` (`natives_audio.cpp:315`):

```petal ignore
# before
pub native fn sfx(name: string, opts?)
# after (proposed)
pub native fn sfx(name: string, opts?) command
# and, if someone writes this, a warning: "a command returns nil"
pub native fn sfx(name: string, opts?) -> int command
```

**6. Hot reload.** Editing a clause in an interface package while a game
runs recompiles the package as any edit does. Because the runtime never
reads the declared row, the reload changes no replay decision, no `state`
slot and no call path; only lint output and the next `verify_interface`
report change. A reloaded program still equals a fresh compile.

**Migration.** Existing declarations are unchanged and keep meaning
"unknown". Annotating a host package is mechanical given its registrations:
a host-side `pb_vm_natives_json` (step 1 of the implementation) could print
the clause for each native, and a small script can splice it in.

## Assessment

### Project goals

Findings: it serves introspection and debuggability directly (the replay
behaviour of a call is readable at the declaration and checkable without the
host) and live coding indirectly (fewer helpers that silently never replay).
It does not touch the dataflow model or `state`. Effects on natives are
something Petal is already distinctive at (rows, kinds, memo); writing them
in source is the natural place for them to surface.
Opinion: worthwhile, but its whole value rests on hosts keeping the clauses
true, which is why the design makes them checked claims.

### Ease of writing

Findings: a script author never writes a clause; only a host author does,
in an interface package. The vocabulary is the one in `petal_bridge.h` and
`native-effects.md`, lower-cased. Someone from TypeScript would read a
trailing word like a `.d.ts` modifier; from Koka, like an effect row.
There are now two places a row is written (the registration and the
declaration), which is the main cost, and the guide must say which wins
(the registration, always).
Opinion: acceptable because no one is required to annotate.

### Bug-freedom

Findings: it prevents drift between what a package says a native does and
what the engine registers (example 2 is a live instance), and lets the
never-replays rule run in CI without a game binary. It cannot introduce a
stale replay, because the declared row never reaches the runtime. The risk
it adds is false confidence: a lint run with declared rows is only as good
as the clauses, and a clause the host never verifies can drift. The
mitigation is the `verify_interface` report, which Cheesecake already runs
(`script_host_tests cheesecake` checks every native has a declaration,
per the task doc).
The differential oracle, golden corpus and Kani proofs are unaffected: no
runtime path changes.

### Performance

Findings: parse cost is a few tokens per declaration. Run time: zero, by
design (nothing reads the declared row at run time). Static analysis reads
the declared row where it now reads `Deps::UNKNOWN`, the same cost as a
registered row. No benchmark is needed; `petal bench` would show no change
because nothing on the run path changes.

### Live coding and hot reload

See example 6: no effect on running programs; state and call paths are
untouched.

### Introspection, debugging and program manipulation

Findings: provenance, `explain`, DM and `propose-edit` read registered rows
or none; they are unaffected. The LSP's hover on a native call could show
the declared kind, which is new introspection. `dep_classes_report` gains
"by its declaration" findings. Source preservation and the formatter must
keep the clause (CST: a new `NativeEffects` node inside `NativeFnDecl`).

### Maintainability of the implementation

Stages: the parser (`parse_native_fn_decl`, `core/src/parse.rs:1272`, and
its CST twin), the AST (`NativeFnDecl`), `native_decl.rs` (a declared row
per form), `dep_classes` (fall back to the declared row for an
unregistered native), `verify_interface` (the `mismatched` list), the type
checker (the `command` warning), the formatter, LSP hover, the C bridge's
`pb_vm_verify_interface` report text, docs (`ffi.md` "Native interfaces",
`native-effects.md`, `embedding-c.md`). Roughly: parser 100 lines,
analysis 80, interface check 80, tests 300. It adds one invariant future
work must keep: the declared row never reaches the runtime.

### Domains

- Games: helps most (Cheesecake, 163 declarations; physics queries and
  commands are exactly the rows the lint cares about).
- Embedding hosts (Rust, C/C++): helps; the drift report is the
  practical gain for any host with an interface package.
- Editor scripting (Garden, 33 declarations): mildly helpful for
  linting panels outside Garden.
- Creative tools (petal-creative, 227 declarations): would help if its
  natives have interesting rows; not surveyed.
- UI apps, dashboards, web, graphics in the stock hosts: neutral; their
  natives are registered in the CLI already.

### Compatibility and migration

No existing code breaks: a clause is optional and its absence keeps today's
meaning. Nothing needs a warning period. It does not close off a future
direction; if Petal later gets checked effect types on user functions, the
same words could be reused there, which argues for picking them carefully
now. It depends on no unfinished work.

### Prior art

- Koka and Eff put effect rows in the type of an `extern`; the compiler
  trusts them, and a wrong row is unsound. This proposal trusts nothing at
  run time, which is weaker but safe.
- Haskell's FFI encodes purity in the imported type (`IO` or not); a pure
  import of an impure C function is a classic source of wrong results.
- GCC `__attribute__((pure))` / `const` are unchecked claims that let the
  optimizer drop calls; the same failure mode as a wrong `PB_FX_CONST`.
- Jetpack Compose's `@Stable` / `@Immutable` are unchecked stability claims
  that skip recomposition; wrong ones cause stale UI, which is Petal's
  stale-replay risk exactly. Compose later added compiler reports to check
  them, the role `verify_interface` plays here.
- TypeScript `.d.ts` files drift from the implementation they describe;
  the usual answer is generating them, which is the tooling alternative
  below.

### Alternatives

1. **Do nothing.** Hosts keep their own report (`--replay-report`). Cost:
   each host rebuilds the same tool, and CI needs the engine binary.
2. **A row table the host exports** (`pb_vm_natives_json` /
   `Env::natives_json`, then `petal lint --natives rows.json`). No syntax,
   always true (it is generated from the registrations), and it gives the
   stock CLI everything the clause would. It does not put the kind next
   to the declaration for a reader, and it is one more file to keep fresh.
   This is the smallest version that delivers the lint, and it is step 1 of
   the implementation either way.
3. **Doc-comment tags** (`// @command`): no grammar change, but comments
   already lie in this corpus (example 2) and nothing would check them
   unless the parser learns them, at which point they are syntax.
4. **Generate the interface package from the registrations.** Cheesecake's
   packages are hand-written with documentation; generation would fight
   that.
5. **The full row in syntax** (`contained`, `no_feedback`, `const`,
   pending policies). Rejected for now: those bits matter to DM and the
   static cache, not to replay linting, and a longer clause invites copying
   rows without understanding them.

## Migration and compatibility

Unchanged code keeps working. A host opts in by annotating its package and
running `verify_interface` in its tests; the mismatch list is empty until
it annotates something. For Cheesecake: annotate
`engine/script/packages/cheesecake/*.ptl`, extend the `script_host_tests
cheesecake` interface check to compare rows, run `never-replays` from the
`petal_check_<game>` ctests, and keep `--replay-report` until the two agree
over the census (`/Volumes/bureau/retained-opt/a7/`), as the task doc says.

## Implementation sketch

1. **Row export (tooling, no language change).** `Env::natives_json()` /
   `pb_vm_natives_json`: each registered native's row in the clause
   vocabulary. `petal lint --natives FILE` loads it as rows for unregistered
   natives in `dep_classes`. Test: the lint over a script with a table equals
   `pb_vm_dep_classes_report` over the same script in a host that registers
   the rows.
2. **The clause.** Lexer: none (contextual identifiers). Parser and CST:
   the clause after the return type. `native_decl.rs`: a `DeclaredRow` per
   form. `dep_classes`: fall back to it when there is no registration.
   Tests: parse errors, the fallback, a declared row never reaching
   `native_effects` at run time (assert on a registered native whose
   declaration disagrees).
3. **The interface check.** `InterfaceMismatch::mismatched`, projection
   rules, the C report text; a bridge test.
4. **The type-checker warning** for `command` with a non-nil return type.
5. Docs and the LSP hover.

## Open questions

- **Is step 1 enough?** If Cheesecake's CI only needs the lint, the row
  table may satisfy it, and the syntax can wait for a second host to want
  the declarations to be readable. Owner decision, after step 1 ships.
- **The exact words.** `probe(PHYSICS)` vs `query(PHYSICS)`: Cheesecake's
  physics reads are registered `Probe | ReadsPhysics`, while
  `native-effects.md` calls the kind `query` and `fx::Query` is
  `ReadsPhysics` alone. The clause should probably expose one word for
  "revisioned read" and a `probe` qualifier, but the row model has both
  and the choice should match whatever the docs settle on.
- **Placement.** After the return type (proposed) or behind a sigil
  (`-> list ! query(PHYSICS)`). A bare word after a type needs the type
  grammar never to take a following identifier; that holds today
  (`parse_return_type`) but would constrain a future postfix type syntax.
- **Mismatch severity.** Report (proposed) or fail `verify_interface`
  outright. A host can turn the report into a failure; the reverse is
  harder.
- **Should core and petal-ui declarations be annotated** so the clause is
  checked against the Rust rows in Petal's own tests? Useful as a drift
  test, but 300 lines of duplication.
