# Garden Pane Protocol (GPP), version 2

GPP lets a child process (a client app) drive the content of a Garden pane.
Right after the handshake the client pushes a Petal UI script, which Garden
compiles and runs in its in-process panel runtime. Garden handles all input,
animation, and rendering locally. The client then acts as the panel's data
server, answering the `query`, `mutate`, and `navigate` requests Garden issues
on the running script's behalf. Only data crosses the pipe.

This is the normative reference for GPP v2, written so a client can be
implemented from it alone in any language. Related:

- The Rust wire definition is the `gpp` crate (`gpp/src/lib.rs`). The host
  side is `garden-app/src/process_pane.rs`, `script_client.rs`, and
  `panel_view.rs`.
- The Rust client library is [petal-query](../../petal-query/README.md)
  (`Provider` plus `gpp::serve`), which every in-tree app uses. The
  step-by-step guide is [writing-gpp-apps.md](writing-gpp-apps.md); the
  Python client is [writing-gpp-apps-python.md](writing-gpp-apps-python.md).
- The draw and input vocabulary of the pushed script is
  [petal-graphical-panels.md](petal-graphical-panels.md).

## Transport

Newline-delimited JSON over stdio:

- Garden writes host-to-client messages to the child's stdin.
- The client writes client-to-host messages to its stdout.
- The child's stderr is free for its own logging (Garden inherits it). Never
  print diagnostics to stdout; every stdout line must be a protocol message.

Framing: exactly one compact JSON object per line, UTF-8, terminated by `\n`,
with no embedded newlines inside the object. "Serialize compact, append `\n`,
flush" is the whole rule. EOF on stdin means the session is over. A line that
does not parse is a protocol error; Garden tears the pane down rather than
guessing.

If a client writes to the pipe from more than one thread (a hot-reload
watcher, say), write each complete line under one lock so two messages cannot
interleave.

## The envelope

Every message is a JSON-RPC 2.0 object:

| Kind | Fields | Notes |
| --- | --- | --- |
| request | `jsonrpc`, `id`, `method`, `params` | expects exactly one response, matched by `id` |
| notification | `jsonrpc`, `method`, `params` | no `id`; never answered |
| response | `jsonrpc`, `id`, and `result` or `error` | never `method`; correlates by `id` only |

```jsonc
// request           {"jsonrpc":"2.0","id":5,"method":"query","params":{…}}
// notification      {"jsonrpc":"2.0","method":"emit","params":{…}}
// success response  {"jsonrpc":"2.0","id":5,"result":{…}}
// error response    {"jsonrpc":"2.0","id":5,"error":{"code":1,"message":"…"}}
```

Absent fields are omitted. `id` is an unsigned integer; Garden starts at 1
with `initialize` and counts up, and notifications consume no ids. Only the
host sends requests, so a client never needs an id counter; it echoes the id
it is answering. Typed params and results use camelCase keys (`paneId`,
`maxAgeMs`).

`error` is `{ "code": <integer>, "message": "<reason>" }`:

| Code | Name | Meaning |
| --- | --- | --- |
| `1` | APP | the handler ran and failed ("not a git repo", "no such screen"); `message` is what the script sees via `error_of` |
| `2` | PROTOCOL_MISMATCH | the peer speaks an incompatible protocol major version |
| `-32601` | METHOD_NOT_FOUND | no handler for the method. A client must answer an unknown request with this, or Garden waits out its timeout. Unknown notifications are silently skipped. |
| `-32602` | INVALID_PARAMS | the params did not decode |

## Message vocabulary

The complete v2 vocabulary:

