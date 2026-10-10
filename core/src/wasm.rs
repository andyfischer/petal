//! WASM bindings for the core Petal runtime.
//!
//! Provides a `PetalRuntime` struct exposed via wasm-bindgen, wrapping
//! the core `Env` with element-tree support for petal-web.

use wasm_bindgen::prelude::*;

use crate::env::Env;
use crate::native_fn::{InputClasses, NativeEffects, NativeResult, PetalCxt};
use crate::program::ProgramId;
use crate::stack::StackKey;
use crate::value::{Value, value_to_json};

// ---------------------------------------------------------------------------
// Env channel names — element tree (petal-web)
// ---------------------------------------------------------------------------

/// Per-render counter handing out element ids; reset to 1 each render cycle.
const ELEMENT_ID_COUNTER: &str = "element_id";
/// Host→script binding: the element id that was clicked (0 = none).
const CLICKED_ID_BINDING: &str = "clicked_id";

/// The interface package of this host: `import web` binds the element-tree
/// natives. Its source lives in `core/packages/web` so the stock CLI can
/// check scripts against it (`petal check -I core/packages`).
pub const WEB_PACKAGE: &str = "web";
const WEB_INTERFACE: &str = include_str!("../packages/web/src/web.ptl");
const WEB_MANIFEST: &str = include_str!("../packages/web/petal.toml");

// ---------------------------------------------------------------------------
// Native functions — element tree
// ---------------------------------------------------------------------------

fn native_next_id(state: &mut PetalCxt) -> NativeResult {
    let sym = state.intern_symbol(ELEMENT_ID_COUNTER);
    let id = state.next_counter(sym);
    state.push_int(id as i64);
    Ok(1)
}

fn native_clicked(state: &mut PetalCxt) -> NativeResult {
    let query_id = state.get_int(1)?;
    let clicked = match state.binding_named(CLICKED_ID_BINDING) {
        Value::Int(n) => n,
        _ => 0,
    };
    state.push_bool(clicked == query_id);
    Ok(1)
}

// ---------------------------------------------------------------------------
// PetalRuntime — WASM-exported struct
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub struct PetalRuntime {
    env: Env,
    active_program: Option<ProgramId>,
    active_stack: Option<StackKey>,
}

#[wasm_bindgen]
impl PetalRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new() -> PetalRuntime {
        let mut env = Env::new();

        // Element-tree functions for petal-web
        env.register_native("next_id", native_next_id, NativeEffects::EFFECT);
        env.register_native(
            "clicked",
            native_clicked,
            NativeEffects::probe(InputClasses::POINTER),
        );

        // The interface package that declares them: scripts `import web`.
        // Its manifest rides along, so the interface is provided at the
        // package's version.
        env.register_package(
            WEB_PACKAGE,
            [
                (WEB_PACKAGE, WEB_INTERFACE),
                (crate::package::MANIFEST_FILE, WEB_MANIFEST),
            ],
        )
        .expect("the web interface package registers");
        env.provides_package(WEB_PACKAGE)
            .expect("the web interface has a version");

        PetalRuntime {
            env,
            active_program: None,
            active_stack: None,
        }
    }

    /// Set which element ID was clicked (call before re-running).
    pub fn set_clicked_id(&mut self, id: i32) {
        let sym = self.env.intern_symbol(CLICKED_ID_BINDING);
        self.env.set_binding(sym, Value::Int(id as i64));
    }

    /// Register an in-memory module: subsequently loaded programs can
    /// `import name`. This is the module path for wasm hosts, which have no
    /// filesystem — see docs/module-system.md.
    pub fn register_module(&mut self, name: &str, source: &str) {
        self.env.register_module(name, source);
    }

    /// Register a whole package from memory. `modules_json` is a JSON object
    /// of module name (package-relative: `menu`, `ui/menu`) to source text —
    /// the form `petal bundle` emits. The modules are then importable as
    /// `name/<module>`, and `import name` loads the module named like the
    /// package. A package that declares natives (`native fn`) is an
    /// interface package: its declarations give this runtime's registered
    /// natives their parameter names.
    pub fn register_package(&mut self, name: &str, modules_json: &str) -> Result<(), JsValue> {
        let modules: std::collections::BTreeMap<String, String> =
            serde_json::from_str(modules_json).map_err(|e| {
                JsValue::from_str(&format!(
                    "register_package: expected a JSON object of module name to source: {e}"
                ))
            })?;
        self.env
            .register_package(name, modules.iter())
            .map(|_| ())
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Check this runtime's registered natives against its interface
    /// packages (a comma-separated list of package names): every declared
    /// native is registered and every registered native is declared. Returns
    /// an empty string when they agree, else what differs.
    pub fn verify_interface(&self, packages: &str) -> String {
        let list: Vec<&str> = packages
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        match self.env.verify_interface(&list) {
            Ok(()) => String::new(),
            Err(mismatch) => mismatch.to_string(),
        }
    }

    /// Announce that this runtime provides the host interface `name` at
    /// `version` (`"0.4.0"`). A package whose `petal.toml` says
    /// `[requires] name = ">=…"` loads only in a runtime that provides it.
    /// Call it where the natives of that interface are registered.
    pub fn provides(&mut self, name: &str, version: &str) -> Result<(), JsValue> {
        self.env
            .provides(name, version)
            .map_err(|e| JsValue::from_str(&e))
    }

    /// The interfaces this runtime provides, as a JSON object of name to
    /// version. Always includes `core`.
    pub fn provided_interfaces(&self) -> String {
        let map: std::collections::BTreeMap<String, String> =
            self.env.provided_interfaces().into_iter().collect();
        serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
    }

    /// Compile-only mode: programs loaded afterwards are checked but not
    /// linked against this runtime's natives, so a script written for
    /// another host can be checked here.
    pub fn set_check_only(&mut self, on: bool) {
        self.env.set_check_only(on);
    }

    /// Compile source code and return a program ID.
    pub fn load_program(&mut self, source: &str) -> Result<u32, JsValue> {
        let pid = self
            .env
            .load_program(source)
            .map_err(|e| JsValue::from_str(&e))?;
        self.active_program = Some(pid);
        Ok(pid.0)
    }

    /// Create an execution stack for a program, returning a stack ID.
    pub fn create_stack(&mut self, program_id: u32) -> Result<u32, JsValue> {
        let sid = self
            .env
            .create_stack(ProgramId(program_id))
            .map_err(|e| JsValue::from_str(&e))?;
        self.active_stack = Some(sid);
        Ok(sid.0)
    }

    /// Run a stack to completion. Returns the result as JSON.
    pub fn run(&mut self, stack_id: u32) -> Result<String, JsValue> {
        // Each render cycle hands out element ids starting from 1.
        let eid = self.env.intern_symbol(ELEMENT_ID_COUNTER);
        self.env.reset_counter(eid, 1);

        let val = self
            .env
            .run(StackKey(stack_id))
            .map_err(|e| JsValue::from_str(&e))?;
        let json = value_to_json(&val, self.env.heap());
        Ok(json.to_string())
    }

    /// Reset a stack (preserving state) and re-run. Returns result as JSON.
    pub fn reset_and_run(&mut self, stack_id: u32) -> Result<String, JsValue> {
        self.env
            .reset_stack(StackKey(stack_id))
            .map_err(|e| JsValue::from_str(&e))?;
        self.run(stack_id)
    }

    /// Take all print output accumulated since the last call. Returns JSON array.
    pub fn take_output(&mut self) -> String {
        let output = self.env.take_output();
        serde_json::to_string(&output).unwrap_or_else(|_| "[]".to_string())
    }
}
