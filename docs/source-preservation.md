# Source Preservation: Editing Literals Without Reformatting Them

When a program rewrites a `.ptl` file — an editor saving a slider, a host
adding an entry to a list — the result should read as if the author had made
the change by hand. This page describes how Petal does that for the literals a
config-style file is made of: numbers, strings, colors, and the records, lists
and constructor calls that hold them.

- **Module:** [`core/src/literal_edit.rs`](../core/src/literal_edit.rs)
  (`petal::literal_edit`)
- **API:** the path goals in [goal-based-editing.md](goal-based-editing.md#path-goals);
  `pb_source_*` in the C bridge ([embedding-c.md](embedding-c.md#editing-source-a-script-as-a-config-file))
- **Tests:** [`core/tests/source_preservation.rs`](../core/tests/source_preservation.rs),
  a table of before / edit / after cases plus the properties below

## What an edit guarantees

1. **Unchanged values are not touched.** Wherever the old literal already
   reads as the new value, no text is written. Comments, blank lines,
   alignment and the spelling of every other value come back byte for byte.
2. **A no-op is a no-op.** Writing a value that already holds returns the
   source byte-identical. A save that writes back every field produces no diff.
3. **An edit and its inverse restore the original bytes**: insert then remove,
   remove then insert, change a scalar and change it back.
4. **New text is formatted like what is already there**: like its siblings
   first, then like the container it goes into, then like the rest of the file.
5. **The result parses**, and reading the edited value back yields exactly the
   value that was written. An edit that cannot meet this is an error and
   changes nothing.

One limit on (3): the comment trailing an element on its line is removed with
the element, so re-inserting the element does not bring the comment back. Text
synthesized for an empty container follows the file's dominant style, so it
matches the original only when the original did too.

## The design: minimal splices, plus captured style for new text

Two approaches were considered.

**Template reapplication.** Capture the token and trivia skeleton of the
original construct, render the new value from scratch, then lay the captured
whitespace and comments back over the new tokens. This works when the old and
new values have the same shape: the skeleton has one slot per token. When the
shape changes — an element added, removed or moved — the slots no longer line
up, and deciding which comment belongs to which new token is a diff problem
anyway. It also re-emits text that did not change, so guarantee (1) holds only
if the reapplication is perfect.

**Minimal text-span splices.** Walk the old literal and the new value together
and replace only the smallest span that carries each difference. Unchanged
text is preserved because it is never rewritten, not because it was
reconstructed correctly.

Petal uses minimal splices for everything that already exists, and keeps the
template idea for the one place it is needed: text that has to be synthesized.
There the "template" is a small `Style` captured from an existing literal —
not a token-by-token skeleton, which would not fit a value of a different
shape, but the handful of choices that make up a literal's formatting:

| Captured | Example |
|---|---|
| one line, or one element per line | `[1, 2]` or a column of entries |
| padding inside the brackets | `{ a: 1 }` or `{a: 1}` |
| the separator | `, ` or `,` |
| the spacing after a record key | `key: value`, `key:value` |
| a trailing comma after the last element | `[a, b,]` |
| the indent step | two spaces, four, a tab |

Scalars carry style too, captured from the token being replaced:

| Captured | Old | New value | Written |
|---|---|---|---|
| zero padding of a float | `3.50` | 4.0 | `4.00` |
| | `3.50` | 3.125 | `3.125` (does not fit two decimals) |
| | `1.45` | 1.2 | `1.2` (was not padded) |
| color case | `#FF2E88` | 29d9ff | `#29D9FF` |
| color short form | `#f80` | 00ff88 | `#0f8` |
| | `#f80` | 29d9ff | `#29d9ff` (does not fit three digits) |

Whether a number is an int or a float is not style: the value decides
(`StaticValue::Int(4)` writes `4`, `Float(4.0)` writes `4.0`).

## Three primitives

Every edit is one of three operations on a path such as
`POST.effects[2].amount` (a top-level name, then `.field` steps into record
literals and `[index]` steps into list literals or call arguments):

| Primitive | Text change |
|---|---|
| **set** a scalar | Replace that one token. |
| **insert** an element or field | Add it next to a sibling, formatted like that sibling. |
| **remove** an element or field | Delete it and the separator that belonged to it. |

**Insert** looks at the sibling the new element goes next to (the one before
it, or the one after when it goes first):

- The sibling has its line to itself: the new element gets its own line at the
  same indentation, placed directly below the previous element's line (so a
  comment above the next element stays with that element). In a record whose
  values are lined up in a column, the new field's value is put in that column.
- Otherwise the new element goes on the same line, with the separator the
  other elements use.
- The container is empty: one that is open across lines (`[` newline `]`) is
  filled one element per line; `{ }` keeps its padding; `[]` and `{}` follow
  the file's dominant style, and a list of records or calls goes one per line,
  as `StaticValue::to_source` writes it.

The value itself is rendered with the sibling as its model, recursively: a new
record in a list of compact one-line records is a compact one-line record.

**Remove** deletes an element's whole line when it has the line to itself
(together with a comment trailing it on that line), and otherwise the element
plus one adjacent separator. Comments on other lines are never deleted. A
multi-line list that loses its last element keeps its trailing-comma habit,
and a container emptied on one line keeps its padding, so a later insert finds
the style again.

## Setting a composite value

Setting a record, list or call is planned as a short script of the same three
primitives, by walking the old literal and the new value together:

1. If the old expression already reads as the new value, stop: nothing to do.
2. A record against a record lines its fields up by key. A list against a list
   (or the arguments of a call to the same function) lines its elements up
   with a longest-common-subsequence diff, in three passes: elements that are
   *equal*; then, between those, elements that *share content* (a record with
   half its fields unchanged, a call to the same function); then elements of
   the same *form* (records with the same keys, scalars of the same type).
3. A matched pair recurses into step 1. An unmatched new element is an insert,
   an unmatched old one a remove. An element that leaves one position and
   arrives at another unchanged is inserted as its original text, with its
   trailing comment, rather than re-rendered.
4. Anything else — a number where a record stood, a computed expression — is
   replaced whole, rendered in the old literal's style where that applies.

The script is applied one primitive at a time, each against the text the
previous one produced, re-parsing in between. That keeps every primitive
simple (it only ever sees a real, parsed file) at the cost of one parse per
structural change, which is negligible for a config file. Inserts within a gap
run before removes, so an insertion always has a sibling to copy.

So changing one parameter, removing one effect and adding another in

```petal
effects: [
  {effect: "lens_dirt", amount: 0.5, scale: 3.0},
  // grain goes last
  {effect: "grain", amount: 0.30},
],
```

by setting the whole list to `[crt 0.2, grain 0.35, halftone 0.7]` gives

```petal
effects: [
  {effect: "crt", amount: 0.2},
  // grain goes last
  {effect: "grain", amount: 0.35},
  {effect: "halftone", amount: 0.7},
],
```

## Colors

`#ff2e88` lowers to the record `{r, g, b}` at run time, but in a file it is one
token. `StaticValue::Color { r, g, b, a }` reads and writes it as such: a path
stops at a color (`POST.tint.r` names nothing), and an edit writes a color
literal, in the old literal's case and length form. A record the author wrote
out as `{ r: 255, g: 46, b: 136 }` is still a record.

## What it does not do

- A path sees only literals. It stops at a name, a spread, a parenthesized
  expression or a computed value; setting such a place replaces it whole.
- Records with a `...spread` or a repeated key are not edited field by field
  when set as a whole; they are replaced. Path edits inside them still work.
- `Goal::should_call` still replaces the whole call.
- Line endings: inserted lines end in `\n`, also in a CRLF file.
