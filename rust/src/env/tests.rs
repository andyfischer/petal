//! Unit tests for the Env runtime, split out of `mod.rs` to keep that file
//! focused on the implementation. These reach into `Env` internals (private
//! fields, `ctx`, `collect_garbage`), so they live as a child module here
//! rather than as black-box integration tests under `tests/`.
//!
//! Each inner module imports the parent `env` module via `use super::super::*`
//! (`super` is this `tests` module; `super::super` is `env`).

mod call_function_tests {
    use super::super::*;

    /// Load+run a program and return (env, stack) ready for `call_function`.
    fn run(source: &str) -> (Env, StackKey) {
        let mut env = Env::new();
        let pid = env.load_program(source).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        env.take_output();
        (env, sid)
    }

    #[test]
    fn calls_named_function_with_args() {
        let (mut env, sid) = run("fn add(a, b)\n  a + b\nend\n");
        let result = env
            .call_function(sid, "add", &[Value::Int(3), Value::Int(4)])
            .unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn calls_named_lambda_binding() {
        let (mut env, sid) = run("let double = fn(x) -> x * 2\n");
        let result = env.call_function(sid, "double", &[Value::Int(21)]).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn resolves_overloaded_function_by_arity() {
        let source = "fn greet(name)\n  name\nend\nfn greet(first, last)\n  first + last\nend\n";
        let (mut env, sid) = run(source);
        let one = env.call_function(sid, "greet", &[Value::Int(1)]).unwrap();
        assert_eq!(one, Value::Int(1));
        let two = env
            .call_function(sid, "greet", &[Value::Int(2), Value::Int(3)])
            .unwrap();
        assert_eq!(two, Value::Int(5));
    }

    #[test]
    fn sees_top_level_state_captured_at_run_time() {
        // A function reads the value of a top-level `state` variable as it
        // stood when the program ran; repeated calls return it consistently.
        let source = "state base = 41\nfn next_val()\n  base + 1\nend\n";
        let (mut env, sid) = run(source);
        assert_eq!(
            env.call_function(sid, "next_val", &[]).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            env.call_function(sid, "next_val", &[]).unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn returns_string_value_via_heap() {
        let (mut env, sid) = run("fn shout(s)\n  s ++ \"!\"\nend\n");
        let arg = Value::String(env.heap_mut().alloc_string("hi".to_string()));
        let result = env.call_function(sid, "shout", &[arg]).unwrap();
        match result {
            Value::String(id) => assert_eq!(env.heap().get_string(id), "hi!"),
            other => panic!("expected string, got {:?}", other),
        }
    }

    #[test]
    fn unknown_function_is_an_error() {
        let (mut env, sid) = run("fn known()\n  1\nend\n");
        let err = env.call_function(sid, "missing", &[]).unwrap_err();
        assert!(err.contains("missing"), "unexpected error: {err}");
    }

    #[test]
    fn arity_mismatch_is_an_error() {
        let (mut env, sid) = run("fn add(a, b)\n  a + b\nend\n");
        let err = env.call_function(sid, "add", &[Value::Int(1)]).unwrap_err();
        assert!(
            err.contains("argument") || err.contains("expects"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn env_implements_debug_and_can_be_embedded() {
        // A host struct that embeds an Env should be able to derive Debug
        // (the motivation: unwrap_err/expect_err and logging in embedders).
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Host {
            env: Env,
            label: &'static str,
        }
        let host = Host {
            env: Env::new(),
            label: "demo",
        };
        let rendered = format!("{:?}", host);
        assert!(rendered.contains("Env"), "got: {rendered}");
        assert!(rendered.contains("native_fns"), "got: {rendered}");
    }

    #[test]
    fn push_output_buffer_round_trips_and_drains() {
        let mut env = Env::new();
        env.run_source(
            "let s = symbol(\"draw\")\n\
             push_output(s, 1)\n\
             push_output(s, 2)\n\
             push_output(s, 3)\n",
        )
        .unwrap();
        let sym = env.intern_symbol("draw");
        let drained = env.take_output_buffer(sym);
        assert_eq!(drained, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        // A second drain is empty — `take` leaves the buffer cleared.
        assert!(env.take_output_buffer(sym).is_empty());
    }

    #[test]
    fn output_buffer_values_survive_gc() {
        // A heap-backed value pushed into a buffer must survive a collection
        // triggered mid-run by other allocations. If buffers weren't GC roots,
        // the string would be swept and its contents corrupted.
        let mut env = Env::new();
        env.run_source(
            "let s = symbol(\"out\")\n\
             push_output(s, \"keep-me\")\n\
             let acc = 0\n\
             for i in range(0, 5000) do\n\
               let tmp = \"garbage\" ++ str(i)\n\
               acc = acc + len(tmp)\n\
             end\n",
        )
        .unwrap();
        let sym = env.intern_symbol("out");
        let drained = env.take_output_buffer(sym);
        assert_eq!(drained.len(), 1);
        match drained[0] {
            Value::String(id) => assert_eq!(env.heap().get_string(id), "keep-me"),
            other => panic!("expected string, got {:?}", other),
        }
    }

    #[test]
    fn binding_is_readable_from_script() {
        let mut env = Env::new();
        let sym = env.intern_symbol("dt");
        env.set_binding(sym, Value::Float(0.016));
        let result = env.run_source("binding(symbol(\"dt\"))").unwrap();
        assert_eq!(result, Value::Float(0.016));
    }

    #[test]
    fn binding_values_survive_gc() {
        // A heap-backed bound Value must survive a mid-run collection.
        let mut env = Env::new();
        let sym = env.intern_symbol("keys");
        let space = Value::String(env.heap_mut().alloc_string("space".to_string()));
        let list = Value::List(env.heap_mut().alloc_list(vec![space]));
        env.set_binding(sym, list);
        // Allocate enough to trip a GC, then read the binding back.
        env.run_source(
            "let acc = 0\n\
             for i in range(0, 5000) do\n\
               let tmp = \"g\" ++ str(i)\n\
               acc = acc + len(tmp)\n\
             end\n",
        )
        .unwrap();
        match env.binding(sym) {
            Some(Value::List(id)) => match env.heap().get_list(id)[0] {
                Value::String(s) => assert_eq!(env.heap().get_string(s), "space"),
                other => panic!("expected string, got {:?}", other),
            },
            other => panic!("expected list binding, got {:?}", other),
        }
    }

    #[test]
    fn counter_increments_and_resets() {
        let mut env = Env::new();
        let sym = env.intern_symbol("canvas_id");
        env.reset_counter(sym, 1);
        assert_eq!(env.next_counter(sym), 1);
        assert_eq!(env.next_counter(sym), 2);
        assert_eq!(env.next_counter(sym), 3);
        env.reset_counter(sym, 1);
        assert_eq!(env.next_counter(sym), 1);
    }

    #[test]
    fn functions_refreshed_after_transfer_state() {
        // Hot reload crosses the program-replacement seam, which invalidates the
        // VM's cached lowering; the refreshed function table must reflect the new
        // program.
        let mut env = Env::new();
        let pid = env.load_program("fn f()\n  1\nend\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.call_function(sid, "f", &[]).unwrap(), Value::Int(1));

        let new_program = env.compile_program(pid, "fn f()\n  2\nend\n").unwrap();
        env.transfer_state(sid, new_program).unwrap();
        // Before re-running, the stale table was cleared.
        assert!(env.call_function(sid, "f", &[]).is_err());
        env.run(sid).unwrap();
        assert_eq!(env.call_function(sid, "f", &[]).unwrap(), Value::Int(2));
    }
}

mod speculative_tests {
    use super::super::*;
    use crate::heap::ListId;

    /// The `let g = <state_var>` aliasing limitation noted in the
    /// speculative-execution plan no longer reproduces: a `let` alias of a
    /// state collection, mutated by (possibly nested) index assignment — at top
    /// level, inside an `if`, or written back into the state var across frames —
    /// behaves with correct value semantics. Pins that it stays fixed.
    #[test]
    fn let_alias_of_state_var_mutated_by_index_persists() {
        let mut env = Env::new();
        // Each frame: alias state `count`, bump g[0], write the alias back.
        let src = "state count = [0, 0, 0]\nlet g = count\ng[0] = g[0] + 1\ncount = g\n";
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        for expected in 1..=3 {
            env.reset_stack(sid).unwrap();
            env.run(sid).unwrap();
            env.take_output();
            let id = state_list(&env, sid);
            assert_eq!(
                env.heap().get_list(id),
                &[Value::Int(expected), Value::Int(0), Value::Int(0)],
                "let-alias index mutation did not persist across frames"
            );
        }
    }

    /// Pull the single state list out of a stack's committed state.
    fn state_list(env: &Env, sid: StackKey) -> ListId {
        let state = env.snapshot_state(sid).unwrap();
        match state.values().next().copied().unwrap() {
            Value::List(id) => id,
            other => panic!("expected a list state value, got {other:?}"),
        }
    }

    /// The core isolation guarantee of the speculative-execution plan: a
    /// speculative frame that "mutates" a state list must NOT disturb the
    /// object the committed state still points at. This now holds for free
    /// because collections are immutable — `items[0] = …` allocates a new list
    /// and rebinds rather than mutating the shared slot in place. Before the
    /// immutable-collections work this test would fail: the in-place SetIndex
    /// corrupted the committed list behind its id.
    #[test]
    fn speculative_run_does_not_corrupt_committed_state_objects() {
        let mut env = Env::new();
        let pid = env
            .load_program("state items = [1, 2, 3]\nitems[0] = items[0] + 100\n")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();

        // First committed frame: items -> [101, 2, 3].
        env.run(sid).unwrap();
        env.take_output();
        let committed = state_list(&env, sid);
        assert_eq!(
            env.heap().get_list(committed),
            &[Value::Int(101), Value::Int(2), Value::Int(3)]
        );

        // A speculative frame advances items to [201, 2, 3] then rolls back.
        env.run_speculative(sid).unwrap();
        env.take_output();

        // The committed list object is byte-for-byte intact…
        assert_eq!(
            env.heap().get_list(committed),
            &[Value::Int(101), Value::Int(2), Value::Int(3)],
            "speculative mutation leaked into the committed state list"
        );
        // …and committed state still resolves to the pre-speculation value.
        assert_eq!(
            env.heap().get_list(state_list(&env, sid)),
            &[Value::Int(101), Value::Int(2), Value::Int(3)]
        );

        // A subsequent *real* frame continues from the committed value,
        // proving the rollback restored state rather than dropping it. Driving
        // a fresh frame means resetting the stack first (as a host's per-frame
        // loop does), since `run_speculative` left it in a completed state.
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(
            env.heap().get_list(state_list(&env, sid)),
            &[Value::Int(201), Value::Int(2), Value::Int(3)]
        );
    }

    /// Now that `run_speculative` forks into its own context, a speculative
    /// frame leaves the source *entirely* untouched — including its print
    /// output. The old snapshot/restore implementation leaked the speculative
    /// run's prints into the shared output buffer; the fork-based one captures
    /// them in the fork's (discarded) context instead. This is strictly
    /// stronger than the heap-isolation guarantee above.
    #[test]
    fn speculative_run_does_not_leak_output_or_heap_into_source() {
        let mut env = Env::new();
        let pid = env
            .load_program("state items = [1, 2, 3]\nitems[0] = items[0] + 100\nprint(\"spec\")\n")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();

        // Real frame: items -> [101, 2, 3] and one "spec" line of output.
        env.run(sid).unwrap();
        let committed = state_list(&env, sid);
        assert_eq!(env.take_output(), vec!["spec".to_string()]);
        assert_eq!(
            env.heap().get_list(committed),
            &[Value::Int(101), Value::Int(2), Value::Int(3)]
        );

        // A speculative frame advances to [201, 2, 3] and prints "spec" again —
        // but inside the fork's own context.
        env.run_speculative(sid).unwrap();

        // The source's print output is NOT polluted by the speculative run.
        assert!(
            env.take_output().is_empty(),
            "speculative print leaked into the source's output buffer"
        );
        // The source heap object is byte-for-byte intact, and committed state
        // still resolves to the pre-speculation value.
        assert_eq!(
            env.heap().get_list(committed),
            &[Value::Int(101), Value::Int(2), Value::Int(3)],
            "speculative run disturbed the source heap"
        );
        assert_eq!(
            env.heap().get_list(state_list(&env, sid)),
            &[Value::Int(101), Value::Int(2), Value::Int(3)]
        );
    }
}

mod fork_tests {
    use super::super::*;
    use crate::heap::ListId;

    /// Pull the single state list out of a stack's committed state, resolved
    /// against that stack's own execution context's heap.
    fn state_list_in_ctx(env: &Env, sid: StackKey) -> (ContextKey, ListId) {
        let stack = env.stacks.get(&sid).unwrap();
        let id = match stack.state.values().next().copied().unwrap() {
            Value::List(id) => id,
            other => panic!("expected a list state value, got {other:?}"),
        };
        (stack.context, id)
    }

    /// Read a list's contents from a specific context's heap.
    fn list_in(env: &Env, ck: ContextKey, id: ListId) -> Vec<Value> {
        env.ctx(ck).heap.get_list(id).to_vec()
    }

    #[test]
    fn fork_runs_independently_and_leaves_source_untouched() {
        let mut env = Env::new();
        let src_program = "state items = [1, 2, 3]\nitems = append(items, 4)\n";
        let pid = env.load_program(src_program).unwrap();
        let src = env.create_stack(pid).unwrap();

        // Source committed frame: items -> [1, 2, 3, 4].
        env.run(src).unwrap();
        env.take_output();
        let (src_ck, src_id0) = state_list_in_ctx(&env, src);
        assert_eq!(
            list_in(&env, src_ck, src_id0),
            vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)],
        );

        // Fork into an isolated side-by-side copy.
        let fork = env.fork_execution(src).unwrap();
        assert_ne!(fork.0, src.0, "fork should get a fresh stack key");
        let fork_ck = env.stacks.get(&fork).unwrap().context;
        assert_ne!(fork_ck.0, src_ck.0, "fork should get a fresh context key");

        // The fork starts from the carried-over [1,2,3,4]. Running it appends
        // 4 again, advancing it to [1,2,3,4,4].
        env.reset_stack(fork).unwrap();
        env.run(fork).unwrap();
        let (fork_ck2, fork_id) = state_list_in_ctx(&env, fork);
        assert_eq!(fork_ck2.0, fork_ck.0);
        assert_eq!(
            list_in(&env, fork_ck2, fork_id),
            vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
                Value::Int(4),
                Value::Int(4)
            ],
            "fork's run should advance its own state",
        );

        // The SOURCE state list is unchanged by the fork's run.
        let (src_ck_after, src_id_after) = state_list_in_ctx(&env, src);
        assert_eq!(src_ck_after.0, src_ck.0);
        assert_eq!(
            list_in(&env, src_ck_after, src_id_after),
            vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)],
            "fork's run leaked into the source state",
        );

        // Advance the fork further; the source remains untouched, proving the
        // heaps are fully independent after the fork.
        env.reset_stack(fork).unwrap();
        env.run(fork).unwrap();
        let (_, fork_id2) = state_list_in_ctx(&env, fork);
        assert_eq!(
            list_in(&env, fork_ck, fork_id2),
            vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
                Value::Int(4),
                Value::Int(4),
                Value::Int(4)
            ],
        );
        let (_, src_id_final) = state_list_in_ctx(&env, src);
        assert_eq!(
            list_in(&env, src_ck, src_id_final),
            vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)],
            "the source state must stay frozen while the fork advances",
        );
    }

    /// Each context owns its own heap (the fork deep-clones it), so a GC cycle
    /// scoped to one context can never free an object that is live only in
    /// another. Make a list unreachable in the source context, collect *that*
    /// context's heap, and confirm the fork's copy is byte-for-byte intact.
    #[test]
    fn gc_in_one_context_does_not_free_objects_live_in_another() {
        let mut env = Env::new();
        let pid = env.load_program("state items = [10, 20, 30]\n").unwrap();
        let src = env.create_stack(pid).unwrap();
        env.run(src).unwrap();
        let (src_ck, list_id) = state_list_in_ctx(&env, src);

        // Fork — the fork's heap is an independent deep clone sharing the id.
        let fork = env.fork_execution(src).unwrap();
        let fork_ck = env.stacks.get(&fork).unwrap().context;
        assert_eq!(
            list_in(&env, fork_ck, list_id),
            vec![Value::Int(10), Value::Int(20), Value::Int(30)],
        );

        // Drop the list's only root in the SOURCE context, then GC just that
        // context. The slot becomes free in the source heap…
        env.restore_state(src, HashMap::new());
        env.collect_garbage(src_ck);

        // …but the fork's heap is a separate slot vector, untouched by the
        // source's sweep.
        assert_eq!(
            list_in(&env, fork_ck, list_id),
            vec![Value::Int(10), Value::Int(20), Value::Int(30)],
            "a GC scoped to the source context must not disturb the fork's heap",
        );
    }
}

/// Step 7 — the host/CLI/WASM surface: a host forks an execution, drives it,
/// drains its *own* output/draw-buffer/state, diffs it against the source, and
/// drops it. These exercise the context-aware `*_for` accessors and `drop_fork`
/// that make a fork's isolated context reachable (the default no-stack
/// accessors only reach the default context).
mod host_surface_tests {
    use super::super::*;
    use crate::native_fn::PetalCxt;

