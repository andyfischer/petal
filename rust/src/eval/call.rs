//! Function calls: closures, overload sets, native functions, and the
//! higher-order intrinsics (map / filter / reduce / forEach) that need the
//! evaluator to call closures synchronously.

use crate::constant_table::ConstantId;
use crate::native_fn::{NativeFnId, PetalCxt};

use super::*;

impl<'a> Evaluator<'a> {
    pub(super) fn exec_call(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        let callable = inputs[0];
        let args = &inputs[1..];

        match callable {
            Value::Closure(_) | Value::OverloadSet(_) => {
                self.push_closure_call(callable, args, term)
            }
            Value::NativeFunction(native_id) => {
                self.call_native_or_intrinsic(native_id, args, term)
            }
            // Calling a fieldless enum variant yields the variant itself.
            Value::EnumVariant { .. } if args.is_empty() => self.produce(term, callable),
            _ => ControlFlow::Error(format!("Cannot call {}", callable.type_name())),
        }
    }

    /// Static builtin call `name(args...)` produced by the compiler when a bare,
    /// unshadowed builtin is called directly. `inputs` are the args (no callable).
    /// Routed through the same path as a dynamic Call so intrinsics (map/filter/
    /// reduce/forEach) still receive their closure arguments.
    pub(super) fn exec_builtin_call(
        &mut self,
        name_cid: ConstantId,
        term: &Term,
        inputs: &[Value],
    ) -> ControlFlow {
        let name = match self.program.get_string_constant(name_cid) {
            Some(s) => s.to_string(),
            None => return ControlFlow::Error("BuiltinCall: invalid name constant".into()),
        };
        let native_id = match self.native_fns.lookup_name(&name) {
            Some(id) => id,
            None => return ControlFlow::Error(format!("Unknown builtin: {}", name)),
        };
        self.call_native_or_intrinsic(native_id, inputs, term)
    }

    /// Method-call syntax `obj.method(args...)`. Resolution order:
    /// 1. a callable field on a record receiver,
    /// 2. a native function called with the receiver prepended to the args.
    pub(super) fn exec_method_call(
        &mut self,
        method_cid: ConstantId,
        term: &Term,
        inputs: &[Value],
    ) -> ControlFlow {
        let obj = inputs[0];
        let args = &inputs[1..];
        let method_name = match self.program.get_string_constant(method_cid) {
            Some(s) => s.to_string(),
            None => return ControlFlow::Error("Invalid method name".into()),
        };

        // 1) If obj is a record, check for a callable field first
        if let Value::Map(map_id) = obj {
            if let Some(&field_val) = self.heap.get_map(map_id).get(&method_name) {
                match field_val {
                    Value::Closure(_) | Value::OverloadSet(_) => {
                        return self.push_closure_call(field_val, args, term);
                    }
                    Value::NativeFunction(native_id) => {
                        return match self.call_native_fn(native_id, args) {
                            Ok(val) => self.produce(term, val),
                            Err(e) => ControlFlow::Error(e),
                        };
                    }
                    _ => {} // not callable, fall through to method lookup
                }
            }
        }

        // 2) Look up the method as a native function, with obj prepended to args
        if let Some(native_id) = self.native_fns.lookup_name(&method_name) {
            let mut full_args = vec![obj];
            full_args.extend_from_slice(args);
            self.call_native_or_intrinsic(native_id, &full_args, term)
        } else {
            let hint = match method_name.as_str() {
                "toString" => Some("use str() or the str() method instead"),
                "log" => Some("use print() instead of console.log()"),
                "indexOf" => Some("use contains() to check membership"),
                "concat" => Some("use the ++ operator to concatenate lists or strings"),
                _ => None,
            };
            let msg = match hint {
                Some(hint) => format!(
                    "No method '{}' on type {} — {}",
                    method_name,
                    obj.type_name(),
                    hint
                ),
                None => format!("No method '{}' on type {}", method_name, obj.type_name()),
            };
            ControlFlow::Error(msg)
        }
    }

