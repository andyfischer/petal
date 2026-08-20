//! The cart-facing native set, and the buffered command protocol behind it.
//!
//! Natives here never touch the PPU or the APU directly. A native is a bare
//! `fn` pointer with no host context (see `docs/ffi.md`), so instead every call
//! *emits* a tagged command into a per-stack output buffer, and the host drains
//! that buffer after the script has run and applies it to the console state.
//!
//! That indirection is not just an FFI workaround — it is what makes the
//! speculative run modes correct. `--screenshot`'s final capture and the
//! agent's `screenshot` command run the cart on a *forked* stack whose effects
//! are discarded; because their video writes sit in the fork's own buffer, the
//! host can apply them to a throwaway `Ppu` clone and leave the live console
//! untouched. A native that mutated a global would corrupt the running game
//! every time an agent took a picture.
//!
//! Three buffers, one per subsystem, each drained by its own module:
//! `nes_video` -> [`video`], `nes_audio` -> [`audio`], and the system channels
//! (`launch_cart`, presentation requests) -> [`system`].

pub mod audio;
pub mod system;
pub mod video;

use petal::env::Env;
use petal::heap::Heap;
use petal::native_fn::PetalCxt;
use petal::stack::StackKey;
use petal::value::Value;

/// Register every native a cart can call. Input natives (`key_down`, `dt`,
/// `frame_count`, …) come from `petal_ui::input` and are registered by the
/// host alongside these.
pub fn register_all(env: &mut Env) {
    video::register_video(env);
    audio::register_audio(env);
    system::register_system(env);
}

// ── The command protocol ──────────────────────────────────────────────────

/// One emitted command: a tag plus its already-decoded arguments.
///
/// Arguments are decoded off the heap at drain time rather than handed around
/// as raw `Value`s, so applying a command needs no heap access and the fork
/// path does not have to keep a borrow of the fork's heap alive.
#[derive(Debug, Clone)]
pub struct Command {
    pub tag: String,
    pub args: Vec<Arg>,
}

/// A decoded command argument. Only the shapes a cart can actually pass to a
/// native are represented; anything else decodes as [`Arg::Nil`].
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Nil,
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Arg>),
}

impl Arg {
    /// Numeric read that accepts either an int or a float, because Petal
    /// arithmetic promotes freely and a cart should not have to care.
    pub fn as_i64(&self) -> i64 {
        match self {
            Arg::Int(n) => *n,
            Arg::Float(f) => *f as i64,
            _ => 0,
        }
    }

    pub fn as_f32(&self) -> f32 {
        match self {
            Arg::Int(n) => *n as f32,
            Arg::Float(f) => *f as f32,
            _ => 0.0,
        }
    }

    /// Petal has no bool-only convention in these natives: `0`/`false`/absent
    /// are all off, anything else is on.
    pub fn as_bool(&self) -> bool {
        match self {
            Arg::Int(n) => *n != 0,
            Arg::Float(f) => *f != 0.0,
            Arg::Str(s) => !s.is_empty(),
            Arg::List(_) => true,
            Arg::Nil => false,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Arg::Str(s) => s,
            _ => "",
        }
    }

    pub fn as_list(&self) -> &[Arg] {
        match self {
            Arg::List(items) => items,
            _ => &[],
        }
    }
}

impl Command {
    /// Positional argument accessor. Out-of-range reads give [`Arg::Nil`] so
    /// an optional trailing argument (the 5-argument `sprite`) needs no arity
    /// branch at the apply site.
    pub fn arg(&self, i: usize) -> &Arg {
        self.args.get(i).unwrap_or(&Arg::Nil)
    }

    pub fn i64(&self, i: usize) -> i64 {
        self.arg(i).as_i64()
    }

    pub fn u8(&self, i: usize) -> u8 {
        self.i64(i).clamp(0, 255) as u8
    }

    pub fn i32(&self, i: usize) -> i32 {
        self.i64(i).clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    pub fn usize(&self, i: usize) -> usize {
        self.i64(i).max(0) as usize
    }

    pub fn f32(&self, i: usize) -> f32 {
        self.arg(i).as_f32()
    }

    pub fn bool(&self, i: usize) -> bool {
        self.arg(i).as_bool()
    }

    pub fn str(&self, i: usize) -> &str {
        self.arg(i).as_str()
    }
}

/// Emit a command into `channel` from inside a native.
pub fn emit(cxt: &mut PetalCxt, channel: &str, tag: &str, args: Vec<Value>) {
    let sym = cxt.intern_symbol(channel);
    cxt.emit(sym, tag, args);
}

/// Collect the first `n` arguments (1-indexed, Lua-style) as numbers, the
/// common shape for these natives. Missing arguments become 0 rather than an
/// error, so one native registered at the widest arity serves every overload
/// (`sprite` with and without `flags`). Floats are preserved — a scroll or a
/// note may legitimately be fractional — and bools fold to 0/1 so
/// `set_sprite_limit(true)` reads the same as `set_sprite_limit(1)`.
pub fn opt_nums(cxt: &PetalCxt, n: usize) -> Vec<Value> {
    (1..=n)
        .map(|i| match cxt.get_value(i) {
            Ok(Value::Bool(b)) => Value::Int(b as i64),
            Ok(v @ (Value::Int(_) | Value::Float(_))) => v,
            _ => Value::Int(0),
        })
        .collect()
}

/// Drain `channel` on the live stack.
pub fn take_commands(env: &mut Env, channel: &str) -> Vec<Command> {
    let sym = env.intern_symbol(channel);
    let values = env.take_output_buffer(sym);
    decode_all(&values, env.heap())
}

/// Drain `channel` on a speculative fork. Returns nothing if the fork is gone.
pub fn take_commands_for(env: &mut Env, stack: StackKey, channel: &str) -> Vec<Command> {
    let sym = env.intern_symbol(channel);
    let values = env.take_output_buffer_for(stack, sym);
    match env.heap_for(stack) {
        Some(heap) => decode_all(&values, heap),
        None => Vec::new(),
    }
}

fn decode_all(values: &[Value], heap: &Heap) -> Vec<Command> {
    values
        .iter()
        .filter_map(|v| match v {
            Value::EnumVariant { tag, data } => Some(Command {
                tag: heap.get_string(*tag).to_string(),
                args: heap
                    .get_list(*data)
                    .iter()
                    .map(|a| decode(a, heap))
                    .collect(),
            }),
            _ => None,
        })
        .collect()
}

fn decode(v: &Value, heap: &Heap) -> Arg {
    match v {
        Value::Int(n) => Arg::Int(*n),
        Value::Float(f) => Arg::Float(*f),
        Value::Bool(b) => Arg::Int(*b as i64),
        Value::String(id) => Arg::Str(heap.get_string(*id).to_string()),
        Value::List(id) => Arg::List(heap.get_list(*id).iter().map(|e| decode(e, heap)).collect()),
        _ => Arg::Nil,
    }
}
