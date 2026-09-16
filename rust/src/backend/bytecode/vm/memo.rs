//! The VM half of memoized scopes (see [`crate::memo`] for the model): the
//! recording hooks the executor calls, and the validation, replay and
//! re-execution a `Call` goes through before deciding whether to push a frame.
//!
//! Split out of `vm/mod.rs`; see that module for the [`Vm`] struct and the
//! core step loop.

use super::*;

use std::collections::HashMap;

use crate::backend::RuntimeClosure;
use crate::heap::CellId;
use crate::memo::{
    ARG_COMPARE_BUDGET, Dep, MAX_SCOPE_DEPS, MIN_SCOPE_INSTS, MemoSlot, OUTPUT_COMPARE_PER_VALUE,
    OpenScope, OutputSegment, PreviousRecord, ScopePath, ScopeValues, container_args_fingerprint,
    holds_local_cell,
};
use crate::program::ClosureId;
use crate::run_deps::Activity;
use crate::stack::RuntimeStateKey;

impl<'a> Vm<'a> {
    // ── Recording hooks ───────────────────────────────────────────────────
    //
    // Each is one flag test when memoization is off or no scope is open,
    // which is every instruction of a top-level script body.

    /// The innermost open scope, when the VM is recording into one.
    #[inline]
    fn memo_recording_scope(&mut self) -> Option<&mut OpenScope> {
        if self.memo && self.stack.memo.recording() {
            self.stack.memo.innermost()
        } else {
            None
        }
    }

    /// A named term was observed with `value`.
    #[inline]
    pub(super) fn memo_note_observation(&mut self, term: TermId, value: Value) {
        if let Some(s) = self.memo_recording_scope() {
            s.deps.push(Dep::Observed { term, value });
        }
    }

    /// A `state` slot was read (or initialized) and held `value`.
    #[inline]
    pub(super) fn memo_note_state_read(&mut self, key: &RuntimeStateKey, value: Option<Value>) {
        if let Some(s) = self.memo_recording_scope() {
            s.deps.push(Dep::StateRead {
                key: key.clone(),
                value,
            });
        }
    }

    /// A `state` slot was written. An in-place producer (`mutated`) already
    /// edited the slot's object, which a replay cannot redo.
    #[inline]
    pub(super) fn memo_note_state_write(
        &mut self,
        key: &RuntimeStateKey,
        value: Value,
        mutated: bool,
    ) {
        if self.memo_recording_scope().is_none() {
            return;
        }
        if mutated || self.memo_value_escapes_local_cell(value) {
            self.stack.memo.note_effect();
        } else if let Some(s) = self.stack.memo.innermost() {
            s.deps.push(Dep::StateWrite {
                key: key.clone(),
                value,
            });
        }
    }

    /// Something happened a replay could not reproduce.
    #[inline]
    pub(super) fn memo_note_effect(&mut self) {
        if let Some(s) = self.memo_recording_scope() {
            s.effectful = true;
        }
    }

    /// A `var` cell was created here.
    #[inline]
    pub(super) fn memo_note_cell_new(&mut self, cell: CellId) {
        if let Some(s) = self.memo_recording_scope() {
            s.local_cells.insert(cell);
        }
    }

    /// A cell created in this scope now lives in a `state` slot: it outlives
    /// the scope, so reads of it from here on are dependencies.
    #[inline]
    pub(super) fn memo_note_cell_escaped(&mut self, cell: CellId) {
        if self.memo_recording_scope().is_some() {
            for s in &mut self.stack.memo.open {
                s.local_cells.remove(&cell);
            }
        }
    }

    /// A `var` cell was read and held `value`.
    #[inline]
    pub(super) fn memo_note_cell_read(&mut self, cell: CellId, value: Value) {
        if let Some(s) = self.memo_recording_scope()
            && !s.local_cells.contains(&cell)
        {
            s.deps.push(Dep::CellRead { cell, value });
        }
    }

    /// A `var` cell was written.
    #[inline]
    pub(super) fn memo_note_cell_write(&mut self, cell: CellId, value: Value) {
        if self.memo_recording_scope().is_none() {
            return;
        }
        if self.memo_value_escapes_local_cell(value) {
            self.stack.memo.note_effect();
        } else if let Some(s) = self.stack.memo.innermost()
            && !s.local_cells.contains(&cell)
        {
            s.deps.push(Dep::CellWrite { cell, value });
        }
    }

