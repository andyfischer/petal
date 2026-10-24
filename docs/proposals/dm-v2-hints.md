# Proposal: `tunable` and `rigid`, direct-manipulation hints

Status: proposed, 23 October 2026; implemented 24 October 2026 (milestone M2
of the plan: the builtins, `core/src/dm_v2/hints.rs`, the three lint rules,
docs/Builtins.md "Direct-Manipulation Hints"). The names still await the
owner's sign-off (open question 1). Part of Direct Manipulation v2: see the
[plan](../dev/direct-manipulation-v2-plan.md) (§2.6 and milestone M2) and the
[task](../tasks/direct-manipulation-v2.md).

## Summary

Add two builtins that do nothing at runtime and tell the direct-manipulation
(DM) solver which literals it may move:

```petal
tunable(x)                              // prefer moving x's literal
tunable(x, min, max)                    // ... and keep it in range
tunable(x, min: 0, max: 255, step: 1)
rigid(x)                                // never move x's literal; nothing upstream of x receives feedback through it
```

Each returns its first argument unchanged. The solver reads them from the IR.
This adds no syntax and no keyword, and nothing existing breaks.

**Recommendation: adopt.** It is a small builtin pair. `config let` stays the
top-level spelling for a tunable and is unchanged.

## Motivation

DM v2 backpropagates a drag through function calls into numeric literals
([plan §2](../dev/direct-manipulation-v2-plan.md#2-architecture)). Once it can
see through calls, a typical drag has several literals that could serve, and
the solver has to choose. Heuristics (measured collateral, distance from the
call, `config let`) settle most cases, but there are three things they cannot
know.

1. **A literal that must not move**, but is not obviously special. In
   `examples/dashboards/server-monitoring/app.ptl:490-493` the orbiting dot is
   at `mark_x + cos(orb) * 11.0` with `let orb = time() * 1.1`. With time
   frozen, `1.1` has a perfectly good derivative, and moving it would "move"
   the dot by changing the animation speed. The plan detects clock coupling
   with an extra run (§2.5.4), but the author knows the answer outright.
2. **A range.** A radius, an alpha, a count of columns: values with a
   meaningful interval. There is no way to say so today. `config let` has no
   bounds.
3. **Function-body and call-site literals.** `config let` inside a function is
   accepted but invisible to the file-level tools
   ([config-let.md, "Where it is allowed"](../syntax/config-let.md#where-it-is-allowed)),
   and a call-site literal cannot be `config` at all. Layout helpers keep
   their constants in the body. For example `label_caps`
   (`server-monitoring/app.ptl:84-86`, `spacing: 1.4`) and `tx_right`
   (`:75-77`).

Corpus counts (2026-10-23, `.ptl` under examples, garden, core-libs, the
prelude, `~/worlds-fair/ui/ptl`, `~/petal-creative` and `~/cheesecake/games`):

- `config let` appears in 177 files. Authors already mark knobs when they can.
- 21 example files call `random`/`random_int`/`noise`. Those are handled by
  the runtime (replayed), not by hints.
- 58 example files use `var`.

There is no existing way to write cases 1 to 3.

## Design

### Signatures

| Call | Returns | Static meaning |
|---|---|---|
| `tunable(x)` | `x` | the literal `x` resolves to gets DM weight 0.25 (preferred) |
| `tunable(x, min, max)` / `tunable(x, min:, max:, step:)` | `x` | the same, with a box constraint. `step` rounds the final value to a multiple of `step` from `min` (or from 0) |
| `rigid(x)` | `x` | the literal `x` resolves to is never a DM target. In a speculative run the call returns its baseline value, so feedback cannot pass through it |

- **Resolution of `x`**, the same as `provenance::resolve_arg`: a literal in
  place (`tunable(40)`), a negated one (`tunable(-5)`), or a name whose
  binding chain ends in a literal (`tunable(radius)`). It marks that one
  literal term.
- **Placement**: anywhere an expression goes:
  - at the definition: `let r = tunable(40, 10, 80)`;
  - at the use: `draw_circle(tunable(cx), cy, 6, c)`;
  - as a statement naming a binding, which reads as a declaration:
    `tunable(radius, 10, 80)`. The result is discarded.
- **`rigid(expr)` on a computed expression** is useful (it is a barrier) and
  is allowed. **`tunable(expr)` on a computed expression** marks nothing and
  gets the lint warning `dm-hint-not-literal`: `` `tunable` marks a literal
  or a name bound to one; `a + b` is computed, so nothing is marked. Wrap the
  literal itself: `tunable(a) + b` ``.
- **Bounds must be literals**. Otherwise the lint warning reads
  `` `tunable` bounds must be literals so tools can read them without running
  the program ``. At runtime the bounds are evaluated and ignored.
- **`min > max`**: a lint error.
- **On a `state` initializer**: the lint warning
  `` a drag cannot move a running `state`'s initial value (hot reload keeps
  state); declare the start value with `config let` and initialise the state
  from it ``. This matches `config state`'s error text.
- **Row**: `NativeEffects::CONST`. The identity is a constant function, so
  static values, memo and the classifier treat it like any constant call. A
  dual passes through unchanged.
- **Shadowing**: a user function called `tunable` or `rigid` compiles to
  `Call`, not `BuiltinCall`, and is ignored by the hint reader. That is
  non-breaking by construction.

### Interaction with `config let`

A literal under both `config let` and `tunable` gets the lower weight
(tunable) and the range. `config let` keeps everything it does today
(`static_bindings`, sliders, `set_config_value`). The guide recommends:
**`config let` for top-level knobs, `tunable` for ranges and for literals
inside functions or calls, and `rigid` to forbid.**

### Names

Checked across the ecosystem (sample-apps, garden, examples, core-libs, the
prelude, `~/worlds-fair/ui/ptl`, `~/petal-creative`, `~/cheesecake/games`;
Rust native registrations in core, core-libs, garden and integrations):

| Name | `.ptl` files defining or calling it | Rust registrations | Verdict |
|---|---|---|---|
| `tunable` | 0 | 0 | chosen |
| `rigid` | 0 | 0 | chosen |
| `fixed` | 16 | builtin (`builtins/mod.rs:450`, number formatting) | taken |
| `freeze` / `frozen` | builtins (`builtins/mod.rs:533-534`) | | taken |
| `pin` / `pinned` | 1 / 2 | 0 / 1 | in use |
| `hold` / `held` | 20 / 22 | | in use |
| `anchor`, `locked`, `knob`, `nudge`, `tune` | 14, 12, 4, 4, 2 | | in use |
| `untunable`, `dm_fixed`, `steady` | 0 | 0 / 0 / 1 | rejected as awkward or taken |

`rigid` was chosen over `untunable` because it reads as a property of the
value ("this number is rigid"), and its barrier meaning ("the drag does not
bend this") follows from the name.

## Before and after

All "before" snippets run today (checked with `petal check` on the files, and
an extract with `petal run`). "After" is proposed.

**1. The common case: top-level knobs.** `examples/games/asteroids/app.ptl:50-57`.

```petal
// before (and after: unchanged)
config let start_lives   = 3
config let turn_rate     = 4.4      // rad/s
```

No change. `config let` is still the better spelling here: it is visible to
`static_bindings`, sliders and live set. Adding `tunable` buys only a range.

**2. A literal that must not move.** `server-monitoring/app.ptl:490-493`.

```petal
// before
let orb = time() * 1.1
draw_circle(cx: mark_x + cos(orb) * 11.0, cy: mark_y + sin(orb) * 11.0, radius: 2.6, color: ACCENT)
```
```petal
// after (proposed)
let orb = time() * rigid(1.1)
```

The writer adds one word. The reader learns "this rate is not a layout knob".
The plan's clock-coupling check would usually rank `1.1` low anyway. The
hint makes it certain and saves the extra run.

**3. A range on a helper constant.**

```petal
// before: server-monitoring/app.ptl:84-86
fn label_caps(s: string, x: num, y: num, c: record)
  draw_text(upper(s), pos: {x: x, y: y}, style: {size: T_MICRO, color: c, spacing: 1.4})
end
```
```petal
// after (proposed)
  draw_text(upper(s), pos: {x: x, y: y}, style: {size: T_MICRO, color: c, spacing: tunable(1.4, 0, 4)})
```

Where `config let` cannot reach, the hint can. One mistake becomes possible:
wrapping a computed value (`tunable(gap * 2)`). The lint catches it.

**4. A call-site literal, without a new binding.**

```petal
// before: server-monitoring/app.ptl:495
tx("ORBITAL", x: mark_x + 24.0, y: 21.0, size: T_LEAD, c: TEXT, spacing: 2.2)
// after (proposed): only the offset moves when the wordmark is dragged
tx("ORBITAL", x: mark_x + tunable(24.0), y: tunable(21.0), size: T_LEAD, c: TEXT, spacing: 2.2)
```

Without the hint, v2's collateral measurement already prefers `24.0` over
`PAD` and `12.0` (both shared by the ring). The hint is optional here, and
this pair shows that the heuristics carry most programs.

**5. Interaction with `state`.**

```petal
// before
state var n = 0
set n = n + 1
// after (proposed): lint warning dm-hint-state-init
state var n = tunable(0)
```

This is the case where the hint is wrong, and the lint says why and what to
write instead.

**Migration**: none needed. Nothing changes meaning, and nothing is renamed.

## Assessment

**Project goals.** Findings: this serves "goal-based program manipulation"
directly, and it is the "userland hints" item of the v2 task. Opinion:
distinctive, since few languages let a program annotate its own
differentiability for an editor. It is surface no other tool needs, which is
why it is a builtin and not syntax.

**Ease of writing.** A newcomer can ignore both, and programs run the same.
The names read as English. There are two ways to mark a top-level knob
(`config let` and `tunable`). The guide picks `config let` for top level. The
lint errors above cover misuse.

**Bug-freedom.** Identity at runtime cannot introduce runtime bugs. The risk
is a silently ignored hint (`tunable(a + b)`), and the lint makes that
visible. `petal check` can flag non-literal bounds statically. There is no
action at a distance beyond DM ranking.

**Performance.** One identity native call per use, `CONST`. At top level it
becomes a static value, so it is computed once. In a hot loop it costs about
one native dispatch. If that ever matters, the lowerer can turn the
`BuiltinCall` into a `Move` while keeping the IR term (a follow-up). No
analysis is defeated: the classifier sees a constant call, and memo sees a
`CONST` row.

**Live coding and hot reload.** Adding or removing a hint is a structural
edit, so it recompiles with state kept (call paths are unchanged, because
builtins have no call-site state). Editing a hinted literal is a value-only
patch as today. The literal is argument 0, not a different term.

**Introspection and manipulation.** Provenance v1 sees `tunable(40)` as a
`BuiltinCall`, a computed arg, so v1 DM would refuse it. M2 teaches
`provenance::resolve_arg` to look through `tunable`/`rigid` as identity, so
v1 keeps working. Autodiff passes through. `explain` and slicing show the
call as one more node.

**Implementation maintainability.** Touched: `builtins/` (two natives with
param declarations), `dm_v2/hints.rs` (the reader), `provenance.rs`
(identity look-through), `lint/` (three rules), `Builtins.md`,
`stdlib.json` regen, and the effect-row pin test in `builtins/mod.rs`. No
lexer, parser, IR, bytecode or type checker changes. The C bridge is not
affected.

**Domains.** Graphics and creative coding: helps most (layout and
animation constants). Games: helps (tuning ranges; `rigid` for physics
constants). UI apps: helps, through helper constants in widget code.
Dashboards: neutral. Web hosts: neutral (they gain the builtins with the
core). Config files: neutral, since `config let` remains the spelling.
Embedding hosts: neutral.

**Compatibility.** Zero breakage: no existing definitions or calls of either
name. A future user function of the same name shadows it harmlessly.

**Prior art.** Bret Victor-style live editors and Sketch-n-Sketch (Chugh et
al., 2016) infer which constants to change from a trace. Sketch-n-Sketch
adds *freeze* annotations (`!` on a number) that keep a constant from being
changed, and range annotations for sliders. Those are the same two concepts,
spelled as syntax. Processing's Tweak Mode makes every literal tunable, with
no control. Houdini and TouchDesigner parameters have ranges declared beside
them. The lesson from Sketch-n-Sketch is that freeze is used more than
anything else, which argues for having `rigid` from day one.

## Alternatives considered

- **Do nothing**: rely on the heuristics and the client's allow-list. That
  covers most drags (pairs 1 and 4), but not ranges or "never this".
- **Syntax** (`40!`, `40 in 10..80`): concise, but it costs a lexer and
  parser change, formatter and LSP work, and it is a new concept to read.
  Rejected for a feature whose audience is tools.
- **`config let x = 40 range 10..80`**: covers top level only. Function
  bodies and call sites stay unreachable.
- **Comments** (`// dm: rigid`): invisible to the IR, fragile under
  formatting. Rejected.
- **Smallest version**: `rigid` alone. This proposal includes `tunable` too,
  because ranges are the other half of the request.

## Implementation sketch

In M2 of the plan:
1. The natives (`tunable(x, min?, max?, step?)`, `rigid(x)`) with
   `declare_params`, as `CONST`.
2. `dm_v2/hints.rs::read_hints(program)`.
3. Identity look-through in `provenance::resolve_arg` and `alias_target`.
4. The spec-mode `rigid` replay in the native gate.
5. The lint rules.
6. Docs.

Tests are M2 acceptance items 5, 6 and 10.

## Open questions

1. The owner signs off on the names `tunable` and `rigid`.
2. Should `step` also drive the int-vs-float spelling (`step: 1` means stay
   an int)? Proposed: yes. Settled in M3 (plan §9, M3 deviation 13): the
   step only rounds the value, and the spelling follows the literal, so
   `step: 1` on an int literal keeps it an int, and on a float literal
   (`tunable(2.0, step: 1)`) keeps a float (`3.0`). Turning a float into an
   int would change what `/` does.
3. Should a module-level switch (`rigid_module()`) mark every literal in a
   library file rigid? Proposed: no. Library files are already ineligible by
   default (plan §2.5.1).
