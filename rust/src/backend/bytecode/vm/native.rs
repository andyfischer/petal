//! Native-function and `BuiltinCall` dispatch: the bridge from the VM into
//! `crate::builtins` via the [`NativeFnTable`] and `PetalCxt`, plus handle-class
//! method dispatch.
//!
//! Split out of `vm/mod.rs`; see that module for the [`Vm`] struct and the
//! core step loop. The higher-order intrinsics that `call_native_or_intrinsic`
//! forks to live in the sibling `intrinsics` file.

use super::*;

use crate::handle::HandleVal;
use crate::native_fn::PetalCxt;

impl<'a> Vm<'a> {
    /// The call chain reaching a native: its own call site, then the return
    /// address of each enclosing frame, innermost first. Empty unless emit
    /// tracing is on — building it is the one per-native-call cost tracing adds,
    /// so no run that will never read it pays for one.
    ///
    /// The leaf alone is not enough for source attribution. A drawing "builtin"
    /// is often a Petal function in a prelude wrapping the real native, so the
    /// leaf is a line of library code and the line the *user* wrote is a frame or
    /// two further out. Recording the chain lets a reader pick the frame in the
    /// file it is showing ([`crate::provenance::pick_frame`]) instead of being
    /// stuck with whichever frame happened to be innermost.
    fn emit_call_chain(&self, origin: Option<TermId>) -> SmallVec<[TermId; 4]> {
        let mut chain: SmallVec<[TermId; 4]> = SmallVec::new();
        if !self.trace_emit {
            return chain;
        }
        chain.extend(origin);
        // The innermost frame is last in `vm_frames`, so walk it backwards. The
        // root frame's `call_site` is `None` — nothing called it — which ends
        // the chain naturally.
        chain.extend(
            self.stack
                .vm_frames
                .iter()
                .rev()
                .filter_map(|f| f.call_site),
        );
        chain
    }

    /// Static builtin call `name(args...)` (unshadowed builtin called directly).
    pub(super) fn do_builtin_call(
        &mut self,
        fi: usize,
        dst: Reg,
        name_cid: crate::constant_table::ConstantId,
        args: &[Value],
        in_place: bool,
        origin: Option<TermId>,
    ) -> Result<(), String> {
        // The name→id resolution is precomputed per program (see
        // `BytecodeProgram::builtin_ids`); only the failure path, which is about
        // to error anyway, goes back to the constant table for a name to print.
        let nid = match self.bc.builtin_id(name_cid) {
            Some(id) => NativeFnId(id),
            // Either the name is not a builtin, or this program was lowered
            // without a native table to resolve against (any path that does not
            // go through `Env::ensure_bytecode`). Fall back to the by-name
            // lookup, which answers both cases correctly.
            None => {
                let program = self.program;
                let name = match program.get_string_constant(name_cid) {
                    Some(s) => s,
                    None => return Err("BuiltinCall: invalid name constant".into()),
                };
                match self.native_fns.lookup_name(name) {
                    Some(id) => id,
                    None => return Err(format!("Unknown builtin: {name}")),
                }
            }
        };
        // `__declare_method` publishes a method into the running stack, which
        // is state no native can reach through `PetalCxt` — so it is handled
        // here rather than dispatched. The compiler is its only caller.
        if self.native_fns.intrinsic_declare_method == Some(nid) {
            let v = self.declare_method(args)?;
            self.set(fi, dst, v);
            return Ok(());
        }
        // Mutating builtins (`append`/`set`/…) are never intrinsics, so the
        // in-place flag can go straight to the leaf.
        let v = if in_place {
            self.call_native_fn(nid, args, true, origin)?
        } else {
            self.call_native_or_intrinsic(nid, args, origin)?
        };
        self.set(fi, dst, v);
        Ok(())
    }