    /// Whether `value`, stored somewhere that outlives the scope, would carry
    /// one of the scope's own cells out with it.
    fn memo_value_escapes_local_cell(&self, value: Value) -> bool {
        self.stack
            .memo
            .open
            .last()
            .is_some_and(|s| holds_local_cell(&value, self.heap, self.closures, &s.local_cells))
    }

    /// A native call finished. Classify it from what it reported doing
    /// between the two activity snapshots: an effect makes the scope
    /// unrecordable; a binding read makes the call a probe, unless it also
    /// emitted (a native that both reads input and draws cannot be
    /// re-evaluated at validation without drawing again).
    pub(super) fn memo_note_native(
        &mut self,
        nid: NativeFnId,
        args: &[Value],
        result: Value,
        before: Activity,
    ) {
        let after = self.stack.run_deps.activity();
        if after.effects != before.effects || matches!(result, Value::Pending(_)) {
            self.stack.memo.note_effect();
            return;
        }
        if after.host_reads != before.host_reads {
            if let Some(s) = self.stack.memo.innermost() {
                s.deps.push(Dep::HostRead);
            }
        }
        if after.resource_reads != before.resource_reads {
            if let Some(s) = self.stack.memo.innermost() {
                s.deps.push(Dep::ResourcesRead);
            }
        }
        if after.binding_reads != before.binding_reads {
            if after.emits != before.emits
                || args.iter().any(|a| {
                    matches!(
                        a,
                        Value::Closure(_)
                            | Value::OverloadSet(_)
                            | Value::Pending(_)
                            | Value::Cell(_)
                    )
                })
            {
                self.stack.memo.note_effect();
                return;
            }
            let args_fp = container_args_fingerprint(args, self.heap);
            if let Some(s) = self.stack.memo.innermost() {
                s.deps.push(Dep::Probe {
                    native: nid,
                    args: args.into(),
                    args_fp,
                    result,
                });
            }
        }
    }

    // ── Scope lifecycle ───────────────────────────────────────────────────

    /// The frame on top of the stack just started running `fn_id` as a
    /// scope: open its record. `previous` is set when this is a re-execution
    /// during validation (see [`memo_reexecute`](Self::memo_reexecute)).
    pub(super) fn memo_open(
        &mut self,
        fn_id: FunctionId,
        cid: ClosureId,
        site: u64,
        args: &[Value],
        previous: Option<Box<PreviousRecord>>,
    ) {
        let path = self
            .stack
            .vm_frames
            .last()
            .map(|f| f.path.clone())
            .unwrap_or_default();
        let out_start = self.memo_output_lens();
        let captures: ScopeValues = self
            .closures
            .closure(cid)
            .captures
            .iter()
            .copied()
            .collect();
        let capture = self.stack.begin_touch_capture();
        self.stack.memo.open.push(OpenScope {
            path,
            depth: self.stack.vm_frames.len(),
            fn_id,
            site,
            captures,
            args: args.iter().copied().collect(),
            deps: Vec::new(),
            out_start,
            insts_at_entry: self.stack.insts,
            rng_at_entry: *self.rng_state,
            effectful: false,
            local_cells: Default::default(),
            capture: Some(capture),
            previous,
        });
    }

