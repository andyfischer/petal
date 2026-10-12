//! Env - The foundational data structure for the Petal runtime.
//!
//! Owns all programs and stacks. Most operations require an Env as context.
//! See docs/Architecture.md for the surrounding runtime design.

use std::collections::HashMap;

use crate::backend::OptFlags;
use crate::backend::bytecode::BytecodeProgram;
use crate::compiler::Compiler;
use crate::execution_context::{ContextKey, ExecutionContext};
use crate::handle::{HandleClass, HandleClassId, HandleVal};
use crate::heap::Heap;
use crate::module::ModuleRegistry;
use crate::native_fn::{NativeClass, NativeEffects, NativeFn, NativeFnId, NativeFnTable};
use crate::observe::Observations;
use crate::policy::RunPolicy;
use crate::effect_audit::EffectAudit;
use crate::profile::VmProfile;
use crate::program::{Program, ProgramId, StateKey};
use crate::stack::{RuntimeStateKey, Stack, StackKey};
use crate::stats::{AllocStats, DupStats};
use crate::symbol::SymbolTable;
use crate::trace::TraceBuffer;
use crate::value::Value;

mod fork;
mod gc;
mod host_io;
mod observations_json;
mod reload;
mod run;
mod state_json;
mod state_storage;
mod statics;

pub use reload::{ConfigSetError, FileChange, ProgramChange, ReloadOutcome, ReloadReport};
pub use state_storage::{StateStorageLoad, StateStorageSave, name_list as state_name_list};
pub use statics::{StaticSlotState, StaticValues};

pub struct Env {
    programs: HashMap<ProgramId, Program>,
    stacks: HashMap<StackKey, Stack>,
    native_fns: NativeFnTable,
    /// Interned symbol names ↔ ordinals, shared with the embedding host.
    symbols: SymbolTable,
    /// Execution-local state bundles (heap + runtime registries), one per
    /// isolated execution. Each `Stack` links to its context by key. See
    /// `execution_context::ExecutionContext`.
    contexts: HashMap<ContextKey, ExecutionContext>,
    /// The context used by no-stack accessor methods and by newly created
    /// stacks.
    default_context: ContextKey,
    next_context_id: u32,
    trace: TraceBuffer,
    /// Last value bound to each named IR term. Off by default; see
    /// [`crate::observe`].
    observations: Observations,
    /// Opt-in execution counters (opcodes, builtins, calls, GC). Off by
    /// default; see [`crate::profile`].
    profile: VmProfile,
    /// Opt-in audit of native effect declarations against observed behavior.
    /// Off by default; see [`crate::effect_audit`].
    effect_audit: EffectAudit,
    /// Opt-in audit of the static-value classifier against what static code
    /// was observed reading. Off by default; see [`crate::static_audit`].
    static_audit: crate::static_audit::StaticAudit,
    next_program_id: u32,
    next_stack_id: u32,
    /// How runs execute: optimizer passes, memoization, frame gating.
    policy: RunPolicy,
    /// Lazily-lowered bytecode, cached next to each `Program`. Populated on the
    /// first bytecode run of a program; an entry's presence means lowering
    /// succeeded. The stored `OptFlags` is the flag set the lowering was built
    /// with — when the active flags change (e.g. `--policy baseline`), the cache is
    /// re-lowered so in-place opcodes match the current optimization gate.
    bytecode: HashMap<ProgramId, (OptFlags, BytecodeProgram)>,
    /// Module resolution: in-memory registrations, search paths, and implicit
    /// imports. See docs/module-system.md.
    modules: ModuleRegistry,
    /// How many times this env compiled a program and lowered one to
    /// bytecode (see [`Env::work_counters`]). `Cell`s because compiling takes
    /// `&self`.
    compile_count: std::cell::Cell<u64>,
    lower_count: std::cell::Cell<u64>,
    /// Host-registered foreign-object classes, indexed by `HandleClassId`.
    handle_classes: Vec<HandleClass>,
    /// Manifests that looked like packages and would not load, collected by
    /// the best-effort discovery [`Env::add_module_path`] does. Read with
    /// [`Env::package_errors`] — the CLI reports them, an embedder may not
    /// care that some directory on a search path has a broken `petal.toml`.
    package_errors: Vec<crate::error::LoadError>,
    /// The same, for the ambient `PETAL_PATH` directories scanned by
    /// [`Env::new`]. Kept apart from `package_errors` because the CLI treats
    /// the two differently: a `-I` the user typed that will not load is fatal,
    /// while a broken manifest somewhere on the machine's `PETAL_PATH` must
    /// not fail every command — `petal packages` reports it instead of
    /// swallowing it.
    ambient_package_errors: Vec<crate::error::LoadError>,
    /// Compile without linking; see [`Env::set_check_only`].
    check_only: bool,
    /// The host interfaces this `Env` provides, by name; see
    /// [`Env::provides`].
    provided: std::collections::BTreeMap<String, crate::package::Version>,
}

/// The interface every `Env` provides: the builtins, at the version of this
/// crate. A package that needs a newer core says `[requires] core = ">=x.y"`.
pub const CORE_INTERFACE: &str = crate::bundled::CORE_PACKAGE;

/// How a host's registered natives and its interface packages disagree; see
/// [`Env::verify_interface`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct InterfaceMismatch {
    /// Host natives a package declares (`native fn`) that the `Env` does not
    /// register, sorted. A script that uses one fails to load.
    pub unregistered: Vec<String>,
    /// Natives the `Env` registers, beyond the builtins, that no package
    /// declares, sorted. No script can reach one.
    pub undeclared: Vec<String>,
    /// Packages that could not be found, and modules that do not parse.
    pub problems: Vec<String>,
}

impl InterfaceMismatch {
    /// Nothing is wrong.
    pub fn is_empty(&self) -> bool {
        self.unregistered.is_empty() && self.undeclared.is_empty() && self.problems.is_empty()
    }
}

impl std::fmt::Display for InterfaceMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if !self.unregistered.is_empty() {
            parts.push(format!(
                "declared by the interface but not registered by the host: {}",
                self.unregistered.join(", ")
            ));
        }
        if !self.undeclared.is_empty() {
            parts.push(format!(
                "registered by the host but declared by no interface package (add a \
                 `native fn` for each): {}",
                self.undeclared.join(", ")
            ));
        }
        parts.extend(self.problems.iter().cloned());
        write!(f, "{}", parts.join("; "))
    }
}

impl std::error::Error for InterfaceMismatch {}

/// Name of the core prelude module: `import std`.
pub const STD_MODULE: &str = "std";

/// Petal source of the core prelude, standard-library helpers written in Petal
/// over the native builtins. Compiled into the binary (works with no
/// filesystem, e.g. wasm) and registered by [`Env::new`] as a permanent
/// implicit import. See `core/prelude/std.ptl`.
pub const STD_PRELUDE: &str = include_str!("../../prelude/std.ptl");

impl Env {
    /// Create a new environment
    pub fn new() -> Self {
        let mut native_fns = NativeFnTable::new();
        crate::builtins::register_builtins(&mut native_fns);
                let default_context = ContextKey(1);
        let mut contexts = HashMap::new();
        contexts.insert(default_context, ExecutionContext::new());
        let mut modules = ModuleRegistry::default();
        // The core prelude ships in every program as the one implicit
        // import.
        modules.register(STD_MODULE, STD_PRELUDE);
        modules.base_implicit_imports = vec![STD_MODULE.to_string()];
        // Libraries sitting on PETAL_PATH are available by package name, the
        // same as the ones a host adds with `add_module_path`. Discovery here
        // is non-fatal: PETAL_PATH is the machine's ambient setting, not this
        // program's argument, so a broken manifest somewhere on it must not
        // fail every Env in the process. The failures are still kept, so
        // `petal packages` can say why a library it was expected to find is
        // missing (see `ambient_package_errors`).
        let mut ambient_package_errors = Vec::new();
        for dir in std::env::var("PETAL_PATH")
            .unwrap_or_default()
            .split(':')
            .filter(|s| !s.is_empty())
        {
            let (packages, errors) = crate::package::discover_packages(std::path::Path::new(dir));
            ambient_package_errors.extend(errors);
            for package in packages {
                modules.add_search_package(package);
            }
        }
        Self {
            programs: HashMap::new(),
            stacks: HashMap::new(),
            native_fns,
            symbols: SymbolTable::new(),
            contexts,
            default_context,
            next_context_id: 2,
            trace: TraceBuffer::new(),
            observations: Observations::new(),
            profile: VmProfile::new(),
            effect_audit: EffectAudit::new(),
            static_audit: crate::static_audit::StaticAudit::from_env(),
            next_program_id: 1,
            next_stack_id: 1,
            policy: RunPolicy::from_env(),
            bytecode: HashMap::new(),
            modules,
            compile_count: std::cell::Cell::new(0),
            lower_count: std::cell::Cell::new(0),
            handle_classes: Vec::new(),
            package_errors: Vec::new(),
            ambient_package_errors,
            check_only: false,
            provided: [(
                CORE_INTERFACE.to_string(),
                crate::package::Version::parse(env!("CARGO_PKG_VERSION"))
                    .expect("the crate version is a version"),
            )]
            .into_iter()
            .collect(),
        }
    }

    /// Announce that this host provides the interface `name` at `version`
    /// (`"0.4.0"`): it registers the natives the interface package of that
    /// name declares. A package states what it needs in its `petal.toml`
    /// (`[requires] ui = ">=0.4"`), and loading a program whose packages
    /// require an interface this `Env` does not provide — or provides at a
    /// version the requirement rejects — is a load error before any line
    /// runs. Call it where the host registers the natives; announcing a name
    /// again replaces its version.
    ///
    /// Errors when `version` is not a dotted number (`1`, `0.4`, `1.2.3`).
    pub fn provides(&mut self, name: &str, version: &str) -> Result<(), String> {
        let parsed = crate::package::Version::parse(version).ok_or_else(|| {
            format!("'{version}' is not a version (expected digits and dots, like 0.4.0)")
        })?;
        self.provided.insert(name.to_string(), parsed);
        Ok(())
    }

