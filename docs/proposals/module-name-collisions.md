# Proposal: name collisions across modules

Status: proposed, 2026-11-09. Parts A and B1 accepted and shipped
2026-11-09 (see [Implementation status](#implementation-status)); C is
proposed, and B2 and B3 are not adopted.

## Summary

The request has two parts: compiler warnings for ambiguous names, and
module-namespaced references (`import ui`, then `ui.Something`). Most of both
already exists. Namespaced references work for functions, values,
constructors, enum variants and `match` patterns. Ambiguous bare uses are
already a compile error. What is missing is narrower than the request, and
has a different cause:

1. **Class and enum names are unique across the whole program**, private ones
   included, because a value's runtime identity is that bare text (`"Phase.play"`,
   `type(r) == "Rect"`). Two modules that never import each other cannot both
   declare a private `enum Phase`. This is the collision reported in
   [fn-ast-enums.md](fn-ast-enums.md), and no import syntax can fix it.
2. **A type annotation cannot be qualified.** `sh.Circle(3)` works, but
   `fn f(c: sh.Circle)` is a parse error.
3. **A user class named like a built-in class replaces it silently**
   (`class Rect`).

Recommendations:

- **A. Qualified type names in annotations: adopt.** Small. Needed by C.
- **B. Ambiguity warnings: adopt one, reject the broad one.**
  - Adopt a compile warning for a class or enum that takes a built-in class's
    name.
  - Reject a default-on "a local declaration hides an import" warning. It
    would fire on 325 declarations in 107 files, and the sampled ones are a
    deliberate convention.
  - Offer qualifier shadowing as an opt-in lint at most.
- **C. Module-scoped class and enum identity: adopt, scheduled before the
  first bundled package exports a type.** It is the only real fix for (1).
  It is also the expensive part, and nothing in today's corpus collides. A
  "prefix your exported type names" convention is the defensible
  alternative until a library needs to export one.

## What works today

Checked with scratch modules under the session scratchpad and the CLI built
from `main`:

| Form | Works? |
|---|---|
| `import ./hud` then `draw()` and `hud.draw()` | yes ([module-system.md](../module-system.md#importing)) |
| `import ./shapes as sh` then `sh.Circle(3)`, `sh.Kind.round` | yes |
| `match k when shapes.Kind.round -> …` | yes |
| `import ./shapes: Kind` then `Kind.square` | yes; `shapes.Circle` is then "undefined variable `shapes`", as documented |
| `fn f(c: sh.Circle)` | **no**: "Expected ',' between parameters" |
| Two plain imports both exporting `draw`, used bare | compile error naming both, at the use ([Which name wins](../module-system.md#which-name-wins)) |
| Two plain imports offering different arities | merged, with a warning per call naming the module it reaches |
| A plain import hiding a `core`/`std` builtin | warning at the first bare use |
| A local `fn` over a bundled package's export (`fn card` with `import ui`) | lint `shadows-builtin` |
| A local `fn`/`let` over a user module's export | silent (tier 1 wins) |
| A local `let`/parameter named like a module qualifier | silent; `hud.draw()` then reads the local ("No method 'draw' on type record") |
| `class Rect` in a user file | silent; replaces the built-in `Rect` |
| `enum Phase` in `a.ptl` and `b.ptl`, both private, both imported by `m.ptl` | **error**: "enum `Phase` is already declared in `a.ptl`, so `b.ptl` may not declare it too" |
| The same with `class Box` | **error**, same message |
| A file declaring `class Circle` and importing `./shapes` (which exports one) | **error** |

The uniqueness rule is `ClassTable::name_taken` (`core/src/classes.rs:283-305`).
It keys classes and enums by bare name for the whole compilation. A built-in
class is exempt, which is why `class Rect` shadows instead of failing.

The cause is runtime identity:
- A variant's tag is `format!("{enum_name}.{variant}")`
  (`core/src/program.rs:822`).
- A class instance carries its class name.
- `type(v)` reports both as the bare name.
- Hosts build variants from tag text (`pb_builder_begin_enum(b, "Shape.circle")`,
  `integrations/petal-c-bridge/include/petal_bridge.h:328`).

## Motivation: the corpus

The corpus is 880 `.ptl` files: this repo's `examples/`, `garden/`,
`core-libs/`, `core-runtime/` and `test/`, plus `~/cheesecake/games`,
`~/petal-creative` and `~/worlds-fair/ui/ptl`.
- 681 files import something, and 382 import a relative module.

**Classes and enums are rare in modules.**
- There are 56 `class`/`enum` declarations, 7 of them `pub`.
- Only one imported module declares any: `~/cheesecake/games/city_broker/config.ptl`
  exports `Tile`, `Fabric`, `Use` and `Kind` (lines 124, 139, 158, 194).
- Names declared more than once: `Tile` and `Phase` three times each; `Use`,
  `Op`, `Kind`, `Host`, `Fx` and `Expr` twice each. Every duplicate is in a
  separate program, for example `enum Phase` in
  `examples/games/snake/app.ptl:39`, `examples/games/flappy/app.ptl:81` and
  `examples/custom-apps/side-scroller/game.ptl:36`.
- Today's rule has caused **no** error in real code. It would the first time
  two of these files ended up in one program (a launcher importing several
  games, a shared `ui` that exports an `Align` enum), or when a bundled
  package exports a type whose name programs already use.

**Ambiguity diagnostics in the corpus** (`petal lint --lenient` on all 880
files):

| Diagnostic | Count | Where |
|---|---|---|
| `shadows-builtin` | 78 | 12 `diagram-canvas`, 12 cheesecake games, 5 `side-scroller`, … Top names: `card` 8, `draw_text` 6, `draw_line` 6, `commas` 6 |
| `` `x` is ambiguous`` (error) | 2 | test fixtures only |
| different-arity merge warning | 2 | test fixtures only |
| import hides a builtin | 1 | test fixture |
| class/enum "already declared in" | 1 | `test/vitest/fixtures/classes/dup` (the rule's own test) |

**Silent shadowing that the broad warning would report.** A script
(top-level `fn`/`let`/`var`/`const`/`state` against the `pub` names of each
relative module the same file imports plainly) finds **325 sites in 107
files**: 113 in `city_broker`, 83 in `solitude`, 26 in `moonbear`, …
Sampled sites are deliberate:
- `~/cheesecake/games/solitude/missions.ptl:395` declares `pub fn info()`
  and imports `./inventory`, which also exports `pub fn info()` (line 279).
  Each subsystem module exports `info`/`reset`/`step`, and callers qualify.
- `city_broker/dev/sim_driver.ptl:197` declares `pub fn month()` and calls
  `economy.month(c)`, `traffic.month(c)` and the rest, qualified.

**Locals named like a module qualifier**, when the file also reads
`q.<name>`: about 18 sites in 12 files, by a rough heuristic. The ones
checked are intentional:
- `city_broker/tests/present_test.ptl:287` has `let ui = panels.info()` beside
  `import ui`, then reads `ui.panel` from the local record.

## Design

### A. Qualified type names

A type annotation may be `qualifier.Name`, where `qualifier` is a module
qualifier bound by a plain import or by `as`, and `Name` is a `pub class` or
`pub enum` of that module:

```petal ignore
import ./shapes as sh
fn area(c: sh.Circle) -> float
  3.14159 * c.radius * c.radius
end
fn rounded(k: sh.Kind) -> bool
  k == sh.Kind.round
end
```

`sh.Circle` names the same type as a bare `Circle` would. An unknown
qualifier or a non-exported name gives the existing `unknown type name`
warning, with the module named:

> warning: unknown type name `sh.Hidden`: module './shapes' has no export `Hidden`

There is no new runtime behaviour: annotations are advisory, except the
`-> nil` and method-receiver cases, and both resolve through the same lookup.

### B. Warnings

**B1, adopt.** Declaring a class or enum with the name of a built-in class
(today only `Rect`, declared in `core/packages/core/src/classes.ptl:9`) is a
compile-check warning:

> warning: class `Rect` replaces the built-in `Rect` in this program: its
> methods (`center_x`, `inset`, …) and every library that takes a `Rect` now
> see this one — pick another name

There are no sites in the corpus. Once C lands, the class becomes the file's
own and no longer replaces anything outside the file. The warning then names
only the shadowing within the file.

**B2, reject.** A default warning for a top-level declaration that hides a
plain import's export. It is the documented tier rule ("shadowing is quiet",
[module-system.md](../module-system.md#collisions-are-loud-shadowing-is-quiet)).
It would fire 325 times on a convention people use on purpose. It cannot be
silenced except by renaming, and qualifying the *other* side is already what
the code does. It would also turn "a module grows a new export" into new
warnings in its importers, which the current design avoids on purpose.

**B3, opt-in lint at most.** `shadows-qualifier`: a parameter or `let` named
like a module qualifier, in a scope that then reads `q.<n>` where `n` is an
export of that module. This is rare (about 18 sites) and the checked ones
are intentional. The runtime error that results ("No method 'draw' on type
record") could name the shadowed module instead, which is cheaper than a lint.

### C. Module-scoped class and enum identity

A class or enum belongs to the module that declares it:

- **Scope.** `name_taken` checks only within the declaring module, and against
  the built-in type names. Two modules may each declare `enum Phase`, private
  or `pub`. A file may declare `class Circle` and plainly import a module that
  exports one. The file's own declaration wins (tier 1), and `shapes.Circle`
  still names the other.
- **Bare use.** A bare `Phase` resolves through the existing tiers. Two plain
  imports both exporting `Phase`, used bare, give the existing ambiguity error.
  Then `a.Phase` (expressions, patterns, and with A, annotations) or a
  selective import picks one.
- **Identity.**
  - An entry-file declaration keeps the tag it has today (`Phase.play`).
  - A declaration in any other module gets a tag that carries the module's
    canonical name, which `state` keys already use
    ([module-system.md](../module-system.md#state-in-modules)):
    `"./level::Phase.play"`, `"ui::Align.left"`.
  - `::` cannot appear in an identifier, so `split_enum_tag` stays
    unambiguous.
  - Two `Phase.play`s from different modules are then different values (`==`
    is false).
- **Display.** `print`, `str`, `.name` and `type()` drop the module part:
  `Phase.play`, `"play"`, `"Phase"`. Two different enums may then print alike.
  Error messages and `petal show-ir` use the full tag.
- **Methods.** `fn Box.area(b: Box)` attaches to the `Box` visible where it is
  declared. Methods stay program-wide for that class.
- **Hosts.**
  - A host-built tag without `::` resolves by its bare text when exactly one
    declared enum has that name. Text lookup is already the fallback
    (`core/src/enum_table.rs:22-26`).
  - When two enums share the name, the host writes the qualified tag.
  - `pb_builder_begin_enum` and `heap.enum_tag_str` are unchanged.
  - Reading a tag gets the full text, so a C++ `switch` on `"Kind.park"` must
    strip the module part or compare with the full tag. The bridge gains a
    `pb_enum_tag_display` helper.

## Before and after

All "before" snippets were run against `main` (the `nc/` scratch files in
the session scratchpad). All "after" snippets are **proposed**. No prototype
was built.

### 1. Two private enums in one program (the reported problem)

Before (`a.ptl`, `b.ptl`, and `m.ptl` importing both):

```petal ignore
// a.ptl
enum Phase
  play, over,
end
pub fn a_phase()
  Phase.play
end

// b.ptl
enum Phase
  menu, play,
end
pub fn b_phase()
  Phase.menu
end
```

```text
Error: b.ptl: enum `Phase` is already declared in `a.ptl`, so `b.ptl` may not declare it too [line 1, column 1]
```

After (C): the same files run and print `Phase.play Phase.menu`. The two
enums are distinct, and `a_phase() == b_phase()` is false even for a
same-named variant.

Writers no longer need to know every private type name in every module of
the program. A reader may now meet two different things that both print as
`Phase.play`, which is the cost of keeping the display short.

### 2. Qualified annotation

Before (works today, because a plain import also binds `Circle` bare):

```petal ignore
import ./shapes
fn f(c: Circle) -> int
  c.radius
end
print(f(shapes.Circle(2)))
```

Before, with an alias (fails today):

```petal ignore
import ./shapes as sh
fn f(c: sh.Circle) -> int      // Error: Expected ',' between parameters
  c.radius
end
```

After (A): the second form compiles. Under C, it is also the only way to
annotate with one of two same-named imported types.

Nothing new to learn: the qualifier works in the one position where it
didn't.

### 3. Where the old, silent form is better (B2 rejected)

Before (`~/cheesecake/games/solitude/missions.ptl`, trimmed):

```petal ignore
import ./inventory        // exports pub fn info()
...
pub fn info()             // line 395: this module's own summary
  ...
end
```

With a "hides an import" warning, this line and 324 like it across 107 files
would warn. The author's intent (each subsystem exports `info`, and callers
write `inventory.info()`) is already clear. The current rule is the better
choice.

### 4. Built-in class shadowing (B1)

Before (runs today, silently):

```petal ignore
class Rect
  q: int,
end
print(Rect(1))            // { q: 1 }
```

After: the same output, plus the B1 warning. The user can still choose the
name.

### 5. Interaction with `state` and hot reload

Before: a variant of a module's enum, kept in `state`, is the value
`Kind.park`, and it survives reloads by its tag text
([hot-reload.md](../features/hot-reload.md#editing-an-enum)).

After:
- **Entry-file enums:** unchanged.
- **Module enums:** the first run after upgrading builds
  `"./config::Kind.park"`. A variant saved by an older build (in `state` or a
  `--state-storage` file) keeps the old tag. That is the documented "enum
  renamed or removed" row: it prints and answers `name` and `type()`, but
  equals no current variant.
- **Afterwards:** moving an enum to another module changes its tag, in the
  same way that moving a `state` declaration already drops its slot.

Exposure in the corpus is `city_broker/config.ptl`'s four enums. No `state`
there is initialized directly from one of their variants, though records
holding them may be.

## Assessment

### Project goals

**Findings.**
- A supports optional types: an annotation should be writable wherever the
  type is reachable.
- C supports building things from modules and packages: bundled libraries
  and host interface packages can export types without reserving names in
  every program. [fn-ast-enums.md](fn-ast-enums.md) proposes the first
  exported enum in a bundled package.
- C fits the "a module growing a new export can never break a file that does
  not use that name bare" property ([Which name wins](../module-system.md#which-name-wins)).
  Today a module *adding a private enum* can break an unrelated module.

**Opinion.** This is generic language surface, not something to be
distinctive at. The model to follow is Rust's or Haskell's, and Petal already
does for values what they do.

### Ease of writing

**Findings.**
- A adds no concept.
- C removes a rule that newcomers trip on ("why can't my private type have
  this name?") and adds a subtler one: two types can print alike.
- B1's message names the consequence. B2 would have taught people to rename
  or ignore warnings.

**Opinion.** Someone coming from Python, JS, Lua or Go expects C's behaviour.
In all of them, two modules' same-named types are distinct.

### Bug-freedom

**Findings.**
- C removes an action-at-a-distance error and adds a possible confusion:
  comparing `a.Phase.play` with `b.Phase.play` is false although both print
  alike. The checker sees both as different `EnumOf` types and can warn on
  `==` between different enums. Does it today? Open question.
- A lets the checker see annotations it cannot see now.
- B1 catches a silent replacement of `Rect` that would break every library
  method on it.
- The differential oracle and golden corpus see no display change. No golden
  example declares a type in an imported module.

### Performance

**Findings.**
- Tags are interned once (`EnumTag`, 4 bytes), and variant reads go through
  the slot index (`enum_table.rs:16-21`), not the text. A longer tag costs
  nothing per read.
- `type()` and printing strip the prefix on demand. That is a string slice on
  paths that already allocate.
- No measurement was taken: no benchmark creates enough distinct tags for this
  to show. `test/benchmarks/micro/enum_reads.ptl` is the one to rerun.

### Live coding and hot reload

**Findings.**
- Entry-file tags are unchanged.
- Module tags change once at upgrade (see pair 5), then are stable under
  edits, and change when an enum moves between modules, in line with
  `state` keys.
- Reload equivalence with a fresh compile is unaffected: the tag is a pure
  function of the canonical module name and the declaration.

### Introspection, debugging and program manipulation

**Findings.**
- Provenance, `explain` and direct manipulation work on terms, not type
  names: no effect.
- Tools that print values (`petal run --observe`, the trace viewer) show the
  short display. They should offer the full tag on demand, because two values
  may print alike.

### Maintainability

**Findings.**
- **A:** the parser's `TypeAnn` takes a dotted name; the checker resolves
  through the module scope `ClassTable::lookup` already uses
  (`classes.rs:266-272`). About 100 lines plus tests.
- **B1:** one check beside `name_taken`.
- **C:**
  - `ClassTable` keyed by (module, name);
  - `enum_variant_tag`/`split_enum_tag` and class-tag construction take the
    module;
  - display stripping in `print`/`str`/`type()`/`.name`;
  - `EnumLayout` unchanged;
  - method resolution through the visible class;
  - C bridge helper and `ffi.md`;
  - the `dup` fixture flips from error to success.

  Medium: an estimated one to two weeks with tests. It adds an invariant:
  display strings are no longer identities, so nothing may key on
  `type(v)`'s text across modules. The corpus has no `type(x) == "SomeClass"`
  comparisons.

### Domains

- **Games:** helps. Per-subsystem modules with their own `Phase`/`State`
  enums are the cheesecake games' shape. Today they collide only by luck.
- **UI apps, Garden:** neutral now. Helps once `ui` or `garden` export enums
  (alignments, key codes).
- **Graphics (`glsl`):** neutral. It enables the `ast` package of
  fn-ast-enums.
- **Dashboards, web:** neutral.
- **Embedding hosts:** a small cost. A host that builds or reads tags of a
  module's enum must use the qualified tag when names repeat.

### Compatibility and migration

**Findings.**
- A and B1 are additive.
- C breaks no compiling program: it only allows what was an error. It changes
  the tags of module-declared enums (4 enums in one program), with a one-time
  stale-state effect, and the meaning of a host-built bare tag when two enums
  share a name (none today).
- No mechanical migration is needed.

### Prior art

- **Rust and Haskell:** two glob/plain imports offering one name are an error
  only at an ambiguous use. Petal already does this. Types are path-qualified
  (`a::Phase`, `M.Map`), and identity includes the defining module.
- **Elm:** qualified types (`Html.Html`) and explicit exposing lists.
- **Python:** module namespaces. Shadowing by a local is silent. Linters
  (pyflakes F811) report *redefinition*, not shadowing of an import, for the
  same noise reason as B2.
- **Go and Lua:** always qualified, so there is nothing to collide.
- **Erlang:** atoms and record names are global, which is Petal's model today.
  Elixir moved structs to module-named identities (`%MyApp.Phase{}`) to
  escape it.

### Alternatives considered

- **Do nothing.** Nothing collides today. Library authors prefix exported type
  names (`AstKind`, `UiAlign`), the C/Objective-C convention. Free, and
  enough while exported types are rare. It does not help private types in
  combined programs.
- **Qualify only `pub` types.** Leaves the private-type collision, which is
  the surprising one.
- **Qualify every type, entry file included.** Uniform, but it changes every
  existing variant tag and every `state` value holding one. The entry-file
  exemption copies what `state` keys already do.
- **Disambiguate only on collision.** The second `Phase` gets a qualified tag.
  Rejected: identity would then depend on what else is in the program, so
  adding a module could change an existing value's tag.
- **B2 as an opt-in lint.** Acceptable. Nobody asked for it, and the corpus
  says the pattern is intended.

## Migration and compatibility

- **A, B1:** none.
- **C:**
  - Flip `test/vitest/fixtures/classes/dup` to expect success.
  - Note the tag change for module enums in CHANGELOG under **Changed**.
  - Document qualified host tags in `docs/usage/ffi.md`.
  - Tell cheesecake that `city_broker/config.ptl`'s enum variants saved before
    the upgrade go stale once.

## Implementation sketch

1. **A:** parser and checker for `q.Name` types, with the `unknown type name`
   message naming the module. Tests in the checker suite.
2. **B1:** a warning in `ClassTable::declare` when a non-built-in declaration
   takes a built-in class's name.
3. **C**, in order:
   - module-keyed `ClassTable` and `name_taken`;
   - tag construction and splitting;
   - display stripping;
   - method resolution;
   - host-tag fallback and the bridge helper.

   Test with the `nc/` cases above, two same-named enums across modules
   compared with `==`, `match` on each, state across a reload, and a host-built
   bare tag in both the unique and the ambiguous case. Run the golden corpus
   and the BC-noopt vs BC-all differential unchanged.
4. Do C before fn-ast-enums lands, or have that proposal use a prefixed name
   and an opt-in package as it already suggests.

## Implementation status

**A (qualified type names): shipped 2026-11-09.**
- The parser reads `q.Name` wherever a type name goes (parameters, `let`,
  `var`, `state`, class fields, return types), one qualifier deep, and keeps
  it as one written name. `q.Name?` is the nullable form; the `?` follows the
  whole name. The CST projection joins the same three tokens.
- `ClassTable` carries the current file's qualifiers (alias → module as
  written + its exports), set beside the per-file scope in
  `prescan_declarations`. `declared_type` resolves `q.Name` to the class or
  enum the module exports, ignoring the bare-name scope, so it is the same
  type the bare name is. Everything that resolves an annotation (checker,
  method-receiver pinning, `-> nil`) goes through it.
- The `unknown type name` warning says why a qualified name missed: "module
  './shapes' has no export `Hidden`", "no import binds the module qualifier
  `nope`", or "`helper` exported by module './shapes' is not a class or
  enum". A selective import binds no qualifier, as for expressions.
- Under C, `qualified_type` is the one place that must look the member up
  by (module, name) rather than by bare name.

**B1 (built-in class name): shipped 2026-11-09.** A top-level `class` or
`enum` named like a built-in class is a compile-check warning, emitted by
the compiler beside the other per-file declaration warnings (not in
`ClassTable::declare`, so that C's rework of the table does not have to
carry it). Message: "class `Rect` replaces the built-in `Rect` in this
program: its methods (`center_x`, `center_y`, …) and every library that
takes a `Rect` now see this one — pick another name". The language guide's
first `class` example, which declared `class Rect`, now declares `class Box`.

**Ecosystem sweep for B1:** no `class Rect` or `enum Rect` in this repo's
`.ptl` files, `sample-apps`, `garden`, `~/worlds-fair/ui/ptl`,
`~/petal-creative`, `~/cheesecake/games` or `~/biz/petal-lang.org`. The
website's copy of the language guide carries the old `class Rect` example
until its next docs sync.

**Tests:** `core/tests/qualified_types.rs`; CST-projection parity cases in
`cst_project.rs`; the B1 warning in `test/vitest/classes.test.ts`.

## Open questions

- **Separator and display.** Is `::` the right internal separator? Should
  `print` ever show the module, for example only when two loaded enums share
  a name? The language owner decides.
- **Enum mixing.** Does the checker warn on `==` between values of two
  different enums today? If not, it should come with C.
- **Host interface packages.** A host that matches tags in C++ must change
  its comparisons. Ask the cheesecake and petal-creative owners.
- **Methods on a built-in class.** Should methods added to `Rect` by a module
  stay program-wide, as now? C keeps them so, and it is not otherwise
  examined here.
- **B3.** Is a better runtime error, one that names the shadowed module, enough
  instead of a lint?