    /// The scope on top finished with `result`: record it, fold it into its
    /// parent, or give it up as effectful.
    pub(super) fn memo_close(&mut self, result: Value) {
        let Some(mut sc) = self.stack.memo.open.pop() else {
            return;
        };
        debug_assert_eq!(
            sc.depth,
            self.stack.vm_frames.len(),
            "memo scope closed off its frame"
        );
        let touches = match sc.capture.take() {
            Some(c) => self.stack.end_touch_capture(c),
            None => Default::default(),
        };
        let detached = sc.previous.is_some();
        let insts = self.stack.insts.saturating_sub(sc.insts_at_entry);

        // The output the scope appended, per buffer.
        let mut outputs = Vec::new();
        for (sym, buf) in self.output_buffers.iter() {
            let start = sc
                .out_start
                .iter()
                .find(|(s, _)| s == sym)
                .map_or(0, |(_, n)| *n);
            if buf.len() > start {
                let origins = if self.trace_emit {
                    Some(
                        self.emit_origins
                            .get(sym)
                            .and_then(|o| o.get(start..))
                            .map(|o| o.to_vec())
                            .unwrap_or_default(),
                    )
                } else {
                    None
                };
                outputs.push(OutputSegment {
                    sym: *sym,
                    values: buf[start..].to_vec(),
                    origins,
                });
            }
        }

        let effectful = sc.effectful
            || self.stack.memo.poisoned
            || *self.rng_state != sc.rng_at_entry
            || matches!(result, Value::Pending(_))
            || sc.deps.len() > MAX_SCOPE_DEPS
            || holds_local_cell(&result, self.heap, self.closures, &sc.local_cells);
        if effectful {
            self.stack.memo.stats.effectful += 1;
            if detached {
                self.stack.memo.last_reexec_changed = Some(true);
            } else if let Some(p) = self.stack.memo.innermost() {
                p.effectful = true;
            }
            return;
        }

        // A tiny scope with nothing to validate is folded into its parent,
        // whose output range and record already cover it — a `draw_rect`
        // wrapper in the prelude is one of these. With no parent to fold
        // into, an emitting scope is recorded regardless: it is the unit the
        // top-level loop calling it would otherwise re-run.
        let has_parent = !self.stack.memo.open.is_empty();
        let worth = detached
            || !sc.deps.is_empty()
            || insts >= MIN_SCOPE_INSTS
            || (!outputs.is_empty() && !has_parent);
        if !worth || (!detached && !self.stack.memo.has_room()) {
            // Fold into the parent: its record covers this call's reads,
            // observations and output range, and inherits its cells so a
            // later escape through the parent's result is still caught.
            self.stack.memo.stats.inlined += 1;
            if let Some(p) = self.stack.memo.innermost() {
                p.deps.append(&mut sc.deps);
                p.local_cells.extend(sc.local_cells.drain());
            }
            return;
        }

        let pure = sc.deps.iter().all(|d| match d {
            Dep::StateWrite { .. } | Dep::CellWrite { .. } => false,
            Dep::Child { path, .. } => self.stack.memo.get(path).is_some_and(|c| c.pure),
            _ => true,
        });
        let serial = self.stack.memo.next_serial();
        let slot = MemoSlot {
            serial,
            visited: self.stack.memo.run(),
            fn_id: sc.fn_id,
            site: sc.site,
            hit: false,
            captures: sc.captures,
            args: sc.args,
            result,
            deps: sc.deps,
            outputs,
            touches,
            host_revision: self.stack.run_deps.host_data_now(),
            resources_revision: self.resources.revision(),
            pure,
        };
        if let Some(previous) = sc.previous {
            let changed = !self.memo_same_effects(&previous, &slot);
            if !changed {
                self.stack.memo.stats.cutoffs += 1;
            }
            self.stack.memo.last_reexec_changed = Some(changed);
        } else if let Some(p) = self.stack.memo.innermost() {
            p.deps.push(Dep::Child {
                path: sc.path.clone(),
                serial,
            });
        }
        self.stack.memo.stats.records += 1;
        self.stack.memo.insert(sc.path, slot);
    }

