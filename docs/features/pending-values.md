# Pending values

A value the host has not got yet (a query in flight, an image still loading)
reaches a script as a **pending value**. The script uses it like any other
value, with no `await` and no callbacks: whatever it computes from the
pending value is pending too, a draw call handed one draws nothing, and on a
later frame, once the resource has landed, the same code runs with the real
value. A few builtins look at a pending value without being absorbed by it,
which is how a script shows a spinner or an error.

This page is the script's side. The design record, with the prior art and
what was rejected, is [dev/pending-values-plan.md](../dev/pending-values-plan.md);
the signatures are in [Builtins](../Builtins.md#pending-values).

## Where pending values come from

From a host native. Garden's `query(kind, arg)` returns the provider's answer
when it has one and a pending value while the fetch is in flight
(`garden/garden-script/src/query.rs`); `petal-ui`'s panel stubs, which stand in
for Garden in tests and in `petal-ui-run`, answer a pending value forever
unless given a fixture (`core-libs/petal-ui/src/panel_stubs.rs`). `host_data`
does not: it keeps its synchronous contract and returns `nil` while data is
unavailable.

The CLI has three test-only builtins that drive the resource table by hand,
which is how every example on this page runs under `petal run`:

| Builtin | What it does |
|---|---|
| `__pending(key)` | the resource named `key`: its value if it has resolved, else a pending value (loading, or errored) |
| `__resolve(key, value)` | marks the resource ready with `value` |
| `__reject(key, error)` | marks the resource failed with `error` |

They are `__`-prefixed because they are not for scripts.

## The rule

**Every operation that touches a pending value returns that pending value,
except the inspectors below.** With two different pending values the leftmost
wins.

```petal
let user = __pending("user")
print(is_pending(user + 1))          // true
print(is_pending(user == 1))         // true
print(is_pending("hi {user}"))       // true
print(is_pending(user.name))         // true
```

| Where the pending value is | What happens |
|---|---|
| arithmetic, a math builtin, a comparison | the result is the pending value; a comparison is never `false` |
| string concatenation, interpolation | the whole string is the pending value |
| field or index access on it | the pending value |
| the condition of `if`, the subject of `match` | no branch runs; the expression's value is the pending value |
| the left side of `&&` or `\|\|` | the right side does not run; the result is the pending value |
| the condition of `while`, the list of `for` | no iteration runs; a `for` in value position is the pending value |
| an argument of a user function | the call runs normally, so a helper deep inside can still ask `is_loading(x)` |
| an argument of a draw call, `print`, `push_output` or another emitting native | the call does nothing and returns `nil` |
| an argument of any other native | the native is not called; the result is the pending value |
| a map key or index (`m[k] = v`) | a runtime error, `Cannot use a pending value as a map key / index` |
| what a `sort` comparator returns | a runtime error |
| called as a function (`u(1)`, `u(...xs)`) or as a method (`u.greet()`) | nothing runs; the result is the pending value |

So `if user.admin || override then … end` draws nothing while `user` is
loading, rather than taking the branch. A pending value on the *right* of
`&&` or `||` is the result when the left side does not decide it
(`true && u` and `false || u` are pending), and a deciding left side still
short-circuits past it (`false && u` is `false`). Count arguments, such as
the `n` of `repeat(s, n)` or `range(n)`, are like any other native argument:
the result is the pending value.

```petal
let user = __pending("user")
let override = false
if user.admin || override then print("admin panel") end
print(is_pending(user.admin && true))   // true
print(is_pending(user.greet()))         // true
```

## Lists and records hold pending values element by element

`[1, u, 3]` is an ordinary list with a pending element, not a pending list.
`len` is 3, `map` calls its function on each element, and `xs[1]` is the
pending value. An operation that has to read every element (`sum`, `join`,
`sort`) is pending if any element is. A list of twenty cards in which one
fetch is slow draws nineteen.

```petal
let u = __pending("slow")
let xs = [1, u, 3]
print(len(xs), xs[0])                // 3 1
print(is_pending(xs[1]))             // true
print(is_pending(sum(xs)))           // true
```

## Inspecting one

These receive the pending value itself and return ordinary values. They are
global and need no import.

| Builtin | Answers |
|---|---|
| `is_pending(x)` | `x` is pending at all, loading or failed |
| `is_ready(x)` | `x` is usable: the negation of `is_pending` |
| `is_loading(x)` / `is_error(x)` | `x` is pending and still loading, or pending and failed |
| `error_of(x)` | the error of a failed resource, else `nil` |
| `x ?? fallback` | `fallback` when `x` is pending (or `nil`); the right side is evaluated only then |
| `or_else(x, fallback)` | the same as a call, both arguments evaluated |
| `resource_key(x)` | the resource's cache key as an int, the same for every pending value of one resource; `nil` for a value that is not pending |

```petal
let pic = __pending("avatar")
__reject("avatar", "404")
print(is_error(pic), error_of(pic), is_loading(pic))   // true 404 false
```

The usual shape is a fallback where the value is used, and a branch where the
whole region depends on it:

```petal ignore
import ui
let user = query("user", id)
if is_loading(user) then
  spinner(10, 10)
elsif is_error(user) then
  draw_text("could not load: {error_of(user)}", 10, 10)
else
  draw_text(user.name, 10, 10)
end
draw_text(user.title ?? "…", 10, 30)
```

## `state` waits for the value

A `state` whose initializer is pending is not committed. The slot stays
empty, reading it gives the pending value, and the initializer runs again on
the next frame, until it gives a value that is not pending. So
`state user = fetch(url)` means "keep the user once it arrives":

```petal
state frame = 0
frame += 1
state user = __pending("user")
print("frame {frame}:", user ?? "loading")
if frame == 2 then __resolve("user", "Ada") end
```

```
$ petal run --runs 4 user.ptl
frame 1: loading
frame 2: loading
frame 3: Ada
frame 4: Ada
```

The rule is per slot, and `state` is keyed per call path
([inline state](inline-state.md)), so a helper with
`state u = query("user", id)` called from a loop over ten ids makes ten
slots, each committing when its own resource lands. Writing a pending value
into a `state` that is already committed (`s = u`) is allowed.

A resource resolves *between* frames, never during one. A pending value a
script already holds stays pending for the rest of that run, even if the host
resolves it meanwhile; the next run asks again and gets the value.

## Why is this region blank?

A pending value is never shown as `nil`. A state dump or an observation
dump writes it as a `{"type": "pending", …}` object with its state and origin.
Two commands report what is pending after a run:

```
$ petal pending-report user.ptl
Pending resources (1):
  loading 0f  absorbed 0x  __pending("user")
```

`petal run --trace-pending` (or `PETAL_TRACE_PENDING=1`) prints the same
report as JSON to stderr after the run, and records every absorption, which is
off by default because it costs memory on a busy frame. Each entry has the
resource's state, its age in frames, how many operations it absorbed and the
call that created it. The MCP server's `PendingReport` tool and the debug
protocol's `pending_report` command return the same report
([CLI](../CLI.md#pending-report--report-live-pending-resources),
[debug protocol](../dev/debug-protocol.md)).

## With the reactive layers

The frame gate counts a resource that changes state as a reason to run, so a
gated host re-runs the frame after a resolution even though no input moved
([frame gate](../dev/frame-gate.md)). A memoized scope whose result is pending
is treated as effectful and runs again ([memo scopes](../dev/memo-scopes.md)).
A fork shares nothing with its source but sees the same resource states it had
when it was forked ([time travel](time-travel.md)).

## Limits

- **Waterfalls.** `query("team", query("user", id).team_id)` cannot start the
  team fetch until the user has resolved, so each frame advances one stage.
- **No frame loop, no retry.** Under plain `petal run` and in tests a script
  sees the pending value and the run ends; only a host that runs frames
  resolves anything.
- **Only Garden fetches.** No other host has a fetcher; `host_data` returns
  `nil` while unavailable.
