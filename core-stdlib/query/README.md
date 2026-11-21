# query

Async data for a script that re-runs every frame: the script side of
petal-query. An **interface package**: `src/query.ptl` declares the natives,
the Rust crate in [`native/`](native/) (`petal-stdlib-query`) implements them,
and the host decides where the data comes from.

```petal ignore
import query            # a Garden panel gets it through `import garden`

let user = query("user", id)
if is_loading(user) then
  spinner(10, 10)
else if is_error(user) then
  draw_text("could not load: {error_of(user)}", 10, 10)
else
  draw_text(user.name, 10, 10)
```

`query(kind, arg)` answers the value itself when the host has it, and until
then a **pending answer**: an opaque value that says the data is still
loading, or that loading failed. The next frame asks again, so the frame loop
is the retry loop. There are no callbacks and no `await`.

| Builtin | Answers |
|---|---|
| `query(kind, arg)` | the host's value for `(kind, arg)`, or a pending answer |
| `invalidate(kind, arg)` | drops the cached answer, so the next `query` asks again |
| `is_ready(x)` | `x` is usable: anything but a pending answer (`nil` included) |
| `is_loading(x)` | `x` is a pending answer still on its way |
| `is_error(x)` | `x` is a pending answer whose load failed |
| `error_of(x)` | the failure's message, else `nil` |

The full reference is the `@doc` on each in [`src/query.ptl`](src/query.ptl).
`ui`'s load-state helpers (`load_state`, `load_poll`, `draw_load`) wrap the
same test in a record kept in `state`, for a panel that keeps drawing the last
value while the next one loads.

## A pending answer is not data

Test it before you use it. Reading a field of one, doing arithmetic on it,
looping over it, passing it to `len` or to a draw call is the ordinary error
any host handle gets, at the line that did it:

```
cannot call .greet() on query("user", 7), still loading: test it with is_ready first
```

It is truthy, like every value but `nil` and `false`, so `if rd then` does not
test it; `is_ready(rd)` does. `??` does not either: `??` falls through `nil`
only. A pending answer cannot be saved with `state` storage (it is a host
handle), and printing one shows `handle(…)`.

## Hosts

petal-ui registers this package with everything else (`petal_ui::register_all`
and `register_prelude` call `petal_stdlib_query::register`), so every petal-ui
host has it, and `petal lint` resolves `import query` from the copy bundled in
the `petal` binary. A host that does not link petal-ui links the crate itself:

```toml
petal-stdlib-query = { path = "<petal>/core-stdlib/query/native" }
```

```rust
petal_stdlib_query::register(&mut env);   // natives, handle class, package, `provides query`
```

Where answers come from is a `QueryHost`, installed for the thread the runs
happen on. With none installed every query answers loading, so a script shows
its loading path rather than crashing.

```rust
use petal_stdlib_query::{QueryHost, ResourceAnswer};

struct MyHost { cache: MyCache }

impl QueryHost for MyHost {
    fn answer(&mut self, cx: &mut PetalCxt, kind: &str, arg: Value)
        -> Result<ResourceAnswer<Value>, String>
    {
        // Answer from what you have; start the fetch as a side effect. Never block.
        Ok(self.cache.lookup(kind, arg).map(|data| to_petal_value(cx, data)))
    }
    fn invalidate(&mut self, cx: &mut PetalCxt, kind: &str, arg: Value) -> Result<(), String> {
        self.cache.drop(kind, arg);
        Ok(())
    }
}

petal_stdlib_query::swap_host(Some(Box::new(MyHost { .. })));
```

`query` is declared a host-data read, so a host whose cache moves calls
`env.note_host_data_changed(stack)`: the frame gate then re-runs the frame,
and memoized scopes that queried are not replayed. Answers land between
frames, never during one.

Garden is the reference host: `garden/garden-script/src/query.rs` answers from
the panel's GPP app through a `petal_query::Cache` (`core-sdk/petal-query`),
which owns the cache policy (fresh, stale-while-revalidate, expired).

**Other natives** can hand a script a pending answer too:
`push_answer(cx, name, ResourceAnswer<Value>)`, or `push_loading` /
`push_failed`. `name` is what error messages and the settle loop call it
(`image("logo.png")`). `ResourceProvider` is a small trait for a keyed source
that answers `ResourceAnswer`s.

**A batch host** (a report builder, a test, a scraper) has no frame loop to
retry for it. `run_until_settled(env, stack, max_runs, resolve)` runs the
script, hands `resolve` the names it asked for and was answered loading, and
runs again until a run asks for nothing new. A run that fails on a loading
answer it used too early is retried once its requests are answered.

## Why this is a library

Pending values used to be part of the core language: an absorbing value that
made every operation that touched it pending too (arithmetic, comparisons,
field reads, `if`/`while`/`for`/`match`, draw calls), with a `state` rule that
did not commit a pending initializer, and a resource table, absorption log and
`pending-report` tooling in the VM. Every real script that used them tested
the answer at the call site (`is_ready` / `is_error`) and used it only once it
was ready; none relied on absorption, and none used `state x = query(...)`.
Absorption put a check on the VM's hot paths and the frame gate, memo and
static caches all had to reason about it, while its known failure mode was a
silently blank region. So the language keeps no notion of "not here yet". The
part scripts use is this package, and a value used too early is an error at
the line that used it.

Still open, as library work: batching every key a frame asks for (Haxl-style)
so that `query("team", query("user", id).team_id)` does not take one frame per
stage.