    /// Whether a re-executed scope did what its previous record says it
    /// did: same result, same output, same writes and children, same state
    /// touched. Reads may differ — that is the whole point of a cutoff.
    fn memo_same_effects(&mut self, prev: &PreviousRecord, now: &MemoSlot) -> bool {
        if !self.memo_eq(prev.result, now.result, ARG_COMPARE_BUDGET) {
            return false;
        }
        if prev.touches != now.touches || prev.outputs.len() != now.outputs.len() {
            return false;
        }
        for (a, b) in prev.outputs.iter().zip(&now.outputs) {
            if a.sym != b.sym || a.values.len() != b.values.len() {
                return false;
            }
            let budget = 256 + OUTPUT_COMPARE_PER_VALUE * a.values.len();
            for (x, y) in a.values.iter().zip(&b.values) {
                if !self.memo_eq(*x, *y, budget) {
                    return false;
                }
            }
        }
        fn is_effect(d: &&Dep) -> bool {
            matches!(
                d,
                Dep::StateWrite { .. } | Dep::CellWrite { .. } | Dep::Child { .. }
            )
        }
        let mut a = prev.deps.iter().filter(is_effect);
        let mut b = now.deps.iter().filter(is_effect);
        loop {
            let same = match (a.next(), b.next()) {
                (None, None) => return true,
                (
                    Some(Dep::StateWrite { key: ka, value: va }),
                    Some(Dep::StateWrite { key: kb, value: vb }),
                ) => ka == kb && self.memo_eq(*va, *vb, ARG_COMPARE_BUDGET),
                (
                    Some(Dep::CellWrite {
                        cell: ca,
                        value: va,
                    }),
                    Some(Dep::CellWrite {
                        cell: cb,
                        value: vb,
                    }),
                ) => ca == cb && self.memo_eq(*va, *vb, ARG_COMPARE_BUDGET),
                (
                    Some(Dep::Child {
                        path: pa,
                        serial: sa,
                    }),
                    Some(Dep::Child {
                        path: pb,
                        serial: sb,
                    }),
                ) => pa == pb && sa == sb,
                _ => false,
            };
            if !same {
                return false;
            }
        }
    }

    /// Every output buffer's length now, to find what a scope appends.
    fn memo_output_lens(&self) -> SmallVec<[(SymbolId, usize); 2]> {
        self.output_buffers
            .iter()
            .map(|(s, v)| (*s, v.len()))
            .collect()
    }

    fn memo_eq(&mut self, a: Value, b: Value, budget: usize) -> bool {
        self.stack
            .memo
            .values_equal(&a, &b, self.heap, self.closures, budget)
    }

    /// A call of `fn_id` (through closure `cid`, with `args`) is about to run
    /// at `path`. If its record is still good, replay it and return the
    /// cached result; otherwise `None`, and the caller pushes the frame.
    pub(super) fn memo_try(
        &mut self,
        path: &ScopePath,
        fn_id: FunctionId,
        cid: ClosureId,
        args: &[Value],
    ) -> Option<Value> {
        let slot = self.stack.memo.get(path)?;
        let same_fn = slot.fn_id == fn_id;
        let (recorded_args, recorded_caps) = (slot.args.clone(), slot.captures.clone());
        let captures: ScopeValues = self
            .closures
            .closure(cid)
            .captures
            .iter()
            .copied()
            .collect();
        let same_call = same_fn
            && self.memo_all_eq(&recorded_args, args)
            && self.memo_all_eq(&recorded_caps, &captures);
        if !same_call || !self.memo_validate(path) {
            self.stack.memo.stats.misses += 1;
            return None;
        }
        self.stack.memo.stats.hits += 1;
        self.stack.memo.note_hit(path);
        self.memo_replay(path, true)
    }

    /// Whether two value lists have the same length and pairwise-equal
    /// elements, by [`memo_eq`](Self::memo_eq).
    fn memo_all_eq(&mut self, recorded: &[Value], now: &[Value]) -> bool {
        recorded.len() == now.len()
            && recorded
                .iter()
                .zip(now)
                .all(|(a, b)| self.memo_eq(*a, *b, ARG_COMPARE_BUDGET))
    }

