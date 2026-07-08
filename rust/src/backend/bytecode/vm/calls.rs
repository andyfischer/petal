//! User-function call and return handling: `Call`/`MethodCall` execution,
//! closure-frame push, return-value delivery, and root-frame completion.
//!
//! Split out of `vm/mod.rs`; see that module for the [`Vm`] struct and the
//! core step loop. Native-function and builtin dispatch (reached from
//! [`Vm::do_call`]/[`Vm::do_method_call`]) live in the sibling `native` file.

use super::*;

use crate::backend::calls;
use crate::program::ClosureId;

impl<'a> Vm<'a> {
    /// A frame ran off the end of its code without an explicit `Return`: its
    /// value is the entry block's last-term register (mirrors the graph
    /// engine's `block_result`). Pop it and deliver the value.
    pub(super) fn finish_frame(&mut self, func: &BytecodeFn) -> StepResult {
        let top = self.stack.vm_frames.len() - 1;
        let result = func
            .result_reg
            .map(|r| self.reg(top, r))
            .unwrap_or(Value::Nil);
        self.deliver_value(result)
    }

    /// Pop the current frame and deliver `value`: to the caller's `dst`
    /// register, or up as `StepResult::Complete` when the root frame finishes.
    pub(super) fn deliver_value(&mut self, value: Value) -> StepResult {
        let mut frame = self.stack.vm_frames.pop().unwrap();
        self.stack.last_pop_result = Some(value);
        let result = if self.stack.vm_frames.is_empty() {
            // The root frame just completed — capture top-level named functions
            // so `Env::call_function` can invoke them without a re-run.
            if frame.func.is_none() {
                self.capture_root_functions(&frame);
            }
            StepResult::Complete(value)
        } else {
            if let Some(dst) = frame.dst_in_caller {
                let caller = self.stack.vm_frames.len() - 1;
                self.set(caller, dst, value);
            }
            // Trace the call's result against the call-site term, so `explain`
            // can show the value of a term whose value came from a call (the
            // `Call`/`MethodCall` op itself is skipped in `step`).
            if self.trace.enabled {
                if let Some(call_site) = frame.call_site {
                    self.trace.push(call_site, &[], value);
                }
            }
            StepResult::Continue
        };
        if self.stack.vm_frame_pool.len() < FRAME_POOL_MAX {
            frame.recycle();
            self.stack.vm_frame_pool.push(frame);
        }
        result
    }

    /// Record top-level named `Closure`/`OverloadSet` bindings from the root
    /// frame into `stack.functions` (mirrors the graph engine).
    fn capture_root_functions(&mut self, frame: &VmFrame) {
        let root = self.program.root_block;
        let Some(term_ids) = self.program.block_terms.get(&root) else {
            return;
        };
        let mut captured = Vec::new();
        for &tid in term_ids {
            let term = self.program.get_term(tid);
            if let Some(name) = term.name.as_ref() {
                let val = frame
                    .regs
                    .get(term.register.0 as usize)
                    .copied()
                    .unwrap_or(Value::Nil);
                if matches!(val, Value::Closure(_) | Value::OverloadSet(_)) {
                    captured.push((name.clone(), val));
                }
            }
        }
        for (name, val) in captured {
            self.stack.functions.insert(name, val);
        }
    }

    /// Dispatch `callable(args...)`, writing the result into `dst` of frame `fi`
    /// (closures push a frame that writes `dst` on return; native/enum results
    /// are written immediately).
    pub(super) fn do_call(
        &mut self,
        fi: usize,
        dst: Reg,
        callable: Value,
        args: &[Value],
        call_site: Option<TermId>,
    ) -> Result<(), String> {
        match callable {
            Value::Closure(_) | Value::OverloadSet(_) => {
                let cid = calls::resolve_callable(
                    self.program,
                    self.closures,
                    self.overload_sets,
                    callable,
                    args.len(),
                )?;
                self.push_closure_frame(cid, args, Some(dst), call_site)?;
            }
            Value::NativeFunction(nid) => {
                let v = self.call_native_or_intrinsic(nid, args)?;
                self.set(fi, dst, v);
            }
            // Calling a fieldless enum variant yields the variant itself.
            Value::EnumVariant { .. } if args.is_empty() => self.set(fi, dst, callable),
            _ => return Err(format!("Cannot call {}", callable.type_name())),
        }
        Ok(())
    }

