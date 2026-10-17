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

use petal::env::{ConfigSetError, Env, ReloadOutcome, ReloadReport};
use petal::static_value::StaticValue;
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
use crate::watch::BrokenWatch;

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
    loaded: Option<Loaded>,
    /// Set by a failed `load_file` or a failed reload of a file-loaded
    /// program, cleared by the next successful one: while it is set, it is
    /// what `sources_changed` / `changed_sources` poll (see `watch.rs`).
    broken: Option<BrokenWatch>,
    input: InputState,
    last_error: Option<Box<PublishedError>>,
    // Everything below is handed out to C and lives until the documented
    // invalidation point. C pointers target the heap buffers *inside* these
    // values, so moving the values themselves (e.g. as the Vecs grow) is fine.
    views: Vec<ViewArena>,
    draws: Vec<DrawList>,
    ui_targets: Vec<crate::draw::TargetList>,
    source_files: CStrList,
    changed_files: CStrList,
    output_lines: CStrList,
    state_json: CString,
    /// How the last successful reload or `set_config` was carried out.
    last_reload: Option<ReloadReport>,
    reload_json: CString,
    source_text: CString,
    profile_report: CString,
    static_values_json: CString,
    diagnostics_text: CString,
    diagnostics_json: CString,
    new_diagnostics_text: CString,
    saved_state: CString,
    /// What the last `save_state` / `load_state` did (`pb_vm_state_report_json`).
    state_report: CString,
    function_json: CString,
    tests_json: CString,
    test_result_json: CString,
    test_in_progress: CString,
    expect_failures_json: CString,
    /// The memo counters when profiling was last turned on: the report shows
    /// what the measured runs added.
    profile_memo_start: Option<petal::memo::MemoStats>,
    package_name: CString,
    /// The last `pb_vm_verify_interface` report.
    interface_report: CString,
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
    /// A VM with the petal-ui natives and the `ui` package registered; a
    /// script reaches them with `import ui`.
    pub fn new() -> Vm {
        let mut env = Env::new();
        // Scripts print through take_output; the host decides where it goes.
        env.set_echo(false);
        petal_ui::register_all(&mut env);
        Vm {
            env,
            loaded: None,
            broken: None,
            input: InputState::new(),
            last_error: None,
            views: Vec::new(),
            draws: Vec::new(),
            ui_targets: Vec::new(),
            source_files: CStrList::default(),
            changed_files: CStrList::default(),
            output_lines: CStrList::default(),
            state_json: CString::default(),
            last_reload: None,
            reload_json: CString::default(),
            source_text: CString::default(),
            profile_report: CString::default(),
            static_values_json: CString::default(),
            diagnostics_text: CString::default(),
            diagnostics_json: CString::default(),
            new_diagnostics_text: CString::default(),
            saved_state: CString::default(),
            state_report: CString::default(),
            function_json: CString::default(),
            tests_json: CString::default(),
            test_result_json: CString::default(),
            test_in_progress: CString::default(),
            expect_failures_json: CString::default(),
            profile_memo_start: None,
            package_name: CString::default(),
            interface_report: CString::default(),
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
        self.ui_targets.clear();
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
        self.broken = None;
        self.last_reload = None;
        Ok(())
    }

    /// Read `path` and [`load`](Self::load) it. On failure (unreadable file
    /// or compile error) the VM watches for a fix: `sources_changed()`
    /// reports an edit to the entry file or to any `.ptl` file under its
    /// directory, and [`reload`](Self::reload) retries this load.
    pub fn load_file(&mut self, path: &Path) -> BResult<()> {
        let result = match std::fs::read_to_string(path) {
            Ok(source) => self.load(&source, Some(path), path.display().to_string()),
            Err(e) => Err(BridgeError::new(
                Status::Io,
                format!("cannot read {}: {e}", path.display()),
            )),
        };
        if result.is_err() {
            self.broken = Some(BrokenWatch::new(path, Vec::new(), true));
        }
        result
    }

    /// A reload of the file-loaded program failed: watch for a fix beyond
    /// the program's own files (see `watch.rs`).
    fn mark_reload_broken(&mut self) {
        let entry = self.loaded.as_ref().and_then(|l| l.entry_path.clone());
        if let Some(entry) = entry {
            self.broken = Some(BrokenWatch::new(&entry, self.source_paths(), false));
        }
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
    /// appeared or disappeared since the last load or reload attempt. After
    /// a failed `load_file` or a failed reload of a file-loaded program, the
    /// watched set also covers every `.ptl` file under the entry file's
    /// directory, including new ones, until a load or reload succeeds.
    pub fn sources_changed(&self) -> bool {
        match &self.broken {
            Some(b) => b.changed(),
            None => self.loaded.as_ref().is_some_and(|l| l.watch.changed()),
        }
    }

    /// The source files [`sources_changed`](Self::sources_changed) would
    /// report, in [`source_paths`](Self::source_paths) order: what a host
    /// names in its "reloaded ..." message, or uses to decide which of its
    /// own caches a reload invalidates. While broken (see
    /// [`sources_changed`](Self::sources_changed)): the entry file first,
    /// then any other watched file, then new `.ptl` files. Empty when
    /// nothing changed or nothing is watched.
    pub fn changed_sources(&self) -> Vec<PathBuf> {
        match &self.broken {
            Some(b) => b.changed_paths(),
            None => self
                .loaded
                .as_ref()
                .map(|l| l.watch.changed_paths())
                .unwrap_or_default(),
        }
    }

    /// Bring the program up to date with `source` as its entry file's text
    /// (keeping the file origin) and with its module files as they are now.
    /// Only the work the edit calls for is done (`Env::reload_program`): a
    /// layout or comment edit moves source positions, a value edit writes
    /// constants in place, anything else recompiles and swaps the program in
    /// with `transfer_state`. [`last_reload`](Self::last_reload) says which.
    /// On a compile error the old program stays loaded.
    pub fn reload_with(&mut self, source: &str) -> BResult<PbReloadResult> {
        let l = self.loaded()?;
        let (stack, origin, name) = (l.stack_id, l.entry_path.clone(), l.entry_name.clone());
        let report = match self.env.reload_program(stack, source, origin.as_deref()) {
            Ok(report) => report,
            Err(e) => {
                // This version of the files has been looked at: don't report
                // it as changed again until it is edited.
                self.refresh_watch();
                self.mark_reload_broken();
                return Err(BridgeError::from_load(&e, &name));
            }
        };
        self.clear_views();
        // The new program may import a different set of files.
        self.refresh_watch();
        self.broken = None;
        let result = PbReloadResult {
            state_preserved: report.state_preserved as u32,
            state_dropped: report.state_dropped as u32,
        };
        self.last_reload = Some(report);
        Ok(result)
    }

    /// What the last successful [`reload`](Self::reload) /
    /// [`reload_with`](Self::reload_with) / [`set_config`](Self::set_config)
    /// did. `None` before the first one.
    pub fn last_reload(&self) -> Option<&ReloadReport> {
        self.last_reload.as_ref()
    }

    /// Set one value of a top-level binding in the running program, without
    /// touching the file (`Env::set_config_value`): `path` is a binding path
    /// (`POST.effects[2].amount`), `value` what it should read, `file` the
    /// source file holding the binding (`None` finds it). The running text
    /// of that file ([`source_text`](Self::source_text)) takes the edit, so
    /// writing it to disk later is a reload with nothing to do.
    pub fn set_config(
        &mut self,
        file: Option<&Path>,
        path: &str,
        value: &StaticValue,
    ) -> BResult<()> {
        let stack = self.loaded()?.stack_id;
        let report = self
            .env
            .set_config_value(stack, file, path, value)
            .map_err(|e| {
                let code = match &e {
                    ConfigSetError::NotFound(_) => Status::NotFound,
                    ConfigSetError::Invalid(_) => Status::InvalidArg,
                    ConfigSetError::NeedsReload(_) => Status::NeedsReload,
                };
                BridgeError::new(code, e.to_string())
            })?;
        if report.outcome != ReloadOutcome::Unchanged {
            self.clear_views();
        }
        self.last_reload = Some(report);
        Ok(())
    }

    /// The text the running program holds for a source file (`None`: the
    /// entry file): what it was compiled from, plus every
    /// [`set_config`](Self::set_config) since.
    pub fn source_text(&self, file: Option<&Path>) -> Option<&str> {
        let l = self.loaded.as_ref()?;
        // The entry file can be named by its path too.
        let entry = l.entry_path.as_deref();
        let file = file.filter(|f| {
            entry.is_none_or(|e| {
                petal::module::canonical_path(e) != petal::module::canonical_path(f)
            })
        });
        self.env.program_source(l.program_id, file)
    }

    /// Re-read the entry file and [`reload_with`](Self::reload_with) it.
    /// After a failed [`load_file`](Self::load_file) there is no program to
    /// carry state from: this retries that load (fresh state; the result
    /// counts nothing preserved or dropped).
    pub fn reload(&mut self) -> BResult<PbReloadResult> {
        if let Some(entry) = self.broken.as_ref().filter(|b| b.retry_load).map(|b| b.entry.clone()) {
            self.load_file(&entry)?;
            return Ok(PbReloadResult {
                state_preserved: 0,
                state_dropped: 0,
            });
        }
        let path = self.loaded()?.entry_path.clone().ok_or_else(|| {
            BridgeError::invalid("pb_vm_reload needs a program loaded with pb_vm_load_file")
        })?;
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.refresh_watch();
                self.mark_reload_broken();
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
        if let Err(e) = self.env.run(stack) {
            // A run that fails stops with its frames still pushed. The next
            // run resets them, but a function called before that (a hook, an
            // agent tool) would run on top of them and list them in the
            // trace of its own failure.
            let _ = self.env.reset_stack(stack);
            return Err(BridgeError::runtime(e, &name));
        }
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
            .map_err(|e| Self::call_error(e, &name))?;
        Ok(self.keep_view(&[result]).0)
    }

    /// Call a top-level function with its arguments given as JSON text: an
    /// array is positional arguments, an object is arguments by parameter
    /// name (a parameter with a default may be left out), `None` is none.
    /// Values are spelled as in a state document ([`save_state`](Self::save_state)),
    /// and a whole number sent for a `float` parameter, or a fraction-less
    /// float for an `int` one, becomes what the parameter declares (see
    /// `call_json.rs` for the whole table). An argument of the wrong kind
    /// for a declared `int`/`float`/`num`/`string`/`bool`/`list`/`record` is
    /// `PB_ERR_INVALID_ARG`, and the function does not run.
    pub fn call_json(&mut self, function: &str, args_json: Option<&str>) -> BResult<*const PbValue> {
        let l = self.loaded()?;
        let (stack, name) = (l.stack_id, l.entry_name.clone());
        let mut args = crate::call_json::parse_args(args_json).map_err(BridgeError::invalid)?;
        if !self.env.has_function(stack, function) {
            return Err(Self::call_error(
                CallError::FunctionNotFound { name: function.to_string() },
                &name,
            ));
        }
        // An overload set has no one declaration to go by: its arguments
        // pass as sent.
        if let Some(params) = self.env.function_params(stack, function) {
            crate::call_json::coerce(function, &params, &mut args).map_err(BridgeError::invalid)?;
        }
        self.clear_views();
        let mut values = Vec::with_capacity(args.values.len());
        for (i, json) in args.values.iter().enumerate() {
            let v = self.env.value_from_storage_json(stack, json).map_err(|e| {
                let which = match args.names.get(i).and_then(Option::as_deref) {
                    Some(n) => format!("argument '{n}'"),
                    None => format!("argument {}", i + 1),
                };
                BridgeError::invalid(format!("{function}(): {which}: {e}"))
            })?;
            values.push(v);
        }
        let names: Vec<Option<&str>> = args.names.iter().map(Option::as_deref).collect();
        let result = self
            .env
            .call_function_named(stack, function, &values, &names)
            .map_err(|e| Self::call_error(e, &name))?;
        Ok(self.keep_view(&[result]).0)
    }

    fn call_error(e: CallError, entry_name: &str) -> BridgeError {
        match e {
            CallError::FunctionNotFound { .. } => BridgeError::new(Status::NotFound, e.to_string()),
            CallError::StackNotFound => BridgeError::new(Status::NotLoaded, e.to_string()),
            CallError::Runtime(msg) => BridgeError::runtime(msg, entry_name),
        }
    }

    /// The declared parameters of a top-level function, as JSON:
    /// `{"name": "toll", "params": [{"name": "cents", "type": "int",
    /// "has_default": true, "default_source": "25"}]}`. `"type"` and
    /// `"default_source"` are `null` where the source has none. `None` when
    /// the last run defined no such function; `"params": null` for one with
    /// no single declaration (an overload set).
    pub fn function_json(&self, function: &str) -> Option<serde_json::Value> {
        let stack = self.loaded.as_ref()?.stack_id;
        if !self.env.has_function(stack, function) {
            return None;
        }
        Some(match self.env.function_params(stack, function) {
            Some(params) => crate::call_json::params_json(function, &params),
            None => serde_json::json!({ "name": function, "params": null }),
        })
    }

    // ── Tests ───────────────────────────────────────────────────────────

    /// The loaded program's `test "title" do … end` blocks (the entry
    /// file's and every module's), as a JSON array in declaration order:
    /// `[{"name": "the door opens", "function": "__test_0", "module": null,
    /// "file": "game.ptl", "path": "/…/game.ptl", "line": 12}]`. `"module"`
    /// is `null` for the entry file; `"function"` is what the `test_*` calls
    /// take.
    pub fn tests_json(&self) -> BResult<serde_json::Value> {
        let l = self.loaded()?;
        Ok(serde_json::Value::Array(
            self.env
                .tests(l.program_id)
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "function": t.function,
                        "module": t.module,
                        "file": t.file,
                        "path": t.path.as_ref().map(|p| p.display().to_string()),
                        "line": t.line,
                    })
                })
                .collect(),
        ))
    }

    fn test_result_value(&self, result: &petal::env::TestResult) -> serde_json::Value {
        let title = self
            .loaded
            .as_ref()
            .and_then(|l| self.env.get_program(l.program_id))
            .and_then(|p| p.test_of_function(&result.function))
            .map(|t| t.name.clone());
        serde_json::json!({
            "function": result.function,
            "name": title,
            "passed": result.passed,
            "checks": result.checks,
            "failed": result.failed,
            "error": result.error,
            "failures": result.failures,
        })
    }

    /// Start the test compiled as `function` (the `"function"` of a
    /// [`tests_json`](Self::tests_json) entry; any top-level function of no
    /// parameters works). Nothing of its body runs until
    /// [`test_resume`](Self::test_resume). The program must have run once
    /// and no other test may be in progress.
    pub fn test_begin(&mut self, function: &str) -> BResult<()> {
        let l = self.loaded()?;
        let (stack, name) = (l.stack_id, l.entry_name.clone());
        self.env
            .test_begin(stack, function)
            .map_err(|e| Self::call_error(e, &name))
    }

    /// Run the test in progress until it finishes, a native suspends it
    /// (`pb_call_suspend`), or about `max_steps` instructions have run
    /// (0 = no bound). When it finishes, the result is kept for
    /// [`test_result`](Self::test_result).
    pub fn test_resume(&mut self, max_steps: u64) -> BResult<petal::env::TestStep> {
        let stack = self.loaded()?.stack_id;
        if self.env.test_in_progress(stack).is_none() {
            return Err(BridgeError::invalid("no test is in progress (pb_vm_test_begin first)"));
        }
        self.clear_views();
        let bound = if max_steps == 0 { u64::MAX } else { max_steps };
        let step = self.env.test_resume_bounded(stack, bound);
        if let petal::env::TestStep::Finished(result) = &step {
            self.test_result_json = cstring_lossy(&self.test_result_value(result).to_string());
        }
        Ok(step)
    }

    /// Give up on the test in progress: what it recorded so far becomes its
    /// result, failed, with `why` as the error.
    pub fn test_abort(&mut self, why: &str) -> BResult<()> {
        let stack = self.loaded()?.stack_id;
        let result = self
            .env
            .test_abort(stack, why)
            .ok_or_else(|| BridgeError::invalid("no test is in progress"))?;
        self.test_result_json = cstring_lossy(&self.test_result_value(&result).to_string());
        Ok(())
    }

    /// Begin and resume in one call, for a test with nothing to wait for; a
    /// test that suspends is aborted with an error saying so. The result is
    /// also what [`test_result`](Self::test_result) returns afterwards.
    pub fn run_test(&mut self, function: &str) -> BResult<&str> {
        let stack = self.loaded()?.stack_id;
        self.clear_views();
        let result = self.env.run_test(stack, function);
        self.test_result_json = cstring_lossy(&self.test_result_value(&result).to_string());
        Ok(self.test_result())
    }

    /// The result of the test that last finished or was aborted, as JSON:
    /// `{"function": "__test_0", "name": "the door opens", "passed": false,
    /// "checks": 3, "failed": 1, "error": null, "failures": [{"check":
    /// "expect_eq", "file": "game.ptl", "line": 14, "column": 3, "message":
    /// "expected 3, got 4", "actual": "4", "expected": "3"}]}`. `"error"` is
    /// the runtime error that ended the test, if one did. Empty before the
    /// first.
    pub fn test_result(&self) -> &str {
        self.test_result_json.to_str().unwrap_or("")
    }

    /// The function of the test in progress (begun and not finished), if any.
    pub fn test_in_progress(&self) -> Option<&str> {
        self.env.test_in_progress(self.loaded.as_ref()?.stack_id)
    }

    /// Take the expectation failures recorded outside tests (by the
    /// program's ordinary runs and the host's own calls) since the last
    /// take: `{"checked": 120, "failed": 2, "failures": [...]}`, the
    /// failures as in [`test_result`](Self::test_result). `checked` and
    /// `failed` count since the program was loaded (or restarted); the log
    /// holds at most 1000 failures between takes and only counts the rest.
    pub fn take_expect_failures(&mut self) -> BResult<serde_json::Value> {
        let stack = self.loaded()?.stack_id;
        let failures = self.env.take_expect_failures(stack);
        let (checked, failed) = self.env.expect_counts(stack);
        Ok(serde_json::json!({ "checked": checked, "failed": failed, "failures": failures }))
    }

    // ── State snapshots ─────────────────────────────────────────────────

    /// Every `state` slot of the program as one JSON document: the format
    /// `petal run --state-storage` writes (`petal::env` `state_storage`).
    /// Top-level and module state, and the slots a declaration inside a
    /// function, a loop or a `state(key)` group owns. [`load_state`](Self::load_state)
    /// puts it back, into this VM or another one running the same program.
    ///
    /// A value that cannot be rebuilt elsewhere — a function, a host handle,
    /// a pending result, a UI element — is not written; the slot holding it
    /// is named in [`state_report`](Self::state_report) and starts from its
    /// initial value after a load.
    pub fn save_state(&mut self) -> BResult<String> {
        let l = self.loaded()?;
        let save = self.env.save_state_storage(l.program_id, l.stack_id);
        let skipped: Vec<serde_json::Value> = save
            .skipped
            .iter()
            .map(|(name, holds)| serde_json::json!({ "name": name, "holds": holds }))
            .collect();
        self.set_state_report(serde_json::json!({
            "op": "save",
            "slots": save.saved,
            "skipped": skipped,
        }));
        Ok(save.document.to_string())
    }

    /// Make the program's state what `document` ([`save_state`](Self::save_state))
    /// holds. The whole state is replaced: a slot the document does not have
    /// starts from its initial value on the next run. Slots are matched by
    /// the ids a hot reload uses, so a document saved before a code edit
    /// loads after it; state the program no longer declares is left out and
    /// named in [`state_report`](Self::state_report).
    ///
    /// `PB_ERR_INVALID_ARG`, with nothing changed, when the text is not a
    /// state document this build reads, or none of its slots is declared by
    /// the loaded program.
    ///
    /// The next run sees the loaded state. A function called before that run
    /// still reads the top-level state its closure captured at the last one.
    pub fn load_state(&mut self, document: &str) -> BResult<()> {
        let l = self.loaded()?;
        let (pid, stack) = (l.program_id, l.stack_id);
        let json: serde_json::Value = serde_json::from_str(document)
            .map_err(|e| BridgeError::invalid(format!("cannot load state: it is not valid JSON ({e})")))?;
        let load = self
            .env
            .restore_state_storage(pid, stack, &json)
            .map_err(|e| BridgeError::invalid(format!("cannot load state: {e}")))?;
        self.clear_views();
        self.set_state_report(serde_json::json!({
            "op": "load",
            "slots": load.restored,
            "dropped": load.dropped,
        }));
        Ok(())
    }

    /// [`save_state`](Self::save_state) to a file (`PB_ERR_IO` if it cannot
    /// be written).
    pub fn save_state_file(&mut self, path: &Path) -> BResult<()> {
        let mut text = self.save_state()?;
        text.push('\n');
        std::fs::write(path, text).map_err(|e| {
            BridgeError::new(Status::Io, format!("cannot write {}: {e}", path.display()))
        })
    }

    /// [`load_state`](Self::load_state) from a file (`PB_ERR_IO` if it
    /// cannot be read).
    pub fn load_state_file(&mut self, path: &Path) -> BResult<()> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            BridgeError::new(Status::Io, format!("cannot read {}: {e}", path.display()))
        })?;
        self.load_state(&text)
    }

    fn set_state_report(&mut self, report: serde_json::Value) {
        self.state_report = cstring_lossy(&report.to_string());
    }

    /// What the last [`save_state`](Self::save_state) /
    /// [`load_state`](Self::load_state) did, as JSON: `{"op": "save",
    /// "slots": 12, "skipped": [{"name": "on_done", "holds": "a function"}]}`
    /// or `{"op": "load", "slots": 11, "dropped": ["old_name"]}`. Empty
    /// before the first one.
    pub fn state_report(&self) -> &str {
        self.state_report.to_str().unwrap_or("")
    }

    /// The findings of the loaded program's compile that did not stop it
    /// (what [`pb_vm_diagnostics_text`] renders), as JSON: `{"errors": 1,
    /// "warnings": 2, "items": [{"severity": "warning", "message": "...",
    /// "file": "game.ptl", "line": 3, "column": 5}]}`. `"file"` is `null`
    /// for a program loaded from a string with no name.
    pub fn diagnostics_json(&self) -> Option<serde_json::Value> {
        let program = self.env.get_program(self.loaded.as_ref()?.program_id)?;
        let items = diagnostic_items(program, &program.warnings);
        let errors = program.warnings.iter().filter(|d| d.is_error()).count();
        Some(serde_json::json!({
            "errors": errors,
            "warnings": items.len() - errors,
            "items": items,
        }))
    }

    /// What the last reload's compile found that the program before the edit
    /// did not have (`ReloadReport::new_diagnostics`), rendered as `petal
    /// check` prints findings: what a host prints after a reload, where
    /// printing every standing warning again on every save would bury the
    /// one the edit introduced. `errors_only` leaves the warnings out. Empty
    /// when the edit introduced nothing, when the reload did not recompile
    /// (a layout or value edit is not checked again), and before the first
    /// reload.
    pub fn new_diagnostics_text(&self, errors_only: bool) -> String {
        let (Some(l), Some(report)) = (&self.loaded, &self.last_reload) else {
            return String::new();
        };
        if errors_only {
            let errors: Vec<_> =
                report.new_diagnostics.iter().filter(|d| d.is_error()).cloned().collect();
            self.env.render_diagnostics(l.program_id, &errors)
        } else {
            self.env.render_diagnostics(l.program_id, &report.new_diagnostics)
        }
    }

    /// Start the loaded program over with fresh `state`, without
    /// recompiling: a new stack on the same program. What a host wants when
    /// it re-enters a screen it already has loaded, or between two test cases
    /// of one script.
    pub fn restart(&mut self) -> BResult<()> {
        let l = self.loaded()?;
        let (program_id, name) = (l.program_id, l.entry_name.clone());
        let stack_id = self
            .env
            .create_stack(program_id)
            .map_err(|e| BridgeError::runtime(e, &name))?;
        self.clear_views();
        if let Some(l) = &mut self.loaded {
            l.stack_id = stack_id;
        }
        Ok(())
    }

    /// Start a frame: promote the input edges by `dt` seconds and bind
    /// `dt()`, `frame_count()` and the absolute clock `time()` (monotonic
    /// seconds, not a sum of `dt`). What `pb_vm_begin_frame` does.
    pub fn begin_frame(&mut self, dt: f64, frame: i64, time_seconds: f64) {
        self.input.begin_frame(dt);
        petal_ui::input::bind_input(&mut self.env, &self.input);
        petal_ui::input::bind_frame_info(&mut self.env, dt, frame);
        petal_ui::input::bind_time(&mut self.env, time_seconds);
    }

    /// Replace the error `pb_vm_last_error` reports (`None` clears it).
    ///
    /// For a Rust crate that layers its own C entry points over a `pb_vm`
    /// (see [`VmHandle::into_raw`]): its failures then reach the host through
    /// the same structured `pb_error` as the bridge's own.
    pub fn set_last_error(&mut self, err: Option<BridgeError>) {
        self.last_error = err.map(PublishedError::new);
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
        // Named-target clicks resolve against what the last run registered.
        let left_down = self.input.button_down(petal_ui::input::buttons::LEFT);
        let env = &mut self.env;
        let (events, _missing) = scenario.events_at(frame, left_down, |id| {
            petal_ui::hit::target(env, id).map(|t| t.center())
        });
        let n = events.len();
        for ev in events {
            self.input.event(ev);
        }
        n
    }
}

