# Garden Pane Protocol (GPP) — version 2

GPP lets a **child process (a client app) drive the content of a Garden pane**.
The model is "web browser + web server": right after the handshake the client
pushes a **Petal UI script** (the "page") that the host compiles and runs in its
in-process panel runtime — the host handles all input, animation, and rendering
locally. The client then acts as the panel's **data server**, answering the
`query` / `mutate` / `navigate` requests the host issues on the running script's
behalf. Only data crosses the pipe; the interaction loop never does.

This document is the **normative reference** for GPP v2. It is written so a
client can be implemented from it alone, in any language (the first non-Rust
client is Python). The single Rust wire definition is the `gpp` crate
(`garden/gpp/src/lib.rs`); the host side lives in
`garden-app/src/process_pane.rs` + `panel_view.rs` + `script_client.rs`; the
Rust client library is [`petal-query`](../../petal-query/README.md)
(`Provider` + `gpp::serve`), which every in-tree app uses. The step-by-step
app-building guide is [`writing-gpp-apps.md`](writing-gpp-apps.md); the panel
draw/input vocabulary the pushed script uses is
[`petal-graphical-panels.md`](petal-graphical-panels.md).

**v2 is a breaking redesign.** v1's Lines mode (client-pushed text `render`s,
key forwarding, takeover layers, mouse forwarding) is gone — the protocol is
**panel-only**. Responses correlate **by id**; failures are JSON-RPC error
responses; `query` args are arbitrary JSON; the handshake carries an explicit
protocol version and capability lists; navigation is a first-class request; and
`openPath` / `setStatus` are folded into reserved `emit` events. A v1 client
against a v2 host (or vice versa) is refused cleanly at `initialize`.

## Transport

Newline-delimited JSON over stdio:

- The host writes host → client messages to the child's **stdin**.
- The client writes client → host messages to its **stdout**.
- The child's **stderr** is free for its own logging (the host inherits it).
  Never print diagnostics to stdout — every stdout line must be a protocol
  message.