    /// [`provides`](Self::provides) for an interface package this host has
    /// registered ([`register_package`](Self::register_package) with its
    /// `petal.toml` among the sources, or [`add_package`](Self::add_package)):
    /// the interface is announced under the package's name at the version its
    /// manifest gives, so the two cannot drift apart. Errors when no such
    /// package is registered or its manifest has no (numeric) version.
    pub fn provides_package(&mut self, name: &str) -> Result<(), String> {
        let version = self
            .modules
            .packages()
            .into_iter()
            .find(|p| p.info.name == name)
            .ok_or_else(|| format!("no package named '{name}' is registered"))?
            .info
            .version
            .clone()
            .ok_or_else(|| {
                format!(
                    "package '{name}' has no version to provide — register its petal.toml \
                     with it (a source named \"petal.toml\"), or call provides(name, version)"
                )
            })?;
        self.provides(name, &version)
    }

    /// The interfaces this `Env` provides, `(name, version)`, sorted by name.
    /// Always includes `core`.
    pub fn provided_interfaces(&self) -> Vec<(String, String)> {
        self.provided
            .iter()
            .map(|(name, version)| (name.clone(), version.to_string()))
            .collect()
    }

    /// The version of the interface `name` this `Env` provides, if any.
    pub fn provided_version(&self, name: &str) -> Option<String> {
        self.provided.get(name).map(|v| v.to_string())
    }

    /// Check `program`'s required host interfaces
    /// ([`Program::requires`](crate::program::Program::requires)) against
    /// what this `Env` [provides](Self::provides). Every compile that is not
    /// [check-only](Self::set_check_only) runs this, before linking.
    pub fn check_requires(&self, program: &Program) -> Result<(), crate::error::LoadError> {
        let mut problems: Vec<String> = Vec::new();
        for req in &program.requires {
            match self.provided.get(&req.interface) {
                None => problems.push(format!(
                    "package '{}' requires the host interface `{}` {}, which this host does \
                     not provide",
                    req.package, req.interface, req.version
                )),
                Some(have) if !req.version.matches(have) => problems.push(format!(
                    "package '{}' requires the host interface `{}` {}, and this host provides \
                     `{}` {have}",
                    req.package, req.interface, req.version, req.interface
                )),
                Some(_) => {}
            }
        }
        if problems.is_empty() {
            return Ok(());
        }
        let ui_hint = if program
            .requires
            .iter()
            .any(|r| r.interface == "ui" && !self.provided.contains_key("ui"))
        {
            crate::native_decl::UI_RUNNER_HINT
        } else {
            ""
        };
        Err(crate::error::LoadError::message(
            crate::error::Phase::Link,
            format!(
                "{} — the program cannot run here; run it with a host that provides {}{ui_hint}",
                problems.join("; "),
                if problems.len() == 1 { "it" } else { "them" }
            ),
        ))
    }

    /// Compile without linking: a program may declare (`native fn`) natives
    /// this `Env` does not register. For tools that only *check* a program —
    /// `petal check`, the LSP, lint, suggest — which answer for a host's
    /// natives from the host's interface module without being that host.
    /// Running such a program still fails, at the first call to a native
    /// that is not there ("Unknown builtin").
    ///
    /// Off by default: loading a program into an `Env` means it is about to
    /// run, and a missing native is then a load error before any line does.
    pub fn set_check_only(&mut self, on: bool) {
        self.check_only = on;
    }

    /// Whether this `Env` compiles without linking
    /// ([`set_check_only`](Self::set_check_only)).
    pub fn is_check_only(&self) -> bool {
        self.check_only
    }

    /// Linking: every native a module of `program` declares with `native fn`
    /// and the program's code refers to must be registered here. The error names each declaring module and its
    /// missing natives — a few, then "+N more".
    ///
    /// Every compile this `Env` does runs this (unless it is
    /// [check-only](Self::set_check_only)), so a host normally never calls
    /// it; it is public for a host that builds a `Program` some other way.
    pub fn link_program(&self, program: &Program) -> Result<(), crate::error::LoadError> {
        match crate::native_decl::missing_natives_message(&program.native_links, |name| {
            self.native_fns.lookup_name(name).is_some()
        }) {
            Some(message) => Err(crate::error::LoadError::message(
                crate::error::Phase::Link,
                message,
            )),
            None => Ok(()),
        }
    }

    /// Apply a native interface to this `Env`'s registered natives: every
    /// `native fn` declared in `source` gives the native it binds its
    /// parameter lists, so a call may pass that native's arguments by name
    /// (`fill(x: 0, y: 0, w: 10, h: 10)`). This is how a host's natives get
    /// the parameter names its interface module declares — call it after
    /// registering the natives, with the same source the host registers as
    /// the module. The builtins get theirs from the bundled `core` package
    /// without being asked.
    ///
    /// A declaration replaces whatever parameter lists the native had
    /// ([`declare_native_params`](Self::declare_native_params) included), so
    /// applying an interface twice is harmless.
    ///
    /// Returns the host names `source` declares that are *not* registered
    /// here, in no particular order — empty when the host provides its whole
    /// interface. Errors when `source` does not parse, or declares one name
    /// for two host natives.
    pub fn declare_natives(&mut self, source: &str) -> Result<Vec<String>, String> {
        let decls = crate::native_decl::NativeDecls::parse(source)?;
        Ok(decls.apply_to(&mut self.native_fns))
    }

    /// Set how subsequent runs execute (see [`RunPolicy`]). Changing the
    /// optimizer passes re-lowers each program on its next run; changing
    /// memoization or gating does not.
    ///
    /// Set it between runs, not inside one: a re-lowering mid-run would leave
    /// live frames pointing into code that no longer exists.
    pub fn set_policy(&mut self, policy: RunPolicy) {
        self.policy = policy;
    }

    /// The active run policy. Starts as [`RunPolicy::from_env`].
    pub fn policy(&self) -> RunPolicy {
        self.policy
    }

    /// Ensure `pid`'s program is lowered to bytecode and cached. Returns the
    /// lowering error (naming the first unlowered op) if it cannot be lowered.
    pub(crate) fn ensure_bytecode(&mut self, pid: ProgramId) -> Result<(), String> {
        let flags = self.effective_opt_flags();
        // Serve the cache only if it was lowered with the flags in effect now.
        if self.bytecode.get(&pid).map(|(f, _)| *f) == Some(flags) {
            return Ok(());
        }
        let program = self.programs.get(&pid).ok_or("Program not found")?;
        self.lower_count.set(self.lower_count.get() + 1);
        let mut bc = self.lower_with(program, flags)?;
        // Bind builtin names to native ids now, once, rather than per call.
        bc.resolve_builtin_names(&program.constants, |name| {
            self.native_fns.lookup_name(name).map(|id| id.0)
        });
        // The static audit indexes the root code it is about to watch, and in
        // its static-only mode rewrites it (see `crate::static_audit`).
        if self.static_audit.enabled {
            let analysis = crate::static_classify::analyze(program, &self.native_fns);
            self.static_audit.prepare(pid, program, &mut bc, &analysis)?;
        }
        self.bytecode.insert(pid, (flags, bc));
        Ok(())
    }

    /// How much front-end and lowering work this env has done since it was
    /// created: the number of source compiles (every `load_program*` and
    /// `compile_program*`, successful or not) and the number of bytecode
    /// lowerings. A host or a test reads the difference across a reload to
    /// see what the reload cost: an incremental one
    /// ([`reload_program`](Self::reload_program) reporting `relocated` or
    /// `patched`) adds nothing to either.
    pub fn work_counters(&self) -> WorkCounters {
        WorkCounters {
            compiles: self.compile_count.get(),
            lowerings: self.lower_count.get(),
        }
    }

    /// Lower `pid` to bytecode now, under the current policy, instead of on
    /// its first run. A host that wants the cost of a reload paid at the
    /// reload (not as a hitch in the next frame) calls this after
    /// [`reload_program`](Self::reload_program); it is a no-op when the
    /// lowering is already cached.
    pub fn lower_program(&mut self, pid: ProgramId) -> Result<(), String> {
        self.ensure_bytecode(pid)
    }

    /// The bytecode `pid` runs, disassembled: the cached lowering itself
    /// (lowering first if the program has not run yet), not a fresh one. The
    /// text `petal show-bytecode` prints for the same program and policy.
    pub fn bytecode_text(&mut self, pid: ProgramId) -> Result<String, String> {
        self.ensure_bytecode(pid)?;
        let program = self.programs.get(&pid).ok_or("Program not found")?;
        let (_, bc) = self.bytecode.get(&pid).ok_or("Program not lowered")?;
        Ok(crate::backend::bytecode::disasm::render_text(bc, program))
    }

    /// The configured flags plus the ones the *current* debug facilities
    /// require. Observation and the `explain` trace both read per-instruction
    /// results, so turning either on holds back the optimization that would
    /// delete the instructions they read (see
    /// [`OptFlags::preserve_observations`]). Because the result is the bytecode
    /// cache key, toggling a facility re-lowers the program on the next run
    /// rather than silently running code compiled for the other setting.
    ///
    /// Toggle between runs, not inside one: a re-lowering mid-run would leave
    /// live frames pointing into code that no longer exists.
    fn effective_opt_flags(&self) -> OptFlags {
        let mut flags = self.policy.opts;
        flags.preserve_observations |= self.observations.enabled || self.trace.enabled;
        flags.preserve_trace |= self.trace.enabled;
        // A run the static cache skips records nothing, so it is off whenever
        // something reads what instructions recorded: observations, the
        // trace, the static audit (which watches static code run and, in its
        // static-only mode, rewrites the root code itself).
        if flags.preserve_observations || flags.preserve_trace || self.static_audit.enabled {
            flags.static_cache = false;
        }
        flags
    }