#[cfg(test)]
impl Vm {
    fn into_handle_for_test() -> *mut VmHandle {
        VmHandle::into_raw(Vm::new())
    }
}

impl VmHandle {
    /// Box `vm` behind a `pb_vm*`, for a Rust crate that configures a [`Vm`]
    /// itself (registers its own natives, loads its own program) and then
    /// hands the host a `pb_vm*` for everything generic: input, draw
    /// commands, errors, bindings. Free it with `pb_vm_destroy`.
    pub fn into_raw(vm: Vm) -> *mut VmHandle {
        Box::into_raw(Box::new(VmHandle {
            busy: Cell::new(false),
            vm: UnsafeCell::new(vm),
        }))
    }

    /// The [`Vm`] behind `ptr`, for the crate that owns the handle. `None`
    /// when `ptr` is NULL or the VM is mid-call (a native running inside it),
    /// where a second `&mut Vm` would alias the one in use.
    ///
    /// # Safety
    /// `ptr` must come from [`VmHandle::into_raw`] or `pb_vm_create` and not
    /// be destroyed, and the returned reference must be dropped before any
    /// `pb_vm_*` call on the same handle.
    pub unsafe fn vm_mut<'a>(ptr: *mut VmHandle) -> Option<&'a mut Vm> {
        let handle = unsafe { ptr.as_ref() }?;
        if handle.busy.get() {
            return None;
        }
        Some(unsafe { &mut *handle.vm.get() })
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
    catch_unwind(|| VmHandle::into_raw(Vm::new())).unwrap_or(std::ptr::null_mut())
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

/// Compile as if the file at `path` held `source` (`Env::override_file_source`):
/// every later import that resolves to that file reads this text instead of
/// the one on disk, and a reload compiles it again, so edits to the file
/// itself do not reach the program while the override stands. The entry file
/// is not an import: load it with `pb_vm_load_source` / `pb_vm_reload_source`
/// to run other text in its place. For a host that rewrites a script before
/// running it (a `--set NAME=VALUE` that replaces a `config let` in an
/// imported module) without touching the file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_override_file(
    vm: *mut VmHandle,
    path: *const c_char,
    source: *const c_char,
) -> Status {
    status(vm, |vm| {
        let path = unsafe { arg_str(path, "path") }?;
        let source = unsafe { arg_str(source, "source") }?;
        vm.env.override_file_source(Path::new(path), source);
        Ok(())
    })
}

/// Register a package from memory: `count` modules, `names[i]` the module's
/// package-relative name and `sources[i]` its source.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_register_package(
    vm: *mut VmHandle,
    name: *const c_char,
    names: *const *const c_char,
    sources: *const *const c_char,
    count: usize,
) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        if count > 0 && (names.is_null() || sources.is_null()) {
            return Err(BridgeError::invalid("module_names or sources is NULL"));
        }
        let mut modules = Vec::with_capacity(count);
        for i in 0..count {
            // SAFETY: the caller passes `count` entries in each array.
            let (module, source) = unsafe { (*names.add(i), *sources.add(i)) };
            modules.push((
                unsafe { arg_str(module, "module_names[i]") }?,
                unsafe { arg_str(source, "sources[i]") }?,
            ));
        }
        vm.env.register_package(name, modules).map_err(|e| {
            let mut err = BridgeError::from_load(&e, name);
            err.code = Status::InvalidArg;
            err
        })?;
        Ok(())
    })
}

