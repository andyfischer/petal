//! `pb_vm`: one Petal `Env` with one loaded program and its stack, plus the
//! petal-ui input state and every buffer the bridge hands out to C.
//!
//! Every `pb_vm_*` entry point goes through [`with_vm`], which
//! - rejects NULL,
//! - rejects re-entry (a host native calling back into its own VM mid-run),
//! - catches panics and reports them as `PB_ERR_PANIC`,
//! - and publishes failures as the VM's `pb_vm_last_error`.

use std::cell::{Cell, UnsafeCell};
use std::ffi::{CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use petal::env::Env;
use petal::error::CallError;
use petal::program::ProgramId;
use petal::source_watch::SourceWatch;
use petal::stack::StackKey;
use petal::value::Value;
use petal_ui::input::{InputEvent, InputState, Modifiers};

use crate::builder::{self, Builder, HostValue};
use crate::draw::{DrawList, PbDrawCmd};
use crate::ffi::{
    BResult, BridgeError, PbError, PublishedError, Status, arg_str, cstring_lossy, panic_message,
};
use crate::natives::{self, HostCallback, PbFreeFn, PbNativeFn};
use crate::scenario::PbScenario;
use crate::view::{Names, PbValue, ViewArena};

/// The loaded program.
struct Loaded {
    program_id: ProgramId,
    stack_id: StackKey,
    /// Entry file path, when loaded from a file (drives `pb_vm_reload`).
    entry_path: Option<PathBuf>,
    /// Label for errors in the entry file (its path, or the source name).
    entry_name: String,
    /// Stamps of every source file at the last (re)load attempt.
    watch: SourceWatch,
}

/// Mirrors `pb_values`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PbValues {
    pub items: *const PbValue,
    pub count: usize,
}

/// Mirrors `pb_reload_result`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PbReloadResult {
    pub state_preserved: u32,
    pub state_dropped: u32,
}

/// A list of C strings handed out to the host, kept alive until replaced.
#[derive(Default)]
struct CStrList {
    owned: Vec<CString>,
    ptrs: Vec<*const c_char>,
}

impl CStrList {
    fn set<I: IntoIterator<Item = String>>(&mut self, items: I) {
        self.owned = items.into_iter().map(|s| cstring_lossy(&s)).collect();
        self.ptrs = self.owned.iter().map(|c| c.as_ptr()).collect();
    }
}

/// One Petal `Env` with one loaded program and its stack: what a `pb_vm*`
/// points at. Rust hosts (a crate linking this one as an rlib) can drive it
/// directly through its `pub` methods instead of the C entry points.
pub struct Vm {
    env: Env,
    implicit_imports: Vec<String>,
    loaded: Option<Loaded>,
    input: InputState,
    last_error: Option<Box<PublishedError>>,
    // Everything below is handed out to C and lives until the documented
    // invalidation point. C pointers target the heap buffers *inside* these
    // values, so moving the values themselves (e.g. as the Vecs grow) is fine.
    views: Vec<ViewArena>,
    draws: Vec<DrawList>,
    source_files: CStrList,
    output_lines: CStrList,
    state_json: CString,
    package_name: CString,
}

/// The allocation behind `pb_vm*`. The busy flag lives outside the
/// `UnsafeCell` so a re-entrant call can be detected without creating a
/// second `&mut Vm`.
pub struct VmHandle {
    busy: Cell<bool>,
    vm: UnsafeCell<Vm>,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// A VM with the petal-ui natives registered and `ui` implicitly imported.
    pub fn new() -> Vm {
        let mut env = Env::new();
        // Scripts print through take_output; the host decides where it goes.
        env.set_echo(false);
        petal_ui::register_all(&mut env);
        Vm {
            env,
            implicit_imports: vec![petal_ui::MODULE_NAME.to_string()],
            loaded: None,
            input: InputState::new(),
            last_error: None,
            views: Vec::new(),
            draws: Vec::new(),
            source_files: CStrList::default(),
            output_lines: CStrList::default(),
            state_json: CString::default(),
            package_name: CString::default(),
        }
    }

    /// The VM's `Env`, for anything the bridge does not wrap.
    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn env_mut(&mut self) -> &mut Env {
        &mut self.env
    }

    /// petal-ui input state; events fed here are promoted by `begin_frame`.
    pub fn input_mut(&mut self) -> &mut InputState {
        &mut self.input
    }

    /// The loaded program and its stack, if any.
    pub fn program(&self) -> Option<(ProgramId, StackKey)> {
        self.loaded.as_ref().map(|l| (l.program_id, l.stack_id))
    }

