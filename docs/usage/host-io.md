# Host I/O from a script

A script talks to the program running it through two channels: it reads
values the host **bound** before the run, and it **pushes** values into named
output buffers that the host drains after the run. Both are named by a
**symbol**. The three builtins that do it are in `import io`:

```petal
import io
let dt = binding(symbol("dt")) ?? 0.016     // host → script
push_output(symbol("shapes"), {kind: "dot", x: 10, y: 20})   // script → host
```

This page is the script's side. The host's side, binding values and draining
buffers from Rust, is in [embedding-guide.md](embedding-guide.md) and
[ffi.md](ffi.md#the-three-host-channels); from C, in
[embedding-c.md](embedding-c.md#the-model). Most scripts never call these
directly: a host's interface package wraps them (the `ui` package's `dt()` and
`draw_*` calls are bindings and buffers underneath), and a script imports that
instead.

## `symbol(name)`

An interned name. Two calls with the same name give the same symbol, in the
script and in the host (`Env::intern_symbol`), which is how both sides agree
on a channel. `type(symbol("x"))` is `"symbol"`, and symbols compare with
`==`.

```petal
import io
print(type(symbol("shapes")), symbol("shapes") == symbol("shapes"))   // symbol true
```

A symbol prints as `symbol#<id>`, not as its name. `symbol` is a constant
function, so `symbol("shapes")` at the top level is built once.

## Reading what the host bound: `binding(sym)`

The value the host bound to `sym` for this run, or `nil` when it bound
nothing. Hosts bind per-frame inputs this way: the frame time, the screen size,
an input snapshot, a model record.

```petal
import io
let speed = binding(symbol("speed")) ?? 1.0
print(speed)                                 // 1.0
```

Under `petal run` nothing is bound, so give every binding a default with
`??`. A binding is read fresh on every run; to keep something across runs,
copy it into a `state`.

The frame gate records which bindings a run read and compares their values on
the next frame, so a script whose bindings did not change can be skipped
([frame gate](../dev/frame-gate.md)). Reading a binding is cheap, but read only
the ones the frame needs.

## Sending values to the host: `push_output(sym, value)`

Appends `value` to the buffer `sym` names and returns `nil`. The host drains
the buffer after the run (`Env::take_output_buffer`, `vm.drain(name)` in C++),
in the order the values were pushed. A buffer is the host's to name and to
interpret: a list of shapes to draw, events to raise, sounds to play.

```petal
import io
let out = symbol("events")
for i in range(0, 3) do
  push_output(out, {kind: "spawn", id: i})
end
```

- **What is pushed is a value.** Lists and records are immutable, so a later
  change to a binding the script pushed does not reach the host.
- **A pending value pushes nothing.** `push_output(sym, u)` with a
  [pending](../features/pending-values.md) `u` is a no-op, like a draw call.
- **Replay keeps it.** A pushed value is the one kind of output a memoized
  call can replay without running, so a helper that only computes and pushes
  stays cheap on frames where its inputs have not changed
  ([memo scopes](../dev/memo-scopes.md)).
- **Nothing drains it under `petal run`.** The values are dropped at the end
  of the run. `petal run --trace-emits` prints every buffer with the call that
  pushed each value, which is the quickest way to see what a host would get
  ([direct manipulation](../features/direct-manipulation.md#tracing-live-code-from-the-cli-and-mcp)).

`print` is a separate channel: the host collects its lines with
`take_output`, and `petal run` writes them to stdout.

## In a module

`import io` puts the three in scope in the file that writes it, bare
(`symbol(…)`), qualified (`io.symbol(…)`) and as methods
(`symbol("log").push_output(x)`). Each file that calls them imports `io`.

## See also

- [Builtins: Host I/O](../Builtins.md#host-io), the signatures.
- [The frame contract](../game-engine-features/frame-contract.md), what a
  host does around each run.
- [Module system: host interface packages](../module-system.md#host-interface-packages),
  how a host gives a script its own natives in place of raw buffers.