/// Announce that this VM provides the host interface `name` at `version`
/// (`Env::provides`): a package whose `petal.toml` says `[requires] name =
/// ">=…"` loads only in a VM that provides it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_provides(
    vm: *mut VmHandle,
    name: *const c_char,
    version: *const c_char,
) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        let version = unsafe { arg_str(version, "version") }?;
        vm.env.provides(name, version).map_err(BridgeError::invalid)
    })
}

/// [`pb_vm_provides`] for a package registered in this VM, at the version
/// its manifest gives (`Env::provides_package`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_provides_package(vm: *mut VmHandle, name: *const c_char) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        vm.env
            .provides_package(name)
            .map_err(|e| BridgeError::new(Status::NotFound, e))
    })
}

/// Check the VM's registered natives against the interface packages that
/// declare them (`Env::verify_interface`). `*out_report` receives an empty
/// string when they agree, else what differs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_verify_interface(
    vm: *mut VmHandle,
    packages: *const *const c_char,
    count: usize,
    out_report: *mut *const c_char,
) -> Status {
    status(vm, |vm| {
        if count > 0 && packages.is_null() {
            return Err(BridgeError::invalid("packages is NULL"));
        }
        let mut names = Vec::with_capacity(count);
        for i in 0..count {
            // SAFETY: the caller passes `count` entries.
            names.push(unsafe { arg_str(*packages.add(i), "packages[i]") }?);
        }
        let result = vm.env.verify_interface(&names);
        let report = result.as_ref().err().map(|m| m.to_string()).unwrap_or_default();
        vm.interface_report = cstring_lossy(&report);
        put(out_report, vm.interface_report.as_ptr());
        match result {
            Ok(()) => Ok(()),
            Err(_) => Err(BridgeError::new(Status::NotFound, report)),
        }
    })
}

/// Check-only mode: programs loaded afterwards compile without being linked
/// against the VM's natives.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_check_only(vm: *mut VmHandle, on: bool) -> Status {
    status(vm, |vm| {
        vm.env.set_check_only(on);
        Ok(())
    })
}