    /// Seed the random-number generator of every execution context in this
    /// env, so `random()` / `random_int()` / `choose()` replay identically.
    ///
    /// Apply it *once*, before the first run: the PRNG then advances naturally
    /// across runs, `reset_stack` frames and forks (a fork copies the state it
    /// was forked at), which is what makes a whole multi-frame session — not
    /// just its first frame — reproducible. Re-seeding every frame would
    /// instead hand every frame the same numbers.
    ///
    /// Seed 0 is remapped ([`normalize_seed`](crate::builtins::normalize_seed)),
    /// since xorshift has no valid zero state.
    pub fn set_seed(&mut self, seed: u64) {
        let state = crate::builtins::normalize_seed(seed);
        for ctx in self.contexts.values_mut() {
            ctx.rng_state = state;
        }
        // A reseeded stream draws different numbers: any run that drew from
        // it is stale (and one that did not is unaffected, but the record
        // cannot say which draws would come next, so force).
        for stack in self.stacks.values_mut() {
            stack.run_deps.force();
        }
    }

    /// Turn stdout echoing of `print` on or off for every execution context in
    /// this env (see [`ExecutionContext::echo`]).
    ///
    /// Prints always accumulate in the context's output buffer; `echo` only
    /// controls whether they *also* go to the process's real stdout. An
    /// embedder that reports prints itself — a headless driver writing a
    /// machine-readable trace, say — wants it off, so its stdout carries
    /// nothing but the trace.
    ///
    /// Like [`set_seed`](Self::set_seed), apply it before the first run;
    /// contexts created later (forks) inherit the setting from their parent.
    pub fn set_echo(&mut self, on: bool) {
        for ctx in self.contexts.values_mut() {
            ctx.set_echo(on);
        }
    }

    /// Turn the value-duplication and heap-allocation counters on or off for
    /// every execution context in this env (see [`crate::stats`]). They start
    /// on in debug builds and off in release ones; `petal run --dup-stats` and
    /// `petal bench` switch them on. Like [`set_echo`](Self::set_echo), apply
    /// it before the first run; forks inherit the setting.
    pub fn set_heap_stats(&mut self, on: bool) {
        for ctx in self.contexts.values_mut() {
            ctx.heap.set_stats_enabled(on);
        }
    }

    /// Shared access to one execution context.
    fn ctx(&self, ck: ContextKey) -> &ExecutionContext {
        self.contexts.get(&ck).expect("context exists")
    }

    /// Mutable access to one execution context.
    fn ctx_mut(&mut self, ck: ContextKey) -> &mut ExecutionContext {
        self.contexts.get_mut(&ck).expect("context exists")
    }

    /// Resolve the context a stack is bound to. `None` if the stack is unknown.
    /// The basis of the `*_for(stack)` accessors that reach a fork's own heap /
    /// output / bindings rather than the default context.
    fn ctx_for(&self, stack_id: StackKey) -> Option<ContextKey> {
        self.stacks.get(&stack_id).map(|s| s.context)
    }

    // ── Module registration & resolution ─────────────────────────

    /// Register an in-memory module: `import name` resolves to `source`
    /// without touching the filesystem. This is how an embedder ships a
    /// Petal-source library (e.g. `env.register_module("ui", include_str!(
    /// "ui.ptl"))`) and how wasm hosts with no filesystem provide modules.
    /// `name` is a package path (`ui`, `bloom/menu`) and is the first place
    /// that path is looked up. A relative import in `source` resolves against
    /// `name` (`./motion` in `bloom/menu` is `bloom/motion`). Call before
    /// `load_program`.
    pub fn register_module(&mut self, name: &str, source: &str) {
        self.modules.register(name, source);
        self.apply_interface(source);
    }

    /// Give the natives registered so far the parameter lists the `native fn`
    /// declarations of `source` give them (see
    /// [`declare_natives`](Self::declare_natives)). Best-effort: a source
    /// that does not parse is reported when a program imports it.
    fn apply_interface(&mut self, source: &str) {
        if source.contains("native fn")
            && let Ok(decls) = crate::native_decl::NativeDecls::parse(source)
        {
            decls.apply_to(&mut self.native_fns);
        }
    }

    /// Apply the interface every module of the registered package `name`
    /// declares.
    fn apply_package_interface(&mut self, name: &str) {
        for (_, source) in self.modules.package_sources(name).unwrap_or_default() {
            self.apply_interface(&source);
        }
    }

    /// Add a directory to the package search path. The CLI's `-I <dir>`
    /// lands here; `PETAL_PATH` directories are searched after these.
    ///
    /// `dir` itself, and each directory directly under it, is offered as a
    /// package when it holds a `petal.toml` (see [`crate::package`]), so
    /// dropping a library beside a search path makes it importable as
    /// `<package>/<module>`. A search path finds packages only: a loose
    /// `.ptl` file is imported relative to its importer (`import ./x`).
    /// Packages found here rank below the host's own registrations and the
    /// importing file's own package, and above the bundled ones.
    ///
    /// Discovery is best-effort — a manifest that will not load is collected
    /// into [`package_errors`](Self::package_errors) rather than thrown,
    /// because a search directory is allowed to contain things that are not
    /// packages. Use [`add_package`](Self::add_package) when the caller
    /// knows the directory *is* a library and wants the error.
    pub fn add_module_path(&mut self, dir: std::path::PathBuf) {
        let (packages, errors) = crate::package::discover_packages(&dir);
        for package in packages {
            // First search directory wins, matching search-path precedence.
            self.modules.add_search_package(package);
        }
        self.package_errors.extend(errors);
    }

    /// Register a whole library from its root directory — the one call that
    /// replaces a loop of [`register_module`](Self::register_module)s.
    ///
    /// `root` must hold a `petal.toml`; every `.ptl` file under the package's
    /// module directory becomes importable as `<name>/<module>`, where `name`
    /// is the manifest's `[package] name`. Returns what was registered.
    /// Errors (missing/malformed manifest, bad name, missing module
    /// directory) are [`LoadError`](crate::error::LoadError)s naming the file.
    pub fn add_package(
        &mut self,
        root: impl AsRef<std::path::Path>,
    ) -> Result<crate::package::PackageInfo, crate::error::LoadError> {
        let package = crate::package::Package::from_dir(root.as_ref())?;
        let info = package.info.clone();
        self.modules.add_package(package);
        self.apply_package_interface(&info.name);
        Ok(info)
    }

