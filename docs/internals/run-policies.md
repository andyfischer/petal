# Run policies

A run policy says which runtime layers are on for a run: the optimizer, the
static-value cache, memoized scopes and the frame gate. It is one value,
`RunPolicy` in [`core/src/policy.rs`](../../core/src/policy.rs), and the
combinations people ask for have names.

```sh
petal run --policy baseline app.ptl > a.txt   # every layer off
petal run --policy fast app.ptl > b.txt       # every layer on
diff a.txt b.txt                              # must print nothing
```

Every layer is an optimization, so none of them may change what a program
prints, draws or stores. That makes each one a test oracle as well as a
switch: run a program under two policies that differ in one layer, and any
difference in the output is a bug in that layer. Most of Petal's runtime
correctness testing is built on this.

This page is for people working on Petal itself. It covers the policies and
modifiers, what each layer is, which policy each entry point uses, how to set
one, how the tests use them, and how to add a layer.

- [The named policies](#the-named-policies)
- [Modifiers](#modifiers)
- [The layers](#the-layers)
- [What an `Env` does with the policy](#what-an-env-does-with-the-policy)
- [The default for each entry point](#the-default-for-each-entry-point)
- [Setting a policy](#setting-a-policy)
- [Policies as test oracles](#policies-as-test-oracles)
- [Adding a layer, a modifier or a name](#adding-a-layer-a-modifier-or-a-name)
- [What not to break](#what-not-to-break)

## The named policies

| Name | Optimizer | Static cache | Memo | Gate | For |
|---|---|---|---|---|---|
| `fast` | on | on | on | on | What a host that runs a script every frame uses. The default of an `Env`. |
| `baseline` | off | off | off | off | The oracle. Every frame runs, every call runs, and the bytecode is the clone-and-alloc lowering. Every other policy must reproduce it. |
| `explain` | on, keeping every traced instruction | off | off | off | Tools that read what each instruction computed. Nothing is skipped or replayed. |
| `replay` | on | on | on | off | `fast` without the gate: every frame runs, so the memo and the static cache are exercised on every frame and not only on the frames the gate lets through. |

These are the constants `RunPolicy::FAST`, `BASELINE`, `EXPLAIN` and `REPLAY`,
and the `NAMED` table in `policy.rs` maps the names to them.

## Modifiers

A name may be followed by any number of modifiers, each `+` or `-` and a
word. They apply left to right.

| Modifier | Switches | Field |
|---|---|---|
| `+memo` / `-memo` | memoized scopes | `RunPolicy::memo` |
| `+gate` / `-gate` | the frame gate | `RunPolicy::gate` |
| `+opt` / `-opt` | every optimizer pass, and the static cache with them | `RunPolicy::opts` |
| `+static` / `-static` | the static-value cache alone | `OptFlags::static_cache` |
| `+fnstatic` / `-fnstatic` | the values the static cache keeps for function bodies | `OptFlags::static_in_functions` |

So `fast-memo` is the gate without the memo, `baseline+gate` is the gate over
unoptimized code, `replay-opt` is the memo over unoptimized code,
`fast-static` is the default without the static cache, and `replay-fnstatic`
is the cache for top-level values only.

Three details of `RunPolicy::parse`:

- `-opt` sets `OptFlags::none()`, which turns the static cache off too, and
  `+opt` sets `OptFlags::default_on()`, which turns it on. Write `-opt+static`
  or `baseline+static` for the cache over unoptimized code.
- `+opt` and `-opt` keep the preservation guards of the policy they modify,
  and `+opt` leaves the static cache off when those guards are set. So
  `explain-opt` still records every traced instruction, and `explain-opt+opt`
  is `explain` again.
- `fnstatic` does nothing unless the static cache is on. It is on in every
  named policy, so that `baseline+static` is the whole cache.

`RunPolicy::name()` is the inverse: it prints a policy as its nearest name
plus modifiers (`baseline+memo+gate`), and returns `None` for an optimizer
setting that no name spells, such as a single pass switched on in a test.
`petal bench` prints the name of each side it measures.

A name or modifier that does not parse is an error that lists what was
expected:

```text
$ petal run --policy quick app.ptl
unknown run policy 'quick' (expected fast, baseline, explain, replay)
$ petal run --policy fast+speed app.ptl
unknown run policy modifier '+speed' in 'fast+speed' (expected gate, memo, opt, static or fnstatic)
```

## The layers

A policy has two halves. `RunPolicy::opts` is an
[`OptFlags`](../../core/src/backend/mod.rs): how the program is lowered to
bytecode. It is the key of an `Env`'s bytecode cache, so changing it lowers
each program again on its next run. `memo` and `gate` change what a run does
and not what it was compiled to, so changing them costs nothing.

**The optimizer.** Five passes over the lowered bytecode, each with a flag of
its own in `OptFlags`: three that turn a copy of a collection into a write in
place (`in_place_mutation`, `in_place_straight_line`, `in_place_cells`),
`copy_propagation`, and `match_dispatch`. A policy name switches them
together. A test can switch one alone with `RunPolicy::with_opts`. What each
pass does is in
[performance.md, What the optimizer does](../dev/performance.md#what-the-optimizer-does).

**The static-value cache.** A top-level value that depends on no dynamic data
is built by the first run and kept for the runs after it, and so is a value
that a function body builds from nothing of the call. The cache adds a skip
instruction in front of the code that builds such a value and a store behind
it. No instruction is moved or removed, so with the cache off the code is the
same code without those two instructions. It lives in `OptFlags` because it
changes the lowering. The user's view is in
[features/static-values.md](../features/static-values.md), and the
classifier and the cache are in [static-values.md](static-values.md).

**Memoized scopes.** A call to a user function whose arguments, captures and
recorded reads are what they were last time is replayed from its record
instead of run. A call whose result the caller mutates in place is never
memoized (`Inst::Call::no_memo`). See
[memo-scopes.md](../dev/memo-scopes.md).

**The frame gate.** A host that runs a script every frame skips a frame when
nothing the last run read has changed, and keeps the output it has. See
[frame-gate.md](../dev/frame-gate.md). The gate is only a policy here: the
runtime answers `Env::run_needed` whatever the policy says, and the host's
frame driver reads `RunPolicy::gate` to decide whether to act on the answer.
A program that is run once, or run by a loop that never asks `run_needed`, is
not gated under any policy.

**Tracing.** Two more flags in `OptFlags` are guards and not optimizations.
`preserve_observations` stops copy propagation from deleting a dead move
whose term is named, so the observation buffer still reports that binding.
`preserve_trace` stops it from deleting any instruction that carries an
origin term, so the execution trace records a value for every term. The
trace is what `explain`, provenance and
[direct manipulation](../features/direct-manipulation.md) read. The `explain`
policy sets both. No modifier switches them, because an `Env` sets them by
itself when a tool turns the trace or the observation buffer on (next
section).

## What an `Env` does with the policy

The policy is what was asked for. What a run uses also depends on which debug
facilities are on, and three methods in `core/src/env/` give the answer:

- `effective_opt_flags` (`env/mod.rs`) starts from `policy.opts`, sets
  `preserve_observations` when the observation buffer or the trace is
  enabled, sets `preserve_trace` when the trace is enabled, and turns the
  static cache off when the trace is preserved or the static audit is
  watching. Observations do not turn the cache off: a skipped segment reports
  what it observed when it last ran.
- `memo_enabled` (`env/run.rs`) is `policy.memo` unless the trace is enabled.
  A replayed scope would leave no instructions in the trace.
- `static_cache_enabled` (`env/statics.rs`) is the static cache flag of
  `effective_opt_flags`.

So a tool that enables the trace on a `fast` `Env` gets the behaviour of
`explain` for the optimizer, the cache and the memo without naming it. Setting
`explain` is for a run that should be lowered that way from the start.

Because the effective flags are the bytecode cache key, turning a facility on
lowers the program again on its next run. Change the policy, or a facility,
between runs and never inside one: lowering again in the middle of a run
would leave live frames pointing into code that no longer exists. A stack's
kept static values do not survive a new lowering.

## The default for each entry point

`Env::new()` starts with `RunPolicy::from_env()`: `PETAL_POLICY` if it is set
and parses, otherwise `baseline` if `PETAL_OPT` is `off`, `none` or `0`,
otherwise `fast`. Everything below starts from that.

| Entry point | Policy | Where |
|---|---|---|
| `petal run <file>` | `fast-memo-static` | `handle_run` in [`core/src/cli/handlers.rs`](../../core/src/cli/handlers.rs) |
| `petal run --runs <n>` | the `Env` default (`fast`) | the same function |
| `petal run --policy <name>`, or with `PETAL_POLICY` set | the named policy, unchanged | the same function |
| `petal bench` | two sides that share the `Env` default and differ in the optimizer alone | `core/src/cli/bench.rs` |
| `petal show-bytecode` | the `Env` default's optimizer flags, without the static cache unless `PETAL_POLICY` is set | `handle_show_bytecode` |
| `petal-ui` (`FrameCore`, `Headless`), `petal-ui-run`, `bench_panel` | the `Env` default | `core-libs/petal-ui/src/frame_core.rs` |
| `petal-desktop-sdl` | the `Env` default | `integrations/petal-desktop-sdl/src/game_loop.rs` |
| The C bridge | the `Env` default | `integrations/petal-c-bridge/src/vm.rs` |
| A Rust test that calls `Env::new()` | the `Env` default, so it follows `PETAL_POLICY` | |

`petal run` turns the memo and the static cache off because it runs the
script once and exits. A memo record made by that run is never replayed, and
recording is expensive for a recursive program. A static value kept for the
next run is kept for nothing, and classifying the program to decide what to
keep costs time at every start. `--runs` is a host's frame loop, where both
pay their way, so it keeps them. Naming any policy, by flag or by
environment variable, switches this adjustment off, so `petal run --policy
fast` really is `fast`. `petal run` never consults the gate, with or without
`--runs`.

`petal help run` says that the default is `fast-memo`. The code is the
authority: the static cache is off as well.

`petal bench` measures with the optimizer on and again with it off, and keeps
the rest of the policy the same on both sides, which is why its second side
prints as `baseline+memo+gate`. If the environment turns the optimizer off
(`PETAL_OPT=off`, `PETAL_POLICY=baseline`), the first side still has it on.

Which host acts on the gate, and when it does not (the SDL loop while the
timeline records, a host whose `Host::frame_gating()` says no), is in
[frame-gate.md, Where it is wired](../dev/frame-gate.md#where-it-is-wired).
The same table for the memo is in
[memo-scopes.md, Where it is wired](../dev/memo-scopes.md#where-it-is-wired).

## Setting a policy

| How | Scope |
|---|---|
| `petal run --policy <name>` | that run. The flag wins over the environment. |
| `petal run --no-opt` | the same as `--policy baseline` |
| `PETAL_POLICY=<name>` | every `Env` in the process: every command, every host, every test. A value that does not parse is reported on stderr and ignored. |
| `PETAL_OPT=off` (or `none`, `0`) | `baseline`, read only when `PETAL_POLICY` is not set. The older spelling. |
| `Env::set_policy(RunPolicy)` | that `Env`, from its next run. `Env::policy()` reads it back. |
| `Env::set_static_cache(bool)` | the static cache flag of that `Env`'s policy |
| `FrameCore::set_policy`, `Headless::set_policy` (petal-ui) | the `Env` of that UI. There is no second copy of the policy. |
| `petal-ui-run --policy <name>`, `bench_panel --policy <name>` | that run. `--no-gate` and `--no-memo` then switch one layer off in whichever policy is in effect. |
| `pb_vm_set_memo`, `pb_vm_set_static_cache` (C bridge) | one layer of that VM's policy ([embedding-c.md](../usage/embedding-c.md)) |

In Rust a policy is built from a constant and the `with_*` methods, or parsed
from the same text the flag takes:

```rust
use petal::policy::RunPolicy;

env.set_policy(RunPolicy::REPLAY.with_static_cache(false));
env.set_policy(RunPolicy::parse("replay-static")?);   // the same policy
```

## Policies as test oracles

The rule every differential rests on: two runs of one program under two
policies must agree on the value, the output and the state they leave. The
tests pick pairs that differ in one layer, so a failure names the layer.

| Pair | Layer under test | Where |
|---|---|---|
| `baseline` against `fast` | everything, the optimizer first | `assert_parity` and `assert_stateful_parity` in [`core/src/backend/bytecode/tests.rs`](../../core/src/backend/bytecode/tests.rs); `--no-opt` against the default in [`tools/test-examples.ts`](../../tools/test-examples.ts) |
| `baseline` against `baseline` with one pass on | one optimizer pass | the fuzzers `fuzz.rs`, `cell_fuzz.rs` and `enum_all_fuzz.rs` in `core/src/backend/bytecode/` |
| `replay` against `replay-static` | the static cache | `--runs 2` in `tools/test-examples.ts`; `static_fuzz.rs` and `static_fn_fuzz.rs`, which also pair `replay-memo`, `fast` and `baseline` with the cache on and off |
| `replay` against `replay-fnstatic` | the values kept for function bodies | `core/tests/static_in_functions.rs`, `core-libs/petal-ui/tests/static_in_functions.rs` |
| `replay` against `replay-memo` | the memo | `core-libs/petal-ui/tests/memo.rs` |
| `fast` against `replay` | the gate | `core-libs/petal-ui/tests/gating.rs` |
| `baseline` against `fast-memo`, `replay` and `fast` | each shipped policy, over UI code from another repository | [`tools/oracle-external.ts`](../../tools/oracle-external.ts), driving `petal-ui-run` |

Things to know when you write or read one of these:

- **Compare rendered values.** Heap ids are not comparable across two runs,
  so the helpers compare display strings, the print buffer, and the state
  map as sorted JSON.
- **Both failing is agreement.** `assert_parity` accepts two errors without
  comparing their messages, and fails when one side succeeds and the other
  does not.
- **The cache and the memo need more than one run.** A value is kept, or a
  record replayed, on the run after the one that made it. Drive one stack
  through several runs: `assert_stateful_parity`, or `petal run --runs 2`
  from a script.
- **The gate needs a frame driver.** `Env::run` does not gate. A gate
  differential runs through `petal-ui`'s `FrameCore` or `petal-ui-run`.
  A skipped frame reports no prints, so compare draw commands and state, as
  `tools/oracle-external.ts` does.
- **Run the observed variant too.** The fuzzers run their pairs again with
  the observation buffer on, and the cache fuzzers compare what a host would
  read from it after each run.
- **Suspect both sides.** `baseline` is the oracle for the optimizer, but in
  a cache differential the bug has been on the side with the cache off.

`baseline` against `fast` only shows that two policies agree. That they agree
on the right answer is held by the golden corpus in `test/example-golden/`
and the script cases under `test/`. How to run all of these, the soak
variables of the fuzzers and what the suite costs are in
[testing.md](../dev/testing.md).

## Adding a layer, a modifier or a name

**A layer that changes the lowering** (a new optimizer pass):

1. Add a field to `OptFlags` in `core/src/backend/mod.rs` and set it in
   `none()`, `all()` and `default_on()`. `default_on` is written out field by
   field so that a pass added to `all()` does not become a default before it
   has been held to the differentials.
2. Gate the pass on the flag where the bytecode is lowered
   (`core/src/backend/bytecode/`).
3. Add a differential: `baseline` against
   `RunPolicy::BASELINE.with_opts(OptFlags { your_pass: true, ..OptFlags::none() })`,
   in `tests.rs` and in the configuration table of a fuzzer. The existing
   `ROUTE_A_ONLY`, `CELLS_ONLY` and `ESCAPE_ONLY` constants are the pattern.
4. If the pass removes or skips instructions, decide what it does under
   `preserve_observations` and `preserve_trace`, and test it with the
   observation buffer on.

**A layer that changes what a run does** (like the memo): add a `bool` to
`RunPolicy`, set it in all four constants, add a `with_*` method, and read it
in the `Env` through a method that also accounts for the trace, as
`memo_enabled` does.

**A modifier for either**, in `RunPolicy::parse` and `RunPolicy::name`:

1. Add an arm to the `match` in `parse`, and the word to the "expected …"
   error text beneath it.
2. Add the field to the list `name()` walks, so that a modified policy prints
   back in the syntax `parse` accepts. For an `OptFlags` field, also
   normalize it in the filter that picks the base name, as `static_cache` and
   `static_in_functions` are.
3. Add tests beside the ones in `policy.rs`: the modifier switches its field
   and nothing else, and `parse(p.name())` is `p`. Update
   `bad_specs_say_what_was_expected`.
4. Update the `--policy` text in `core/src/cli/help.rs` and the tables on
   this page.

**A named policy**: add the constant and a row in `NAMED`.
`RunPolicy::names()`, the error text and `every_name_parses_to_itself_and_prints_back`
follow from the table. `name()` prefers names in table order.

## What not to break

- **Output is the same under every policy.** A change that makes one policy
  print, draw or store something another does not is a bug, whichever side
  looks right.
- **`baseline` is everything off.** It is the oracle and `--no-opt` and
  `PETAL_OPT=off` mean it. A new layer is off in `baseline`.
- **The gate is advisory.** The runtime must answer `run_needed` under every
  policy. Only a frame driver reads `RunPolicy::gate`.
- **Preservation survives `+opt` and `-opt`.** Switching the optimizer does
  not change what a tool asked to keep (`OptFlags::preserving`).
- **A name round-trips.** `RunPolicy::parse(&p.name().unwrap())` is `p` for
  every policy a name can spell.
- **`OptFlags` stays the whole key of the bytecode cache.** Anything that
  changes the lowered code belongs in it, or two settings will share code
  compiled for one of them.
- **An unparseable `PETAL_POLICY` must not fail the process.** `from_env`
  runs in every `Env::new`, so it reports and falls back to `fast`.

## See also

- [`core/src/policy.rs`](../../core/src/policy.rs): the type, the parser and
  its tests.
- [memo-scopes.md](../dev/memo-scopes.md), [frame-gate.md](../dev/frame-gate.md),
  [performance.md](../dev/performance.md) and [static-values.md](static-values.md):
  the layers.
- [CLI reference, `run`](../CLI.md#run--execute-a-program): the flags.
- [testing.md](../dev/testing.md): running the differentials.
