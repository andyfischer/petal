# The language server

`petal lsp` is Petal's language server. An editor starts it as a child
process, sends it the text of each open `.ptl` file, and gets back
diagnostics, hover text, definitions, completions and an outline. The server
is the `petal` binary itself, so anyone who has Petal installed has it. The
code is in [`core/src/lsp/`](../../core/src/lsp/).

With this file open:

```petal
enum Kind
    park(code: 2),
    shop(code: 5)
end

let LIMIT = 10

fn area(w, h = 2) -> int
    w * h
end

let k = Kind.park
print(area(LIMIT), k.code)   // 20 2
```

hover on `area` in the last line answers with a Markdown block:

```text
(function) area fn(w, h = 2) -> int
// function, static-pure
```

hover on `code` answers `(constant data) Kind.park.code = 2`,
go-to-definition on `code` lands on the `park(code: 2)` line, and completion
after `k.` offers `code` and `name`.

This page is for people working on the server. What a user needs (how to
point an editor at it) is in
[Getting Started](../Getting_Started.md#editor-support).

- [What it implements](#what-it-implements)
- [Structure](#structure)
- [From text to diagnostics](#from-text-to-diagnostics)
- [Imports, projects and hosts that are not here](#imports-projects-and-hosts-that-are-not-here)
- [Hover](#hover)
- [Go to definition](#go-to-definition)
- [Completion](#completion)
- [Enum members](#enum-members)
- [Document symbols](#document-symbols)
- [Positions](#positions)
- [Tests](#tests)
- [Editors](#editors)
- [Adding a feature](#adding-a-feature)
- [What not to break](#what-not-to-break)
- [Known gaps](#known-gaps)

## What it implements

The dispatch is two `match` statements in `Server::handle_request` and
`Server::handle_notification` ([`server.rs`](../../core/src/lsp/server.rs)).
This is the whole list.

| Requests | Answer |
|---|---|
| `initialize` | the capabilities below |
| `shutdown` | `null` |
| `textDocument/hover` | a `Hover` with Markdown contents, or `null` |
| `textDocument/definition` | one `Location`, or `null` |
| `textDocument/completion` | a `CompletionList` with `isIncomplete: false` |
| `textDocument/documentSymbol` | a tree of `DocumentSymbol` |

| Notifications from the client | Effect |
|---|---|
| `initialized` | none |
| `textDocument/didOpen` | store and analyze the document, publish its diagnostics |
| `textDocument/didChange` | replace the text, analyze again, publish |
| `textDocument/didClose` | forget the document, publish an empty diagnostics list for it |
| `exit` | the stdio loop stops |

| Notifications to the client | When |
|---|---|
| `textDocument/publishDiagnostics` | after every open, change and close |

Any other request gets the error `-32601` (method not found), and any other
notification is ignored. The four `textDocument/` requests answer `-32002`
(server not initialized) before `initialize`. Parameters that do not
deserialize get `-32602`. A message that is not JSON is dropped.

The capabilities `initialize` returns:

```json
{
  "positionEncoding": "utf-16",
  "textDocumentSync": { "openClose": true, "change": 1 },
  "hoverProvider": true,
  "definitionProvider": true,
  "completionProvider": { "triggerCharacters": [".", "/"] },
  "documentSymbolProvider": true
}
```

`change: 1` is full-document sync: every `didChange` carries the whole text,
and the server reads the last entry of `contentChanges`. The one client
capability the server reads is `general.positionEncodings`, to agree on how
columns are counted ([Positions](#positions)); `positionEncoding` is
`utf-16`, `utf-8` or `utf-32`.

There is no formatting, code action, rename, find-references, signature help,
workspace symbol or semantic-token support. Formatting, import fixes and
refactorings exist as commands (`petal fmt`, `petal fix-imports`,
`petal apply-change`, `petal lint --fix`; see the
[CLI reference](../CLI.md)) and are not wired into the server.

## Structure

| File | Holds |
|---|---|
| `mod.rs` | the module list; re-exports `Server` |
| `server.rs` | `Server`: dispatch and one handler per request |
| `document.rs` | `DocumentStore`, `Document`, `Analysis`; `analyze_at`; definition collection; the text helpers that find the word, qualifier or import path at a position; URI and path conversion; `KEYWORDS` |
| `members.rs` | enum members: what a dotted path such as `Kind.park` or `k.code` resolves to, and its completions, hover and declaration |
| `symbols.rs` | the document outline |
| `host.rs` | `HostView`: what the document's project says its host puts in scope (`[host]` in `petal.toml`), and `Joined`, the document joined with the host's prelude files, with the line each joined line came from |
| `position.rs` | `PositionEncoding`: negotiating the column unit and converting columns between it and code points |
| `protocol.rs` | JSON-RPC message types, `Content-Length` framing helpers, error codes |
| `lsp_types.rs` | the subset of LSP types the server uses, written by hand with serde |
| `stdio.rs` | `serve`: the read loop `petal lsp` runs |

`Server` knows nothing about transport. `Server::handle_message(&str)` takes
one JSON-RPC message and returns the messages to send back, so a test or
another host can drive it directly. `stdio::serve` wraps it: it reads the
headers line by line, then exactly `Content-Length` bytes of body, writes each
reply with its own header, and flushes after every message. It stops at end
of input or after an `exit` notification. `petal lsp` takes no file and no
options (`handle_lsp` in `core/src/cli/handlers.rs`).

The server is single-threaded and synchronous. Each message is handled to
completion before the next is read, and nothing is cancelled or debounced.

A `Document` is the URI, the version, the text and an `Analysis`. An
`Analysis` is everything the handlers read: the diagnostics, the definitions
found in the file, the outline, and the imports as they resolved. It is
built once per open or change, and every request is answered from it and
from the text.

## From text to diagnostics

`DocumentStore::open` and `change` call `analyze_at(text, path, open)` in
`document.rs`. An empty or whitespace-only document gets an empty analysis.
Otherwise it does two independent things.

**1. A tolerant parse, for symbols.** `parse_tolerant` parses the text with
`cst::parse_source_phased`. If that fails it repairs the line the error is on
and tries again, up to six times: a dangling `.` gets a placeholder name
after it, any other line that will not parse is blanked, and when the error
is at the start of a line the unfinished line above is blanked. Lines keep
their numbers. The statements that come out feed `collect_definitions` (the
definitions) and `symbols::document_symbols` (the outline). This is why
completion, go-to-definition and the outline keep working in a file that does
not compile, which is the usual state of a file being typed.

**2. A compile, for diagnostics and imports.** `prepare` makes a fresh `Env`
and calls:

- `Env::set_check_only(true)`, so the compile does not link (next section);
- `Env::override_file_source(path, text)` for every other open document, so
  an unsaved edit to a module is what its importers see;
- `HostView::apply`, which binds the host's implicit imports when the
  document's project declares a host
  ([Imports, projects and hosts](#imports-projects-and-hosts-that-are-not-here));

and then `Env::load_program_diag(text, path)`, the same front end
`petal check` and `petal run` use: parse, module loading, the compiler with
the type checker, the name scan for undefined names, and the `const` check.
`text` is the document, or the document joined with its host's prelude
files.

If the load succeeds, each entry of `Program::warnings` becomes a diagnostic,
with severity `Error` or `Warning` as the entry says. If it fails, each item
of the `LoadError` becomes an `Error` diagnostic, so every error the front
end found in one pass is shown. Either way, one whose span is in the open
file is placed at that span.

One that is in another file has no position in this one, and both paths
treat it the same way. It is placed on the path of the `import` line that
brings the module in, with the module's file name at the front of the
message and its place in the module as `relatedInformation` (an editor
shows it as a link). A module that comes in through another module has no
line of its own here, and goes to line 0, column 0. A successful compile
knows each diagnostic's file from its span (`span.file` in the program's
source map); a failed one knows only the module's display name
(`ErrorItem::file`), which is matched against the files the document's import
lines resolve to. An import that does not resolve (`cannot find module
'./x'`) goes on its own line, and an item with no span otherwise at the top.
These rules are `program_diagnostic`, `load_error_diagnostic` and
`ImportSites` in `document.rs`.

A diagnostic in one of the host's prelude files is not the document's: a
successful compile leaves it out (the file shows it when it is open), and a
failed one puts it at the top as `<file> (host prelude): <message>`, with its
place in the file as related information.

Every diagnostic has `source: "petal"`. The server does not lower the program
to bytecode and does not run the lint rules, so two things `petal check` and
`petal lint` report are absent: a program that compiles but fails to lower,
and lint findings.

The imports are read off the compiled program by `imported_modules`. If the
compile failed, the server compiles the file's leading `import` lines alone
(`import_header` finds them without a parse), with an `import` line for each
of the host's implicit imports, so that imported names still complete and
resolve while the rest of the file is broken. The host's prelude files are
read with the tolerant parse, so their names are there either way.

Nothing is cached between analyses. Each open or change parses and compiles
the document and the modules it imports from scratch, in a new `Env`. A
change to one document does not re-analyze the other open documents that
import it; they pick the new text up at their own next change.

## Imports, projects and hosts that are not here

**The document's path.** `uri_to_path` turns a `file:///…` URI into a path,
decoding percent escapes. The path is passed to the compile as the program's
origin, so `import ./level` resolves against the document's own directory.
A URI of any other scheme (an untitled buffer) has no path, and its relative
imports can reach only modules held in memory.

**The project.** The module loader finds the nearest `petal.toml` at or above
the document's directory by itself (`ModuleRegistry::begin_load` in
`core/src/module.rs`), so the `[dependencies]` of the document's project
resolve with no configuration. A manifest that does not parse is a
diagnostic. The server takes no `-I` directories. Besides the project's
dependencies it sees the packages bundled in the binary and those on
`PETAL_PATH`, which `Env::new` scans. See [packages.md](../packages.md) and
[module-system.md](../module-system.md) for the resolution rules.

**No fetching.** `Env::set_install_on_demand` is off by default and the
server leaves it off. A git dependency that is not installed is an
unresolved import, and the server never touches the network.

**Host natives.** A script written for a host calls natives that this
process does not register. The host's interface package declares them with
`native fn`, and a check-only `Env` skips the two steps that would reject
them: `check_requires` (does this host provide the interfaces the packages
`[requires]`) and `link_program` (is every declared native registered). So a
call to a declared native type-checks against its declaration, hovers with
its signature, and produces no diagnostic.

**An interface package the server cannot find.** If the document imports a
package that is not a dependency of its project, bundled or on `PETAL_PATH`,
the import fails and the document gets one diagnostic at the top:

```text
cannot find module 'sdl' (imported by game.ptl): there is no package named 'sdl': …
```

The fix is on the user's side: name the package in the project's
`petal.toml`, as the trees under `examples/` do.

**A host that puts names in scope without an import.** Some hosts run
scripts that import nothing: the host binds its interface as an implicit
prelude, or joins library files ahead of each script into one program (the
worlds-fair UI bundles `host/garden.ptl` and `lib/*.ptl` ahead of each
screen). Compiled alone, such a script calls nothing but unknown functions.
Its project can say what the host does, in a `[host]` section of
`petal.toml` ([packages.md](../packages.md#host)):

```toml
[host]
imports = ["ui", "garden"]
prelude = ["host/garden.ptl", "lib/theme.ptl", "lib/basics.ptl", "lib/layout.ptl",
           "lib/widgets.ptl", "lib/parts.ptl"]
```

`HostView::for_document` ([`host.rs`](../../core/src/lsp/host.rs)) reads it
from the nearest `petal.toml` at or above the document, and the server
compiles the document as that host would:

- **`imports`** are bound the way the core prelude `std` is, through
  `Env::add_tooling_implicit_imports` (crate-private; hosts themselves have no
  implicit imports): every export bare and weak, below the file's own imports
  and declarations, and loaded only when the file names one of its exports.
  The module loader orders a module that such a prelude imports (`ui`
  imports `color`) ahead of it even when the script imports it too.
- **`prelude`** files are joined ahead of the document as one program
  (`HostView::join`). Each file's leading import lines move to the top of the
  joined text, since imports must come first, and every line is kept whole,
  so `Joined` maps a joined line back to the file and line it came from with
  the same column. The names they declare need no `pub` and reach hover,
  go-to-definition (into the prelude file) and completion as a bare import's
  names do. A document that is itself one of the files sees only the files
  listed before it, which is the order rule the host's join imposes.
  An open prelude file's unsaved text is what is joined; one that cannot be
  read is a warning at the top of the document.

With the section above in a copy of the worlds-fair `ui/ptl` (and `garden` as
a path dependency), `host/garden.ptl` goes from 31 diagnostics to 2 (both
real: `key_pressed("]")` is not a key name) and `screens/missions.ptl` from 78
to 0. Only the server reads `[host]`; `petal check` checks a file as it is
written.

**`const` and the static classifier.** Whether a native is constant is the
host's to say. A `const` built from natives the server does not have is
reported as neither right nor wrong, and its hover says `declared const:
cannot be verified without its host`. A `native fn` of a host that is not
here counts as dynamic, so a host may call more declarations static than the
server does, never fewer.

## Hover

`handle_hover` finds the identifier under the cursor
(`identifier_at_position`) and tries these in order. The first that answers
wins.

1. **An enum member**: the word follows a dotted path that resolves to an
   enum or a variant value ([Enum members](#enum-members)).
2. **A qualified name** `m.name`, where `m` is an import's qualifier: the
   module's export, with `// from module <name>`. A name the module does not
   export answers `null`, and nothing else of that name is tried.
3. **A definition in the file**, chosen by `Analysis::definition_in_scope`:
   a parameter or loop variable whose function or loop contains the cursor
   (the innermost one), else the last declaration of that name above the
   cursor, else the first one in the file. The answer carries the
   declaration's range.
4. **A name an import binds bare**, with the module it came from.
5. **A keyword**: ``(keyword) `fn` ``.

Otherwise `null`. A builtin implemented in Rust (`print`, `len`) has no
declaration to show and gets no hover. A function of the `std` prelude does,
because `std` is a module the document imports implicitly.

The text is `(kind) name detail` in a `petal` code block. The kinds are
`variable`, `function`, `enum` and `parameter`. The detail is built when
definitions are collected: `: type` for an annotated binding,
`fn(a, b: int = 2) -> T` for a function (defaults as the source spells them),
`native fn(…)` for a native declaration, and the variants for an enum.

**The static-value note.** For a top-level declaration, case 3 adds a comment
line with what the static-value classifier makes of it: `static`,
`static, kept between runs`, `static, not kept: <why>`,
`function, static-pure`, `function, not static-pure: <why>` or
`dynamic: <the first reason>`, with `; its initializer is static` for a
`state` and a remark for a `const` that fails or cannot be checked. The
wording is `static_notes` in `document.rs`. The classifier is described in
[static-values.md](static-values.md).

This costs a second compile, a classification and a lowering (to see which
declarations the cache gives a slot, `Env::kept_declarations`), which is too
much to pay on every keystroke. So `Analysis::static_notes` is a `OnceLock`
that the first hover after a change fills (`DocumentStore::static_note`) and
later hovers reuse. A test counts classifier runs to hold this.

## Go to definition

`handle_definition` answers with one `Location`, trying in order:

1. **An import path.** With the cursor on the path of an `import` line, the
   file the path names (`Env::resolve_module_file`), at the top of the file.
2. **An enum member**: the variant's own line of the `enum`, in whichever
   file declares the enum.
3. **A qualified name** `m.name`: the declaration in the module's file.
4. **A definition in the file**, by the same scope rule as hover. The range
   is the whole declaring statement.
5. **A qualifier itself** (`hud` in `hud.draw`): the top of the module's
   file.
6. **A name an import binds bare**: its declaration in its own file. A
   re-exported name goes to the file that declares it, not to the module
   that re-exports it.

A name declared in a module that has no file (the `std` prelude, a module a
host registered in memory) answers `null`.

## Completion

`handle_completion` works out a prefix (the identifier under the cursor, or
the one that ends just before it) and then picks one of four lists.

1. **On an `import` line** (`import_path_prefix`): the next path segment of
   every module that could be named. For a relative path, the `.ptl` files
   and directories beside the document. Otherwise the packages the process
   can see: bundled, on `PETAL_PATH`, and the dependencies of the document's
   project, plus `package/module` for each of their modules. `/` is a trigger
   character for this.
2. **After an enum or a variant value and a dot**: its members and nothing
   else ([Enum members](#enum-members)).
3. **After an import's qualifier and a dot**: the module's exports and
   nothing else.
4. **Anywhere else**: the qualifiers of the document's imports, the names
   the imports bind bare, every definition collected from the file, and the
   keywords, each filtered by the prefix.

In the fourth list, definitions are not filtered by scope: a parameter of one
function is offered inside another. With an empty prefix the names of the
implicit `std` prelude are left out, since that is every `std` function on
every keystroke. Items carry a label, a kind and a detail string, and no edit
or snippet.

`KEYWORDS` in `document.rs` is the lexer's keyword list plus the contextual
ones (`as`, `class`, `config`, `native`).
`core/tests/keyword_sync.rs` fails if it drifts from the lexer.

## Enum members

`members.rs` answers "what is this dotted path" from declarations and text
alone. Nothing is compiled or run. `path_before_position` reads the path to
the left of the cursor, and `Analysis::member_at` resolves it to a `Member`:

| `Member` | Written as | Completion offers |
|---|---|---|
| `Enum` | `Kind`, `town.Kind` | the variants, `all`, `from` |
| `Variant` | `Kind.park` | its payload fields, its data keys, `name` |
| `Value` | a value known to be some variant | the data keys, `name`, every payload field |
| `Constructor`, `All`, `From` | `Kind.custom(…)`, `Kind.all`, `Kind.from(…)` | nothing |

A variable resolves through its `ValueHint`, which is recorded when
definitions are collected: its annotation (`k: Kind`), the path its
initializer spells (`let k = Kind.park`), or the path its loop iterates
(`for k in Kind.all`). The enum itself may be declared in the file, imported
bare, or reached through a module qualifier. Hover, completion and
go-to-definition are each a reading of the same `Member`
(`Member::hover`, `completions`, `declaration`).

## Document symbols

`symbols::document_symbols` lists the top-level declarations of the tolerant
parse: functions and native declarations (`Function`), `let` and `var`
(`Variable`), `const` (`Constant`), `state` (`Variable`), classes with their
fields as children, and enums with their variants as children. `range` is the
whole statement and `selectionRange` is the first whole-word occurrence of
the name inside it. Nested declarations are not listed.

## Positions

Inside the server every column counts Unicode code points, which is what the
lexer's spans count. The client counts in the unit agreed in `initialize`,
and the server converts at its edge, in [`position.rs`](../../core/src/lsp/position.rs).

| Where | Unit |
|---|---|
| An incoming `Position.character` | the agreed encoding; `PositionEncoding::to_internal` turns it into code points as the request is read |
| A `SourceSpan` line and column from the lexer | 1-based, columns in code points |
| A `SourceSpan` offset | code points from the start of the file |
| A `Range` inside the server (`span_to_range`, the `Analysis`) | line and column minus one, so code points |
| An outgoing `Range` | the agreed encoding; `range_out` converts each one as the reply is written (diagnostics and their related locations, hover, definition, the outline) |
| `Content-Length` | bytes |

**Negotiation.** `PositionEncoding::negotiate` reads the client's
`general.positionEncodings`. The server answers `utf-16` when the client
offers it or offers nothing (UTF-16 is the protocol's default, and every
server must support it), and otherwise the first of `utf-32` and `utf-8` the
client lists. It says which in the `positionEncoding` capability.

The units agree on an ASCII line, and conversion skips such a line. UTF-16
differs from code points by one for each character outside the Basic
Multilingual Plane (most emoji) to the left of the position, and UTF-8 by
one to three for each non-ASCII character. A position that falls inside a
character (the second unit of a surrogate pair) is that character's. A
location in another file is converted with that file's text: the open
document's, or the file on disk.

Lines are split on `\n`, and a `\r` before it is not part of the line.

The framing is byte-exact. `stdio::read_message` reads `Content-Length`
bytes and then decodes UTF-8, so a body with multi-byte characters is never
cut inside one. `protocol::encode_lsp_message` uses the byte length of the
JSON.

## Tests

| Where | What |
|---|---|
| [`core/tests/lsp_tests.rs`](../../core/tests/lsp_tests.rs) | the main suite: a `Server` driven through `handle_message` with JSON built by small helpers (`init`, `open`, `change`, `hover`, `definition`, `completion`). Lifecycle, diagnostics, each request, imports across files written to a temporary directory, enum members, the outline, the static-value hover. |
| `core/src/lsp/stdio.rs` (unit tests) | the framed loop over in-memory streams: a session end to end, `exit` with input pending, a multi-byte body, a truncated body |
| `core/src/lsp/position.rs`, `host.rs` (unit tests) | encoding negotiation and column conversion; joining a document with its host's prelude files and mapping lines back |
| `core/src/package.rs`, `core/src/module.rs` (unit tests) | parsing `[host]`; an implicit prelude that imports a module the script imports too |
| `core/tests/keyword_sync.rs` | `KEYWORDS` against the lexer |
| `core/tests/manifest.rs` | a `petal.toml` dependency resolves and completes with no configuration |
| `core/src/check.rs`, `core/tests/pub_keyword.rs` | the server shows what `petal check` reports |

```sh
cd core && cargo test --test lsp_tests
cd core && cargo test --lib lsp::
```

To try a change by hand, pipe framed messages into `petal lsp`, or open a
file in Garden.

## Editors

Any client that can spawn a server on stdio works: the command is `petal`
with the single argument `lsp`, for files with the `.ptl` extension.

[`editor-support/`](../../editor-support/README.md) holds syntax
highlighting only (a tree-sitter grammar and a Vim syntax file). It has no
LSP client configuration.

The one client in this repository is Garden. Its registry entry
(`garden/garden-app/src/lsp/registry.rs`) is the command `petal`, the
argument `lsp`, the language id `petal` and the extension `ptl`, with
`GARDEN_PETAL_LSP_BIN` to override the binary. Garden sends `didOpen`,
`didChange` and `didClose`, shows the published diagnostics, and requests
completion.

## Adding a feature

To add a request, say `textDocument/references`:

1. **Types.** Add the parameter and result types to `lsp_types.rs`. They are
   written by hand; copy the field names from the specification and use
   `#[serde(rename = "…")]` for camel case. Enums that go over the wire as
   numbers use the `protocol_enum!` macro.
2. **Dispatch.** Add an arm to `handle_request` in `server.rs` with the same
   `initialized` guard as the others, and a `handle_…` method. Answer
   `INVALID_PARAMS` for parameters that do not deserialize and `null` (or an
   empty list) when there is nothing to say.
3. **Capability.** Add the provider field to `ServerCapabilities` and set it
   in `handle_initialize`. A client does not send a request the server did
   not advertise.
4. **Data.** Answer from the `Analysis`. If the feature needs something the
   analysis does not hold, add a field and fill it in `analyze_at`, from the
   tolerant parse when it must survive a file that does not compile. If it is
   expensive, compute it on first use as `static_notes` does.
5. **Positions.** Convert the request's position with
   `self.encoding.to_internal` before using it, convert every span with
   `span_to_range`, read the text at a position by indexing the line's
   `char`s, as the existing helpers do, and convert each range in the reply
   with `range_out` (or `self.encoding.range_out` for the document itself).
6. **Tests.** Add a helper and cases to `core/tests/lsp_tests.rs`, including
   a file that does not compile and a name that comes through an import.
7. **Docs.** Add the request to the tables on this page and to the feature
   lists in [CLI.md](../CLI.md#lsp--serve-the-language-server) and
   [Getting Started](../Getting_Started.md#editor-support).

A new notification from the client is the same without a reply. A new kind
of diagnostic usually belongs in the compiler, as an entry of
`Program::warnings`: the server then shows it with no change here, and so do
`petal check` and every host.

## What not to break

- **The server never runs the program.** It compiles and classifies. A
  feature that needs a value has to get it without executing user code.
- **The `Env` is check-only.** Linking would reject every script written for
  a host.
- **No network.** Leave `set_install_on_demand` off.
- **Symbols survive a broken file.** Definitions and the outline come from
  the tolerant parse, not from the compile. Completion is asked for exactly
  when the line does not parse.
- **Open documents shadow the disk.** An importer must see the unsaved text
  of a module that is open.
- **The analysis on each change stays cheap.** Anything slow is computed on
  demand and kept until the next change.
- **Diagnostics are what `petal check` reports.** The same front end, the
  same messages.
- **`handle_message` stays free of transport.** The tests and any embedding
  host depend on it.
- **Columns are code points inside, the client's unit outside.** Convert a
  request's position once as it is read and each reply's ranges once as they
  are written ([Positions](#positions)); nothing in between converts.
- **Nothing is written to stdout except framed messages.** A stray `print`
  in the server corrupts the stream.

## Known gaps

- **A module reached through another module.** A diagnostic in it is placed
  at the top of the importing document, since no import line of the document
  names it. A failed compile in one is placed by its display name, which can
  miss a module of a package whose file is not where its name says
  (`bloom/menu.ptl` under `src/`); it then goes to the top too.
- **`[host]` is the server's alone.** `petal check` does not read it, so the
  two disagree on a script that relies on it. A prelude file is joined as
  text, so a relative import in one resolves against the document's
  directory, not the file's.
- **Importers are not re-analyzed** when a module they import changes.
- **Completion ignores scope** outside the member and qualifier cases.
- **No workspace.** The server reads no `rootUri` and no configuration, and
  of the client's capabilities only `general.positionEncodings`. Each document
  is analyzed alone, with what its own project's `petal.toml` says.

## See also

- [CLI reference, `lsp`](../CLI.md#lsp--serve-the-language-server).
- [Getting Started, Editor support](../Getting_Started.md#editor-support).
- [`core/src/check.rs`](../../core/src/check.rs): what "the check" is, and
  which parts of it the server shares.
- [module-system.md](../module-system.md) and [packages.md](../packages.md):
  how an import resolves.
