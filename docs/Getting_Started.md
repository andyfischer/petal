# Getting Started

## Install

Install the `petal` CLI as a prebuilt binary. It runs on macOS (Apple Silicon or
Intel) and Linux (x86_64 or arm64):

```bash
curl -fsSL https://petal-lang.org/install.sh | sh
```

This puts `petal` in `~/.petal/bin` and adds that directory to your PATH. It
needs no `sudo` and has no dependencies. Open a new shell, then check it:

```bash
petal --version
```

The installer reads a few environment variables:

| Variable | Effect |
|----------|--------|
| `PETAL_VERSION` | Install a specific release tag, e.g. `v0.1.0`, instead of the latest |
| `PETAL_INSTALL` | Install prefix (default `~/.petal`; the binary goes in `$PETAL_INSTALL/bin`) |
| `PETAL_NO_MODIFY_PATH=1` | Leave your shell rc file alone; add `~/.petal/bin` to PATH yourself |

To uninstall:

```bash
curl -fsSL https://petal-lang.org/uninstall.sh | sh
```

## Your first program

Create a file called `hello.ptl`:

```petal
let name = "world"
print("hello, {name}!")
```

Run it:

```bash
petal run hello.ptl
```

`petal hello.ptl` is shorthand for `petal run hello.ptl`, and `-e` runs code
given on the command line:

```bash
petal run -e 'print(1 + 2)'
```

## The loop you will use

```bash
petal check app.ptl    # compile and check without running; exits 1 on any error
petal run app.ptl      # then run it
```

`check` is fast. It catches syntax errors, misspelled names, calls with the
wrong number of arguments, and type mismatches against any annotations you
wrote, all before anything runs. Run it after every edit.

When a program runs but prints the wrong thing, the CLI can show you what
happened:

```bash
petal run --observe app.ptl        # the last value of every variable, even after a crash
petal explain --term total app.ptl # where the value of `total` came from, step by step
```

Every subcommand has built-in help: `petal help`, `petal help run`,
`petal help check`.

## Imports and projects

`print`, `len`, `range`, the collection and string functions and plain math
are always in scope. Everything else is one `import` away, at the top of the
file:

```petal
import creative                 // lerp, map_range, distance, …

print(lerp(0, 10, 0.5))         // 5.0
```

```petal ignore
import ./level                  // level.ptl beside this file
import vec2                     // vec2, vec3, normalize, dot, …
import ui                       // drawing and input, for a graphical host
```

`petal check` names the import a script is missing, and `petal fix-imports
app.ptl` adds it. To depend on a library, make the directory a project and
add it; every command then finds the project from the file it is given:

```bash
petal init                                            # writes petal.toml
petal add tween --git https://example.org/tween --tag v1.2.0
petal run app.ptl                                     # `import tween` now resolves
```

See [Module System](module-system.md) for the import rules and
[Packages](packages.md) for `petal.toml`, the lockfile and the package
commands.

## Editor support

- **LSP:** `petal lsp` speaks the Language Server Protocol over stdio, with
  diagnostics, hover, go-to-definition, completion and a document outline.
  Point your editor's LSP client at it: the command is `petal lsp`, for
  `.ptl` files. It finds a file's project (the nearest `petal.toml`) by
  itself, so imports of the project's dependencies resolve with no setup.
  What it implements and how it works is in
  [The language server](internals/lsp.md).
- **Syntax highlighting:** a tree-sitter grammar (Neovim, Helix, Zed, Emacs)
  and a classic Vim syntax file are in
  [`editor-support/`](../editor-support/README.md).

## Learning the language

Read these in order:

1. **[Writing Petal](writing-petal-guide.md).** This is the guide to read
   first. It covers how a program is shaped, the rules that differ from other
   languages (`let` / `var` / `state`, immutable values, integer division), and
   how to use the tooling. It takes about an afternoon.
2. **[The console examples](../examples/console/).** Each file is a short,
   commented treatment of one feature. The test suite runs every one of them,
   so they are always current. Start with `fizzbuzz.ptl`, `records.ptl`,
   `classes.ptl`, `pattern_matching.ptl` and `state.ptl`.
3. **[Language Guide](language-guide.md).** The full reference, for when you
   need the exact rule.

Some topics have a page of their own:

- **[Syntax](syntax/overview.md).** The overview is a compact map of every
  form. The other pages in [`syntax/`](README.md#syntax) each take one piece
  that needs more than a paragraph, such as [`var`, `set` and
  `get`](syntax/var.md), [overloading and named
  arguments](syntax/function-overloading.md) and [implicit return
  values](syntax/implicit-return-values.md).
- **[Features](README.md#features).** The features that reach across the
  language, the runtime and the tools. Read [Inline
  State](features/inline-state.md) early: the `state` keyword is how a
  program that runs every frame remembers anything. [Hot
  Reload](features/hot-reload.md) explains what happens to a running program
  when you edit its source.

Keep the [Builtins Reference](Builtins.md) and the [CLI Reference](CLI.md)
open while you work. The [documentation index](README.md) lists everything.

## Building apps

The `petal` CLI runs console programs: `print` output, no window. Graphical
and interactive programs run inside a host that gives Petal a screen and input,
such as the SDL desktop runner, a web canvas, or the Garden editor.
[Building Apps](usage/building-apps.md) explains the options and how to choose
one. The other pages in [`usage/`](README.md#usage) cover the work after that
choice: embedding Petal in a [Rust](usage/ffi.md) or [C/C++](usage/embedding-c.md)
program, using a `.ptl` file as [configuration](usage/config-files.md), and
[drawing text](usage/text-and-fonts.md). The
[examples](../examples/README.md) directory has games, productivity apps and
dashboards built that way.

## Working on Petal itself

To change the compiler or runtime, you build from source instead. See
[Developer Scripts & Commands](dev/scripts.md) and [Testing](dev/testing.md).
