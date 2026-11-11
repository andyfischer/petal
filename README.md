# Petal

Petal is a programming language for creative coding.

It's an experimental
project that supports new ways of editing code, including: live coding,
coding by direct manipulation, programs as documents, immediate-mode support,
and more.

Most languages treat a running program as a black box. In comparison, a running Petal program
is a living thing that can be poked, inspected, tested and manipulated.

## Goals

At a high level, the project's goals are:

 - Easy to use, batteries-included for creative applications.
 - Supports live coding.
 - Supports coding by direct manipulation and other outcome-based coding.
 - High introspection and debuggability.
 - Pragmatic instead of principled.

## Foundational language decisions

To achieve those goals, the language design has a few core decisions:

### Pure functional semantics

The language embraces pure functional semantics in order to have safe replayability,
where the runtime knows how to re-run a program in a way that has no outside effects.

This means:
 - Most functions are pure.
 - The runtime understands which functions have effects. Functions with effects do not return values.
 - Certain functions like `random()` are special-cased and can be replayed deterministically.
 - Values are immutable.
 - Most code uses dataflow semantics, without circular references.
 - The language does pragmatically allow `var` which is a standard variable slot.

Most programs can be traced as a dataflow diagram, which allows for very high program
introspection.

#### References and research

 - Wadge and Ashcroft, *Lucid, the Dataflow Programming Language* (1985)
 - Halbwachs, Caspi, Raymond and Pilaud, *The synchronous data flow programming language LUSTRE* (Proceedings of the IEEE, 1991)
 - Elliott and Hudak, [*Functional Reactive Animation*](http://conal.net/papers/icfp97/) (ICFP 1997)
 - Czaplicki and Chong, *Asynchronous Functional Reactive Programming for GUIs* (PLDI 2013), the Elm paper

### First-class state declarations

The language has a first-class way of declaring persisted state, using the `state` keyword.
State values persist across time. This is a foundational feature which enables a lot
of interesting editing flows, listed in more detail below.

#### References and research

 - React Hooks ([useState](https://overreacted.io/why-do-hooks-rely-on-call-order/))
 - Jetpack Compose's [positional memoization](https://newsletter.jorgecastillo.dev/p/positional-memoization-in-jetpack)

## Major features

Building on those core design choices, here are some of the end-to-end features that the
language enables:

### Immediate-mode coding

Immediate-mode is a term coined in game development for writing UIs (also called IMGUI). In this
approach, the source code looks like a series of imperative function calls, but behind the
scenes, those calls produce a retained structure that is automatically managed. So instead
of the author thinking about creating and deleting a tree object to manage a GUI, the
author just writes calls, and the runtime figures out what structures to create and destroy
based on that.

Petal has really good support for immediate-mode, supported by the `state` keyword (which enables
nested calls to have preserved data and stored handles), and also by the smart runtime which
helps reconcile calls and automatically do diff detection.

#### References and research

 - Casey Muratori, [*Immediate-Mode Graphical User Interfaces*](https://caseymuratori.com/blog_0001) (2005), the talk that named IMGUI
 - [Dear ImGui](https://github.com/ocornut/imgui), the most widely used immediate-mode UI library

### State management

This section has a few features that are all related to state management. The runtime understands
which program values should be persisted (with the `state` keyword). With that ability,
we can build features that transfer state across programs, or freeze and rewind state, or duplicate
state into multiple runs.

#### Live code editing

With live code editing, you can modify a program and see the update appear immediately as it's running.

This works thanks to `state` declarations.
When a code change is
detected, the runtime will extract existing state values, compile the new code, and then
insert the preserved state into the new program (in a best-effort way). As long as there
aren't any drastic structural code changes, this approach works well at preserving the
user's existing session across a code change.

Immediate-mode coding is also helpful for supporting live coding, since programs don't
need to worry about the lifecycle of initializing or tearing down resources. They
just declare what should happen and the runtime takes care of the resources.

##### References and research

 - Tanimoto, *VIVA: A Visual Language for Image Processing* (1990), the source of the "levels of liveness"
 - Bret Victor, [*Learnable Programming*](https://worrydream.com/LearnableProgramming/) (2012)
 - McDirmid, *Living It Up with a Live Programming Language* (OOPSLA 2007)
 - Rein, Ramson, Lincke, Hirschfeld and Pape, [*Exploratory and Live, Programming and Coding*](https://arxiv.org/abs/1807.08578) (The Art, Science, and Engineering of Programming, 2019), a survey of liveness across the research
 - [React Fast Refresh](https://reactnative.dev/docs/fast-refresh)

#### Time travel debugging

With time-travel debugging, the runtime can take snapshots of old program states, and these can be
restored and replayed later. This is also thanks to `state` declarations and immediate-mode coding.

##### References and research

 - Bil Lewis, [*Debugging Backwards in Time*](https://arxiv.org/abs/cs/0310016) (2003)
 - Elm's [time-traveling debugger](https://elm-lang.org/news/time-travel-made-easy) (2014)
 - Dan Abramov, [*Live React: Hot Reloading with Time Travel*](https://www.youtube.com/watch?v=xsSnOQynTHs) (2015)

#### Speculative execution

This feature is enabled by the same state management.
With speculative execution, we can safely re-run the same program, maybe with slight variations,
to explore or predict something. The system knows how to freeze and duplicate state,
so the speculative runs are each isolated.

##### References and research

 - Warth, Ohshima, Kaehler and Kay, [*Worlds: Controlling the Scope of Side Effects*](https://tinlizzie.org/VPRIPapers/tr2011001_final_worlds.pdf) (ECOOP 2011)
 - Harris, Marlow, Peyton Jones and Herlihy, *Composable Memory Transactions* (PPoPP 2005), whose undo-log approach Petal's speculative execution resembles
 - Horowitz, Blinn and Heer, *Declarative Dragging* (UIST '26), which renders each candidate state speculatively

### Outcome-based programming

Outcome-based programming is an umbrella term for a few strategies for modifying
a program's code based on its output.

Usually when modifying code, the author works directly with the source code, and they have to
use their imagination and a mental model to predict how their code change will
affect the program's eventual behavior.

With outcome-based programming, we can have edit operations that use the program's actual observed output
as a reference point. And based on that, the author can express 'desired' or 'goal' outcomes.
The runtime can then work backwards to resolve those goals against the program's structure,
and figure out a code modification that satisfies them.

It's not a foolproof technique. There
are a lot of situations where solving a goal might be ambiguous or unsolvable. In most
cases there are several possible solutions, so it becomes a back-and-forth process where the author
needs to give restrictions on which part of the program is eligible to be modified. But
it generally works pretty well, and often has less cognitive load than traditional programming.

Here are some examples:

#### Programming by direct manipulation

In this feature, the author interacts with the running program, often by clicking and dragging
objects, and the runtime translates the drag gesture into a change to the source code.

For example, dragging a circle on the canvas rewrites the numbers passed to the
`draw_circle` call that drew it.
Only literal values change; the rest of the file keeps its formatting
and comments.

The *Declarative Dragging* paper showed that this needs no differentiation
built into the language: re-running the code and watching what moves is enough.

##### References and research

 - Bret Victor, [*Inventing on Principle*](https://vimeo.com/906418692) (2012)
 - Horowitz, Blinn and Heer, *Declarative Dragging* (UIST '26)
 - Chugh, Hempel, Spradlin and Albers, [*Programmatic and Direct Manipulation, Together at Last*](https://arxiv.org/abs/1507.02988) (PLDI 2016), the Sketch-n-Sketch paper
 - Shneiderman, *Direct Manipulation: A Step Beyond Programming Languages* (IEEE Computer, 1983)

#### How goals get solved

Petal works a goal back to the source with three tools:

 - **Algebraic inversion.** When the value runs through a chain of `+ - * /`, the
   literal that makes it hit the goal can be solved for exactly, with no iteration.
 - **Measured sensitivities and Gauss-Newton steps.** In general, Petal re-runs the
   code with each candidate literal nudged, measures how the output moved (finite
   differences), and takes damped Gauss-Newton steps toward the goal, re-running to
   verify each one. Each measurement also says how far to trust it: smooth, a kink,
   a branch that flipped, a step. Scripts get the same machinery through the
   `autodiff` package (`deriv`, `jacobian`, `solve_step`, `solve`).
 - **Speculative search.** Every probe runs speculatively: its `state` writes and
   drawing are put back afterwards, so a whole view or simulation step can be tried
   as a candidate without disturbing the running program.

Exact derivatives are planned to come back as a symbolic analysis of the program's
term graph, with differences as the fallback for what it can't differentiate.

##### References and research

 - Spreadsheet goal seek and `scipy.optimize`, which default to finite differences
 - Nocedal and Wright, *Numerical Optimization* (Springer, 2006), for Gauss-Newton and damping
 - Baydin, Pearlmutter, Radul and Siskind, [*Automatic Differentiation in Machine Learning: a Survey*](https://arxiv.org/abs/1502.05767) (JMLR 2018), for the symbolic route

### Provenance

Provenance means looking at a value and answering the question, "where did this value come from?".
When a program is represented as a dataflow graph, it's straightforward to find an output's
provenance. This helps with introspection and debuggability.

#### References and research

 - Ko and Myers, *Designing the Whyline: A Debugging Interface for Asking Questions about Program Behavior* (CHI 2004), which answers "why did" and "why didn't" questions about a run
 - Weiser, *Program Slicing* (ICSE 1981), on finding the parts of a program that a value depends on
 - Perera, Acar, Cheney and Levy, *Functional Programs that Explain Their Work* (ICFP 2012)
 - Buneman, Khanna and Tan, *Why and Where: A Characterization of Data Provenance* (ICDT 2001)

## Project status

This project is in an early, experimental phase. Large backwards-incompatible changes are still happening. Stability not guaranteed.

## Install

Install the `petal` CLI as a prebuilt binary (macOS Apple Silicon or Intel,
Linux x86_64 or arm64):

```bash
curl -fsSL https://petal-lang.org/install.sh | sh
```

This puts `petal` in `~/.petal/bin` and adds it to your PATH. It needs no
`sudo` and no dependencies. To uninstall:

```bash
curl -fsSL https://petal-lang.org/uninstall.sh | sh
```

## Build from source

```bash
# Build the compiler
make build

# Hello world
cli/target/debug/petal run -e 'print("hello, world!")'

# Run an example
cli/target/debug/petal run examples/console/fizzbuzz.ptl
```

For the full list of developer commands, see [Developer Scripts & Commands](docs/dev/scripts.md).

## Repository layout

| Directory | Description |
|-----------|-------------|
| [`core/`](core/) | The core language implementation: lexer, parser, compiler, IR, bytecode VM |
| [`cli/`](cli/) | The `petal` command-line tool |
| [`core-libs/`](core-libs/) | Standard native libraries for a few tasks including UI, asynchronous data, and package management |
| [`core-runtime/`](core-runtime/README.md) | Standard libraries written in Petal |
| [`integrations/`](integrations/) | A few native hosts for using Petal in real programs, including: SDL desktop, web HTML, web canvas, and the C bridge |
| [`garden/`](garden/README.md) | Text editor and IDE built with and for Petal |
| [`examples/`](examples/README.md) | Collection of runnable examples |
| [`docs/`](docs/README.md) | Documentation |
| [`editor-support/`](editor-support/README.md) | Files for IDEs, syntax definitions, etc. |
| [`tools/`](tools/) | Scripts and tooling for development |
| [`test/`](test/README.md) | Test suite |

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/Getting_Started.md) | Install the CLI, run your first program, and find what to read next |
| [Language Guide](docs/language-guide.md) | The full language reference |
| [Inline State](docs/features/inline-state.md) | The `state` keyword: persistent values declared where they are used |
| [Hot Reload](docs/features/hot-reload.md) | What reloading a running program keeps and costs |
| [Builtins Reference](docs/Builtins.md) | Every built-in function |
| [CLI Reference](docs/CLI.md) | Every `petal` subcommand and flag |
| [Testing](docs/testing.md) | Test blocks, expectations and `petal test` |
| [Module System](docs/module-system.md) | `import` and module resolution |
| [Packages](docs/packages.md) | `petal.toml`, dependencies, the lockfile and the package commands |
| [Architecture](docs/dev/Architecture.md) | How the implementation works |
| [Changelog](CHANGELOG.md) | What each release contains |

The [documentation index](docs/README.md) lists everything, grouped into
syntax, features, usage and internals.

## License

Petal is released under the [MIT License](LICENSE).