/// Where `import ./x` resolves for a program loaded from a string (NULL:
/// against registered modules only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_inline_import_dir(
    vm: *mut VmHandle,
    dir: *const c_char,
) -> Status {
    status(vm, |vm| {
        let dir = if dir.is_null() {
            None
        } else {
            Some(PathBuf::from(unsafe { arg_str(dir, "dir") }?))
        };
        vm.env.set_inline_import_dir(dir);
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_declare_native_params(
    vm: *mut VmHandle,
    name: *const c_char,
    params: *const c_char,
) -> Status {
    status(vm, |vm| {
        let name = unsafe { arg_str(name, "name") }?;
        let params = unsafe { arg_str(params, "params") }?;
        vm.env
            .declare_native_params_by_name(name, params)
            .map_err(BridgeError::invalid)
    })
}

// ── Loading ──────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_load_file(vm: *mut VmHandle, path: *const c_char) -> Status {
    status(vm, |vm| {
        let path = unsafe { arg_str(path, "path") }?;
        vm.load_file(Path::new(path))
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
    bind_with(vm, name, |vm| Ok(vm.env.heap_mut().vec3_value(x, y, z)))
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
        vm.begin_frame(dt, frame, time_seconds);
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
pub unsafe extern "C" fn pb_vm_set_text_advances(
    vm: *mut VmHandle,
    advances: *const f64,
    n: usize,
) -> Status {
    status(vm, |vm| {
        let table: &[f64] = if advances.is_null() || n == 0 {
            &[]
        } else {
            // SAFETY: the caller passes `n` readable doubles.
            unsafe { std::slice::from_raw_parts(advances, n) }
        };
        petal_ui::text::bind_text_advance_table(&mut vm.env, table);
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
pub extern "C" fn pb_vm_restart(vm: *mut VmHandle) -> Status {
    status(vm, |vm| vm.restart())
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

/// Call a top-level function with JSON arguments (see [`Vm::call_json`]).
/// `args_json` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_call_json(
    vm: *mut VmHandle,
    function: *const c_char,
    args_json: *const c_char,
    out_result: *mut *const PbValue,
) -> Status {
    let (st, result) = with_vm(vm, std::ptr::null(), |vm| {
        let function = unsafe { arg_str(function, "function") }?;
        let args = if args_json.is_null() {
            None
        } else {
            Some(unsafe { arg_str(args_json, "args_json") }?)
        };
        vm.call_json(function, args)
    });
    if st == Status::Ok {
        put(out_result, result);
    }
    st
}

/// A function's declared parameters as JSON (see [`Vm::function_json`]);
/// NULL (`PB_ERR_NOT_FOUND`) when the last run defined no such function.
/// Valid until the next call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_function_json(
    vm: *mut VmHandle,
    function: *const c_char,
) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        let function = unsafe { arg_str(function, "function") }?;
        vm.loaded()?;
        let json = vm.function_json(function).ok_or_else(|| {
            BridgeError::new(
                Status::NotFound,
                CallError::FunctionNotFound { name: function.to_string() }.to_string(),
            )
        })?;
        vm.function_json = cstring_lossy(&json.to_string());
        Ok(vm.function_json.as_ptr())
    })
    .1
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

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_draw_caps(vm: *mut VmHandle, caps: u32) -> Status {
    status(vm, |vm| {
        petal_ui::compose::set_host_caps(
            &mut vm.env,
            petal_ui::compose::HostCaps::from_bits(caps as i64),
        );
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_invalidate_layers(vm: *mut VmHandle) -> Status {
    status(vm, |vm| {
        petal_ui::compose::invalidate_caches(&mut vm.env);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_ui_targets(
    vm: *mut VmHandle,
    out_targets: *mut *const crate::draw::PbUiTarget,
    out_count: *mut usize,
) -> Status {
    let (st, (targets, count)) = with_vm(vm, (std::ptr::null(), 0), |vm| {
        let mut list = crate::draw::TargetList::default();
        for t in petal_ui::hit::targets(&mut vm.env) {
            list.push(&t);
        }
        let out = (list.targets.as_ptr(), list.targets.len());
        vm.ui_targets.push(list);
        Ok(out)
    });
    put(out_targets, targets);
    put(out_count, count);
    st
}

/// # Safety
/// `id` must be NULL or a NUL-terminated string; `out` NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_ui_target(
    vm: *mut VmHandle,
    id: *const c_char,
    out: *mut crate::draw::PbUiTarget,
) -> i32 {
    let (st, found) = with_vm(vm, None, |vm| {
        // SAFETY: the caller's contract on `id`.
        let id = unsafe { arg_str(id, "id") }?;
        Ok(petal_ui::hit::target(&mut vm.env, id).map(|t| {
            let mut list = crate::draw::TargetList::default();
            list.push(&t);
            let one = list.targets[0];
            vm.ui_targets.push(list);
            one
        }))
    });
    match (st, found) {
        (Status::Ok, Some(t)) => {
            put(out, t);
            1
        }
        (Status::Ok, None) => 0,
        _ => -1,
    }
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
pub extern "C" fn pb_vm_changed_sources(
    vm: *mut VmHandle,
    out_paths: *mut *const *const c_char,
    out_count: *mut usize,
) -> Status {
    let (st, (paths, count)) = with_vm(vm, (std::ptr::null(), 0), |vm| {
        let paths: Vec<String> = vm
            .changed_sources()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        vm.changed_files.set(paths);
        Ok((vm.changed_files.ptrs.as_ptr(), vm.changed_files.ptrs.len()))
    });
    put(out_paths, paths);
    put(out_count, count);
    st
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

/// Mirrors `pb_reload_kind`.
fn reload_kind(outcome: ReloadOutcome) -> i32 {
    match outcome {
        ReloadOutcome::Unchanged => 0,
        ReloadOutcome::Relocated => 1,
        ReloadOutcome::Patched => 2,
        ReloadOutcome::Recompiled => 3,
    }
}

/// The JSON `pb_vm_last_reload_json` hands out.
fn reload_report_json(report: &ReloadReport) -> serde_json::Value {
    use petal::ast::Literal;
    use petal::source_diff::{ConstructEdit, ConstructKind, SourceChange};
    use serde_json::json;
    let literal = |l: &Literal| match l {
        Literal::Nil => serde_json::Value::Null,
        Literal::Bool(b) => json!(b),
        Literal::Int(n) => json!(n),
        Literal::Float(f) => json!(f),
        Literal::String(s) => json!(s),
    };
    let mut out = json!({
        "outcome": report.outcome.label(),
        "change": report.change.label(),
        "files": report.changed_files,
        "state_preserved": report.state_preserved,
        "state_dropped": report.state_dropped,
    });
    match &report.change {
        SourceChange::Values(values) => {
            out["values"] = values
                .iter()
                .map(|v| {
                    json!({
                        "path": v.path,
                        "config": v.config,
                        "old": literal(&v.old),
                        "new": literal(&v.new),
                        "line": v.new_span.start.line,
                    })
                })
                .collect();
        }
        SourceChange::Constructs(constructs) => {
            out["constructs"] = constructs
                .iter()
                .map(|c| {
                    json!({
                        "kind": match c.kind {
                            ConstructKind::Function => "fn",
                            ConstructKind::Let => "let",
                            ConstructKind::ConfigLet => "config let",
                            ConstructKind::Const => "const",
                            ConstructKind::Var => "var",
                            ConstructKind::State => "state",
                            ConstructKind::Enum => "enum",
                            ConstructKind::Class => "class",
                            ConstructKind::Import => "import",
                            ConstructKind::Statement => "statement",
                        },
                        "name": c.name,
                        "edit": match c.edit {
                            ConstructEdit::Added => "added",
                            ConstructEdit::Removed => "removed",
                            ConstructEdit::Changed => "changed",
                        },
                        "line": c.span.start.line,
                    })
                })
                .collect();
        }
        SourceChange::Full(why) => out["reason"] = json!(why),
        SourceChange::None => {}
    }
    if let Some(why) = &report.fallback {
        out["fallback"] = json!(why);
    }
    // What became of the static values, and the stale seeds: present when
    // the program runs with the static-value cache.
    if let Some(statics) = &report.statics {
        out["statics"] = statics.to_json();
    }
    out
}

/// Diagnostics as the items of `pb_vm_diagnostics_json`.
fn diagnostic_items(
    program: &petal::program::Program,
    diagnostics: &[petal::diagnostic::Diagnostic],
) -> Vec<serde_json::Value> {
    diagnostics
        .iter()
        .map(|d| {
            serde_json::json!({
                "severity": d.severity.label(),
                "message": d.message,
                "file": program.source_map.file_name_for_span(&d.span),
                "line": d.span.start.line,
                "column": d.span.start.column,
            })
        })
        .collect()
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_last_reload_kind(vm: *mut VmHandle) -> i32 {
    with_vm(vm, -1, |vm| {
        Ok(vm.last_reload.as_ref().map_or(-1, |r| reload_kind(r.outcome)))
    })
    .1
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_last_reload_json(vm: *mut VmHandle) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        let Some(report) = &vm.last_reload else {
            return Ok(std::ptr::null());
        };
        let mut json = reload_report_json(report);
        // What the edit introduced, as the items of pb_vm_diagnostics_json.
        let program = vm.loaded.as_ref().and_then(|l| vm.env.get_program(l.program_id));
        json["new_diagnostics"] = match program {
            Some(p) => diagnostic_items(p, &report.new_diagnostics).into(),
            None => serde_json::json!([]),
        };
        vm.reload_json = cstring_lossy(&json.to_string());
        Ok(vm.reload_json.as_ptr())
    })
    .1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_set_config(
    vm: *mut VmHandle,
    file: *const c_char,
    path: *const c_char,
    value_json: *const c_char,
) -> Status {
    with_vm(vm, (), |vm| {
        let file = if file.is_null() {
            None
        } else {
            Some(PathBuf::from(unsafe { arg_str(file, "file") }?))
        };
        let path = unsafe { arg_str(path, "path") }?;
        let json = unsafe { arg_str(value_json, "value_json") }?;
        let value = petal::static_json::from_json(json)
            .map_err(|e| BridgeError::invalid(format!("value_json: {e}")))?;
        vm.set_config(file.as_deref(), path, &value)
    })
    .0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_source_text(vm: *mut VmHandle, file: *const c_char) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        vm.loaded()?;
        let file = if file.is_null() {
            None
        } else {
            Some(PathBuf::from(unsafe { arg_str(file, "file") }?))
        };
        let text = vm
            .source_text(file.as_deref())
            .ok_or_else(|| {
                BridgeError::new(Status::NotFound, "not a source file of the loaded program")
            })?
            .to_string();
        vm.source_text = cstring_lossy(&text);
        Ok(vm.source_text.as_ptr())
    })
    .1
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

/// The whole state as a document `pb_vm_load_state` reads (see
/// [`Vm::save_state`]); NULL with no program loaded. Valid until the next
/// call.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_save_state(vm: *mut VmHandle) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        let text = vm.save_state()?;
        vm.saved_state = cstring_lossy(&text);
        Ok(vm.saved_state.as_ptr())
    })
    .1
}

/// Replace the state with a saved document (see [`Vm::load_state`]).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_load_state(vm: *mut VmHandle, document: *const c_char) -> Status {
    status(vm, |vm| {
        let document = unsafe { arg_str(document, "document") }?;
        vm.load_state(document)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_save_state_file(vm: *mut VmHandle, path: *const c_char) -> Status {
    status(vm, |vm| {
        let path = unsafe { arg_str(path, "path") }?;
        vm.save_state_file(Path::new(path))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_load_state_file(vm: *mut VmHandle, path: *const c_char) -> Status {
    status(vm, |vm| {
        let path = unsafe { arg_str(path, "path") }?;
        vm.load_state_file(Path::new(path))
    })
}

/// What the last save or load of state did, as JSON (see
/// [`Vm::state_report`]); "" before the first. Valid until the next save or
/// load. Unlike the other entry points this one leaves `pb_vm_last_error`
/// alone, so it can be read after a load that failed.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_state_report_json(vm: *const VmHandle) -> *const c_char {
    if vm.is_null() {
        return std::ptr::null();
    }
    // SAFETY: as in pb_vm_last_error.
    let handle = unsafe { &*vm };
    if handle.busy.get() {
        return std::ptr::null();
    }
    unsafe { &*handle.vm.get() }.state_report.as_ptr()
}

/// Turn call memoization on or off for this VM's runs (the
/// [`RunPolicy::memo`](petal::policy::RunPolicy::memo) switch; on by
/// default, or as `PETAL_POLICY` says). Memo pays off for UI-like programs
/// whose inputs mostly repeat frame to frame; a game whose inputs change
/// every frame runs faster without the bookkeeping. Output is the same
/// either way. Takes effect at the next run; the first run without memo
/// drops the records it leaves unvisited.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_memo(vm: *mut VmHandle, on: bool) -> Status {
    with_vm(vm, (), |vm| {
        let policy = vm.env.policy().with_memo(on);
        vm.env.set_policy(policy);
        Ok(())
    })
    .0
}

/// Whether call memoization is on (see [`pb_vm_set_memo`]).
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_memo(vm: *mut VmHandle) -> bool {
    with_vm(vm, false, |vm| Ok(vm.env.policy().memo)).1
}

/// Turn the static-value cache on or off for this VM's runs (the
/// [`OptFlags::static_cache`](petal::backend::OptFlags) switch; on by
/// default, or as `PETAL_POLICY` says). With it on, a top-level value that
/// depends on no dynamic data is built by the first run after a load or a
/// reload and kept for the runs after it. Output is the same either way.
/// Takes effect at the next run, which lowers the program again.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_static_cache(vm: *mut VmHandle, on: bool) -> Status {
    with_vm(vm, (), |vm| {
        vm.env.set_static_cache(on);
        Ok(())
    })
    .0
}

/// Whether runs keep static values right now (see
/// [`pb_vm_set_static_cache`]): the switch is on and nothing that reads
/// per-instruction results is.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_static_cache(vm: *mut VmHandle) -> bool {
    with_vm(vm, false, |vm| Ok(vm.env.static_cache_enabled())).1
}

/// Drop every static value the loaded program's stack keeps; the next run
/// builds them again. A reload drops only the ones the edit reaches. This is
/// for a host that changed something a native it registered `PB_FX_CONST`
/// answers from, which is the one thing the cache cannot see.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_clear_static_values(vm: *mut VmHandle) -> Status {
    with_vm(vm, (), |vm| {
        let stack = vm.loaded()?.stack_id;
        vm.env.clear_static_values(stack);
        Ok(())
    })
    .0
}

/// Whether a reload keeps the static values the edit cannot have changed
/// (`true`, the default) or rebuilds every one (`Env::set_static_follow`).
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_static_follow(vm: *mut VmHandle, on: bool) -> Status {
    with_vm(vm, (), |vm| {
        vm.env.set_static_follow(on);
        Ok(())
    })
    .0
}

/// How a slow static rebuild waits while `pb_vm_set_config` is driven
/// continuously (`Env::set_static_debounce`): a value whose last build took
/// at least `threshold_ms` keeps its old value for every tick after the
/// first of a drag, and is rebuilt by the first run that starts `quiet_ms`
/// after the last tick. A negative `threshold_ms` rebuilds on every tick.
/// The default is 4 and 150.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_static_debounce(
    vm: *mut VmHandle,
    threshold_ms: f64,
    quiet_ms: f64,
) -> Status {
    with_vm(vm, (), |vm| {
        let ms = |v: f64| std::time::Duration::from_secs_f64(v.max(0.0) / 1000.0);
        vm.env.set_static_debounce((threshold_ms >= 0.0).then(|| {
            petal::static_cache::StaticDebounce {
                threshold: ms(threshold_ms),
                quiet: ms(quiet_ms),
            }
        }));
        Ok(())
    })
    .0
}

/// Rebuild on the next run what a drag put off, without waiting for it to
/// pause (`Env::flush_static_rebuilds`): for a host that knows the drag
/// ended. Returns how many values were waiting; -1 with no program loaded.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_flush_static_rebuilds(vm: *mut VmHandle) -> i32 {
    with_vm(vm, -1, |vm| {
        let stack = vm.loaded()?.stack_id;
        Ok(vm.env.flush_static_rebuilds(stack) as i32)
    })
    .1
}

/// What the static-value cache holds for the loaded program, as JSON:
///
/// ```json
/// {"enabled": true, "slots": 12, "filled": 11, "runs": 4,
///  "skips": 96, "skipped_instructions": 4810, "clears": 1,
///  "rejected": [{"name": "ICONS", "at": "ui.ptl:12", "const": true,
///                "holds": "a function"}],
///  "warnings": ["const ICONS (ui.ptl:12) holds a function, ..."]}
/// ```
///
/// `rejected` lists every value that could not be kept and is therefore
/// built on every run; `warnings` is the same in words, and only what was
/// not handed out by an earlier call since the last load or reload, so a
/// host can log it after each frame and say each thing once. The counters
/// are since the stack was created. Valid until the next call.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_static_values_json(vm: *mut VmHandle) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        let stack = vm.loaded()?.stack_id;
        let warnings = vm.env.take_static_warnings(stack);
        let values = vm.env.static_values(stack).unwrap_or_default();
        let filled = values
            .slots
            .iter()
            .filter(|(_, s)| matches!(s, petal::env::StaticSlotState::Filled(_)))
            .count();
        let rejected: Vec<serde_json::Value> = values
            .slots
            .iter()
            .filter_map(|(info, state)| match state {
                petal::env::StaticSlotState::Rejected(what) => Some(serde_json::json!({
                    "name": info.name,
                    "at": info.at,
                    "const": info.is_const,
                    "holds": what,
                })),
                _ => None,
            })
            .collect();
        let text = serde_json::json!({
            "enabled": vm.env.static_cache_enabled(),
            "slots": values.slots.len(),
            "filled": filled,
            "runs": values.runs,
            "skips": values.stats.skips,
            "skipped_instructions": values.stats.skipped_instructions,
            "clears": values.stats.clears,
            "rejected": rejected,
            "warnings": warnings,
        })
        .to_string();
        vm.static_values_json = cstring_lossy(&text);
        Ok(vm.static_values_json.as_ptr())
    })
    .1
}

/// What the compile of the loaded program found and did not stop for,
/// rendered as `petal check` prints it: "" when there is nothing, NULL with
/// no program loaded. `errors_only` leaves the warnings out. The finding a
/// host should always print is a `const` that is not constant: the program
/// loads and runs, with that binding built on every run as a `let` is. It
/// is judged against the natives this VM has registered, so only the host
/// can report it. Valid until the next call.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_diagnostics_text(vm: *mut VmHandle, errors_only: bool) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        let pid = vm.loaded()?.program_id;
        let text = if errors_only {
            vm.env.error_diagnostics_text(pid)
        } else {
            vm.env.diagnostics_text(pid)
        };
        vm.diagnostics_text = cstring_lossy(&text);
        Ok(vm.diagnostics_text.as_ptr())
    })
    .1
}

