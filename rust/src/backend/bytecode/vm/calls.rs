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
                let v = self.call_native_or_intrinsic(nid, args, call_site)?;
                self.set(fi, dst, v);
            }
            // Calling a fieldless enum variant yields the variant itself.
            Value::EnumVariant { .. } if args.is_empty() => self.set(fi, dst, callable),
            _ => return Err(format!("Cannot call {}", callable.type_name())),
        }
        Ok(())
    }

    /// Method-call syntax `recv.name(args...)`. Resolution order, first match
    /// wins (docs/language-guide.md, Classes & Methods):
    ///
    /// 1. a **callable record field** — `r.f()` where `f` is a field holding a
    ///    function. Data beats declarations: a record that carries its own
    ///    behavior is the older feature, and a class instance is a record.
    /// 2. a **user-declared method** for the receiver's class —
    ///    `fn Rect.area(r: Rect)`, published into `stack.methods` when its
    ///    declaration ran. Ahead of the built-ins so a program can extend, or
    ///    override, a built-in class method.
    /// 3. a **built-in class method** — `Rect.center_x` and friends, registered
    ///    as natives under their qualified names.
    /// 4. a **handle method**, for a handle receiver.
    /// 5. a **global native** with `recv` prepended — `[1,2,3].len()`.
    ///
    /// Step 5 is where a class instance can go wrong. `P(1).get()` is almost
    /// always a call to a method that does not exist, but `get` is also a
    /// global builtin, so the fallback used to run it and report the builtin's
    /// own complaint (`get() expects 2 arguments`) — a message that never
    /// mentions the class and is actively misleading during a live edit, where
    /// deleting `fn P.get` makes the reload fail with the builtin's words. So
    /// when the receiver is a class instance and the fallback *fails*, the
    /// class-aware "No method 'get' on class P" is reported instead. The
    /// fallback keeps working when it works: `p.str()` and `p.keys()` are
    /// unchanged.
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
                        let v = self.call_native_fn(nid, args, call_site)?;
                        self.set(fi, dst, v);
                        return Ok(());
                    }
                    _ => {} // not callable — fall through to method lookup
                }
            }
        }

        // 2/3) The receiver's class, when it has one: a user-declared method
        //      first, then the class's built-in methods. Both are keyed by the
        //      class tag the instance carries, so an untagged record — and any
        //      non-record — skips straight past.
        if let Value::Map(map_id) = recv
            && let Some(class) = self.heap.map_class_name(map_id)
        {
            if let Some(func) = self
                .stack
                .methods
                .get(class)
                .and_then(|m| m.get(method_name))
                .copied()
            {
                return self.do_call(fi, dst, func, &with_receiver(recv, args), call_site);
            }
            if let Some(nid) = self.native_fns.lookup_class_method(class, method_name) {
                let v =
                    self.call_native_or_intrinsic(nid, &with_receiver(recv, args), call_site)?;
                self.set(fi, dst, v);
                return Ok(());
            }
        }

        // 4) Handle receiver: dispatch through the handle class's own method
        //    table. This runs before the native-table lookup so class methods
        //    win over same-named globals (e.g. the builtin `get`).
        if let Value::Handle(h) = recv {
            let v = self.call_handle_method(h, method_name, args, call_site)?;
            self.set(fi, dst, v);
            return Ok(());
        }

        // The receiver's class, for the diagnostics below.
        let class = match recv {
            Value::Map(id) => self.heap.map_class_name(id).map(str::to_string),
            _ => None,
        };

        // 5) Native function with `recv` prepended.
        if let Some(nid) = self.native_fns.lookup_name(method_name) {
            match self.call_native_or_intrinsic(nid, &with_receiver(recv, args), call_site) {
                Ok(v) => {
                    self.set(fi, dst, v);
                    Ok(())
                }
                // A class instance that reaches the global-native fallback and
                // fails there was asking for a method of its class, not for
                // the builtin of the same name.
                Err(e) => Err(match &class {
                    Some(class) => no_method(method_name, &format!("class {class}")),
                    None => e,
                }),
            }
        } else {
            // Name the *class* when the receiver has one: "no method 'nope' on
            // type record" is useless next to "on class Rect".
            let what = match &class {
                Some(class) => format!("class {class}"),
                None => match recv {
                    Value::Map(_) => "type record".to_string(),
                    _ => format!("type {}", recv.type_name()),
                },
            };
            Err(no_method(method_name, &what))
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
            // A method's receiver is a parameter the *call site* supplies:
            // `c.foo()` passes `c` even though the user wrote no arguments.
            // Counting it would report `C.foo() expects 2 arguments, got 1` at
            // a call that wrote none of either. Saturating because the count is
            // only a message: the prescan rejects a receiverless method, but
            // hand-written IR reaches here without passing through it.
            let hidden = usize::from(crate::classes::split_qualified_method_name(name).is_some());
            let want = func.params.len().saturating_sub(hidden);
            let got = args.len().saturating_sub(hidden);
            return Err(format!(
                "{}() expects {} argument{}, got {}",
                name,
                want,
                if want == 1 { "" } else { "s" },
                got
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

/// "No method 'x' on class C" — the one wording for an unresolved method call,
/// with the JS-habit hints attached. `what` is already spelled ("class Rect",
/// "type record", "type list").
fn no_method(method_name: &str, what: &str) -> String {
    let hint = match method_name {
        "toString" => Some("use str() or the str() method instead"),
        "log" => Some("use print() instead of console.log()"),
        "indexOf" => Some("use contains() to check membership"),
        "concat" => Some("use the ++ operator to concatenate lists or strings"),
        _ => None,
    };
    match hint {
        Some(hint) => format!("No method '{method_name}' on {what} — {hint}"),
        None => format!("No method '{method_name}' on {what}"),
    }
}

/// A method's real argument list: the receiver, then the written arguments.
/// Every dispatch path but the callable-field one passes the receiver
/// explicitly, since `r.f(a)` supplies it implicitly at the call site.
fn with_receiver(recv: Value, args: &[Value]) -> SmallVec<[Value; 8]> {
    let mut full: SmallVec<[Value; 8]> = SmallVec::with_capacity(args.len() + 1);
    full.push(recv);
    full.extend_from_slice(args);
    full
}