    fn loaded(&self) -> BResult<&Loaded> {
        self.loaded
            .as_ref()
            .ok_or_else(|| BridgeError::new(Status::NotLoaded, "no program is loaded"))
    }

    fn clear_views(&mut self) {
        self.views.clear();
        self.draws.clear();
    }

    /// Decode `values` into a new arena kept until the next clear.
    fn keep_view(&mut self, values: &[Value]) -> (*const PbValue, usize) {
        let env = &self.env;
        let sym = |s: petal::symbol::SymbolId| env.symbol_name(s).map(str::to_string);
        let class = |c: u16| env.handle_classes().get(c as usize).map(|h| h.name.clone());
        let names = Names {
            symbol: Some(&sym),
            handle_class: Some(&class),
        };
        let mut arena = ViewArena::new();
        let first = arena.decode(values, env.heap(), &names);
        arena.finish();
        let p = arena.node_ptr(first);
        self.views.push(arena);
        (p, values.len())
    }

    /// Materialize host values onto the default heap.
    fn host_values(&mut self, values: &[HostValue]) -> Vec<Value> {
        let syms = builder::intern_symbols(values, |s| self.env.intern_symbol(s));
        builder::materialize(values, self.env.heap_mut(), &syms)
    }

    fn bind(&mut self, name: &str, value: HostValue) {
        let v = self
            .host_values(std::slice::from_ref(&value))
            .pop()
            .unwrap_or(Value::Nil);
        let sym = self.env.intern_symbol(name);
        self.env.set_binding(sym, v);
    }

    fn bind_raw(&mut self, name: &str, v: Value) {
        let sym = self.env.intern_symbol(name);
        self.env.set_binding(sym, v);
    }

    // ── Loading ─────────────────────────────────────────────────────────

    /// Compile `source` and make it the program, with a fresh stack.
    /// `origin` is the entry file (imports resolve next to it, and it is
    /// watched for hot reload); `entry_name` labels its errors.
    pub fn load(&mut self, source: &str, origin: Option<&Path>, entry_name: String) -> BResult<()> {
        let program_id = self
            .env
            .load_program_diag(source, origin)
            .map_err(|e| BridgeError::from_load(&e, &entry_name))?;
        let stack_id = self
            .env
            .create_stack(program_id)
            .map_err(|e| BridgeError::runtime(e, &entry_name))?;
        self.clear_views();
        self.loaded = Some(Loaded {
            program_id,
            stack_id,
            entry_path: origin.map(Path::to_path_buf),
            entry_name,
            watch: self.env.watch_program_sources(program_id, origin),
        });
        Ok(())
    }

    /// Every source file of the loaded program: the entry file, then each
    /// module with a filesystem origin, without duplicates.
    pub fn source_paths(&self) -> Vec<PathBuf> {
        match &self.loaded {
            Some(l) => self
                .env
                .program_source_paths(l.program_id, l.entry_path.as_deref()),
            None => Vec::new(),
        }
    }

    /// Re-stamp the source files as of now (a (re)load attempt looked at them).
    fn refresh_watch(&mut self) {
        if let Some(l) = &mut self.loaded {
            l.watch = self
                .env
                .watch_program_sources(l.program_id, l.entry_path.as_deref());
        }
    }