/// The same findings as [`pb_vm_diagnostics_text`], as JSON (see
/// [`Vm::diagnostics_json`]); NULL with no program loaded. Valid until the
/// next call.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_diagnostics_json(vm: *mut VmHandle) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        vm.loaded()?;
        let json = vm.diagnostics_json().unwrap_or(serde_json::Value::Null);
        vm.diagnostics_json = cstring_lossy(&json.to_string());
        Ok(vm.diagnostics_json.as_ptr())
    })
    .1
}

/// What the last reload introduced, as text (see
/// [`Vm::new_diagnostics_text`]); "" when nothing. Valid until the next
/// call.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_new_diagnostics_text(vm: *mut VmHandle, errors_only: bool) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        vm.loaded()?;
        vm.new_diagnostics_text = cstring_lossy(&vm.new_diagnostics_text(errors_only));
        Ok(vm.new_diagnostics_text.as_ptr())
    })
    .1
}

/// Turn the VM profiler on or off. Turning it on clears earlier counts.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_profiling(vm: *mut VmHandle, on: bool) -> Status {
    with_vm(vm, (), |vm| {
        vm.env.profile_mut().set_enabled(on);
        vm.profile_memo_start = vm.loaded.as_ref().and_then(|l| vm.env.memo_stats(l.stack_id));
        Ok(())
    })
    .0
}

/// The time at or above which a native call counts as slow in the profile
/// report's "native calls of N µs or longer" section. Kept across
/// pb_vm_set_profiling.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_set_profile_slow_native_us(vm: *mut VmHandle, microseconds: f64) -> Status {
    status(vm, |vm| {
        if !microseconds.is_finite() || microseconds < 0.0 {
            return Err(BridgeError::invalid("microseconds must be a finite number >= 0"));
        }
        vm.env.profile_mut().slow_native = std::time::Duration::from_secs_f64(microseconds / 1e6);
        Ok(())
    })
}

// ── Tests ────────────────────────────────────────────────────────────────

/// See [`Vm::tests_json`]. NULL with no program loaded. Valid until the next
/// call.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_tests_json(vm: *mut VmHandle) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        vm.tests_json = cstring_lossy(&vm.tests_json()?.to_string());
        Ok(vm.tests_json.as_ptr())
    })
    .1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_test_begin(vm: *mut VmHandle, function: *const c_char) -> Status {
    status(vm, |vm| {
        let function = unsafe { arg_str(function, "function") }?;
        vm.test_begin(function)
    })
}

/// Returns a `pb_test_step` (0 finished, 1 suspended, 2 out of steps), or -1
/// on failure (no test in progress, no program).
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_test_resume(vm: *mut VmHandle, max_steps: u64) -> i32 {
    with_vm(vm, -1, |vm| {
        Ok(match vm.test_resume(max_steps)? {
            petal::env::TestStep::Finished(_) => 0,
            petal::env::TestStep::Suspended => 1,
            petal::env::TestStep::OutOfSteps => 2,
        })
    })
    .1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_test_abort(vm: *mut VmHandle, why: *const c_char) -> Status {
    status(vm, |vm| {
        let why = if why.is_null() { "aborted by the host" } else { unsafe { arg_str(why, "why") }? };
        vm.test_abort(why)
    })
}

/// The result JSON, or NULL on failure. Valid until the next test finishes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_vm_run_test(vm: *mut VmHandle, function: *const c_char) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        let function = unsafe { arg_str(function, "function") }?;
        vm.run_test(function)?;
        Ok(vm.test_result_json.as_ptr())
    })
    .1
}

/// "" before the first finished test. Does not touch `pb_vm_last_error`.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_test_result_json(vm: *const VmHandle) -> *const c_char {
    // SAFETY: as pb_vm_state_report_json — a read of a buffer no call in
    // progress can be replacing, since a busy VM is refused.
    match unsafe { vm.as_ref() } {
        Some(handle) if !handle.busy.get() => unsafe { &*handle.vm.get() }.test_result_json.as_ptr(),
        _ => c"".as_ptr(),
    }
}

/// The function of the test in progress, or NULL when there is none. Valid
/// until the next call.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_test_in_progress(vm: *mut VmHandle) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        Ok(match vm.test_in_progress().map(str::to_string) {
            Some(f) => {
                vm.test_in_progress = cstring_lossy(&f);
                vm.test_in_progress.as_ptr()
            }
            None => std::ptr::null(),
        })
    })
    .1
}

/// See [`Vm::take_expect_failures`]. NULL with no program loaded. Valid
/// until the next call.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_take_expect_failures_json(vm: *mut VmHandle) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        vm.expect_failures_json = cstring_lossy(&vm.take_expect_failures()?.to_string());
        Ok(vm.expect_failures_json.as_ptr())
    })
    .1
}