    /// Pending interception (Chunk C). If any argument is a `Pending`, apply the
    /// native's classification: `Strict` absorbs (return the leftmost `Pending`
    /// arg, don't call), `Effectful` no-ops (return `Nil`, emit nothing, don't
    /// call), `AllowPending` proceeds (it inspects the pending itself). Returns
    /// `None` to proceed with the real call.
    ///
    /// Cheap early-out: only a top-level `Pending` *argument* triggers it — a
    /// pending nested inside a resolved list is left alone (element-wise). This
    /// MUST be consulted at every native entry point, because a native can be
    /// invoked three ways that don't share a single call site: the intrinsic
    /// fork below (map/filter/… never reach the leaf), the shared leaf
    /// [`call_native_fn`](Vm::call_native_fn) (plain + in-place mutating
    /// builtins), and record-field method calls. Guarding only one path would make absorption
    /// depend on the in-place optimizer or call syntax.
    fn intercept_pending(
        &mut self,
        nid: NativeFnId,
        args: &[Value],
        origin: Option<TermId>,
    ) -> Option<Value> {
        let pending = *args.iter().find(|v| matches!(v, Value::Pending(_)))?;
        match self.native_fns.get_class(nid) {
            // Both absorbing outcomes swallow the leftmost Pending — bump its
            // always-on absorbed_count (and log `(origin, id)` when the debug
            // trace is on). AllowPending inspects it instead, so it does not count.
            crate::native_fn::NativeClass::Strict => {
                self.note_absorption(pending, origin);
                Some(pending)
            }
            crate::native_fn::NativeClass::Effectful => {
                self.note_absorption(pending, origin);
                Some(Value::Nil)
            }
            crate::native_fn::NativeClass::AllowPending => None,
        }
    }

    /// Dispatch a native function, handling the higher-order intrinsics
    /// specially (they call closures synchronously).
    pub(super) fn call_native_or_intrinsic(
        &mut self,
        nid: NativeFnId,
        args: &[Value],
        origin: Option<TermId>,
    ) -> Result<Value, String> {
        // Intercept before the intrinsic fork: map/filter/reduce/forEach are
        // dispatched here and never reach the leaf, so a Pending collection base
        // (e.g. `map(pending, f)`) must be absorbed here.
        if let Some(v) = self.intercept_pending(nid, args, origin) {
            return Ok(v);
        }
        let nf = self.native_fns;
        // The intrinsics never reach the shared leaf, so they are counted here;
        // everything else is counted once, in `call_native_fn`.
        if nf.intrinsic_map == Some(nid)
            || nf.intrinsic_filter == Some(nid)
            || nf.intrinsic_reduce == Some(nid)
            || nf.intrinsic_for_each == Some(nid)
        {
            self.profile.record_native(nid.0);
        }
        if nf.intrinsic_map == Some(nid) {
            self.builtin_map(args)
        } else if nf.intrinsic_filter == Some(nid) {
            self.builtin_filter(args)
        } else if nf.intrinsic_reduce == Some(nid) {
            self.builtin_reduce(args)
        } else if nf.intrinsic_for_each == Some(nid) {
            self.builtin_for_each(args)
        } else {
            self.call_native_fn(nid, args, false, origin)
        }
    }

    /// Record `fn Class.method` in the stack's per-run method table.
    /// `args` is `[class name, method name, callable]`, all emitted by the
    /// compiler — a malformed call can only come from hand-written IR.
    fn declare_method(&mut self, args: &[Value]) -> Result<Value, String> {
        let [Value::String(class), Value::String(method), func] = args else {
            return Err(
                "internal error: __declare_method expects (class, method, function)".into(),
            );
        };
        let class = self.heap.get_string(*class).to_string();
        let method = self.heap.get_string(*method).to_string();
        self.stack
            .methods
            .entry(class)
            .or_default()
            .insert(method, *func);
        Ok(Value::Nil)
    }

