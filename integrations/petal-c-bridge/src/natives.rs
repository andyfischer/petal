//! Host natives: C callbacks and emitters callable from Petal by name.
//!
//! Each host native is one boxed Petal native
//! ([`Env::register_native_boxed`]): a closure that owns what the C host
//! registered — the callback, its userdata and the userdata's free function
//! (a [`HostCallback`]), or the buffer and tag of an emitter. The closure
//! belongs to the VM's `Env`, so
//!
//! - dispatch state is per VM (two VMs registering the same name never
//!   share anything, and nothing is global);
//! - the userdata is freed exactly once, when the `Env` drops the closure
//!   ([`HostCallback`]'s `Drop`), i.e. on `pb_vm_destroy`;
//! - there is no fixed cap on how many natives a VM can register.
//!
//! A forked or speculative execution of the same `Env` calls the same
//! closure, as for any boxed native.

use std::cell::RefCell;
use std::ffi::{CString, c_char, c_void};
use std::rc::Rc;

use petal::env::Env;
use petal::native_fn::{
    InputClasses, NativeClass, NativeEffects, NativeFnId, NativeResult, PetalCxt,
};
use petal::symbol::SymbolId;
use petal::value::Value;

use crate::builder::{self, Builder};
use crate::view::{Names, PbValue, ViewArena};

/// `pb_native_fn`.
pub type PbNativeFn = unsafe extern "C" fn(call: *mut PbCall, userdata: *mut c_void) -> i32;
/// `pb_free_fn`.
pub type PbFreeFn = unsafe extern "C" fn(userdata: *mut c_void);

// Effect flag bits — mirror PB_FX_* in petal_bridge.h.
pub const FX_PROBE: u32 = 1 << 0;
pub const FX_EMITS: u32 = 1 << 1;
pub const FX_EFFECT: u32 = 1 << 2;
pub const FX_PENDING_EFFECTFUL: u32 = 1 << 3;
pub const FX_PENDING_ALLOW: u32 = 1 << 4;
pub const FX_READS_SHIFT: u32 = 8;
pub const FX_READS_HOST_DATA: u32 = 1 << 12;

/// Translate `PB_FX_*` flags into Petal's effect row.
pub fn effects_from_flags(flags: u32) -> NativeEffects {
    let pending = if flags & FX_PENDING_ALLOW != 0 {
        NativeClass::AllowPending
    } else if flags & FX_PENDING_EFFECTFUL != 0 {
        NativeClass::Effectful
    } else {
        NativeClass::Strict
    };
    NativeEffects {
        reads: InputClasses(((flags >> FX_READS_SHIFT) & 0xff) as u16),
        probe: flags & FX_PROBE != 0,
        emits: flags & FX_EMITS != 0,
        effect: flags & FX_EFFECT != 0,
        pending,
    }
}

/// A C callback and the userdata it was registered with. Owned by the boxed
/// native's closure; dropping it (with the `Env`) frees the userdata.
pub struct HostCallback {
    name: CString,
    flags: u32,
    func: PbNativeFn,
    userdata: *mut c_void,
    free: Option<PbFreeFn>,
    /// Buffers reused from call to call: the argument list, the decoded
    /// argument views and the result builder. A game calls its natives
    /// thousands of times a frame, and building these fresh each time was
    /// most of the bridge's cost.
    scratch: RefCell<CallScratch>,
}

#[derive(Default)]
struct CallScratch {
    args: Vec<Value>,
    arena: ViewArena,
    result: Builder,
}

impl HostCallback {
    /// Take ownership of `userdata`: from here on it is freed exactly once,
    /// when this value drops.
    pub fn new(
        name: &str,
        flags: u32,
        func: PbNativeFn,
        userdata: *mut c_void,
        free: Option<PbFreeFn>,
    ) -> Self {
        HostCallback {
            name: crate::ffi::cstring_lossy(name),
            flags,
            func,
            userdata,
            free,
            scratch: RefCell::default(),
        }
    }

