# Edit Paths: Naming a Value Inside a Literal

An edit path names one value inside a top-level binding, so a tool can set,
insert, remove or move it as a minimal splice. Element children are reached
with XPath-style steps. Indices count from 0, like Petal's lists:

```text
POST.effects[2].amount           a record field, a list element, a field
layers[2].x                      an attribute of an element
layers[2]/group[0]/image[1].x    into an element's children
layers[2]/*[0]/@x                any tag; `/@x` is the same as `.x`
```

- **Module:** [`core/src/edit_path.rs`](../../core/src/edit_path.rs)
  (`petal::edit_path`; `PathSeg`, `parse_edit_path` and `parse_binding_path`
  are re-exported from `petal::rewrite`)
- **Used by:** the path goals ([goal-based-editing.md](goal-based-editing.md#path-goals)),
  `literal_edit::{set_path, insert_path, append_path, remove_path, move_path}`
  ([source-preservation.md](source-preservation.md)), `rewrite::find_binding_path`,
  the `pb_source_*` functions in the C bridge, and `Env::set_config_value`
- **Tests:** [`core/tests/all/element_paths.rs`](../../core/tests/all/element_paths.rs),
  and the unit tests in `edit_path.rs`

## Grammar

A path is a binding name followed by steps:

| Step | Enters | Example |
|---|---|---|
| `.name` | a record's field, or an element's attribute | `POST.exposure`, `layers[0].x` |
| `[n]` | a list's element, or a call's argument | `POST.effects[2]`, `DIR[1]` in `vec3(…)` |
| `/tag[n]` | the `n`-th child element with that tag | `layers[2]/image[1]` |
| `/*[n]` | the `n`-th child element of any tag (except `<br />`) | `layers[2]/*[0]` |
| `/tag` | the only child element with that tag | `card/title` |
| `/@name` | an attribute, XPath's spelling of `.name` | `layers[2]/image[1]/@x` |

An index `n` is decimal digits only (`[+1]` and `[-1]` are not paths);
blanks around it are allowed.

Every path that worked before child steps existed means what it did before.
`.` and `[n]` are unchanged. `/` was not valid in a path until now.

### Decisions

- **0-based indices.** XPath counts from 1. Petal counts list elements from 0,
  and `layers[2]/image[1]` mixes both kinds of step, so both kinds count the
  same way.
- **`/tag` without an index** means the only child element with that tag. If
  there are several, it is an error that lists the indexed spellings:
  "has 2 `<image>` children, so `image` is ambiguous: write `image[0]` …
  `image[1]`". An edit that guesses the first one could change the wrong
  layer without anyone noticing.
- **What counts as a child.** Only child elements count: `<tag …>` written
  between the tags, or an element in a `{<tag />}` hole. These are not
  counted:
  - text, and the whitespace between tags;
  - `{/* … */}` comment holes and `//` comment lines (comment trivia);
  - `{expr}` holes that are not element literals.

  `/*[n]` also skips `<br />`, which is a line break in text, not a layer.
  To name one, use `/br[n]`.
- **Text children have no path.** `text()` (XPath's text node test) is
  rejected with an explanation. To edit text, set the whole element. A
  `<text>` *element* is addressed like any other tag, as `/text[0]`.
- **`/@name` is accepted** as an alias for `.name`. The `@` annotation syntax
  ([annotation-syntax.md](../proposals/annotation-syntax.md)) is Petal source.
  A path is never lexed as Petal, so the two cannot collide. `.name` is the
  canonical spelling, and it is what error messages use.
- **No `children[j]` step.** `layers[2].children[0]` already means "element 0
  of the list in the attribute `children`", and an element may have such an
  attribute. Making it mean the element's children would change what an
  existing path names. `/*[j]` is the spelling for this.

## What each edit does at a child step

The same formatting rules apply as for list elements
([source-preservation.md](source-preservation.md#three-primitives)):

| Edit | At `group/image[1]` |
|---|---|
| set | Edit the child in place. Same tag: only the attributes and children that differ change. Another tag: the child's text is replaced. A child can only be set to an element. |
| insert | Insert before the current `image[1]`. With the index one past the last `<image>`, insert just after that `<image>`. `/*[0]` makes it the first child. Needs an index. Only an element can be inserted. |
| append | `append_path("layers[2]", el)` on an element adds `el` as its last child element. |
| remove | Remove that child. On its own line it takes the line with it, and a `// note` after it on that line (text, though written as a comment about the element). Comments and blank lines on other lines stay. |
| move | `move_path(from, to)`: remove `from` and insert its text at `to`. `to` is read after `from` is taken out, and may be in another list or element. A `to` that names the place `from` already is (`g/image[1]` → `g/*[1]`) changes nothing. The comment trailing a moved list element (or the note after a moved child) comes along; among children it goes on its own line above the element, since after a tag it would be text. |

Layout of a new or moved child:

- Its siblings each have a line: it gets a line at their indentation,
  directly below the previous child's line. A first child goes directly below
  the open tag, or below the text there if text comes before the old first
  child.
- Its siblings share a line (`<a /> <b />`): it joins that line, with the
  spacing they use.
- The element has no child elements yet: the child goes on its own line, one
  indent step in. A self-closing `<group />` gains a close tag. Comments
  already inside stay above the new child.
- A moved multi-line child is re-indented to its new depth.
- In a file whose lines end in CRLF, inserted lines do too (lists alike).
- An element in a `{(<tag />)}` hole is spanned with its parentheses and
  braces, like one in a `{<tag />}` hole.

Setting a whole element whose children are all elements, before and after,
edits the children one at a time, like a list's elements. So a host that writes
back a whole `layers` list after moving a layer inside a group changes only
what moved. A child (or list element) that only moved takes the comment
lines directly above it along (`#` lines above a list element; `{/* … */}`
or `//` lines above a child) (`move_path` does not: it moves only the
element and its trailing comment). An element with no children before counts
as all elements: the new ones go below any comments inside. When text is
among the children and stays the same, with each element child still an
element in the same place, the element children are edited in place;
otherwise the children are rewritten whole, and only if they differ.

## Errors

A path that cannot be parsed is `Invalid`, and the message points at the
problem: "`layers[2]/` is not an edit path: expected a tag name, `*` or
`@attribute` after `/` at `/` (after `layers[2]`)".

A step that names nothing is `NotFound`. The message names the step and
describes what is there:

```text
`layers[2]/image[5].x`: step `/image[5]` names nothing — the `<group>` element at `layers[2]` has 2 `<image>` children
`layers[2][0]`: step `[0]` names nothing — the `<group>` element at `layers[2]` is not a list; its child elements are `layers[2]/*[0]`
`POST.tint.r`: step `.r` names nothing — `POST.tint` is a color, which has no fields
```

## Surfaces

| Surface | Takes paths |
|---|---|
| `Goal::should_set_path` / `should_insert` / `should_append` / `should_remove` / `should_move` | yes |
| `literal_edit::set_path` / `insert_path` / `append_path` / `remove_path` / `move_path` | yes |
| `rewrite::find_binding_path` | yes (reads only) |
| `pb_source_expr` / `_set` / `_set_value` / `_insert` / `_append` / `_remove` / `_move` | yes. Element values have no JSON form yet, so a child is inserted or set whole only from Rust. Attribute edits, removes and moves work. |
| `Env::set_config_value` | yes |
| `petal` CLI, MCP tools | no command takes a binding path |