**Framing:** exactly one compact JSON object per line, UTF-8, terminated by
`\n`, with **no embedded newlines** inside the object (newlines in strings are
escaped as `\n` by any JSON serializer, so "serialize compact, append `\n`,
flush" is the whole rule). A reader consumes one line, parses it as one
envelope; EOF on stdin means the session is over. A line that does not parse is
a protocol error; the host tears the pane down rather than guessing.

If a client writes to the pipe from more than one thread (e.g. a hot-reload
watcher), each complete line must be written atomically — serialize the whole
envelope under one lock so two messages cannot interleave inside a line.

## The envelope

Every message is a JSON-RPC 2.0 shaped object:

| kind | fields | notes |
| --- | --- | --- |
| **request** | `jsonrpc`, `id`, `method`, `params` | expects exactly one response, matched by `id` |
| **notification** | `jsonrpc`, `method`, `params` | no `id`; fire-and-forget, never answered |
| **response** | `jsonrpc`, `id`, and `result` *or* `error` | never `method`; correlates to its request **by `id` only** — nothing is echoed |

```jsonc
// request           {"jsonrpc":"2.0","id":5,"method":"query","params":{…}}
// notification      {"jsonrpc":"2.0","method":"emit","params":{…}}
// success response  {"jsonrpc":"2.0","id":5,"result":{…}}
// error response    {"jsonrpc":"2.0","id":5,"error":{"code":1,"message":"…"}}
```

Absent fields are omitted on the wire. `id` is an unsigned integer; each side
mints ids for its own requests (the host starts at 1 with `initialize` and
counts up; notifications consume no ids). In v2 **only the host sends
requests**, so a client never needs an id counter — it only echoes the id it is
answering. A response carries `result` on success or `error` on failure, never
both.

All typed params/results use **camelCase** JSON keys (`paneId`, `maxAgeMs`).

`error` is an `RpcError`:

```jsonc
{ "code": <integer>, "message": "<human-readable reason>" }
```

| code | name | meaning |
| --- | --- | --- |
| `1` | APP | An application-level failure: the handler ran and failed ("not a git repo", "no such screen"). The `message` is what the panel script surfaces via `error_of`. |
| `2` | PROTOCOL_MISMATCH | The peer speaks an incompatible protocol major version. |
| `-32601` | METHOD_NOT_FOUND | The request's method has no handler. A client MUST answer an unknown *request* with this (or the host waits out its timeout); unknown *notifications* are silently skipped — that is the forward-compatibility rule. |
| `-32602` | INVALID_PARAMS | The request's params did not decode. |

## Message vocabulary

The complete v2 vocabulary — nothing else is on the wire:

| method | kind | direction | params → result | purpose |
| --- | --- | --- | --- | --- |
| `initialize` | request (id 1) | host → client | `InitializeParams` → `InitializeResult` | version + capability handshake; hands the client its pane id, size, launch args, cwd |
| `shutdown` | notification | host → client | `{}` | the client should exit (it also exits on stdin EOF) |
| `setScript` | notification | client → host | `{ source }` | (re)load the pane's Petal UI script; the first push right after the handshake, later pushes hot-reload |
| `query` | request | host → client | `{ kind, arg }` → `{ value?, cache? }` | the running script called `query(kind, arg)` and the host has no fresh cached value |
| `mutate` | request | host → client | `{ name, arg }` → `{ value? }` | run the effectful `mutation(name, arg)` the script asked for; never cached |
| `navigate` | request | host → client | `{ screen, arg? }` → `{ screen, source }` | serve a declared screen's UI source; the host owns the history stack, the client owns the sources |
| `emit` | notification | both | `{ event, arg }` | host → client: the script's `emit(event, arg)` calls. client → host: a client-raised event; only the reserved names below are acted on |
| `invalidate` | notification | client → host | `{ kind, arg }` | drop the cached value for `(kind, arg)` so the script re-queries it — how a client pushes fresh data |

Reserved client → host `emit` event names (any other event is ignored by the
host, reserved for future use):

| event | arg | effect |
| --- | --- | --- |
| `open_path` | `{ "path": "<abs path>" }` | replace this pane with a normal editor on `path`; ends the session (the host shuts the client down) |
| `status` | `{ "text": "<line>" }` | set the pane's status-bar text |

## Handshake

1. The host spawns the child and writes an `initialize` **request** (id 1):

   ```jsonc
   { "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
       "protocol": 2,          // the protocol MAJOR the host speaks
       "paneId": 0,
       "rows": 40, "cols": 120, // pane size in character cells (informational)
       "args": ["/tmp/demo"],  // launch arguments — how a client learns what to serve
       "cwd": "/tmp/demo",
       "capabilities": ["query", "mutate", "navigate", "emit", "hotReload"]
   } }
   ```

2. The client MUST answer this exact id **before sending anything else** — the
   host blocks reading exactly one line during the handshake, so a client that
   pushes its script first would deadlock it:

   ```jsonc
   { "jsonrpc": "2.0", "id": 1, "result": {
       "protocol": 2,
       "name": "my-app",       // pane display name until the drawer draws one
       "capabilities": ["query", "mutate", "navigate", "emit", "setScript"]
   } }
   ```

3. The client then SHOULD immediately push its UI script:

   ```jsonc
   { "jsonrpc": "2.0", "method": "setScript", "params": { "source": "…petal…" } }
   ```

**Version rule.** `protocol` is the major version; both halves carry it. A
missing `protocol` field decodes as `1` (a pre-versioning peer). If the client
sees a major it does not speak, it answers `initialize` with a
`PROTOCOL_MISMATCH` (code 2) error response and exits. If the host sees a
mismatched (or missing) `protocol` in the result — or an error response — it
tears the pane down cleanly and surfaces the message in the pane (the error
card), instead of leaving a wedged panel.

**Capabilities** are freeform name lists, advisory in both directions: each
side ignores names it does not know. The host currently reports `query`,
`mutate`, `navigate`, `emit`, and `hotReload` (a later `setScript` swaps the
running drawer in place, preserving panel `state`); the Rust client library
reports `query`, `mutate`, `navigate`, `emit`, `setScript`. Minor protocol
additions ride on capabilities; the major only bumps on breaking changes.

## `setScript` — the page

`{ "source": "<petal ui script>" }`. The host compiles the source and runs it
as an in-process [panel](petal-graphical-panels.md): every non-reserved key,
mouse and wheel event goes to the script; the host command bar (`:`) and the
global chords (quit, window nav, clipboard) stay with the host.

A **later** `setScript` hot-reloads: the host recompiles in place and keeps
both the panel's `state` and its query cache, so a reload costs a recompile,
not a refetch. A source that fails to compile leaves the old program running
and surfaces the error in the pane — pushing mid-keystroke is safe.

## `query` — the cacheable pull

The running script calls `query(kind, arg)` every frame; the host serves it
from a per-pane cache and only crosses the pipe on a miss (or staleness):

```jsonc
→ { "jsonrpc":"2.0", "id":5, "method":"query",
    "params": { "kind": "table", "arg": { "name": "users", "page": 3 } } }
← { "jsonrpc":"2.0", "id":5,
    "result": { "value": { "rows": [ … ] },
                "cache": { "maxAgeMs": 3000, "staleWhileRevalidateMs": 60000 } } }
```

- **`arg` is any JSON value** — a string, a record, a list, a number, or absent
  (decodes as `null`). Composite keys need no string encoding. The host caches
  per `(kind, arg)`; `arg` equality is by canonical JSON serialization.
  (Petal's script-side `query(kind, arg)` native currently passes a string arg,
  which crosses the wire as a JSON string — the wire and the cache already
  accept richer keys.)
- **Success** is a result with:
  - `value` — the resolved data, a JSON tree the panel runtime maps onto Petal
    values (object → record, array → list, string, bool, null → nil; a number
    that is exactly an integer becomes an int, anything fractional a float).
  - `cache` (optional) — the answer's [cache policy](#cache-control). Absent =
    fresh forever (cache until an `invalidate`).
  - `value` **absent** (an empty result `{}`) means **still loading**: the
    client acknowledges the request but the data is not ready (a background
    thread is working). The host keeps the script's spinner up without
    re-requesting, until the client pushes an `invalidate` for the key.
- **Failure** is an error response (code `1`/APP); the script reads the message
  via `error_of`. A failed query is cached as an errored entry until
  invalidated.
- A query for a kind the client does not serve conventionally answers
  `{"value": null}` (the Rust library does this) rather than erroring.

### Cache control

`cache` tells the host how cacheable one answer is — the pull-model cousin of
an HTTP `Cache-Control` header. Durations are whole **milliseconds**. Fields
(all optional; an absent field takes its default):

| field | type | meaning |
| --- | --- | --- |
| `maxAgeMs` | integer | how long the answer stays **fresh** after it lands; while fresh the host never re-asks. Absent = fresh forever (cache until `invalidate`). |
| `staleWhileRevalidateMs` | integer | how long **past** `maxAgeMs` a stale answer is still served while a background refetch runs. Absent = no stale window: at `maxAgeMs` the entry hard-expires and the next query shows a spinner until the refetch lands. |
| `noStore` | bool | never fresh, never expired: always serve the last value *and* trigger a background refetch. Live data with no spinner flicker. When true the other fields are ignored. |

An omitted `cache` object (the default) serializes to nothing and means *fresh
forever* — the right choice for a value addressed by an immutable key (a commit
hash). Freshness at age *t*: `noStore` → always stale (serve + revalidate);
no `maxAgeMs` → always fresh; `t ≤ maxAgeMs` → fresh; `t ≤ maxAgeMs +
staleWhileRevalidateMs` → stale (serve + revalidate); beyond → expired (drop;
next query is a spinner-and-refetch miss).

## `mutate` — the effectful call

The script called `mutate(name, arg)` — an effectful request/response call, the
fourth quadrant beside `query` (cacheable pull) and `emit` (fire-and-forget
push). `arg` is any JSON tree. The result is **never cached**.

```jsonc
→ { "jsonrpc":"2.0", "id":9, "method":"mutate",
    "params": { "name": "apply", "arg": { "edits": [ … ] } } }
← { "jsonrpc":"2.0", "id":9, "result": { "value": "wrote 2 files" } }
```

A string `value` is surfaced verbatim as the pane's status line ("wrote 2
files"); an absent `value` shows "`<name>`: done". A failed mutation is an APP
error response, shown as a status error. The host blocks up to **2000 ms** for
the response (mutations are user-initiated); a timeout is reported as an error.

**Host-owned mutations.** A short list of names is answered by the **host
itself** and never reaches the client — the one documented channel a drawer has
to ask Garden to act (an in-process `panel(...)` pane has no client at all, so
`mutate` must work without one):

| name | arg | effect |
| --- | --- | --- |
| `open_path` | `{ "path": "…" }` | the file replaces the focused pane (ends a GPP session) |
| `open_project` | `{ "path": "…" }` | record the project, then browse it |
| `open_pr` | `{ "number": 12 }` | open the `garden-diff` review on `--pr <n>` |
| `open_file_dialog` | `{ "mode": "file" \| "folder" }` | native picker, then the matching open |

Every other name is forwarded to the client's `mutate` handler unchanged.
Clients must not define mutations with these reserved names.

## `navigate` — multi-screen apps

A first-class request (v1 drove this through a magic `navigate` *mutation*).
The running script navigated to a declared screen (`navigate("detail.ptl")`,
or the two-argument `navigate("detail.ptl", { id: 7 })` whose subject the
target screen reads back with `nav_arg()`); the host needs that screen's UI
source:

```jsonc
→ { "jsonrpc":"2.0", "id":4, "method":"navigate",
    "params": { "screen": "detail.ptl", "arg": { "id": 7 } } }   // arg omitted for the 1-arg form
← { "jsonrpc":"2.0", "id":4,
    "result": { "screen": "detail.ptl", "source": "…petal…" } }
```

The **host owns the history stack** (back/forward, per-entry `state`
snapshots); the **client owns the sources** (its declared screens are its
allowlist — refuse an undeclared screen with an APP error, message
`no such screen '<name>'`). Back and forward **re-issue** the restored entry's
`navigate` with that entry's own `arg`, so a client whose navigate handler has
side effects (priming the target screen's data) re-primes on every revisit —
write it idempotent per visit. The replay is best effort: on error/timeout the
host keeps the cached source and shows the reason in the status note. The host
blocks up to **500 ms** for a navigate response.

## `emit` — the fire-and-forget push

A notification, in **both** directions; no id, no reply, unknown events skipped
by the receiver.

- **Host → client**: the script's `emit(event, arg)` calls, drained once per
  frame tick and forwarded in call order. `arg` is any JSON tree. This is the
  channel for "the user did something your app wants to persist or act on" — a
  divider position on drag-end, a request to kick a refresh.
- **Client → host**: a client-raised event. The host acts only on the
  [reserved names](#message-vocabulary) `open_path` and `status`; anything else
  is ignored (reserved for future use).

```jsonc
→ { "jsonrpc":"2.0", "method":"emit", "params": { "event":"divider", "arg": { "pos": 240 } } }
← { "jsonrpc":"2.0", "method":"emit", "params": { "event":"status", "arg": { "text": "3 files" } } }
```

## `invalidate` — the client pushes fresh data

Client → host notification: drop the cached value for `(kind, arg)` so the
next frame's `query` re-requests it. The client-driven counterpart of the
script's own `invalidate(...)` — how a file watcher, a poller, or a
finished background job publishes a new answer. `arg` must equal the queried
arg (the same JSON value) for the keys to match.

```jsonc
← { "jsonrpc":"2.0", "method":"invalidate", "params": { "kind": "log", "arg": "" } }
```

## Lifecycle

1. **Spawn** — the host starts the child with stdin/stdout piped, stderr
   inherited. The pane's cwd and launch args ride in `initialize`.
2. **Handshake** — `initialize` request (id 1) → response → `setScript`, as
   above. On a spawn or handshake failure the pane shows the error; on a
   protocol mismatch the message names both versions.
3. **Priming** — right after the handshake the host runs a few synchronous
   query round-trips (bounded, ~200 ms waits) so the pane's first painted frame
   has data instead of a spinner.
4. **Steady state** — the host polls the pipe on its ~200 ms tick: it drains
   client messages (responses, `setScript`, `invalidate`, `emit`), applies
   query answers to the cache, and flushes the queries the last frame missed
   on. Query latency is therefore ~one poll tick. `mutate` and `navigate` are
   bounded synchronous waits (2000 ms / 500 ms); envelopes that arrive while
   one is waiting are applied normally, nothing is lost.
5. **End of session** — the host sends `shutdown` and closes stdin; the client
   exits on either (`shutdown` or EOF). A session also ends when the client
   emits `open_path` (the pane becomes an editor) or the pane is closed. The
   host then kills and reaps the child.

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

## Worked example — directory browser

The in-tree `directory-browser` app (`gpp-apps/directory-browser`): the drawer
lists a directory via `query("list", dir)` and opens a file via the host-owned
`open_path` mutation. Lines are pretty-printed here; on the wire each is one
compact line. `→` is host → client (stdin), `←` is client → host (stdout).

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

// 2. The drawer's first frame calls query("list", "") — a cache miss, so the
//    host asks. "" means "the launch directory".
→ {"jsonrpc":"2.0","id":2,"method":"query","params":{"kind":"list","arg":""}}
← {"jsonrpc":"2.0","id":2,
   "result":{"value":{"path":"/tmp/demo","parent":"/tmp","home":"/Users/andy",
                      "entries":[{"name":"subdir","is_dir":true,"path":"/tmp/demo/subdir"},
                                 {"name":"file_a.txt","is_dir":false,"path":"/tmp/demo/file_a.txt"}]},
             "cache":{"maxAgeMs":2000,"staleWhileRevalidateMs":60000}}}

// 3. The user presses j / clicks — all handled host-side, zero pipe traffic.
//    Descending into subdir re-keys the drawer's query — another miss:
→ {"jsonrpc":"2.0","id":3,"method":"query","params":{"kind":"list","arg":"/tmp/demo/subdir"}}
← {"jsonrpc":"2.0","id":3,"result":{"value":{"path":"/tmp/demo/subdir", …}}}

// 4. Enter on a file: the drawer calls mutate("open_path", {path}). The host
//    answers that mutation itself — nothing crosses the pipe — and swaps the
//    pane to an editor, ending the session:
→ {"jsonrpc":"2.0","method":"shutdown","params":{}}
// …then stdin closes; the client exits.
```

A failure, for contrast — `git-log` in a directory that is not a repository:

```jsonc
→ {"jsonrpc":"2.0","id":2,"method":"query","params":{"kind":"log","arg":""}}
← {"jsonrpc":"2.0","id":2,"error":{"code":1,"message":"not a git repo: /tmp/demo"}}
```

The script's `query("log", "")` reads as an errored pending value;
`error_of(rd)` is `"not a git repo: /tmp/demo"`.

## The in-tree apps

All seven are `petal-query` providers (`Provider` + `gpp::serve`), one crate
per directory under `gpp-apps/`:

- **`directory-browser`** — the netrw-style listing behind `garden <dir>` and
  `:E` / `-`; `query("list", dir)`, opens files via the host-owned `open_path`
  mutation.
- **`git-viewers`** (bin `git-log`) — the `:Git` history browser;
  `query("log")` / `query("commit", arg)` by shelling `git`; the full
  cache-policy range (short `maxAge` for the log, immutable per-hash diffs,
  `noStore` for the worktree diff).
- **`garden-diff`** — the *editable* diff/review behind `:Diff` / `:Review*` /
  `:PR` and `garden diff` / `garden pr`; `query("doc")`, `query("commits")`,
  and the `mutate("apply", { edits })` write-back — the reference for a
  non-read-only app.
- **`sqlite-browser`** — read-only SQLite *and* Postgres browser behind a
  `db::Backend` trait; short `maxAge` + stale-while-revalidate answers.
- **`main-menu`** — the start screen a bare `garden` opens; `query("recents")`
  from the state database; opens rows via the host-owned mutations.
- **`screens-demo`** — the worked example of `navigate`: two declared screens,
  no queries.
- **`gpp-test-app`** — a fixture: the launch arg (`ok` / `runtime-error` /
  `runtime-error-long` / `query-error` / `save`) puts a pane into that exact
  state for screenshots and integration tests
  (`garden --subprocess gpp-test-app runtime-error`).

## Change log

- **v2** (2026-08) — panel-only (Lines mode, `render`, key/mouse forwarding,
  takeover layers, `resize`, and the `Key`/`StyleKind`/`BgKind` encodings
  removed); responses correlate by id only; failures are `error` responses;
  `query.arg` is arbitrary JSON; `initialize` carries `protocol` +
  `capabilities` both ways; `navigate` is a first-class request; `openPath` /
  `setStatus` replaced by reserved `emit` events; notifications no longer
  consume request ids.
- **v1** (2026-07) — the original two-mode protocol.