    /// The in-memory twin of [`add_package`](Self::add_package): register a
    /// whole library from sources the host already holds — `include_str!`,
    /// `include_dir!`, or anything a wasm host carries with no filesystem.
    ///
    /// ```no_run
    /// # let mut env = petal::env::Env::new();
    /// env.register_package("bloom", [
    ///     ("menu", include_str!("../../prelude/std.ptl")),
    ///     ("motion", include_str!("../../prelude/std.ptl")),
    /// ]).unwrap();
    /// ```
    ///
    /// Module names are package-relative (`menu`, `ui/menu`); a `.ptl` suffix
    /// is stripped if present. The modules are then importable as
    /// `bloom/menu`, and reach each other with `import ./motion`.
    pub fn register_package<'a, N: AsRef<str>, S: AsRef<str>>(
        &mut self,
        name: &str,
        modules: impl IntoIterator<Item = (N, S)>,
    ) -> Result<crate::package::PackageInfo, crate::error::LoadError> {
        let package = crate::package::Package::in_memory(name, modules)?;
        let info = package.info.clone();
        self.modules.add_package(package);
        self.apply_package_interface(&info.name);
        Ok(info)
    }

    /// Check this `Env`'s registered natives against the interface packages
    /// that declare them: every `native fn` a module of `packages` declares
    /// must be registered here, and every native registered here beyond the
    /// builtins must be declared by one of them.
    ///
    /// This is a host's drift test. A host registers its natives and its
    /// interface package(s), then asserts
    /// `env.verify_interface(&["ui", "sdl"]).is_ok()`: a native added in Rust
    /// with no declaration is unreachable from script, and a declaration
    /// with no native is a load error for every script that uses it, and
    /// this reports both before a user does.
    ///
    /// Each name is a package (host-registered, on a search path, or bundled),
    /// one module of a package (`ui/input`, for a host that registers only
    /// that half), or a module registered in memory under that name. Passing all of a
    /// host's packages in one call is what makes the "registered but not
    /// declared" half meaningful.
    pub fn verify_interface(&self, packages: &[&str]) -> Result<(), InterfaceMismatch> {
        let mut mismatch = InterfaceMismatch::default();
        let mut declared: std::collections::HashSet<String> = Default::default();
        for name in packages {
            let Some(sources) = self.modules.package_sources(name) else {
                mismatch
                    .problems
                    .push(format!("no package or module named '{name}' is available"));
                continue;
            };
            for (module, source) in sources {
                match crate::native_decl::NativeDecls::parse(&source) {
                    Ok(decls) => declared.extend(decls.host_names()),
                    Err(e) => mismatch.problems.push(format!("{module}: {e}")),
                }
            }
        }
        // A host may register a native under a builtin's name (its own
        // `log`), replacing it: declared and registered, so not missing —
        // and, being a builtin's name (`core` or a domain builtin package),
        // not something a host's package has to declare.
        let core = crate::native_decl::builtin_host_names();
        let all: std::collections::HashSet<String> = self.native_fn_names().into_iter().collect();
        mismatch.unregistered = declared.difference(&all).cloned().collect();
        mismatch.undeclared = all
            .iter()
            .filter(|name| !core.contains(*name) && !declared.contains(*name))
            .cloned()
            .collect();
        mismatch.unregistered.sort();
        mismatch.undeclared.sort();
        if mismatch.is_empty() {
            Ok(())
        } else {
            Err(mismatch)
        }
    }

    /// Every registered package — what a host prints, or tells a script, when
    /// asked which libraries are available.
    pub fn packages(&self) -> Vec<crate::package::PackageInfo> {
        self.modules
            .packages()
            .into_iter()
            .map(|p| p.info.clone())
            .collect()
    }

    /// The project the file at `file` belongs to — the package whose
    /// `petal.toml` is nearest at or above it — or `None` when there is no
    /// manifest above the file. `Err` when that manifest does not load.
    pub fn project_of(
        &self,
        file: &std::path::Path,
    ) -> Result<Option<crate::package::PackageInfo>, String> {
        use crate::module::ModuleResolver;
        self.modules.begin_load(Some(file));
        Ok(self.modules.package_of_file(file)?.map(|p| p.info.clone()))
    }

    /// The `[dependencies]` of the project the file at `file` belongs to, as
    /// they resolve: `(import name, the package or why it does not load)`.
    pub fn project_dependencies(
        &self,
        file: &std::path::Path,
    ) -> Vec<(String, Result<crate::package::PackageInfo, String>)> {
        use crate::module::ModuleResolver;
        self.modules.begin_load(Some(file));
        self.modules
            .project_dependencies(file)
            .into_iter()
            .map(|(name, pkg)| (name, pkg.map(|p| p.info.clone())))
            .collect()
    }

    /// Let a load run `petal install` for its project when it imports a git
    /// dependency that is not installed (fetching with `git`; see
    /// [`crate::pkgman`]). Off by default — a host decides when the network
    /// is touched. The `petal` CLI turns it on for the commands that compile.
    pub fn set_install_on_demand(&mut self, on: bool) {
        self.modules.install_on_demand = on;
    }

    /// The packages compiled into the binary, in bundling order.
    pub fn bundled_packages() -> Vec<crate::package::PackageInfo> {
        crate::module::bundled_package_infos()
    }

    /// Manifests that failed to load during the best-effort package discovery
    /// of [`add_module_path`](Self::add_module_path).
    pub fn package_errors(&self) -> &[crate::error::LoadError] {
        &self.package_errors
    }

    /// Manifests on `PETAL_PATH` that looked like packages and would not load.
    /// Non-fatal by design (see [`Env::new`]) — `petal packages` prints them
    /// so a library that silently failed to register is still accounted for.
    pub fn ambient_package_errors(&self) -> &[crate::error::LoadError] {
        &self.ambient_package_errors
    }

    /// Compile as if the file at `path` held `source`: every import that
    /// resolves to that file loads this text instead of the one on disk. For
    /// tools that must prove an edit before writing it (`petal apply-change`).
    pub fn override_file_source(&mut self, path: &std::path::Path, source: &str) {
        self.modules.override_file(path, source);
    }

    /// The file `import name` written in the file at `importer` resolves to,
    /// or `None` when it resolves to nothing or to a module held in memory.
    pub fn resolve_module_file(
        &self,
        name: &str,
        importer: &std::path::Path,
    ) -> Option<std::path::PathBuf> {
        use crate::module::{Importer, ModuleOrigin, ModuleResolver};
        let origin = ModuleOrigin::File(importer.to_path_buf());
        let importer_name = self.modules.module_name_of_file(importer);
        self.modules.begin_load(Some(importer));
        let importer = Importer {
            name: importer_name.as_deref(),
            origin: Some(&origin),
        };
        match self.modules.resolve(name, &importer).ok()?.origin {
            ModuleOrigin::File(path) => Some(path),
            ModuleOrigin::Memory => None,
        }
    }

    /// Let a program loaded with no file path (`load_program(source)`)
    /// resolve `import ./x` against `dir`, after the in-memory modules. The
    /// CLI points this at the working directory for `-e` code and stdin; a
    /// host that never calls it keeps such programs to in-memory modules.
    pub fn set_inline_import_dir(&mut self, dir: Option<std::path::PathBuf>) {
        self.modules.inline_import_dir = dir;
    }

    /// What `import path`, written in the file at `importer`, resolves to:
    /// the module's canonical name. `Err` is why it resolves to nothing.
    pub fn resolve_import(
        &self,
        path: &str,
        importer: &std::path::Path,
    ) -> Result<String, String> {
        use crate::module::{Importer, ModuleOrigin, ModuleResolver};
        let origin = ModuleOrigin::File(importer.to_path_buf());
        self.modules.begin_load(Some(importer));
        let importer_name = self.modules.module_name_of_file(importer);
        let found = self.modules.resolve(
            path,
            &Importer {
                name: importer_name.as_deref(),
                origin: Some(&origin),
            },
        )?;
        Ok(found.name.unwrap_or_else(|| path.to_string()))
    }

    /// The names the module at the package path `path` exports — what a
    /// plain `import path` binds bare.
    pub fn exports_of(&self, path: &str) -> Result<Vec<String>, crate::error::LoadError> {
        self.exports_and_modules_of(path).map(|(exports, _)| exports)
    }

    /// [`exports_of`](Self::exports_of), plus the canonical name of every
    /// module `import path` loads (the module itself and everything it
    /// imports, transitively).
    pub fn exports_and_modules_of(
        &self,
        path: &str,
    ) -> Result<(Vec<String>, Vec<String>), crate::error::LoadError> {
        self.exports_and_modules_of_at(path, None)
    }

    /// [`exports_and_modules_of`](Self::exports_and_modules_of) as the file
    /// at `importer` would see it: a `[dependencies]` entry of that file's
    /// project resolves.
    pub fn exports_and_modules_of_at(
        &self,
        path: &str,
        importer: Option<&std::path::Path>,
    ) -> Result<(Vec<String>, Vec<String>), crate::error::LoadError> {
        let modules = crate::module::load_modules(
            &format!("import {path}\n"),
            importer,
            &self.modules,
            &[],
        )?;
        let program = Compiler::new().compile_modules(&modules, ProgramId(0), &self.native_fns)?;
        let exports = program
            .imports
            .entry
            .first()
            .and_then(|import| program.imports.exports.get(&import.module))
            .cloned()
            .unwrap_or_default();
        let loaded = modules.iter().filter_map(|m| m.name.clone()).collect();
        Ok((exports, loaded))
    }

    /// Every package path a script could write after `import` to reach a
    /// module that is not already in scope: each registered, search-path and
    /// bundled package by its own name when it has a module of that name,
    /// else each of its modules (`pkg/module`). The always-in-scope `core`
    /// package and `std` are left out.
    pub fn importable_paths(&self) -> Vec<String> {
        self.modules.importable_paths()
    }

    /// [`importable_paths`](Self::importable_paths) for the file at `file`:
    /// the `[dependencies]` of its project (the nearest `petal.toml` above
    /// it) that resolve are importable too, under their manifest keys.
    pub fn importable_paths_at(&self, file: &std::path::Path) -> Vec<String> {
        let mut paths = self.importable_paths();
        for (name, package) in self.project_dependencies(file) {
            let Ok(package) = package else { continue };
            // The manifest's package of this name replaces any other.
            paths.retain(|p| p.split('/').next() != Some(name.as_str()));
            if package.modules.iter().any(|m| *m == package.name) {
                paths.push(name);
            } else {
                paths.extend(package.modules.iter().map(|m| format!("{name}/{m}")));
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    /// Resolve imports and compile: the shared back half of
    /// [`load_program`](Self::load_program) / [`compile_program`](Self::compile_program).
    /// `origin` is the entry source's file path when it has one — the anchor
    /// for resolving imports relative to the importing file.
    fn compile_source(
        &self,
        program_id: ProgramId,
        source: &str,
        origin: Option<&std::path::Path>,
    ) -> Result<Program, crate::error::LoadError> {
        self.compile_count.set(self.compile_count.get() + 1);
        let modules = crate::module::load_modules(
            source,
            origin,
            &self.modules,
            &self.modules.base_implicit_imports,
        )?;
        let mut program =
            Compiler::new().compile_modules(&modules, program_id, &self.native_fns)?;
        self.scan_unresolved(&mut program);
        self.check_consts(&mut program);
        if !self.check_only {
            self.check_requires(&program)?;
            self.link_program(&program)?;
        }
        Ok(program)
    }

    /// The name scan that ends every compile: each call to, or read of, a
    /// name nothing defines, and each named argument a native cannot take,
    /// joins the program's diagnostics as an error
    /// ([`unresolved_globals`](crate::typecheck::globals::unresolved_globals)).
    /// The compiler leaves these to fail when their line runs; with the scan
    /// done here, everything that reads a program's `warnings` — `petal
    /// check`, `petal run`, the LSP, a host — sees the same findings.
    fn scan_unresolved(&self, program: &mut Program) {
        let found = crate::typecheck::globals::unresolved_globals(program, |name| {
            self.native_signatures(name)
        });
        program.warnings.extend(found);
    }

    /// The `const` check that ends every compile: a declaration written
    /// `const` whose value is not static data joins the program's diagnostics
    /// as an error carrying the classifier's reason chain
    /// ([`crate::static_classify`]). The binding itself compiles and runs as
    /// the `let` it otherwise is.
    ///
    /// The answer is this `Env`'s: a native counts as constant only when it
    /// is registered `CONST` here. A `const` that is dynamic only because of
    /// natives nothing here registers (`petal check` and the language server
    /// on a script written for a host) is not reported: it cannot be verified
    /// here, and the host that has those natives reports it if it is wrong.
    /// The classifier runs only for a program that declares a `const`.
    fn check_consts(&self, program: &mut Program) {
        use crate::static_classify::{ConstCheck, DeclClass};
        if !program.terms.iter().any(|t| t.is_const) {
            return;
        }
        let report = crate::static_classify::classify(program, &self.native_fns);
        for d in &report.declarations {
            if d.const_check != Some(ConstCheck::NotConstant) {
                continue;
            }
            // Dynamic because of an error the compile has already reported:
            // that error is the finding.
            if d.reason.last().is_some_and(|r| r.compile_error) {
                continue;
            }
            let span = program
                .source_map
                .get(d.term)
                .copied()
                .unwrap_or(crate::source_map::ZERO_SPAN);
            if d.class == DeclClass::Function {
                program.warnings.push(crate::diagnostic::Diagnostic::error(
                    span,
                    format!(
                        "`{}` is declared `const` but is a function, not a constant value; \
                         declare it with `fn` or `let`",
                        d.name
                    ),
                ));
                continue;
            }
            let mut template = format!("`{}` is declared `const` but is not constant", d.name);
            let mut cited = Vec::new();
            for step in &d.reason {
                template.push_str(&format!("\n  = {} {}", step.subject, step.text));
                let Some(at) = step.span else { continue };
                let line = match cited.len() {
                    0 => "{line}".to_string(),
                    n => format!("{{line:{n}}}"),
                };
                match &step.file {
                    Some(file) => template.push_str(&format!(" at {file}:{line}")),
                    None => template.push_str(&format!(" at line {line}")),
                }
                cited.push(at);
            }
            program.warnings.push(crate::diagnostic::Diagnostic::error_citing(
                span, template, cited,
            ));
        }
    }

    /// Check a loaded program: will it run? The findings of its compile
    /// (which include the name scan), plus whether it lowers to bytecode
    /// under the policy in effect. See [`crate::check`].
    ///
    /// The lowering is the one a run would do and is kept, so checking a
    /// program before running it costs nothing extra. Nothing is linked: a
    /// program written for a host this `Env` is not can still pass; a run
    /// follows a passed check with [`link`](Self::link).
    pub fn check_program(&mut self, pid: ProgramId) -> crate::check::CheckReport {
        let lower_error = self.ensure_bytecode(pid).err();
        crate::check::CheckReport {
            diagnostics: self
                .programs
                .get(&pid)
                .map(|p| p.warnings.clone())
                .unwrap_or_default(),
            lower_error,
        }
    }

    /// Every finding of `pid`'s compile, rendered as `petal check` prints
    /// them (empty when there are none). For a host that has no diagnostics
    /// surface of its own: print this to stderr after a load or a reload.
    pub fn diagnostics_text(&self, pid: ProgramId) -> String {
        self.programs
            .get(&pid)
            .map(|p| crate::check::render_diagnostics(p, &p.warnings, false))
            .unwrap_or_default()
    }

    /// Link a program that was loaded [check-only](Self::set_check_only):
    /// the host interfaces it requires and the natives it declares must be
    /// this `Env`'s ([`check_requires`](Self::check_requires),
    /// [`link_program`](Self::link_program)). A load that is not check-only
    /// has already done this.
    pub fn link(&self, pid: ProgramId) -> Result<(), crate::error::LoadError> {
        let Some(program) = self.programs.get(&pid) else {
            return Ok(());
        };
        self.check_requires(program)?;
        self.link_program(program)
    }

    /// Compile for `petal suggest`: the evidence every module's type checker
    /// observed, plus the class table the suggestions are spelled against.
    ///
    /// Separate from the ordinary compile because collecting is pure overhead
    /// for a program that is going to run — see
    /// [`Compiler::collecting_inferences`]. The `Program` is dropped: this
    /// path only ever reads what the front end learned on the way.
    pub fn compile_collecting(
        &self,
        source: &str,
        origin: Option<&std::path::Path>,
    ) -> Result<
        (
            crate::typecheck::infer::Inferences,
            crate::classes::ClassTable,
        ),
        crate::error::LoadError,
    > {
        let (_program, inferences, classes) = self.compile_collecting_program(source, origin)?;
        Ok((inferences, classes))
    }

    /// [`compile_collecting`](Self::compile_collecting), keeping the
    /// `Program` too — for the half of `petal suggest` that reads the IR
    /// (which function a call resolves to) rather than the checker's evidence.
    #[allow(clippy::type_complexity)]
    pub fn compile_collecting_program(
        &self,
        source: &str,
        origin: Option<&std::path::Path>,
    ) -> Result<
        (
            Program,
            crate::typecheck::infer::Inferences,
            crate::classes::ClassTable,
        ),
        crate::error::LoadError,
    > {
        let modules = crate::module::load_modules(
            source,
            origin,
            &self.modules,
            &self.modules.base_implicit_imports,
        )?;
        let (mut program, inferences, classes) = Compiler::new()
            .collecting_inferences()
            .compile_modules_collecting(&modules, ProgramId(0), &self.native_fns)?;
        self.scan_unresolved(&mut program);
        self.check_consts(&mut program);
        Ok((program, inferences, classes))
    }

    /// Compile source code into a Program without loading it.
    /// Use this to prepare a program for `transfer_state`.
    pub fn compile_program(&self, program_id: ProgramId, source: &str) -> Result<Program, String> {
        self.compile_program_diag(program_id, source, None)
            .map_err(|e| e.to_string())
    }

    /// [`compile_program`](Self::compile_program) /
    /// [`compile_program_at`](Self::compile_program_at) with a *typed* error —
    /// the recompile half of a hot reload, as
    /// [`load_program_diag`](Self::load_program_diag) is of a first load. A
    /// host that shows compile errors structurally (phase, file, line and
    /// column per diagnostic) uses this rather than recompiling through
    /// `load_program_diag` to recover them. `origin` is the entry file's path
    /// when it has one: imports resolve relative to its directory first.
    pub fn compile_program_diag(
        &self,
        program_id: ProgramId,
        source: &str,
        origin: Option<&std::path::Path>,
    ) -> Result<Program, crate::error::LoadError> {
        self.compile_source(program_id, source, origin)
    }

    /// [`compile_program`](Self::compile_program) for source that lives at a
    /// filesystem path: imports resolve relative to `origin`'s directory
    /// first. Hosts hot-reloading a script file should use this.
    pub fn compile_program_at(
        &self,
        program_id: ProgramId,
        source: &str,
        origin: &std::path::Path,
    ) -> Result<Program, String> {
        self.compile_program_diag(program_id, source, Some(origin))
            .map_err(|e| e.to_string())
    }

    /// Load a program from source code
    pub fn load_program(&mut self, source: &str) -> Result<ProgramId, String> {
        self.load_program_diag(source, None)
            .map_err(|e| e.to_string())
    }

    /// [`load_program`](Self::load_program) for source read from a file:
    /// imports resolve relative to `origin`'s directory first.
    pub fn load_program_at(
        &mut self,
        source: &str,
        origin: &std::path::Path,
    ) -> Result<ProgramId, String> {
        self.load_program_diag(source, Some(origin))
            .map_err(|e| e.to_string())
    }

    /// [`load_program`](Self::load_program) with a *typed* error: which phase
    /// of the front end rejected the program, and each diagnostic separately
    /// (see [`crate::error::LoadError`]). The `String`-returning methods above
    /// are this one plus `.to_string()`, so the public facade is unchanged;
    /// only tools that need structure (the CLI's `--json` output) use this.
    pub fn load_program_diag(
        &mut self,
        source: &str,
        origin: Option<&std::path::Path>,
    ) -> Result<ProgramId, crate::error::LoadError> {
        let id = ProgramId(self.next_program_id);
        let program = self.compile_source(id, source, origin)?;
        self.next_program_id += 1;
        self.programs.insert(id, program);
        Ok(id)
    }

    /// The module manifest of a loaded program: one entry per source file it
    /// was compiled from (entry file first). Hosts use this to answer "does
    /// editing file X invalidate program P?" and to watch every file a
    /// program depends on, not just its entry (see petal-sdl's watcher).
    /// Single-file programs get their one entry with `origin: None` — the
    /// host already knows the entry path it loaded from.
    pub fn module_manifest(&self, program_id: ProgramId) -> Vec<ModuleManifestEntry> {
        let Some(program) = self.programs.get(&program_id) else {
            return Vec::new();
        };
        let hash = |s: &str| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        };
        if program.source_map.files.is_empty() {
            return vec![ModuleManifestEntry {
                name: "<entry>".to_string(),
                origin: None,
                content_hash: hash(&program.source),
            }];
        }
        program
            .source_map
            .files
            .iter()
            .map(|f| ModuleManifestEntry {
                name: f.name.clone(),
                origin: f.origin.clone(),
                content_hash: hash(&f.source),
            })
            .collect()
    }

    /// Every file on disk a loaded program was compiled from: `entry` (the
    /// path the host loaded it from, if any) first, then each module in the
    /// [`module_manifest`](Self::module_manifest) that has a filesystem
    /// origin. Deduped by filesystem identity (two spellings of one file
    /// collapse, the first spelling kept). Modules registered in memory
    /// ([`register_module`](Self::register_module), the `std` prelude) and
    /// modules of in-memory or bundled packages (`ui`, `io`, …) have no file
    /// and are not listed. These are the files a hot-reloading host
    /// watches; [`watch_program_sources`](Self::watch_program_sources)
    /// snapshots them.
    pub fn program_source_paths(
        &self,
        program_id: ProgramId,
        entry: Option<&std::path::Path>,
    ) -> Vec<std::path::PathBuf> {
        let origins = self
            .module_manifest(program_id)
            .into_iter()
            .filter_map(|m| m.origin)
            // A module of a package held in memory (bundled: `ui`, `io`, …;
            // or host-registered) carries a path that names no file.
            .filter(|path| !self.modules.is_memory_origin(path));
        crate::source_watch::dedup_paths(entry.map(|p| p.to_path_buf()).into_iter().chain(origins))
    }

    /// A [`SourceWatch`](crate::source_watch::SourceWatch) over
    /// [`program_source_paths`](Self::program_source_paths), stamped now.
    /// Take it right after compiling; poll
    /// [`changed`](crate::source_watch::SourceWatch::changed) to decide when
    /// to recompile; take a fresh one from the new program after a reload.
    pub fn watch_program_sources(
        &self,
        program_id: ProgramId,
        entry: Option<&std::path::Path>,
    ) -> crate::source_watch::SourceWatch {
        crate::source_watch::SourceWatch::new(self.program_source_paths(program_id, entry))
    }

    /// Load a program from its JSON IR form (the shape `show-ir --json` emits)
    /// rather than from source. Validates the graph and assigns it a fresh
    /// ProgramId. See `docs/ir-as-target.md`.
    pub fn load_program_ir(&mut self, json: &str) -> Result<ProgramId, String> {
        let mut program = Program::from_json(json)?;
        // Serialized IR does not carry the program's `native fn` links, so
        // they are read back off its calls: every native a `BuiltinCall`
        // names must be registered here, or the load fails before any line
        // runs — the same linking a compile from source does.
        if !self.check_only {
            let mut seen = std::collections::HashSet::new();
            let mut links = Vec::new();
            for term in &program.terms {
                let crate::program::TermOp::BuiltinCall(cid) = term.op else {
                    continue;
                };
                let Some(name) = program.get_string_constant(cid) else {
                    continue;
                };
                if !seen.insert(name) {
                    continue;
                }
                let module = program
                    .source_map
                    .get(term.id)
                    .and_then(|span| program.source_map.file_name_for_span(span))
                    .map(|file| file.trim_end_matches(".ptl"))
                    .unwrap_or(crate::module::INLINE_ENTRY_NAME)
                    .to_string();
                links.push(crate::native_decl::NativeLink {
                    host: name.to_string(),
                    module,
                    signatures: Vec::new(),
                });
            }
            let missing = crate::native_decl::missing_natives_message(&links, |name| {
                self.native_fns.lookup_name(name).is_some()
            });
            if let Some(message) = missing {
                return Err(message);
            }
        }
        self.scan_unresolved(&mut program);
        self.check_consts(&mut program);
        let id = ProgramId(self.next_program_id);
        self.next_program_id += 1;
        program.id = id;
        self.programs.insert(id, program);
        Ok(id)
    }

    /// Create a new execution stack for a program
    pub fn create_stack(&mut self, program_id: ProgramId) -> Result<StackKey, String> {
        // Validate the program exists (the stack references it by id).
        self.programs.get(&program_id).ok_or("Program not found")?;

        let key = StackKey(self.next_stack_id);
        self.next_stack_id += 1;

        // The bytecode VM pushes its own root frame on the first step of a run
        // (gated by `vm_started`), so a freshly created stack needs no frame.
        let stack = Stack::new(key, program_id, self.default_context);

        self.stacks.insert(key, stack);
        Ok(key)
    }

    /// Access the shared trace buffer (for recording/queries).
    pub fn trace(&self) -> &TraceBuffer {
        &self.trace
    }

    /// Mutable access to the trace buffer (to enable/clear/configure).
    pub fn trace_mut(&mut self) -> &mut TraceBuffer {
        &mut self.trace
    }

    /// Access the observation buffer — the last value bound to each named IR
    /// term. Read it by name with
    /// [`get_observations_json`](Self::get_observations_json).
    pub fn observations(&self) -> &Observations {
        &self.observations
    }

    /// Mutable access to the observation buffer (to enable/clear it). Recording
    /// is off by default: `env.observations_mut().enable()` turns it on for
    /// every subsequent run.
    pub fn observations_mut(&mut self) -> &mut Observations {
        &mut self.observations
    }

    /// Execution counters for the last profiled span — opcode and builtin
    /// histograms, call and GC totals. Empty unless profiling was enabled; see
    /// [`crate::profile`].
    pub fn profile(&self) -> &VmProfile {
        &self.profile
    }

    /// Mutable access to the execution counters. Profiling is off by default:
    /// `env.profile_mut().set_enabled(true)` turns it on for subsequent runs
    /// (and clears whatever was already counted).
    pub fn profile_mut(&mut self) -> &mut VmProfile {
        &mut self.profile
    }

    /// The profile report for program `pid`: [`VmProfile::report`] plus the
    /// per-function section, whose function slots are resolved against `pid`'s
    /// bytecode (the lowering that ran). `elapsed` is the wall time measured.
    pub fn profile_report(
        &self,
        pid: ProgramId,
        elapsed: Option<std::time::Duration>,
        top_n: usize,
    ) -> String {
        let bc = self.bytecode.get(&pid).map(|(_, bc)| bc);
        let program = self.programs.get(&pid);
        self.profile.report_with_functions(
            elapsed,
            |nid| self.native_fn_name(nid),
            |slot| {
                let bc = bc?;
                if slot == 0 {
                    return Some("<root>".to_string());
                }
                let f = bc.fns.get(slot - 1)?;
                let name = f.name.as_deref().unwrap_or("<fn>");
                // `file:line` of the body's first instruction with a source
                // position: tells same-named functions in different modules
                // apart, and places anonymous ones.
                let at = program.and_then(|p| {
                    let span = f.origins.iter().flatten().find_map(|&t| {
                        p.source_map.get(t).filter(|s| s.start.line > 0)
                    })?;
                    Some(match p.source_map.file_name_for_span(span) {
                        Some(file) => {
                            let file = file.rsplit('/').next().unwrap_or(file);
                            format!(" {file}:{}", span.start.line)
                        }
                        None => format!(" line {}", span.start.line),
                    })
                });
                Some(format!("{name}{}", at.unwrap_or_default()))
            },
            top_n,
        )
    }

    /// Turn the native effect audit on (or off) for subsequent runs: every
    /// native call is then classified by inference as well as by its row, and
    /// the two are compared in [`effect_audit_report`](Self::effect_audit_report).
    /// Turning it on clears whatever was already observed.
    pub fn set_effect_audit(&mut self, on: bool) {
        self.effect_audit.set_enabled(on);
    }

    /// What the audit has observed so far; see [`crate::effect_audit`].
    pub fn effect_audit(&self) -> &EffectAudit {
        &self.effect_audit
    }

    /// The audit's findings against the registered natives' declared rows.
    pub fn effect_audit_report(&self) -> crate::effect_audit::Report {
        self.effect_audit.report(&self.native_fns, &self.symbols)
    }

    /// Turn the static-value audit on (or off) for subsequent runs: while a
    /// top-level term the classifier calls static is being evaluated, every
    /// dynamic input it touches is recorded, and
    /// [`static_audit_report`](Self::static_audit_report) lists them. In
    /// [`StaticOnly`](crate::static_audit::StaticAuditMode::StaticOnly) mode a
    /// run evaluates the static top-level statements and nothing else.
    ///
    /// Setting the mode clears what was observed and drops every cached
    /// lowering, so set it between runs, never inside one. See
    /// [`crate::static_audit`].
    pub fn set_static_audit(&mut self, mode: crate::static_audit::StaticAuditMode) {
        self.static_audit.set_mode(mode);
        self.bytecode.clear();
    }

    /// The mode [`set_static_audit`](Self::set_static_audit) left.
    pub fn static_audit_mode(&self) -> crate::static_audit::StaticAuditMode {
        self.static_audit.mode()
    }

    /// What the static audit has caught so far, with declarations, natives
    /// and positions named.
    pub fn static_audit_report(&self) -> crate::static_audit::Report {
        self.static_audit
            .report(|pid| self.programs.get(&pid), &self.native_fns, &self.symbols)
    }

    /// The registered name of a native function by table index — how a
    /// [`VmProfile`] report turns its `NativeFnId` counters back into builtin
    /// names. Out-of-range ids report as `<native N>` rather than panicking:
    /// a profile can outlive the table it counted against.
    pub fn native_fn_name(&self, nid: u32) -> String {
        if (nid as usize) < self.native_fns.count() {
            self.native_fns.get_name(NativeFnId(nid)).to_string()
        } else {
            format!("<native {nid}>")
        }
    }

    /// Whether a native of this name is registered.
    pub fn has_native(&self, name: &str) -> bool {
        self.native_fns.lookup_name(name).is_some()
    }

    /// Every registered native's name, in registration order. What a host
    /// test compares against a static list of the natives it provides (see
    /// [`crate::typecheck::globals`]).
    pub fn native_fn_names(&self) -> Vec<String> {
        (0..self.native_fns.count())
            .map(|i| self.native_fns.get_name(NativeFnId(i as u32)).to_string())
            .collect()
    }

    /// Get a reference to a loaded program
    pub fn get_program(&self, id: ProgramId) -> Option<&Program> {
        self.programs.get(&id)
    }

    /// Get a mutable reference to a loaded program — for a tool that adds
    /// diagnostics of its own after the compile (`petal check`).
    pub fn get_program_mut(&mut self, id: ProgramId) -> Option<&mut Program> {
        self.programs.get_mut(&id)
    }

    /// Reset a stack to re-run while keeping state
    pub fn reset_stack(&mut self, stack_id: StackKey) -> Result<(), String> {
        let stack = self.stacks.get_mut(&stack_id).ok_or("Stack not found")?;

        // Keep state, reset execution; the VM re-pushes its root frame on the
        // next run (gated by `vm_started`, cleared by `reset_execution`).
        let ck = stack.context;
        stack.reset_execution();

        // A stack reset is the per-frame boundary (a host resets before each
        // frame's run). Clear the per-frame absorption state — the debug log and
        // the always-on `absorbed_count`s — so both describe just the next frame.
        // The cross-frame resource table itself is kept.
        if let Some(ctx) = self.contexts.get_mut(&ck) {
            ctx.reset_frame_absorption();
        }

        Ok(())
    }

    /// Advance the frame counter of the context `stack_id` runs in (see
    /// [`ExecutionContext::advance_frame`](crate::execution_context::ExecutionContext::advance_frame)).
    /// A host calls this once per rendered frame so resource ages
    /// (`current_frame - frame_started`) grow over time. No-op if the stack is
    /// unknown.
    pub fn advance_frame(&mut self, stack_id: StackKey) {
        if let Some(ck) = self.ctx_for(stack_id) {
            self.ctx_mut(ck).advance_frame();
        }
    }

    /// Turn on the debug-gated pending absorption log in the context `stack_id`
    /// runs in (see
    /// [`ExecutionContext::enable_pending_trace`](crate::execution_context::ExecutionContext::enable_pending_trace)).
    /// The `--trace-pending` CLI flag and the debug protocol flip this before
    /// stepping so the frame's absorptions are recorded. No-op if the stack is
    /// unknown.
    pub fn enable_pending_trace(&mut self, stack_id: StackKey) {
        if let Some(ck) = self.ctx_for(stack_id) {
            self.ctx_mut(ck).enable_pending_trace();
        }
    }

    /// Turn call-site attribution of buffered output on or off for the default
    /// context (see
    /// [`ExecutionContext::enable_emit_trace`](crate::execution_context::ExecutionContext::enable_emit_trace)).
    ///
    /// A host turns this on when it wants to map emitted values — draw commands,
    /// most usefully — back to the code that produced them, and reads the result
    /// with [`take_output_origins`](Self::take_output_origins). Off by default,
    /// so a production run never pays for it.
    pub fn enable_emit_trace(&mut self, on: bool) {
        let ck = self.default_context;
        self.ctx_mut(ck).enable_emit_trace(on);
    }

    /// Whether emit tracing is on for the default context.
    pub fn emit_trace_enabled(&self) -> bool {
        self.ctx(self.default_context).trace_emit
    }

    /// A structured, per-frame report of every live pending resource in the
    /// context `stack_id` runs in: a JSON array of
    /// `{ id, key, state, age_frames, origin, absorbed_count }` objects (see
    /// [`crate::value::pending_report_json`] for the exact shape). This is what
    /// the debug-protocol `pending_report` query and the petal-ui overlay hook
    /// surface, and what `--trace-pending` prints per frame. An empty array when
    /// the program is unknown (no source map to resolve origins against).
    pub fn pending_report(&self, program_id: ProgramId, stack_id: StackKey) -> serde_json::Value {
        let ck = self.ctx_for(stack_id).unwrap_or(self.default_context);
        let ctx = self.ctx(ck);
        match self.get_program(program_id) {
            Some(program) => {
                crate::value::pending_report_json(&ctx.resources, program, ctx.frame())
            }
            None => serde_json::Value::Array(Vec::new()),
        }
    }

    /// Register a native function that can be called from Petal code,
    /// together with its declared [`NativeEffects`]: what it reads, whether it
    /// emits, whether it does something a replay could not reproduce, and its
    /// `Pending` policy. Must be called before `load_program`.
    ///
    /// The reactive layers take the row at its word — the memo classifies
    /// every call from it and never looks at the activity counters — so the
    /// row is required, not advisory. A native that reaches host state
    /// without saying so looks pure, and every memoized scope that calls it
    /// replays stale. `petal run --effect-audit` / `petal-ui-run
    /// --effect-audit` hold what a native was observed doing against its row.
    pub fn register_native(
        &mut self,
        name: &str,
        func: NativeFn,
        effects: NativeEffects,
    ) -> NativeFnId {
        self.native_fns.register(name, func, effects)
    }

    /// [`register_native`](Self::register_native) for a native that owns
    /// captured state — a closure instead of a bare `fn`. The embedder's
    /// userdata rides in the captures, so a host registering many natives at
    /// runtime (a C bridge forwarding each one to its own callback) needs no
    /// trampoline pool and no global dispatch table.
    ///
    /// Everything else is as for a bare native: call it before
    /// `load_program`; the effect row is required and means the same thing;
    /// [`set_native_class`](Self::set_native_class) and `--effect-audit`
    /// apply unchanged. Two things to keep in mind:
    ///
    /// - **Captured state is invisible to the runtime.** A closure that
    ///   mutates its captures (through a `Cell`/`RefCell`) must declare
    ///   `effect`, and one that answers from captured state the host changes
    ///   between frames must declare what it reads
    ///   ([`InputClasses::HOST_DATA`](crate::native_fn::InputClasses::HOST_DATA),
    ///   plus [`note_host_data_changed`](Self::note_host_data_changed) when it
    ///   moves) — the effect audit sees only what the call does through its
    ///   [`PetalCxt`](crate::native_fn::PetalCxt), not what it does to its
    ///   captures.
    /// - **Captures are per-`Env`, not per-execution.** A forked execution
    ///   (`fork_execution`, `run_speculative`) calls the same closure; a
    ///   native whose captures must not see speculative calls should keep its
    ///   state in the execution (bindings, counters, output buffers) instead.
    pub fn register_native_boxed(
        &mut self,
        name: &str,
        func: impl Fn(&mut crate::native_fn::PetalCxt) -> crate::native_fn::NativeResult + 'static,
        effects: NativeEffects,
    ) -> NativeFnId {
        self.native_fns.register_boxed(name, Box::new(func), effects)
    }

    /// Declare the parameter names of a native, by the id
    /// [`register_native`](Self::register_native) returned, so Petal code may
    /// call it with named arguments: after
    /// `declare_native_params(id, "x, y, w, h, alpha?")`, a script can write
    /// `fill(x: 0, y: 0, w: 10, h: 10)`. Call it before `load_program`.
    ///
    /// `spec` lists the names in the order the native reads its arguments; a
    /// trailing `?` marks an optional parameter, and optional ones must come
    /// last. The native itself does not change: named arguments are permuted
    /// into positional order before it runs, so it keeps reading by index. A
    /// native that takes several argument counts declares each form with its
    /// own call, and a named call uses the first form it fits. One that
    /// declares nothing keeps refusing names
    /// (`builtin 'f' does not accept named arguments`).
    ///
    /// Errors on a malformed spec (an empty, repeated or non-identifier name,
    /// or a required parameter after an optional one).
    pub fn declare_native_params(&mut self, id: NativeFnId, spec: &str) -> Result<(), String> {
        self.native_fns.declare_params(id, spec)
    }

    /// [`declare_native_params`](Self::declare_native_params) by the native's
    /// registered name. Errors when no native of that name is registered.
    pub fn declare_native_params_by_name(&mut self, name: &str, spec: &str) -> Result<(), String> {
        let id = self
            .native_fns
            .lookup_name(name)
            .ok_or_else(|| format!("no native named '{name}' is registered"))?;
        self.native_fns.declare_params(id, spec)
    }

    /// Give a native, by the id [`register_native`](Self::register_native)
    /// returned, exactly these parameter lists — the form
    /// [`declare_natives`](Self::declare_natives) takes for one native, for a
    /// host that reads its interface module once and applies it as it
    /// registers each native
    /// ([`NativeDecls::signatures_by_host`](crate::native_decl::NativeDecls::signatures_by_host)).
    pub fn set_native_signatures(
        &mut self,
        id: NativeFnId,
        signatures: Vec<crate::native_fn::NativeSignature>,
    ) {
        self.native_fns.set_signatures(id, signatures);
    }

    /// The parameter lists the native of this name declares. `None` when no
    /// such native is registered; empty when it takes arguments by position
    /// only.
    pub fn native_signatures(&self, name: &str) -> Option<&[crate::native_fn::NativeSignature]> {
        let id = self.native_fns.lookup_name(name)?;
        Some(self.native_fns.signatures(id))
    }

    /// The declared effect row of a native.
    pub fn native_effects(&self, id: NativeFnId) -> NativeEffects {
        self.native_fns.effects(id)
    }

    /// Whether a call to this native can be part of a static value
    /// ([`NativeConstness`](crate::native_fn::NativeConstness)): `Const` only
    /// for a native registered with
    /// [`NativeEffects::CONST`](crate::native_fn::NativeEffects::CONST),
    /// `ConstIfCallbacksAre` for the higher-order builtins, `Dynamic` for
    /// everything else.
    pub fn native_constness(&self, id: NativeFnId) -> crate::native_fn::NativeConstness {
        self.native_fns.constness(id)
    }

    /// [`native_constness`](Self::native_constness) by name. A name nothing
    /// registered — a `native fn` declared by an interface module whose host
    /// is not present, as under `petal check` — is `Dynamic`.
    pub fn native_constness_of_name(&self, name: &str) -> crate::native_fn::NativeConstness {
        self.native_fns.constness_of_name(name)
    }

    /// Classify every top-level declaration of `program` as static data, a
    /// function or dynamic, against this `Env`'s natives: the reason chain
    /// for a dynamic one, whether dynamic code reads a static one, and the
    /// declarations each depends on. Nothing is run and nothing changes. See
    /// [`crate::static_classify`]; `petal show-static` prints the result.
    ///
    /// `program` should be one this `Env` compiled (or one compiled against
    /// the same natives): a native this `Env` does not register is dynamic.
    pub fn static_report(&self, program: &Program) -> crate::static_classify::StaticReport {
        crate::static_classify::classify(program, &self.native_fns)
    }

    /// [`static_report`](Self::static_report), keeping the class of every
    /// term for a caller that walks the graph.
    pub fn static_analysis(&self, program: &Program) -> crate::static_classify::StaticAnalysis {
        crate::static_classify::analyze(program, &self.native_fns)
    }

    /// Override the [`NativeClass`] of an already-registered native (by the id
    /// [`register_native`](Self::register_native) returned). Registration stays
    /// append-only — indices are stable — so an embedding classifies its natives
    /// afterward. Mark side-effecting emitters (natives that only
    /// [`push_output`](crate::native_fn::PetalCxt::push_output)/
    /// [`emit`](crate::native_fn::PetalCxt::emit) into a buffer)
    /// [`Effectful`](NativeClass::Effectful) so a `Pending` argument makes the
    /// call a no-op instead of being absorbed as the result.
    pub fn set_native_class(&mut self, id: NativeFnId, class: NativeClass) {
        self.native_fns.set_class(id, class);
    }

    /// Register a class of host-owned foreign objects, returning its id.
    /// Unlike `register_native`, this may be called at any time — handle
    /// classes are not referenced by compiled programs.
    pub fn register_handle_class(&mut self, class: HandleClass) -> HandleClassId {
        let id = HandleClassId(self.handle_classes.len() as u16);
        self.handle_classes.push(class);
        id
    }

    /// Mint a `Value::Handle` for a (slot, serial) address in `class`.
    pub fn make_handle(&self, class: HandleClassId, slot: u32, serial: u32) -> Value {
        Value::Handle(HandleVal {
            class,
            slot,
            serial,
        })
    }

    /// The registered handle classes, indexed by `HandleClassId`.
    pub fn handle_classes(&self) -> &[HandleClass] {
        &self.handle_classes
    }

    // ── Heap access ──────────────────────────────────────────────

    pub fn heap(&self) -> &Heap {
        &self.ctx(self.default_context).heap
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        let ck = self.default_context;
        &mut self.ctx_mut(ck).heap
    }

    /// The default context's closures and overload sets — collected with the
    /// heap (see [`ClosureTable`](crate::closure_table::ClosureTable)). Host
    /// introspection: a per-frame embedder watches `closure_count()` to see
    /// that re-running a program is not accumulating closures.
    pub fn closures(&self) -> &crate::closure_table::ClosureTable {
        &self.ctx(self.default_context).closures
    }

    /// Heap of the context a specific stack is bound to. For a forked stack this
    /// is the fork's own heap — the one its state `Value` ids resolve against —
    /// not the default context's. Use this (not [`heap`](Self::heap)) to decode
    /// or diff a fork's objects. `None` if the stack is unknown.
    pub fn heap_for(&self, stack_id: StackKey) -> Option<&Heap> {
        self.ctx_for(stack_id).map(|ck| &self.ctx(ck).heap)
    }

    /// Mutable [`heap_for`](Self::heap_for): the heap of a specific stack's
    /// context, e.g. to allocate inputs directly into a fork before running it.
    pub fn heap_for_mut(&mut self, stack_id: StackKey) -> Option<&mut Heap> {
        let ck = self.ctx_for(stack_id)?;
        Some(&mut self.ctx_mut(ck).heap)
    }

    // ── Duplication statistics ───────────────────────────────────

    /// Value-duplication statistics for the default execution context. Counts
    /// copy-on-write duplications and fork copies; all zero unless collection
    /// is on ([`set_heap_stats`](Self::set_heap_stats)). See [`crate::stats`].
    pub fn dup_stats(&self) -> &DupStats {
        self.ctx(self.default_context).dup_stats()
    }

    /// Duplication statistics for the context a specific stack is bound to (its
    /// own fork's heap, for a forked stack). `None` if the stack is unknown.
    pub fn dup_stats_for(&self, stack_id: StackKey) -> Option<&DupStats> {
        self.ctx_for(stack_id).map(|ck| self.ctx(ck).dup_stats())
    }

    /// Heap-allocation statistics (objects created per kind) for the default
    /// execution context. See [`crate::stats`].
    pub fn alloc_stats(&self) -> &AllocStats {
        self.ctx(self.default_context).alloc_stats()
    }

    /// Allocation statistics for the context a specific stack is bound to.
    /// `None` if the stack is unknown.
    pub fn alloc_stats_for(&self, stack_id: StackKey) -> Option<&AllocStats> {
        self.ctx_for(stack_id).map(|ck| self.ctx(ck).alloc_stats())
    }

    // ── State inspection ─────────────────────────────────────────

    /// Get the current value of a single top-level state variable.
    /// For state reached through a call path, use `get_all_state` and filter by
    /// base key.
    ///
    /// # Contract: the returned `Value` is a snapshot, not a handle
    /// Read it — or copy what you need out of it — **before the next run of this
    /// stack**, and do not stash it. A container id in a state slot may be
    /// mutated *in place* by a later run: the escape analysis
    /// (`backend::bytecode::escape`, "the `state`-backed accumulator") proves the
    /// slot's id is the program's alone and lets a frame-loop simulation update
    /// its arrays without cloning them every write. A `Value` held across a run
    /// would observe those updates instead of the values it was handed.
    ///
    /// In-tree this holds by construction: the only non-test caller is
    /// [`Env::get_state_json`], which serializes immediately.
    pub fn get_state(&self, stack_id: StackKey, key: StateKey) -> Option<Value> {
        let stack = self.stacks.get(&stack_id)?;
        // Top-level state runs on the root frame, whose path is empty.
        let runtime_key = RuntimeStateKey {
            base: key,
            path: smallvec::SmallVec::new(),
        };
        stack.state.get(&runtime_key).copied()
    }

    /// Get all current state as a reference to the HashMap.
    ///
    /// The same snapshot contract as [`Env::get_state`] applies to every `Value`
    /// in the map: read it before the next run of this stack, and do not retain
    /// it — a later run may mutate a state container's backing store in place.
    pub fn get_all_state(&self, stack_id: StackKey) -> Option<&HashMap<RuntimeStateKey, Value>> {
        self.stacks.get(&stack_id).map(|s| &s.state)
    }

    /// Set a top-level state variable's value directly.
    ///
    /// The stack takes ownership of `value`: pass a container the caller does
    /// not otherwise hold or share with another slot, since a later run may
    /// mutate it in place (the counterpart of the [`Env::get_state`] contract).
    /// The in-tree caller, [`Env::set_state_from_json`], allocates a fresh value
    /// per call.
    pub fn set_state(&mut self, stack_id: StackKey, key: StateKey, value: Value) {
        let runtime_key = RuntimeStateKey {
            base: key,
            path: smallvec::SmallVec::new(),
        };
        // A `state var`'s slot holds a cell, and the script reads that cell —
        // replacing the slot outright would strand every `CellRead` compiled
        // against it. Write *through* the cell instead, which is also what the
        // caller means: they are setting the variable, not the box.
        let existing = self
            .stacks
            .get(&stack_id)
            .and_then(|s| s.state.get(&runtime_key).copied());
        // State edited from outside is a change the run's dependency record
        // cannot see: the next frame must run.
        self.invalidate_run(stack_id);
        if let Some(Value::Cell(cell)) = existing {
            let ck = self.ctx_for(stack_id).unwrap_or(self.default_context);
            self.ctx_mut(ck).heap.cell_write(cell, value);
            return;
        }
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.state.insert(runtime_key, value);
        }
    }

    // ── State key name resolution ────────────────────────────────

    /// Build a map from StateKey → variable name by scanning program terms.
    /// O(n) over terms, call once or on hot-reload.
    pub fn state_key_names(&self, program_id: ProgramId) -> HashMap<StateKey, String> {
        let mut map = HashMap::new();
        if let Some(program) = self.programs.get(&program_id) {
            for (sk, name) in program.state_terms() {
                if let Some(name) = name {
                    map.entry(sk).or_insert_with(|| name.clone());
                }
            }
        }
        map
    }

    // ── State snapshots ──────────────────────────────────────────

    /// Clone all state values. Cheap since Value is Copy.
    pub fn snapshot_state(&self, stack_id: StackKey) -> Option<HashMap<RuntimeStateKey, Value>> {
        self.stacks.get(&stack_id).map(|s| s.state.clone())
    }

    /// Restore state from a previous snapshot, replacing all current state.
    pub fn restore_state(&mut self, stack_id: StackKey, snapshot: HashMap<RuntimeStateKey, Value>) {
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.state = snapshot;
            stack.run_deps.force();
        }
    }

    // ── Internal accessors (used by transfer_state module) ─────────

    /// Get a shared reference to a stack.
    pub(crate) fn stack(&self, key: StackKey) -> Option<&Stack> {
        self.stacks.get(&key)
    }

    /// Get a mutable reference to a stack.
    pub(crate) fn stack_mut(&mut self, key: StackKey) -> Option<&mut Stack> {
        self.stacks.get_mut(&key)
    }

    /// Insert or replace a program. Replacing under the same id (hot reload /
    /// `transfer_state`) drops the cached bytecode lowering, which is derived
    /// from the program and would otherwise be served stale by `ensure_bytecode`.
    pub(crate) fn insert_program(&mut self, id: ProgramId, program: Program) {
        self.bytecode.remove(&id);
        self.static_audit.forget(id);
        self.programs.insert(id, program);
    }

    /// Clear all runtime closures and overload sets.
    pub(crate) fn clear_closures(&mut self) {
        let ck = self.default_context;
        let ctx = self.ctx_mut(ck);
        ctx.closures.clear();
    }
}