    /// Walk a record's dependencies in order against the present. Writes
    /// seen along the way overlay the live values, so a read after a write
    /// in the same record checks against what the record wrote.
    fn memo_validate(&mut self, path: &ScopePath) -> bool {
        let (mut deps, host_rev, res_rev) = {
            let Some(slot) = self.stack.memo.get_mut(path) else {
                return false;
            };
            (
                std::mem::take(&mut slot.deps),
                slot.host_revision,
                slot.resources_revision,
            )
        };
        let mut state_overlay: HashMap<RuntimeStateKey, Value> = HashMap::new();
        let mut cell_overlay: HashMap<CellId, Value> = HashMap::new();
        let mut ok = true;
        for dep in deps.iter_mut() {
            let valid = match dep {
                Dep::Probe {
                    native,
                    args,
                    args_fp,
                    result,
                } => {
                    if *args_fp != container_args_fingerprint(args, self.heap) {
                        false
                    } else {
                        self.stack.memo.suppress = true;
                        let answer = self.call_native_fn(*native, args, false, None);
                        self.stack.memo.suppress = false;
                        match answer {
                            Ok(v) => self.memo_eq(v, *result, ARG_COMPARE_BUDGET),
                            Err(_) => false,
                        }
                    }
                }
                Dep::StateRead { key, value } => {
                    let now = state_overlay
                        .get(key)
                        .copied()
                        .or_else(|| self.stack.state.get(key).copied());
                    match (now, *value) {
                        (None, None) => true,
                        (Some(a), Some(b)) => self.memo_eq(a, b, ARG_COMPARE_BUDGET),
                        _ => false,
                    }
                }
                Dep::StateWrite { key, value } => {
                    state_overlay.insert(key.clone(), *value);
                    true
                }
                Dep::CellRead { cell, value } => {
                    if !self.heap.is_live(Value::Cell(*cell)) {
                        false
                    } else {
                        let now = cell_overlay
                            .get(cell)
                            .copied()
                            .unwrap_or_else(|| self.heap.cell_read(*cell));
                        self.memo_eq(now, *value, ARG_COMPARE_BUDGET)
                    }
                }
                Dep::CellWrite { cell, value } => {
                    if !self.heap.is_live(Value::Cell(*cell)) {
                        false
                    } else {
                        cell_overlay.insert(*cell, *value);
                        true
                    }
                }
                Dep::HostRead => self.stack.run_deps.host_data_now() == host_rev,
                Dep::ResourcesRead => self.resources.revision() == res_rev,
                Dep::Child { path, serial } => match self.memo_verify_child(path, *serial) {
                    Some(s) => {
                        *serial = s;
                        true
                    }
                    None => false,
                },
                Dep::Observed { .. } => true,
            };
            if !valid {
                ok = false;
                break;
            }
        }
        match self.stack.memo.get_mut(path) {
            Some(slot) => {
                slot.deps = deps;
                ok
            }
            None => false,
        }
    }

    /// A parent's record names child `path` at record `serial`. Still good?
    /// Validate the child; if that fails and the child is pure, run it alone
    /// and see whether it produces what it produced before. Returns the
    /// serial the parent should now name, or `None` if the child's effects
    /// changed (or may have: a child that writes state is not re-executed
    /// speculatively, since the parent's own re-run would write it again).
    fn memo_verify_child(&mut self, path: &ScopePath, serial: u64) -> Option<u64> {
        let slot = self.stack.memo.get(path)?;
        if slot.serial != serial {
            return None;
        }
        let pure = slot.pure;
        if self.memo_validate(path) {
            self.stack.memo.visit(path);
            return Some(serial);
        }
        if !pure {
            return None;
        }
        self.memo_reexecute(path)
    }