    /// Whether any source file changed (modification time or length),
    /// appeared or disappeared since the last load or reload attempt.
    pub fn sources_changed(&self) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.watch.changed())
    }

    /// Recompile the program from `source` (keeping its entry file origin)
    /// and swap it in with `transfer_state`. On a compile error the old
    /// program stays loaded.
    pub fn reload_with(&mut self, source: &str) -> BResult<PbReloadResult> {
        let l = self.loaded()?;
        let (pid, stack, origin, name) = (
            l.program_id,
            l.stack_id,
            l.entry_path.clone(),
            l.entry_name.clone(),
        );
        let compiled = self
            .env
            .compile_program_diag(pid, source, origin.as_deref());
        let program = match compiled {
            Ok(p) => p,
            Err(e) => {
                // This version of the files has been looked at: don't report
                // it as changed again until it is edited.
                self.refresh_watch();
                return Err(BridgeError::from_load(&e, &name));
            }
        };
        let result = self
            .env
            .transfer_state(stack, program)
            .map_err(|e| BridgeError::runtime(e, &name))?;
        self.clear_views();
        // The new program may import a different set of files.
        self.refresh_watch();
        Ok(PbReloadResult {
            state_preserved: result.state_preserved as u32,
            state_dropped: result.state_dropped as u32,
        })
    }

    /// Re-read the entry file and [`reload_with`](Self::reload_with) it.
    pub fn reload(&mut self) -> BResult<PbReloadResult> {
        let path = self.loaded()?.entry_path.clone().ok_or_else(|| {
            BridgeError::invalid("pb_vm_reload needs a program loaded with pb_vm_load_file")
        })?;
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.refresh_watch();
                return Err(BridgeError::new(
                    Status::Io,
                    format!("cannot read {}: {e}", path.display()),
                ));
            }
        };
        self.reload_with(&source)
    }

    // ── Running ─────────────────────────────────────────────────────────

    /// Run one frame: clear output buffers and canvas ids, reset the stack
    /// and run the whole program.
    pub fn run(&mut self) -> BResult<()> {
        let l = self.loaded()?;
        let (stack, name) = (l.stack_id, l.entry_name.clone());
        self.clear_views();
        // A fresh frame: nothing left over from the last run (or a run that
        // errored half way) may be mistaken for this frame's output.
        for sym in self.env.output_channels() {
            self.env.clear_output_buffer(sym);
        }
        petal_ui::draw::reset_canvas_ids(&mut self.env);
        self.env
            .reset_stack(stack)
            .map_err(|e| BridgeError::runtime(e, &name))?;
        self.env
            .run(stack)
            .map_err(|e| BridgeError::runtime(e, &name))?;
        Ok(())
    }

    fn call(&mut self, function: &str, args: &[HostValue]) -> BResult<*const PbValue> {
        let l = self.loaded()?;
        let (stack, name) = (l.stack_id, l.entry_name.clone());
        self.clear_views();
        let args = self.host_values(args);
        let result = self
            .env
            .call_function_diag(stack, function, &args)
            .map_err(|e| match e {
                CallError::FunctionNotFound { .. } => {
                    BridgeError::new(Status::NotFound, e.to_string())
                }
                CallError::StackNotFound => BridgeError::new(Status::NotLoaded, e.to_string()),
                CallError::Runtime(msg) => BridgeError::runtime(msg, &name),
            })?;
        Ok(self.keep_view(&[result]).0)
    }

    /// Whether the last run defined a top-level function `name`.
    pub fn has_function(&self, name: &str) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|l| self.env.has_function(l.stack_id, name))
    }

    /// Apply every event `scenario` schedules for `frame` to the input
    /// state; returns how many. Call before `begin_frame` for that frame.
    pub fn apply_scenario(
        &mut self,
        scenario: &petal_ui::scenario::Scenario,
        frame: usize,
    ) -> usize {
        let mut n = 0;
        for ev in scenario.events.iter().filter(|e| e.at == frame) {
            self.input.event(ev.event.clone());
            n += 1;
        }
        n
    }
}

// ── Entry-point plumbing ──────────────────────────────────────────────────

/// Run `f` on the VM behind `ptr` with NULL/re-entrancy/panic protection,
/// recording the outcome as the VM's last error. Returns `f`'s value, or
/// `fallback` on any failure.
fn with_vm<T>(
    ptr: *mut VmHandle,
    fallback: T,
    f: impl FnOnce(&mut Vm) -> BResult<T>,
) -> (Status, T) {
    if ptr.is_null() {
        return (Status::InvalidArg, fallback);
    }
    // SAFETY: a non-null pb_vm* came from pb_vm_create and is not destroyed.
    let handle = unsafe { &*ptr };
    if handle.busy.get() {
        return (Status::Reentrant, fallback);
    }
    handle.busy.set(true);
    // SAFETY: `busy` guarantees this is the only live `&mut Vm`.
    let vm = unsafe { &mut *handle.vm.get() };
    let outcome = catch_unwind(AssertUnwindSafe(|| f(vm)));
    handle.busy.set(false);
    match outcome {
        Ok(Ok(v)) => {
            vm.last_error = None;
            (Status::Ok, v)
        }
        Ok(Err(e)) => {
            let code = e.code;
            vm.last_error = Some(PublishedError::new(e));
            (code, fallback)
        }
        Err(payload) => {
            let msg = format!("petal-bridge internal panic: {}", panic_message(&*payload));
            vm.last_error = Some(PublishedError::new(BridgeError::new(Status::Panic, msg)));
            (Status::Panic, fallback)
        }
    }
}

/// `with_vm` for status-returning entry points.
fn status(ptr: *mut VmHandle, f: impl FnOnce(&mut Vm) -> BResult<()>) -> Status {
    with_vm(ptr, (), f).0
}

/// Write `v` through an optional out-pointer.
fn put<T>(out: *mut T, v: T) {
    if !out.is_null() {
        // SAFETY: caller-provided out-pointer.
        unsafe { out.write(v) };
    }
}

