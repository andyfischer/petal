# transpile_to_react_js

Petal source in, React.js source out. The program is read, never run, so the
output keeps the program's shape: a function that builds an element is a
component, `state` is `useState`, a loop over a data list is a `.map` with
keys, and the comments come along. What has no React form is converted as
far as it goes and reported, both as a warning and as a
`// TODO(petal-to-react)` comment where it happened.

It is the first of a family: the same reading of a Petal program is meant to
drive SwiftUI and Compose writers next (Mixit claim 13, "an approved design
is easy to implement natively").

```bash
cargo run --bin petal-to-react -- menu.ptl                # print the module
cargo run --bin petal-to-react -- design-folder out/      # write a project
cargo run --bin petal-to-react -- app.ptl out/ --dialect web --name Menu --strict
```

With an output folder it writes every module the entry reaches (relative
imports, and packages the nearest `petal.toml` lists as path dependencies,
under `packages/<name>/`), the runtime files they import, and an
`elements.jsx` of stubs for tags no dialect knows. `--strict` exits 1 when
there are warnings.

## What becomes what

| Petal | React |
|---|---|
| `fn card(p) <vstack>…</vstack>` (returns an element) | `export function Card({ p }) { return <VStack>…</VStack>; }` |
| `card(x)`, `card(p: x)` | `<Card p={x} />` |
| `state n = 0` in a component | `const [n, setN] = useState(0)` (`useState(() => …)` when the initializer is not cheap) |
| `fn f() state v = …; v` (keeps state, returns a value) | a custom hook, `useF()` |
| `state var theme = "dark"` at the top level | `export const themeCell = cell("dark")`, read with `useCell(themeCell)` |
| top-level `state` and what depends on it | the default-exported app component |
| `if clicked(id) then open = !open` with `let id = next_id()` and `eid={id}` (petal-web-html) | `const id = () => setOpen(!open)` and `onClick={id}` |
| `map(xs, f)`, `for x in xs key x.id do …` | `xs.map((x, i) => <F key={i} … />)`, `key={x.id}` |
| a list of elements, `append(xs, <el/>)`, `a ++ b` | keyed by position (Petal keys unkeyed state by position too), per operand in a concatenation |
| `r.a.b = 3` | `r = { ...r, a: { ...r.a, b: 3 } }` (records are values) |
| `x == nil`, `a == b`, `a == "x"` | `x == null`, `eq(a, b)` (by content), `a === "x"` |
| `:FF9500` | `hex("FF9500")` (`{r, g, b}`) |
| `"{a} b"`, `a ++ b` | template literal, `+` or `concat` |
| `f(1, scale: 2)` | arguments placed by the parameter list; a runtime function's trailing `{ … } = {}` takes the named ones |
| overloads `fn area(r)` / `fn area(w, h)` | `area_1`, `area_2`, and an `area(...args)` dispatcher |
| `enum`, `match` | frozen objects with `tag`, an `if` chain |
| a collecting `for` with `break`/`continue` | an explicit loop that pushes |
| `import ./x`, `pub import ./x`, `x.name` | `import { … } from "./x.jsx"`, `export * from`, the name (aliased when it would clash) |

## Dialects

Petal elements are data a host draws, so what `<text>` means depends on the
host. The dialect is detected from the imports (`--dialect` overrides):

| Dialect | Detected by | Elements |
|---|---|---|
| `design` | `import design` (or a kit) | Mixit's vocabulary, as components of `petal-design.jsx`: `<vstack>` → `VStack`, `<instance of="card">` → `<Card>`, `<slot>` → `slot(props.children, …)`. `component("card", card, props: …)` makes `card(p, cx)` a component whose props are merged over `Card.defaults`, with `cx` from `useCx()`. A `<frame id="…">` written at the top level is lifted into a component of its own (`TodayIos`). `design(doc, components, pages)` is the default export. |
| `web` | `import web` | HTML; `class` → `className`, `style="a: b"` → a style object, the click polls as above. |
| `immediate` | `import ui` / `bloom` | converted as far as the language goes; the drawing calls have no React form and become stubs. |
| `generic` | anything else | HTML tags are DOM elements; other tags are components imported from `./elements.jsx`. |

A module that names no dialect (a design's helper module) takes its
importer's.

## The runtime files

The output imports small runtime files, written next to it:

- `petal-runtime.js`: the Petal builtins with no direct JS form (`len`,
  `range`, `eq`, `with_at`, `format`, `hex`, `Map`…), and `cell`.
- `petal-react.js`: `useCell`.
- `petal-design.jsx`: the design vocabulary. Layout follows Mixit's own HTML
  export (stacks are flexbox, a zstack a one-cell grid, a frame's children at
  their x/y inside its safe area); tokens resolve per platform and mode from
  the frame they are drawn in.
- `petal-design-data.js`: the design package's tables (devices, type ramps),
  **generated** from its Petal source by this transpiler
  (`runtime/gen-design-data.sh`).

The runtime files are the list of what converts: a call is converted when a
runtime exports the name, and its named arguments are placed by the JS
signature (`src/sigs.rs` reads the files). Adding a builtin is adding an
export.

## Warnings

| Code | Meaning |
|---|---|
| `unknown-name` | a name defined nowhere the transpiler can see: a stub that throws when called |
| `state-write-in-render` | state changed while rendering (Petal re-runs every frame; React changes state in handlers) |
| `state-in-loop`, `hook-in-loop`, `hook-call` | a hook React would not allow there (Petal keeps a slot per iteration or branch) |
| `state-outside-component` | `state` in a callback or helper React does not render |
| `write-in-handler`, `write-in-callback` | a click (or a function) changes a name that is not state |
| `immediate-mode`, `native`, `package` | drawing calls, host natives, packages not transpiled |
| `unknown-element`, `unknown-component` | a tag or `<instance of>` with no definition in reach |
| `approximate` | drawn approximately by the runtime (icons, materials, `system(…)` colours) |
| `top-level-state-read` | a top-level function reads top-level state, which now lives in the app component |
| `overload`, `memo`, `test`, `event`, `style`, `render-call` | smaller cases, each explained in its message |

## Testing

```bash
cd core-sdk/transpile_to_react_js
(cd e2e && npm ci)      # once: esbuild, React and jsdom for the end-to-end tests
cargo test
```

- `tests/transpile.rs`: what each construct becomes.
- `tests/corpus_and_e2e.rs`: every example in the repository transpiles;
  the menu example renders in React and its clicks open and close the menus;
  a design fixture renders, alone and frame by frame. Without
  `e2e/node_modules` the end-to-end tests skip and say so.
- `e2e/corpus.mjs <bin> <folders…>`: transpile every `.ptl` and check each
  module parses as JSX. On 2026-11-21: all 247 files of `examples/`,
  `garden/examples/`, petal-web-html and Mixit (`app/examples`,
  `packages/`).
- `e2e/examples.sh <bin> <out> <designs…>`: transpile each design as a
  project and render it (`e2e/render.mjs`, which can also `--click`). On
  2026-11-21 all 6 Mixit examples (the iOS and Material kits included) and
  all 11 Squads designs render with no React error.
- `e2e/shot.sh <entry.jsx> <export> <w> <h> <out.png>`: a Chrome screenshot
  of one export, to compare with `mixit render`.

## Not yet

- Types: Petal's `/` truncates between two ints, JS's does not; only
  literals are folded. A `str` of a float prints `1` where Petal prints
  `1.0`. Truthiness follows JS (`[]` is truthy).
- Closures capture by reference in JS and by value in Petal: a lambda that
  reads a name rebound after it was made sees the new value.
- `state` inside a loop or branch is reported, not extracted into a keyed
  child component.
- `overrides` reach nodes by `id` through the element tree the component
  returns, and through nested design components; not through plain function
  components.
- Comments are kept above statements; comments inside expressions and
  element children are lost (the AST does not carry them).
- Immediate-mode drawing (`import ui`) has no React form.
- Design rules are kept as data, not checked.

See `docs/petal-upstream-ideas.md` in the Mixit repository for what in
Petal itself would make these better.
