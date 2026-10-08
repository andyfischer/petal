# A suggestion channel (`petal suggest`)

Status: **shipped** for type annotations (`rust/src/typecheck/infer.rs` holds
the analysis), for named arguments (`rust/src/suggest/named_args.rs`, over
the callee resolution in `rust/src/named_calls.rs`), for loop-tail return
types (`rust/src/suggest/return_types.rs`) and for advice
(`rust/src/suggest/advice.rs`); `rust/src/suggest/` is the command. The
catalogue of further suggestion rules in §8 is not scheduled.

The command reference is in [CLI.md](../CLI.md#suggest--suggest-safe-refactors-for-a-file),
and `petal help suggest` is the same text. Sections 2–6 below are about the
type-annotation kind; the later sections take the named-argument kind, advice,
and loop-tail return types in turn.

**`suggest` does not promise to preserve the IR.** It did, in effect, while
its only tenants were annotations (which happen to leave the IR alone across
the corpus) and named arguments (which are proven to). The return-type kind
ended that: `-> nil` is suggested *because* it changes what a function
compiles to. What the command promises instead is narrower and per kind — each
edit is held to the strongest proof that is true of it, the report says which
proof that was, and an edit whose safety depends on callers the analysis
cannot see is never written. See "Return types for loop tails" below.

## 1. Why a third channel

Petal already had two ways to say something about source, and neither fit.

| Channel | Verdict | Applied |
|---|---|---|
| `petal check` | a **warning** — this is wrong | never |
| `petal lint` | a **normalization** — this is not how we write it | automatically, `--fix` |
| `petal suggest` | a **proposal** — this might be what you meant | only on request |

The distinction that matters is not how confident the tool is, it is who
decides. A warning claims the code is wrong; a lint rule claims there is one
right spelling. A type annotation is neither. `fn probe(r)` is not wrong, and
`r: record` is not the only right spelling of it — it is a judgement about
what the function is *for*, and the author is the one who knows.

So the channel is proposal-shaped: it reports with evidence, it never runs
during a compile, it cannot fail a build, and applying is an explicit act with
its own gate.

## 2. What it infers today

Function **parameter** types and **return** types. Not `let` bindings: an
annotation on a local is checked at exactly one place, its own initializer,
which is the place that already told the checker the type. It buys
documentation and nothing else, and there are a great many of them.

## 3. The evidence

For a parameter, three sources, in `typecheck/infer.rs`:

- **Call sites.** What callers actually pass. Literal arguments make this
  productive in wholly un-annotated code: `ease_flag(true, 18.0)` says `bool`
  and a number outright.
- **Flows-into.** The parameter is handed straight to a call whose matching
  parameter *is* annotated. This is the source that compounds — see §5.
- **Field reads.** `r.w` proves `r` is record-shaped.

For a return type: the body's tail expression, and every explicit `return`.

Evidence is gathered by the type checker itself, on its ordinary walk, behind
a `collect_inferences` flag that an ordinary compile leaves off. There is no
second analysis to keep in step with the first: what `suggest` believes about
a type is by construction what `check` believes about it.

### Rejected sources, and why

**Arithmetic.** `x * 2.0` looks like it proves `x: num`. It does not: `vec2`
and `dual` are multiplicative too, and `vec2(1,2) + 2.0` is a `vec2`. Petal's
`+` rejects strings and lists, which is not the same as proving `num`.

**Truthiness.** `if p then …`, `!p` and `p && q` all accept any value, and the
logical operators *return* one (`0 || 42` is `42`), so none of them proves
`bool`. This is why bloom's `on`/`trigger` parameters get no suggestion even
though a reader can see they are flags.

**A field read naming a class.** Reading `.x`, `.y`, `.w` and `.h` picks out
`Rect` uniquely, and concluding `Rect` was the first implementation. It is
wrong: a plain `{x, y, w, h}` record has the same fields and is *not*
assignable to a class. `petal-ui`'s `point_in(px, py, r)` is called both ways,
and the wrong annotation was caught by the `--apply` gate on the first corpus
run. Field reads now conclude `record`; the matching class is reported as a
hint the author can narrow to by hand.

## 4. A parameter is a precondition, a return type is a promise

The two slots want opposite defaults.

**Numeric evidence about a parameter always concludes `num`.** The callers
this compile happens to see are not the callers there are: one call passing
`18.0` does not make the parameter a `float`, and three passing `18` do not
make it an `int`.

**A return type keeps the precise type** the body produces. `-> float` on a
function whose tail is a `float` is exactly true, and narrowing it to `num`
would throw away what callers can rely on. It widens only when the body really
does produce two different numeric types.

This split is why `binary_type` had to learn that `num + int` is `num` rather
than `any`. Before that, writing the annotation this tool most often proposes
*erased* every inference downstream of it — the tool would have been
undermining itself one pass at a time. (That change also turned up a latent
bug: `examples/console/classes.ptl` declared `fn Rect.area(r: Rect) -> int` on
a class whose fields are `num`.)

## 5. Suggestions compound

An applied annotation is evidence for the next pass, through the flows-into
source. On `petal-libs/bloom/src/motion.ptl` the first pass finds 16
annotations, and those 16 make 6 more visible on the second:

```
pass 1: 16 suggestions across 24 functions.
pass 2:  6 suggestions across 24 functions.
pass 3: no suggestions (24 functions examined)
```

Re-running to a fixpoint is the intended workflow, and `suggest` reaching
silence is a real statement about the file.

## 6. Safeguards

Three, layered.

**The evidence rules are the first gate**, and they are the important one:
everything in §3 concludes a type only when every observation agrees, and
`any` counts as silence rather than as disagreement. A slot whose evidence
conflicts gets no suggestion at all.

**Ambiguity is dropped.** Evidence is keyed by `(name, arity)` with no module
in it, matching the checker's own signature table. A key that more than one
module declares is dropped rather than suggested from a mixture.

**`--apply` is verified.** The rewritten source must compile, and must gain no
type-checker warning the original did not already have. That gate is what
makes the whole thing safe to iterate: a suggestion is a *claim* about a type,
and the checker is the thing that verifies claims, so a wrong inference costs
a refusal rather than a file. Baselining against the original matters —
plenty of working files carry a warning already, and counting those would
refuse every suggestion in them for an unrelated reason.

**Insertion is by splice, never by reprinting.** `Param` carries no span, so
the insertion point is scanned out of the declaration's own source text and
re-validated against it before it is accepted (the same discipline
`lint`'s cast rule keeps). A scan that goes wrong costs a suggestion, never a
corrupted file. Comments and layout inside a parameter list survive.

### Verified over the corpus

`suggest --apply` was run over every `.ptl` in `examples/`,
`garden/examples/` and `petal-ui/`:

- 57 files annotated, 0 refused;
- all 57 have **IR byte-identical** to the original under `petal ir-equal`,
  which is the strongest available statement that the rewrite is
  behavior-preserving.

IR equality is not an *invariant* of the feature and so is not the `--apply`
gate: an annotation that pins a method call's receiver to one class
deliberately changes codegen (see
[type-declarations-plan.md](type-declarations-plan.md), "Annotations drive
static dispatch"). It happens to hold across this corpus because nothing in it
has a pinnable receiver that was not already pinned. The return-type kind goes
further and changes the IR as its purpose (`-> nil`); the figures above are for
the type-annotation kind alone (`--only types`).

## 7. What it does not find

Measured against a hand-annotated `motion.ptl`, the tool proposes a strict
*subset*: everything it says is right, and it stays silent on about half of
what a human writes. The gaps are worth naming, because each is a specific
missing analysis rather than a general fuzziness.

- **`state` and `var` tails.** `ease_to` ends in `v`, a `state` binding, which
  types as `any` by design — a `set` can retype the cell from anywhere. So no
  return type, and the whole chain of functions ending in `ease_to(...)` is
  blocked behind it. **A write-set analysis** — if every assignment to a cell
  in its module is numeric, the cell is `num` — would unblock most of it, and
  is the single highest-value addition.
- **Flag parameters.** `on`, `trigger`, `enabled` are obviously `bool` to a
  reader and unprovable to the checker, per §3.
- **Defensive casts.** bloom writes `float(dur)`, `float(i)`, `float(step_s)`
  at nearly every use of a numeric parameter. `float()` accepts a string, so
  each of those casts *destroys* the evidence its argument would otherwise
  have carried. A library that annotated instead of casting would need neither.

## 8. Catalogue of further suggestion rules (not scheduled)

The channel is the reusable part; annotations are just its first tenant.
Rules that want to propose rather than enforce:

- **Extract a repeated literal to a `let`.** A magic number appearing four
  times is probably a name that was not written.
- **Split a function that exceeds a size.** Never a rule, always a proposal.
- **Name a lambda** that is passed to the same combinator repeatedly.
- **`str(a) ++ "/" ++ str(b)` → `"{a}/{b}"`.** Currently in the linter's
  unscheduled catalogue; it fits better here, since the interpolated form is a
  style preference rather than a normalization.
- **A class for a record shape** that is built with the same field set in
  several places.

Two of the linter's own unscheduled rules
([linter-plan.md](linter-plan.md) §6) arguably belong on this channel for the
same reason: `if c then x else x end → x` is a normalization, but "hoist a
repeated pure subexpression to a `let`" is a judgement call about naming.

## 9. Verification recipe

```bash
cd rust && cargo test --lib typecheck::infer::   # the evidence rules
cargo test --test suggest                        # the command, end to end
cargo test --test suggest_return_types           # loop-tail return types

B=rust/target/debug/petal
$B suggest -I petal-libs petal-libs/bloom/src/motion.ptl
$B suggest --json -e 'fn f(a)
  len(a)
end
print(f([1]))'

# The corpus check: apply everywhere in a scratch copy, then require the IR
# to be unchanged for every file that was rewritten.
```

## 8. Named arguments

The second kind of suggestion: write a long positional call with its
parameter names. It differs from annotations in what it claims — not "this is
probably what you meant" but "this is exactly what you wrote" — and so in what
proves it.

**Analysis from the IR, not the checker.** The checker's knowledge of a callee
is keyed by bare name across the whole compilation, which is the right
looseness for a warning and the wrong one for a rewrite. The compiled program
has the exact answer: a call term's callee edge, followed back through copies,
closure captures and function cells to the `MakeClosure` or `MakeOverloadSet`
that produced it (`named_calls::CallResolver`). SSA makes shadowing and
rebinding a non-question — a rebound name is a different term, and a join is a
phi, which does not resolve.

**The proof is IR equality, extended rather than weakened.** A call term keeps
the names its arguments were written with, so a named call and a positional
one are different IR and `ir-equal` says so. `ir_equivalent_modulo_named_args`
compares everything as strictly as before, and for a pair of calls whose
written names differ it resolves both callees with the same resolver and
accepts the pair only when each side selects the same variant
(`backend::calls::accepts_call`, the runtime's own definition) and binds every
argument to the same slot. An unresolvable callee is a difference. A builtin
the `Env` registers never reaches the comparison at all: the compiler has
already put its named arguments in positional order.

**True names are a separate question from a safe rewrite.** A variant that
serves two shapes on one count (the `ui` prelude's "also positionally"
declarations) would lend the wrong shape's names to a call that is provably
unchanged by them. The suggestion is withheld whenever the variants accepting
a call disagree on what its positional arguments are called.

The noise rules — which leading arguments stay positional, and which calls
are not worth naming at all (a bare `(r, g, b)` colour, a function literal
under a one-letter name, a call that would mostly echo its own arguments) —
are in the module docs of `suggest/named_args.rs` and in CLI.md. The second
group came out of applying the tool to the example corpus: every one of those
rewrites was provably safe and read worse than the call it replaced.

## 10. Advice: a suggestion with no rewrite

Everything above ends in text to splice, behind a proof that splicing it is
safe. That is the wrong shape for a finding like "this loop is an insertion
sort": the detection is a heuristic, the replacement depends on intent
(`sort` with the comparator already written? `sort_by` with a key?), and no
equivalence check could vouch for it. `petal lint` cannot hold it either —
every lint rule carries a verified fix.

So `petal suggest` has a kind with no rewrite, `advice` (`suggest/advice.rs`): a
place, a comment, and a rule name. It carries no edit, which is what keeps the
rest of the command's guarantees intact — `--apply` and `--verify` only ever
look at the rewriting kinds, and a `--json` consumer sees an empty `edits`.

The bar for a rule: right often enough to be worth reading, and impossible to
make exact enough for a lint fix. A false positive costs a line of output. The
first rule, `hand-written-sort`, exists because two example apps wrote an
insertion sort by hand after `sort(list, compare)` and `sort_by` had shipped;
it fires on both originals and on nothing else in the example corpus.

## 11. Return types for loop tails

A `for` in tail position collects a list, and a function's tail is its
implicit return ([implicit-return-values.md](../implicit-return-values.md)).
So an un-annotated function that ends in a loop is one of two different
things, and the source does not say which:

```
fn squares(xs)                 fn draw_all(items)
  for x in xs do x * x end       for it in items do draw(it) end
end                            end
```

The first is a mapping. The second builds, returns and drops a list of
whatever `draw` yields on every call. Since `-> nil` now turns the implicit
return off, the second has a one-word fix — and `suggest/return_types.rs` is
the kind that finds it (`--only return-types`).

**The evidence is how the function is called**, read from the AST by mirroring
the compiler's own value-position rule:

| Calls in view | Suggestion | `--apply` |
|---|---|---|
| some call uses the result | `-> list` | written; proven IR-equal |
| called, none uses the result | `-> nil` | written; compiles, no new warning |
| never called, or `pub` | both options, under `choose:` | skipped |

A call that is the tail of another un-annotated function is neither used nor
discarded — its result is that function's result — so usage is propagated
through such forwarding to a fixpoint. It only ever rises (unused → unknown →
used), which is what makes the order of resolution irrelevant.

**Matching is by name, and every doubt is resolved away from `-> nil`.**
Callee resolution here is deliberately cruder than the named-argument kind's:
that one proves a rewrite changes nothing, and needs the exact callee; this one
only needs to never *miss* a use. So a call counts toward every declaration of
that bare name (overload variants share their evidence; a method is matched by
its method name on any receiver), a `--from` file's calls count by name, a
tail call in a `--from` file is a use, a lambda's tail is a use, and a function
that is ever read as a value instead of called is "unknown".

**This is the one kind that is not IR-preserving.** `-> list` tells the checker
something already true, so it gets the strong proof: `ir_equivalent` on the
compiled programs. `-> nil` cannot be held to the IR — the changed IR is the
suggestion. It is behaviour-preserving exactly when no caller reads the list,
and that is an analysis result rather than something a recompile can check, so
the gate is the weaker one (compiles; gains no warning) and the safety comes
from where the suggestion is *withheld*:

- a `pub` function, whose callers are in other files;
- a function nothing in view calls — in a UI app or an embedding that is
  usually one the host calls by name, and the host is not in view either;
- a function passed around as a value;
- a function whose name some inner scope also binds (a `let`, a parameter, a
  loop or pattern variable, a nested `fn`): a statement call through that name
  may be a call to the local, and matching is by name.

Those are reported with both options and never written. `--json` says so per
item (`"preserves_ir": false`, `"usage": "unknown"`, empty `edits`).

**Scope: loop tails only.** "Every un-annotated function whose result no
caller uses" was considered and rejected. A function that ends in `count + 1`
and is called as a statement is not costing anything, and `-> nil` there would
be a claim about intent with no payoff; a loop tail is the case where the
implicit return has a real cost (an allocation per call) and where the
ambiguity is in the language rather than in the author's head. A body with a
`return <value>` of its own is skipped — the declaration would have to describe
that exit too — and a tail where only some branches end in a loop gets `-> nil`
or nothing, since there is no single type to name when it is used.

**Against the other kinds.** The type-annotation kind never proposes a return
type for a loop tail on its own (a `for` statement has no type to the checker),
but if it does have an answer for the slot, that one wins and this kind is
dropped for the function, so the two never insert at the same place. Return
types are proven after annotations and before named arguments, and each kept
edit becomes the baseline for the next.

Tests: `suggest::return_types::tests` (the analysis) and
`rust/tests/suggest_return_types.rs` (the command, including that an applied
`-> nil` is a different program under `petal ir-equal` and prints the same).
