# Proposal: `@` annotation syntax

Status: **accepted** (D1), 2026-11-09, with the owner's decisions recorded
under [Open questions](#open-questions): `@` is an accepted *alternative*
spelling and the call forms stay fully valid; value-first insertion; `tunable`
takes named `min:`/`max:`; one-line annotations are allowed. F1 and F2 stay
deferred. D1 is implemented (see [Implementation](#implementation-as-built)).
Requested in `docs/tasks/annotation-syntax.md` (removed 2026-11-11 once shipped). The old `@` rebind operator
this proposal inventoried was removed the same day (commit `f0522249`); the
sections below that describe it are a record of what was there.

## Summary

The request: give `@` the meaning Python gives it, syntactic sugar that runs a
declaration (or an expression) through a function and binds the result, and
use it for the static annotations `const` and `tunable`.

The findings, in short:

- **The `@` rebind operator** (`f(@x)` meant `x = f(x)`) was in the parser,
  the CST, a desugar pass and about 20 readers when this was written, but the
  Petal-using corpus barely used it: one `.ptl` file in this repo
  (`test/at-arg-update/main.ptl`, 4 uses), about a dozen Rust string fixtures,
  and nothing in Cheesecake, petal-creative, worlds-fair or petal-lang.org.
  It has since been removed (`f0522249`; `petal migrate-old-code`'s
  `at-rebind` migration rewrites it). The lexer still produces the `@`
  token. The inventory is in
  [The old `@` rebind operator](#the-old--rebind-operator).
- **`@` at the start of a statement is unused syntax.** Today an `@` anywhere
  an expression or statement starts, `@d` and `@d(1)` included, is a parse
  error, so a statement-start annotation cannot change what any working
  program means. Python uses the
  same trick: `@` is matrix multiplication between operands and a decorator at
  the start of a statement.
- **Petal cannot match Python's semantics for `fn` exactly.** Python's
  decorator is a run-time call whose result is bound to the name. In Petal
  that conflicts with five things: top-level `fn` hoisting (rebinding a hoisted
  name is already a compile error), self-recursion (a Petal `fn` calls itself
  directly, so `@memoize fn fib` would not memoize the recursive calls; this
  was checked), arity overloading, method tables and compile-time method
  pinning, and every compile-time reader of a function (the type checker's
  call-site checks, `fn_introspect`, the LSP, provenance). The corpus also
  shows no demand: no top-level `let g = wrap(f)` and only one
  `let x = call(fn …)` anywhere in five projects.
- **For `let`, Python's argument order is wrong for Petal.** Python passes the
  decorated thing as the only argument of the decorator expression's result,
  so `@tunable(0, 10)` would mean `tunable(0, 10)(4.0)`. Petal's annotations
  take the value first (`tunable(4.0, 0, 10)`), and so do Petal's pipe
  (`x |> f(a)` is `f(x, a)`), method syntax and the old rebind operator. The
  convention that fits is Swift's property wrappers: the declared value goes
  in as the first argument.

**Recommendation: adopt a smaller version, now that the rebind operator is
removed.**

1. `@name` and `@name(args)` are **declaration annotations**. They are
   written at the start of a `let` (before `pub`), and `name` must be one of
   the reserved annotation builtins: `const`, `tunable` or `rigid`. They are
   allowed wherever the call form is: `@const` only on a top-level `let`,
   `@tunable` and `@rigid` on any `let` ([D1](#d1-proposed-declaration-annotations-on-let)).
   `@f(a, b) let X = v` means exactly `let X = f(v, a, b)`. Stacked
   annotations apply bottom-up, as in Python. The parser lowers it to the
   call, so the AST, the IR, the static classifier, DM, hot reload and hosts
   see what they see today.
2. **No user-defined run-time decorators** on `fn`, `class` or `enum` for
   now. `@` before `fn` is reserved for compile-time annotations with a fixed
   vocabulary (the natural first one is the effect clause from
   `native-effect-annotations.md`). Decorators in the Python sense are
   deferred and can be added later in the non-hoisted subset described under
   [Design](#f2-python-decorators-on-fn-not-proposed).
3. **No `@` in expressions.** The call form (`rigid(2.0)`, `const(e)`) already
   is the expression form, and an expression `@` has precedence problems.
4. Whether `@` becomes the canonical spelling, which means a second migration
   of about 1,870 declarations, is an owner decision
   ([open question 1](#open-questions)). This proposal recommended making it
   canonical for whole-initializer annotations on declarations, with the call
   form kept for sub-expressions. "Defer" is a reasonable answer too, because
   most of the gain is on 146 multi-line tables. **Decided: an accepted
   alternative.** Both spellings are valid and neither is deprecated; there is
   no corpus migration, no `migrate-old-code` rule and no lint either way.

## Motivation

The lifecycle refactor (`docs/tasks/lifecycle-annotation-functions-refactor.md`,
Phases 1 to 5 shipped on 9 November 2026) replaced the `const` and `config let`
keywords with identity builtins on the initializer. Its Goal sets this rule:

> **a keyword changes what a program does** (`let`, `var`, `state`, `memo`,
> `fn`). **An annotation changes only what the checker or the tools do with a
> value**, and it is a reserved builtin function that returns its first
> argument.

That rule says *what* an annotation is. The call syntax still makes an
annotation look like any other function call, and the refactor's own Risks
section names the cost: "`let X = const([` … `])` and `tunable({` … `})` add
nesting and a trailing `)`".

The corpus, measured on 9 November 2026 (top-level `let`/`pub let` whose whole
initializer is the annotation call; scripts in the session scratchpad, counts
by line pattern):

| Project | `const(…)` | `tunable(…)` | `rigid(…)` | Multi-line initializer | `tunable` with bounds or options |
|---|---:|---:|---:|---:|---:|
| petal (examples, garden, test, packages, libs, integrations) | 43 | 148 | 12 | 17 | 3 |
| Cheesecake (`games/`, `engine/script/packages`) | 268 | 1,384 (171 files) | 5 | 129 (79 `const`, 50 `tunable`) | 10 |
| petal-lang.org (`frontend/public/garden/samples`) | 0 | 8 | 6 | 0 | 5 |
| petal-creative, worlds-fair `ui/ptl` | 0 | 0 | 0 | 0 | 0 |
| **Total** | **311** | **1,540** | **23** | **146** | **18** |

About 9 more `tunable` calls use named arguments (`options:`, `step:`).

Uses that are *not* a whole initializer, where `@` cannot apply, are rare but
real. For example, `cheesecake/games/canopy/core/jump.ptl:10,21,35,38` uses
`rigid(2.0)` inside arithmetic, `canopy/core/level.ptl:85,105` uses
`tunable(1.7)` inside a list, and `petal-lang.org/.../bar-chart.ptl:13` puts
`rigid` on record fields.

Run-time function wrapping, the thing Python decorators are mostly used for,
is close to absent:

- top-level `let NAME = call(declared_fn, …)`: 0 in all five projects;
- top-level `let NAME = call(fn(…) …)`: 1 (`examples/console/speculate.ptl:40`,
  `speculate`, which is not a wrapper);
- functions whose one parameter is a function (`fn x(f)`): none of the hits is
  a wrapper. They are font helpers in `petal-ui/prelude/draw.ptl:1030`, DM
  helpers in `drag.ptl:1087`, and game code where `f`/`g` is data.

So the motivation is the annotations, not decorators.

## Python's semantics, exactly

From the Python Language Reference, "Compound statements" §8.7 *Function
definitions* and §8.8 *Class definitions*
(<https://docs.python.org/3/reference/compound_stmts.html#function-definitions>),
PEP 318 (function decorators, Python 2.4), PEP 3129 (class decorators, 3.0)
and PEP 614 (relaxed grammar, 3.9). Each point below was checked against
CPython 3.14.6 (script and `dis` output in the session scratchpad).

1. **Grammar.** `decorators ::= decorator+`,
   `decorator ::= "@" assignment_expression NEWLINE`. Before 3.9 (PEP 318) a
   decorator was a dotted name with an optional call. PEP 614 allows any
   expression, such as `@buttons[0].clicked.connect`. Each decorator ends with
   a NEWLINE, so a decorator always has a line of its own.
2. **What it means.** "Decorator expressions are evaluated when the function
   is defined, in the scope that contains the function definition. The result
   must be a callable, which is invoked with the function object as the only
   argument. The returned value is bound to the function name instead of the
   function object." So `@f1(arg) @f2 def func(): pass` is "roughly
   equivalent to `func = f1(arg)(f2(func))`".
3. **Evaluation order.** The decorator *expressions* are evaluated top to
   bottom first. Then the default argument values are evaluated, then the
   function object is made. Then the decorators are *applied* bottom-up (the
   one nearest the `def` first). Observed: `eval outer, eval inner,
   eval default, apply inner, apply outer`. The bytecode loads `a`, calls
   `b(1)`, evaluates the defaults, runs `MAKE_FUNCTION`, then `CALL` twice,
   then one `STORE_NAME f`.
4. **The name binding.** Only the final result is bound, and the original
   function "is not temporarily bound to the name". A decorator that looks up
   `func2` while decorating gets `NameError`. The result can be anything: a
   decorator returning `42` binds `func2 = 42`.
5. **Recursion goes through the name.** A function body refers to its own
   name by a lookup at call time (a global lookup for a module-level `def`,
   a closure cell for a nested one), so it reaches the *decorated* object.
   This is why `@functools.lru_cache def fib` memoizes the recursive calls.
   Checked: a logging decorator on a recursive `fact` logs `call 3`, `call 2`
   and `call 1`.
6. **Class decorators** (PEP 3129) follow the same rules on a `class`
   statement. The decorator receives the class object and its result is bound
   to the class name (`@dataclass` returns the same class, changed, unless
   `slots=True`, which makes a new class).
7. **Methods.** A decorator inside a class body runs while the body executes,
   and what is stored in the class namespace is its result, often not a
   function at all (`@property`, `@staticmethod` and `@classmethod` return
   descriptors).
8. **There are no decorators on assignments or expressions.**
   `ast.parse("@d\nx = 1")` is a `SyntaxError`. Python attaches metadata to a
   variable through its *type* (`x: Annotated[int, meta] = 1`, PEP 593), never
   through `@`. Decorators on variables have come up on python-ideas more than
   once and have never been accepted.
9. **`@` is also an operator.** PEP 465 (3.5) made `a @ b` matrix
   multiplication. The parser tells them apart by position: a decorator is
   `@` at the start of a statement.

## Design

### The old `@` rebind operator

**Removed on 9 November 2026 (`f0522249`).** What is left: the lexer still
produces `Token::At` (`core/src/lexer.rs:84`, now commented "reserved for
annotations"), and an `@` where an expression starts is a parse error from
`parse::at_rebind_removed_message`, which says to write `x = f(x)` and run
`petal migrate-old-code`. The `at-rebind` migration
(`core/src/migrate/at_rebind.rs`) rewrites old code. The inventory below is
kept as the record of what the removal touched; its line numbers are as of
`7844902b`, before the removal.

- **Lexer:** `Token::At` (`core/src/lexer.rs:84` "in-out argument marker",
  produced at `:640-643`, named at `:184`).
- **Parser:** a primary expression `@ident` becomes `ExprKind::AtVar`
  (`core/src/parse.rs:2474-2480`). Its operator name is at `:138`, and the
  error "`@var` cannot be called as a function" is at `:2388`.
- **CST:** `SyntaxKind::AtVarExpr` (`core/src/cst/mod.rs:96`), projected at
  `core/src/cst_project.rs:876`, with a round-trip test at `:1992`.
- **AST:** `ExprKind::AtVar` (`core/src/ast.rs:63-68`).
- **Desugar:** `core/src/desugar.rs:5-56` (module docs) and `:330-480`. It
  rewrites `f(@x)` into `x = f(x)` before compilation. This is the one pass
  that already runs "before anything analyses or compiles a module".
- **Compiler:** a deferred error for an `@x` that was not lifted
  (`core/src/compiler/expr.rs:63-73`, "`@x` can only be used as an argument to
  a call at statement level"). Rebinding roots are collected at
  `compiler/mod.rs:982, 2724, 2904, 3313`, and `compiler/function.rs:481`.
- **Readers that match the variant:** `ast_display.rs:242`,
  `fn_introspect.rs:564`, `source_diff.rs:990`, `static_value.rs:1261`,
  `typecheck/mod.rs:1665, 2937`, `typecheck/param_reqs.rs:514`,
  `suggest/named_args.rs:326`, `suggest/return_types.rs:510-511, 708`,
  `lint/dead_alias.rs:230-231, 504`, `lint/field_set.rs:129-130`,
  `lint/util.rs:169`, `lint/var_to_let.rs:368`,
  `apply_change/convert_to_var.rs:692`, the comment at `lint/mod.rs:83-86`,
  and the help text at `core/src/cli/help.rs:1140`.
- **Not in tree-sitter.** `editor-support/tree-sitter-petal/grammar.js` has
  no rule for `@`, so editors already show `f(@x)` as an ERROR node.
  petal-lang.org's `frontend/src/lib/petal/highlight.ts` does not know it
  either. `editor-support/vim/syntax/petal.vim` only listed `@` among its
  single-character operators (`petalOperator`); the removal took it out.
- **Corpus.** A lexer-accurate scan (comments and strings skipped,
  interpolation holes scanned) of every `.ptl` file found:

  | Where | `.ptl` files | Files with `@` | `@` tokens |
  |---|---:|---:|---:|
  | `~/petal` | 2,234 | 1 (+5 stale copies under `.temp/verify-runs`) | 4 |
  | `~/cheesecake` (whole repo) | 499 | 0 | 0 |
  | `~/petal-creative` | 93 | 0 | 0 |
  | `~/worlds-fair/ui/ptl` | 20 | 0 | 0 |
  | `~/biz/petal-lang.org` | 36 | 0 | 0 |

  The one file is `test/at-arg-update/main.ptl` (with `expects`), at lines 12,
  18, 24 and 25.
- **Rust string fixtures:** `core/tests/all/enums.rs:1161`,
  `cli/tests/apply_change.rs:465, 473, 621, 898`, and the unit tests in
  `desugar.rs:494-572`, `cst_project.rs:1992` and
  `suggest/return_types.rs:708`.
- **Docs:** `docs/syntax/rebind-operator.md` (the whole page),
  `docs/language-guide.md:1053-1075`, `docs/writing-petal-guide.md:434-438,
  537`, `docs/syntax/overview.md:132, 545`, `docs/syntax/var.md:92-93`,
  `docs/CLI.md:1015, 1196, 1452, 2367`, `docs/Builtins.md:881`,
  `docs/usage/ffi.md:774` and `docs/README.md:46`. They are mirrored in
  `~/biz/petal-lang.org/content/docs/` (the same files plus
  `syntax/rebind-operator.md`). worlds-fair has a stale vendored copy under
  `ui/temp/vendor-head/`.
- **Policy:** `lint/mod.rs:83-86` already records that the linter stopped
  pushing code into `f(@x)` "because it reads as sugar that has to be
  learned".

Removing it was a prerequisite, and is done. With the rebind operator gone,
`@` means only "annotation" and an `@` in an expression is simply an error.
The removal was mechanical: `f(@x)` becomes `x = f(x)` (the nested form, a
preceding assignment), exactly what the desugar pass produced, and the
`at-rebind` rule of `petal migrate-old-code` does the rewrite.

### Other uses of `@` (no conflict)

- **Strings.** `@` inside a string or an interpolation's literal part is
  text, for example `examples/games/boids/boids.ptl:34`,
  `examples/productivity/settings/schema.ptl:299`, and
  `petal_quest/game.ptl:504` (`when "@" ->`). Interpolation uses `{…}`, not
  `@`.
- **Comments and doc tags.** These are free text. The effect-annotations
  proposal rejected `// @command` doc tags (its alternative 3).
- **Text dumps.** AST, IR and CLI dumps print spans as `@line:col`
  (`ast_display.rs:404-406`, `docs/CLI.md:2114, 2238, 2534`). This is output,
  never parsed as Petal.
- **GLSL.** Petal's GLSL output (the opt-in `glsl` library) contains no `@`.
  GLSL has no `@`.
- **WGSL** uses `@vertex fn`, `@builtin(…)` and `@location(…)` attributes
  (Cheesecake's `engine/gfx/src/webgpu/webgpu_device.cpp:367`). This appears
  only in C++ strings, and it is precedent for the syntax rather than a
  conflict.
- **The CLI** has no `@` selector syntax.

### Today's parse of the proposed syntax

The proposed syntax is checked against today's compiler
(`cli/target/debug/petal`, after the rebind operator's removal):

```
@d                      `@d` is not an expression: the `@` rebind operator was removed. …
fn f(x) … end           (parse error)

@d(1)                   the same parse error
fn f(x) … end

@tunable let X = 4      the same parse error
```

No program that runs today has `@` at the start of a statement, so giving
that position a meaning changes no working program. (Before the removal,
`@d` alone was a compile error and `@d(1)` a parse error, so this held then
too: the rebind operator could have stayed alongside annotations, because an
annotation is at the start of a statement and a rebind was inside a call's
argument list. Removing it first was still the right order, because "`@`
means one thing" is what makes the annotation readable.) When D1 lands, the
parse error for `@` in an expression should point at the annotation syntax
as well as at the migration.

### D1 (proposed): declaration annotations on `let`

```
annotated_decl ::= annotation+ decl
annotation     ::= "@" NAME [ "(" args ")" ] ( NEWLINE | " " )
NAME           ::= const | tunable | rigid     -- the reserved hint names
decl           ::= [ "pub" ] "let" NAME [ ":" type ] "=" expr
```

- **Meaning: insert the value as the first argument.**
  `@f(a, b) let X = v` is `let X = f(v, a, b)`, and `@f let X = v` is
  `let X = f(v)`. Named arguments pass through:
  `@tunable(options: ["title", "play"]) let START = "title"` is
  `let START = tunable("title", options: ["title", "play"])`.
- **Stacking: bottom-up, as Python applies them.** `@a @b(1) let X = v` is
  `let X = a(b(v, 1))`. The annotation nearest the declaration is innermost.
  `@const @tunable let X = 4` therefore becomes `const(tunable(4))`, which
  keeps today's error: "a setting of literals is already static; drop
  `const(…)`".
- **Evaluation order:** not observable. All three annotations are identity
  functions, `const` is erased at compile time, and the arguments are static
  data. So the Python order (decorator expressions first, then the value) and
  the call's order (the value first) cannot be told apart. Petal evaluates in
  the call's order, because the parser produces the call.
- **Placement:** on the line before the declaration, or on the same line
  (`@tunable let KICK = 6.0`). Python requires a line of its own. Petal allows
  both, as Java, TypeScript and Swift do, because 1,384 Cheesecake settings
  are one-liners and a separate line would double the length of a settings
  file. Annotations go **before** `pub`. A comment or blank line between an
  annotation and its declaration is allowed. `petal fmt` keeps the layout as
  written, since it is token-based (`core/src/fmt/mod.rs:1-30`), and
  `spacing.rs` gets one rule: no space after `@`.
- **Lowering:** the parser builds the `Call` expression, so `StmtKind::Let`
  and everything downstream are unchanged. The CST keeps an `Annotation` node
  and the `@`, so source preservation, `petal fmt` and text rewriters see what
  was written. The synthetic call's span covers the value. The call node
  carries a `written_as_annotation` marker (or the CST is consulted) for the
  three kinds of tool that splice text around an initializer: the migration
  fixers, `apply_change`, and Garden's DM hint toggler.
- **Where it is allowed:** wherever the call form is. `@const` follows the
  `const(…)` placement rules (top-level `let` only, until lifecycle Phase 7).
  `@tunable` and `@rigid` are allowed on any `let`. Inside a function body,
  `@tunable` is a DM hint, exactly as the call is.

Errors (proposed wording):

```
@foo let X = 1
  error: `@foo` is not an annotation. `@` takes `const`, `tunable` or `rigid`.
         To pass the value through a function, write `let X = foo(1)`.

@tunable(0, 10) fn speed() … end
  error: `@tunable` annotates a `let`; a function is not a setting.

@const var X = 1          (and `state`, `memo`)
  error: `@const` checks a top-level `let`; a `var` is assigned by definition.
         (the misplaced-const(…) family, unchanged)

@tunable var X = 1
  warning [tunable-on-var]: (the existing lint, unchanged)

@const
  error: an annotation must be followed by a declaration (`let`, `pub let`).

let y = @rigid 2.0
  error: `@` annotates a declaration and goes before `let`.
         Inside an expression, write the call: `rigid(2.0)`.

@tunable 4                (an expression statement)
  error: (the same: an annotation needs a declaration)
```

`class`, `enum`, `var`, `state` and `memo` get no annotations in D1. None of
the three annotations means anything on a class or enum, and on `var`,
`state` and `memo` the call form is already an error or a lint finding with a
better suggestion (`dm-hint-state-init`, `state-as-constant`). Lowering to the
call keeps those diagnostics without new code.

### Argument order: why not Python's

Python's rule gives `@tunable(0, 10) let SPEED = 4.0` ⇒ `tunable(0, 10)(4.0)`.
There were four ways to deal with the mismatch:

| Option | `@tunable(0, 10) let S = 4.0` means | Cost |
|---|---|---|
| **A. Python, curried** | `tunable(0, 10)(4.0)` | `tunable(0, 10)` already means "value 0, min 10". Arity cannot tell the two forms apart, so the curried form would need another name (`range(0, 10)`) or named arguments only. The result is a closure made at run time, which breaks erasure (`const` is a compiler intrinsic) and every reader that recognizes the annotation by its call (`static_value::tunable_args`, `setting_value_expr`, `is_setting`, `dm_hints.rs`). |
| **B. First-argument insertion (proposed)** | `tunable(4.0, 0, 10)` | It is not Python. It matches Petal's own conventions (`x |> f(a)` is `f(x, a)`, checked; `x.f(a)` is `f(x, a)`; the old `f(@x)`), Swift property wrappers, and today's builtins, so it needs no change to any builtin, reader or host. |
| C. Python for `fn`, B for `let` | — | Two rules for one character. |
| D. Last-argument insertion | `tunable(0, 10, 4.0)` | It matches nothing in Petal. |

B is the only option that keeps the shipped annotation functions exactly as
they are.

### F1 (proposed, reserved): compile-time annotations on `fn`

`@` before `fn`, `pub fn`, `fn T.m` or `native fn` is reserved for annotations
from a fixed vocabulary that **do not call anything**. They are read by the
compiler and tools, as the effect clause in `native-effect-annotations.md`
would be (`@reads(POINTER) native fn mouse_x() -> int` instead of
`native fn mouse_x() -> int reads(POINTER)`). D1 adds no `fn` annotations.
Until one exists, the parser reports "`@` on a function is reserved for
compile-time annotations; there are none yet."

This is how Rust, Java, Kotlin, C#, Dart and WGSL use annotations. It keeps
the Goal's rule visible: whatever follows an `@` never changes what the
program computes.

### F2 (not proposed): Python decorators on `fn`

The decorator would be any expression, and `@d fn f(…) … end` would mean
`let f = d(<the function>)`. This was investigated so the owner can see the
whole cost.

| Petal feature | What Python's semantics would do to it | Evidence |
|---|---|---|
| **Hoisting** (`language-guide.md` "Declaration order") | A hoisted `fn` is bound to a cell before the file's first statement (`compiler/function.rs` `bind_fn_result`; `compiler/mod.rs:2770-2860` `hoistable_fn_names`). The decorator can only run where the declaration is written, so every call that runs before that point would get the *undecorated* function. Python gets `NameError` there instead. A decorated `fn` would have to stop being hoisted, which a reader cannot see at the call site. | Today the manual equivalent `inc = twice(inc)`, after a hoisted use, is a compile error: "`inc` is a function declared in this file and is used before its declaration, so its name cannot be reassigned." |
| **Self-recursion** | A Petal `fn` body calls itself directly, not through the name at call time. `@memoize fn fib` would memoize only the outermost call. | `fact = logged(fact)` then `fact(3)` prints `call 3` once in Petal and `call 3`, `call 2`, `call 1` in Python (both run). |
| **Overloading by arity** | Variants are compiled as `name#arity` and joined by `MakeOverloadSet` (`compiler/function.rs:204-245`), whose members must be closures with known arity. The decorator could see one variant (and its result might not be callable) or the whole set (which is not Python's "the function object"). | `inc = twice(inc)` on a two-variant set runs, with `twice` receiving the set. |
| **Methods** (`fn T.m`) | They are registered in the class method table under `"T.m"` and bound at compile time when the receiver is pinned ("When the call is resolved at compile time"). A decorated method would be a run-time value, so pinning would have to fall back to dynamic dispatch, or bind to the undecorated body, which is wrong. | — |
| **`native fn`** | It has no body. The decorator would wrap a host function, and `--effect-audit`/`NativeEffects` rows describe the native, not the wrapper. | — |
| **`pub`** | It works: `pub` exports whatever is bound. The module's export table would list a value, not a declared function, so importers' type checks are lost. | — |
| **Type checking** | Call sites are checked against the declared signature ("A method's annotations are read at the call site"). After decoration the name's type is the decorator's return type, `any` unless the decorator declares one. Typed code loses its checks at every call. | — |
| **Static classification** | A top-level `fn` is known code. A decorated one is the result of a call, which is static only if the decorator is and is not a function value (`const` refuses functions). The cache and DM see a call term where they saw a function. | — |
| **DM, provenance, `explain`, `fn_introspect`, LSP** | They trace a value back to `fn f`'s body. Through a wrapper closure they reach the wrapper's body, and go-to-definition, hover signatures and `propose-edit` lose the declaration. | — |
| **State and hot reload** | `state` is keyed per call path (`PathPart::Call`, `core/src/memo.rs:56-62`). A wrapper adds a call frame, so adding or removing a decorator moves every slot below it and resets that state. Hot reload must also treat a decorator edit as structural. Reload drops closures held in state ("closures, the captured function table: dropped"), so a decorator that returns a closure kept in state behaves like any held closure. | `docs/features/hot-reload.md` table |

A defensible subset does exist: a decorated `fn` is not hoisted, cannot be
overloaded, cannot be a method or a native, and recursion is redirected
through the binding. Even that subset is not Python's semantics exactly
(point 5 above needs the redirect), it costs about as much as the rest of
this proposal together, and nobody in the corpus wants it. Petal also already
has keywords for what decorators are most used for: `memo` (caching), `test`
(registration) and `native` (FFI). TC39 kept decorators off JavaScript
function declarations for the hoisting reason in the first row.

### E (not proposed): `@` on expressions

`let y = @rigid 2.0 * g` raises a question with no good answer: does the
annotation take `2.0` or `2.0 * g`? Julia's macros (`@time f(x)`) take "the
rest of the expression" and are a known source of surprise. Python has no
expression decorators. The call form already is the expression annotation
(`rigid(2.0)`, and `const(e)` in lifecycle Phase 7), and Garden's DM hint
toggler writes exactly that form around a literal
(`garden/garden-script/src/dm_hints.rs`). Expression `@` would add a second
spelling with worse precedence and would collide with the rebind operator if
that were kept.

## Before and after

"Before" snippets are quoted from the corpus. Trimmed versions were run on
today's compiler (`cli/target/debug/petal run`): output
`7.0 1800.0 32 2.863022388059701 title`, with no diagnostics beyond the
`state` notice. "After" snippets are **proposed syntax** and do not run.

**1. A settings block with bounds, the common Cheesecake case**
(`cheesecake/games/canopy/core/tune.ptl:12-15`)

```petal ignore
// before
pub let RUN = tunable(7.0, 3.0, 12.0)                 // top speed, tiles/s
pub let ACCEL = tunable(60.0, 10.0, 200.0)            // on the ground, tiles/s²

// after
@tunable(3.0, 12.0) pub let RUN = 7.0                 // top speed, tiles/s
@tunable(10.0, 200.0) pub let ACCEL = 60.0            // on the ground, tiles/s²
```

The line length is the same. The value now sits where every other `let`
puts it, after `=`, so a reader scanning the column of values sees `7.0` and
not `tunable(7.0`. The bounds now come before the value they bound, which is
Swift's order and arguably worse for a reader who wants the value first. This
is a wash. There is one new mistake to make: `@tunable(3.0, 12.0)` looks like
"value 3, min 12" to someone who knows the call form.

**2. A plain setting** (`cheesecake/games/demo/basic.ptl:25`, one of 1,324
one-argument `tunable`s)

```petal ignore
// before
let KICK = tunable(6.0)          // upward impulse of the space-bar kick (N·s)
// after
@tunable let KICK = 6.0          // upward impulse of the space-bar kick (N·s)
```

Same length. The declaration now begins by saying it is a setting, so a
reader sees it before reading the name. That is the "make the annotation
visible" argument, and it is a modest one.

**3. A multi-line constant table, the case the change helps most**
(`cheesecake/games/gta/vehicles.ptl:22-29`, one of 146 multi-line sites)

```petal ignore
// before
pub let KINDS = const({
  sedan: {size: vec3(1.8, 0.6, 4.2), mass: 1200.0, engine_torque: 1500.0, max_speed: 30.0,
    wheel_radius: 0.38, ride_height: 0.35, wheelbase: 2.6, track: 1.6, cabin: 0.55},
  van: {size: vec3(2.0, 1.4, 4.8), mass: 1800.0, engine_torque: 2000.0, max_speed: 25.0,
    wheel_radius: 0.4, ride_height: 0.7, wheelbase: 3.0, track: 1.7, cabin: 0.0},
})

// after
@const
pub let KINDS = {
  sedan: {size: vec3(1.8, 0.6, 4.2), mass: 1200.0, engine_torque: 1500.0, max_speed: 30.0,
    wheel_radius: 0.38, ride_height: 0.35, wheelbase: 2.6, track: 1.6, cabin: 0.55},
  van: {size: vec3(2.0, 1.4, 4.8), mass: 1800.0, engine_torque: 2000.0, max_speed: 25.0,
    wheel_radius: 0.4, ride_height: 0.7, wheelbase: 3.0, track: 1.7, cabin: 0.0},
}
```

The `})` and the extra nesting are gone, and the table looks like the
declaration it was before the lifecycle migration (`const KINDS = {`). Adding
or removing the check is now a one-line edit, not an edit at both ends of a
table, which is what the migration fixer had to work hard at
(`lifecycle_keywords.rs`'s statement-end `)` and comment re-padding).

**4. Where the old form is better** (`cheesecake/games/canopy/core/jump.ptl:10`)

```petal
// before, and after: unchanged
pub fn apex()
  tune.JUMP_V * tune.JUMP_V / (rigid(2.0) * tune.GRAV)
end
```

A hint on a literal inside an expression has no declaration to annotate.
Inventing `let TWO = …` to use `@rigid` would be worse. The call form stays,
so `rigid` and `tunable` have two spellings for good, and the guide needs the
one-line rule "on a declaration, `@`; inside an expression, the call."

**5. Interaction with `state` and hot reload** (the `START` pattern from
`syntax/settings.md`)

```petal ignore
// before
let START = tunable("title", options: ["title", "select", "play"])
state screen = START

// after
@tunable(options: ["title", "select", "play"])
let START = "title"
state screen = START
```

`@tunable state screen = "title"` is refused, by the same
`dm-hint-state-init` guidance as today. Changing `"title"` to `"play"` while
running is a literal patch with the path `START` in both spellings, because
the AST is identical. Respelling `let START = tunable(…)` as `@tunable …`
gives an identical AST with different spans. It must be classified like
`export` → `pub` (a `Full` diff), not as `None`, or the program's recorded
positions go stale. That is an implementation requirement, not a user-visible
change.

**6. A Python-style function decorator: not proposed** (constructed; the
corpus has none)

```petal ignore
// what someone coming from Python will try
@logged
fn fact(n)
  if n <= 1 then 1 else n * fact(n - 1) end
end
// D1: error: `@` on a function is reserved for compile-time annotations; there are none yet.

// what works today, and its real behaviour
fn fact(n) … end
fact = logged(fact)
print(fact(3))      // logs only `call 3`: the recursion calls the undecorated fact
```

Refusing this is a cost: someone coming from Python will try it. But
accepting it with the semantics in the last line would be a silent
difference from Python, which is worse than a clear error.

### Migration of existing code

| Change | Kind |
|---|---|
| Rebind operator removed: `f(@x)` ⇒ `x = f(x)` (and the nested form ⇒ a preceding assignment) | done (`f0522249`): the `at-rebind` rule of `migrate-old-code`, which writes what the desugar pass produced. One `.ptl` file and about 12 Rust fixtures were changed. |
| `let X = const(e)` / `tunable(v, …)` / `rigid(v)` as a whole initializer ⇒ `@…` | mechanical, *if* the owner makes `@` canonical; a `migrate-old-code` rule. About 1,870 sites, 1,657 of them in Cheesecake. |
| `const X = e`, `config let` (deprecated in Phase 5) | the existing lifecycle rules. If `@` is canonical, they could rewrite straight to `@const` / `@tunable` and skip the call form. |
| Nested hint calls | unchanged |

## Assessment

### Project goals

- **Findings.** "Easy to use": `@` is the annotation marker that people from
  Python, Java, TypeScript and Swift already read as "metadata on the next
  declaration". "Goal-based program manipulation" and "introspection": D1 is
  lowered by the parser, so they are untouched. Users of the data rely on the
  call form already. F2 would *hurt* both, because tools would lose the
  function behind a wrapper.
- **Opinion.** D1 serves the Goal's keyword/annotation rule better than the
  call form does: the syntax would now *show* that a thing is an annotation.
  F2 works against the rule, because a Python decorator is exactly an
  annotation that changes what the program does.

### Ease of writing

- **Findings.** What a newcomer must learn is "`@` takes the value as its
  first argument". A Python user would guess "the decorator is called with
  the thing", which is the same for the bare `@tunable` (1,324 of 1,384 uses)
  and differs only when there are arguments (18 positional sites). Two
  spellings now exist for `tunable` and `rigid`. The guide rule is "on a
  declaration, `@`; inside an expression, the call". Errors are listed under
  Design.
- **Opinion.** The bounds-before-value order in example 1 is the weakest
  point. A reader of `@tunable(3.0, 12.0) pub let RUN = 7.0` has to know the
  convention. Named bounds (`@tunable(min: 3.0, max: 12.0)`) would make it
  self-explaining, and `tunable` would have to accept them (open question 3).

### Bug-freedom

- **Findings.** The lifecycle refactor's Risks section names "a wrapper
  placed around part of an initializer" (`let X = const(a) + b`), which today
  needs `tunable-not-whole-initializer` and the misplaced-`const` error. `@`
  is whole-initializer by construction, so that mistake cannot be written in
  the `@` form. One new mistake is possible: reading `@tunable(3, 12)` as
  value-then-bound. The type checker sees the lowered call, which is typed as
  the identity (`typecheck/builtin_types.rs`), so nothing is lost. The
  differential oracle, the golden IR and the parity harnesses
  (`const_parity.rs`, `setting_parity.rs`) can prove the two spellings equal
  in the same way they proved keyword ≡ call.
- **Opinion.** D1 is a small net gain. F2 would add silent divergences from
  Python (recursion, hoisting) that no checker would report.

### Performance

- **Findings.** D1 costs nothing at run time and nothing measurable at
  compile time: the parser emits the call it would have parsed. Today's
  costs stay as they are (`tunable` is still a native call; lowering it like
  `const` is the lifecycle doc's open item). F2 would add a wrapper call on
  every call of a decorated function and defeat static binding and pinning.
  The benchmark that would show it is `petal bench` on a decorated hot
  function. Nothing in the corpus would be affected because nothing would use
  it.
- **Opinion.** Neutral for D1.

### Live coding and hot reload

- **Findings.** The AST is identical, so literal patches, setting paths
  (`SPEED`, `POST.bloom`), construct kinds (`tunable let`, `const`) and the
  equality "reload equals a fresh compile" all hold. One requirement:
  respelling between the forms must not be classified `None`, because the
  spans move (see example 5). Adding or removing an annotation is a construct
  change and a recompile, as adding or removing the call is today. F2 would
  reset the state under a decorated function on every decorator edit
  (call-path keys).
- **Opinion.** No new hazard for D1 if the respelling case is handled.

### Introspection, debugging and program manipulation

- **Findings.** Provenance, `explain`, time travel and `fn_introspect` read
  the AST and IR, which do not change. Source-side tools must learn the CST
  node:
  - Garden's DM hint toggler (`garden/garden-script/src/dm_hints.rs`), which
    wraps and unwraps literals line by line, must recognize `@tunable`/`@rigid`
    on a declaration line when the literal is the whole initializer. Otherwise
    it would wrap `@tunable let X = tunable(4)`.
  - `propose-edit` / `apply_change` (`convert_to_var.rs` changes the keyword
    and must keep or refuse the annotation).
  - The `migrate-old-code` fixers that splice at initializer text edges.
  - petal-lang.org's `DmStudio.tsx` `findKnobs`, which finds knobs by text.
- **Opinion.** This is manageable, but it is the real work, and the reason a
  parity test should run against DM and `propose-edit`, not only the
  compiler.

### Maintainability of the implementation

- **Findings.** Stages touched by D1:
  - The rebind operator is already removed (`f0522249`: the desugar pass's
    lifting, `AtVar` and its about 20 arms).
  - Parser: statement-start annotations, about 100 to 150 lines.
  - CST: an `Annotation` node and its projection.
  - fmt `spacing.rs`: one rule.
  - tree-sitter: an `annotation` rule, highlights and corpus tests; vim;
    petal-lang.org's `highlight.ts`.
  - LSP: completion after `@` (three names) and hover.
  - Garden's `dm_hints.rs`, `DmStudio.tsx`, `apply_change`.
  - A `migrate-old-code` rule, if `@` is canonical.
  - Docs: the language guide's "Annotations" section, `syntax/settings.md`,
    `Builtins.md`, `overview.md`.

  No IR, bytecode, VM, embedding API or bridge change. It removes one special
  case (`@` as rebind) and adds one (`@` as annotation). The invariant it adds
  is "an `@` annotation lowers to exactly the whole-initializer call".
- **Opinion.** It is cheap because it is pure sugar. F2 would touch hoisting,
  overloading, method tables, the type checker and every function reader.

### Domains

- **Games:** helps a little. Cheesecake's 171 settings files are the main
  audience, and the gain is mostly on the 129 multi-line tables. Neutral on
  per-frame code.
- **Graphics and creative coding:** neutral to small help. The DM samples on
  petal-lang.org use `tunable`/`rigid` on declarations (14 sites) and on
  record fields (`bar-chart.ptl:13`, `kite.ptl:14`), where the call stays.
- **Productivity, UI, dashboards, data:** neutral. petal-creative and
  worlds-fair use no annotations.
- **Web (`petal-web-canvas`, `petal-web-html`):** neutral beyond the
  highlighter.
- **Editor scripting and config files:** a small help. `.ptl` used as config
  reads more like config when settings are marked `@tunable`.
- **Embedding hosts:** neutral. Hosts read `StaticBinding` and the
  `"tunable"` JSON, which do not change.

### Compatibility and migration

- **Findings.** Nothing breaks: the new syntax is in unused space. The rebind
  removal breaks one test file and about 12 fixtures, and a rule handles them.
  Making `@` canonical would be a second sweep of the same about 1,870
  declarations that were swept on 9 November. It would be done by the
  `petal migrate-old-code` tool (`docs/dev/migrations.md`,
  `core/src/migrate/`). It could ship as an accepted alternative first and be
  canonicalised later. It depended on the rebind removal, which is done
  (`f0522249`). It closes off nothing: F1's reserved slot keeps both
  compile-time `fn` annotations and a later F2 possible.
- **Opinion.** The churn is the strongest argument against. If the owner does
  canonicalise, it is best to do it before lifecycle Phase 6 removes the
  keywords, so the removal errors can suggest the `@` form directly.

### Prior art

- **Python** (above): run-time call; result bound to the name; no variable
  decorators. Its expressive power is real, and so is its classic surprise:
  decorators that drop `__name__`/the signature, fixed by `functools.wraps`.
- **JavaScript/TypeScript** (TC39 decorators, stage 3; TS 5.0): classes and
  class elements only. A *field* decorator returns an initializer transformer,
  `(initial) => value`, which is Python's curried shape applied to a value.
  Function declarations are left out because of hoisting.
- **Swift** property wrappers (SE-0258): `@Clamped(0...10) var x = 4` becomes
  `Clamped(wrappedValue: 4, 0...10)`, the value first. This is the closest
  precedent for D1. Swift's other attributes (`@MainActor`, `@discardableResult`)
  are compile-time.
- **Java, Kotlin, C#, Dart** annotations; **Rust** `#[attr]`; **WGSL**
  `@vertex fn`, `@builtin(…)`: metadata read by the compiler and tools, never
  a call. This is F1's model. Rust's procedural attribute macros, which do
  rewrite, are a separate opt-in mechanism.
- **Julia** `@macro expr`: `@` in expression position, taking an AST. This is
  the closest precedent for E, and it shows the precedence surprise.
- **Elixir** `@attr value`: module attributes, which are a declaration form.

### Alternatives

1. **Do nothing.** The call form works, the migration just landed, and the
   guide's "Annotations" section already explains the rule. The cost is the
   146 wrapped multi-line tables and annotations that look like calls.
2. **Python-exact `@` everywhere** (F2 + A + E): rejected above.
3. **`@` on `let` only as an optional alternative**, never canonical, with no
   migration. This is the smallest version and delivers the multi-line gain
   to those who want it. Its cost is two spellings forever with no guide
   preference.
4. **Bring back keyword modifiers** (`const let`, `tunable let`): this
   contradicts the Goal's rule, which was decided four phases ago.
5. **A trailing form** (`let X = 4.0 @tunable(0, 10)`): it keeps the value
   first, but it parses as an operator and collides with E. No precedent.
6. **A formatter or lint fix for the `})` problem:** it cannot remove the
   nesting.

## Implementation sketch

Order:

1. **Remove the rebind operator.** Done (`f0522249`), with the
   `migrate-old-code` rule `at-rebind` (`f(@x)` ⇒ `x = f(x)`). An `@` in an
   expression is now a parse error that points at the migration; step 2
   should make it point at the annotation syntax too.
2. **Parser and CST:** `annotation+` at statement start, before `pub`/`let`.
   Lower to the call, innermost annotation nearest the value. Add the CST
   `Annotation` node and `cst_project`, plus the errors under Design. For
   `fn`/`native fn`, add the reserved-slot error (F1).
3. **Parity test** `core/tests/all/annotation_parity.rs`, on the model of
   `const_parity.rs`/`setting_parity.rs`. Rewrite every whole-initializer
   annotation call in `examples/`, `core/tests` and the guide's snippets into
   the `@` form, then assert identical IR (`ir_equivalent`), `show-static
   --json`, diagnostics, `pb_source_bindings_json` and reload construct kinds.
   Also assert that a respelling is classified `Full`, not `None`.
4. **Tools:** fmt spacing, tree-sitter (grammar, highlights, corpus), vim,
   LSP completion and hover, petal-lang.org `highlight.ts` and `findKnobs`,
   Garden `dm_hints.rs` (with tests for toggling on an `@` line), and
   `apply_change`.
5. **Docs:** the language guide's "Annotations" section, `syntax/settings.md`,
   `Builtins.md`, `overview.md`, `features/static-values.md`, and a
   `CHANGELOG.md` "Added" entry.
6. **If canonical** (open question 1; *not taken*: `@` is an alternative, so
   there is no migration): a `migrate-old-code` rule, then the
   corpus, petal-lang.org and Cheesecake (re-vendor, one commit per game, as in
   lifecycle Phase 4). The lint never pushes code the other way.

Size: steps 2 to 5 are about 3 to 4 days, and step 6 is a day per large
consumer.

## Implementation (as built)

Shipped 2026-11-09, with the decisions below.

- **Parser** (`core/src/parse.rs`, `Parser::parse_annotated`): `@name` /
  `@name(args)` at the start of a statement, any number, on the
  declaration's line or on lines of their own (comments and blank lines may
  sit between). The annotations are lowered onto the value by
  `ast::apply_annotations`, innermost nearest the declaration; the synthetic
  call's span is the value's and its callee spans the name, which is how
  `ast::is_annotation_call` tells the spellings apart for source tools. The
  declarations that take one are `let`/`pub let` and, so that the call form's
  own diagnostics apply, `var`, `state`, `memo` and the deprecated
  `const NAME =` / `config let`. The errors are the ones under Design, plus
  "annotations go before `pub`", "no space after the `@`" and "cannot also be
  named `value:`".
- **CST**: `SyntaxKind::Annotation` (`@`, the name, an optional `ArgList`)
  as the first children of the declaration node; `EventBuilder::hoist_open`
  moves the declaration's `Open` back over them. `cst_project` lowers them
  the same way (`Projector::lower_annotations`), so the debug-build check
  that projection ≡ direct parse covers them.
- **Diagnostics**: what is said about the call as a whole (its argument
  count, a named argument the builtin cannot take, a misplaced `const`) is
  blamed at the `@name` (`ast::call_diagnostic_span`; the compiler also
  places the call's IR term there), not at the declared value whose span the
  call has. The misplaced-`const` messages name `@const` when that is what
  was written ("drop `@const`", "`@const` takes no arguments").
- **Parity**: `core/tests/all/annotation_parity.rs` rewrites every
  whole-initializer `const`/`tunable`/`rigid` binding in `examples/`,
  `core/tests` (`.ptl` and Rust fixtures) and the guide, settings,
  static-values and Builtins snippets into the `@` form, on the same line and
  on lines of their own, and asserts identical IR, `show-static --json`,
  diagnostics, `static_bindings`, set-by-name results, DM knobs and reload
  construct kinds, and that a respelling is never classified `none`.
- **Tools**: `petal fmt` keeps annotations where they were written (no forced
  line break; decision 4) and keeps `@` tight to its name; tree-sitter
  (`annotation` rule, highlights, corpus), vim, LSP (completion after `@`,
  hover, symbols), Garden's DM hint toggler (`garden-script/src/dm_hints.rs`,
  which reads and cycles `@tunable`/`@rigid` on a declaration line) and
  petal-lang.org's `findKnobs` and highlighter.

## Open questions

All decided by the owner on 2026-11-09.

1. **Decided: an accepted alternative, not canonical.** The call forms
   `let X = const(e)`, `tunable(…)` and `rigid(…)` stay fully valid and are not
   deprecated. No corpus migration, no `migrate-old-code` rule, no lint pushing
   either way. (Asked: should `@` become *the* spelling for whole-initializer
   annotations on declarations, a second sweep of about 1,870 sites, or an
   accepted alternative with the call form canonical?)
2. **Decided: value-first (first-argument insertion) is approved.**
   `@tunable(0, 10) let S = 4.0` ≡ `let S = tunable(4.0, 0, 10)`. (Asked: is
   first-argument insertion acceptable as "not Python"?)
3. **Decided: yes.** `tunable` takes named `min:` and `max:`, in the call form
   and the annotation form alike, alongside `step:` and `options:`:
   `@tunable(min: 3.0, max: 12.0) pub let RUN = 7.0`.
4. **Decided: one-line annotations are allowed** (`@tunable let X = 4`), as is
   an annotation on its own line above the declaration, stacking bottom-up.
   `petal fmt` keeps the layout as written and does not force a line of its
   own for long argument lists.
5. **Deferred** (F1): no `fn` annotations yet; `@` before `fn` stays the
   reserved-slot error. A joint decision with `native-effect-annotations.md`
   comes with the first one.
6. **Deferred** (F2): no run-time decorators; `@` in an expression stays a
   clear error. A real need gets its own proposal.
7. **Decided: keep the annotations where they live now** (no
   `annotations.ptl` move).
