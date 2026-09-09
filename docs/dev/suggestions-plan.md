# A suggestion channel (`petal suggest`)

Status: **shipped** for type annotations. `rust/src/typecheck/infer.rs` holds
the analysis, `rust/src/suggest/` the command. The catalogue of further
suggestion rules in §7 is not scheduled.

The command reference is in [CLI.md](../CLI.md#suggest--propose-type-annotations),
and `petal help suggest` is the same text.

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
has a pinnable receiver that was not already pinned.

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

B=rust/target/debug/petal
$B suggest -I petal-libs petal-libs/bloom/src/motion.ptl
$B suggest --json -e 'fn f(a)
  len(a)
end
print(f([1]))'

# The corpus check: apply everywhere in a scratch copy, then require the IR
# to be unchanged for every file that was rewritten.
```