/// Counts of the expensive work an [`Env`] has done (see
/// [`Env::work_counters`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkCounters {
    /// Source compiles: parse, check and compile to the term graph.
    pub compiles: u64,
    /// Lowerings of a program to bytecode, optimizer passes included.
    pub lowerings: u64,
}

/// Outcome of a bounded run (see [`Env::run_bounded`]).
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// The program ran to completion within the step budget, producing `Value`.
    Done(Value),
    /// The step budget was exhausted before the program completed. `steps` is
    /// how many steps were consumed (equal to the budget). The stack is left
    /// runnable — call [`Env::run_bounded`] again to resume.
    Yielded { steps: u64 },
}

/// One state variable that differs between two executions (see
/// [`Env::diff_state`]). `source`/`fork` hold the variable's JSON value in each
/// run, or `None` where that run has no such variable.
#[derive(Debug, Clone, PartialEq)]
pub struct StateDiff {
    pub name: String,
    pub source: Option<serde_json::Value>,
    pub fork: Option<serde_json::Value>,
}

/// One source file a program was compiled from (see
/// [`Env::module_manifest`]). `origin` is the filesystem path when the module
/// was resolved from disk (`None` for in-memory registrations and inline
/// entry sources); `content_hash` is a stable hash of the source text so
/// hosts can detect real changes without re-reading files.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleManifestEntry {
    pub name: String,
    pub origin: Option<std::path::PathBuf>,
    pub content_hash: u64,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Env {
    /// Summary view: counts rather than full contents, so a host struct that
    /// embeds an `Env` can `#[derive(Debug)]` (needed by `unwrap_err` /
    /// `expect_err` in tests, and for logging). The heap, closures, and trace
    /// buffer are intentionally elided via `finish_non_exhaustive`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let default_ctx = self.contexts.get(&self.default_context);
        f.debug_struct("Env")
            .field("programs", &self.programs.len())
            .field("stacks", &self.stacks.len())
            .field("native_fns", &self.native_fns.count())
            .field("contexts", &self.contexts.len())
            .field(
                "closures",
                &default_ctx.map(|c| c.closures.closure_count()).unwrap_or(0),
            )
            .field(
                "pending_output_lines",
                &default_ctx.map(|c| c.output.len()).unwrap_or(0),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
