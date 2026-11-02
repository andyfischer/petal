# petal-images follow-up: things to fix in Petal

Status: 2026-10-23, re-checked against `1064dcd` on 2026-11-02: every
item below is still open. These came out of building
[`petal-creative/petal-images`](../../../petal-creative/petal-images), a layered
image editor whose documents are `.ptl` scripts of JSX-style elements
(`<image>`, `<text>`, `<group>`…) edited by the GUI through goal-based literal
edits. The original notes are in `petal-images/docs/petal-notes.md`. Nothing
in `~/petal` was changed. They're listed by priority.

## 1. Goal-based edits can't step into element children

`literal_edit` treats an element's attributes as a path step
(`layers[0].x`), but not its children. A layer inside a `<group>` can't be
addressed as `layers[2].children[0].x`.

What petal-images does now:

- **Attribute edits** on a nested layer use a workaround (`Doc::edit_nested`).
  It parses the AST, finds the child's span, edits that one element in a
  scratch `let` source, and splices the result back.
- **Structural edits** (insert, remove, move inside a group) still set the
  whole `layers` list. That re-renders every child of the group, so the
  comments and hand formatting between its tags are lost. Before the
  workaround, even attribute edits replaced references with their values
  (`fill={accent}` became `fill={#ff2d75}`), which silently broke shared
  template colours.

Action:
- [ ] Add a `children[j]` path step on elements (or plain `[j]`) that counts
      element children only, skipping whitespace text and `<br />`.
- [ ] Support insert, remove and move at such a path, with the same
      formatting rules as list items: keep the indentation and put the new
      child on its own line when its siblings each have one.
- [ ] After that, petal-images can delete `edit_nested` and its
      whole-`layers` rewrite.

## 2. Attribute and record key order is lost on the way to the host

The guide says an element's `props` are "a record of the attributes, in
order". `value_to_json`, though, hands the host a `serde_json::Map`, which
sorts its keys unless serde_json's `preserve_order` feature is on.

This causes two problems:
- **Effect order can't follow the file.** petal-images can't apply effects in
  the order they're written (`<text stroke drop_shadow>` vs
  `<text drop_shadow stroke>`). It falls back to a fixed order plus an
  `effects={[…]}` list.
- **Records come back in the wrong order.** A record sent to a native
  (`{x, y, w, h}`) arrives as `{h, w, x, y}` and gets written back to the file
  that way. petal-images re-sorts records against a hard-coded list of
  familiar keys.

Action:
- [ ] Keep insertion order when converting values for hosts: turn on
      `preserve_order`, or use an ordered map in `value_to_json`.
- [ ] Add `value_to_static`, which converts a Petal value straight to a
      `StaticValue` with order kept, so hosts that write values back to
      source skip the JSON detour.
- [ ] Add a test that `props` keeps the order the attributes were written in.

## 3. `StaticValue::to_source` pads record braces

Re-rendered records come out as `{ x: 520, y: 180 }`. A literal edit keeps the
file's own `{x: 520, y: 180}`, and that unpadded form is also what the docs
and hand-written Petal use. Because of item 1, a file's style drifts depending
on which layers were edited.

Action:
- [ ] Render records without the inner padding, or follow a formatter
      setting. Check the other `to_source` callers for snapshot tests that
      would change.

## 4. AST span offsets are characters, not bytes, and this isn't documented

The lexer works on a `Vec<char>`, so `SourcePosition::offset` counts `char`s.
A host that slices the `&str` source by it gets the wrong text, or a panic,
after any non-ASCII character. Marketing copy is full of them: `café`, `→`,
`·`, `—`. petal-images converts the offsets with `char_indices`.

Action:
- [ ] Say on `SourcePosition` (and in the embedding docs) that offsets count
      characters.
- [ ] Consider a `byte_offset` alongside it, or a helper
      `span_to_byte_range(src, span)`. This ties in with the code-point
      switch in [code-point-migration.md](code-point-migration.md).

## 5. A `//` line inside element children becomes text

```petal
<group>
  // the photo
  <image src="a.png" />
</group>
```

`// the photo` becomes a text child of the group. JSX does the same, but
`//` is a comment everywhere else in Petal, and Petal has no `{/* */}` form.
Authors comment their layer stacks constantly.

Action:
- [ ] Decide on one of these:
  - Treat a line in element children whose first non-blank characters are
    `//` as a comment. Text that really starts with `//` would need `{"//"}`.
  - Add `{/* … */}` and say in the error or docs that it's the way to comment
    inside an element.
- [ ] Either way, the rewriter has to keep these comments when it edits
      siblings (see item 1).

## 6. Record fields have no shorthand for `true`

An element attribute written bare is `true` (`<text uppercase>`), but
`mask={{type: "rect", invert}}` fails to parse with "Expected ':'".
Documents mix attributes and option records all the time, so authors trip on
the difference.

Action:
- [ ] At least, replace the parse error with one that names the fix: "record
      fields need a value; did you mean `invert: true`?"
- [ ] Decide whether `{invert}` should mean `{invert: true}` inside element
      attributes only. JS readers would expect `{invert: invert}`, so a clear
      error may be the better choice.

## 7. A stale installed CLI is hard to spot

petal-images hit a keyed-loop parse error that turned out to be a stale
`~/.cargo/bin/petal`. `petal --version` now prints the commit and build
date (`petal 0.1.0 (1064dcd004 2026-11-02, built 2026-11-02)`).

Action:
- [ ] Let `petal.toml` state a minimum Petal version and have `petal check`
      warn when the binary is older. Today `[requires] petal = ">=9.0"` is
      read as a host interface named `petal`, and `run` fails with "requires
      the host interface `petal`". Moses asked for the same
      ([moses-debrief.md](moses-debrief.md)).

## 8. JSON encoding is hard to find

petal-images reported "no `json_encode` builtin". In fact `json_stringify`
exists behind `import json` ([Builtins.md](../Builtins.md)). The note came
from guessing the name, so this is about how easy it is to find, not a
missing feature.

Action:
- [ ] When the name being looked up is `json_encode` (or `to_json`,
      `stringify`), have the "unknown function" error suggest
      `json_stringify` and `import json`.