    /// Run the scope recorded at `path` again, alone, with the arguments and
    /// captures it recorded, at the same path. Its record is refreshed; the
    /// output it emits is discarded (the parent's own replay or re-run
    /// supplies it). Returns the new serial if it produced the same effects.
    fn memo_reexecute(&mut self, path: &ScopePath) -> Option<u64> {
        self.stack.memo.stats.reexecs += 1;
        // The record is consumed: the re-execution writes a fresh one at the
        // same path, and the old one is only needed for the comparison.
        let slot = self.stack.memo.take(path)?;
        let (fn_id, site, captures, args) = (slot.fn_id, slot.site, slot.captures, slot.args);
        let previous = PreviousRecord {
            result: slot.result,
            deps: slot.deps,
            outputs: slot.outputs,
            touches: slot.touches,
        };
        let bcfn = self.bc.function(fn_id);
        let func = self.program.functions.get(fn_id.0 as usize)?;
        if args.len() != func.params.len() {
            return None;
        }
        let target = self.stack.vm_frames.len();
        let out_lens = self.memo_output_lens();

        // The frame is built by hand: there is no live closure for this call.
        // A function that refers to itself gets one minted for its
        // self-reference register and left to the collector.
        let closure = RuntimeClosure {
            function_id: fn_id,
            captures: captures.to_vec(),
        };
        self.heap
            .charge_external_alloc(ClosureTable::alloc_cost(&closure));
        let cid = self.closures.alloc_closure(closure);
        let mut frame = self.frame_from_pool(Some(fn_id), bcfn.reg_count, None, None, None);
        frame.path = path.clone();
        for (i, &preg) in bcfn.param_regs.iter().enumerate() {
            if let Some(slot) = frame.regs.get_mut(preg as usize) {
                *slot = args[i];
            }
        }
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
        frame.memo_scope = true;
        self.stack.vm_frames.push(frame);
        self.memo_open(fn_id, cid, site, &args, Some(Box::new(previous)));
        self.stack.memo.last_reexec_changed = None;

        let mut failed = false;
        loop {
            if self.stack.vm_frames.len() <= target {
                break;
            }
            match self.step() {
                StepResult::Continue => {}
                StepResult::Complete(_) => break,
                StepResult::Error(_) => {
                    failed = true;
                    break;
                }
            }
        }
        // The output it emitted is not this frame's: the parent replays or
        // re-runs it.
        for (sym, len) in out_lens {
            if let Some(buf) = self.output_buffers.get_mut(&sym) {
                buf.truncate(len);
            }
            if let Some(o) = self.emit_origins.get_mut(&sym) {
                o.truncate(len);
            }
        }
        if failed || self.stack.vm_frames.len() > target {
            // Unwind what the failed run left, and stop memoizing for the
            // rest of this run: the parent will re-run and hit the same
            // error in the ordinary way.
            while self.stack.vm_frames.len() > target {
                let f = self.stack.vm_frames.pop().unwrap();
                self.recycle_frame(f);
            }
            while self
                .stack
                .memo
                .open
                .last()
                .is_some_and(|s| s.depth > target)
            {
                let mut sc = self.stack.memo.open.pop().unwrap();
                if let Some(c) = sc.capture.take() {
                    let _ = self.stack.end_touch_capture(c);
                }
            }
            self.stack.memo.poisoned = true;
            return None;
        }
        match self.stack.memo.last_reexec_changed.take() {
            Some(false) => self.stack.memo.get(path).map(|s| s.serial),
            _ => None,
        }
    }

    /// Replay a validated record: splice its output back, re-apply its writes
    /// and its children's in order, keep its state alive through the sweep,
    /// and restore its observations. Returns the cached result.
    ///
    /// A record's output segment already covers its children's output, so
    /// the recursion into children passes `with_output = false` and replays
    /// only their writes and observations.
    fn memo_replay(&mut self, path: &ScopePath, with_output: bool) -> Option<Value> {
        let mut slot = self.stack.memo.take(path)?;
        slot.visited = self.stack.memo.run();
        for seg in slot.outputs.iter().filter(|_| with_output) {
            let buf = self.output_buffers.entry(seg.sym).or_default();
            buf.extend_from_slice(&seg.values);
            if self.trace_emit {
                let len = buf.len();
                let origins = self.emit_origins.entry(seg.sym).or_default();
                origins.resize_with(len - seg.values.len(), Default::default);
                match &seg.origins {
                    Some(o) if o.len() == seg.values.len() => origins.extend(o.iter().cloned()),
                    _ => origins.resize_with(len, Default::default),
                }
            }
        }
        for dep in &slot.deps {
            match dep {
                Dep::StateWrite { key, value } => {
                    self.stack.touch_state(key);
                    let old = self.stack.state.insert(key.clone(), *value);
                    self.stack.run_deps.note_state_write(
                        old,
                        *value,
                        false,
                        self.heap,
                        self.closures,
                    );
                }
                Dep::CellWrite { cell, value } => {
                    if self.heap.is_live(Value::Cell(*cell)) {
                        self.heap.cell_write(*cell, *value);
                    }
                }
                Dep::Child { path, .. } => {
                    self.memo_replay(path, false);
                }
                Dep::Observed { term, value } => {
                    if self.observations.enabled {
                        self.observations.record(*term, *value);
                    }
                }
                _ => {}
            }
        }
        self.stack.retain_touches(&slot.touches);
        if self.stack.memo.recording() {
            let serial = slot.serial;
            if let Some(p) = self.stack.memo.innermost() {
                p.deps.push(Dep::Child {
                    path: path.clone(),
                    serial,
                });
            }
        }
        let result = slot.result;
        self.stack.memo.insert(path.clone(), slot);
        Some(result)
    }
}