    /// A stand-in for a host draw-command native: emit one `rect` command into
    /// the buffer the host later drains. Models `petal-sdl`'s draw natives.
    fn emit_rect(cx: &mut PetalCxt) -> Result<u32, String> {
        let sym = cx.intern_symbol("draw_commands");
        cx.emit(sym, "rect", vec![Value::Int(7)]);
        cx.push_nil();
        Ok(1)
    }

    const FRAME: &str = "state n = 0\nn = n + 1\nprint(\"hi\")\nemit_rect()\n";

    /// The regression Step 7 fixes: a forked run's print output and draw-command
    /// buffer live in the fork's own context. `run_speculative` discards them
    /// (its whole point), so a host that needs them must drive the fork itself
    /// and drain it through the `*_for` accessors before `drop_fork`.
    #[test]
    fn host_drains_a_forks_output_and_draw_buffer_then_drops_it() {
        let mut env = Env::new();
        env.register_native("emit_rect", emit_rect);
        let pid = env.load_program(FRAME).unwrap();
        let src = env.create_stack(pid).unwrap();
        env.run(src).unwrap();
        let _ = env.take_output(); // drain the source's first committed frame

        let draw = env.intern_symbol("draw_commands");
        let _ = env.take_output_buffer(draw);

        // run_speculative isolates *and discards* the fork's side effects, so
        // nothing leaks back into the source's default context.
        env.run_speculative(src).unwrap();
        assert!(
            env.take_output().is_empty(),
            "a speculative run's print must not leak into the source sink",
        );
        assert!(
            env.output_buffer(draw).is_empty(),
            "a speculative run's draw commands must not leak into the source buffer",
        );

        // Driving the fork by hand is how the host keeps the output: fork → run
        // → drain via *_for → drop.
        let fork = env.fork_execution(src).unwrap();
        env.reset_stack(fork).unwrap();
        env.run(fork).unwrap();

        // The fork's print output is reachable through take_output_for, and is
        // absent from the source's own sink.
        assert_eq!(env.take_output_for(fork), vec!["hi".to_string()]);
        assert!(
            env.take_output().is_empty(),
            "the fork's output must not appear in the source sink",
        );

        // The fork's draw buffer is reachable, and its commands decode against
        // the fork's *own* heap (heap_for), not the default context's.
        let cmds = env.take_output_buffer_for(fork, draw);
        assert_eq!(cmds.len(), 1, "the fork emitted exactly one draw command");
        let heap = env.heap_for(fork).unwrap();
        match cmds[0] {
            Value::EnumVariant { tag, .. } => {
                assert_eq!(heap.get_string(tag), "rect");
            }
            other => panic!("expected an emitted draw command, got {other:?}"),
        }
        // The source/default context never saw the fork's draw command.
        assert!(env.output_buffer(draw).is_empty());

        // Releasing the fork removes its stack and its exclusively-owned context.
        let ctx_count_before = env.contexts.len();
        env.drop_fork(fork);
        assert!(
            env.stack(fork).is_none(),
            "drop_fork removes the fork's stack"
        );
        assert_eq!(
            env.contexts.len(),
            ctx_count_before - 1,
            "drop_fork releases the fork's exclusively-owned context",
        );
    }

    /// Two executions stay concurrently live and independent: advancing the fork
    /// changes only its own state, and `diff_state` reports the divergence by
    /// value (each side resolved against its own heap), omitting equal vars.
    #[test]
    fn diff_state_compares_two_live_executions_by_value() {
        let mut env = Env::new();
        let pid = env
            .load_program("state n = 0\nstate tag = \"x\"\nn = n + 1\n")
            .unwrap();
        let src = env.create_stack(pid).unwrap();
        env.run(src).unwrap(); // source: n = 1, tag = "x"
        let _ = env.take_output();

        let fork = env.fork_execution(src).unwrap();
        // Advance the fork two more frames: n = 3, tag unchanged.
        for _ in 0..2 {
            env.reset_stack(fork).unwrap();
            env.run(fork).unwrap();
        }

        // Source is frozen at n = 1 while the fork moved to n = 3.
        let diffs = env.diff_state(pid, src, fork);
        assert_eq!(diffs.len(), 1, "only `n` should differ; `tag` is equal");
        let d = &diffs[0];
        assert_eq!(d.name, "n");
        assert_eq!(d.source, Some(serde_json::json!(1)));
        assert_eq!(d.fork, Some(serde_json::json!(3)));

        // Both stacks are still live and usable after the diff.
        env.reset_stack(src).unwrap();
        env.run(src).unwrap(); // source advances to n = 2
        let diffs = env.diff_state(pid, src, fork);
        assert_eq!(diffs[0].source, Some(serde_json::json!(2)));
        assert_eq!(diffs[0].fork, Some(serde_json::json!(3)));

        env.drop_fork(fork);
    }

    /// A fork can be fed different host inputs than its source via the
    /// context-aware binding accessors, without disturbing the source binding.
    #[test]
    fn set_binding_for_feeds_a_fork_without_touching_the_source() {
        let mut env = Env::new();
        let pid = env.load_program("state n = 0\n").unwrap();
        let src = env.create_stack(pid).unwrap();
        let sym = env.intern_symbol("speed");
        env.set_binding(sym, Value::Int(1));

        let fork = env.fork_execution(src).unwrap();
        // The fork inherits the source's binding…
        assert_eq!(env.binding_for(fork, sym), Some(Value::Int(1)));
        // …and can be rebound independently.
        env.set_binding_for(fork, sym, Value::Int(99));
        assert_eq!(env.binding_for(fork, sym), Some(Value::Int(99)));
        assert_eq!(
            env.binding(sym),
            Some(Value::Int(1)),
            "rebinding the fork must not disturb the source binding",
        );

        env.drop_fork(fork);
    }
}

/// `run_bounded` — bounding a single run with a step budget so an in-process
/// host can stay in control of the thread (see idea-b4c98a38).
mod run_bounded_tests {
    use super::super::*;

