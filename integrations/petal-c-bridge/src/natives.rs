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
pub const FX_CONST: u32 = 1 << 5;
pub const FX_READS_SHIFT: u32 = 8;
pub const FX_READS_HOST_DATA: u32 = 1 << 12;

/// Translate `PB_FX_*` flags into Petal's effect row.
///
/// `PB_FX_CONST` is carried over as the row's `constant` bit and nothing
/// else: flags `0` (`PB_FX_PURE`) stay pure-and-dynamic, so a host that never
/// heard of the flag has no native treated as a constant function. A
/// contradictory combination (`PB_FX_CONST | PB_FX_EFFECT`, or with a read)
/// is dynamic — [`NativeEffects::is_const`] decides.
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
        constant: flags & FX_CONST != 0,
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
            // The call outlives neither `cxt` nor this function; the pointer
            // is cleared below before the borrow ends.
            cxt: (cxt as *mut PetalCxt<'_>).cast(),
            location_file: None,
        };
        // SAFETY: the host's callback with the userdata it registered.
        let rc = unsafe { (self.func)(&mut call, self.userdata) };
        call.cxt = std::ptr::null_mut();
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
    /// The `PetalCxt` of the script call, lifetime erased; non-null only
    /// while the callback runs.
    cxt: *mut c_void,
    /// Keeps the string `pb_call_location` handed out alive for the call.
    location_file: Option<CString>,
}

impl PbCall {
    /// The script call's context, while the callback is running.
    fn cxt<'c>(&mut self) -> Option<&'c mut PetalCxt<'c>> {
        // SAFETY: set from a live `&mut PetalCxt` for the duration of the
        // callback (HostCallback::call) and nulled when it returns; the host
        // can only reach it through the `pb_call*` it was handed, on the
        // thread running the callback.
        unsafe { self.cxt.cast::<PetalCxt<'c>>().as_mut() }
    }
}

/// Borrow an optional C string, lossily.
unsafe fn opt_text(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

/// `pb_call_suspend`: ask the test this call runs in to stop after the call
/// returns ([`PetalCxt::suspend`]). 0 on success; nonzero when nothing can
/// resume the call, in which case the call's error is set to the reason (the
/// callback should return nonzero).
#[unsafe(no_mangle)]
pub extern "C" fn pb_call_suspend(call: *mut PbCall) -> i32 {
    crate::ffi::guard(1, || {
        let Some(call) = (unsafe { call.as_mut() }) else {
            return 1;
        };
        let Some(cxt) = call.cxt() else { return 1 };
        match cxt.suspend() {
            Ok(()) => 0,
            Err(e) => {
                call.error = Some(e);
                1
            }
        }
    })
}

/// `pb_call_can_suspend`: whether `pb_call_suspend` would succeed here.
#[unsafe(no_mangle)]
pub extern "C" fn pb_call_can_suspend(call: *mut PbCall) -> bool {
    crate::ffi::guard(false, || {
        unsafe { call.as_mut() }
            .and_then(|c| c.cxt())
            .is_some_and(|cxt| cxt.can_suspend())
    })
}

/// `pb_call_expect`: record an expectation of the host's own, at the
/// position of the script call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_call_expect(
    call: *mut PbCall,
    ok: bool,
    check: *const c_char,
    message: *const c_char,
) {
    let check = unsafe { opt_text(check) };
    let message = unsafe { opt_text(message) };
    crate::ffi::guard((), || {
        let Some(call) = (unsafe { call.as_mut() }) else {
            return;
        };
        let name = unsafe { std::ffi::CStr::from_ptr(call.name) }
            .to_string_lossy()
            .into_owned();
        let Some(cxt) = call.cxt() else { return };
        if ok {
            cxt.pass_expectation();
        } else {
            let check = check.unwrap_or(name);
            let message = message.unwrap_or_else(|| format!("{check} failed"));
            cxt.fail_expectation(&check, message);
        }
    })
}

/// `pb_call_location`: where the script call is written. False when the call
/// has no source position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_call_location(
    call: *mut PbCall,
    out_file: *mut *const c_char,
    out_line: *mut u32,
    out_column: *mut u32,
) -> bool {
    crate::ffi::guard(false, || {
        let Some(call) = (unsafe { call.as_mut() }) else {
            return false;
        };
        let Some(loc) = call.cxt().and_then(|cxt| cxt.call_location()) else {
            return false;
        };
        let file = call.location_file.insert(crate::ffi::cstring_lossy(&loc.file));
        unsafe {
            if !out_file.is_null() {
                out_file.write(file.as_ptr());
            }
            if !out_line.is_null() {
                out_line.write(loc.line);
            }
            if !out_column.is_null() {
                out_column.write(loc.column);
            }
        }
        true
    })
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