    /// Run the callback for one script call.
    fn call(&self, cxt: &mut PetalCxt) -> NativeResult {
        if self.flags & FX_READS_HOST_DATA != 0 {
            cxt.note_host_read();
        }
        if self.flags & FX_EFFECT != 0 {
            cxt.note_effect();
        }
        // The scratch is busy only if the host re-entered this same native
        // from inside its callback; that call gets buffers of its own.
        let mut fresh = CallScratch::default();
        let mut guard = self.scratch.try_borrow_mut();
        let scratch = match guard {
            Ok(ref mut s) => &mut **s,
            Err(_) => &mut fresh,
        };
        scratch.args.clear();
        for i in 1..=cxt.arg_count() {
            scratch.args.push(cxt.get_value(i)?);
        }
        scratch.arena.reset();
        let first = scratch.arena.decode(&scratch.args, cxt.heap(), &Names::NONE);
        scratch.arena.finish();
        scratch.result.clear();
        let mut call = PbCall {
            name: self.name.as_ptr(),
            args: scratch.arena.node_ptr(first),
            argc: scratch.args.len(),
            result: std::mem::take(&mut scratch.result),
            error: None,
        };
        // SAFETY: the host's callback with the userdata it registered.
        let rc = unsafe { (self.func)(&mut call, self.userdata) };
        let result = call.result.take_result();
        scratch.result = call.result;
        if rc != 0 || call.error.is_some() {
            return Err(call.error.unwrap_or_else(|| {
                format!("host native `{}` failed", self.name.to_string_lossy())
            }));
        }
        let result = result.map_err(|e| e.message)?;
        let one = [result];
        let syms = builder::intern_symbols(&one, |s| cxt.intern_symbol(s));
        let value = builder::materialize(&one, cxt.heap_mut(), &syms)
            .pop()
            .unwrap_or(Value::Nil);
        cxt.push_value(value);
        Ok(1)
    }
}

impl Drop for HostCallback {
    fn drop(&mut self) {
        if let Some(free) = self.free {
            // SAFETY: the host supplied this destructor for this pointer, and
            // a HostCallback is never cloned, so it runs once.
            unsafe { free(self.userdata) };
        }
    }
}

/// Every argument of the current native call.
fn args_of(cxt: &PetalCxt) -> Result<Vec<Value>, String> {
    (1..=cxt.arg_count()).map(|i| cxt.get_value(i)).collect()
}

/// Register a C callback as the native `name`. The returned id is the
/// native's; `callback` (and so the userdata) now belongs to `env`.
pub fn register_callback(env: &mut Env, callback: HostCallback) -> NativeFnId {
    let name = callback.name.to_string_lossy().into_owned();
    let effects = effects_from_flags(callback.flags);
    env.register_native_boxed(&name, move |cxt| callback.call(cxt), effects)
}

/// Register an emitter: `name(args...)` pushes `tag(args...)` into `buffer`
/// and returns nil. Declared `Emits` with the Pending-no-op policy.
pub fn register_emitter(env: &mut Env, name: &str, buffer: SymbolId, tag: &str) -> NativeFnId {
    let tag: Rc<str> = Rc::from(tag);
    env.register_native_boxed(
        name,
        move |cxt| {
            let args = args_of(cxt)?;
            cxt.emit(buffer, &tag, args);
            cxt.push_nil();
            Ok(1)
        },
        effects_from_flags(FX_EMITS | FX_PENDING_EFFECTFUL),
    )
}

/// The state behind a `pb_call*`: one in-flight host-native invocation.
pub struct PbCall {
    name: *const c_char,
    args: *const PbValue,
    argc: usize,
    result: Builder,
    error: Option<String>,
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_call_name(call: *const PbCall) -> *const c_char {
    if call.is_null() {
        return c"".as_ptr();
    }
    unsafe { &*call }.name
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_call_arg_count(call: *const PbCall) -> usize {
    if call.is_null() {
        return 0;
    }
    unsafe { &*call }.argc
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_call_arg(call: *const PbCall, i: usize) -> *const PbValue {
    if call.is_null() {
        return std::ptr::null();
    }
    let call = unsafe { &*call };
    if i < call.argc {
        // SAFETY: args points at `argc` contiguous nodes.
        unsafe { call.args.add(i) }
    } else {
        std::ptr::null()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_call_args(call: *const PbCall) -> *const PbValue {
    if call.is_null() {
        return std::ptr::null();
    }
    unsafe { &*call }.args
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_call_result(call: *mut PbCall) -> *mut Builder {
    if call.is_null() {
        return std::ptr::null_mut();
    }
    &mut unsafe { &mut *call }.result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_call_set_error(call: *mut PbCall, message: *const c_char) {
    if call.is_null() {
        return;
    }
    let msg = if message.is_null() {
        "host native failed".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    unsafe { &mut *call }.error = Some(msg);
}