    /// A program that finishes within the budget reports `Done` with the same
    /// value an unbounded `run` would produce.
    #[test]
    fn completes_within_budget() {
        let src = "let total = 0\nfor i in range(0, 50) do\n  total = total + i\nend\ntotal\n";
        let mut env = Env::new();
        let pid = env.load_program(src).unwrap();

        // Reference value from an ordinary unbounded run on its own stack.
        let reference = env.create_stack(pid).unwrap();
        let expected = env.run(reference).unwrap();

        let sid = env.create_stack(pid).unwrap();
        match env.run_bounded(sid, 1_000_000).unwrap() {
            RunOutcome::Done(val) => assert_eq!(val, expected),
            other => panic!("expected Done, got {:?}", other),
        }
    }

    /// A runaway script yields control instead of hanging the host. The budget
    /// is consumed exactly, and the host can keep re-entering (and keep getting
    /// `Yielded`) without ever blocking.
    #[test]
    fn yields_on_runaway_loop_instead_of_hanging() {
        let mut env = Env::new();
        let pid = env
            .load_program("state x = 0\nwhile true do\n  x = x + 1\nend\n")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();

        assert_eq!(
            env.run_bounded(sid, 100).unwrap(),
            RunOutcome::Yielded { steps: 100 },
        );
        // Re-entering a yielded runaway loop yields again — the host stays in
        // control across as many frames as it likes.
        assert!(matches!(
            env.run_bounded(sid, 100).unwrap(),
            RunOutcome::Yielded { .. },
        ));
    }

    /// Splitting a run across many tiny-budget `run_bounded` calls produces the
    /// same result as one unbounded `run`, and genuinely yields along the way.
    #[test]
    fn resuming_in_small_steps_matches_a_single_run() {
        let src = "let total = 0\nfor i in range(0, 50) do\n  total = total + i\nend\ntotal\n";
        let mut env = Env::new();
        let pid = env.load_program(src).unwrap();

        let reference = env.create_stack(pid).unwrap();
        let expected = env.run(reference).unwrap();

        let sid = env.create_stack(pid).unwrap();
        let mut yields = 0;
        let final_val = loop {
            match env.run_bounded(sid, 5).unwrap() {
                RunOutcome::Yielded { steps } => {
                    assert_eq!(steps, 5);
                    yields += 1;
                    assert!(yields < 10_000, "run never completed");
                }
                RunOutcome::Done(val) => break val,
            }
        };
        assert_eq!(final_val, expected);
        assert!(
            yields > 0,
            "a 5-step budget should have yielded at least once"
        );
    }

    /// `state` accumulated across a resumed run is identical to a single run:
    /// resumption does not restart the run (which would reset the loop) and the
    /// untouched-state sweep, which fires only on completion, keeps the state.
    #[test]
    fn state_survives_resumption() {
        let src = "state total = 0\nfor i in range(0, 20) do\n  total = total + 1\nend\n";
        let mut env = Env::new();
        let pid = env.load_program(src).unwrap();

        let reference = env.create_stack(pid).unwrap();
        env.run(reference).unwrap();
        let names = env.state_key_names(pid);
        let total_key = *names.iter().find(|(_, n)| n.as_str() == "total").unwrap().0;
        let expected_total = env.get_state(reference, total_key).unwrap();

        let sid = env.create_stack(pid).unwrap();
        while let RunOutcome::Yielded { .. } = env.run_bounded(sid, 3).unwrap() {}
        assert_eq!(env.get_state(sid, total_key).unwrap(), expected_total);
    }

    /// An error inside the budget surfaces as `Err`, exactly as `run` would.
    #[test]
    fn surfaces_errors_like_run() {
        let mut env = Env::new();
        let pid = env.load_program("let x = 1 / 0\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        assert!(env.run_bounded(sid, 1_000).is_err());
    }
}

/// Chunk A of the pending-values feature: the `Value::Pending` kind plus the
/// resource table living in `ExecutionContext`, driven by the deterministic
/// test-only builtins `__pending` / `__resolve` / `__reject`.
///
/// All `run_source` calls here share the Env's single default context, so the
/// resource table persists across separate runs — this is exactly the
/// "between-frame resolution" model the design relies on (the frame loop's
/// re-run picks up a value resolved since the previous frame).
mod pending_value_chunk_a_tests {
    use super::super::*;

    /// `__pending("k")` with no prior entry yields a fresh `Value::Pending`
    /// whose `type_name` is `"pending"`.
    #[test]
    fn pending_builtin_yields_pending_value() {
        let mut env = Env::new();
        let v = env.run_source("__pending(\"k\")\n").unwrap();
        assert!(
            matches!(v, Value::Pending(_)),
            "expected Pending, got {v:?}"
        );
        assert_eq!(v.type_name(), "pending");
    }

    /// After `__resolve("k", 42)`, a later `__pending("k")` returns the real
    /// value (`Int(42)`) — modelling a resource that landed between frames.
    #[test]
    fn resolve_then_pending_returns_ready_value() {
        let mut env = Env::new();
        // First fetch: loading -> a Pending value.
        let loading = env.run_source("__pending(\"k\")\n").unwrap();
        assert!(
            matches!(loading, Value::Pending(_)),
            "expected Pending, got {loading:?}"
        );
        // Resolve the entry, then re-fetch on a fresh run (same context).
        env.run_source("__resolve(\"k\", 42)\n").unwrap();
        let ready = env.run_source("__pending(\"k\")\n").unwrap();
        assert_eq!(ready, Value::Int(42));
    }

    /// After `__reject("k2", "boom")`, `__pending("k2")` still yields a
    /// `Value::Pending` (an errored resource is a pending-kind value; it does
    /// NOT return the resolved value the way a Ready entry would).
    #[test]
    fn reject_then_pending_stays_pending() {
        let mut env = Env::new();
        env.run_source("__reject(\"k2\", \"boom\")\n").unwrap();
        let v = env.run_source("__pending(\"k2\")\n").unwrap();
        assert!(
            matches!(v, Value::Pending(_)),
            "errored resource should still fetch as Pending, got {v:?}"
        );
    }

    /// Two `__pending` calls with the same key dedup to the same
    /// `PendingId` (the resource table is keyed by the hashed key).
    #[test]
    fn same_key_dedups_to_same_pending_id() {
        let mut env = Env::new();
        let a = env.run_source("__pending(\"dupe\")\n").unwrap();
        let b = env.run_source("__pending(\"dupe\")\n").unwrap();
        match (a, b) {
            (Value::Pending(ia), Value::Pending(ib)) => {
                assert_eq!(ia, ib, "same key must return the same PendingId");
            }
            other => panic!("expected two Pending values, got {other:?}"),
        }
    }

    /// A heap-backed resolved value (here a list) must survive a garbage
    /// collection: the resource table outlives any single run's stack, so its
    /// `Ready`/`Errored` payloads are GC roots. Without that rooting, the list
    /// slot is swept while the resource still references it and `__pending`
    /// returns a dangling id. (Regression: the earlier Int-only tests missed
    /// this because inline ints are never collected.)
    #[test]
    fn resolved_heap_value_survives_gc() {
        let mut env = Env::new();
        // Resolve with a list. After this run's stack is reset, the resource
        // table entry is the list's only root.
        env.run_source("__resolve(\"gk\", [1, 2, 3])\n").unwrap();
        // Force a collection on the default context. Pre-fix, this sweeps the
        // list slot.
        let ck = env.default_context;
        env.collect_garbage(ck);
        // Re-fetch: the Ready value must still be intact.
        let v = env.run_source("__pending(\"gk\")\n").unwrap();
        match v {
            Value::List(id) => assert_eq!(
                env.ctx(ck).heap.get_list(id).to_vec(),
                vec![Value::Int(1), Value::Int(2), Value::Int(3)],
                "resolved list must survive GC (resource table must be a GC root)",
            ),
            other => panic!("expected the resolved List to survive GC, got {other:?}"),
        }
    }
}

/// Chunk B of the pending-values feature: `Value::Pending` must be *absorbed*
/// through the primitive OPERATORS in `backend/ops.rs` (arithmetic, comparison,
/// unary not/negate, string concat/interpolation, field/index access), mirroring
/// the existing `Value::Dual` propagation pattern — plus the one hard-error
/// position (a Pending used as a map KEY).
///
/// These are TDD tests written BEFORE the implementation: they encode the
/// strict/non-strict table from `docs/dev/pending-values-plan.md`. They are
/// expected to FAIL against today's code, which either errors on a Pending
/// operand (arithmetic / `<` / negate / field / index) or — worse — silently
/// collapses it to a real value (comparison → `Bool`, `!` → `Bool`, concat and
/// interpolation → `String`). That silent collapse is precisely the SQL-NULL
/// footgun the design forbids.
///
/// Pendings are constructed with the deterministic test builtin `__pending("k")`
/// (Chunk A). All `run_source` calls on one `Env` share its default context, so
/// the same key deduplicates to the same `PendingId` across separate runs — this
/// is how the leftmost-wins assertion recovers the id of the left operand.
mod pending_operator_absorption_tests {
    use super::super::*;

    /// Extract the `PendingId` from a `Value::Pending`, panicking with a helpful
    /// message otherwise. Used both to assert absorption and to compare ids.
    fn expect_pending(v: &Value) -> crate::value::PendingId {
        match v {
            Value::Pending(id) => *id,
            other => panic!("expected a Pending value, got {other:?}"),
        }
    }

    /// Rule 1 — Arithmetic (`+ - * / %`) with a Pending operand yields that
    /// Pending. Covers Pending on the left, on the right, and both operands
    /// Pending (leftmost id wins). Today `p + 1` errors "Cannot add pending and
    /// int", so these fail at `unwrap`.
    #[test]
    fn arithmetic_absorbs_pending() {
        let mut env = Env::new();
        // Pending on the left.
        let left = env.run_source("let p = __pending(\"k\")\np + 1\n").unwrap();
        assert!(
            matches!(left, Value::Pending(_)),
            "p + 1 should be Pending, got {left:?}"
        );
        // Pending on the right.
        let right = env.run_source("let p = __pending(\"k\")\n1 + p\n").unwrap();
        assert!(
            matches!(right, Value::Pending(_)),
            "1 + p should be Pending, got {right:?}"
        );

        // Both operands Pending: the result is the LEFTMOST Pending. `__pending`
        // dedups by key across runs in this shared context, so a fresh fetch of
        // key "a" recovers the exact id the product must carry.
        let a = env.run_source("__pending(\"a\")\n").unwrap();
        let a_id = expect_pending(&a);
        let prod = env
            .run_source("let a = __pending(\"a\")\nlet b = __pending(\"b\")\na * b\n")
            .unwrap();
        assert_eq!(
            expect_pending(&prod),
            a_id,
            "a * b must be the leftmost Pending (a)"
        );
    }