    /// Build an overload-set value from per-arity closures, patching each
    /// closure's self-recursion capture (which was Nil at MakeClosure time
    /// because the set didn't exist yet).
    pub(super) fn exec_make_overload_set(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        let program = self.program;
        let mut entries = Vec::with_capacity(inputs.len());
        for &input in inputs {
            if let Value::Closure(cid) = input {
                let func =
                    &program.functions[self.closures[cid.0 as usize].function_id.0 as usize];
                entries.push(OverloadEntry {
                    arity: func.params.len(),
                    closure_id: cid,
                });
            }
        }
        let set_id = OverloadSetId(self.overload_sets.len() as u32);
        let overload_val = Value::OverloadSet(set_id);

        // Derive the base name from an internal name (e.g. "count#1" → "count"),
        // then patch every capture of that name to the overload set value.
        let base_name = entries.first().and_then(|e| {
            let func = &program.functions
                [self.closures[e.closure_id.0 as usize].function_id.0 as usize];
            func.name
                .as_ref()
                .and_then(|n| n.rfind('#').map(|pos| n[..pos].to_string()))
        });
        if let Some(ref base) = base_name {
            for entry in &entries {
                let closure = &mut self.closures[entry.closure_id.0 as usize];
                let func = &program.functions[closure.function_id.0 as usize];
                for (i, cap_name) in func.capture_names.iter().enumerate() {
                    if cap_name == base {
                        closure.captures[i] = overload_val;
                    }
                }
            }
        }

        self.overload_sets.push(entries);
        self.produce(term, overload_val)
    }

    // -----------------------------------------------------------------------
    // Closure calls
    // -----------------------------------------------------------------------

    /// Resolve `callable`, build its frame, advance the caller past the call
    /// term, and push the frame.
    fn push_closure_call(&mut self, callable: Value, args: &[Value], term: &Term) -> ControlFlow {
        let closure_id = match self.resolve_callable(callable, args.len()) {
            Ok(id) => id,
            Err(e) => return ControlFlow::Error(e),
        };
        match self.build_closure_frame(Value::Closure(closure_id), args, Some(term.id)) {
            Ok(frame) => {
                if let Some(caller_frame) = self.stack.frames.last_mut() {
                    caller_frame.current_term = term.block_next;
                }
                self.stack.push_frame(frame);
                ControlFlow::FramePushed
            }
            Err(e) => ControlFlow::Error(e),
        }
    }

    /// Resolve a callable to a ClosureId. Handles both plain closures and
    /// overload sets (picked by argument count).
    fn resolve_callable(&self, callable: Value, arg_count: usize) -> Result<ClosureId, String> {
        match callable {
            Value::Closure(id) => Ok(id),
            Value::OverloadSet(set_id) => {
                self.resolve_overload(&self.overload_sets[set_id.0 as usize], arg_count)
            }
            _ => Err(format!("Expected a function, got {}", callable.type_name())),
        }
    }