/// The profiler's report for the loaded program since profiling was turned on.
#[unsafe(no_mangle)]
pub extern "C" fn pb_vm_profile_report(vm: *mut VmHandle, top_n: usize) -> *const c_char {
    with_vm(vm, std::ptr::null(), |vm| {
        let (pid, stack) = (vm.loaded()?.program_id, vm.loaded()?.stack_id);
        let mut text = vm.env.profile_report(pid, None, top_n);
        if let Some(now) = vm.env.memo_stats(stack) {
            let was = vm.profile_memo_start.unwrap_or_default();
            let d = |a: u64, b: u64| a.saturating_sub(b);
            text.push_str(&format!(
                "\n  memo (call memoization, over the measured runs):\n    \
                 hits {}  misses {}  records {}  inlined {}  effectful {}  cold {}  tiny {}  evicted {}\n",
                d(now.hits, was.hits),
                d(now.misses, was.misses),
                d(now.records, was.records),
                d(now.inlined, was.inlined),
                d(now.effectful, was.effectful),
                d(now.cold, was.cold),
                d(now.tiny, was.tiny),
                d(now.evicted, was.evicted),
            ));
        }
        vm.profile_report = cstring_lossy(&text);
        Ok(vm.profile_report.as_ptr())
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

    /// `PB_FX_CONST` is the only way a C native becomes a constant function:
    /// flags `0` (`PB_FX_PURE`) stay dynamic, and the flag is ignored next
    /// to an effect or a read.
    #[test]
    fn only_pb_fx_const_makes_a_native_constant() {
        use petal::native_fn::NativeConstness;
        let mut vm = Vm::new();
        for (name, flags, want) in [
            ("k_pure", 0, NativeConstness::Dynamic),
            ("k_const", natives::FX_CONST, NativeConstness::Const),
            (
                "k_const_pending",
                natives::FX_CONST | natives::FX_PENDING_ALLOW,
                NativeConstness::Const,
            ),
            (
                "k_const_effect",
                natives::FX_CONST | natives::FX_EFFECT,
                NativeConstness::Dynamic,
            ),
            (
                "k_const_reads",
                natives::FX_CONST | natives::FX_READS_HOST_DATA,
                NativeConstness::Dynamic,
            ),
            (
                "k_const_probe",
                natives::FX_CONST | natives::FX_PROBE,
                NativeConstness::Dynamic,
            ),
            ("k_effect", natives::FX_EFFECT, NativeConstness::Dynamic),
        ] {
            let cb = HostCallback::new(name, flags, add_one, std::ptr::null_mut(), None);
            let id = natives::register_callback(&mut vm.env, cb);
            assert_eq!(vm.env.native_constness(id), want, "{name}");
            assert_eq!(vm.env.native_constness_of_name(name), want, "{name}");
        }
        let sym = vm.env.intern_symbol("out");
        let id = natives::register_emitter(&mut vm.env, "k_emit", sym, "tag");
        assert_eq!(vm.env.native_constness(id), NativeConstness::Dynamic);
    }

    #[test]
    fn boxed_native_owns_and_frees_its_userdata() {
        FREED.store(0, Ordering::SeqCst);
        let mut vm = Vm::new();
        let ud = Box::into_raw(Box::new(100i64)) as *mut c_void;
        let cb = HostCallback::new("add_base", 0, add_one, ud, Some(free_i64));
        natives::register_callback(&mut vm.env, cb);
        vm.load(
            "import io; native fn add_base(n)\npush_output(symbol(\"out\"), add_base(5))\n",
            None,
            "t".into(),
        )
        .unwrap();
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![105]);
        // A reload keeps the same closure (and userdata).
        vm.reload_with("import io; native fn add_base(n)\npush_output(symbol(\"out\"), add_base(7))\n")
            .unwrap();
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![107]);
        assert_eq!(FREED.load(Ordering::SeqCst), 0);
        drop(vm);
        assert_eq!(FREED.load(Ordering::SeqCst), 1);
    }

    /// A crate layering its own entry points: it configures a `Vm`, hands
    /// out the `pb_vm*`, keeps driving the same VM from Rust, and publishes
    /// its own failures through `pb_vm_last_error`.
    #[test]
    fn a_rust_owned_vm_is_shared_with_c() {
        let mut vm = Vm::new();
        vm.load(
            "import io; state n = 0\nn += 1\npush_output(symbol(\"out\"), n)\n",
            None,
            "t".into(),
        )
        .unwrap();
        let ptr = VmHandle::into_raw(vm);
        assert_eq!(pb_vm_run(ptr), Status::Ok);
        {
            let vm = unsafe { VmHandle::vm_mut(ptr) }.unwrap();
            assert_eq!(out_ints(vm), vec![1]);
            vm.run().unwrap();
            assert_eq!(out_ints(vm), vec![2]);
            vm.restart().unwrap();
            vm.run().unwrap();
            assert_eq!(out_ints(vm), vec![1]);
            vm.set_last_error(Some(BridgeError::new(Status::NotFound, "no such screen")));
        }
        let err = pb_vm_last_error(ptr);
        assert!(!err.is_null());
        assert_eq!(unsafe { &*err }.code, Status::NotFound);
        unsafe { VmHandle::vm_mut(ptr) }
            .unwrap()
            .set_last_error(None);
        assert!(pb_vm_last_error(ptr).is_null());
        assert!(unsafe { VmHandle::vm_mut(std::ptr::null_mut()) }.is_none());
        pb_vm_destroy(ptr);
    }

    #[test]
    fn vec3_binds_and_decodes_natively() {
        let mut vm = Vm::new();
        vm.bind("v", HostValue::Vec3(1.0, 2.0, 3.0));
        vm.load(
            "import io; import vec2; let v = binding(symbol(\"v\"))\npush_output(symbol(\"out\"), v + vec3(0.5, 0, -1))\n",
            None,
            "t".into(),
        )
        .unwrap();
        vm.run().unwrap();
        let sym = vm.env.intern_symbol("out");
        let vals = vm.env.take_output_buffer(sym);
        let mut arena = crate::view::ViewArena::new();
        let first = arena.decode(&vals, vm.env.heap(), &crate::view::Names::NONE);
        arena.finish();
        let node = unsafe { &*arena.node_ptr(first) };
        assert_eq!(node.kind, crate::view::Kind::Vec3 as u32);
        assert_eq!((node.number, node.y, node.z), (1.5, 2.0, 2.0));
    }

    /// A handle decodes as slot in `integer`, serial in `count` and the class
    /// name in `str` (docs/usage/embedding-c.md, the value table).
    #[test]
    fn a_handle_decodes_slot_serial_and_class_name() {
        let mut vm = Vm::new();
        let class = vm.env.register_handle_class(petal::HandleClass {
            name: "Actor".to_string(),
            is_valid: Box::new(|_, _| true),
            describe: Box::new(|slot, serial| format!("Actor({slot}, {serial})")),
            call_method: Box::new(|_, m| Err(format!("no method {m}"))),
        });
        let h = vm.env.make_handle(class, 7, 3);
        let (p, n) = vm.keep_view(&[h]);
        assert_eq!(n, 1);
        let node = unsafe { &*p };
        assert_eq!(node.kind, crate::view::Kind::Handle as u32);
        assert_eq!((node.integer, node.count), (7, 3));
        let name = unsafe { std::slice::from_raw_parts(node.str_ as *const u8, node.str_len) };
        assert_eq!(name, b"Actor");
    }

    /// A run that fails is not rolled back: `state` keeps what it wrote before
    /// the error, and the next run carries on from there
    /// (docs/game-engine-features/frame-contract.md, "Errors").
    #[test]
    fn a_failed_run_keeps_the_state_it_wrote_and_the_next_run_goes_on() {
        let mut vm = Vm::new();
        vm.load(
            "state n = 0\nn += 1\nprint(n)\nif n == 2 then print(1 / 0) end\n",
            None,
            "t".into(),
        )
        .unwrap();
        vm.run().unwrap();
        assert!(vm.run().is_err(), "the second run divides by zero");
        vm.run().unwrap();
        assert_eq!(vm.env.take_output(), ["1", "2", "3"]);
    }

    #[test]
    fn reload_compile_errors_are_structured_without_recompiling() {
        let mut vm = Vm::new();
        vm.load(
            "import io; state n = 0\nn += 1\npush_output(symbol(\"out\"), n)\n",
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

    /// A `const` built with a native this VM did not register constant
    /// loads and runs as a `let`; the diagnostics text is where the host
    /// learns of it, on the load and again on a reload, and it is gone once
    /// the native is constant or the declaration is a `let`.
    #[test]
    fn a_const_that_is_not_constant_is_in_the_diagnostics_text() {
        use petal::native_fn::NativeEffects;
        fn ticks(cxt: &mut petal::native_fn::PetalCxt) -> Result<u32, String> {
            cxt.push_int(7);
            Ok(1)
        }
        fn text(vm: &mut Vm, errors_only: bool) -> String {
            let pid = vm.loaded().unwrap().program_id;
            if errors_only {
                vm.env.error_diagnostics_text(pid)
            } else {
                vm.env.diagnostics_text(pid)
            }
        }
        let src = "import io
native fn ticks() -> int
const T = ticks()
let unused = 1
push_output(symbol(\"out\"), T)
";
        let mut vm = Vm::new();
        vm.env.register_native("ticks", ticks, NativeEffects::PURE);
        vm.load(src, None, "main.ptl".into()).unwrap();
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![7]);
        let errors = text(&mut vm, true);
        assert!(
            errors.contains("error: `T` is declared `const` but is not constant"),
            "{errors}"
        );
        assert!(errors.contains("ticks, which is not declared constant"), "{errors}");
        assert!(!errors.contains("warning:"), "{errors}");
        assert!(text(&mut vm, false).contains(&errors));

        // Still there after a reload that keeps the `const`...
        vm.reload_with(&src.replace("let unused = 1", "let unused = 2")).unwrap();
        assert!(text(&mut vm, true).contains("`T` is declared `const`"));
        // ...and gone when the declaration stops claiming it.
        vm.reload_with(&src.replace("const T", "let T")).unwrap();
        assert_eq!(text(&mut vm, true), "");

        // The same script under a VM that registers the native constant has
        // nothing to say.
        let mut vm = Vm::new();
        vm.env.register_native("ticks", ticks, NativeEffects::CONST);
        vm.load(src, None, "main.ptl".into()).unwrap();
        vm.run().unwrap();
        assert_eq!(text(&mut vm, true), "");
    }

    /// `pb_vm_last_reload_json` says what the reload did to the constants,
    /// and which `state` kept a value its initializer no longer gives.
    #[test]
    fn the_reload_report_names_constants_and_stale_seeds() {
        let src = "import io
let TILES = [{w: 2}, {w: 3}]
let NAMES = [\"ab\", \"c\"]
state widths = map(TILES, fn(t) t.w end)
state n = 0
n += 1
push_output(symbol(\"out\"), n + len(NAMES[n % 2]) + TILES[n % 2].w + widths[0])
";
        let mut vm = Vm::new();
        vm.load(src, None, "main.ptl".into()).unwrap();
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![1 + 1 + 3 + 2]);

        // A table edited by shape: it is rebuilt, the other one is kept,
        // and the state seeded from it is stale.
        vm.reload_with(&src.replace("{w: 3}]", "{w: 3}, {w: 4}]")).unwrap();
        let json = reload_report_json(vm.last_reload().unwrap());
        assert_eq!(json["outcome"], "recompiled");
        let statics = &json["statics"];
        assert_eq!(statics["rebuilt_names"], serde_json::json!(["TILES"]), "{statics}");
        assert!(statics["kept"].as_u64().unwrap() >= 1, "{statics}");
        assert_eq!(statics["stale_seeds"][0]["name"], "widths");
        assert_eq!(statics["stale_seeds"][0]["read"], "TILES");
        assert_eq!(statics["cleared"], serde_json::Value::Null);
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![2 + 2 + 2 + 2]);

        // With the cache off there is nothing to report.
        vm.env.set_static_cache(false);
        vm.run().unwrap();
        vm.reload_with(src).unwrap();
        assert!(reload_report_json(vm.last_reload().unwrap()).get("statics").is_none());
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

    /// A failed host call must not leave its frames under the next one
    /// (Cheesecake's `agent_call`: every later trace listed the earlier
    /// failures again).
    #[test]
    fn a_failed_call_does_not_show_up_in_a_later_trace() {
        let mut vm = Vm::new();
        vm.load(
            "fn find(id)\n  assert(id > 0, \"no such id\")\n  id\nend\n\
             fn agent_call(id)\n  find(id) + 1\nend\n\
             fn other(x)\n  assert(false, \"other failed\")\nend\n",
            None,
            "t".into(),
        )
        .unwrap();
        vm.run().unwrap();
        let first = vm.call("agent_call", &[HostValue::Int(0)]).unwrap_err().message;
        assert_eq!(first.matches("in find()").count(), 1, "{first}");
        for _ in 0..3 {
            let again = vm.call("agent_call", &[HostValue::Int(0)]).unwrap_err().message;
            assert_eq!(again, first);
        }
        // A different failure names only its own calls.
        let other = vm.call("other", &[HostValue::Int(0)]).unwrap_err().message;
        assert!(!other.contains("find()") && !other.contains("agent_call"), "{other}");
        // The JSON entry point is the same call.
        let err = vm.call_json("agent_call", Some("[0]")).unwrap_err();
        assert_eq!((err.code, err.message), (Status::Runtime, first));
        assert!(vm.call_json("agent_call", Some("[2]")).is_ok());
        // And a frame after all that runs.
        vm.run().unwrap();
    }

    /// Nor do the frames of a run that failed: a hook called after a frame
    /// errored reports its own failure, not the frame's.
    #[test]
    fn a_failed_run_does_not_show_up_in_a_later_call_trace() {
        let mut vm = Vm::new();
        vm.load(
            "fn hook(x)\n  assert(x > 0, \"bad x\")\n  x\nend\n\
             fn frame(n)\n  assert(n < 2, \"frame failed\")\nend\n\
             state n = 0\nn += 1\nframe(n)\n",
            None,
            "t".into(),
        )
        .unwrap();
        vm.run().unwrap();
        let clean = vm.call("hook", &[HostValue::Int(0)]).unwrap_err().message;
        let failed = vm.run().unwrap_err().message;
        assert!(failed.contains("in frame()"), "{failed}");
        let after = vm.call("hook", &[HostValue::Int(0)]).unwrap_err().message;
        assert_eq!(after, clean);
        assert!(vm.call("hook", &[HostValue::Int(1)]).is_ok());
    }

    fn printed(vm: &mut Vm) -> String {
        vm.run().unwrap();
        vm.env.take_output().join("|")
    }

    /// Every kind of value a `state` can hold and a document can carry.
    const WORLD: &str = "import io; import vec2
enum Kind
  empty,
  road(lanes),
  tower(height, owner),
end
class Tile
  kind,
  at,
end
let grown = fn(t) Tile(Kind.tower(t, \"me\"), vec2(t, 0.5)) end
state tick = 0
tick += 1
state grid = {size: {w: 2, h: 1}, cells: [Tile(Kind.empty, vec2(0, 0)), Tile(Kind.road(2), vec2(1, 0))]}
state var log = [\"start\"]
state table = freeze({b: [1, 2.5, nil, true], a: {deep: [[1], [2, [3]]]}})
state where = vec3(1, 2, 3)
state odd = {far: 1e308 * 10.0, none: nil, tag: symbol(\"north\")}
state samples = f64_array(2)
fn bump(key)
  state count = 0
  count += tick
  count
end
let counts = map([\"a\", \"b\"], fn(k) bump(k) end)
if tick % 2 == 0 then
  grid = {size: grid.size, cells: grid.cells ++ [grown(tick)]}
  set log = log ++ [\"t{tick}\"]
end
print(tick, grid, log, table, frozen(table), where, odd.far, odd.none, odd.tag == symbol(\"north\"), samples, counts)
";

    #[test]
    fn state_saved_from_one_vm_loads_into_another() {
        let mut a = Vm::new();
        a.load(WORLD, None, "world.ptl".into()).unwrap();
        for _ in 0..4 {
            printed(&mut a);
        }
        let saved = a.save_state().unwrap();
        let report: serde_json::Value = serde_json::from_str(a.state_report()).unwrap();
        assert_eq!(report["op"], "save");
        assert_eq!(report["skipped"], serde_json::json!([]), "everything in WORLD can be saved");
        assert_eq!(report["slots"], 8, "seven top-level slots and the one inside `bump`");
        // Saving is a read: twice gives the same bytes.
        assert_eq!(a.save_state().unwrap(), saved);
        let (a5, a6) = (printed(&mut a), printed(&mut a));

        // A second VM running the same program, loaded before its first run,
        // carries on exactly where the first was.
        let mut b = Vm::new();
        b.load(WORLD, None, "world.ptl".into()).unwrap();
        b.load_state(&saved).unwrap();
        let report: serde_json::Value = serde_json::from_str(b.state_report()).unwrap();
        assert_eq!((&report["op"], &report["dropped"]), (&serde_json::json!("load"), &serde_json::json!([])));
        // The one thing a document does not carry: `frozen(table)`.
        let unfrozen = |s: &str| s.replacen("} true vec3", "} false vec3", 1);
        assert!(a5.contains("} true vec3"), "{a5}");
        assert_eq!(printed(&mut b), unfrozen(&a5));
        assert_eq!(printed(&mut b), unfrozen(&a6));
        assert_eq!(b.save_state().unwrap(), a.save_state().unwrap());

        // Undo in place: the first VM goes back to the save point and replays
        // the same two frames, any number of times, with memoization on or off.
        for memo in [true, false, true] {
            let policy = a.env.policy().with_memo(memo);
            a.env.set_policy(policy);
            a.load_state(&saved).unwrap();
            assert_eq!(printed(&mut a), unfrozen(&a5));
            assert_eq!(printed(&mut a), unfrozen(&a6));
        }
    }

    #[test]
    fn state_survives_a_code_edit_by_the_reload_rules() {
        let v1 = "state kept = 0\nkept += 1\nstate removed = 100\nremoved += 1\n\
                  state on_done = fn() 1 end\nprint(kept, removed)\n";
        let v2 = "state kept = 0\nkept += 1\nstate added = 50\nadded += 1\n\
                  state on_done = fn() 1 end\nprint(kept, added, on_done())\n";
        let mut vm = Vm::new();
        vm.load(v1, None, "main.ptl".into()).unwrap();
        printed(&mut vm);
        assert_eq!(printed(&mut vm), "2 102");
        let saved = vm.save_state().unwrap();
        // What cannot be stored is named, not written as text.
        let report: serde_json::Value = serde_json::from_str(vm.state_report()).unwrap();
        assert_eq!(report["slots"], 2);
        assert_eq!(report["skipped"], serde_json::json!([{"name": "on_done", "holds": "a function"}]));
        assert!(!saved.contains("on_done"), "{saved}");

        // A new process, with the edited program.
        let mut vm = Vm::new();
        vm.load(v2, None, "main.ptl".into()).unwrap();
        vm.load_state(&saved).unwrap();
        let report: serde_json::Value = serde_json::from_str(vm.state_report()).unwrap();
        assert_eq!(report, serde_json::json!({"op": "load", "slots": 1, "dropped": ["removed"]}));
        // `kept` carries on; `added` and the function start from their initializers.
        assert_eq!(printed(&mut vm), "3 51 1");
        // The next save no longer has the removed slot.
        assert!(!vm.save_state().unwrap().contains("removed"));

        // The same VM, reloaded in place and then put back to the save point.
        let mut vm = Vm::new();
        vm.load(v1, None, "main.ptl".into()).unwrap();
        printed(&mut vm);
        printed(&mut vm);
        printed(&mut vm);
        vm.reload_with(v2).unwrap();
        assert_eq!(printed(&mut vm), "4 51 1");
        vm.load_state(&saved).unwrap();
        assert_eq!(printed(&mut vm), "3 51 1");
    }

    #[test]
    fn an_unusable_state_document_changes_nothing() {
        let mut vm = Vm::new();
        assert_eq!(vm.save_state().unwrap_err().code, Status::NotLoaded);
        assert_eq!(vm.load_state("{}").unwrap_err().code, Status::NotLoaded);
        vm.load("state n = 0\nn += 1\nprint(n)\n", None, "t".into()).unwrap();
        assert_eq!(printed(&mut vm), "1");
        for (doc, why) in [
            ("{", "not valid JSON"),
            ("[]", "not a Petal state storage file"),
            ("{\"format\": \"petal-state\", \"version\": 99, \"slots\": []}", "version 99"),
            (
                "{\"format\": \"petal-state\", \"version\": 1, \"slots\": [{\"name\": \"other\", \"value\": 1}]}",
                "written by a different script",
            ),
            (
                "{\"format\": \"petal-state\", \"version\": 1, \"slots\": [{\"name\": \"n\", \"value\": {\"$petal\": \"what\"}}]}",
                "unknown value tag",
            ),
        ] {
            let err = vm.load_state(doc).unwrap_err();
            assert_eq!(err.code, Status::InvalidArg, "{doc}");
            assert!(err.message.contains(why), "{}", err.message);
        }
        assert_eq!(printed(&mut vm), "2");
        // A hand-written document, by name; and an empty one clears the state.
        vm.load_state("{\"format\": \"petal-state\", \"version\": 1, \"slots\": [{\"name\": \"n\", \"value\": 40}]}")
            .unwrap();
        assert_eq!(printed(&mut vm), "41");
        vm.load_state("{\"format\": \"petal-state\", \"version\": 1, \"slots\": []}").unwrap();
        assert_eq!(printed(&mut vm), "1");

        let dir = std::env::temp_dir().join(format!("petal-c-bridge-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("save.json");
        printed(&mut vm);
        vm.save_state_file(&file).unwrap();
        printed(&mut vm);
        vm.load_state_file(&file).unwrap();
        assert_eq!(printed(&mut vm), "3");
        assert_eq!(vm.load_state_file(&dir.join("nope.json")).unwrap_err().code, Status::Io);
        assert_eq!(vm.save_state_file(&dir.join("no/such/dir.json")).unwrap_err().code, Status::Io);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_json_call_binds_by_name_and_follows_the_declared_types() {
        let mut vm = Vm::new();
        vm.load(
            "enum Kind\n  road(lanes),\n  empty,\nend\n\
             fn toll(id: string, cents: int = 25, rate: float = 0.5, note = \"-\")\n\
               \"{id} {cents} {type(cents)} {rate} {type(rate)} {note}\"\nend\n\
             fn lanes(k)\n  match k\n    when Kind.road(n) -> n\n    when _ -> 0\n  end\nend\n\
             fn over(a)\n  \"one\"\nend\nfn over(a, b)\n  \"two\"\nend\n\
             fn none()\n  7\nend\n",
            None,
            "t".into(),
        )
        .unwrap();
        vm.run().unwrap();
        fn text(vm: &mut Vm, function: &str, args: Option<&str>) -> String {
            let p = vm.call_json(function, args).unwrap();
            let node = unsafe { &*p };
            if node.kind == crate::view::Kind::Int as u32 {
                return node.integer.to_string();
            }
            unsafe { std::ffi::CStr::from_ptr(node.str_) }.to_str().unwrap().to_string()
        }
        // By name: defaults fill what is left out, in any order.
        assert_eq!(text(&mut vm, "toll", Some(r#"{"id": "a"}"#)), "a 25 int 0.5 float -");
        assert_eq!(
            text(&mut vm, "toll", Some(r#"{"note": "n", "rate": 2, "id": "a"}"#)),
            "a 25 int 2.0 float n"
        );
        // By position; a whole float for an int, a whole number for a float.
        assert_eq!(text(&mut vm, "toll", Some(r#"["b", 40.0, 3]"#)), "b 40 int 3.0 float -");
        assert_eq!(text(&mut vm, "none", None), "7");
        assert_eq!(text(&mut vm, "none", Some("{}")), "7");
        // Values are spelled as in a state document.
        assert_eq!(
            text(&mut vm, "lanes", Some(r#"[{"$petal": "enum", "tag": "Kind.road", "data": [3]}]"#)),
            "3"
        );
        // An overload set is resolved by count; nothing is coerced.
        assert_eq!(text(&mut vm, "over", Some("[1, 2]")), "two");

        let err = |vm: &mut Vm, function: &str, args: &str| vm.call_json(function, Some(args)).unwrap_err();
        let e = err(&mut vm, "toll", r#"{"id": "a", "cents": "abc"}"#);
        assert_eq!(e.code, Status::InvalidArg);
        assert_eq!(e.message, "toll(): parameter 'cents' is declared int, got a string (\"abc\")");
        assert_eq!(err(&mut vm, "toll", r#"["a", 2.5]"#).code, Status::InvalidArg);
        assert_eq!(err(&mut vm, "toll", "3").code, Status::InvalidArg);
        assert_eq!(err(&mut vm, "nope", "[]").code, Status::NotFound);
        let e = err(&mut vm, "toll", r#"{"cents": 1}"#);
        assert_eq!(e.code, Status::Runtime);
        assert!(e.message.contains("missing a value for parameter 'id'"), "{}", e.message);
        let e = err(&mut vm, "toll", r#"{"id": "a", "bogus": 1}"#);
        assert!(e.message.contains("no parameter named 'bogus'"), "{}", e.message);
        let e = err(&mut vm, "lanes", r#"[{"$petal": "what"}]"#);
        assert_eq!(e.code, Status::InvalidArg);
        assert!(e.message.contains("lanes(): argument 1"), "{}", e.message);

        let info = vm.function_json("toll").unwrap();
        assert_eq!(info["name"], "toll");
        assert_eq!(
            info["params"][1],
            serde_json::json!({"name": "cents", "type": "int", "has_default": true, "default_source": "25"})
        );
        assert_eq!(info["params"][3]["type"], serde_json::Value::Null);
        assert_eq!(vm.function_json("over").unwrap()["params"], serde_json::Value::Null);
        assert!(vm.function_json("nope").is_none());
    }

    #[test]
    fn diagnostics_are_available_as_json() {
        let mut vm = Vm::new();
        assert!(vm.diagnostics_json().is_none());
        vm.load("let n: int = \"one\"\nprint(n)\n", None, "main.ptl".into()).unwrap();
        let json = vm.diagnostics_json().unwrap();
        assert_eq!((json["errors"].as_u64(), json["warnings"].as_u64()), (Some(0), Some(1)), "{json}");
        let item = &json["items"][0];
        assert_eq!(item["severity"], "warning");
        assert_eq!(item["line"], 1);
        assert!(item["message"].as_str().unwrap().contains("int"), "{item}");
        // A clean reload clears them.
        vm.reload_with("let n: int = 1\nprint(n)\n").unwrap();
        assert_eq!(vm.diagnostics_json().unwrap()["items"], serde_json::json!([]));
    }

    /// After a reload the host prints what the edit introduced, not every
    /// standing warning again.
    #[test]
    fn a_reload_reports_only_the_findings_the_edit_introduced() {
        let base = "fn double(x: int) x end\nlet a = double(\"one\")\nlet b = 4\nprint(a, b)\n";
        let mut vm = Vm::new();
        vm.load(base, None, "main.ptl".into()).unwrap();
        assert_eq!(vm.diagnostics_json().unwrap()["warnings"], 1);
        assert_eq!(vm.new_diagnostics_text(false), "", "before any reload");

        // The edit adds a second mistake, two lines below a new comment.
        let edited = base.replace("let b = 4", "// moved\nlet b: string = 4");
        vm.reload_with(&edited).unwrap();
        assert_eq!(vm.diagnostics_json().unwrap()["warnings"], 2);
        let new = vm.new_diagnostics_text(false);
        assert_eq!(new.matches("warning:").count(), 1, "{new}");
        assert!(new.contains("line 4"), "{new}");
        assert_eq!(vm.new_diagnostics_text(true), "", "a warning is not an error");

        // An edit that changes a value is not checked again; one that fixes
        // the mistake introduces nothing.
        vm.reload_with(&edited.replace("print(a, b)", "print(b, a)")).unwrap();
        assert_eq!(vm.new_diagnostics_text(false), "");
    }

    #[test]
    fn sources_changed_tracks_the_entry_file() {
        let dir = std::env::temp_dir().join(format!("petal-c-bridge-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("main.ptl");
        std::fs::write(&main, "import io; push_output(symbol(\"out\"), 1)\n").unwrap();
        let mut vm = Vm::new();
        let src = std::fs::read_to_string(&main).unwrap();
        vm.load(&src, Some(&main), main.display().to_string())
            .unwrap();
        assert_eq!(vm.source_paths(), vec![main.clone()]);
        assert!(!vm.sources_changed());
        assert!(vm.changed_sources().is_empty());
        // A length change is detected even within one mtime tick.
        std::fs::write(&main, "import io; push_output(symbol(\"out\"), 22)\n").unwrap();
        assert!(vm.sources_changed());
        assert_eq!(vm.changed_sources(), vec![main.clone()]);
        vm.reload().unwrap();
        assert!(!vm.sources_changed());
        assert!(vm.changed_sources().is_empty());
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![22]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A reload does only what the edit calls for, a value can be set
    /// without the file changing, and the running text is what to write when
    /// the drag ends.
    #[test]
    fn reload_takes_the_cheap_path_and_set_config_needs_no_file_write() {
        let dir =
            std::env::temp_dir().join(format!("petal-c-bridge-config-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("main.ptl");
        let config = dir.join("config.ptl");
        let entry = "import io; import ./config
state n = 0
n += config.LOOK.step
push_output(symbol(\"out\"), n)
";
        let knobs = "// knobs
pub config let LOOK = {step: 2, fog: 0.5}
";
        std::fs::write(&main, entry).unwrap();
        std::fs::write(&config, knobs).unwrap();
        let mut vm = Vm::new();
        vm.load_file(&main).unwrap();
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![2]);
        assert!(vm.last_reload().is_none());
        let work = vm.env.work_counters();

        // A value edit in the module file: patched, nothing compiled.
        std::fs::write(&config, "// knobs
pub config let LOOK = {step: 10, fog: 0.5}
").unwrap();
        assert!(vm.sources_changed());
        vm.reload().unwrap();
        assert_eq!(vm.last_reload().unwrap().outcome, ReloadOutcome::Patched);
        assert!(!vm.sources_changed());
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![12]);

        // A comment: relocated.
        std::fs::write(&main, format!("// the game
{entry}")).unwrap();
        vm.reload().unwrap();
        assert_eq!(vm.last_reload().unwrap().outcome, ReloadOutcome::Relocated);
        assert_eq!(vm.env.work_counters(), work);

        // A drag: the value changes, the file does not.
        vm.set_config(None, "LOOK.step", &StaticValue::int(100)).unwrap();
        assert_eq!(vm.last_reload().unwrap().outcome, ReloadOutcome::Patched);
        assert!(!vm.sources_changed());
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![112]);
        assert_eq!(vm.env.work_counters(), work);
        let json = reload_report_json(vm.last_reload().unwrap());
        assert_eq!(json["outcome"], "patched");
        assert_eq!(json["values"][0]["path"], "LOOK.step");
        assert_eq!(json["values"][0]["new"], 100);
        // A different type needs the file and a reload; nothing changes.
        let err = vm
            .set_config(Some(&config), "LOOK.step", &StaticValue::float(1.5))
            .unwrap_err();
        assert_eq!(err.code, Status::NeedsReload);
        assert_eq!(
            vm.set_config(None, "NOPE", &StaticValue::int(1)).unwrap_err().code,
            Status::NotFound
        );

        // The drag ends: the host writes the running text, and the reload
        // that follows has nothing to do.
        let text = vm.source_text(Some(&config)).unwrap().to_string();
        assert_eq!(text, "// knobs\npub config let LOOK = {step: 100, fog: 0.5}\n");
        std::fs::write(&config, &text).unwrap();
        assert!(vm.sources_changed());
        vm.reload().unwrap();
        assert_eq!(vm.last_reload().unwrap().outcome, ReloadOutcome::Unchanged);
        assert_eq!(vm.env.work_counters(), work);
        assert_eq!(vm.source_text(None).unwrap(), format!("// the game\n{entry}"));
        assert_eq!(vm.source_text(Some(&main)).unwrap(), format!("// the game\n{entry}"));

        // A structural edit recompiles.
        std::fs::write(&main, format!("{entry}push_output(symbol(\"out\"), 7)\n")).unwrap();
        vm.reload().unwrap();
        assert_eq!(vm.last_reload().unwrap().outcome, ReloadOutcome::Recompiled);
        vm.run().unwrap();
        assert_eq!(out_ints(&mut vm), vec![212, 7]);
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
            "import io; import ui; push_output(symbol(\"out\"), if key_pressed(\"space\") then 1 else 0 end)\n\
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

    /// One frame of `vm`, as the `pb_draw_cmd`s `pb_vm_drain_draw` hands out.
    fn drawn(vm: &mut Vm) -> DrawList {
        vm.input.begin_frame(1.0 / 60.0);
        petal_ui::input::bind_input(&mut vm.env, &vm.input);
        vm.run().unwrap();
        let commands = petal_ui::draw::take_draw_commands(&mut vm.env);
        let names = Names { symbol: None, handle_class: None };
        let mut list = DrawList::default();
        for c in &commands {
            list.push(c, vm.env.heap(), &names);
        }
        list
    }

    const RETAINED: &str = "import ui\n\
        cached_layer(1, rect(10, 20, 30, 40), fn()\n\
          draw_rects([0, 0, 4, 4, 0, 4, 0, 4, 4, 1], [#ff0000, {r: 0, g: 255, b: 0, a: 128}])\n\
        end)\n\
        button(rect(100, 100, 80, 20), \"Go\")\n";

    #[test]
    fn batches_and_retained_layers_are_lowered_by_default() {
        use crate::draw::DrawKind as K;
        let mut vm = Vm::new();
        vm.load(RETAINED, None, "t".into()).unwrap();
        let list = drawn(&mut vm);
        let kinds: Vec<u32> = list.cmds.iter().take(4).map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [K::ClipPush as u32, K::Rect as u32, K::Rect as u32, K::ClipPop as u32]
        );
        // The content, moved to where the layer is drawn.
        assert_eq!((list.cmds[1].x, list.cmds[1].y), (10, 20));
        assert_eq!((list.cmds[2].x, list.cmds[2].color.a), (14, 128));
    }

    #[test]
    fn a_host_that_asks_gets_batches_and_retained_layers_as_commands() {
        use crate::draw::DrawKind as K;
        let mut vm = Vm::new();
        vm.load(RETAINED, None, "t".into()).unwrap();
        petal_ui::compose::set_host_caps(
            &mut vm.env,
            petal_ui::compose::HostCaps::from_bits(3),
        );
        let list = drawn(&mut vm);
        let kinds: Vec<u32> = list.cmds.iter().take(4).map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [K::CacheBegin as u32, K::Rects as u32, K::CacheEnd as u32, K::CacheDraw as u32]
        );
        let (begin, batch, draw) = (&list.cmds[0], &list.cmds[1], &list.cmds[3]);
        assert_eq!((begin.w, begin.h), (30, 40));
        assert_eq!((draw.id, draw.x, draw.y), (begin.id, 10, 20));
        assert_eq!(batch.point_count, 2);
        // SAFETY: `points` holds `point_count` fives, owned by `list`.
        let ints = unsafe { std::slice::from_raw_parts(batch.points, 10) };
        assert_eq!(&ints[..4], [0, 0, 4, 4]);
        assert_eq!(ints[4] as u32, 0xff0000ff, "r << 24 | g << 16 | b << 8 | a");
        assert_eq!(ints[9] as u32, 0x00ff0080);

        // The next frame only draws the layer the host kept.
        let list = drawn(&mut vm);
        assert_eq!(list.cmds[0].kind, K::CacheDraw as u32);
    }

    #[test]
    fn the_target_registry_is_readable_from_the_host() {
        let mut vm = Vm::new();
        vm.load(RETAINED, None, "t".into()).unwrap();
        drawn(&mut vm);
        let go = petal_ui::hit::target(&mut vm.env, "Go").expect("the button registered");
        assert_eq!(go.center(), (140, 110));
        let mut list = crate::draw::TargetList::default();
        list.push(&go);
        // SAFETY: the id is a NUL-terminated string owned by `list`.
        let id = unsafe { std::ffi::CStr::from_ptr(list.targets[0].id) };
        assert_eq!(id.to_str().unwrap(), "Go");
        assert_eq!(list.targets[0].blocks, 1);
    }

    #[test]
    fn an_image_carries_its_tint_source_rect_and_slice() {
        let mut vm = Vm::new();
        vm.load(
            "import ui\n\
             draw_image(\"a.png\", rect(1, 2, 30, 40), {tint: #ff8000, src: rect(16, 0, 8, 8), slice: {l: 1, t: 2, r: 3, b: 4}})\n\
             draw_image(\"a.png\", rect(1, 2, 30, 40))\n",
            None,
            "t".into(),
        )
        .unwrap();
        let list = drawn(&mut vm);
        let c = &list.cmds[0];
        assert_eq!((c.color.r, c.color.g, c.color.b, c.color.a), (255, 128, 0, 255));
        assert_eq!((c.x1, c.y1, c.x2, c.y2), (16, 0, 8, 8));
        assert_eq!((c.x3, c.y3, c.cx, c.cy), (1, 2, 3, 4));
        let plain = &list.cmds[1];
        assert_eq!((plain.color.r, plain.color.g, plain.color.b), (255, 255, 255));
        assert_eq!((plain.x2, plain.x3, plain.cx), (0, 0, 0));
    }

    // ── Tests ────────────────────────────────────────────────────────────

    static WAIT: AtomicUsize = AtomicUsize::new(0);

    /// `wait_frames(n)`: note `n`, suspend the test.
    unsafe extern "C" fn wait_frames(call: *mut natives::PbCall, _ud: *mut c_void) -> i32 {
        let n = unsafe { &*natives::pb_call_arg(call, 0) }.integer;
        if natives::pb_call_suspend(call) != 0 {
            return 1;
        }
        WAIT.store(n as usize, Ordering::SeqCst);
        0
    }

    /// `expect_small(n)`: a host expectation, with where it was called.
    unsafe extern "C" fn expect_small(call: *mut natives::PbCall, _ud: *mut c_void) -> i32 {
        let n = unsafe { &*natives::pb_call_arg(call, 0) }.integer;
        let (mut file, mut line, mut col) = (std::ptr::null(), 0u32, 0u32);
        let known = unsafe { natives::pb_call_location(call, &mut file, &mut line, &mut col) };
        assert!(known && line > 0 && col > 0);
        let msg = cstring_lossy(&format!("{n} is not small"));
        unsafe { natives::pb_call_expect(call, n < 10, c"expect_small".as_ptr(), msg.as_ptr()) };
        0
    }

    const TESTED: &str = "import io\n\
        native fn wait_frames(n)\n\
        native fn expect_small(n)\n\
        state var ticks = 0\n\
        set ticks = ticks + 1\n\
        fn double(x) x * 2 end\n\
        test \"doubling\" do\n\
          expect_eq(double(2), 4)\n\
          expect_eq(double(3), 7, \"three\")\n\
          expect_small(3)\n\
          expect_small(30)\n\
        end\n\
        test \"three frames pass\" do\n\
          let before = get ticks\n\
          wait_frames(3)\n\
          expect_eq(get ticks, before + 3)\n\
        end\n\
        test \"raises\" do\n\
          expect(true)\n\
          let xs = [1]\n\
          print(xs[5])\n\
        end\n";

    fn tested_vm() -> Vm {
        let mut vm = Vm::new();
        for (name, f) in [
            ("wait_frames", wait_frames as PbNativeFn),
            ("expect_small", expect_small as PbNativeFn),
        ] {
            let cb = HostCallback::new(name, natives::FX_EFFECT, f, std::ptr::null_mut(), None);
            natives::register_callback(&mut vm.env, cb);
        }
        vm.load(TESTED, None, "game.ptl".into()).unwrap();
        vm.run().unwrap();
        vm
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_tests_of_a_program_are_listed_and_run() {
        let mut vm = tested_vm();
        let tests = vm.tests_json().unwrap();
        let names: Vec<&str> =
            tests.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["doubling", "three frames pass", "raises"]);
        assert_eq!(tests[0]["function"], "__test_0");
        assert_eq!(tests[0]["module"], serde_json::Value::Null);
        assert_eq!(tests[0]["line"], 7);

        let r = json(vm.run_test("__test_0").unwrap());
        assert_eq!(r["name"], "doubling");
        assert_eq!(r["passed"], false);
        assert_eq!((r["checks"].as_u64(), r["failed"].as_u64()), (Some(4), Some(2)));
        assert_eq!(r["error"], serde_json::Value::Null);
        let f = &r["failures"];
        assert_eq!(f[0]["check"], "expect_eq");
        assert_eq!((f[0]["line"].as_u64(), f[0]["expected"].as_str()), (Some(9), Some("7")));
        assert_eq!(f[0]["note"], "three");
        // The host's own expectation, at the script call that made it.
        assert_eq!(f[1]["check"], "expect_small");
        assert_eq!(f[1]["message"], "30 is not small");
        assert_eq!(f[1]["line"], 11);
        assert_eq!(vm.test_result(), r.to_string());

        // A test that raises: the error, and what it checked before it.
        let r = json(vm.run_test("__test_2").unwrap());
        assert_eq!(r["passed"], false);
        assert_eq!(r["checks"], 1);
        assert!(r["error"].as_str().unwrap().contains("out of"), "{r}");
        // The frame loop is unharmed.
        vm.run().unwrap();

        // No such test, and nothing in progress to resume or abort.
        assert_eq!(vm.test_begin("nope").unwrap_err().code, Status::NotFound);
        assert_eq!(vm.test_resume(0).unwrap_err().code, Status::InvalidArg);
        assert_eq!(vm.test_abort("x").unwrap_err().code, Status::InvalidArg);
    }

    #[test]
    fn a_test_waits_across_the_hosts_frames() {
        let mut vm = tested_vm();
        vm.test_begin("__test_1").unwrap();
        assert_eq!(vm.test_in_progress(), Some("__test_1"));
        WAIT.store(0, Ordering::SeqCst);
        let mut frames = 0;
        let result = loop {
            match vm.test_resume(0).unwrap() {
                petal::env::TestStep::Finished(r) => break r,
                petal::env::TestStep::Suspended => {
                    for _ in 0..WAIT.swap(0, Ordering::SeqCst) {
                        vm.run().unwrap();
                        frames += 1;
                    }
                }
                petal::env::TestStep::OutOfSteps => unreachable!(),
            }
        };
        assert_eq!(frames, 3);
        assert!(result.passed, "{result:?}");
        assert_eq!(result.checks, 1);
        assert_eq!(vm.test_in_progress(), None);

        // Outside a test the native cannot wait: the call fails with the reason.
        vm.load(
            "native fn wait_frames(n)\nwait_frames(1)\n",
            None,
            "t".into(),
        )
        .unwrap();
        let err = vm.run().unwrap_err();
        assert!(err.message.contains("inside a test"), "{}", err.message);
    }

    #[test]
    fn a_test_can_be_bounded_and_aborted() {
        let mut vm = Vm::new();
        vm.load(
            "test \"forever\" do\n  var i = 0\n  expect(true)\n  while true do set i = i + 1 end\nend\n",
            None,
            "t".into(),
        )
        .unwrap();
        vm.run().unwrap();
        vm.test_begin("__test_0").unwrap();
        assert_eq!(vm.test_resume(10_000).unwrap(), petal::env::TestStep::OutOfSteps);
        assert_eq!(vm.test_resume(10_000).unwrap(), petal::env::TestStep::OutOfSteps);
        vm.test_abort("took too long").unwrap();
        let r = json(vm.test_result());
        assert_eq!(r["error"], "took too long");
        assert_eq!((r["passed"].as_bool(), r["checks"].as_u64()), (Some(false), Some(1)));
        vm.run().unwrap();
    }

    #[test]
    fn expectations_outside_tests_are_taken_by_the_host() {
        let mut vm = Vm::new();
        vm.load("expect_eq(1 + 1, 3)\nexpect(true)\n", None, "t".into()).unwrap();
        vm.run().unwrap();
        vm.run().unwrap();
        let taken = vm.take_expect_failures().unwrap();
        assert_eq!((taken["checked"].as_u64(), taken["failed"].as_u64()), (Some(4), Some(2)));
        assert_eq!(taken["failures"].as_array().unwrap().len(), 2);
        assert_eq!(taken["failures"][0]["line"], 1);
        let again = vm.take_expect_failures().unwrap();
        assert_eq!(again["failures"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn the_slow_native_threshold_is_the_hosts_to_set() {
        let vm = Vm::into_handle_for_test();
        assert_eq!(pb_vm_set_profile_slow_native_us(vm, 250.0), Status::Ok);
        assert_eq!(pb_vm_set_profile_slow_native_us(vm, -1.0), Status::InvalidArg);
        let slow = unsafe { VmHandle::vm_mut(vm) }.unwrap().env.profile_mut().slow_native;
        assert_eq!(slow, std::time::Duration::from_micros(250));
        pb_vm_destroy(vm);
    }
}