    /// Rule 2 — Comparison (`== != < <= > >=`) with a Pending operand yields that
    /// Pending, and CRITICALLY never collapses to `Bool`. Today `p == 5` returns
    /// `Bool(true/false)` and `p != 5` returns `Bool(true)` — the SQL-NULL
    /// footgun — while `p < 5` errors "Cannot compare pending and int".
    #[test]
    fn comparison_absorbs_pending_never_bool() {
        let mut env = Env::new();

        let eq = env
            .run_source("let p = __pending(\"k\")\np == 5\n")
            .unwrap();
        assert!(
            !matches!(eq, Value::Bool(_)),
            "p == 5 must NOT collapse to a Bool (SQL-NULL footgun), got {eq:?}"
        );
        assert!(
            matches!(eq, Value::Pending(_)),
            "p == 5 should be Pending, got {eq:?}"
        );

        let ne = env
            .run_source("let p = __pending(\"k\")\np != 5\n")
            .unwrap();
        assert!(
            matches!(ne, Value::Pending(_)),
            "p != 5 should be Pending, got {ne:?}"
        );

        let lt = env.run_source("let p = __pending(\"k\")\np < 5\n").unwrap();
        assert!(
            matches!(lt, Value::Pending(_)),
            "p < 5 should be Pending, got {lt:?}"
        );
    }

    /// Rule 3 — Unary `!` (logical not) and `-` (negate) on a Pending yield that
    /// Pending. Today `!p` returns `Bool` and `-p` errors "Cannot negate
    /// pending". (Petal spells logical-not `!`, not `not`.)
    #[test]
    fn unary_not_and_negate_absorb_pending() {
        let mut env = Env::new();

        let notted = env.run_source("let p = __pending(\"k\")\n!p\n").unwrap();
        assert!(
            !matches!(notted, Value::Bool(_)),
            "!p must NOT collapse to a Bool, got {notted:?}"
        );
        assert!(
            matches!(notted, Value::Pending(_)),
            "!p should be Pending, got {notted:?}"
        );

        let negated = env.run_source("let p = __pending(\"k\")\n-p\n").unwrap();
        assert!(
            matches!(negated, Value::Pending(_)),
            "-p should be Pending, got {negated:?}"
        );
    }

    /// Rule 4 — String concatenation (`++`) and interpolation (`"hi {p}"`, which
    /// lowers to concat) with a Pending part make the whole string Pending.
    /// Today both return a real `String` (the Pending rendered into text).
    #[test]
    fn string_concat_and_interpolation_absorb_pending() {
        let mut env = Env::new();

        let concatenated = env
            .run_source("let p = __pending(\"k\")\n\"hi \" ++ p\n")
            .unwrap();
        assert!(
            !matches!(concatenated, Value::String(_)),
            "\"hi \" ++ p must NOT collapse to a String, got {concatenated:?}"
        );
        assert!(
            matches!(concatenated, Value::Pending(_)),
            "\"hi \" ++ p should be Pending, got {concatenated:?}"
        );

        let interpolated = env
            .run_source("let p = __pending(\"k\")\n\"hi {p}\"\n")
            .unwrap();
        assert!(
            matches!(interpolated, Value::Pending(_)),
            "interpolation \"hi {{p}}\" should be Pending, got {interpolated:?}"
        );
    }

    /// Rule 5 — Field access (`p.name`) and index access (`p[0]`) with a Pending
    /// BASE yield that Pending. Today both error ("Cannot access field 'name' on
    /// pending" / "Cannot index pending with int"). (This is the Pending-base
    /// case only; an element-wise resolved collection containing a Pending is a
    /// later chunk.)
    #[test]
    fn field_and_index_on_pending_base_absorb_pending() {
        let mut env = Env::new();

        let field = env
            .run_source("let p = __pending(\"k\")\np.name\n")
            .unwrap();
        assert!(
            matches!(field, Value::Pending(_)),
            "p.name should be Pending, got {field:?}"
        );

        let index = env.run_source("let p = __pending(\"k\")\np[0]\n").unwrap();
        assert!(
            matches!(index, Value::Pending(_)),
            "p[0] should be Pending, got {index:?}"
        );
    }

    /// Rule 6 — A Pending in a map KEY position is a HARD runtime error, never
    /// absorbed (an absorbed key would silently corrupt structure). Petal has no
    /// computed-map-key literal syntax, so the reachable key-position construct
    /// is index-assignment `m[p] = 1`; this run must return `Err`.
    ///
    /// NOTE: unlike the other cases this already errors today (Petal rejects
    /// index-assignment on records outright), so it currently PASSES as a guard
    /// rather than failing — see the module's fail report. It pins that whichever
    /// map-key path exists, a Pending key stays a hard error under Chunk B.
    #[test]
    fn pending_map_key_is_hard_error() {
        let mut env = Env::new();
        let result = env.run_source("let p = __pending(\"k\")\nlet m = {}\nm[p] = 1\n");
        assert!(
            result.is_err(),
            "a Pending map key must be a hard error, got {result:?}"
        );
    }
}

/// Chunk C of the pending-values feature: NATIVE-FN classification. Every native
/// call flows through `call_native_or_intrinsic`; before the native runs, its
/// argument list is scanned for a `Value::Pending`, and if one is present the
/// native's class decides the outcome:
///
///   * Strict (default)  -> return the LEFTMOST Pending arg without calling.
///   * Effectful (print / draw emitters) -> no-op: return Nil, emit nothing.
///   * AllowPending (`__pending`/`__resolve`/`__reject`) -> call normally.
///
/// This mirrors the strict/non-strict table in `docs/dev/pending-values-plan.md`:
/// arithmetic/math builtins, `str`, `map`, etc. absorb a Pending operand, while
/// effectful natives silently no-op (the frame emits no command).
///
/// These are TDD tests written BEFORE the Chunk C implementation. They FAIL
/// against today's code, which has no classification layer:
///   - `sqrt(__pending("k"))` errors "Expected float at arg 1, got pending"
///     (want: Pending),
///   - `str(__pending("k"))` returns the String "<pending>" (want: Pending),
///   - `map(__pending("k"), fn)` errors "map() expects a list as first
///     argument" (the collection base is arg[0]; want: Pending),
///   - `print(__pending("k"))` prints "<pending>" (want: NO output, run ok).
/// The two guards (`map` over a real list; `sqrt` after `__resolve`) already
/// pass today and pin that classification must not disturb the no-Pending path.
///
/// Pendings are built with the deterministic Chunk A test builtin `__pending`.
/// All `run_source` calls on one `Env` share its default context, so a resolve
/// in one run is visible to a later run (the between-frame resolution model).
mod pending_native_classification_tests {
    use super::super::*;

    /// Pull the `PendingId` out of a `Value::Pending`, or panic helpfully.
    fn expect_pending(v: &Value) -> crate::value::PendingId {
        match v {
            Value::Pending(id) => *id,
            other => panic!("expected a Pending value, got {other:?}"),
        }
    }

    /// A Strict math native (`sqrt`) absorbs a Pending argument: it returns the
    /// Pending itself, NOT a Float and NOT an error. Today `sqrt(pending)`
    /// errors "Expected float at arg 1, got pending", so this fails at `unwrap`.
    #[test]
    fn strict_math_native_absorbs_pending() {
        let mut env = Env::new();
        let v = env.run_source("sqrt(__pending(\"k\"))\n").unwrap();
        assert!(
            matches!(v, Value::Pending(_)),
            "sqrt(pending) should absorb to Pending, got {v:?}"
        );
    }

    /// `str` is Strict too: `str(pending)` yields the Pending, never a rendered
    /// String. Today `str(pending)` returns String("<pending>"), so this fails.
    #[test]
    fn strict_str_native_absorbs_pending() {
        let mut env = Env::new();
        let v = env.run_source("str(__pending(\"k\"))\n").unwrap();
        assert!(
            !matches!(v, Value::String(_)),
            "str(pending) must NOT collapse to a String, got {v:?}"
        );
        assert!(
            matches!(v, Value::Pending(_)),
            "str(pending) should absorb to Pending, got {v:?}"
        );
    }

    /// A higher-order intrinsic (`map`) over a Pending BASE absorbs: the base is
    /// arg[0], a Pending, so the whole call returns that Pending (the closure
    /// never runs). The returned id must be exactly the base's id (leftmost
    /// wins; here it is the only Pending). Today `map(pending, fn)` errors
    /// "map() expects a list as first argument", so this fails at `unwrap`.
    #[test]
    fn map_over_pending_base_absorbs_pending() {
        let mut env = Env::new();
        // `__pending` dedups by key across runs in this shared context, so a
        // fresh fetch of "k" recovers the id the map result must carry.
        let base = env.run_source("__pending(\"k\")\n").unwrap();
        let base_id = expect_pending(&base);
        let mapped = env
            .run_source("map(__pending(\"k\"), fn(x) -> x)\n")
            .unwrap();
        assert_eq!(
            expect_pending(&mapped),
            base_id,
            "map over a Pending base must return that Pending"
        );
    }

    /// An Effectful native (`print`) no-ops on a Pending argument: the run
    /// succeeds and NOTHING is printed. Today `print(pending)` writes the line
    /// "<pending>" to the output buffer, so `take_output()` is non-empty and
    /// this assertion fails.
    #[test]
    fn effectful_print_noops_on_pending() {
        let mut env = Env::new();
        env.run_source("print(__pending(\"k\"))\n").unwrap();
        let printed = env.take_output();
        assert!(
            printed.is_empty(),
            "print(pending) must emit nothing (effectful no-op), got {printed:?}"
        );
    }

    /// Regression guard: classification must not touch the no-Pending path.
    /// `map` over a RESOLVED list runs normally and returns the mapped list —
    /// arg[0] is a List, not a Pending, so there is no interception (the
    /// element-wise rule stays intact). This passes today and must keep passing.
    #[test]
    fn map_over_real_list_is_unaffected() {
        let mut env = Env::new();
        let v = env.run_source("map([1, 2, 3], fn(x) -> x * 2)\n").unwrap();
        let ck = env.default_context;
        match v {
            Value::List(id) => assert_eq!(
                env.ctx(ck).heap.get_list(id).to_vec(),
                vec![Value::Int(2), Value::Int(4), Value::Int(6)],
                "map over a real list must still compute element-wise"
            ),
            other => panic!("expected a List result, got {other:?}"),
        }
    }

    /// Guard: absorption only applies WHILE pending. After `__resolve("k", 9.0)`
    /// the resource is Ready, so `__pending("k")` returns the real Float(9.0) and
    /// `sqrt` computes normally (3.0) — proving the Strict interception keys off
    /// the Pending value, not off the call site. Passes today; pins that the
    /// classification early-out is skipped when no arg is Pending.
    #[test]
    fn resolved_value_computes_normally_through_strict_native() {
        let mut env = Env::new();
        let v = env
            .run_source("__resolve(\"k\", 9.0)\nsqrt(__pending(\"k\"))\n")
            .unwrap();
        assert_eq!(v, Value::Float(3.0), "sqrt of a resolved 9.0 must be 3.0");
    }