// ── Lifecycle and modules ────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_create() -> *mut VmHandle {
    catch_unwind(|| {
        Box::into_raw(Box::new(VmHandle {
            busy: Cell::new(false),
            vm: UnsafeCell::new(Vm::new()),
        }))
    })
    .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_destroy(ptr: *mut VmHandle) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: from pb_vm_create; ownership returns to us.
    let handle = unsafe { &*ptr };
    if handle.busy.get() {
        // Destroying a VM from inside its own native callback would free the
        // Env under the running VM; refuse (and leak) rather than crash.
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(ptr) })));
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_last_error(ptr: *const VmHandle) -> *const PbError {
    if ptr.is_null() {
        return std::ptr::null();
    }
    let handle = unsafe { &*ptr };
    if handle.busy.get() {
        return std::ptr::null();
    }
    let vm = unsafe { &*handle.vm.get() };
    vm.last_error.as_ref().map_or(std::ptr::null(), |e| &e.c)
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_echo(vm: *mut VmHandle, on: bool) -> Status {
    status(vm, |vm| {
        vm.env.set_echo(on);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_add_module_path(vm: *mut VmHandle, dir: *const c_char) -> Status {
    status(vm, |vm| {
        let dir = unsafe { arg_str(dir, "dir") }?;
        vm.env.add_module_path(PathBuf::from(dir));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_register_module(
    vm: *mut VmHandle,
    name: *const c_char,
    source: *const c_char,
) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        let source = unsafe { arg_str(source, "source") }?;
        vm.env.register_module(name, source);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_add_package(
    vm: *mut VmHandle,
    root: *const c_char,
    out_name: *mut *const c_char,
) -> Status {
    status(vm, |vm| {
        let root = unsafe { arg_str(root, "root") }?;
        let info = vm.env.add_package(root).map_err(|e| {
            let mut err = BridgeError::from_load(&e, root);
            err.code = Status::NotFound;
            err
        })?;
        vm.package_name = cstring_lossy(&info.name);
        put(out_name, vm.package_name.as_ptr());
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_add_implicit_import(
    vm: *mut VmHandle,
    module: *const c_char,
) -> Status {
    status(vm, |vm| {
        let module = unsafe { arg_str(module, "module_name") }?;
        if !vm.implicit_imports.iter().any(|m| m == module) {
            vm.implicit_imports.push(module.to_string());
        }
        let names: Vec<&str> = vm.implicit_imports.iter().map(String::as_str).collect();
        vm.env.set_implicit_imports(&names);
        Ok(())
    })
}

// ── Host natives ─────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_register_native(
    vm: *mut VmHandle,
    name: *const c_char,
    func: Option<PbNativeFn>,
    userdata: *mut c_void,
    free_userdata: Option<PbFreeFn>,
    effects: u32,
) -> Status {
    // Ownership of the userdata passes to the bridge here, whatever happens:
    // on success the Env's boxed native owns it (freed with the VM); on a bad
    // argument the HostCallback made below — or this guard, if we fail before
    // making one — frees it now. A re-entrant call is the one exception: it
    // is rejected before `status` runs the closure, so the host keeps it.
    let mut pending = free_userdata.map(|free| (free, userdata));
    let st = status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") };
        let func = func.ok_or_else(|| BridgeError::invalid("native callback is NULL"));
        let (name, func) = match (name, func) {
            (Ok(n), Ok(f)) => (n, f),
            (Err(e), _) | (_, Err(e)) => return Err(e),
        };
        pending = None;
        let callback = HostCallback::new(name, effects, func, userdata, free_userdata);
        natives::register_callback(&mut vm.env, callback);
        Ok(())
    });
    if st != Status::Reentrant
        && let Some((free, userdata)) = pending
    {
        unsafe { free(userdata) };
    }
    st
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_register_emitter(
    vm: *mut VmHandle,
    name: *const c_char,
    buffer: *const c_char,
    tag: *const c_char,
) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        let buffer = unsafe { arg_str(buffer, "buffer") }?;
        let tag = if tag.is_null() {
            name
        } else {
            unsafe { arg_str(tag, "tag") }?
        };
        let sym = vm.env.intern_symbol(buffer);
        natives::register_emitter(&mut vm.env, name, sym, tag);
        Ok(())
    })
}

// ── Loading ──────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_load_file(vm: *mut VmHandle, path: *const c_char) -> Status {
    status(vm, |vm| {
        let path = unsafe { arg_str(path, "path") }?;
        let source = std::fs::read_to_string(path)
            .map_err(|e| BridgeError::new(Status::Io, format!("cannot read {path}: {e}")))?;
        vm.load(&source, Some(Path::new(path)), path.to_string())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_load_source(
    vm: *mut VmHandle,
    source: *const c_char,
    name: *const c_char,
) -> Status {
    status(vm, |vm| {
        let source = unsafe { arg_str(source, "source") }?;
        let name = if name.is_null() {
            "<source>".to_string()
        } else {
            unsafe { arg_str(name, "name") }?.to_string()
        };
        vm.load(source, None, name)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_is_loaded(vm: *const VmHandle) -> bool {
    with_vm(vm as *mut VmHandle, false, |vm| Ok(vm.loaded.is_some())).1
}

// ── Bindings ─────────────────────────────────────────────────────────────

fn bind_with(
    vm: *mut VmHandle,
    name: *const c_char,
    make: impl FnOnce(&mut Vm) -> BResult<Value>,
) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        let v = make(vm)?;
        vm.bind_raw(name, v);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_float(vm: *mut VmHandle, name: *const c_char, v: f64) -> Status {
    bind_with(vm, name, |_| Ok(Value::Float(v)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_int(vm: *mut VmHandle, name: *const c_char, v: i64) -> Status {
    bind_with(vm, name, |_| Ok(Value::Int(v)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_bool(vm: *mut VmHandle, name: *const c_char, v: bool) -> Status {
    bind_with(vm, name, |_| Ok(Value::Bool(v)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_string(
    vm: *mut VmHandle,
    name: *const c_char,
    utf8: *const c_char,
) -> Status {
    bind_with(vm, name, |vm| {
        let s = unsafe { arg_str(utf8, "value") }?.to_string();
        Ok(Value::String(vm.env.heap_mut().alloc_string(s)))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_vec2(
    vm: *mut VmHandle,
    name: *const c_char,
    x: f64,
    y: f64,
) -> Status {
    bind_with(vm, name, |_| Ok(Value::Vec2(x, y)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_vec3(
    vm: *mut VmHandle,
    name: *const c_char,
    x: f64,
    y: f64,
    z: f64,
) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        vm.bind(name, builder::vec3(x, y, z));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_floats(
    vm: *mut VmHandle,
    name: *const c_char,
    values: *const f64,
    n: usize,
) -> Status {
    bind_with(vm, name, |vm| {
        let items: Vec<Value> = if values.is_null() || n == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(values, n) }
                .iter()
                .map(|f| Value::Float(*f))
                .collect()
        };
        Ok(Value::List(vm.env.heap_mut().alloc_list(items)))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_value(
    vm: *mut VmHandle,
    name: *const c_char,
    value: *const Builder,
) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        if value.is_null() {
            return Err(BridgeError::invalid("value builder is NULL"));
        }
        let v = unsafe { &*value }.single()?.clone();
        vm.bind(name, v);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_clear_binding(vm: *mut VmHandle, name: *const c_char) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        let sym = vm.env.intern_symbol(name);
        vm.env.clear_binding(sym);
        Ok(())
    })
}

// ── petal-ui input ───────────────────────────────────────────────────────

fn input(vm: *mut VmHandle, ev: impl FnOnce() -> BResult<InputEvent>) -> Status {
    status(vm, |vm| {
        vm.input.event(ev()?);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_input_mouse_move(vm: *mut VmHandle, x: i32, y: i32) -> Status {
    input(vm, || Ok(InputEvent::MouseMove { x, y }))
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_input_mouse_motion(vm: *mut VmHandle, dx: i32, dy: i32) -> Status {
    input(vm, || Ok(InputEvent::MouseRelative { dx, dy }))
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_input_mouse_button(vm: *mut VmHandle, button: u8, down: bool) -> Status {
    input(vm, || {
        Ok(if down {
            InputEvent::MouseDown { button }
        } else {
            InputEvent::MouseUp { button }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_input_scroll(vm: *mut VmHandle, dx: f64, dy: f64) -> Status {
    input(vm, || Ok(InputEvent::Scroll { dx, dy }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_input_key(
    vm: *mut VmHandle,
    key: *const c_char,
    down: bool,
) -> Status {
    input(vm, || {
        let key = unsafe { arg_str(key, "key") }?;
        if !petal_ui::input::is_canonical_key(key) {
            return Err(BridgeError::invalid(
                petal_ui::input::non_canonical_key_error(key),
            ));
        }
        let key = key.to_string();
        Ok(if down {
            InputEvent::KeyDown { key }
        } else {
            InputEvent::KeyUp { key }
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_input_text(vm: *mut VmHandle, utf8: *const c_char) -> Status {
    input(vm, || {
        let text = unsafe { arg_str(utf8, "text") }?.to_string();
        Ok(InputEvent::Text { text })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_input_modifiers(vm: *mut VmHandle, bits: u32) -> Status {
    input(vm, || {
        Ok(InputEvent::Modifiers(Modifiers {
            shift: bits & 1 != 0,
            ctrl: bits & 2 != 0,
            alt: bits & 4 != 0,
            cmd: bits & 8 != 0,
        }))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_key_is_canonical(key: *const c_char) -> bool {
    crate::ffi::guard(false, || match unsafe { arg_str(key, "key") } {
        Ok(k) => petal_ui::input::is_canonical_key(k),
        Err(_) => false,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_apply_scenario(
    vm: *mut VmHandle,
    scenario: *const PbScenario,
    frame: usize,
    out_applied: *mut usize,
) -> Status {
    let (st, n) = with_vm(vm, 0, |vm| {
        if scenario.is_null() {
            return Err(BridgeError::invalid("scenario is NULL"));
        }
        // SAFETY: a non-null pb_scenario* came from pb_scenario_new.
        Ok(vm.apply_scenario(&unsafe { &*scenario }.scenario, frame))
    });
    put(out_applied, n);
    st
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_begin_frame(
    vm: *mut VmHandle,
    dt: f64,
    frame: i64,
    time_seconds: f64,
) -> Status {
    status(vm, |vm| {
        vm.input.begin_frame(dt);
        petal_ui::input::bind_input(&mut vm.env, &vm.input);
        petal_ui::input::bind_frame_info(&mut vm.env, dt, frame);
        petal_ui::input::bind_time(&mut vm.env, time_seconds);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_dimensions(vm: *mut VmHandle, width: i32, height: i32) -> Status {
    status(vm, |vm| {
        petal_ui::input::bind_dimensions(&mut vm.env, width, height);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_seed(vm: *mut VmHandle, seed: u64) -> Status {
    status(vm, |vm| {
        vm.env.set_seed(seed);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_text_metrics(vm: *mut VmHandle, advance_ratio: f64) -> Status {
    status(vm, |vm| {
        petal_ui::draw::bind_text_metrics(&mut vm.env, advance_ratio);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_text_vertical_metrics(
    vm: *mut VmHandle,
    baseline: f64,
    descent: f64,
    line_height: f64,
    cap_height: f64,
    x_height: f64,
) -> Status {
    status(vm, |vm| {
        let m = petal_ui::draw::VerticalMetrics {
            baseline,
            descent,
            line_height,
            cap_height,
            x_height,
        };
        petal_ui::draw::bind_text_vertical_metrics(&mut vm.env, &m);
        Ok(())
    })
}

// ── Running ──────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_run(vm: *mut VmHandle) -> Status {
    status(vm, |vm| vm.run())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_call(
    vm: *mut VmHandle,
    function: *const c_char,
    args: *const Builder,
    out_result: *mut *const PbValue,
) -> Status {
    let (st, result) = with_vm(vm, std::ptr::null(), |vm| {
        let function = unsafe { arg_str(function, "function") }?;
        let args: Vec<HostValue> = if args.is_null() {
            Vec::new()
        } else {
            unsafe { &*args }.roots()?.to_vec()
        };
        vm.call(function, &args)
    });
    if st == Status::Ok {
        put(out_result, result);
    }
    st
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_has_function(vm: *mut VmHandle, function: *const c_char) -> bool {
    with_vm(vm, false, |vm| {
        let function = unsafe { arg_str(function, "function") }?;
        Ok(vm.has_function(function))
    })
    .1
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_clear_views(vm: *mut VmHandle) {
    status(vm, |vm| {
        vm.clear_views();
        Ok(())
    });
}

// ── Output buffers ───────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_drain(
    vm: *mut VmHandle,
    buffer: *const c_char,
    out: *mut PbValues,
) -> Status {
    let (st, values) = with_vm(vm, (std::ptr::null(), 0), |vm| {
        let buffer = unsafe { arg_str(buffer, "buffer") }?;
        let sym = vm.env.intern_symbol(buffer);
        let values = vm.env.take_output_buffer(sym);
        Ok(vm.keep_view(&values))
    });
    put(
        out,
        PbValues {
            items: values.0,
            count: values.1,
        },
    );
    st
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_take_mouse_grab(vm: *mut VmHandle) -> i32 {
    with_vm(vm, -1, |vm| {
        Ok(match petal_ui::input::take_mouse_grab(&mut vm.env) {
            Some(true) => 1,
            Some(false) => 0,
            None => -1,
        })
    })
    .1
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_drain_draw(
    vm: *mut VmHandle,
    out_cmds: *mut *const PbDrawCmd,
    out_count: *mut usize,
) -> Status {
    let (st, (cmds, count)) = with_vm(vm, (std::ptr::null(), 0), |vm| {
        let commands = petal_ui::draw::take_draw_commands(&mut vm.env);
        let env = &vm.env;
        let sym = |s: petal::symbol::SymbolId| env.symbol_name(s).map(str::to_string);
        let names = Names {
            symbol: Some(&sym),
            handle_class: None,
        };
        let mut list = DrawList::default();
        list.cmds.reserve(commands.len());
        for c in &commands {
            list.push(c, env.heap(), &names);
        }
        let out = (list.cmds.as_ptr(), list.cmds.len());
        vm.draws.push(list);
        Ok(out)
    });
    put(out_cmds, cmds);
    put(out_count, count);
    st
}

// ── Hot reload ───────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_source_files(
    vm: *mut VmHandle,
    out_paths: *mut *const *const c_char,
    out_count: *mut usize,
) -> Status {
    let (st, (paths, count)) = with_vm(vm, (std::ptr::null(), 0), |vm| {
        vm.loaded()?;
        let paths: Vec<String> = vm
            .source_paths()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        vm.source_files.set(paths);
        Ok((vm.source_files.ptrs.as_ptr(), vm.source_files.ptrs.len()))
    });
    put(out_paths, paths);
    put(out_count, count);
    st
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_sources_changed(vm: *mut VmHandle) -> bool {
    with_vm(vm, false, |vm| Ok(vm.sources_changed())).1
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_reload(vm: *mut VmHandle, out: *mut PbReloadResult) -> Status {
    let (st, r) = with_vm(vm, None, |vm| vm.reload().map(Some));
    if let Some(r) = r {
        put(out, r);
    }
    st
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_reload_source(
    vm: *mut VmHandle,
    source: *const c_char,
    out: *mut PbReloadResult,
) -> Status {
    let (st, r) = with_vm(vm, None, |vm| {
        let source = unsafe { arg_str(source, "source") }?;
        vm.reload_with(source).map(Some)
    });
    if let Some(r) = r {
        put(out, r);
    }
    st
}

// ── State and output tooling ─────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_state_json(vm: *mut VmHandle) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        let l = vm.loaded()?;
        let map = vm.env.get_state_json(l.program_id, l.stack_id);
        let text = serde_json::Value::Object(map).to_string();
        vm.state_json = cstring_lossy(&text);
        Ok(vm.state_json.as_ptr())
    })
    .1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_get_state(
    vm: *mut VmHandle,
    name: *const c_char,
    out: *mut *const PbValue,
) -> Status {
    let (st, v) = with_vm(vm, std::ptr::null(), |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        let l = vm.loaded()?;
        let (pid, stack) = (l.program_id, l.stack_id);
        let key = vm
            .env
            .state_key_names(pid)
            .into_iter()
            .find_map(|(k, n)| (n == name).then_some(k))
            .ok_or_else(|| {
                BridgeError::new(Status::NotFound, format!("no state variable `{name}`"))
            })?;
        let value = vm.env.get_state(stack, key).ok_or_else(|| {
            BridgeError::new(
                Status::NotFound,
                format!("state variable `{name}` has no value yet"),
            )
        })?;
        // A `state var` is stored as a cell: hand out what it holds.
        let value = match value {
            Value::Cell(id) => vm.env.heap().cell_read(id),
            v => v,
        };
        Ok(vm.keep_view(&[value]).0)
    });
    put(out, v);
    st
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_take_output(
    vm: *mut VmHandle,
    out_lines: *mut *const *const c_char,
    out_count: *mut usize,
) -> Status {
    let (st, (lines, count)) = with_vm(vm, (std::ptr::null(), 0), |vm| {
        let lines = vm.env.take_output();
        vm.output_lines.set(lines);
        Ok((vm.output_lines.ptrs.as_ptr(), vm.output_lines.ptrs.len()))
    });
    put(out_lines, lines);
    put(out_count, count);
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn out_ints(vm: &mut Vm) -> Vec<i64> {
        let sym = vm.env.intern_symbol("out");
        vm.env
            .take_output_buffer(sym)
            .into_iter()
            .map(|v| match v {
                Value::Int(i) => i,
                other => panic!("expected an int, got {other:?}"),
            })
            .collect()
    }

    static FREED: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn add_one(call: *mut natives::PbCall, userdata: *mut c_void) -> i32 {
        let base = unsafe { *(userdata as *const i64) };
        let arg = unsafe { &*pb_call_arg(call, 0) };
        crate::builder::pb_builder_int(pb_call_result(call), base + arg.integer);
        0
    }

    unsafe extern "C" fn free_i64(userdata: *mut c_void) {
        drop(unsafe { Box::from_raw(userdata as *mut i64) });
        FREED.fetch_add(1, Ordering::SeqCst);
    }

    use crate::natives::{pb_call_arg, pb_call_result};

    #[test]
    fn boxed_native_owns_and_frees_its_userdata() {
        FREED.store(0, Ordering::SeqCst);
        let mut vm = Vm::new();
        let ud = Box::into_raw(Box::new(100i64)) as *mut c_void;
        let cb = HostCallback::new("add_base", 0, add_one, ud, Some(free_i64));
        natives::register_callback(&mut vm.env, cb);
        vm.load(
            "push_output(symbol(\"out\"), add_base(5))\n",
            None,
            "t".into(),
        )
        .unwrap();
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![105]);
        // A reload keeps the same closure (and userdata).
        vm.reload_with("push_output(symbol(\"out\"), add_base(7))\n")
            .unwrap();
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![107]);
        assert_eq!(FREED.load(Ordering::SeqCst), 0);
        drop(vm);
        assert_eq!(FREED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reload_compile_errors_are_structured_without_recompiling() {
        let mut vm = Vm::new();
        vm.load(
            "state n = 0\nn += 1\npush_output(symbol(\"out\"), n)\n",
            None,
            "main.ptl".into(),
        )
        .unwrap();
        vm.run().unwrap();
        let err = vm.reload_with("state n = 0\nn += (\n").unwrap_err();
        assert_eq!(err.code, Status::Compile);
        assert!(!err.phase.is_empty());
        assert_eq!(err.items[0].file, "main.ptl");
        assert!(err.items[0].line >= 2);
        // The old program still runs, with its state.
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![2]);
    }

    #[test]
    fn missing_function_is_not_found_and_failures_are_runtime() {
        let mut vm = Vm::new();
        vm.load(
            "fn f(x)\n  assert(x > 0, \"x must be positive\")\n  x\nend\n",
            None,
            "t".into(),
        )
        .unwrap();
        vm.run().unwrap();
        assert!(vm.has_function("f"));
        assert!(!vm.has_function("g"));
        assert_eq!(vm.call("g", &[]).unwrap_err().code, Status::NotFound);
        let err = vm.call("f", &[HostValue::Int(-1)]).unwrap_err();
        assert_eq!(err.code, Status::Runtime);
        assert!(
            err.message.contains("x must be positive"),
            "{}",
            err.message
        );
        assert!(vm.call("f", &[HostValue::Int(3)]).is_ok());
    }

    #[test]
    fn sources_changed_tracks_the_entry_file() {
        let dir = std::env::temp_dir().join(format!("petal-c-bridge-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("main.ptl");
        std::fs::write(&main, "push_output(symbol(\"out\"), 1)\n").unwrap();
        let mut vm = Vm::new();
        let src = std::fs::read_to_string(&main).unwrap();
        vm.load(&src, Some(&main), main.display().to_string())
            .unwrap();
        assert_eq!(vm.source_paths(), vec![main.clone()]);
        assert!(!vm.sources_changed());
        // A length change is detected even within one mtime tick.
        std::fs::write(&main, "push_output(symbol(\"out\"), 22)\n").unwrap();
        assert!(vm.sources_changed());
        vm.reload().unwrap();
        assert!(!vm.sources_changed());
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![22]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scenarios_feed_the_input_state() {
        let scenario = petal_ui::scenario::Scenario::from_json_str(
            r#"{"events": [{"at": 1, "key": "space"}, {"at": 1, "mouse_move": [3, 4]}]}"#,
        )
        .unwrap();
        let mut vm = Vm::new();
        vm.load(
            "push_output(symbol(\"out\"), if key_pressed(\"space\") then 1 else 0 end)\n\
             push_output(symbol(\"out\"), mouse_x())\n",
            None,
            "t".into(),
        )
        .unwrap();
        let mut seen = Vec::new();
        for frame in 0..3 {
            let applied = vm.apply_scenario(&scenario, frame);
            assert_eq!(applied, if frame == 1 { 3 } else { 0 });
            vm.input.begin_frame(1.0 / 60.0);
            petal_ui::input::bind_input(&mut vm.env, &vm.input);
            vm.run().unwrap();
            seen.push(out_ints(&mut vm));
        }
        assert_eq!(seen, vec![vec![0, 0], vec![1, 3], vec![0, 3]]);
    }
}