| Method | Kind | Direction | Params to result | Purpose |
| --- | --- | --- | --- | --- |
| `initialize` | request (id 1) | host to client | `InitializeParams` to `InitializeResult` | version and capability handshake; carries the pane id, size, launch args, cwd |
| `shutdown` | notification | host to client | `{}` | the client should exit (it also exits on stdin EOF) |
| `setScript` | notification | client to host | `{ source }` | load or hot-reload the pane's Petal UI script |
| `query` | request | host to client | `{ kind, arg }` to `{ value?, cache? }` | the script called `query(kind, arg)` and Garden has no fresh cached value |
| `mutate` | request | host to client | `{ name, arg }` to `{ value? }` | run the effectful `mutate(name, arg)` the script asked for; never cached |
| `navigate` | request | host to client | `{ screen, arg? }` to `{ screen, source }` | serve a declared screen's UI source |
| `emit` | notification | both | `{ event, arg }` | host to client: the script's `emit(event, arg)` calls; client to host: a client-raised event (reserved names below) |
| `invalidate` | notification | client to host | `{ kind, arg }` | drop the cached value for `(kind, arg)` so the script re-queries it |

Reserved client-to-host `emit` events. Any other event is ignored by Garden:

| Event | Arg | Effect |
| --- | --- | --- |
| `open_path` | `{ "path": "<abs path>" }` | replace this pane with an editor on `path`; ends the session |
| `status` | `{ "text": "<line>" }` | set the pane's status-bar text |

## Handshake

1. Garden spawns the child and writes an `initialize` request (id 1):

   ```jsonc
   { "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
       "protocol": 2,          // the protocol major the host speaks
       "paneId": 0,
       "rows": 40, "cols": 120, // pane size in character cells (informational)
       "args": ["/tmp/demo"],  // launch arguments: how a client learns what to serve
       "cwd": "/tmp/demo",
       "capabilities": ["query", "mutate", "navigate", "emit", "hotReload"]
   } }
   ```

2. The client must answer this id before sending anything else. Garden blocks
   reading exactly one line during the handshake, so a client that pushes its
   script first deadlocks it:

   ```jsonc
   { "jsonrpc": "2.0", "id": 1, "result": {
       "protocol": 2,
       "name": "my-app",       // pane display name until the drawer draws one
       "capabilities": ["query", "mutate", "navigate", "emit", "setScript"]
   } }
   ```

3. The client then pushes its UI script:

   ```jsonc
   { "jsonrpc": "2.0", "method": "setScript", "params": { "source": "…petal…" } }
   ```

**Version rule.** `protocol` is the major version; both halves carry it. A
missing `protocol` decodes as `1`. A client that sees a major it does not
speak answers `initialize` with a `PROTOCOL_MISMATCH` error and exits. A host
that sees a mismatched or missing `protocol`, or an error response, tears the
pane down and shows the message in the pane's error card.

**Capabilities** are freeform name lists, advisory in both directions; each
side ignores names it does not know. Garden reports `query`, `mutate`,
`navigate`, `emit`, and `hotReload` (a later `setScript` swaps the running
drawer in place, preserving panel `state`). The Rust client library reports
`query`, `mutate`, `navigate`, `emit`, `setScript`. Minor protocol additions
ride on capabilities; the major only bumps on breaking changes.

## `setScript`: the page

`{ "source": "<petal ui script>" }`. Garden compiles the source and runs it
as an in-process [panel](petal-graphical-panels.md). Every non-reserved key,
mouse, and wheel event goes to the script; the `:` command line and the
global chords (quit, window nav, clipboard) stay with Garden.

A later `setScript` hot-reloads: Garden recompiles in place and keeps both
the panel's `state` and its query cache. A source that fails to compile
leaves the old program running and shows the error in the pane, so pushing
mid-keystroke is safe.

## `query`: the cacheable pull

The script calls `query(kind, arg)` every frame. Garden serves it from a
per-pane cache and only crosses the pipe on a miss or on staleness:

```jsonc
→ { "jsonrpc":"2.0", "id":5, "method":"query",
    "params": { "kind": "table", "arg": { "name": "users", "page": 3 } } }
← { "jsonrpc":"2.0", "id":5,
    "result": { "value": { "rows": [ … ] },
                "cache": { "maxAgeMs": 3000, "staleWhileRevalidateMs": 60000 } } }
```

