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
    /// Static builtin call `name(args...)` (unshadowed builtin called directly).
    pub(super) fn do_builtin_call(
        &mut self,
        fi: usize,
        dst: Reg,
        name_cid: crate::constant_table::ConstantId,
        args: &[Value],
        in_place: bool,
    ) -> Result<(), String> {
        // `program` is a `Copy` borrow with the Vm's own lifetime, so the name
        // &str detaches from `self` — no per-call String allocation.
        let program = self.program;
        let name = match program.get_string_constant(name_cid) {
            Some(s) => s,
            None => return Err("BuiltinCall: invalid name constant".into()),
        };
        let nid = match self.native_fns.lookup_name(name) {
            Some(id) => id,
            None => return Err(format!("Unknown builtin: {}", name)),
        };
        // Mutating builtins (`append`/`set`/…) are never intrinsics, so the
        // in-place flag only reaches `call_native_fn`.
        let v = if in_place {
            self.call_native_fn_in_place(nid, args)?
        } else {
            self.call_native_or_intrinsic(nid, args)?
        };
        self.set(fi, dst, v);
        Ok(())
    }

    /// Pending interception (Chunk C). If any argument is a `Pending`, apply the
    /// native's classification: `Strict` absorbs (return the leftmost `Pending`
    /// arg, don't call), `Effectful` no-ops (return `Nil`, emit nothing, don't
    /// call), `NonStrict` proceeds (it inspects the pending itself). Returns
    /// `None` to proceed with the real call.
    ///
    /// Cheap early-out: only a top-level `Pending` *argument* triggers it — a
    /// pending nested inside a resolved list is left alone (element-wise). This
    /// MUST be consulted at every native entry point, because a native can be
    /// invoked three ways that don't share a single call site: the intrinsic
    /// fork below (map/filter/… never reach the leaf), the shared leaf
    /// `call_native_fn_flagged` (plain + in-place mutating builtins), and
    /// record-field method calls. Guarding only one path would make absorption
    /// depend on the in-place optimizer or call syntax.
    fn intercept_pending(&self, nid: NativeFnId, args: &[Value]) -> Option<Value> {
        let pending = args.iter().find(|v| matches!(v, Value::Pending(_)))?;
        match self.native_fns.get_class(nid) {
            crate::native_fn::NativeClass::Strict => Some(*pending),
            crate::native_fn::NativeClass::Effectful => Some(Value::Nil),
            crate::native_fn::NativeClass::NonStrict => None,
        }
    }

    /// Dispatch a native function, handling the higher-order intrinsics
    /// specially (they call closures synchronously).
    pub(super) fn call_native_or_intrinsic(&mut self, nid: NativeFnId, args: &[Value]) -> Result<Value, String> {
        // Intercept before the intrinsic fork: map/filter/reduce/forEach are
        // dispatched here and never reach the leaf, so a Pending collection base
        // (e.g. `map(pending, f)`) must be absorbed here.
        if let Some(v) = self.intercept_pending(nid, args) {
            return Ok(v);
        }
        let nf = self.native_fns;
        if nf.intrinsic_map == Some(nid) {
            self.builtin_map(args)
        } else if nf.intrinsic_filter == Some(nid) {
            self.builtin_filter(args)
        } else if nf.intrinsic_reduce == Some(nid) {
            self.builtin_reduce(args)
        } else if nf.intrinsic_for_each == Some(nid) {
            self.builtin_for_each(args)
        } else {
            self.call_native_fn(nid, args)
        }
    }

    /// Call a non-intrinsic native function via `PetalCxt` (clone-and-alloc).
    pub(super) fn call_native_fn(&mut self, nid: NativeFnId, args: &[Value]) -> Result<Value, String> {
        self.call_native_fn_flagged(nid, args, false)
    }

    /// Call a non-intrinsic native function marked in-place: a mutating builtin
    /// (`append`/`set`/…) may reuse its container argument's backing store.
    /// Only reached when escape analysis proved the container unique +
    /// non-escaping (M4).
    fn call_native_fn_in_place(&mut self, nid: NativeFnId, args: &[Value]) -> Result<Value, String> {
        self.call_native_fn_flagged(nid, args, true)
    }

    fn call_native_fn_flagged(
        &mut self,
        nid: NativeFnId,
        args: &[Value],
        in_place: bool,
    ) -> Result<Value, String> {
        // The shared leaf for every real native invocation — plain calls, the
        // in-place mutating path (`append`/`set`/…, on by default via the
        // optimizer), and record-field method calls. Intercept here so a Pending
        // argument is absorbed/no-op'd regardless of which path or optimization
        // reached this native (redundant with the pre-fork check on the plain
        // path, but that check only returns early; the scan is a cheap no-op
        // when no arg is Pending).
        if let Some(v) = self.intercept_pending(nid, args) {
            return Ok(v);
        }
        let func = self.native_fns.get_func(nid);
        let mut cxt = PetalCxt::new(
            args,
            self.heap,
            self.output,
            self.symbols,
            self.output_buffers,
            self.bindings,
            self.counters,
            self.rng_state,
            self.noise_seed,
            self.resources,
            self.echo,
            self.handle_classes,
        );
        cxt.set_in_place(in_place);
        let count = func(&mut cxt)?;
        let results = cxt.take_results();
        Ok(if count > 0 && !results.is_empty() {
            results[0]
        } else {
            Value::Nil
        })
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
    ) -> Result<Value, String> {
        // `handle_classes` is a shared `&'a [HandleClass]`, so copying the
        // reference detaches `class` from `self` and the `&mut` field
        // reborrows below don't conflict.
        let handle_classes = self.handle_classes;
        let class = handle_classes.get(h.class.0 as usize).ok_or_else(|| {
            format!("Handle references unregistered handle class id {}", h.class.0)
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
        let mut cxt = PetalCxt::new(
            &full_args,
            self.heap,
            self.output,
            self.symbols,
            self.output_buffers,
            self.bindings,
            self.counters,
            self.rng_state,
            self.noise_seed,
            self.resources,
            self.echo,
            handle_classes,
        );
        let count = (class.call_method)(&mut cxt, method_name)?;
        let results = cxt.take_results();
        Ok(if count > 0 && !results.is_empty() {
            results[0]
        } else {
            Value::Nil
        })
    }
}
