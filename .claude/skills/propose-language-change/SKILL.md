---
name: propose-language-change
description: Write a researched proposal for a change to the Petal language or its core design (syntax, semantics, type system, state model, effects, module system, builtins, IR/VM contracts). Use whenever a language or core-design change is being proposed, considered or compared, before any implementation starts.
---

# Propose a language change

Use this whenever a change to the Petal language or the core design is on the
table: new or changed syntax, semantics, the type checker, `state` / effects /
memoization rules, the module system, the builtin surface, or a contract of
the IR or VM that programs or hosts can observe.

The output is a proposal, not an implementation. Do not change `core/` while
running this skill. Throwaway experiments belong in the scratchpad or a
worktree.

A proposal is worth reading only if it is grounded in what the code and the
corpus actually say. Every claim about "how programs are written today" needs
a file path behind it, and every "after" snippet must be honest about what it
costs.

## 1. Pin down the change

State in two or three sentences what changes and what does not. If the request
is vague ("better error handling"), write down the one or two concrete designs
you will evaluate and carry them all the way through as alternatives. Ask the
user only when the candidates differ in kind and you cannot tell which they
mean.

Then read what already exists:

- `README.md`: the goals (easy to use, live coding, introspection and
  debuggability, goal-based program manipulation) and the design decisions
  that follow from them.
- `docs/language-guide.md`, `docs/syntax/overview.md`,
  `docs/writing-petal-guide.md`: the current behaviour of the area.
- `docs/dev/Architecture.md`, and any `docs/dev/*-plan.md` or `docs/tasks/*`
  that touches the area. An earlier plan may have already decided or rejected
  this; say so if it did.
- The implementation: `core/src/` (lexer, parser, compiler, `backend/bytecode/`,
  builtins), plus `core-libs/` and `core-runtime/` when the change reaches them.

## 2. Find where it matters in real code

Search the Petal corpus for the places the change would touch:

- `examples/` (console, games, productivity, dashboards, ui, custom-apps)
- `garden/` (its `.ptl` scripts and `init.ptl`)
- `core-libs/`, `core-runtime/`, `test/`
- any other Petal-using projects named in `CLAUDE.md` or `CLAUDE.local.md`

Record how often the relevant pattern occurs and where, with `file:line`
references. Pick three to six sites that represent the range: the common
case, the worst case the change is meant to fix, and at least one where the
change makes things no better or worse. If the pattern barely occurs, that is
a finding; report it.

## 3. Before and after

For each chosen site, show the code as it is written today next to the code
as it would be written with the change.

- "Before" is real code, quoted from the corpus with its path. Trim it, but do
  not tidy it into a strawman.
- Verify that every "before" snippet runs today (`TestSnippet` / `CheckSnippet`
  from the `petal-tools` MCP server, or `./tools/run-petal.ts run`).
- "After" uses the proposed design exactly as specified. It will not run yet;
  label it as proposed. If a prototype is cheap, build one in a worktree and
  say that the snippet was run against it.
- Under each pair, say in a line or two what changed for the writer and the
  reader: lines, names to invent, concepts to know, mistakes no longer possible,
  mistakes newly possible.
- Include one pair where the old form is still the better choice, if there is
  one, and one showing how the feature interacts with `state`, hot reload or
  another existing feature it is likely to collide with.

Also show what migration looks like for existing code: unchanged, mechanically
rewritable (`petal fix-imports`-style, `lint` fix, `apply-change`), or manual.

## 4. Research each perspective

Answer every question below with evidence: code read, a measurement, a count
from the corpus, or a named precedent. "No effect" is an acceptable answer when
you can say why. Skip none; add any that the specific change raises.

For a substantial change, run the perspectives as parallel subagents, each
given the pinned-down design from step 1 and the sites from step 2, and each
asked for its findings and its honest opinion, including whether it would
reject the change. Do not hand them a conclusion to confirm. For a small
change, work through them yourself.

**Project goals**
- Which of the README goals does this advance, and how directly?
- Does it work against any of them? Does it fit the design decisions (dataflow
  with immutable values, limited effects, first-class `state`, optional types)?
- Is this the kind of thing Petal should be distinctive at, or is it generic
  language surface?

