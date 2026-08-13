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

    /// Ask a user comparator whether `a` sorts strictly before `b`.
    ///
    /// Both comparator conventions are accepted, because both are what someone
    /// reaches for first and neither can be mistaken for the other:
    ///
    /// * a **number** — negative means "a first", positive means "b first",
    ///   zero means "equal" (the C/JS `sort` convention);
    /// * a **boolean** — `true` means "a first" (a plain less-than predicate).
    ///
    /// Equality (`0` / `false`) answers `false`, and the merge below only ever
    /// moves an element ahead of an earlier one on a strict `true`, which is
    /// what makes the sort stable.
    fn sorts_before(&mut self, func: Value, a: Value, b: Value) -> Result<bool, String> {
        match self.call_closure_sync(func, &[a, b])? {
            Value::Int(n) => Ok(n < 0),
            Value::Float(f) => Ok(f < 0.0),
            Value::Bool(t) => Ok(t),
            Value::Nil => Ok(false),
            other => Err(format!(
                "sort() comparator must return a number (negative/zero/positive) \
                 or a bool (true = first argument sorts first), got {}",
                other.type_name()
            )),
        }
    }

    /// Stable merge sort driven by a user comparator. Merge sort rather than a
    /// `sort_by` over the heap-side slice because each comparison can call back
    /// into the VM (and can fail), which `std`'s comparator signature has no way
    /// to express.
    ///
    /// Stability comes from the tie rule: the right run's head is taken only
    /// when it sorts *strictly* before the left run's head, so equal elements
    /// keep the order they were given in.
    fn merge_sort_by(&mut self, items: Vec<Value>, func: Value) -> Result<Vec<Value>, String> {
        if items.len() <= 1 {
            return Ok(items);
        }
        let mut right = items;
        let left: Vec<Value> = right.drain(..right.len() / 2).collect();
        let left = self.merge_sort_by(left, func)?;
        let right = self.merge_sort_by(right, func)?;

        let mut out = Vec::with_capacity(left.len() + right.len());
        let (mut i, mut j) = (0usize, 0usize);
        while i < left.len() && j < right.len() {
            if self.sorts_before(func, right[j], left[i])? {
                out.push(right[j]);
                j += 1;
            } else {
                out.push(left[i]);
                i += 1;
            }
        }
        out.extend_from_slice(&left[i..]);
        out.extend_from_slice(&right[j..]);
        Ok(out)
    }

    /// `sort(list, cmp)` — the comparator form of `sort`. The one-argument
    /// `sort(list)` never reaches here; see the dispatcher in `vm::native`.
    pub(super) fn builtin_sort_cmp(&mut self, args: &[Value]) -> Result<Value, String> {
        let [list, func] = args else {
            return Err("sort() expects 1 or 2 arguments (list, comparator)".into());
        };
        let Value::List(list_id) = *list else {
            return Err("sort() expects a list as first argument".into());
        };
        let elements = self.heap.get_list(list_id).to_vec();
        let sorted = self.merge_sort_by(elements, *func)?;
        Ok(Value::List(self.heap.alloc_list(sorted)))
    }

    /// `sort_by(list, key_fn)` / `sort_by(list, key_fn, descending)` — sort by a
    /// key extracted from each element, the shape a table view actually wants
    /// (`sort_by(rows, fn(r) r.score, true)`).
    ///
    /// The key function is called exactly **once per element**, before any
    /// comparison, so sorting 1,000 rows costs 1,000 calls rather than the
    /// ~10,000 a comparator would. Keys are compared with the same ordering
    /// `sort(list)` uses (numbers, then strings, then everything else).
    ///
    /// The third argument chooses the direction: `true` or `"desc"` for
    /// descending, `false`/`"asc"`/omitted for ascending. Descending is still
    /// stable — elements with equal keys keep their original order either way,
    /// which is what makes sorting a table by one column and then another
    /// produce the composed ordering.
    pub(super) fn builtin_sort_by(&mut self, args: &[Value]) -> Result<Value, String> {
        let (list, func, dir) = match args {
            [list, func] => (list, func, Value::Bool(false)),
            [list, func, dir] => (list, func, *dir),
            _ => {
                return Err("sort_by() expects 2 or 3 arguments (list, key_fn, descending)".into());
            }
        };
        let Value::List(list_id) = *list else {
            return Err("sort_by() expects a list as first argument".into());
        };
        let descending = match dir {
            Value::Bool(b) => b,
            Value::Nil => false,
            Value::String(sid) => match self.heap.get_string(sid) {
                "desc" | "descending" | "down" => true,
                "asc" | "ascending" | "up" => false,
                other => {
                    return Err(format!(
                        "sort_by() direction must be \"asc\" or \"desc\" (or a bool), got \"{other}\""
                    ));
                }
            },
            other => {
                return Err(format!(
                    "sort_by() direction must be a bool or \"asc\"/\"desc\", got {}",
                    other.type_name()
                ));
            }
        };

        let elements = self.heap.get_list(list_id).to_vec();
        let mut keyed: Vec<(crate::builtins::SortKey, Value)> = Vec::with_capacity(elements.len());
        for elem in elements {
            let key = self.call_closure_sync(*func, &[elem])?;
            keyed.push((crate::builtins::SortKey::of(self.heap, key), elem));
        }
        // `Vec::sort_by` is stable, and reversing the comparison keeps it so:
        // equal keys stay in their original relative order in both directions.
        if descending {
            keyed.sort_by(|(a, _), (b, _)| b.cmp(a));
        } else {
            keyed.sort_by(|(a, _), (b, _)| a.cmp(b));
        }
        let sorted: Vec<Value> = keyed.into_iter().map(|(_, v)| v).collect();
        Ok(Value::List(self.heap.alloc_list(sorted)))
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