    /// Bypass regression: the in-place mutating path must absorb too. `append`
    /// is a Strict mutating builtin; with the last-use in-place optimizer on (the
    /// default), `append(xs, pending)` routes through `call_native_fn_in_place`,
    /// NOT `call_native_or_intrinsic`. Absorption must still happen — otherwise
    /// the Pending is physically appended and semantics diverge purely from the
    /// optimizer. Interception lives at the shared leaf to cover this.
    #[test]
    fn in_place_mutating_native_absorbs_pending() {
        let mut env = Env::new();
        // `xs` is dead after the append, so the optimizer picks the in-place path.
        let v = env
            .run_source("let xs = [1, 2]\nappend(xs, __pending(\"k\"))\n")
            .unwrap();
        assert!(
            matches!(v, Value::Pending(_)),
            "append(xs, pending) must absorb to Pending regardless of the in-place \
             optimization, got {v:?}"
        );
    }

    /// Bypass regression: a native stored as a record field and invoked via
    /// method syntax (`r.f(pending)`) dispatches through the record-field path,
    /// which also bottoms out at the shared leaf. A Strict native there must
    /// absorb, and an Effectful one must no-op.
    #[test]
    fn record_field_native_call_absorbs_pending() {
        let mut env = Env::new();
        let v = env
            .run_source("let r = { f: sqrt }\nr.f(__pending(\"k\"))\n")
            .unwrap();
        assert!(
            matches!(v, Value::Pending(_)),
            "r.f(pending) via a record-field native must absorb to Pending, got {v:?}"
        );
        // And the Effectful case: a print stored on a record must still no-op.
        let mut env2 = Env::new();
        env2.run_source("let r = { p: print }\nr.p(__pending(\"k\"))\n")
            .unwrap();
        assert!(
            env2.take_output().is_empty(),
            "r.p(pending) for an Effectful native must emit nothing"
        );
    }
}

/// Chunk D of the pending-values feature: the AllowPending META builtins that are
/// the ONLY sanctioned way to inspect pending-ness (everything else absorbs).
/// All of these must be registered `NativeClass::AllowPending` — a Strict
/// registration would let a Pending argument be absorbed before the builtin ever
/// ran, so the function could never see it. See the classification note in
/// `docs/dev/pending-values-plan.md`.
///
/// Builtins under test:
///   - `is_loading(x)`   -> Bool: true iff x is Pending + Loading.
///   - `is_error(x)`     -> Bool: true iff x is Pending + Errored.
///   - `is_pending(x)`   -> Bool: true iff x is a Pending at all (Loading OR Errored).
///   - `is_ready(x)`     -> Bool: true iff x is NOT a Pending (== !is_pending).
///   - `error_of(x)`     -> Value: the stored error if Pending + Errored, else Nil.
///   - `or_else(x, d)`   -> Value: `d` if x is Pending (loading OR errored), else x.
///   - `resource_key(x)` -> Value: the resource cache key as an Int if x is Pending,
///                          else Nil; equal keys for two pendings of the same key.
///
/// These are TDD tests written BEFORE the Chunk D implementation. They are
/// expected to FAIL against today's code, which has none of these builtins — so
/// each `run_source` errors with an unknown-identifier/compile error and the
/// `unwrap` panics. `Env` shares its default context across `run_source` calls,
/// so a `__resolve`/`__reject` in one run is visible to a later `__pending` (the
/// between-frame resolution model). A `Ready` resource never surfaces as a
/// `Value::Pending` (`__pending` returns the real value once Ready), so the
/// "resolved" cases below fetch the real value directly.
mod pending_meta_builtins_tests {
    use super::super::*;

    /// Read the heap string behind a `Value::String`, panicking otherwise.
    fn expect_string(env: &Env, v: Value) -> String {
        match v {
            Value::String(id) => env.ctx(env.default_context).heap.get_string(id).to_string(),
            other => panic!("expected a String, got {other:?}"),
        }
    }

    /// `is_loading` is true for a loading `__pending`, false once the resource is
    /// resolved (a Ready resource fetches as its real value, not a Pending), and
    /// false for a plain value.
    #[test]
    fn is_loading_true_only_for_loading_pending() {
        let mut env = Env::new();
        let loading = env.run_source("is_loading(__pending(\"k\"))\n").unwrap();
        assert_eq!(
            loading,
            Value::Bool(true),
            "is_loading(loading pending) should be true"
        );

        // After resolve, `__pending("k")` returns the real Int(7), not a Pending.
        let resolved = env
            .run_source("__resolve(\"k\", 7)\nis_loading(__pending(\"k\"))\n")
            .unwrap();
        assert_eq!(
            resolved,
            Value::Bool(false),
            "is_loading(resolved value) should be false"
        );

        let plain = env.run_source("is_loading(5)\n").unwrap();
        assert_eq!(plain, Value::Bool(false), "is_loading(5) should be false");
    }

    /// `is_error` is true for an errored resource (rejected then fetched via
    /// `__pending`), false for a still-loading pending, and false for a plain
    /// value.
    #[test]
    fn is_error_true_only_for_errored_pending() {
        let mut env = Env::new();
        let errored = env
            .run_source("__reject(\"e\", \"boom\")\nis_error(__pending(\"e\"))\n")
            .unwrap();
        assert_eq!(
            errored,
            Value::Bool(true),
            "is_error(errored pending) should be true"
        );

        let loading = env.run_source("is_error(__pending(\"k\"))\n").unwrap();
        assert_eq!(
            loading,
            Value::Bool(false),
            "is_error(loading pending) should be false"
        );

        let plain = env.run_source("is_error(5)\n").unwrap();
        assert_eq!(plain, Value::Bool(false), "is_error(5) should be false");
    }

    /// `is_pending` and `is_ready` are complementary: on a loading pending
    /// pending=true/ready=false; on a plain value pending=false/ready=true; and
    /// after resolve the value fetches real so pending=false/ready=true.
    #[test]
    fn is_pending_and_is_ready_are_complementary() {
        let mut env = Env::new();

        // Loading pending: pending, not ready.
        let p_loading = env.run_source("is_pending(__pending(\"k\"))\n").unwrap();
        let r_loading = env.run_source("is_ready(__pending(\"k\"))\n").unwrap();
        assert_eq!(
            p_loading,
            Value::Bool(true),
            "is_pending(loading) should be true"
        );
        assert_eq!(
            r_loading,
            Value::Bool(false),
            "is_ready(loading) should be false"
        );

        // Plain value: not pending, ready.
        let p_plain = env.run_source("is_pending(5)\n").unwrap();
        let r_plain = env.run_source("is_ready(5)\n").unwrap();
        assert_eq!(p_plain, Value::Bool(false), "is_pending(5) should be false");
        assert_eq!(r_plain, Value::Bool(true), "is_ready(5) should be true");

        // After resolve, `__pending` returns the real value: not pending, ready.
        let p_resolved = env
            .run_source("__resolve(\"k\", 7)\nis_pending(__pending(\"k\"))\n")
            .unwrap();
        let r_resolved = env.run_source("is_ready(__pending(\"k\"))\n").unwrap();
        assert_eq!(
            p_resolved,
            Value::Bool(false),
            "is_pending(resolved) should be false"
        );
        assert_eq!(
            r_resolved,
            Value::Bool(true),
            "is_ready(resolved) should be true"
        );
    }

    /// `error_of` returns the stored error value for an errored resource, and Nil
    /// for a loading pending and for a plain value.
    #[test]
    fn error_of_returns_error_value_else_nil() {
        let mut env = Env::new();
        let err = env
            .run_source("__reject(\"e\", \"boom\")\nerror_of(__pending(\"e\"))\n")
            .unwrap();
        assert_eq!(
            expect_string(&env, err),
            "boom",
            "error_of(errored) should be the error value"
        );

        let loading = env.run_source("error_of(__pending(\"k\"))\n").unwrap();
        assert_eq!(
            loading,
            Value::Nil,
            "error_of(loading pending) should be Nil"
        );

        let plain = env.run_source("error_of(5)\n").unwrap();
        assert_eq!(plain, Value::Nil, "error_of(5) should be Nil");
    }

    /// `or_else(x, default)` returns `default` when x is Pending (loading OR
    /// errored) and returns x when it is a plain/resolved value. Both args are
    /// eagerly evaluated (this is NOT the short-circuit `??` of Chunk E).
    #[test]
    fn or_else_falls_back_only_on_pending() {
        let mut env = Env::new();

        // Loading pending -> default.
        let loading = env.run_source("or_else(__pending(\"k\"), 99)\n").unwrap();
        assert_eq!(loading, Value::Int(99), "or_else(loading, 99) should be 99");

        // Plain value -> the value itself.
        let plain = env.run_source("or_else(42, 99)\n").unwrap();
        assert_eq!(plain, Value::Int(42), "or_else(42, 99) should be 42");

        // Resolved value -> the real value (fetches non-pending).
        let resolved = env
            .run_source("__resolve(\"k\", 7)\nor_else(__pending(\"k\"), 99)\n")
            .unwrap();
        assert_eq!(
            resolved,
            Value::Int(7),
            "or_else(resolved 7, 99) should be 7"
        );

        // Errored pending -> default.
        let errored = env
            .run_source("__reject(\"e\", \"boom\")\nor_else(__pending(\"e\"), 99)\n")
            .unwrap();
        assert_eq!(errored, Value::Int(99), "or_else(errored, 99) should be 99");
    }

    /// `resource_key` returns an Int cache key for a Pending (equal for two
    /// pendings of the same key) and Nil for a plain value.
    #[test]
    fn resource_key_is_stable_int_for_pending_else_nil() {
        let mut env = Env::new();

        let a = env
            .run_source("resource_key(__pending(\"same\"))\n")
            .unwrap();
        let b = env
            .run_source("resource_key(__pending(\"same\"))\n")
            .unwrap();
        assert!(
            matches!(a, Value::Int(_)),
            "resource_key(pending) should be an Int, got {a:?}"
        );
        assert!(
            matches!(b, Value::Int(_)),
            "resource_key(pending) should be an Int, got {b:?}"
        );
        assert_eq!(
            a, b,
            "two pendings for the same key must have equal resource_key"
        );

        let plain = env.run_source("resource_key(5)\n").unwrap();
        assert_eq!(plain, Value::Nil, "resource_key(5) should be Nil");
    }
}

/// Chunk E — the `??` coalescing operator: `x ?? y` yields `y` when `x` is
/// `nil` or a `Pending` (loading OR errored), otherwise `x`; the RHS is
/// evaluated only when the LHS is absent (short-circuit). Precedence sits
/// between comparison (looser) and concat (tighter).
mod pending_coalesce_operator_chunk_e_tests {
    use super::super::*;

    /// nil falls back to the RHS; a present value wins over it.
    #[test]
    fn nil_falls_back_present_value_wins() {
        let mut env = Env::new();
        assert_eq!(env.run_source("nil ?? 5\n").unwrap(), Value::Int(5));
        assert_eq!(env.run_source("3 ?? 5\n").unwrap(), Value::Int(3));
    }

    /// A loading pending falls back; once resolved, the real value wins.
    #[test]
    fn loading_pending_falls_back_then_resolved_wins() {
        let mut env = Env::new();
        assert_eq!(
            env.run_source("__pending(\"k\") ?? 5\n").unwrap(),
            Value::Int(5),
            "a loading pending must fall back to the RHS"
        );
        env.run_source("__resolve(\"k\", 7)\n").unwrap();
        assert_eq!(
            env.run_source("__pending(\"k\") ?? 5\n").unwrap(),
            Value::Int(7),
            "once resolved, the real value wins over the fallback"
        );
    }

    /// An errored pending also falls back (absent = nil OR pending, either state).
    #[test]
    fn errored_pending_falls_back() {
        let mut env = Env::new();
        env.run_source("__reject(\"e\", \"boom\")\n").unwrap();
        assert_eq!(
            env.run_source("__pending(\"e\") ?? 42\n").unwrap(),
            Value::Int(42),
            "an errored pending must fall back to the RHS"
        );
    }