    /// Method-call syntax `recv.name(args...)`: a callable field on a record
    /// receiver, else the handle class's `call_method` on a handle receiver,
    /// else a native function with `recv` prepended to the args.
    pub(super) fn do_method_call(
        &mut self,
        fi: usize,
        dst: Reg,
        recv: Value,
        name_cid: crate::constant_table::ConstantId,
        args: &[Value],
        call_site: Option<TermId>,
    ) -> Result<(), String> {
        let program = self.program;
        let method_name = match program.get_string_constant(name_cid) {
            Some(s) => s,
            None => return Err("Invalid method name".into()),
        };

        // 1) Callable field on a record receiver.
        if let Value::Map(map_id) = recv {
            let field_val = self.heap.get_map(map_id).get(method_name).copied();
            if let Some(field_val) = field_val {
                match field_val {
                    Value::Closure(_) | Value::OverloadSet(_) => {
                        return self.do_call(fi, dst, field_val, args, call_site);
                    }
                    Value::NativeFunction(nid) => {
                        let v = self.call_native_fn(nid, args)?;
                        self.set(fi, dst, v);
                        return Ok(());
                    }
                    _ => {} // not callable — fall through to method lookup
                }
            }
        }

        // 2) Handle receiver: dispatch through the handle class's own method
        //    table. This runs before the native-table lookup so class methods
        //    win over same-named globals (e.g. the builtin `get`).
        if let Value::Handle(h) = recv {
            let v = self.call_handle_method(h, method_name, args)?;
            self.set(fi, dst, v);
            return Ok(());
        }

        // 3) Native function with `recv` prepended.
        if let Some(nid) = self.native_fns.lookup_name(method_name) {
            let mut full_args: SmallVec<[Value; 8]> = SmallVec::new();
            full_args.push(recv);
            full_args.extend_from_slice(args);
            let v = self.call_native_or_intrinsic(nid, &full_args)?;
            self.set(fi, dst, v);
            Ok(())
        } else {
            let hint = match method_name {
                "toString" => Some("use str() or the str() method instead"),
                "log" => Some("use print() instead of console.log()"),
                "indexOf" => Some("use contains() to check membership"),
                "concat" => Some("use the ++ operator to concatenate lists or strings"),
                _ => None,
            };
            Err(match hint {
                Some(hint) => format!(
                    "No method '{}' on type {} — {}",
                    method_name,
                    recv.type_name(),
                    hint
                ),
                None => format!("No method '{}' on type {}", method_name, recv.type_name()),
            })
        }
    }

    /// Push a closure activation record onto the frame stack. Mirrors the graph
    /// engine's `build_closure_frame`, but sizes and populates the *flat*
    /// register file using the lowered function's binding metadata.
    pub(super) fn push_closure_frame(
        &mut self,
        cid: ClosureId,
        args: &[Value],
        dst: Option<Reg>,
        call_site: Option<TermId>,
    ) -> Result<(), String> {
        let bc = self.bc;
        let program = self.program;
        let fn_id = self.closures[cid.0 as usize].function_id;

        let bcfn = bc.function(fn_id);
        let func = &program.functions[fn_id.0 as usize];
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

        let mut frame = self.frame_from_pool(Some(fn_id), bcfn.reg_count, dst, call_site);
        for (i, &preg) in bcfn.param_regs.iter().enumerate() {
            if let Some(slot) = frame.regs.get_mut(preg as usize) {
                *slot = args[i];
            }
        }
        // Reborrowed (not cloned) — the frame is local, so nothing conflicts.
        let captures = &self.closures[cid.0 as usize].captures;
        for (i, &creg) in bcfn.capture_regs.iter().enumerate() {
            if let (Some(slot), Some(cap)) = (frame.regs.get_mut(creg as usize), captures.get(i)) {
                *slot = *cap;
            }
        }
        if let Some(sreg) = bcfn.self_ref_reg {
            if let Some(slot) = frame.regs.get_mut(sreg as usize) {
                *slot = Value::Closure(cid);
            }
        }
        self.stack.vm_frames.push(frame);
        Ok(())
    }
}
