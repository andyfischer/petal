//! Higher-order intrinsics (`map`/`filter`/`reduce`/`forEach`) implemented in
//! the VM, plus [`Vm::call_closure_sync`] — the synchronous closure driver they
//! and the host (`Env::call_function`) share.
//!
//! Split out of `vm/mod.rs`; see that module for the [`Vm`] struct and the
//! core step loop.

use super::*;

use crate::backend::calls;

impl<'a> Vm<'a> {
    /// Call a closure synchronously: push its frame, step until it pops, and
    /// return its result. Used by the synchronous higher-order intrinsics and by
    /// `Env::call_function` (the host-facing "invoke one function" API). Works
    /// from any frame depth, including a fresh VM with no root frame.
    pub(crate) fn call_closure_sync(
        &mut self,
        callable: Value,
        call_args: &[Value],
    ) -> Result<Value, String> {
        let cid = calls::resolve_callable(
            self.program,
            self.closures,
            self.overload_sets,
            callable,
            call_args.len(),
        )?;
        let target_depth = self.stack.vm_frames.len();
        self.push_closure_frame(cid, call_args, None, None)?;
        self.stack.last_pop_result = None;

        loop {
            if self.stack.vm_frames.len() <= target_depth {
                return Ok(self.stack.last_pop_result.take().unwrap_or(Value::Nil));
            }
            match self.step() {
                StepResult::Continue => {}
                StepResult::Complete(v) => return Ok(v),
                StepResult::Error(e) => {
                    // `e` is already annotated at the closure's failing term. Flag
                    // the outer `step` (which will receive this via `?`) not to
                    // annotate it again at the intrinsic's call site.
                    self.error_already_annotated = true;
                    return Err(e);
                }
            }
        }
    }

    pub(super) fn builtin_map(&mut self, args: &[Value]) -> Result<Value, String> {
        let [list, func] = args else {
            return Err("map() expects 2 arguments (list, function)".into());
        };
        let Value::List(list_id) = *list else {
            return Err("map() expects a list as first argument".into());
        };
        let elements = self.heap.get_list(list_id).to_vec();
        let mut results = Vec::with_capacity(elements.len());
        for elem in elements {
            results.push(self.call_closure_sync(*func, &[elem])?);
        }
        Ok(Value::List(self.heap.alloc_list(results)))
    }

    pub(super) fn builtin_filter(&mut self, args: &[Value]) -> Result<Value, String> {
        let [list, func] = args else {
            return Err("filter() expects 2 arguments (list, function)".into());
        };
        let Value::List(list_id) = *list else {
            return Err("filter() expects a list as first argument".into());
        };
        let elements = self.heap.get_list(list_id).to_vec();
        let mut results = Vec::new();
        for elem in elements {
            if self.call_closure_sync(*func, &[elem])?.is_truthy() {
                results.push(elem);
            }
        }
        Ok(Value::List(self.heap.alloc_list(results)))
    }

    pub(super) fn builtin_reduce(&mut self, args: &[Value]) -> Result<Value, String> {
        let [list, initial, func] = args else {
            return Err("reduce() expects 3 arguments (list, initial, function)".into());
        };
        let Value::List(list_id) = *list else {
            return Err("reduce() expects a list as first argument".into());
        };
        let elements = self.heap.get_list(list_id).to_vec();
        let mut acc = *initial;
        for elem in elements {
            acc = self.call_closure_sync(*func, &[acc, elem])?;
        }
        Ok(acc)
    }

    pub(super) fn builtin_for_each(&mut self, args: &[Value]) -> Result<Value, String> {
        let [list, func] = args else {
            return Err("forEach() expects 2 arguments (list, function)".into());
        };
        let Value::List(list_id) = *list else {
            return Err("forEach() expects a list as first argument".into());
        };
        let elements = self.heap.get_list(list_id).to_vec();
        for elem in elements {
            self.call_closure_sync(*func, &[elem])?;
        }
        Ok(Value::Nil)
    }
}