    /// `??` is NOT `||`: present-but-falsy LHS values (false, 0, "") win over
    /// the RHS — only nil and pending fall back.
    #[test]
    fn present_but_falsy_lhs_is_kept() {
        let mut env = Env::new();
        assert_eq!(
            env.run_source("0 ?? 5\n").unwrap(),
            Value::Int(0),
            "0 is present"
        );
        assert_eq!(
            env.run_source("false ?? 9\n").unwrap(),
            Value::Bool(false),
            "false is present"
        );
        let empty = env.run_source("\"\" ?? \"x\"\n").unwrap();
        match empty {
            Value::String(id) => assert_eq!(
                env.ctx(env.default_context).heap.get_string(id),
                "",
                "empty string is present"
            ),
            other => panic!("expected empty String, got {other:?}"),
        }
    }

    /// Short-circuit: when the LHS is present, the RHS is NOT evaluated, so its
    /// side effect (a print) never runs.
    #[test]
    fn present_lhs_short_circuits_rhs_side_effect() {
        let mut env = Env::new();
        let v = env.run_source("1 ?? print(\"boom\")\n").unwrap();
        assert_eq!(v, Value::Int(1));
        assert!(
            env.take_output().is_empty(),
            "present LHS must short-circuit — the RHS print must not run"
        );
    }

    /// Short-circuit: when the LHS is absent (nil), the RHS IS evaluated.
    #[test]
    fn absent_lhs_evaluates_rhs_side_effect() {
        let mut env = Env::new();
        env.run_source("nil ?? print(\"run\")\n").unwrap();
        assert_eq!(
            env.take_output(),
            vec!["run".to_string()],
            "absent LHS must evaluate the RHS side effect"
        );
    }

    /// Precedence: `??` binds TIGHTER than comparison, so `3 ?? 0 > 5` parses as
    /// `(3 ?? 0) > 5` == false. A present LHS (3) discriminates the two parses:
    /// the wrong `3 ?? (0 > 5)` would yield Int(3).
    #[test]
    fn coalesce_binds_tighter_than_comparison() {
        let mut env = Env::new();
        assert_eq!(
            env.run_source("3 ?? 0 > 5\n").unwrap(),
            Value::Bool(false),
            "must parse as (3 ?? 0) > 5"
        );
        assert_eq!(
            env.run_source("nil ?? 10 > 5\n").unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            env.run_source("nil ?? 0 > 5\n").unwrap(),
            Value::Bool(false)
        );
    }

    /// Precedence: `++` (concat) binds TIGHTER than `??`. `"a" ++ p ?? "y"`
    /// parses as `("a" ++ p) ?? "y"`; the concat absorbs the pending so the whole
    /// LHS is pending and falls back to "y" (the wrong parse would give "ay").
    #[test]
    fn concat_binds_tighter_than_coalesce() {
        let mut env = Env::new();
        let v = env
            .run_source("\"a\" ++ __pending(\"k\") ?? \"y\"\n")
            .unwrap();
        match v {
            Value::String(id) => assert_eq!(
                env.ctx(env.default_context).heap.get_string(id),
                "y",
                "must parse as (\"a\" ++ p) ?? \"y\" and fall back to \"y\""
            ),
            other => panic!("expected String \"y\", got {other:?}"),
        }
    }

    /// Left-associative: `a ?? b ?? c` is `((a ?? b) ?? c)` — the first present
    /// value wins.
    #[test]
    fn coalesce_is_left_associative() {
        let mut env = Env::new();
        assert_eq!(env.run_source("nil ?? nil ?? 3\n").unwrap(), Value::Int(3));
        assert_eq!(env.run_source("nil ?? 2 ?? 3\n").unwrap(), Value::Int(2));
    }

    /// Lexer edge: a single-`?` predicate identifier still lexes as one
    /// identifier, and `??` still tokenizes next to it.
    #[test]
    fn single_question_identifier_preserved_alongside_coalesce() {
        let mut env = Env::new();
        let v = env.run_source("let ok? = nil\nok? ?? 5\n").unwrap();
        assert_eq!(
            v,
            Value::Int(5),
            "ok? is one identifier; ok? ?? 5 falls back to 5"
        );
    }
}

/// Chunk F — control-flow conditions absorb a Pending. `if`/`while` with a
/// Pending condition run NEITHER branch / ZERO iterations, and the whole
/// expression evaluates to that Pending rather than treating it as truthy.
mod pending_control_flow_chunk_f_tests {
    use super::super::*;

    fn expect_pending(v: &Value) -> crate::value::PendingId {
        match v {
            Value::Pending(id) => *id,
            other => panic!("expected a Pending value, got {other:?}"),
        }
    }

    /// `if <pending> then A else B` runs neither arm and evaluates to the
    /// Pending itself. Today a Pending is truthy, so the `then` arm runs and
    /// this returns 1.
    #[test]
    fn if_with_pending_condition_absorbs() {
        let mut env = Env::new();
        let cond = env.run_source("__pending(\"c\")\n").unwrap();
        let cond_id = expect_pending(&cond);
        let v = env
            .run_source("if __pending(\"c\") then 1 else 2 end\n")
            .unwrap();
        assert_eq!(
            expect_pending(&v),
            cond_id,
            "if on a pending condition must evaluate to that pending"
        );
    }

    /// Neither branch of a pending `if` executes its side effects.
    #[test]
    fn if_with_pending_condition_runs_no_branch() {
        let mut env = Env::new();
        env.run_source("if __pending(\"c\") then print(\"then\") else print(\"else\") end\n")
            .unwrap();
        assert!(
            env.take_output().is_empty(),
            "no branch of a pending if may run"
        );
    }

    /// Once the condition resolves, the `if` selects the real branch normally.
    #[test]
    fn if_resolves_to_branch_once_ready() {
        let mut env = Env::new();
        env.run_source("__resolve(\"c\", true)\n").unwrap();
        assert_eq!(
            env.run_source("if __pending(\"c\") then 1 else 2 end\n")
                .unwrap(),
            Value::Int(1),
            "a resolved-true condition takes the then branch"
        );
    }

    /// `while <pending>` runs zero iterations and evaluates to the Pending.
    #[test]
    fn while_with_pending_condition_absorbs_and_runs_zero_iterations() {
        let mut env = Env::new();
        let cond = env.run_source("__pending(\"w\")\n").unwrap();
        let cond_id = expect_pending(&cond);
        let v = env
            .run_source("while __pending(\"w\") do print(\"body\") end\n")
            .unwrap();
        assert!(
            env.take_output().is_empty(),
            "a pending while condition must run zero iterations"
        );
        assert_eq!(
            expect_pending(&v),
            cond_id,
            "a pending while must evaluate to that pending"
        );
    }

    /// A while whose pending condition resolves to false runs zero iterations
    /// but is NOT pending — it terminates normally.
    #[test]
    fn while_resolved_false_is_not_pending() {
        let mut env = Env::new();
        env.run_source("__resolve(\"w\", false)\n").unwrap();
        let v = env
            .run_source("while __pending(\"w\") do print(\"body\") end\n")
            .unwrap();
        assert!(
            !matches!(v, Value::Pending(_)),
            "a resolved-false while is not pending"
        );
        assert!(env.take_output().is_empty());
    }
}

/// Chunk G — `for` over a Pending iterable and `match` on a Pending subject
/// both absorb: zero iterations / no arm runs, and the expression evaluates to
/// that Pending. A wildcard `match` arm does NOT catch a Pending — the absorb
/// happens before any arm is tested.
mod pending_control_flow_chunk_g_tests {
    use super::super::*;

    fn expect_pending(v: &Value) -> crate::value::PendingId {
        match v {
            Value::Pending(id) => *id,
            other => panic!("expected a Pending value, got {other:?}"),
        }
    }

    /// `for x in <pending>` runs zero iterations and yields the Pending. Today
    /// it errors "Cannot iterate over pending".
    #[test]
    fn for_over_pending_absorbs_and_runs_zero_iterations() {
        let mut env = Env::new();
        let iter = env.run_source("__pending(\"k\")\n").unwrap();
        let iter_id = expect_pending(&iter);
        let v = env
            .run_source("for x in __pending(\"k\") do print(x) end\n")
            .unwrap();
        assert!(
            env.take_output().is_empty(),
            "a pending iterable must run zero iterations"
        );
        assert_eq!(
            expect_pending(&v),
            iter_id,
            "for over a pending iterable must evaluate to that pending"
        );
    }

    /// Once the iterable resolves to a real list, the loop runs normally.
    #[test]
    fn for_resolves_to_list_and_iterates() {
        let mut env = Env::new();
        env.run_source("__resolve(\"k\", [1, 2, 3])\n").unwrap();
        env.run_source("for x in __pending(\"k\") do print(x) end\n")
            .unwrap();
        assert_eq!(env.take_output(), vec!["1", "2", "3"]);
    }

    /// `match <pending>` runs no arm and yields the Pending. Today the pending
    /// matches no concrete arm and hits `MatchFail` (a runtime error).
    #[test]
    fn match_on_pending_absorbs() {
        let mut env = Env::new();
        let subj = env.run_source("__pending(\"m\")\n").unwrap();
        let subj_id = expect_pending(&subj);
        let v = env
            .run_source("match __pending(\"m\")\n  when 1 -> \"a\"\n  when 2 -> \"b\"\nend\n")
            .unwrap();
        assert_eq!(
            expect_pending(&v),
            subj_id,
            "match on a pending subject must evaluate to that pending"
        );
    }

    /// A wildcard arm must NOT catch a Pending: absorption precedes arm testing.
    #[test]
    fn match_wildcard_does_not_catch_pending() {
        let mut env = Env::new();
        let subj = env.run_source("__pending(\"m\")\n").unwrap();
        let subj_id = expect_pending(&subj);
        let v = env
            .run_source("match __pending(\"m\")\n  when other -> print(\"caught\")\nend\n")
            .unwrap();
        assert!(
            env.take_output().is_empty(),
            "a wildcard arm must not run on a pending subject"
        );
        assert_eq!(
            expect_pending(&v),
            subj_id,
            "the match still evaluates to the pending, not the arm body"
        );
    }

    /// Once the subject resolves, `match` selects the real arm normally.
    #[test]
    fn match_resolves_to_arm_once_ready() {
        let mut env = Env::new();
        env.run_source("__resolve(\"m\", 2)\n").unwrap();
        let v = env
            .run_source("match __pending(\"m\")\n  when 1 -> 10\n  when 2 -> 20\nend\n")
            .unwrap();
        assert_eq!(v, Value::Int(20), "a resolved subject matches its arm");
    }
}

/// Chunk H — collections are element-wise. A list literal with a Pending
/// element is a REAL list (not a pending list): `len` and indexing stay exact.
/// Only aggregates that must read every element (`join`, `sort`) absorb a
/// Pending element and return that Pending.
mod pending_collections_chunk_h_tests {
    use super::super::*;

    fn expect_pending(v: &Value) -> crate::value::PendingId {
        match v {
            Value::Pending(id) => *id,
            other => panic!("expected a Pending value, got {other:?}"),
        }
    }

    /// A Pending element does not make the list pending: `len` is exact.
    #[test]
    fn len_of_list_with_pending_element_is_exact() {
        let mut env = Env::new();
        assert_eq!(
            env.run_source("len([1, __pending(\"e\"), 3])\n").unwrap(),
            Value::Int(3),
            "a pending element must not collapse the list; len stays 3"
        );
    }

