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
        assert_eq!(env.call_function(sid, "next_val", &[]).unwrap(), Value::Int(42));
        assert_eq!(env.call_function(sid, "next_val", &[]).unwrap(), Value::Int(42));
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
        let err = env
            .call_function(sid, "add", &[Value::Int(1)])
            .unwrap_err();
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
        // Both backends: hot reload crosses the program-replacement seam,
        // which each engine handles differently (the VM also invalidates its
        // cached lowering).
        for backend in [crate::backend::Backend::Graph, crate::backend::Backend::Bytecode] {
            let mut env = Env::new();
            env.set_backend(backend);
            let pid = env.load_program("fn f()\n  1\nend\n").unwrap();
            let sid = env.create_stack(pid).unwrap();
            env.run(sid).unwrap();
            assert_eq!(env.call_function(sid, "f", &[]).unwrap(), Value::Int(1));

            let new_program = env.compile_program(pid, "fn f()\n  2\nend\n").unwrap();
            env.transfer_state(sid, new_program).unwrap();
            // Before re-running, the stale table was cleared.
            assert!(env.call_function(sid, "f", &[]).is_err(), "[{backend:?}]");
            env.run(sid).unwrap();
            assert_eq!(env.call_function(sid, "f", &[]).unwrap(), Value::Int(2), "[{backend:?}]");
        }
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
            .load_program(
                "state items = [1, 2, 3]\nitems[0] = items[0] + 100\nprint(\"spec\")\n",
            )
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
        assert!(env.stack(fork).is_none(), "drop_fork removes the fork's stack");
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
        let pid = env.load_program("state x = 0\nwhile true do\n  x = x + 1\nend\n").unwrap();
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
        assert!(yields > 0, "a 5-step budget should have yielded at least once");
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

