//! User-function call and return handling: `Call`/`MethodCall` execution,
//! closure-frame push, return-value delivery, and root-frame completion.
//!
//! Split out of `vm/mod.rs`; see that module for the [`Vm`] struct and the
//! core step loop. Native-function and builtin dispatch (reached from
//! [`Vm::do_call`]/[`Vm::do_method_call`]) live in the sibling `native` file.

use super::*;

use crate::backend::bytecode::isa::ArgNames;
use crate::backend::calls;
use crate::program::{ClosureId, base_fn_name};

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
        // A frame that opened a memo scope closes it as it pops, with the
        // value it is about to deliver.
        if self.stack.vm_frames.last().is_some_and(|f| f.memo_scope) {
            self.memo_close(value);
        }
        self.stack.last_pop_result = Some(value);
        let top = self.stack.vm_frames.len() - 1;
        // A call `petal bench` is measuring ends here (see `bench_enter`).
        if self.profile.bench.any_open() {
            let (stack, heap) = (&*self.stack, &*self.heap);
            self.profile
                .bench
                .leave(top, || bench_counters(stack, heap));
        }
        if top == 0 {
            // The root frame just completed — capture top-level named functions
            // so `Env::call_function` can invoke them without a re-run.
            let frame = self.stack.vm_frames.pop().unwrap();
            if frame.func.is_none() {
                self.capture_root_functions(&frame);
            }
            self.recycle_frame(frame);
            return StepResult::Complete(value);
        }
        // Read what the return needs, then hand the frame straight to the
        // pool rather than moving it through a local.
        let (dst_in_caller, call_site) = {
            let f = &self.stack.vm_frames[top];
            (f.dst_in_caller, f.call_site)
        };
        self.pop_frame_to_pool();
        if let Some(dst) = dst_in_caller {
            self.set(top - 1, dst, value);
        }
        // Trace the call's result against the call-site term, so `explain`
        // can show the value of a term whose value came from a call (the
        // `Call`/`MethodCall` op itself is skipped in `step`).
        if self.trace.enabled {
            if let Some(call_site) = call_site {
                self.trace.push(call_site, &[], value);
            }
        }
        // Likewise for observation: `let x = f()` binds its value here, not
        // in `step`, so without this the whole call-valued half of a
        // program's named terms would be missing.
        if let (Some(spec), Some(call_site)) = (self.spec.as_deref_mut(), call_site) {
            spec.note_watch(call_site, value, self.heap);
        }
        if self.observations.enabled {
            if let Some(call_site) = call_site {
                if self.is_observable(call_site) {
                    self.observations.record(call_site, value);
                    self.memo_note_observation(call_site, value);
                    self.stack.statics.note_observation(call_site, value);
                }
            }
        }
        StepResult::Continue
    }

    /// Tell the call bench (`petal bench`, off by default) that a frame for
    /// `fn_id` was just pushed at index `fi`. One branch when it is off.
    #[inline(always)]
    pub(super) fn bench_enter(&mut self, fn_id: FunctionId, fi: usize) {
        if self.profile.bench.enabled {
            let (stack, heap) = (&*self.stack, &*self.heap);
            self.profile
                .bench
                .enter(fn_id, fi, || bench_counters(stack, heap));
        }
    }

    /// Pop the top frame into the pool (emptied), or drop it if the pool is
    /// full.
    fn pop_frame_to_pool(&mut self) {
        let stack = &mut *self.stack;
        if stack.vm_frame_pool.len() < FRAME_POOL_MAX {
            if let Some(mut f) = stack.vm_frames.pop() {
                f.recycle();
                stack.vm_frame_pool.push(f);
            }
        } else {
            stack.vm_frames.pop();
        }
    }

    /// Return a finished frame to the pool, unless the pool is full.
    pub(super) fn recycle_frame(&mut self, mut frame: VmFrame) {
        if self.stack.vm_frame_pool.len() < FRAME_POOL_MAX {
            frame.recycle();
            self.stack.vm_frame_pool.push(frame);
        }
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
                // An overload variant's term is named `box#1` internally; the
                // set that joins the variants carries the source name, so
                // skipping the mangled ones keeps `stack.functions` — a public
                // map hosts enumerate and call through — free of names no one
                // wrote. (Variant names are also unqualified, so admitting them
                // would let a module's `box#1` collide with the entry file.)
                if name.contains('#') {
                    continue;
                }
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
    /// are written immediately). `memoize` is false for a call the lowering
    /// kept out of memoized scopes (see `Inst::Call::no_memo`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn do_call(
        &mut self,
        fi: usize,
        dst: Reg,
        callable: Value,
        args: &[Value],
        arg_names: &ArgNames,
        call_site: Option<TermId>,
        memoize: bool,
    ) -> Result<(), String> {
        // `f(...args)`: only a call that names an argument can be one, so the
        // all-positional call pays one length test for it.
        if !arg_names.is_empty() && self.is_spread_call(arg_names) {
            return self.do_spread_call(fi, dst, callable, args, call_site, memoize);
        }
        match callable {
            Value::Closure(_) | Value::OverloadSet(_) => {
                // Overload selection is by argument count first; the written
                // names (and which parameters have defaults) decide between
                // variants only when the count alone does not — see
                // `calls::resolve_overload`.
                let cid = match callable {
                    Value::Closure(cid) => cid,
                    _ if arg_names.is_empty() => calls::resolve_callable(
                        self.program,
                        self.closures,
                        callable,
                        args.len(),
                        &[],
                    )?,
                    _ => {
                        let names = arg_name_strs(self.program, arg_names)?;
                        calls::resolve_callable(
                            self.program,
                            self.closures,
                            callable,
                            args.len(),
                            &names,
                        )?
                    }
                };
                let site = self.site_of(call_site);
                self.push_closure_frame(cid, args, arg_names, Some(dst), call_site, site, memoize)?;
            }
            Value::NativeFunction(nid) => {
                let bound = self.bind_native_args(nid, args, arg_names)?;
                let args = bound.as_deref().unwrap_or(args);
                let v = self.call_native_or_intrinsic(nid, args, call_site)?;
                self.set_call_result(fi, dst, v, call_site);
            }
            // Calling a fieldless enum variant yields the variant itself.
            Value::EnumVariant { .. } if args.is_empty() => {
                self.set_call_result(fi, dst, callable, call_site)
            }
            // Calling a Pending absorbs: nothing runs and the result is the
            // Pending itself, as for a strict native given a Pending argument.
            Value::Pending(_) => self.set_call_result(fi, dst, callable, call_site),
            _ => return Err(format!("Cannot call {}", callable.type_name())),
        }
        Ok(())
    }

    /// Whether the class label this receiver carries means nothing in the
    /// program now running — either there is no label, or it names a class this
    /// program does not declare.
    ///
    /// Both are shapes a live edit produces. A record built before its class
    /// existed has no label; an instance built as `C` and kept across an edit
    /// that renamed `C` has one that no longer resolves. Anything else — a
    /// label naming a class that is really here — is a live value whose own
    /// class must win, which is what keeps polymorphism through a single
    /// binding working.
    fn receiver_label_is_stale(&self, recv: Value) -> bool {
        match recv {
            Value::Map(id) => match self.heap.map_class_name(id) {
                Some(label) => !self.program.class_names.contains(label),
                None => true,
            },
            _ => false,
        }
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
    /// Between 3 and 4 sits the `hint`: the class the *declaration* of the slot
    /// this receiver came from named, supplied by the compiler for receivers
    /// whose type it could not pin down (`crate::typecheck::MethodDispatch`).
    /// It is consulted only when steps 2–3 found nothing *and* the receiver's
    /// own label is meaningless in this program — it names no class here, or
    /// there is no label at all. That is what a live edit leaves behind: a
    /// value built by code that no longer exists, labelled `C` after `C` was
    /// renamed, or an untagged record from before the class existed. Since the
    /// label has already failed, deferring to what the code says cannot
    /// override a live class, so polymorphism through one binding is untouched.
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
        arg_names: &ArgNames,
        hint_cid: Option<crate::constant_table::ConstantId>,
        call_site: Option<TermId>,
    ) -> Result<(), String> {
        let program = self.program;
        let method_name = match program.get_string_constant(name_cid) {
            Some(s) => s,
            None => return Err("Invalid method name".into()),
        };

        // 1) Callable field on a record receiver, or on an enum variant (a
        //    payload field or constant data holding a function).
        let field_val = match recv {
            Value::Map(map_id) => self.heap.get_map(map_id).get(method_name).copied(),
            Value::EnumVariant { tag, data } => {
                self.memo_note_enum_read();
                let enums = &self.stack.enums;
                crate::backend::ops::enum_variant_field(program, self.heap, enums, tag, data, method_name)
            }
            _ => None,
        };
        {
            if let Some(field_val) = field_val {
                match field_val {
                    Value::Closure(_) | Value::OverloadSet(_) => {
                        return self.do_call(fi, dst, field_val, args, arg_names, call_site, true);
                    }
                    Value::NativeFunction(nid) => {
                        let bound = self.bind_native_args(nid, args, arg_names)?;
                        let args = bound.as_deref().unwrap_or(args);
                        let v = self.call_native_fn(nid, args, false, call_site)?;
                        self.set_call_result(fi, dst, v, call_site);
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
                return self.do_call(
                    fi,
                    dst,
                    func,
                    &with_receiver(recv, args),
                    &with_receiver_names(arg_names),
                    call_site,
                    true,
                );
            }
            if let Some(nid) = self.native_fns.lookup_class_method(class, method_name) {
                // The receiver is the native's first parameter, and takes
                // the leading positional slot.
                let full = with_receiver(recv, args);
                let bound =
                    self.bind_native_args(nid, &full, &with_receiver_names(arg_names))?;
                let full = bound.as_deref().unwrap_or(&full);
                let v = self.call_native_or_intrinsic(nid, full, call_site)?;
                self.set_call_result(fi, dst, v, call_site);
                return Ok(());
            }
        }

        // 3.5) The declaration's class, when the receiver's own label cannot
        //      answer. Only reached if 2/3 found nothing, and only when the
        //      label is meaningless *here* — absent, or naming a class this
        //      program does not declare. A live-edited value is exactly that.
        if let Some(hint_cid) = hint_cid
            && let Some(hint) = program.get_string_constant(hint_cid)
            && self.receiver_label_is_stale(recv)
        {
            if let Some(func) = self
                .stack
                .methods
                .get(hint)
                .and_then(|m| m.get(method_name))
                .copied()
            {
                return self.do_call(
                    fi,
                    dst,
                    func,
                    &with_receiver(recv, args),
                    &with_receiver_names(arg_names),
                    call_site,
                    true,
                );
            }
            if let Some(nid) = self.native_fns.lookup_class_method(hint, method_name) {
                // The receiver is the native's first parameter, and takes
                // the leading positional slot.
                let full = with_receiver(recv, args);
                let bound =
                    self.bind_native_args(nid, &full, &with_receiver_names(arg_names))?;
                let full = bound.as_deref().unwrap_or(&full);
                let v = self.call_native_or_intrinsic(nid, full, call_site)?;
                self.set_call_result(fi, dst, v, call_site);
                return Ok(());
            }
        }

        // 4) Handle receiver: dispatch through the handle class's own method
        //    table. This runs before the native-table lookup so class methods
        //    win over same-named globals (e.g. the builtin `get`).
        if let Value::Handle(h) = recv {
            // A handle class dispatches its methods by name inside one host
            // callback; it has no per-method parameter list to bind against.
            let spread;
            let args = if arg_names.is_empty() {
                args
            } else if self.is_spread_call(arg_names) {
                // Only a list has anything to give a method with no declared
                // parameter names.
                spread = spread_positional(self.heap, args)
                    .ok_or_else(|| crate::native_fn::undeclared_named_args(method_name))??;
                &spread
            } else {
                return Err(crate::native_fn::undeclared_named_args(method_name));
            };
            let v = self.call_handle_method(h, method_name, args, call_site)?;
            self.set_call_result(fi, dst, v, call_site);
            return Ok(());
        }

        // The receiver's class, for the diagnostics below.
        let class = match recv {
            Value::Map(id) => self.heap.map_class_name(id).map(str::to_string),
            _ => None,
        };

        // 5) Native function with `recv` prepended: a `core` builtin, or a
        //    host native the program declares where a method call of that
        //    name is written (`Program::method_natives`). An undeclared
        //    native is not reachable this way either.
        //    The declaration must be in scope in the file the call is
        //    written in, exactly as for a bare call.
        let reachable = crate::native_decl::core_host_names().contains(method_name)
            || program.method_natives.as_ref().is_none_or(|declared| {
                declared.get(method_name).is_some_and(|files| {
                    match call_site.and_then(|t| program.source_map.get(t)) {
                        Some(span) => files.contains(&span.file),
                        None => true,
                    }
                })
            });
        if reachable && let Some(nid) = self.native_fns.lookup_name(method_name) {
            // `xs.slice(start: 1, end: 3)`: the receiver fills the builtin's
            // first parameter, and the names bind against the rest.
            let full = with_receiver(recv, args);
            let called = self
                .bind_native_args(nid, &full, &with_receiver_names(arg_names))
                .and_then(|bound| {
                    let full = bound.as_deref().unwrap_or(&full);
                    self.call_native_or_intrinsic(nid, full, call_site)
                });
            match called {
                Ok(v) => {
                    self.set_call_result(fi, dst, v, call_site);
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
        } else if let Value::Pending(_) = recv {
            // A method no builtin answers, called on a Pending receiver (a
            // record still loading whose field would hold the function):
            // absorb, as the field read `u.handler` and the builtin methods do.
            self.set_call_result(fi, dst, recv, call_site);
            Ok(())
        } else {
            // Name the *class* when the receiver has one: "no method 'nope' on
            // type record" is useless next to "on class Rect".
            let what = match &class {
                Some(class) => format!("class {class}"),
                None => match recv {
                    Value::Map(_) => "type record".to_string(),
                    Value::EnumVariant { tag, .. } => {
                        let tag = self.heap.get_string(tag);
                        match crate::program::split_enum_tag(tag).0 {
                            Some(enum_name) => format!("enum {enum_name}"),
                            None => "type enum".to_string(),
                        }
                    }
                    _ => format!("type {}", recv.type_name()),
                },
            };
            let mut message = no_method(method_name, &what);
            // A domain builtin of this name exists and the file has not
            // imported its package: say so, unless the receiver is a class
            // instance (then it was asking for a method of its class).
            if class.is_none()
                && let Some(package) = crate::native_decl::builtin_package_of(method_name)
            {
                message.push_str(&format!(
                    " — `{method_name}` is a builtin of the `{package}` package: \
                     add `import {package}` to call it as a method"
                ));
            }
            Err(message)
        }
    }

    /// Push a closure activation record onto the frame stack. Mirrors the graph
    /// engine's `build_closure_frame`, but sizes and populates the *flat*
    /// register file using the lowered function's binding metadata.
    ///
    /// `site` is the callsite id pushed onto the new frame's state path — the
    /// compile-time hash of the callee's canonical text at this call site, which
    /// is what gives each callsite of a function its own `state` slots.
    #[allow(clippy::too_many_arguments)]
    /// Write the result of a call that finished without pushing a frame — a
    /// native, a builtin method, a handle method, a fieldless variant — and
    /// record it against the call site.
    ///
    /// `step` cannot do this: `Inst::dst()` is `None` for `Call` and
    /// `MethodCall`, because a closure call fills `dst` only when its frame
    /// returns, and [`deliver_value`](Self::deliver_value) records it there.
    /// Without this, every named binding whose value came from a native reached
    /// through a value or a method (`let r = rect(…)` with `rect = Rect`,
    /// `let n = xs.len()`, `let cx = r.center_x()`) was missing from the
    /// observation buffer and the trace.
    fn set_call_result(&mut self, fi: usize, dst: Reg, value: Value, call_site: Option<TermId>) {
        self.set(fi, dst, value);
        let Some(call_site) = call_site else { return };
        if self.trace.enabled {
            self.trace.push(call_site, &[], value);
        }
        if self.observations.enabled && self.is_observable(call_site) {
            self.observations.record(call_site, value);
            self.memo_note_observation(call_site, value);
            self.stack.statics.note_observation(call_site, value);
        }
    }

    pub(super) fn push_closure_frame(
        &mut self,
        cid: ClosureId,
        args: &[Value],
        arg_names: &ArgNames,
        dst: Option<Reg>,
        call_site: Option<TermId>,
        site: u64,
        memoize: bool,
    ) -> Result<(), String> {
        self.push_closure_frame_with(
            cid,
            args,
            CallNames::Constants(arg_names),
            dst,
            call_site,
            site,
            memoize,
        )
    }

    /// [`push_closure_frame`](Self::push_closure_frame), with the argument
    /// names either as the call instruction carries them or as strings: a
    /// spread call's names are the keys of a record, which are no constants
    /// of the program.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn push_closure_frame_with(
        &mut self,
        cid: ClosureId,
        args: &[Value],
        arg_names: CallNames<'_, '_>,
        dst: Option<Reg>,
        call_site: Option<TermId>,
        site: u64,
        memoize: bool,
    ) -> Result<(), String> {
        // Every Petal call, callbacks included, pushes its frame here, so
        // this one check bounds recursion of every shape.
        if self.stack.vm_frames.len() >= super::MAX_CALL_DEPTH {
            return Err(super::stack_overflow_message(super::MAX_CALL_DEPTH));
        }
        let program = self.program;
        let fn_id = self.closures.closure(cid).function_id;

        let func = &program.functions[fn_id.0 as usize];
        // An overload variant is named `box#1` internally; every message below
        // names the function as the source wrote it, like `resolve_overload`.
        // (`split_qualified_method_name` still reads the stripped name — the
        // `Class.method` prefix is untouched by the suffix.)
        let fn_name = base_fn_name(func.name.as_deref().unwrap_or("<anonymous>"));
        // A parameter with a default value may be left out, so a function that
        // has any accepts a range of counts; too *few* for the required ones
        // is reported below, by name, when the arguments are bound.
        let optional = func.optional_params as usize;
        // A record spread that leaves a required parameter out is told which
        // one, by the binder below: the count alone would not say.
        let names_a_few = args.len() < func.params.len()
            && matches!(arg_names, CallNames::Strings(names) if !names.is_empty());
        if args.len() != func.params.len()
            && (optional == 0 || args.len() > func.params.len())
            && !names_a_few
        {
            // A method's receiver is a parameter the *call site* supplies:
            // `c.foo()` passes `c` even though the user wrote no arguments.
            // Counting it would report `C.foo() expects 2 arguments, got 1` at
            // a call that wrote none of either. Saturating because the count is
            // only a message: the prescan rejects a receiverless method, but
            // hand-written IR reaches here without passing through it.
            let hidden =
                usize::from(crate::classes::split_qualified_method_name(fn_name).is_some());
            let want = func.params.len().saturating_sub(hidden);
            let got = args.len().saturating_sub(hidden);
            return Err(format!(
                "{}() expects {} argument{}, got {}",
                fn_name,
                calls::arity_range(func.required_params().saturating_sub(hidden), want),
                if want == 1 { "" } else { "s" },
                got
            ));
        }

        // Named arguments are permuted into parameter order here: the one
        // place the callee's `params` sits next to the caller's `args`. An
        // empty `arg_names` — every call that writes no name — to a function
        // without defaults reuses `args` untouched, so the common path
        // allocates nothing. A function *with* defaults always goes through
        // the binder, which appends the was-it-passed flag its prologue reads
        // for each optional parameter (inline storage; still no allocation
        // for up to eight values).
        let bound;
        let args = if arg_names.is_empty() && optional == 0 {
            args
        } else {
            bound = match arg_names {
                CallNames::Constants(ids) => {
                    let names = arg_name_strs(program, ids)?;
                    calls::bind_named_args(fn_name, &func.params, optional, args, &names)?
                }
                CallNames::Strings(names) => {
                    calls::bind_named_args(fn_name, &func.params, optional, args, names)?
                }
            };
            &bound[..]
        };

        // A call with a destination register is a memo scope: replay it if
        // its record is still good, else run it and record it. Calls driven
        // synchronously (intrinsics, the host), and calls whose result the
        // caller mutates in place (`memoize` false), are not scopes; their
        // reads land in whatever scope encloses them.
        // A site whose records keep dying unreplayed stops being recorded:
        // the recording is all cost there (`MemoTable::site_records`).
        let scope = self.memo && memoize && dst.is_some() && !self.stack.memo.poisoned;
        let scope = scope && {
            let records = self.stack.memo.site_records(fn_id, site);
            if !records {
                self.stack.memo.stats.cold += 1;
            }
            records
        };
        // A site whose scopes are always folded as too small opens none.
        let scope = scope && {
            let tiny = self.stack.memo.site_is_tiny(fn_id, site);
            if tiny {
                self.stack.memo.stats.tiny += 1;
            }
            !tiny
        };
        // Nor does a call nested too deep: a record costs its path's length
        // to make and keep, so recursion that opened a scope per level paid
        // quadratically in depth (`MAX_SCOPE_FRAME_DEPTH`). Tested before the
        // path is built, which is itself a walk of the frames.
        let mut scope =
            scope && self.stack.vm_frames.len() <= crate::memo::MAX_SCOPE_FRAME_DEPTH;
        if scope {
            // The scope's path: the callee frame's, which is the caller's
            // full path plus this call's part (see `push_frame`).
            let caller = self.stack.vm_frames.len() - 1;
            let mut path = FramePath::new();
            self.full_path_into(caller, &mut path);
            path.push(crate::stack::PathPart::Call(site));
            // A full table records nothing more this run, so a call with no
            // record to replay has nothing to gain from a scope: it would be
            // opened, tracked and folded into its parent on the way out.
            if !self.stack.memo.has_room() && !self.stack.memo.contains(&path) {
                self.stack.memo.stats.inlined += 1;
                scope = false;
            } else if let Some(value) = self.memo_try(&path, fn_id, cid, args) {
                self.profile.bench.note_replay(fn_id);
                if let Some(dst) = dst {
                    self.set(caller, dst, value);
                }
                if self.observations.enabled
                    && let Some(call_site) = call_site
                    && self.is_observable(call_site)
                {
                    self.observations.record(call_site, value);
                    self.memo_note_observation(call_site, value);
                    self.stack.statics.note_observation(call_site, value);
                }
                return Ok(());
            }
        }
        self.enter_closure_frame(cid, fn_id, args, dst, call_site, site, scope);
        Ok(())
    }

    /// A host call with some arguments given by name
    /// (`Env::call_function_named`): [`push_closure_frame`](Self::push_closure_frame)
    /// for a synchronous call — no destination register, no memo scope — with
    /// the names as strings, since a host's names are not constants of the
    /// program. The checks and the binder are the same ones.
    pub(super) fn push_named_frame(
        &mut self,
        cid: ClosureId,
        args: &[Value],
        names: &[Option<&str>],
        site: u64,
    ) -> Result<(), String> {
        if self.stack.vm_frames.len() >= super::MAX_CALL_DEPTH {
            return Err(super::stack_overflow_message(super::MAX_CALL_DEPTH));
        }
        let fn_id = self.closures.closure(cid).function_id;
        let func = &self.program.functions[fn_id.0 as usize];
        let fn_name = base_fn_name(func.name.as_deref().unwrap_or("<anonymous>"));
        let optional = func.optional_params as usize;
        if args.len() > func.params.len() {
            return Err(format!(
                "{}() expects {} argument{}, got {}",
                fn_name,
                calls::arity_range(func.required_params(), func.params.len()),
                if func.params.len() == 1 { "" } else { "s" },
                args.len()
            ));
        }
        let bound = calls::bind_named_args(fn_name, &func.params, optional, args, names)?;
        self.enter_closure_frame(cid, fn_id, &bound, None, None, site, false);
        Ok(())
    }

    /// Push the frame of closure `cid` with `args` already laid out as its
    /// frame takes them (see `calls::bind_named_args`), and open its memo
    /// scope when `scope` is set.
    #[inline(always)]
    fn enter_closure_frame(
        &mut self,
        cid: ClosureId,
        fn_id: FunctionId,
        args: &[Value],
        dst: Option<Reg>,
        call_site: Option<TermId>,
        site: u64,
        scope: bool,
    ) {
        let bcfn = self.bc.function(fn_id);
        self.profile.record_call();
        let fi = self.push_frame(Some(fn_id), bcfn.reg_count, dst, call_site, Some(site));
        let frame = &mut self.stack.vm_frames[fi];
        for (i, &preg) in bcfn.param_regs.iter().enumerate() {
            if let Some(slot) = frame.regs.get_mut(preg as usize) {
                *slot = args[i];
            }
        }
        let captures = &self.closures.closure(cid).captures;
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
        frame.memo_scope = scope;
        if scope {
            self.memo_open(fn_id, cid, site, args, None);
        }
        // Last, so argument binding and the memo bookkeeping stay outside the
        // interval a benched call is timed over.
        self.bench_enter(fn_id, fi);
    }
}

/// The cumulative counters a benched call is measured against, read at its
/// push and its pop (see [`crate::profile::CallBench`]).
fn bench_counters(stack: &Stack, heap: &Heap) -> crate::profile::CallCounters {
    let dups = heap.dup_stats();
    // A fork is not a copy the script asked for; the per-call "copies" are
    // the copy-on-write ones.
    let fork = dups.get(crate::stats::DupKind::Fork);
    crate::profile::CallCounters {
        insts: stack.insts,
        allocs: heap.alloc_stats().total(),
        copies: dups.total_count() - fork.count,
        copy_bytes: dups.total_bytes() - fork.bytes,
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

/// The written argument names of a call, as strings (`None` = positional).
/// Empty for a call that names nothing.
fn arg_name_strs<'p>(
    program: &'p crate::program::Program,
    arg_names: &ArgNames,
) -> Result<SmallVec<[Option<&'p str>; 4]>, String> {
    let mut names: SmallVec<[Option<&str>; 4]> = SmallVec::with_capacity(arg_names.len());
    for cid in arg_names {
        names.push(match cid {
            Some(cid) => match program.get_string_constant(*cid) {
                Some(s) => Some(s),
                None => return Err("Invalid argument name".into()),
            },
            None => None,
        });
    }
    Ok(names)
}

/// The argument names for a method's real argument list: a leading `None` for
/// the receiver, then the written names. Kept aligned with [`with_receiver`].
/// An unnamed call stays empty so the callee takes the fast path.
fn with_receiver_names(arg_names: &ArgNames) -> ArgNames {
    if arg_names.is_empty() {
        return ArgNames::new();
    }
    let mut full = ArgNames::with_capacity(arg_names.len() + 1);
    full.push(None);
    full.extend_from_slice(arg_names);
    full
}

impl<'a> Vm<'a> {
    /// Named arguments to a native, permuted into the positional order the
    /// native reads them in — `None` when the call names nothing, which is
    /// every call on the hot path: it returns before touching the table and
    /// the caller passes its own `args` straight through.
    ///
    /// The names bind against the parameter lists the native declared
    /// ([`NativeFnTable::declare_params`](crate::native_fn::NativeFnTable::declare_params)),
    /// with the errors a Petal `fn` gives for the same slips. A native that
    /// declared none refuses names rather than guessing at them. Every
    /// dispatch path that reaches a native with caller-written names goes
    /// through here: `BuiltinCall`, a native held in a value or a record
    /// field, a built-in class method, and the method-syntax fallback.
    pub(super) fn bind_native_args(
        &self,
        nid: NativeFnId,
        args: &[Value],
        arg_names: &ArgNames,
    ) -> Result<Option<SmallVec<[Value; 8]>>, String> {
        if arg_names.is_empty() {
            return Ok(None);
        }
        if self.is_spread_call(arg_names) {
            return self.bind_native_spread(nid, args).map(Some);
        }
        let name = self.native_fns.get_name(nid);
        let sigs = self.native_signatures(nid);
        if sigs.is_empty() {
            return Err(crate::native_fn::undeclared_named_args(name));
        }
        let program = self.program;
        let mut names: SmallVec<[Option<&str>; 4]> = SmallVec::with_capacity(arg_names.len());
        for cid in arg_names {
            names.push(match cid {
                Some(cid) => match program.get_string_constant(*cid) {
                    Some(s) => Some(s),
                    None => return Err("Invalid argument name".into()),
                },
                None => None,
            });
        }
        crate::native_fn::bind_native_args(name, sigs, args, &names).map(Some)
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

/// The written names of a call's arguments, in whichever form the caller has
/// them (see `Vm::push_closure_frame_with`).
#[derive(Clone, Copy)]
enum CallNames<'a, 's> {
    /// As a call instruction carries them: constants of the program.
    Constants(&'a ArgNames),
    /// As strings, parallel to the arguments (`None` = positional) or empty.
    Strings(&'a [Option<&'s str>]),
}

impl CallNames<'_, '_> {
    fn is_empty(&self) -> bool {
        match self {
            CallNames::Constants(ids) => ids.is_empty(),
            CallNames::Strings(names) => names.is_empty(),
        }
    }
}

impl<'a> Vm<'a> {
    /// Whether a call's argument names say its last argument is a spread
    /// (`f(...args)`): the compiler names that argument
    /// [`SPREAD_ARG`](crate::ast::SPREAD_ARG), which no parameter can be
    /// called.
    fn is_spread_call(&self, arg_names: &ArgNames) -> bool {
        match arg_names.last() {
            Some(Some(cid)) => {
                self.program.get_string_constant(*cid) == Some(crate::ast::SPREAD_ARG)
            }
            _ => false,
        }
    }

    /// `callable(a, b, ...spread)`: `args` is the written positional
    /// arguments followed by the value spread.
    ///
    /// - A **list** supplies further positional arguments, and the call is
    ///   then exactly the call that wrote them out.
    /// - A **record** supplies arguments by parameter name, bound by the
    ///   binder a written `f(name: value)` uses: a parameter with a default
    ///   may be left out, and a key that names no parameter, a parameter also
    ///   filled by position, or a required one left out is that binder's
    ///   error.
    ///
    /// Anything else is an error. A `Pending` spread is absorbed: the call
    /// does not happen and its value is that pending, as for a native handed
    /// one.
    fn do_spread_call(
        &mut self,
        fi: usize,
        dst: Reg,
        callable: Value,
        args: &[Value],
        call_site: Option<TermId>,
        memoize: bool,
    ) -> Result<(), String> {
        let Some((&spread, lead)) = args.split_last() else {
            return Err("internal error: a spread call with no arguments".into());
        };
        match spread {
            Value::List(id) => {
                let mut full: SmallVec<[Value; 8]> = SmallVec::from_slice(lead);
                full.extend_from_slice(self.heap.get_list(id));
                self.do_call(
                    fi,
                    dst,
                    callable,
                    &full,
                    &ArgNames::new(),
                    call_site,
                    memoize,
                )
            }
            Value::Map(id) => {
                let record = self.heap.get_map(id);
                let keys: SmallVec<[crate::record::Key; 8]> = record.keys().cloned().collect();
                let mut full: SmallVec<[Value; 8]> = SmallVec::from_slice(lead);
                full.extend(record.values().copied());
                let mut names: SmallVec<[Option<&str>; 8]> =
                    smallvec::smallvec![None; lead.len()];
                names.extend(keys.iter().map(|k| Some(k.as_str())));
                // A record with no fields names nothing: the call is the
                // positional one.
                let names: &[Option<&str>] = if keys.is_empty() { &[] } else { &names };
                match callable {
                    Value::Closure(_) | Value::OverloadSet(_) => {
                        let cid = calls::resolve_callable(
                            self.program,
                            self.closures,
                            callable,
                            full.len(),
                            names,
                        )?;
                        let site = self.site_of(call_site);
                        self.push_closure_frame_with(
                            cid,
                            &full,
                            CallNames::Strings(names),
                            Some(dst),
                            call_site,
                            site,
                            memoize,
                        )
                    }
                    Value::NativeFunction(nid) => {
                        let bound = self.bind_native_spread(nid, args)?;
                        let v = self.call_native_or_intrinsic(nid, &bound, call_site)?;
                        self.set_call_result(fi, dst, v, call_site);
                        Ok(())
                    }
                    Value::EnumVariant { .. } if full.is_empty() => {
                        self.set_call_result(fi, dst, callable, call_site);
                        Ok(())
                    }
                    // A Pending callee absorbs, as in `do_call`.
                    Value::Pending(_) => {
                        self.set_call_result(fi, dst, callable, call_site);
                        Ok(())
                    }
                    _ => Err(format!("Cannot call {}", callable.type_name())),
                }
            }
            Value::Pending(_) => {
                self.set_call_result(fi, dst, spread, call_site);
                Ok(())
            }
            other => Err(not_spreadable(other)),
        }
    }
}

/// What a spread value is not: the refusal every spread call shares.
fn not_spreadable(v: Value) -> String {
    format!(
        "A spread argument (`...`) must be a list (arguments by position) or a record \
         (arguments by parameter name), got {}",
        v.type_name()
    )
}

/// The positional argument list of a spread call whose spread is a list, an
/// empty record or a `Pending` (which stays an argument, for the callee's own
/// pending policy): `args` is the written arguments followed by the value
/// spread. `None` when the spread is a record with fields, whose names only
/// the callee can place.
fn spread_positional(
    heap: &crate::heap::Heap,
    args: &[Value],
) -> Option<Result<SmallVec<[Value; 8]>, String>> {
    let (&spread, lead) = args.split_last()?;
    let mut full: SmallVec<[Value; 8]> = SmallVec::from_slice(lead);
    match spread {
        Value::List(id) => full.extend_from_slice(heap.get_list(id)),
        Value::Map(id) if heap.get_map(id).is_empty() => {}
        Value::Map(_) => return None,
        Value::Pending(_) => full.push(spread),
        other => return Some(Err(not_spreadable(other))),
    }
    Some(Ok(full))
}

impl<'a> Vm<'a> {
    /// The arguments of a spread call to native `nid`, laid out as it takes
    /// them: `args` is the written arguments followed by the value spread. A
    /// record binds against the parameter names the native declares
    /// (`native fn`), with the binder a written `f(name: value)` uses.
    fn bind_native_spread(
        &self,
        nid: NativeFnId,
        args: &[Value],
    ) -> Result<SmallVec<[Value; 8]>, String> {
        if let Some(positional) = spread_positional(self.heap, args) {
            return positional;
        }
        let Some((&Value::Map(id), lead)) = args.split_last() else {
            return Err("internal error: a spread call with no arguments".into());
        };
        let name = self.native_fns.get_name(nid);
        let sigs = self.native_signatures(nid);
        if sigs.is_empty() {
            return Err(crate::native_fn::undeclared_named_args(name));
        }
        let record = self.heap.get_map(id);
        let mut full: SmallVec<[Value; 8]> = SmallVec::from_slice(lead);
        full.extend(record.values().copied());
        let mut names: SmallVec<[Option<&str>; 8]> = smallvec::smallvec![None; lead.len()];
        names.extend(record.keys().map(|k| Some(k.as_str())));
        crate::native_fn::bind_native_args(name, sigs, &full, &names)
    }
}

impl<'a> Vm<'a> {
    /// The parameter lists native `nid` declares, for binding arguments by
    /// name when the call runs: the ones the host gave it, or else the ones
    /// the running program's own `native fn` declaration carries
    /// (`Program::native_links`). Empty when neither names any.
    pub(super) fn native_signatures(&self, nid: NativeFnId) -> &[crate::native_fn::NativeSignature] {
        let sigs = self.native_fns.signatures(nid);
        if !sigs.is_empty() {
            return sigs;
        }
        let name = self.native_fns.get_name(nid);
        self.program
            .native_links
            .iter()
            .find(|link| link.host == name)
            .map_or(&[], |link| &link.signatures)
    }
}
