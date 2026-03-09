//! WASM bindings for the core Petal runtime.
//!
//! Provides a `PetalRuntime` struct exposed via wasm-bindgen, wrapping
//! the core `Env` with element-tree support for petal-web.

use std::cell::RefCell;

use wasm_bindgen::prelude::*;

use crate::env::Env;
use crate::native_fn::{NativeResult, PetalCxt};
use crate::program::ProgramId;
use crate::stack::StackKey;
use crate::value::value_to_json;

// ---------------------------------------------------------------------------
// Thread-local state — element tree (petal-web)
// ---------------------------------------------------------------------------

thread_local! {
    /// Auto-incrementing element ID counter, reset each render cycle.
    static NEXT_EID: RefCell<i64> = RefCell::new(1);
    /// The element ID that was clicked (0 = none).
    static CLICKED_ID: RefCell<i64> = RefCell::new(0);
}

// ---------------------------------------------------------------------------
// Native functions — element tree
// ---------------------------------------------------------------------------

fn native_next_id(state: &mut PetalCxt) -> NativeResult {
    let id = NEXT_EID.with(|c| {
        let mut val = c.borrow_mut();
        let id = *val;
        *val += 1;
        id
    });
    state.push_int(id);
    Ok(1)
}

fn native_clicked(state: &mut PetalCxt) -> NativeResult {
    let query_id = state.get_int(1)?;
    let clicked = CLICKED_ID.with(|c| *c.borrow());
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
    pub fn set_clicked_id(&self, id: i32) {
        CLICKED_ID.with(|c| *c.borrow_mut() = id as i64);
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
        let val = self
            .env
            .run(StackKey(stack_id))
            .map_err(|e| JsValue::from_str(&e))?;
        let json = value_to_json(&val, self.env.heap());
        Ok(json.to_string())
    }

    /// Reset a stack (preserving state) and re-run. Returns result as JSON.
    pub fn reset_and_run(&mut self, stack_id: u32) -> Result<String, JsValue> {
        // Reset the EID counter each frame
        NEXT_EID.with(|c| *c.borrow_mut() = 1);

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