    /// Indexing a real list returns the Pending element itself, element-wise.
    #[test]
    fn indexing_returns_the_pending_element() {
        let mut env = Env::new();
        let elem = env.run_source("__pending(\"e\")\n").unwrap();
        let elem_id = expect_pending(&elem);
        let v = env.run_source("[1, __pending(\"e\"), 3][1]\n").unwrap();
        assert_eq!(
            expect_pending(&v),
            elem_id,
            "index access yields the pending element unchanged"
        );
    }

    /// `join` must read every element, so a Pending element absorbs: the whole
    /// joined string is unknown. Today join renders it as "<pending>".
    #[test]
    fn join_absorbs_a_pending_element() {
        let mut env = Env::new();
        let elem = env.run_source("__pending(\"e\")\n").unwrap();
        let elem_id = expect_pending(&elem);
        let v = env
            .run_source("join([1, __pending(\"e\"), 3], \",\")\n")
            .unwrap();
        assert_eq!(
            expect_pending(&v),
            elem_id,
            "join over a list with a pending element must absorb to that pending"
        );
    }

    /// `sort` cannot order a list with an unknown key, so a Pending element
    /// absorbs. Today it sorts the pending to the end as an "other" key.
    #[test]
    fn sort_absorbs_a_pending_element() {
        let mut env = Env::new();
        let elem = env.run_source("__pending(\"e\")\n").unwrap();
        let elem_id = expect_pending(&elem);
        let v = env.run_source("sort([3, __pending(\"e\"), 1])\n").unwrap();
        assert_eq!(
            expect_pending(&v),
            elem_id,
            "sort over a list with a pending element must absorb to that pending"
        );
    }

    /// Regression: aggregates over a fully-resolved list are untouched.
    #[test]
    fn aggregates_over_resolved_lists_still_work() {
        let mut env = Env::new();
        let joined = env.run_source("join([1, 2, 3], \"-\")\n").unwrap();
        match joined {
            Value::String(id) => {
                assert_eq!(env.ctx(env.default_context).heap.get_string(id), "1-2-3")
            }
            other => panic!("expected a String, got {other:?}"),
        }
    }
}

/// Chunk I — the StateInit no-commit rule. A `state x = <pending>` init does
/// NOT commit the slot while the value is pending: this frame reads the
/// Pending, and the init block re-runs on later frames until it resolves, then
/// commits normally. Ordinary reassignment (`x = <pending>`) still commits.
mod pending_state_init_chunk_i_tests {
    use super::super::*;

    /// The core rule across two frames: a loading init stays uncommitted (the
    /// frame sees the Pending); once the resource resolves, the re-run init
    /// commits the real value. The resource table persists on the context
    /// across `reset_stack`, so resolving at the end of frame 1 is visible to
    /// frame 2 (the between-frame resolution model).
    #[test]
    fn state_init_does_not_commit_pending_and_reinits_until_resolved() {
        let mut env = Env::new();
        let src = "state user = __pending(\"u\")\nlet shown = user\n__resolve(\"u\", 42)\nshown\n";
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();

        // Frame 1: the fetch is loading → the init result is Pending → the slot
        // is NOT committed → this frame's value is the Pending.
        let f1 = env.run(sid).unwrap();
        assert!(
            matches!(f1, Value::Pending(_)),
            "a pending StateInit must not commit; frame 1 must see the pending, got {f1:?}"
        );

        // Frame 2: the slot was never committed, so StateInit re-runs; the
        // resource is now resolved, so it commits the real value.
        env.reset_stack(sid).unwrap();
        let f2 = env.run(sid).unwrap();
        assert_eq!(
            f2,
            Value::Int(42),
            "once resolved, the re-run StateInit commits the real value"
        );
    }

    /// A pending init that never resolves keeps re-initializing: every frame
    /// re-reads the Pending, and the slot is never permanently cached as
    /// loading (the failure mode this rule exists to prevent).
    #[test]
    fn unresolved_state_init_reinits_every_frame() {
        let mut env = Env::new();
        let pid = env
            .load_program("state user = __pending(\"u\")\nuser\n")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();
        for frame in 0..3 {
            if frame > 0 {
                env.reset_stack(sid).unwrap();
            }
            let v = env.run(sid).unwrap();
            assert!(
                matches!(v, Value::Pending(_)),
                "frame {frame}: an unresolved init must re-read the pending, got {v:?}"
            );
        }
    }

    /// The no-commit rule is scoped to the init commit: an ordinary
    /// reassignment of a state var to a Pending still commits (Q3
    /// allow-and-flag), so `x` reads back as the Pending this frame.
    #[test]
    fn ordinary_reassignment_commits_a_pending() {
        let mut env = Env::new();
        let v = env
            .run_source("state x = 0\nx = __pending(\"p\")\nx\n")
            .unwrap();
        assert!(
            matches!(v, Value::Pending(_)),
            "a plain reassignment to a pending is allowed and committed, got {v:?}"
        );
    }

    /// State-in-loop interaction: a per-iteration `state` initialized from a
    /// pending stays uncommitted in every iteration's slot, so each reads the
    /// Pending. Guards that the re-enterable init path does not disturb the
    /// loop-carry / phi machinery.
    #[test]
    fn pending_state_init_inside_loop_stays_uncommitted_per_iteration() {
        let mut env = Env::new();
        env.run_source(
            "for i in [0, 1, 2] do\n  state s = __pending(\"s\")\n  print(is_loading(s))\nend\n",
        )
        .unwrap();
        assert_eq!(
            env.take_output(),
            vec!["true", "true", "true"],
            "each iteration's pending state init must stay uncommitted"
        );
    }
}

/// Chunk J of the pending-values feature: PROVENANCE on every Pending. When a
/// resource is created, its `ResourceEntry` records the call-site `TermId` that
/// requested it (`origin`) and the frame it started in (`frame_started`), so the
/// observability tooling can render "where did this come from" and "how many
/// frames has it been loading". A frame counter on the `ExecutionContext` makes
/// age-in-frames computable.
///
/// The `origin` assertion FAILS against pre-Chunk-J code: `get_or_create_loading`
/// hard-codes `origin: None`, so the entry carries no call site.
mod pending_provenance_tests {
    use super::super::*;

    /// Pull the `PendingId` out of a `Value::Pending`, or panic helpfully.
    fn expect_pending(v: &Value) -> crate::value::PendingId {
        match v {
            Value::Pending(id) => *id,
            other => panic!("expected a Pending value, got {other:?}"),
        }
    }