- `arg` is any JSON value: a string, a record, a list, a number, or absent
  (decodes as `null`). Garden caches per `(kind, arg)`; `arg` equality is by
  canonical JSON serialization. (The script-side `query` native currently
  passes a string arg; the wire and the cache already accept richer keys.)
- Success is a result with `value` (a JSON tree mapped onto Petal values:
  object to record, array to list, null to nil, an exactly-integer number to
  int, anything fractional to float) and an optional `cache`
  ([cache control](#cache-control), absent means fresh forever).
- A result with no `value` (`{}`) means still loading: the client
  acknowledges the request but the data is not ready. Garden keeps the
  script's spinner up without re-requesting until the client pushes an
  `invalidate` for the key.
- Failure is an APP error response; the script reads the message via
  `error_of`. A failed query is cached as an errored entry until invalidated.
- A kind the client does not serve conventionally answers `{"value": null}`
  (the Rust library does this) rather than erroring.

### Cache control

`cache` says how cacheable one answer is, the pull-model cousin of an HTTP
`Cache-Control` header. Durations are whole milliseconds. All fields are
optional:

| Field | Type | Meaning |
| --- | --- | --- |
| `maxAgeMs` | integer | how long the answer stays fresh; while fresh Garden never re-asks. Absent means fresh forever (until `invalidate`). |
| `staleWhileRevalidateMs` | integer | how long past `maxAgeMs` a stale answer is still served while a background refetch runs. Absent means no stale window: at `maxAgeMs` the entry expires and the next query shows a spinner until the refetch lands. |
| `noStore` | bool | never fresh, never expired: always serve the last value and trigger a background refetch. Live data with no spinner flicker. When true the other fields are ignored. |

Freshness at age *t*: `noStore` means always stale (serve and revalidate); no
`maxAgeMs` means always fresh; `t ≤ maxAgeMs` is fresh; `t ≤ maxAgeMs +
staleWhileRevalidateMs` is stale (serve and revalidate); beyond that the
entry is dropped and the next query is a spinner-and-refetch miss.

## `mutate`: the effectful call

The script called `mutate(name, arg)`. `arg` is any JSON tree. The result is
never cached.

```jsonc
→ { "jsonrpc":"2.0", "id":9, "method":"mutate",
    "params": { "name": "apply", "arg": { "edits": [ … ] } } }
← { "jsonrpc":"2.0", "id":9, "result": { "value": "wrote 2 files" } }
```

A string `value` becomes the pane's status line; an absent `value` shows
"`<name>`: done". A failed mutation is an APP error, shown as a status error.
Garden blocks up to 2000 ms for the response; a timeout is reported as an
error.

**Host-owned mutations.** These names are answered by Garden itself and never
reach the client. They are how a drawer asks Garden to act, and they work in
an in-process `panel(...)` pane that has no client at all:

| Name | Arg | Effect |
| --- | --- | --- |
| `open_path` | `{ "path": "…" }` | the file replaces the focused pane (ends a GPP session) |
| `open_project` | `{ "path": "…" }` | record the project, then browse it |
| `open_pr` | `{ "number": 12 }` | open the `garden-diff` review on that PR |
| `open_file_dialog` | `{ "mode": "file" \| "folder" }` | native picker, then the matching open |

Every other name is forwarded to the client unchanged. Clients must not
define mutations with these names.

## `navigate`: multi-screen apps

The script navigated to a declared screen (`navigate("detail.ptl")`, or
`navigate("detail.ptl", { id: 7 })`, whose argument the target reads back
with `nav_arg()`), and Garden needs that screen's source:

```jsonc
→ { "jsonrpc":"2.0", "id":4, "method":"navigate",
    "params": { "screen": "detail.ptl", "arg": { "id": 7 } } }   // arg omitted for the 1-arg form
← { "jsonrpc":"2.0", "id":4,
    "result": { "screen": "detail.ptl", "source": "…petal…" } }
```

Garden owns the history stack (back, forward, per-entry `state` snapshots);
the client owns the sources. Its declared screens are its allowlist: refuse
an undeclared screen with an APP error, message `no such screen '<name>'`.

Back and forward re-issue the restored entry's `navigate` with that entry's
own `arg`, so a handler with side effects re-runs them on every revisit;
write it idempotent per visit. The replay is best effort: on error or timeout
Garden keeps the cached source and shows the reason in the status note.
Garden blocks up to 500 ms for a navigate response.

## `emit`: the fire-and-forget push

A notification in both directions: no id, no reply, unknown events skipped.

- Host to client: the script's `emit(event, arg)` calls, drained once per
  frame tick and forwarded in call order. `arg` is any JSON tree. This is the
  channel for "the user did something your app wants to persist or act on".
- Client to host: a client-raised event. Garden acts only on the reserved
  names `open_path` and `status`.

```jsonc
→ { "jsonrpc":"2.0", "method":"emit", "params": { "event":"divider", "arg": { "pos": 240 } } }
← { "jsonrpc":"2.0", "method":"emit", "params": { "event":"status", "arg": { "text": "3 files" } } }
```

## `invalidate`: the client pushes fresh data

A client-to-host notification: drop the cached value for `(kind, arg)` so
the next frame's `query` re-requests it. This is how a file watcher, a
poller, or a finished background job publishes a new answer. `arg` must equal
the queried arg (the same JSON value) for the keys to match.

```jsonc
← { "jsonrpc":"2.0", "method":"invalidate", "params": { "kind": "log", "arg": "" } }
```

## Lifecycle

1. **Spawn.** Garden starts the child with stdin and stdout piped, stderr
   inherited.
2. **Handshake.** `initialize` request, response, `setScript`. On a spawn or
   handshake failure the pane shows the error; on a protocol mismatch the
   message names both versions.
3. **Priming.** Right after the handshake Garden runs a few synchronous query
   round-trips (bounded, about 200 ms waits) so the first painted frame has
   data instead of a spinner.
4. **Steady state.** Garden polls the pipe on its ~200 ms tick: it drains
   client messages, applies query answers to the cache, and flushes the
   queries the last frame missed on. Query latency is therefore about one
   tick. `mutate` and `navigate` are bounded synchronous waits (2000 ms and
   500 ms); envelopes that arrive while one is waiting are applied normally.
5. **End of session.** Garden sends `shutdown` and closes stdin; the client
   exits on either. A session also ends when the client emits `open_path` or
   the pane is closed. Garden then kills and reaps the child.

The client loop, in pseudocode:

```text
read initialize; check protocol == 2; reply (id 1); send setScript
for each line on stdin:
    query     → run handler → response (result {value, cache?} | APP error)
    mutate    → run handler → response (result {value?}        | APP error)
    navigate  → look up screen → response ({screen, source}    | APP error)
    emit      → run handler (no reply)
    shutdown  → exit
    unknown request       → METHOD_NOT_FOUND error response
    unknown notification  → ignore
on stdin EOF → exit
```

## Worked example: directory browser

The in-tree `directory-browser` app lists a directory via `query("list",
dir)` and opens a file via the host-owned `open_path` mutation. Lines are
pretty-printed here; on the wire each is one compact line. `→` is host to
client (stdin), `←` is client to host (stdout).

```jsonc
// 1. Handshake.
→ {"jsonrpc":"2.0","id":1,"method":"initialize",
   "params":{"protocol":2,"paneId":0,"rows":40,"cols":120,
             "args":["/tmp/demo"],"cwd":"/tmp/demo",
             "capabilities":["query","mutate","navigate","emit","hotReload"]}}
← {"jsonrpc":"2.0","id":1,
   "result":{"protocol":2,"name":"/tmp/demo",
             "capabilities":["query","mutate","navigate","emit","setScript"]}}
← {"jsonrpc":"2.0","method":"setScript","params":{"source":"…browser.ptl…"}}

// 2. The drawer's first frame calls query("list", ""): a cache miss, so the
//    host asks. "" means "the launch directory".
→ {"jsonrpc":"2.0","id":2,"method":"query","params":{"kind":"list","arg":""}}
← {"jsonrpc":"2.0","id":2,
   "result":{"value":{"path":"/tmp/demo","parent":"/tmp","home":"/Users/andy",
                      "entries":[{"name":"subdir","is_dir":true,"path":"/tmp/demo/subdir"},
                                 {"name":"file_a.txt","is_dir":false,"path":"/tmp/demo/file_a.txt"}]},
             "cache":{"maxAgeMs":2000,"staleWhileRevalidateMs":60000}}}

// 3. The user presses j or clicks: handled host-side, no pipe traffic.
//    Descending into subdir re-keys the drawer's query: another miss.
→ {"jsonrpc":"2.0","id":3,"method":"query","params":{"kind":"list","arg":"/tmp/demo/subdir"}}
← {"jsonrpc":"2.0","id":3,"result":{"value":{"path":"/tmp/demo/subdir", …}}}

// 4. Enter on a file: the drawer calls mutate("open_path", {path}). The host
//    answers that itself and swaps the pane to an editor, ending the session.
→ {"jsonrpc":"2.0","method":"shutdown","params":{}}
// …then stdin closes; the client exits.
```

A failure, for contrast: `git-log` in a directory that is not a repository.

```jsonc
→ {"jsonrpc":"2.0","id":2,"method":"query","params":{"kind":"log","arg":""}}
← {"jsonrpc":"2.0","id":2,"error":{"code":1,"message":"not a git repo: /tmp/demo"}}
```

The script's `query("log", "")` reads as an errored pending value;
`error_of(rd)` is `"not a git repo: /tmp/demo"`.

## The in-tree apps

All seven are `petal-query` providers, one crate per directory under
`gpp-apps/`:

- `directory-browser`: the netrw-style listing behind `garden <dir>`, `:E`,
  and `-`. `query("list", dir)`; opens files via the host-owned `open_path`.
- `git-viewers` (binary `git-log`): the `:Git` history browser. `query("log")`
  and `query("commit", hash)` by shelling out to `git`; uses the full
  cache-policy range.
- `garden-diff`: the editable diff review behind `:Diff`, `:Review`, `:PR`,
  `garden diff`, and `garden pr`. `query("doc")`, `query("commits")`, and the
  `mutate("apply", { edits })` write-back.
- `sqlite-browser`: a read-only SQLite and Postgres browser.
- `main-menu`: the start screen a bare `garden` opens. `query("recents")`
  from the state database; opens rows via the host-owned mutations.
- `screens-demo`: the worked example of `navigate`, two declared screens and
  no queries.
- `gpp-test-app`: a fixture. Its launch arg (`ok`, `runtime-error`,
  `runtime-error-long`, `query-error`, `save`) puts a pane into that state
  for screenshots and integration tests.

## Change log

- **v2** (2026-08): panel-only. v1's Lines mode (client-pushed text renders,
  key and mouse forwarding, takeover layers, `resize`) is gone. Responses
  correlate by id only; failures are `error` responses; `query.arg` is
  arbitrary JSON; `initialize` carries `protocol` and `capabilities` both
  ways; `navigate` is a first-class request; `openPath` and `setStatus` are
  replaced by reserved `emit` events; notifications no longer consume ids. A
  v1 client against a v2 host, or vice versa, is refused at `initialize`.
- **v1** (2026-07): the original two-mode protocol.