    /// Build the `PetalCxt` a native call runs against: this `Vm`'s borrows of
    /// the owning context's heap, output, bindings, resources and flags, plus
    /// this call's `args`, emit `chain`, `origin`, and `in_place` eligibility.
    /// The one place those twenty-odd borrows are threaded together — every
    /// native entry point goes through it, so a new piece of context reaches
    /// all of them at once.
    fn native_cxt<'c>(
        &'c mut self,
        args: &'c [Value],
        chain: &'c [TermId],
        origin: Option<TermId>,
        in_place: bool,
    ) -> PetalCxt<'c> {
        PetalCxt {
            args,
            heap: self.heap,
            output: self.output,
            symbols: self.symbols,
            output_buffers: self.output_buffers,
            trace_emit: self.trace_emit,
            emit_origins: self.emit_origins,
            emit_chain: chain,
            bindings: self.bindings,
            counters: self.counters,
            rng_state: self.rng_state,
            noise_seed: self.noise_seed,
            resources: self.resources,
            trace_pending: self.trace_pending,
            absorption_log: self.absorption_log,
            origin,
            frame: self.frame,
            echo: self.echo,
            handle_classes: self.handle_classes,
            results: Vec::new(),
            in_place,
        }
    }

    /// Call a non-intrinsic native function via `PetalCxt`. `origin` is the
    /// requesting call site, stamped onto any resource the native creates.
    /// `in_place` lets a mutating builtin (`append`/`set`/…) reuse its container
    /// argument's backing store instead of cloning it; it is set only when
    /// escape analysis proved that container unique + non-escaping (M4).
    ///
    /// This is the shared leaf for every real native invocation — plain calls,
    /// the in-place mutating path, and record-field method calls.
    pub(super) fn call_native_fn(
        &mut self,
        nid: NativeFnId,
        args: &[Value],
        in_place: bool,
        origin: Option<TermId>,
    ) -> Result<Value, String> {
        // Intercept here so a Pending argument is absorbed/no-op'd regardless of
        // which path or optimization reached this native (redundant with the
        // pre-fork check in `call_native_or_intrinsic`, but that check only
        // returns early; the scan is a cheap no-op when no arg is Pending).
        if let Some(v) = self.intercept_pending(nid, args, origin) {
            return Ok(v);
        }
        self.profile.record_native(nid.0);
        let func = self.native_fns.get_func(nid);
        let chain = self.emit_call_chain(origin);
        let mut cxt = self.native_cxt(args, &chain, origin, in_place);
        let count = func(&mut cxt)?;
        Ok(cxt.take_result(count))
    }

    /// Dispatch `h.method(args...)` through the handle class registered for
    /// `h.class`. Mirrors the graph engine's `call_handle_method` (including
    /// error messages): liveness is checked first, and a stale handle errors
    /// with the class name and `describe()` output without invoking
    /// `call_method`. The receiver is prepended, so it is cxt arg 1.
    pub(super) fn call_handle_method(
        &mut self,
        h: HandleVal,
        method_name: &str,
        args: &[Value],
        origin: Option<TermId>,
    ) -> Result<Value, String> {
        // `handle_classes` is a shared `&'a [HandleClass]`, so copying the
        // reference detaches `class` from `self` and the `&mut` field
        // reborrows below don't conflict.
        let handle_classes = self.handle_classes;
        let class = handle_classes.get(h.class.0 as usize).ok_or_else(|| {
            format!(
                "Handle references unregistered handle class id {}",
                h.class.0
            )
        })?;
        if !(class.is_valid)(h.slot, h.serial) {
            return Err(format!(
                "Stale {} handle: {}",
                class.name,
                (class.describe)(h.slot, h.serial)
            ));
        }
        let mut full_args: SmallVec<[Value; 8]> = SmallVec::new();
        full_args.push(Value::Handle(h));
        full_args.extend_from_slice(args);
        let chain = self.emit_call_chain(origin);
        let mut cxt = self.native_cxt(&full_args, &chain, origin, false);
        let count = (class.call_method)(&mut cxt, method_name)?;
        Ok(cxt.take_result(count))
    }
}