**Ease of writing**
- What does a newcomer have to learn that they did not before? Does it match
  what someone coming from JavaScript, Python or Lua would guess?
- How does it read six months later? Is there now more than one way to do the
  same thing, and which should the guide recommend?
- What do the error messages look like when it is used wrong?

**Bug-freedom**
- Which classes of bug does it prevent, and which become possible?
- Can the type checker, `lint` and `check` see misuse statically? Does it
  create silent failures, surprising defaults or action at a distance?
- Does it stay testable under the differential oracle (optimizer on vs
  `--no-opt`), the golden corpus, the small-scope check and the Kani proofs?

**Performance**
- Cost at compile time and at run time, on the hot path and when unused.
- Effect on the optimizer: in-place mutation, memo scopes, the frame gate,
  copies and allocations. Does it defeat an analysis that currently holds?
- Name the benchmark that would show it (`petal bench`, `tools/bench-opts.ts`,
  `bench_panel`; see `docs/dev/performance.md`). Measure today's baseline for
  the affected sites where that is cheap.

**Live coding and hot reload**
- What happens when code using the feature is edited while running? How are
  `state` slots and call paths affected?
- Does the reloaded program still equal a fresh compile (`docs/features/hot-reload.md`)?

**Introspection, debugging and program manipulation**
- Does provenance, `explain`, slicing, time travel and autodiff still see
  through it? Does the term graph stay analyzable?
- Can goal-based editing, `propose-edit`, source preservation and direct
  manipulation still trace a value back to source and rewrite it?

**Maintainability of the implementation**
- Which stages change (lexer, parser, AST, compiler, IR, bytecode, VM, type
  checker, LSP, formatter, linter, MCP tools, editor support) and roughly how
  much in each?
- Does it add a special case or remove one? What invariants does it add that
  future work must keep?
- What does it cost in docs, tests, golden files and the embedding APIs
  (`docs/usage/ffi.md`, the C bridge)?

**Domains**
For each, say whether the change helps, is neutral or gets in the way, with a
corpus example where one exists:
- Graphics and creative coding (draw calls, shaders, `to_glsl`, animation)
- Games (per-frame loops, entity state, input, the handle FFI)
- Productivity and UI apps (widgets, layout, text, forms)
- Dashboards and data (queries, transforms, charts)
- Web (`integrations/petal-web-canvas`, `petal-web-html`)
- Editor scripting and config files (Garden, `.ptl` as configuration)
- Embedding hosts (Rust, C/C++)

**Compatibility and migration**
- Does existing code break? How much of the corpus, counted?
- Can it ship behind a warning first? Is there a mechanical migration?
- Does it close off a future direction, or depend on unfinished work
  (see `docs/tasks/`)?

**Prior art**
- How do other languages handle this, especially the ones in the README's
  related-work list? What went wrong for them?

**Alternatives**
- The do-nothing option, a library or builtin instead of syntax, a lint or
  tooling fix instead of a language change, and the smallest version of the
  feature that still delivers most of the benefit.

## 5. Write the proposal

Write it to `docs/proposals/<slug>.md` unless the user names another place or
asks for it inline. Lead with the conclusion.

```
# Proposal: <name>

Status: proposed, <date>

## Summary
What changes, in a few sentences, and the recommendation:
adopt / adopt a smaller version / defer / reject.

## Motivation
The problem, with counts and file references from the corpus.

## Design
The exact syntax and semantics, including edge cases and error messages.

## Before and after
The pairs from step 3.

## Assessment
One subsection per perspective from step 4. Findings first, then opinion.
Where perspectives disagree, say so and leave the disagreement visible.

## Alternatives considered

## Migration and compatibility

## Implementation sketch
Stages touched, rough size, the order to build it in, how it will be tested.

## Open questions
What is still undecided and who has to decide it.
```

Keep findings and opinions distinguishable: a reader should be able to tell
what was measured or read from what is judgement. Do not let the recommendation
soften the costs; a proposal that concludes "reject" or "defer" is a successful
use of this skill.

When reporting back, give the recommendation, the two or three findings that
drove it, the strongest argument against it, and the open questions.
