# regex

Regular expressions for Petal scripts: an **optional native package**.

```petal ignore
import regex

let m = regex_find("Lot 9 opens 8:30", "(\d+):(\d+)")   # {match: "8:30", start: 12, end: 16, groups: ["8", "30"]}
regex_all("a1 b22", "\d+")                             # two match records
regex_test("2026-11-17", "^\d\{4}-\d\{2}-\d\{2}$")     # true
regex_replace("a  b   c", "\s+", " ")                  # "a b c"
regex_split("a, b ,c", "\s*,\s*")                      # ["a", "b", "c"]
regex_escape("1+1=2?")                                 # "1\+1=2\?"
```

The full reference is the `@doc` on each function in
[`src/regex.ptl`](src/regex.ptl) (`petal doc core-stdlib/regex`).

## Why it is optional

Neither core, the `petal` command nor Garden links a regex engine: a game or
a wasm build that never matches a pattern should not carry one. A host that
wants it opts in by linking the crate in [`native/`](native/) and registering
it:

```toml
# Cargo.toml
petal-regex = { path = "<petal>/core-stdlib/regex/native" }
```

```rust
let mut env = petal::env::Env::new();
petal_regex::register(&mut env);   // the natives, the package, and `provides regex`
let pid = env.load_program(source)?;
```

`register` registers the six natives, registers the package source (so
`import regex` resolves with nothing on disk) and announces that the `Env`
provides `regex` at this package's version, so a package that says
`[requires] regex = ">=0.1"` loads there. A host that loads the package from
disk instead (`env.add_package`) calls `petal_regex::register_natives`.

In a host that has not registered it, `import regex` fails to load
(`petal run` included).

## Semantics

- **Syntax** is Rust's [`regex`](https://docs.rs/regex) crate: no
  backreferences or look-around, and in exchange matching is linear in the
  text, so a pattern from an untrusted source cannot hang a script.
- **Positions count characters** (code points), like every string position
  in Petal: `slice(text, m.start, m.end) == m.match`.
- **`nil` text is no match**: `regex_find` gives `nil`, `regex_all` and
  `regex_split` `[]`, `regex_test` `false`, `regex_replace` `nil`. Any other
  non-string is an error.
- **A bad pattern is an error**: `regex_find: bad pattern "a(b": unclosed
  group (at character 1)`.
- **Braces**: in a `"…"` string a `{` starts an interpolation, so write a
  counted repeat `"\d\{4}"` and a named replacement `"$\{name}"`, or use a
  raw `"""…"""` string. `\d`, `\s`, `\w`, `\b` need no doubling.
- Every native is a constant function of its arguments
  (`NativeEffects::CONST`). Compiled patterns are cached per thread, 128 of
  them, least recently used out first.

## Tests

`tests/regex.ptl` holds the package's `test` blocks. `petal test` cannot
register a host's natives, so they run from Rust:

```bash
cd core-stdlib/regex/native && cargo test
```

`tests/package.rs` there runs every block with the natives registered and
checks that `src/regex.ptl`'s declarations agree with what `register`
registers (`Env::verify_interface`).
