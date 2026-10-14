# Host handles

A handle is a Petal value that refers to an object the engine owns: an actor,
an entity, a texture, a sound. The script holds the reference and calls
methods on it; the object itself never leaves the engine.

```petal ignore
let hero = spawn("hero")      // a host native that returns a handle
hero.move_by(3.0, 4.0)        // dispatched to the engine
print(hero.x())               // 3.0
print(is_valid(hero))         // true, until the engine destroys the entity
```

On the Rust side a handle is `Value::Handle(HandleVal { class, slot, serial })`:
three integers, no pointer. The engine decides what they address.

This page covers handles from both sides: what the engine registers, and what
a script can do with the value. The rest of the Rust embedding API is in
[usage/ffi.md](../usage/ffi.md).

- [Why handles exist](#why-handles-exist)
- [The model: class, slot, serial](#the-model-class-slot-serial)
- [Registering a class and making handles](#registering-a-class-and-making-handles)
- [Reading a handle in a native](#reading-a-handle-in-a-native)
- [What a script can do with a handle](#what-a-script-can-do-with-a-handle)
- [Where a handle can be kept](#where-a-handle-can-be-kept)
- [Effect rows for natives that use handles](#effect-rows-for-natives-that-use-handles)
- [From C and C++](#from-c-and-c)
- [A worked example](#a-worked-example)
- [Limits](#limits)

## Why handles exist

Every other value crosses the boundary between Petal and the host **by
value**. A native that returns a record builds a new record on Petal's heap; a
binding the host sets is a copy; what the host drains from an output buffer is
a snapshot. That suits commands and per-frame data, and it is why a script can
be re-run, replayed and forked freely.

It does not suit an engine object. An actor is large, it changes under the
script between frames (physics moves it, another system destroys it), and the
engine has to stay its only owner. Copying it into a record gives the script a
stale picture and no way to act on the original.

A handle is the one value that crosses **by reference**. The script gets a
small opaque token; every read and write goes back through the engine, which
looks the object up in its own table.

For resources that live for a single frame (an offscreen canvas, a DOM id) a
plain integer id is enough: see
[Integer ids](../usage/ffi.md#integer-ids). Use a handle when the object
outlives the frame and can be destroyed while a script still refers to it,
because a handle is typed and its staleness is detected.

## The model: class, slot, serial

```rust
pub struct HandleClassId(pub u16);

pub struct HandleVal {
    pub class: HandleClassId,
    pub slot: u32,
    pub serial: u32,
}
```

- **class** says what kind of object this is. The host registers a
  `HandleClass` per kind (`Entity`, `Texture`) and gets a `HandleClassId`
  back. The class carries the callbacks Petal uses to ask about an object and
  to call methods on it.
- **slot** and **serial** are the address of one object inside the host's
  storage. Petal never interprets them. It passes them back to the class's
  callbacks unchanged.

The intended storage is a slot map with generation counters: `slot` is an
index into a table, and `serial` is a counter the host bumps every time that
slot is given to a new object. A handle to a destroyed object then still names
a slot, but with an old serial, and the host can tell.

**Staleness is the host's answer, asked each time.** Petal keeps no list of
live handles and is never told that an object died. Whenever it matters, Petal
calls the class's `is_valid(slot, serial)`:

- before a method call on a handle,
- when a native reads a handle argument with `get_handle`,
- when a script calls the `is_valid` builtin.

A handle whose object is gone is called **stale**. A stale handle is still an
ordinary value: it can be stored, compared and printed. Only using it to reach
the object fails.

A handle to a reused slot is not equal to the old one, because the serial
differs:

```petal ignore
let a = spawn("a")
print(a)                // handle(0:1#1)
a.destroy()
let b = spawn("b")      // the host reuses slot 1
print(b)                // handle(0:1#2)
print(a == b, is_valid(a), is_valid(b))   // false false true
```

## Registering a class and making handles

```rust
use petal::{HandleClass, HandleClassId, HandleVal};

pub struct HandleClass {
    pub name: String,
    pub is_valid: Box<dyn Fn(u32, u32) -> bool>,
    pub describe: Box<dyn Fn(u32, u32) -> String>,
    pub call_method: Box<dyn Fn(&mut PetalCxt, &str) -> NativeResult>,
}

impl Env {
    pub fn register_handle_class(&mut self, class: HandleClass) -> HandleClassId;
    pub fn make_handle(&self, class: HandleClassId, slot: u32, serial: u32) -> Value;
    pub fn handle_classes(&self) -> &[HandleClass];
}
```

| Field | Called with | Purpose |
|---|---|---|
| `name` | | The class name used in error messages (`Stale Entity handle: …`). |
| `is_valid` | `(slot, serial)` | Whether that address still refers to a live object. |
| `describe` | `(slot, serial)` | Text for a stale-handle error. It is called for an object that is gone, so it must not assume the slot is live. |
| `call_method` | `(cxt, method_name)` | Runs `h.method(args)`. See [Method calls](#method-calls). |

The callbacks are boxed closures, so they capture whatever shared access to
the engine's table they need (an `Rc<RefCell<World>>`, a raw pointer the host
keeps valid).

`register_handle_class` may be called at any time, before or after a program
is loaded. Compiled programs do not refer to handle classes, unlike natives,
which must be registered before the load that uses them. Ids are handed out
in registration order starting at 0, and a class cannot be removed.

There are three ways to get a handle to a script:

```rust
// 1. Return one from a native.
cxt.push_value(Value::Handle(HandleVal { class: entity, slot, serial }));

// 2. Set one as a binding, which a script reads with `binding(symbol("player"))`
//    after `import io`.
let sym = env.intern_symbol("player");
env.set_binding(sym, env.make_handle(entity, slot, serial));

// 3. Pass one as an argument to `Env::call_function`.
```

`make_handle` only builds the value. It does not check that the class is
registered or that the object exists.

## Reading a handle in a native

```rust
impl PetalCxt<'_> {
    pub fn get_handle(&self, index: usize, class: HandleClassId) -> Result<HandleVal, String>;
    pub fn handle_class(&self, id: HandleClassId) -> Option<&HandleClass>;
}
```

`get_handle(index, class)` reads argument `index` (1-based) and checks three
things, each with its own error:

| The argument is | Error |
|---|---|
| not a handle | `Expected Entity handle at arg 2, got int` |
| a handle of another class | `Expected Entity handle at arg 2, got Texture handle` |
| stale | `Stale Entity handle at arg 2: <describe output>` |

Returning the error with `?` raises it in the script at the call. A native
that wants to accept a stale handle (a `destroy` that is safe to call twice,
say) reads the argument with `get_value` and matches `Value::Handle(h)`
itself.

## What a script can do with a handle

### Method calls

`h.method(args)` on a handle calls the class's `call_method`:

```rust
call_method: Box::new(move |cxt: &mut PetalCxt, method: &str| -> NativeResult {
    let Value::Handle(h) = cxt.get_value(1)? else { unreachable!() };
    match method {
        "x" => { cxt.push_float(world.borrow().x(h.slot)); Ok(1) }
        "move_by" => {
            let (dx, dy) = (cxt.get_float(2)?, cxt.get_float(3)?);
            world.borrow_mut().move_by(h.slot, dx, dy);
            Ok(0)
        }
        other => Err(format!("Entity has no method '{other}'")),
    }
}),
```

- The receiver is argument 1 and the call's own arguments follow from 2.
- The callback returns as a native does: push the result and return `Ok(1)`,
  or return `Ok(0)` for `nil`.
- **Liveness is checked first.** If `is_valid` says no, the call raises
  `Stale Entity handle: <describe output>` and `call_method` is not invoked.
  The receiver a `call_method` sees is always live.
- **The class wins over natives.** A method call on a handle goes to
  `call_method` before any native or builtin of that name is considered, so a
  class method named `get` or `len` is the class's. It also means no builtin
  is reachable through method syntax on a handle: `h.is_valid()` asks the
  class for a method called `is_valid`. Write `is_valid(h)`.
- An unknown method is whatever `call_method` returns for it. Return an `Err`
  that names the class and the method.
- Named arguments are rejected (`h.move_by(dx: 1.0)` is a runtime error): a
  class dispatches by name inside one callback and has no parameter list to
  bind against.
- A handle has no fields. `h.name` without parentheses is the error
  `Cannot access field 'name' on handle`.

### `is_valid`

```petal
print(is_valid(nil))   // false
print(is_valid(5))     // false
```

`is_valid(v)` is true only for a handle of a registered class whose object
the host says is alive. It never raises: `nil`, any value that is not a
handle, and a stale handle are all `false`. Use it to guard a handle that the
engine may have destroyed since the script stored it:

```petal ignore
if is_valid(target) then
    target.take_damage(10)
end
```

A handle is always truthy, stale or not, so `if target then` does not do this
job.

### Comparison and keys

Two handles are equal when their class, slot and serial are all equal. Nothing
is asked of the host, so equality works on stale handles and two handles to
the same object made at different times are equal.

```petal ignore
let a = spawn("a")
let b = spawn("b")
print(a == a, a == b, a == nil)             // true false false
print(contains([a, b], a), index_of([a, b], b))   // true 1
```

Handles have no order: `a < b` is the error `Cannot compare handle and handle`.

A handle hashes by the same three numbers, so it works as a
[`state(key)`](../features/inline-state.md#keyed-state-statekey) key. This
gives each entity its own slot of script-side state:

```petal ignore
fn hp(e)
    state(e) v = 100
    v -= 10
    v
end
print(hp(a), hp(a), hp(b))    // 90 80 90
```

On the Rust side `HandleVal` is `Copy`, `Eq` and `Hash`, so it is usable as a
`HashMap` key directly.

### Printing and `type`

`type(h)` is `"handle"`. A handle prints as `handle(class:slot#serial)`, with
the class as its numeric id:

```petal ignore
print(h)            // handle(0:1#1)
print("{h}")        // handle(0:1#1)
print([h, h])       // [handle(0:1#1), handle(0:1#1)]
print(type(h))      // handle
```

The printed form does not use the class name or `describe`. For a readable
name, give the class a method that returns one.

### The `handle` type annotation

`handle` is a type name in
[annotations](../language-guide.md#type-annotations):

```petal
fn nudge(e: handle)
    e.move_by(1.0, 0.0)
end
```

It means any handle. There is no annotation for one handle class, so the
checker cannot tell an `Entity` from a `Texture`; that check happens in
`get_handle` at run time. As with other annotations, a mismatch is a checker
warning (``argument 1 to `nudge`: expected `handle`, found `int` ``) and the
program still runs. `nil` is not a `handle`.

## Where a handle can be kept

| Place | Works? | Notes |
|---|---|---|
| `let`, function arguments and results | yes | |
| inside a list, a record, an enum payload | yes | |
| `state` | yes, within one process | Kept across runs and hot reloads. |
| `state(h)` as the key | yes | |
| state inspection JSON (`get_state_json`) | as text | Shown as the string `"handle(0:1#1)"`. |
| state storage (`save_state_storage`, `petal run --state-storage`) | no | The slot is skipped and reported. |
| `const` and the static-value cache | no | Rebuilt on every run, with a warning. |

### In `state`

A handle in a `state` variable is kept from one run to the next like any
other value, alone or inside a list or record. This is the usual way for a
script to own an engine object: create it once, keep the handle.

```petal ignore
state hero = spawn("hero")    // spawn runs on the first run only
hero.move_by(1.0, 0.0)        // every run
```

The object can die while the handle is kept. The next run still reads the
handle from `state`, and the first method call on it raises
`Stale Entity handle: …`. Guard with `is_valid`, or reset the slot.

### In state JSON and state storage

The two JSON surfaces for state treat a handle differently.

`Env::get_state_json` is the inspection view. A handle appears as its printed
form, a string:

```json
{"hero": "handle(0:0#1)", "pair": {"e": "handle(0:0#1)", "n": 0}}
```

That string is for reading. Setting it back does not make a handle.

`Env::save_state_storage` (what `petal run --state-storage` writes) saves only
values that another process can rebuild, and a slot/serial pair means nothing
to a new engine instance. A state variable whose value is or contains a handle
is left out of the document and listed in `StateStorageSave::skipped` as
`(name, "a host handle")`. The whole variable is skipped, not only the handle
inside it. On the next process the declaration starts from its initializer
again, which for `state hero = spawn("hero")` is what you want.

### Across a hot reload

A [hot reload](../features/hot-reload.md) keeps `state`, so it keeps the
handles in it. The handle classes live on the `Env`, not in the program, and
are untouched by a reload; the engine's objects are untouched too. In the
example above, editing `move_by(1.0, 0.0)` to `move_by(10.0, 0.0)` and
reloading leaves `hero` pointing at the same entity, which simply starts
moving faster. `spawn` does not run again.

### In a `const` and the static-value cache

A handle cannot be a kept static value. Two separate things stop it.

First, a native that makes a handle answers from host state, so it must not be
registered as a constant native
([Effect rows](#effect-rows-for-natives-that-use-handles)). With an honest
row, a `const` built from it is a load-time diagnostic:

```text
error: `BOSS` is declared `const` but is not constant
  = BOSS calls spawn, which is not declared constant
```

Second, if a handle does end up in a top-level value that is otherwise static
(the native was wrongly registered `CONST`), the static-value cache refuses to
keep it. The value is rebuilt on every run, and
`Env::take_static_warnings(stack)` reports it once:

```text
const BOSS (line 4) holds a host handle, which cannot be kept between runs, so it is rebuilt on every run
```

"Rebuilt on every run" means the native is called on every run. For a native
that spawns, that is a new entity each frame. Keep engine objects in `state`,
and treat this warning as a wrong effect row.

[features/static-values.md](../features/static-values.md) describes static
values and `const`, and [Constant natives](../usage/ffi.md#constant-natives)
the `CONST` row.

### In memoized scopes

Under the `fast` and `replay` run policies a function call whose arguments and
reads did not change can be replayed from a record instead of run
([memo-scopes.md](../dev/memo-scopes.md)). A replay calls no native and no
handle method. Petal accounts for this in two ways:

- **A handle method call is always treated as an effect.** A function that
  calls `h.method(...)` is never recorded, and neither is any function that
  called it. So `fn patrol(e) e.move_by(1.0, 0.0) end` reaches the engine on
  every run, even though its argument is the same handle each time.
- **A handle argument compares by identity.** For a function that takes a
  handle but calls only natives on it, the memo sees an unchanged argument
  when the handle is the same triple. Whether the call is replayed then
  depends on the effect rows of those natives, which is the next section.

## Effect rows for natives that use handles

Method calls need no declaration. Natives do: every native is registered with
a [`NativeEffects` row](../usage/ffi.md#declaring-native-effects-honestly),
and the row has to say that the native reaches host state through the handle.
The handle is the same value every frame while the object behind it changes,
so "computes from its arguments" is never true of a native that looks the
object up.

| The native… | Row |
|---|---|
| creates an object and returns its handle (`spawn`) | `EFFECT` |
| changes or destroys the object (`set_pos(e, …)`, `destroy(e)`) | `EFFECT` |
| reads the object (`distance(a, b)`, `pos(e)`), in an engine whose objects change without telling Petal | `EFFECT` |
| reads the object, and the host calls `Env::note_host_data_changed(stack)` whenever objects change | `reads(InputClasses::HOST_DATA)`, with `cxt.note_host_read()` in the native |
| only inspects the handle value, never the object (returns `h.slot`) | `PURE` |
| any of the above | never `CONST` |

`get_handle` itself asks the host whether the object is alive, so a native
that calls it has read host state even if it does nothing else.

The builtin `is_valid` is registered `PURE` and not `CONST`: the same handle
answers differently once the host drops the object, so a value computed from
`is_valid` is never a static value.

`petal run --effect-audit` and `Env::set_effect_audit` check a row against
what a native does through `PetalCxt`. They cannot see what a closure does to
its captured engine table, so for handle natives the table above is the rule.

## From C and C++

`petal-c-bridge` does not expose handle classes. There is no `pb_*` function
to register a class, and `pb_builder` has no call that builds a handle, so a
host written only against `petal_bridge.h` or `petal.hpp` cannot give a script
a handle.

What the bridge does have is the decode side. If a handle reaches the host in
a decoded value (a native argument, a drained buffer, a call result), it
arrives as `PB_HANDLE`:

| | C (`pb_value`) | C++ (`petal::Value`) |
|---|---|---|
| is it a handle | `kind == PB_HANDLE` | `is_handle()` |
| slot | `integer` | `integer()` |
| serial | `count` | not wrapped; read the `pb_value` |
| class name | `str` / `str_len` | `str()` |

A C or C++ engine has two options:

- **Integer or string ids.** Pass an id by value and keep the table
  host-side. Nothing detects a stale id, so the engine's natives must check
  it. This is what a pure C host does today.
- **A small Rust layer.** A crate that depends on `petal-c-bridge` as an
  `rlib` can reach the VM's `Env` with `vm.env_mut()` and call
  `register_handle_class` with callbacks that forward to the engine's own C
  functions. The decode side above then gives the engine's C callbacks the
  slot and serial of each handle argument. See
  [Layering an engine's own C ABI in Rust](../usage/embedding-c.md#layering-an-engines-own-c-abi-in-rust).

## A worked example

An engine with an entity table, a handle class over it, and two natives. The
table is a slot map: a destroyed slot is reused, and its serial goes up.

```rust
use std::cell::RefCell;
use std::rc::Rc;

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::value::Value;
use petal::{HandleClass, HandleVal};

#[derive(Default)]
struct Entity { serial: u32, live: bool, name: String, x: f64, y: f64 }

#[derive(Default)]
struct World { slots: Vec<Entity> }

impl World {
    fn spawn(&mut self, name: &str, x: f64, y: f64) -> (u32, u32) {
        let slot = match self.slots.iter().position(|e| !e.live) {
            Some(free) => free,
            None => { self.slots.push(Entity::default()); self.slots.len() - 1 }
        };
        let e = &mut self.slots[slot];
        *e = Entity { serial: e.serial + 1, live: true, name: name.into(), x, y };
        (slot as u32, e.serial)
    }
    fn is_live(&self, slot: u32, serial: u32) -> bool {
        self.slots.get(slot as usize).is_some_and(|e| e.live && e.serial == serial)
    }
}

fn main() {
    let world = Rc::new(RefCell::new(World::default()));
    let mut env = Env::new();

    // The handle class.
    let (w1, w2) = (world.clone(), world.clone());
    let entity = env.register_handle_class(HandleClass {
        name: "Entity".into(),
        is_valid: Box::new(move |slot, serial| w1.borrow().is_live(slot, serial)),
        describe: Box::new(|slot, serial| format!("entity {slot}#{serial} (destroyed)")),
        call_method: Box::new(move |cxt: &mut PetalCxt, method: &str| -> NativeResult {
            // Arg 1 is the receiver, already checked for liveness.
            let Value::Handle(h) = cxt.get_value(1)? else { unreachable!() };
            let mut world = w2.borrow_mut();
            let e = &mut world.slots[h.slot as usize];
            match method {
                "move_by" => { e.x += cxt.get_float(2)?; e.y += cxt.get_float(3)?; Ok(0) }
                "x" => { cxt.push_float(e.x); Ok(1) }
                "name" => { let name = e.name.clone(); cxt.push_string(name); Ok(1) }
                "destroy" => { e.live = false; Ok(0) }
                other => Err(format!("Entity has no method '{other}'")),
            }
        }),
    });

    // spawn(name, x, y) -> handle. Creates an engine object: an effect.
    let w = world.clone();
    env.register_native_boxed("spawn", move |cxt: &mut PetalCxt| {
        let (name, x, y) = (cxt.get_string(1)?, cxt.get_float(2)?, cxt.get_float(3)?);
        let (slot, serial) = w.borrow_mut().spawn(&name, x, y);
        cxt.note_effect();
        cxt.push_value(Value::Handle(HandleVal { class: entity, slot, serial }));
        Ok(1)
    }, NativeEffects::EFFECT);

    // distance(a, b) -> float. Reads positions that move every frame: an effect.
    let w = world.clone();
    env.register_native_boxed("distance", move |cxt: &mut PetalCxt| {
        let a = cxt.get_handle(1, entity)?;      // class and liveness checked
        let b = cxt.get_handle(2, entity)?;
        let world = w.borrow();
        let (a, b) = (&world.slots[a.slot as usize], &world.slots[b.slot as usize]);
        cxt.note_effect();
        cxt.push_float(((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt());
        Ok(1)
    }, NativeEffects::EFFECT);

    let pid = env.load_program(SCRIPT).unwrap();
    let stack = env.create_stack(pid).unwrap();
    for _frame in 0..3 {
        env.reset_stack(stack).unwrap();
        env.run(stack).unwrap();                 // `print` goes to stdout
    }
}
```

The script. Natives must be declared with
[`native fn`](../usage/ffi.md#native-interfaces-native-fn); handle methods are
not declared anywhere.

```petal ignore
native fn spawn(name, x, y)
native fn distance(a, b)

state hero = spawn("hero", 0.0, 0.0)      // created on the first frame only
state target = spawn("target", 3.0, 0.0)

fn patrol(e: handle, speed)
    e.move_by(speed, 0.0)
end

patrol(hero, 1.0)
if is_valid(target) && distance(hero, target) < 2.0 then
    target.destroy()
end
print(hero.name(), hero.x(), is_valid(target))
```

Three frames print:

```text
hero 1.0 true
hero 2.0 false
hero 3.0 false
```

The two handles live in `state`, so `spawn` runs once and the world holds two
entities for the whole session. `patrol` moves the hero every frame although
its arguments never change, because a handle method call is never replayed.
On the second frame the hero is within range and the script destroys the
target. From then on `is_valid(target)` is false, and the `&&` skips the
`distance` call, which would otherwise raise
`Stale Entity handle at arg 2: entity 1#1 (destroyed)`.

## Limits

- **No handles through the C bridge.** A C or C++ host needs a Rust layer to
  register a class ([From C and C++](#from-c-and-c)).
- **A handle does not own its object.** Petal's garbage collector treats a
  handle as a plain value and tells the host nothing when the last copy is
  gone. There is no finalizer and no reference count. The engine decides when
  an object dies, and scripts find out through `is_valid`.
- **Staleness is only as good as `is_valid`.** If the host reuses a slot
  without changing the serial, an old handle silently refers to the new
  object.
- **No per-class types.** The annotation is `handle` for every class. A
  script cannot ask a handle for its class name, and the printed form shows
  the numeric class id.
- **No fields, no named arguments, no declared signatures.** Everything goes
  through `call_method` by name with positional arguments. The checker knows
  nothing about a class's methods, so a misspelled method is a runtime error
  from the host.
- **No ordering.** Handles support `==` and `!=` only.
- **Not serializable.** A handle is dropped from state storage and is a
  string in state inspection JSON. A script that must survive a restart
  stores its own id (a name, a save-game key) and asks the host for a handle
  again.
- **Not a static value.** A handle in a `const` or a static top-level table
  makes that value rebuild every run.
- **Classes are per `Env` and permanent.** A class cannot be unregistered,
  and a handle made for one `Env` means nothing in another. A handle whose
  class id was never registered is not valid, and a method call on it raises
  `Handle references unregistered handle class id N`.
- **Callbacks cannot call back into Petal.** Like any native, `call_method`
  cannot invoke a Petal closure.
