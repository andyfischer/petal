//! WASM bindings for the core Petal runtime.
//!
//! Provides a `PetalRuntime` struct exposed via wasm-bindgen, wrapping
//! the core `Env` with element-tree support for petal-web.

use wasm_bindgen::prelude::*;

use crate::env::Env;
use crate::native_fn::{NativeResult, PetalCxt};
use crate::program::ProgramId;
use crate::stack::StackKey;
use crate::value::{value_to_json, Value};

// ---------------------------------------------------------------------------
// Env channel names — element tree (petal-web)
// ---------------------------------------------------------------------------

/// Per-render counter handing out element ids; reset to 1 each render cycle.
const ELEMENT_ID_COUNTER: &str = "element_id";
/// Host→script binding: the element id that was clicked (0 = none).
const CLICKED_ID_BINDING: &str = "clicked_id";

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
        env.register_native("next_id", native_next_id);
        env.register_native("clicked", native_clicked);

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

    /// Declare modules every loaded program imports implicitly (a host
    /// prelude). Pass a comma-separated list of registered module names.
    pub fn set_implicit_imports(&mut self, names: &str) {
        let list: Vec<&str> = names
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        self.env.set_implicit_imports(&list);
    }

    /// Compile source code and return a program ID.
    pub fn load_program(&mut self, source: &str) -> Result<u32, JsValue> {
        let pid = self.env.load_program(source).map_err(|e| JsValue::from_str(&e))?;
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