    /// Resolve an overload set to the correct closure based on argument count.
    fn resolve_overload(
        &self,
        entries: &[OverloadEntry],
        arg_count: usize,
    ) -> Result<ClosureId, String> {
        for entry in entries {
            if entry.arity == arg_count {
                return Ok(entry.closure_id);
            }
        }
        // Derive the base function name from the first entry's internal name
        // (e.g. "foo#2" → "foo")
        let base_name = entries
            .first()
            .and_then(|e| {
                let func = &self.program.functions
                    [self.closures[e.closure_id.0 as usize].function_id.0 as usize];
                func.name
                    .as_ref()
                    .and_then(|n| n.split('#').next().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| "<anonymous>".to_string());
        let arities: Vec<String> = entries.iter().map(|e| e.arity.to_string()).collect();
        Err(format!(
            "{}() expects {} arguments, got {}",
            base_name,
            arities.join(" or "),
            arg_count,
        ))
    }

    /// Build a Frame for calling a closure with the given arguments.
    /// Handles parameter binding, capture registers, and self-reference.
    fn build_closure_frame(
        &self,
        callable: Value,
        args: &[Value],
        return_term: Option<TermId>,
    ) -> Result<Frame, String> {
        let closure_id = match callable {
            Value::Closure(id) => id,
            _ => return Err(format!("Expected a function, got {}", callable.type_name())),
        };

        let closure = &self.closures[closure_id.0 as usize];
        let func = &self.program.functions[closure.function_id.0 as usize];
        let body_block = func.body_block;
        let block = self.program.get_block(body_block);

        if args.len() != func.params.len() {
            let name = func.name.as_deref().unwrap_or("<anonymous>");
            return Err(format!(
                "{}() expected {} argument{}, got {}",
                name,
                func.params.len(),
                if func.params.len() == 1 { "" } else { "s" },
                args.len()
            ));
        }

        let reg_count = block.register_count as usize;
        let mut registers = vec![Value::Nil; reg_count];

        // Set parameter registers
        for (i, arg) in args.iter().enumerate() {
            if i < registers.len() {
                registers[i] = *arg;
            }
        }

        // Set capture registers
        for (i, cap) in closure.captures.iter().enumerate() {
            if i < func.capture_registers.len() {
                let reg_idx = func.capture_registers[i].0 as usize;
                if reg_idx < registers.len() {
                    registers[reg_idx] = *cap;
                }
            }
        }

        // Self-reference for recursion
        if let Some(self_reg) = func.self_ref_register {
            let reg_idx = self_reg.0 as usize;
            if reg_idx < registers.len() {
                registers[reg_idx] = callable;
            }
        }

        let mut frame = Frame::new(body_block, block.entry, 0, return_term, None);
        frame.registers = registers;
        // Strip internal "#arity" suffix from overloaded function names for display
        frame.fn_name = func.name.as_ref().map(|n| match n.rfind('#') {
            Some(pos) => n[..pos].to_string(),
            None => n.clone(),
        });
        Ok(frame)
    }

    // -----------------------------------------------------------------------
    // Native function dispatch
    // -----------------------------------------------------------------------

    /// Call a native function (non-intrinsic) via PetalCxt, returning the
    /// result value.
    fn call_native_fn(&mut self, native_id: NativeFnId, args: &[Value]) -> Result<Value, String> {
        let func = self.native_fns.get_func(native_id);
        let mut cxt = PetalCxt::new(args, self.heap, self.output);
        let count = func(&mut cxt)?;
        let results = cxt.take_results();
        Ok(if count > 0 && !results.is_empty() {
            results[0]
        } else {
            Value::Nil
        })
    }

    /// Dispatch a native function call, handling the higher-order intrinsics
    /// specially since they need evaluator context to call closures.
    fn call_native_or_intrinsic(
        &mut self,
        native_id: NativeFnId,
        args: &[Value],
        term: &Term,
    ) -> ControlFlow {
        let nf = self.native_fns;
        let result = if nf.intrinsic_map == Some(native_id) {
            self.builtin_map(args)
        } else if nf.intrinsic_filter == Some(native_id) {
            self.builtin_filter(args)
        } else if nf.intrinsic_reduce == Some(native_id) {
            self.builtin_reduce(args)
        } else if nf.intrinsic_for_each == Some(native_id) {
            self.builtin_for_each(args)
        } else {
            self.call_native_fn(native_id, args)
        };

        match result {
            Ok(val) => self.produce(term, val),
            Err(e) => ControlFlow::Error(e),
        }
    }

    // -----------------------------------------------------------------------
    // Higher-order intrinsics
    // -----------------------------------------------------------------------

    /// Call a closure synchronously: push its frame, then run nested steps
    /// until that frame pops, returning the closure's result. Accepts a plain
    /// `Closure` or an `OverloadSet` (resolved by argument count), so it backs
    /// both the higher-order intrinsics and the host-facing
    /// `Env::call_function`.
    pub(crate) fn call_closure_sync(
        &mut self,
        callable: Value,
        call_args: &[Value],
    ) -> Result<Value, String> {
        let closure_id = self.resolve_callable(callable, call_args.len())?;
        let frame = self.build_closure_frame(Value::Closure(closure_id), call_args, None)?;
        let target_depth = self.stack.frames.len();
        self.stack.push_frame(frame);

        self.stack.last_pop_result = None;

        loop {
            if self.stack.frames.len() <= target_depth {
                // Frame was popped — retrieve the result
                return Ok(self.stack.last_pop_result.take().unwrap_or(Value::Nil));
            }
            match self.step() {
                StepResult::Continue => {}
                StepResult::Complete(v) => return Ok(v),
                StepResult::Error(e) => return Err(e),
            }
        }
    }

    fn builtin_map(&mut self, args: &[Value]) -> Result<Value, String> {
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

    fn builtin_filter(&mut self, args: &[Value]) -> Result<Value, String> {
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

    fn builtin_reduce(&mut self, args: &[Value]) -> Result<Value, String> {
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

    fn builtin_for_each(&mut self, args: &[Value]) -> Result<Value, String> {
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