    /// A resource created by `__pending("k")` carries the origin `TermId` of the
    /// requesting call site, and its `frame_started` is the context's current
    /// frame (0 in a fresh Env). The origin's source span must cover the
    /// `__pending` call. Today `origin` is hard-coded `None`, so the
    /// `.expect("origin ...")` panics — this is the failing assertion.
    #[test]
    fn pending_records_origin_and_start_frame() {
        let mut env = Env::new();
        let pid = env.load_program("let x = __pending(\"k\")\nx\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        let v = env.run(sid).unwrap();
        let id = expect_pending(&v);

        let ck = env.default_context;
        let entry = env.ctx(ck).resources.entry(id);

        // frame_started is the context frame at creation — 0 for a fresh Env.
        assert_eq!(entry.frame_started, 0, "a fresh Env starts at frame 0");

        // origin points at the __pending call site.
        let origin = entry
            .origin
            .expect("origin should be populated with the call site");
        let program = env.get_program(pid).unwrap();
        let span = program
            .source_map
            .get(origin)
            .expect("origin term has a source span");
        let text = &program.source[span.start.offset as usize..span.end.offset as usize];
        assert!(
            text.contains("__pending"),
            "origin should point at the __pending call, got source text {text:?}"
        );
    }

    /// `advance_frame()` bumps the context frame counter, and `age_frames` on a
    /// resource entry is `current_frame - frame_started` (saturating). A resource
    /// created at frame 0 is 2 frames old after two advances.
    #[test]
    fn age_frames_tracks_advances() {
        let mut env = Env::new();
        let pid = env.load_program("let x = __pending(\"k\")\nx\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        let id = expect_pending(&env.run(sid).unwrap());

        let ck = env.default_context;
        assert_eq!(env.ctx(ck).frame(), 0, "fresh context is at frame 0");
        assert_eq!(env.ctx(ck).resources.entry(id).age_frames(0), 0);

        env.ctx_mut(ck).advance_frame();
        env.ctx_mut(ck).advance_frame();
        assert_eq!(env.ctx(ck).frame(), 2, "two advances reach frame 2");

        let current = env.ctx(ck).frame();
        assert_eq!(
            env.ctx(ck).resources.entry(id).age_frames(current),
            2,
            "a resource born at frame 0 is 2 frames old at frame 2"
        );
    }
}

/// Chunk K of the pending-values feature: the always-on `absorbed_count`. Every
/// strict-operator absorption and every effectful no-op bumps a cheap counter on
/// the absorbed resource's entry — the steady-state observability signal that
/// answers "how many operations swallowed this Pending". Meta builtins
/// (`is_loading`, …) inspect a Pending instead of absorbing it, so they must NOT
/// bump.
///
/// Pre-Chunk-K, nothing increments `absorbed_count` (it inits to 0 and stays
/// there), so the `== N` assertions FAIL (they read 0).
mod pending_absorbed_count_tests {
    use super::super::*;

    /// Pull the `PendingId` out of a `Value::Pending`, or panic helpfully.
    fn expect_pending(v: &Value) -> crate::value::PendingId {
        match v {
            Value::Pending(id) => *id,
            other => panic!("expected a Pending value, got {other:?}"),
        }
    }

    /// Three arithmetic operations that each absorb the same Pending bump its
    /// `absorbed_count` to 3. The additions are chained so dead-code elimination
    /// can't drop them, and every intermediate is the same `PendingId`.
    #[test]
    fn absorbing_arithmetic_bumps_absorbed_count() {
        let mut env = Env::new();
        let pid = env
            .load_program(
                "let x = __pending(\"k\")\nlet a = x + 1\nlet b = a + 1\nlet c = b + 1\nc\n",
            )
            .unwrap();
        let sid = env.create_stack(pid).unwrap();
        let id = expect_pending(&env.run(sid).unwrap());

        let ck = env.default_context;
        assert_eq!(
            env.ctx(ck).resources.entry(id).absorbed_count,
            3,
            "three absorbing `+` ops should bump absorbed_count to 3"
        );
    }

    /// An effectful no-op (`print` on a Pending emits nothing but still absorbs)
    /// bumps `absorbed_count`.
    #[test]
    fn effectful_noop_bumps_absorbed_count() {
        let mut env = Env::new();
        let pid = env
            .load_program("let x = __pending(\"k\")\nprint(x)\nx\n")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();
        let id = expect_pending(&env.run(sid).unwrap());

        let ck = env.default_context;
        assert_eq!(
            env.ctx(ck).resources.entry(id).absorbed_count,
            1,
            "an effectful no-op on a Pending should bump absorbed_count"
        );
    }

    /// A meta builtin (`is_loading`) inspects the Pending rather than absorbing
    /// it (it is registered `AllowPending`), so it must NOT bump `absorbed_count`.
    #[test]
    fn meta_call_does_not_bump_absorbed_count() {
        let mut env = Env::new();
        let pid = env
            .load_program("let x = __pending(\"k\")\nis_loading(x)\nx\n")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();
        let id = expect_pending(&env.run(sid).unwrap());

        let ck = env.default_context;
        assert_eq!(
            env.ctx(ck).resources.entry(id).absorbed_count,
            0,
            "a non-absorbing meta call must not bump absorbed_count"
        );
    }
}

/// Chunk L of the pending-values feature: the DEBUG-GATED per-frame absorption
/// log. When `ExecutionContext::trace_pending` is enabled, every absorption
/// records `(origin, PendingId)` into `absorption_log` — the span→id pairs a
/// dataflow viz paints as a resource's downstream cone. Off by default (the log
/// stays empty, paying only the always-on `absorbed_count`). The log is
/// per-frame: cleared at the stack reset while the cross-frame `ResourceTable`
/// survives.
///
/// Pre-Chunk-L there is no `trace_pending` flag or `absorption_log` field, so this
/// module does not even compile — the behavior is entirely new.
mod pending_absorption_log_tests {
    use super::super::*;

    /// Pull the `PendingId` out of a `Value::Pending`, or panic helpfully.
    fn expect_pending(v: &Value) -> crate::value::PendingId {
        match v {
            Value::Pending(id) => *id,
            other => panic!("expected a Pending value, got {other:?}"),
        }
    }

    const TWO_ABSORBS: &str = "let x = __pending(\"k\")\nlet a = x + 1\nlet b = a + 1\nb\n";

    /// With the flag on, two absorbing `+` ops each log one `(origin, id)` pair:
    /// the entry ids are the absorbed Pending, and each carries a source origin.
    #[test]
    fn enabled_log_records_origin_and_id_per_absorption() {
        let mut env = Env::new();
        let ck = env.default_context;
        env.ctx_mut(ck).enable_pending_trace();

        let pid = env.load_program(TWO_ABSORBS).unwrap();
        let sid = env.create_stack(pid).unwrap();
        let id = expect_pending(&env.run(sid).unwrap());

        let log = &env.ctx(ck).absorption_log;
        assert_eq!(log.len(), 2, "two absorbing `+` ops should log two entries");
        for (origin, logged) in log {
            assert_eq!(*logged, id, "each entry logs the absorbed Pending's id");
            assert!(
                origin.is_some(),
                "each absorption is attributed to its call site"
            );
        }
    }

    /// With the flag OFF (the default), nothing is logged — the log pays only the
    /// always-on `absorbed_count`, not the unbounded per-absorption push.
    #[test]
    fn disabled_log_stays_empty() {
        let mut env = Env::new();
        let ck = env.default_context;
        // flag left OFF (default)

        let pid = env.load_program(TWO_ABSORBS).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();

        assert!(
            env.ctx(ck).absorption_log.is_empty(),
            "the log stays empty when trace_pending is off"
        );
    }

    /// The log is per-frame: a frame boundary (`advance_frame` + `reset_stack`)
    /// clears it, while the enable flag persists for the next frame.
    #[test]
    fn log_clears_between_frames() {
        let mut env = Env::new();
        let ck = env.default_context;
        env.ctx_mut(ck).enable_pending_trace();

        let pid = env.load_program(TWO_ABSORBS).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert!(
            !env.ctx(ck).absorption_log.is_empty(),
            "frame 1 populated the log"
        );

        // Next frame: advance the counter and reset the stack — the per-frame log
        // clears (the enable flag is not touched).
        env.ctx_mut(ck).advance_frame();
        env.reset_stack(sid).unwrap();
        assert!(
            env.ctx(ck).absorption_log.is_empty(),
            "the per-frame log clears at the stack reset"
        );
    }
}

/// Chunk M of the pending-values feature: a pending value must render
/// *distinctly* — with its resolution state, origin, and age — in every
/// debugging surface, never as `nil` or a bare `<pending>`. The provenance-rich
/// [`pending_to_display`](crate::value::pending_to_display) drives human-facing
/// output (`<pending __pending("k") loading 2f>`), and JSON state dumps route
/// through [`value_to_json_ctx`](crate::value::value_to_json_ctx) so a pending
/// state var serializes as a structured object rather than `"<pending>"`.
mod pending_render_chunk_m_tests {
    use super::super::*;

    /// Pull the `PendingId` out of a `Value::Pending`, or panic helpfully.
    fn expect_pending(v: &Value) -> crate::value::PendingId {
        match v {
            Value::Pending(id) => *id,
            other => panic!("expected a Pending value, got {other:?}"),
        }
    }

    /// The context-aware renderer names the resource's state, the source text of
    /// its origin call site, and its age in frames. Pre-Chunk-M there is no such
    /// renderer — the only rendering is the bare `<pending>` — so this pins the
    /// new provenance-rich format.
    #[test]
    fn pending_to_display_shows_state_origin_and_age() {
        let mut env = Env::new();
        let pid = env.load_program("let x = __pending(\"k\")\nx\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        let id = expect_pending(&env.run(sid).unwrap());

        let ck = env.default_context;
        // Age the resource two frames so the age component is non-zero.
        env.ctx_mut(ck).advance_frame();
        env.ctx_mut(ck).advance_frame();

        let program = env.get_program(pid).unwrap();
        let frame = env.ctx(ck).frame();
        let rendered = crate::value::pending_to_display(id, &env.ctx(ck).resources, program, frame);

        assert!(
            rendered.contains("loading"),
            "state missing from {rendered:?}"
        );
        assert!(
            rendered.contains("__pending"),
            "origin source text missing from {rendered:?}"
        );
        assert!(
            rendered.contains("2f"),
            "age-in-frames missing from {rendered:?}"
        );
    }

    /// A JSON state dump containing a pending state var must serialize that var
    /// as a structured `{"type":"pending", "state":…}` object — not the bare
    /// `"<pending>"` string (nor `null`). `state x = <pending>` never commits
    /// (StateInit no-commit rule), so we commit the pending via an ordinary
    /// reassignment, which is allowed-and-flagged.
    #[test]
    fn state_dump_json_renders_pending_as_structured_object() {
        let mut env = Env::new();
        let pid = env
            .load_program("state x = 0\nx = __pending(\"k\")\nx\n")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();

        let json = env.get_state_json(pid, sid);
        let x = json.get("x").expect("state var x should be in the dump");
        assert_eq!(
            x.get("type").and_then(|t| t.as_str()),
            Some("pending"),
            "a pending state var must dump as a structured pending object, got {x}"
        );
        assert_eq!(
            x.get("state").and_then(|s| s.as_str()),
            Some("loading"),
            "the structured pending object must carry its resolution state, got {x}"
        );
    }

    /// The host→script prop-feed contract that petal-web-canvas's `setProp`
    /// relies on: a value pushed via `set_state_from_json` *before* the run is
    /// seen by that run, and it overrides the `state x = <init>` default (the
    /// initializer is skipped because the key is already committed). Without
    /// this, a host-controlled prop would flash its default on the first frame.
    #[test]
    fn pushed_state_overrides_initializer_on_first_run() {
        let mut env = Env::new();
        let pid = env.load_program("state cube = 0\ncube\n").unwrap();
        let sid = env.create_stack(pid).unwrap();

        // Host stages the prop before the first run, exactly as the WASM shim
        // does (set_state_json → reset_and_run).
        env.set_state_from_json(pid, sid, "cube", &serde_json::json!(42))
            .unwrap();
        let result = env.run(sid).unwrap();

        assert_eq!(
            result,
            Value::Int(42),
            "the run must observe the host-pushed value, not the `= 0` default"
        );
    }

    /// A prop re-pushed between frames is visible to the next frame, and
    /// `reset_stack` (the per-frame boundary) preserves it — so the shim's
    /// "flush props, then reset_and_run each frame" loop delivers updates.
    #[test]
    fn pushed_state_updates_across_frames_through_reset() {
        let mut env = Env::new();
        let pid = env.load_program("state cube = 0\ncube\n").unwrap();
        let sid = env.create_stack(pid).unwrap();

        env.set_state_from_json(pid, sid, "cube", &serde_json::json!(1))
            .unwrap();
        assert_eq!(env.run(sid).unwrap(), Value::Int(1));

        // Next frame: reset (keeps state), push a new value, run again.
        env.reset_stack(sid).unwrap();
        env.set_state_from_json(pid, sid, "cube", &serde_json::json!(2))
            .unwrap();
        assert_eq!(env.run(sid).unwrap(), Value::Int(2));

        // A frame with no push keeps the last pushed value through reset.
        env.reset_stack(sid).unwrap();
        assert_eq!(env.run(sid).unwrap(), Value::Int(2));
    }

    /// A pushed compound value (nested arrays + objects) is reconstructed into
    /// native lists/maps the script can index into — the case the petal-web-canvas
    /// cube renderer relies on (host stages an array of `{c, n, p}` sticker quads).
    #[test]
    fn pushed_state_supports_nested_arrays_and_objects() {
        let mut env = Env::new();
        let pid = env
            .load_program("state faces = []\nfaces[0].c[1]\n")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();

        env.set_state_from_json(
            pid,
            sid,
            "faces",
            &serde_json::json!([{ "c": [10, 20, 30], "n": [0, 1, 0] }]),
        )
        .unwrap();

        assert_eq!(
            env.run(sid).unwrap(),
            Value::Int(20),
            "the script must be able to index a host-pushed array of objects"
        );
    }
}

/// Chunk N of the pending-values feature: the frame pending report — a
/// structured, per-frame summary of every live resource, surfaced through
/// [`Env::pending_report`] (and, downstream, the debug-protocol `pending_report`
/// query and the petal-ui overlay hook). Each entry names the resource's id,
/// key, resolution state, age in frames, origin call site, and this-frame
/// absorption count.
mod pending_report_chunk_n_tests {
    use super::super::*;

    /// A live pending resource appears in the report with its `loading` state,
    /// its age in frames (grown by `advance_frame`), a zero absorption count
    /// (nothing absorbed it this frame), and an origin object whose source text
    /// is the `__pending("k")` call site. Pre-Chunk-N there is no report builder
    /// at all, so this pins the whole shape.
    #[test]
    fn pending_report_lists_live_resource_with_state_age_and_origin() {
        let mut env = Env::new();
        let pid = env.load_program("let x = __pending(\"k\")\nx\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();

        // Age the resource two frames so `age_frames` is a non-trivial value.
        let ck = env.default_context;
        env.ctx_mut(ck).advance_frame();
        env.ctx_mut(ck).advance_frame();

        let report = env.pending_report(pid, sid);
        let arr = report.as_array().expect("report must be a JSON array");
        assert_eq!(
            arr.len(),
            1,
            "exactly one live resource expected, got {report}"
        );

        let entry = &arr[0];
        assert_eq!(
            entry.get("state").and_then(|s| s.as_str()),
            Some("loading"),
            "resource state missing/wrong in {entry}"
        );
        assert_eq!(
            entry.get("age_frames").and_then(|a| a.as_u64()),
            Some(2),
            "age-in-frames missing/wrong in {entry}"
        );
        assert_eq!(
            entry.get("absorbed_count").and_then(|a| a.as_u64()),
            Some(0),
            "absorbed_count missing/wrong in {entry}"
        );
        let origin = entry.get("origin").expect("origin field missing");
        let text = origin.get("text").and_then(|t| t.as_str()).unwrap_or("");
        assert!(
            text.contains("__pending"),
            "origin source text should name the call site, got {origin}"
        );
    }
}
