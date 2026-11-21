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

    /// A host call that fails must not leave its frames under the next one:
    /// each failure's trace lists its own calls and nothing from before.
    #[test]
    fn a_failed_host_call_leaves_no_frames_behind() {
        let source = "fn find(id)\n  assert(id > 0, \"no such id\")\n  id\nend\n\
                      fn call(id)\n  find(id) + 1\nend\n\
                      fn agent_call(id)\n  call(id)\nend\n";
        let (mut env, sid) = run(source);
        let depth = |env: &Env| env.stacks.get(&sid).unwrap().vm_frames.len();
        let base = depth(&env);
        let first = env.call_function(sid, "agent_call", &[Value::Int(0)]).unwrap_err();
        assert!(first.contains("no such id"), "{first}");
        assert_eq!(depth(&env), base, "the failed call's frames were dropped");
        for _ in 0..3 {
            let again = env.call_function(sid, "agent_call", &[Value::Int(-1)]).unwrap_err();
            assert_eq!(again, first, "a later failure reads like the first");
            assert_eq!(again.matches("in find()").count(), first.matches("in find()").count());
        }
        assert_eq!(depth(&env), base);
        // And a call that works still works, after and between failures.
        assert_eq!(
            env.call_function(sid, "agent_call", &[Value::Int(4)]).unwrap(),
            Value::Int(5)
        );
        assert_eq!(depth(&env), base);
    }

    /// A failure inside a callback (a nested synchronous call) is cleaned up
    /// the same way.
    #[test]
    fn a_failed_host_call_through_a_callback_leaves_no_frames_behind() {
        let source = "fn check(x)\n  assert(x < 2, \"too big\")\n  x\nend\n\
                      fn all(xs)\n  map(xs, fn(x) check(x) end)\nend\n";
        let (mut env, sid) = run(source);
        let base = env.stacks.get(&sid).unwrap().vm_frames.len();
        let xs = Value::List(env.heap_mut().alloc_list(vec![Value::Int(1), Value::Int(2)]));
        let first = env.call_function(sid, "all", &[xs]).unwrap_err();
        let xs = Value::List(env.heap_mut().alloc_list(vec![Value::Int(1), Value::Int(2)]));
        let second = env.call_function(sid, "all", &[xs]).unwrap_err();
        assert_eq!(first, second);
        let stack = env.stacks.get(&sid).unwrap();
        assert_eq!(stack.vm_frames.len(), base);
        assert_eq!(stack.sync_depth, 0);
        assert!(stack.callback.is_none());
    }

    /// A call unwound past the restore of the callback record (a panic the
    /// C bridge catches) leaves it set; the next host call or run must not
    /// take the stack to be inside a callback still.
    #[test]
    fn a_callback_left_recorded_is_cleared_by_the_next_call_or_run() {
        let source = "fn seen(x)\n  state n = 0\n  n += 1\n  n\nend\n";
        let (mut env, sid) = run(source);
        let strand = |env: &mut Env| {
            let stack = env.stacks.get_mut(&sid).unwrap();
            stack.sync_depth = 1;
            stack.callback = Some(crate::stack::BuiltinCallback {
                builtin: "map",
                call: None,
                frames: 0,
            });
        };
        strand(&mut env);
        assert_eq!(env.call_function(sid, "seen", &[Value::Int(0)]).unwrap(), Value::Int(1));
        strand(&mut env);
        env.reset_stack(sid).unwrap();
        let stack = env.stacks.get(&sid).unwrap();
        assert_eq!(stack.sync_depth, 0);
        assert!(stack.callback.is_none());
    }

    /// `call_function_named`: arguments by parameter name, bound as a written
    /// `f(1, y: 2)` is.
    #[test]
    fn calls_a_function_with_named_arguments() {
        let source = "fn toll(id, cents = 25, note = \"-\")\n  \"{id}:{cents}:{note}\"\nend\n\
                      fn pair(a, b)\n  a * 10 + b\nend\n";
        let (mut env, sid) = run(source);
        let text = |env: &Env, v: Value| match v {
            Value::String(id) => env.heap().get_string(id).to_string(),
            other => panic!("expected a string, got {other:?}"),
        };
        let note = Value::String(env.heap_mut().alloc_string("n".to_string()));
        // Every argument named, in an order that is not the parameters'.
        let v = env
            .call_function_named(
                sid,
                "toll",
                &[note, Value::Int(7)],
                &[Some("note"), Some("id")],
            )
            .unwrap();
        assert_eq!(text(&env, v), "7:25:n");
        // Positional first, then named; a default in the middle left out.
        let v = env
            .call_function_named(sid, "toll", &[Value::Int(1), note], &[None, Some("note")])
            .unwrap();
        assert_eq!(text(&env, v), "1:25:n");
        let v = env
            .call_function_named(sid, "pair", &[Value::Int(2), Value::Int(1)], &[Some("b"), Some("a")])
            .unwrap();
        assert_eq!(v, Value::Int(12));

        let err = |env: &mut Env, args: &[Value], names: &[Option<&str>]| {
            env.call_function_named(sid, "toll", args, names).unwrap_err().to_string()
        };
        assert!(
            err(&mut env, &[Value::Int(1)], &[Some("cents")]).contains("missing a value for parameter 'id'")
        );
        assert!(
            err(&mut env, &[Value::Int(1), Value::Int(2)], &[Some("id"), Some("nope")])
                .contains("no parameter named 'nope'")
        );
        assert!(
            err(&mut env, &[Value::Int(1), Value::Int(2)], &[None, Some("id")])
                .contains("multiple values for parameter 'id'")
        );
        // The failures left nothing behind.
        assert!(env.stacks.get(&sid).unwrap().vm_frames.is_empty());
    }

    #[test]
    fn function_params_reads_the_declaration() {
        let source = "fn toll(id: string, cents: int = 25, rate: float = 0.5, extra = nil)\n  id\nend\n\
                      fn over(a)\n  a\nend\nfn over(a, b)\n  a\nend\n";
        let (env, sid) = run(source);
        let params = env.function_params(sid, "toll").unwrap();
        let summary: Vec<(&str, Option<&str>, bool)> = params
            .iter()
            .map(|p| (p.name.as_str(), p.ty.as_deref(), p.has_default))
            .collect();
        assert_eq!(
            summary,
            [
                ("id", Some("string"), false),
                ("cents", Some("int"), true),
                ("rate", Some("float"), true),
                ("extra", None, true),
            ]
        );
        assert_eq!(params[1].default_source.as_deref(), Some("25"));
        assert!(env.function_params(sid, "over").is_none(), "an overload set");
        assert!(env.function_params(sid, "nope").is_none());
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
            "import io; let s = symbol(\"draw\")\n\
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

    /// Emit tracing attributes each buffered value to the *call site* that
    /// pushed it — the fact the whole direct-manipulation story rests on. Three
    /// pushes on three lines must come back pointing at those three lines, in
    /// the same order as the values.
    #[test]
    fn emit_trace_attributes_each_value_to_its_own_call_site() {
        let mut env = Env::new();
        env.enable_emit_trace(true);
        let pid = env
            .load_program(
                "import io; let s = symbol(\"draw\")\n\
                 push_output(s, 1)\n\
                 push_output(s, 2)\n\
                 push_output(s, 3)\n",
            )
            .unwrap();
        let stack = env.create_stack(pid).unwrap();
        env.run(stack).unwrap();

        let sym = env.intern_symbol("draw");
        let values = env.take_output_buffer(sym);
        let origins = env.take_output_origins(sym);
        assert_eq!(values.len(), 3);
        assert_eq!(origins.len(), values.len(), "one origin per emitted value");

        let program = env.get_program(pid).unwrap();
        let lines: Vec<u32> = origins
            .iter()
            .map(|o| {
                let term = o.leaf().expect("every push has a call site");
                let site =
                    crate::provenance::CallSite::resolve(program, term).expect("site resolves");
                assert_eq!(site.callee.as_deref(), Some("push_output"));
                site.span.expect("a mapped span").start.line
            })
            .collect();
        assert_eq!(lines, vec![2, 3, 4]);

        // Draining took the origins with the values, so the next frame starts
        // clean rather than inheriting this frame's attribution.
        assert!(env.take_output_origins(sym).is_empty());
    }

    /// An emit from inside a function records the *chain*: the call site in the
    /// function, then the call that entered it. This is what lets a reader pick
    /// the frame in the file it is showing — without it, a `draw_*` that a
    /// library wraps in a Petal function always attributes to the library.
    #[test]
    fn emit_trace_turned_on_late_still_attributes_memoized_calls() {
        // A host that turns tracing on after some frames (an edit mode) must
        // not get the memo's replay of a call recorded untraced, which kept
        // no origins.
        let mut env = Env::new();
        let pid = env
            .load_program(
                "import io; let s = symbol(\"draw\")\n\
                 fn shape(x)\n  \
                   push_output(s, x)\n\
                 end\n\
                 shape(1)\n",
            )
            .unwrap();
        let stack = env.create_stack(pid).unwrap();
        let sym = env.intern_symbol("draw");
        for _ in 0..3 {
            env.reset_stack(stack).unwrap();
            env.run(stack).unwrap();
            let _ = env.take_output_buffer(sym);
        }
        env.enable_emit_trace(true);
        env.reset_stack(stack).unwrap();
        env.run(stack).unwrap();
        assert_eq!(env.take_output_buffer(sym).len(), 1);
        let sites = env.take_output_origins(sym);
        assert_eq!(sites.len(), 1);
        assert!(!sites[0].chain.is_empty(), "the replayed emit has its call chain");
    }

    #[test]
    fn emit_trace_records_the_whole_call_chain() {
        let mut env = Env::new();
        env.enable_emit_trace(true);
        let pid = env
            .load_program(
                "import io; let s = symbol(\"draw\")\n\
                 fn shape()\n  \
                   push_output(s, 1)\n\
                 end\n\
                 shape()\n",
            )
            .unwrap();
        let stack = env.create_stack(pid).unwrap();
        env.run(stack).unwrap();

        let sym = env.intern_symbol("draw");
        let _ = env.take_output_buffer(sym);
        let sites = env.take_output_origins(sym);
        assert_eq!(sites.len(), 1);

        let program = env.get_program(pid).unwrap();
        let lines: Vec<u32> = sites[0]
            .chain
            .iter()
            .filter_map(|t| program.source_map.get(*t))
            .map(|s| s.start.line)
            .collect();
        assert!(
            lines.starts_with(&[3]),
            "the innermost frame is the push on line 3, got {lines:?}"
        );
        assert!(
            lines.contains(&5),
            "and the chain reaches the call on line 5, got {lines:?}"
        );
    }

    /// Tracing is off by default: the values still flow, the attribution costs
    /// nothing and records nothing.
    #[test]
    fn emit_trace_records_nothing_when_disabled() {
        let mut env = Env::new();
        env.run_source("import io; let s = symbol(\"draw\")\npush_output(s, 1)\n")
            .unwrap();
        let sym = env.intern_symbol("draw");
        assert_eq!(env.take_output_buffer(sym).len(), 1);
        assert!(env.take_output_origins(sym).is_empty());
    }

    /// Clearing a buffer must clear its attribution too, or the next frame's
    /// first value inherits the previous frame's call site.
    #[test]
    fn clearing_a_buffer_clears_its_origins() {
        let mut env = Env::new();
        env.enable_emit_trace(true);
        env.run_source("import io; let s = symbol(\"draw\")\npush_output(s, 1)\n")
            .unwrap();
        let sym = env.intern_symbol("draw");
        env.clear_output_buffer(sym);
        assert!(env.take_output_buffer(sym).is_empty());
        assert!(env.take_output_origins(sym).is_empty());
    }

    #[test]
    fn output_buffer_values_survive_gc() {
        // A heap-backed value pushed into a buffer must survive a collection
        // triggered mid-run by other allocations. If buffers weren't GC roots,
        // the string would be swept and its contents corrupted.
        let mut env = Env::new();
        env.run_source(
            "import io; let s = symbol(\"out\")\n\
             push_output(s, \"keep-me\")\n\
             let acc = 0\n\
             for i in range(0, 5000) do\n  \
               let tmp = \"garbage\" ++ str(i)\n  \
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
    fn re_running_a_program_does_not_accumulate_closures() {
        // The per-frame host shape: one program, run again and again off the
        // same stack. Every run re-executes the `fn` declarations, so every run
        // allocates fresh closures; the ones the previous run made are garbage
        // and must be reclaimed. Before the ClosureTable they never were — a
        // Garden panel at 60fps leaked ~130 closures a frame, and because their
        // captures were treated as GC roots they pinned the heap with them.
        let mut env = Env::new();
        let program = env
            .load_program(
                "fn outer(n)\n  \
                   let scale = n * 2\n  \
                   let inner = fn(x) -> x * scale\n  \
                   inner(3)\n\
                 end\n\
                 let acc = 0\n\
                 for i in range(0, 50) do\n  \
                   acc = acc + outer(i)\n\
                 end\n\
                 acc\n",
            )
            .unwrap();
        let stack = env.create_stack(program).unwrap();

        env.run(stack).unwrap();
        let after_first = env.closures().closure_count();
        for _ in 0..200 {
            env.reset_stack(stack).unwrap();
            env.run(stack).unwrap();
        }
        // Collections are budget-driven, so the live count sawtooths between
        // cycles; force one and the answer is exact — every closure the 200
        // dead runs made is gone, and the live run's are still there.
        let ck = env.stacks.get(&stack).unwrap().context;
        env.collect_garbage(ck);
        let after_collect = env.closures().closure_count();
        assert!(
            after_collect <= after_first,
            "after 200 runs and a collection {after_collect} closures are live, \
             up from {after_first} after the first run — the table is leaking"
        );
    }

    #[test]
    fn a_closure_held_in_state_survives_gc() {
        // The other half of the contract: a closure the program still holds —
        // here across runs, in a `state` var, plus one stored inside a record —
        // must never be swept, and its captures must stay intact.
        let mut env = Env::new();
        let program = env
            .load_program(
                "state held = { f: fn(x) -> x + 40 }\n\
                 let acc = 0\n\
                 for i in range(0, 2000) do\n  \
                   let tmp = \"garbage\" ++ str(i)\n  \
                   acc = acc + len(tmp)\n\
                 end\n\
                 held.f(2)\n",
            )
            .unwrap();
        let stack = env.create_stack(program).unwrap();
        for _ in 0..20 {
            env.reset_stack(stack).unwrap();
            assert_eq!(env.run(stack).unwrap(), Value::Int(42));
        }
    }

    #[test]
    fn binding_is_readable_from_script() {
        let mut env = Env::new();
        let sym = env.intern_symbol("dt");
        env.set_binding(sym, Value::Float(0.016));
        let result = env.run_source("import io; binding(symbol(\"dt\"))").unwrap();
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
             for i in range(0, 5000) do\n  \
               let tmp = \"g\" ++ str(i)\n  \
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

    /// The inverse of a fork. Restoring a snapshot puts the live execution back
    /// at that moment — heap objects the state points at included — and leaves
    /// the snapshot reusable, which is what lets a host rewind to it twice.
    #[test]
    fn restore_execution_rewinds_the_live_stack_to_a_fork() {
        let mut env = Env::new();
        let pid = env
            .load_program("state var n = 0\nstate var xs = []\nset n = n + 1\nset xs = append(xs, n)\n")
            .unwrap();
        let live = env.create_stack(pid).unwrap();
        for _ in 0..3 {
            env.reset_stack(live).unwrap();
            env.run(live).unwrap();
        }
        let snap = env.fork_execution(live).unwrap();
        for _ in 0..2 {
            env.reset_stack(live).unwrap();
            env.run(live).unwrap();
        }
        assert_eq!(env.get_state_json(pid, live)["n"], 5);

        env.restore_execution(live, snap).unwrap();
        let s = env.get_state_json(pid, live);
        assert_eq!(s["n"], 3, "the live stack is back at the snapshot");
        assert_eq!(s["xs"], serde_json::json!([1, 2, 3]), "heap objects came back with it");

        // It runs on from there, under the same stack key…
        env.reset_stack(live).unwrap();
        env.run(live).unwrap();
        assert_eq!(env.get_state_json(pid, live)["n"], 4);
        // …and the snapshot is untouched, so it can be restored again.
        env.restore_execution(live, snap).unwrap();
        assert_eq!(env.get_state_json(pid, live)["n"], 3);
    }

    /// A snapshot taken before a hot reload restores onto the program loaded
    /// *now*: state the new program no longer declares is dropped, state it
    /// added initializes on the next run, and the new code is what runs.
    #[test]
    fn restore_execution_reshapes_a_snapshot_onto_the_program_loaded_now() {
        let mut env = Env::new();
        let pid = env
            .load_program("state var n = 0\nstate var gone = 1\nset n = n + 1\n")
            .unwrap();
        let live = env.create_stack(pid).unwrap();
        env.run(live).unwrap();
        let snap = env.fork_execution(live).unwrap();

        let edited = env
            .compile_program_at(
                pid,
                "state var n = 0\nstate var fresh = 7\nset n = n + 10\n",
                std::path::Path::new("edited.ptl"),
            )
            .unwrap();
        env.transfer_state(live, edited).unwrap();
        env.reset_stack(live).unwrap();
        env.run(live).unwrap();
        assert_eq!(env.get_state_json(pid, live)["n"], 11);

        env.restore_execution(live, snap).unwrap();
        let s = env.get_state_json(pid, live);
        assert_eq!(s["n"], 1, "restored to the pre-edit value");
        assert!(s.get("gone").is_none(), "a declaration the edit removed is dropped");

        env.reset_stack(live).unwrap();
        env.run(live).unwrap();
        let s = env.get_state_json(pid, live);
        assert_eq!(s["n"], 11, "the edited program is what runs from the restored state");
        assert_eq!(s["fresh"], 7, "a declaration the edit added initializes");
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

    const FRAME: &str =
        "native fn emit_rect()\nstate n = 0\nn = n + 1\nprint(\"hi\")\nemit_rect()\n";

    /// The regression Step 7 fixes: a forked run's print output and draw-command
    /// buffer live in the fork's own context. `run_speculative` discards them
    /// (its whole point), so a host that needs them must drive the fork itself
    /// and drain it through the `*_for` accessors before `drop_fork`.
    #[test]
    fn host_drains_a_forks_output_and_draw_buffer_then_drops_it() {
        let mut env = Env::new();
        env.register_native("emit_rect", emit_rect, NativeEffects::EMITS);
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
                assert_eq!(heap.tag_str(tag), "rect");
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

/// Garbage-collection pressure: the runtime must reclaim large short-lived
/// payloads *cheaply*, and must pace collections against bytes allocated rather
/// than object count.
///
/// The workload these pin is the reported one: a simulation frame that writes
/// into a large `f64` array many times. Every write copy-on-writes the whole
/// backing buffer (value semantics), so one frame produces megabytes of garbage
/// from a handful of objects. Two defects made that pathological:
///
///  1. `sweep` called `Vec::clear()` on a reclaimed payload, which keeps the
///     allocation. A swept 160 KB array sat on the free list still holding
///     160 KB, and `Slab::alloc` overwrites `slot.data` wholesale — so that
///     capacity was never even reused. Retained memory grew with the *burst*
///     size, not the live set.
///  2. The collection trigger counted allocations (1024 of them) regardless of
///     size, while a collection is a full O(slots) trace. Big-array churn
///     therefore traced the whole heap constantly, and the cost of each trace
///     grew with the slab high-water mark — "slower the longer it runs".
///
/// Assertions are on heap accounting (`live_bytes`, `reserved_bytes`,
/// `collections`), never wall-clock timing, so they are stable in CI.
mod gc_pressure_tests {
    use super::super::*;
    use crate::policy::RunPolicy;

    /// Frames of a simulation that writes into a large array. `n` frames, each
    /// doing `writes` copy-on-write element writes into a `len`-element array —
    /// i.e. `writes` fresh arrays of `len * 8` bytes per frame, all but the last
    /// immediately garbage. Returns the env and its context key.
    fn run_array_churn(frames: usize, writes: usize, len: usize) -> (Env, ContextKey) {
        let source = format!(
            "state arr = f64_array({len})\n\
             for f in range(0, {frames}) do\n\
             \x20 for i in range(0, {writes}) do\n\
             \x20   arr = set_at(arr, i, 1.0)\n\
             \x20 end\n\
             end\n"
        );
        let mut env = Env::new();
        // The point of this workload is the garbage that copy-on-write produces,
        // so disable the in-place optimizations that would elide the copies —
        // otherwise the test measures the escape analysis, not the collector.
        env.set_policy(RunPolicy::BASELINE);
        let pid = env.load_program(&source).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let ck = env.stacks.get(&sid).unwrap().context;
        (env, ck)
    }

    /// Repeatedly allocating and dropping large arrays must not grow retained
    /// memory. After a collection, the bytes the heap is *holding* (including
    /// the free list) must be close to the bytes actually live. Pre-fix this
    /// failed spectacularly: every swept array kept its full backing buffer, so
    /// retained memory tracked peak churn instead of the live set.
    #[test]
    fn swept_array_buffers_release_their_backing_memory() {
        let (mut env, ck) = run_array_churn(20, 64, 20_000);
        // Force a final collection so we measure post-sweep state rather than
        // whatever happened to be floating when the program ended.
        env.collect_garbage(ck);
        let heap = &env.ctx(ck).heap;
        let (live, reserved) = (heap.live_bytes(), heap.reserved_bytes());
        eprintln!("[gc bench] after sweep: live={live} reserved={reserved}");
        // Slack for the program's own small objects; the failure mode this
        // pins is off by two orders of magnitude (163 MB held for 160 KB live).
        assert!(
            reserved < live + 64 * 1024,
            "reclaimed slots are still holding their buffers: \
             live={live} reserved={reserved}"
        );
    }

    /// End to end: a long run of large-array churn must not let retained memory
    /// grow with the amount of garbage produced. This run allocates ~205 MB of
    /// arrays with only one live at a time; pre-fix the heap ended up holding
    /// 163 MB of it (41 MB uncollected plus 122 MB of freed-but-retained
    /// buffers). The bound below is the GC budget floor plus generous slack —
    /// what matters is that it is a constant, not a fraction of total churn.
    #[test]
    fn steady_large_array_churn_keeps_retained_memory_bounded() {
        let (env, ck) = run_array_churn(20, 64, 20_000);
        let heap = &env.ctx(ck).heap;
        let (live, reserved, collections) =
            (heap.live_bytes(), heap.reserved_bytes(), heap.collections());
        eprintln!(
            "[gc bench] end of run: live={live} reserved={reserved} collections={collections}"
        );
        assert!(
            reserved < 8 * 1024 * 1024,
            "retained memory grows with total churn, not the live set: \
             live={live} reserved={reserved} after ~205 MB allocated"
        );
    }

    /// Collections must be paced by bytes allocated, not by object count. Two
    /// runs allocating the *same number of objects* 250x apart in bytes must not
    /// collect the same number of times: the byte-heavy one legitimately needs
    /// many cycles, while the byte-light one must not be dragged through a full
    /// heap trace just for having made a thousand small objects.
    #[test]
    fn collection_count_tracks_bytes_not_object_count() {
        // Same object count (20 * 64 = 1280 arrays), 250x apart in bytes.
        let (big_env, big_ck) = run_array_churn(20, 64, 20_000);
        let (small_env, small_ck) = run_array_churn(20, 64, 80);
        let big = big_env.ctx(big_ck).heap.collections();
        let small = small_env.ctx(small_ck).heap.collections();
        eprintln!("[gc bench] collections: big-arrays={big} small-arrays={small}");
        assert!(
            big > 10 * small.max(1),
            "collection count is driven by object count, not bytes: \
             big={big} small={small}"
        );
        // ~1280 arrays of 640 bytes is under a megabyte of churn in total: the
        // byte-light workload should not need to collect at all.
        assert_eq!(
            small, 0,
            "a byte-light workload should not trigger a collection"
        );
    }
}

/// Observation (`crate::observe`): the runtime's "read any named value by name"
/// facility. These run real programs — the facts under test are which terms the
/// VM actually retires and what the qualified-name walk makes of their blocks,
/// so a synthetic buffer would be asserting the fixture rather than the code.
mod observation_tests {
    use super::super::*;

    /// Run `source` with observation on and return the qualified-name map.
    fn observe(source: &str) -> serde_json::Map<String, serde_json::Value> {
        let (env, pid, sid) = run_observed(source);
        env.get_observations_json(pid, sid)
    }

    fn run_observed(source: &str) -> (Env, ProgramId, StackKey) {
        let mut env = Env::new();
        env.observations_mut().enable();
        let pid = env.load_program(source).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        (env, pid, sid)
    }

    /// 1. Top-level `let`s are readable by their plain source name, with the
    ///    JSON shape their Petal type implies.
    #[test]
    fn top_level_lets_are_observable_by_plain_name() {
        let map = observe(
            "let count = 42\n\
             let label = \"hello\"\n\
             let ready = true\n\
             let nums = [1, 2, 3]\n\
             let point = { x: 1, y: 2 }\n",
        );
        assert_eq!(map.get("count"), Some(&serde_json::json!(42)));
        assert_eq!(map.get("label"), Some(&serde_json::json!("hello")));
        assert_eq!(map.get("ready"), Some(&serde_json::json!(true)));
        assert_eq!(map.get("nums"), Some(&serde_json::json!([1, 2, 3])));
        assert_eq!(
            map.get("point"),
            Some(&serde_json::json!({ "x": 1, "y": 2 }))
        );
    }

    /// A binding whose value came from a native reached *indirectly* — through
    /// a value (`let r = rect(…)` where `rect = Rect`, which is how the `ui`
    /// prelude exports it), or as a method (`xs.len()`, `r.center_x()`) — is
    /// observable like any other. Those calls lower to `Call`/`MethodCall`,
    /// whose result used to be recorded only when a pushed closure frame
    /// returned, so a native's in-place result was silently dropped.
    #[test]
    fn natives_called_through_a_value_or_method_are_observable() {
        let map = observe(
            "let rr = Rect\n\
             let r = rr(1, 2, 3, 4)\n\
             let f = abs\n\
             let g = f(-3)\n\
             let xs = [1, 2, 3]\n\
             let n = xs.len()\n\
             let cx = Rect(10, 0, 4, 4).center_x()\n",
        );
        assert_eq!(
            map.get("r"),
            Some(&serde_json::json!({ "x": 1, "y": 2, "w": 3, "h": 4 }))
        );
        assert_eq!(map.get("g"), Some(&serde_json::json!(3)));
        assert_eq!(map.get("n"), Some(&serde_json::json!(3)));
        assert_eq!(map.get("cx"), Some(&serde_json::json!(12)));
    }

    /// 2. The collision this facility exists to fix: a binding inside `fn foo`
    ///    and a same-named top-level binding are two different values, and both
    ///    are readable — the function-local one under `foo.<name>`.
    #[test]
    fn function_local_and_top_level_names_coexist() {
        let map = observe(
            "fn foo(n)\n  let total = n * 10\n  total\nend\n\
             let total = 1\n\
             print(foo(5))\n",
        );
        assert_eq!(map.get("total"), Some(&serde_json::json!(1)));
        assert_eq!(map.get("foo.total"), Some(&serde_json::json!(50)));
    }

    /// A function nested inside a function qualifies with both, outermost
    /// first.
    #[test]
    fn nested_functions_qualify_outermost_first() {
        let map = observe(
            "fn outer(n)\n\
             \x20 fn inner(m)\n\
             \x20   let deep = m + 1\n\
             \x20   deep\n\
             \x20 end\n\
             \x20 inner(n)\n\
             end\n\
             print(outer(1))\n",
        );
        assert_eq!(map.get("outer.inner.deep"), Some(&serde_json::json!(2)));
    }

    /// 3. Non-function child blocks do not qualify: a binding in a top-level
    ///    `if` branch reads under its plain name. A binding in the branch that
    ///    never ran is absent rather than stale or null.
    #[test]
    fn if_branches_key_under_plain_names() {
        let map = observe(
            "let n = 1\n\
             if n > 0 then\n\
             \x20 let taken = \"yes\"\n\
             \x20 print(taken)\n\
             else\n\
             \x20 let skipped = \"no\"\n\
             \x20 print(skipped)\n\
             end\n",
        );
        assert_eq!(map.get("taken"), Some(&serde_json::json!("yes")));
        assert!(
            !map.contains_key("skipped"),
            "a branch that never ran must not report a value: {:?}",
            map
        );
    }

    /// A binding whose value arrives from a *child block* — every value-position
    /// `if`, `&&`, `||` and `??` — must report the value the program actually
    /// computed.
    ///
    /// These lower to a join `Move` into the control term's register, and that
    /// move used to inherit the arm's origin rather than the control term's. So
    /// the named term was credited with whatever it held *before* the arm ran
    /// (`nil` for `??`/`if`) or never recorded at all (the `&&`/`||` rhs paths) —
    /// an observation that silently disagreed with the program's own output.
    /// Every value here is asserted against what the expression evaluates to.
    #[test]
    fn values_joined_from_a_child_block_are_observed() {
        let map = observe(
            "let t = true\n\
             let f = false\n\
             let or_taken = f || \"right\"\n\
             let or_short = t || \"skipped\"\n\
             let and_taken = t && \"right\"\n\
             let and_short = f && \"skipped\"\n\
             let coalesced = nil ?? \"fallback\"\n\
             let present = \"kept\" ?? \"unused\"\n\
             let if_val = if t then \"then\" else \"else\" end\n",
        );
        // The short-circuit paths always worked; the joined ones did not.
        assert_eq!(map.get("or_taken"), Some(&serde_json::json!("right")));
        assert_eq!(map.get("or_short"), Some(&serde_json::json!(true)));
        assert_eq!(map.get("and_taken"), Some(&serde_json::json!("right")));
        assert_eq!(map.get("and_short"), Some(&serde_json::json!(false)));
        assert_eq!(map.get("coalesced"), Some(&serde_json::json!("fallback")));
        assert_eq!(map.get("present"), Some(&serde_json::json!("kept")));
        assert_eq!(map.get("if_val"), Some(&serde_json::json!("then")));
    }

    /// The same join, one level down: a name *rebound* inside a branch or a loop
    /// body reaches the reader through the block's phi carry-out, so the phi —
    /// not the arm's last expression — has to own that write.
    #[test]
    fn a_name_rebound_inside_a_block_reports_the_rebound_value() {
        let map = observe(
            "let mode = \"split\"\n\
             if 1 > 0 then\n\
             \x20 mode = \"unified\"\n\
             end\n\
             let total = 0\n\
             for i in range(0, 4) do\n\
             \x20 total = total + i\n\
             end\n\
             print(mode)\n\
             print(total)\n",
        );
        assert_eq!(map.get("mode"), Some(&serde_json::json!("unified")));
        assert_eq!(map.get("total"), Some(&serde_json::json!(6)));
    }

    /// A value-position loop is a third control term whose value arrives late:
    /// the collected list is written into the loop's register after the body,
    /// by which point lowering had left the origin on the body's last term. So
    /// `let a = for …` was never observed, and the body's last binding was
    /// reported as holding the whole collected list.
    #[test]
    fn a_collecting_loop_is_observed_under_its_own_name() {
        let map = observe(
            "let sel = [1, 2]\n\
             let a = for i in sel do\n\
             \x20 let q = i * 10\n\
             end\n\
             let b = for i in range(0, 3) do i + 1 end\n\
             let none = for i in [] do i end\n",
        );
        assert_eq!(map.get("a"), Some(&serde_json::json!([10, 20])));
        assert_eq!(map.get("q"), Some(&serde_json::json!(20)));
        assert_eq!(map.get("b"), Some(&serde_json::json!([1, 2, 3])));
        assert_eq!(map.get("none"), Some(&serde_json::json!([])));
    }

    /// 4. Documented last-write-wins: a loop temp reports its final iteration.
    #[test]
    fn loop_temp_reports_its_final_iteration() {
        let map = observe(
            "for i in range(0, 5) do\n\
             \x20 let doubled = i * 2\n\
             \x20 print(doubled)\n\
             end\n",
        );
        assert_eq!(map.get("doubled"), Some(&serde_json::json!(8)));
    }

    /// 5. The ~70 synthetic builtin-table terms are named but have no source
    ///    location; they must never surface as observable values.
    #[test]
    fn builtin_phantom_terms_are_never_observed() {
        let map = observe("let n = len([1, 2, 3])\nprint(n)\n");
        assert_eq!(map.get("n"), Some(&serde_json::json!(3)));
        for phantom in ["print", "range", "len", "abs", "push"] {
            assert!(
                !map.contains_key(phantom),
                "phantom builtin '{}' leaked into the observation map",
                phantom
            );
        }
    }

    /// 6. Nothing eliminates a binding that is never read — observation reports
    ///    what the program bound, not what it used.
    #[test]
    fn unused_bindings_are_still_observed() {
        let map = observe("let rows = [1, 2, 3]\nprint(\"done\")\n");
        assert_eq!(map.get("rows"), Some(&serde_json::json!([1, 2, 3])));
    }

    /// 7. Observed values are heap ids. A collection that did not treat the
    ///    buffer as a root would sweep them and leave the reader decoding a
    ///    recycled slot.
    #[test]
    fn observed_values_survive_a_collection() {
        let (mut env, pid, sid) = run_observed(
            "let words = [\"alpha\", \"beta\"]\n\
             let greeting = \"hello world\"\n",
        );
        let ck = env.stacks.get(&sid).unwrap().context;
        env.collect_garbage(ck);
        let map = env.get_observations_json(pid, sid);
        assert_eq!(
            map.get("words"),
            Some(&serde_json::json!(["alpha", "beta"]))
        );
        assert_eq!(map.get("greeting"), Some(&serde_json::json!("hello world")));
    }

    /// 8. Off by default: recording costs nothing and reports nothing until a
    ///    host asks for it.
    #[test]
    fn observation_is_off_by_default() {
        let mut env = Env::new();
        let pid = env.load_program("let count = 42\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert!(env.get_observations_json(pid, sid).is_empty());
    }

    /// 9. The per-run clear must fire on a fresh entry only. A run driven in
    ///    tiny `run_bounded` budgets (how an editor drives a frame) must not
    ///    wipe its own observations on every resume.
    #[test]
    fn stepped_runs_observe_what_a_single_run_does() {
        let src = "let total = 0\n\
                   for i in range(0, 50) do\n\
                   \x20 total = total + i\n\
                   end\n\
                   let final_total = total\n";
        let mut env = Env::new();
        env.observations_mut().enable();
        let pid = env.load_program(src).unwrap();

        let reference = env.create_stack(pid).unwrap();
        env.run(reference).unwrap();
        let expected = env.get_observations_json(pid, reference);
        assert_eq!(expected.get("final_total"), Some(&serde_json::json!(1225)));

        let sid = env.create_stack(pid).unwrap();
        let mut guard = 0;
        loop {
            guard += 1;
            assert!(guard < 10_000, "run never completed");
            if let RunOutcome::Done(_) = env.run_bounded(sid, 5).unwrap() {
                break;
            }
        }
        assert_eq!(env.get_observations_json(pid, sid), expected);
    }

    /// 10. `state` variables are named terms too, so one uniform read covers
    ///     both plain bindings and persistent state.
    #[test]
    fn state_variables_are_observable() {
        let map = observe(
            "state tick = 0\n\
             let tick_plus = tick + 1\n",
        );
        assert_eq!(map.get("tick"), Some(&serde_json::json!(0)));
        assert_eq!(map.get("tick_plus"), Some(&serde_json::json!(1)));
    }

    /// Observation is a read-only side channel: turning it on must not change
    /// what the program computes or prints.
    #[test]
    fn observation_does_not_perturb_the_program() {
        let src = "fn tri(n)\n\
                   \x20 let acc = 0\n\
                   \x20 for i in range(0, n) do\n\
                   \x20   acc = acc + i\n\
                   \x20 end\n\
                   \x20 acc\n\
                   end\n\
                   for k in range(1, 6) do\n\
                   \x20 print(tri(k))\n\
                   end\n";

        let mut plain = Env::new();
        let pid = plain.load_program(src).unwrap();
        let sid = plain.create_stack(pid).unwrap();
        let plain_result = plain.run(sid).unwrap();
        let plain_output = plain.take_output();

        let mut observed = Env::new();
        observed.observations_mut().enable();
        let opid = observed.load_program(src).unwrap();
        let osid = observed.create_stack(opid).unwrap();
        let observed_result = observed.run(osid).unwrap();
        let observed_output = observed.take_output();

        assert_eq!(observed_output, plain_output);
        assert_eq!(observed_result, plain_result);
        assert!(!observed.get_observations_json(opid, osid).is_empty());
    }
}

/// `Env::set_seed` — the determinism knob the refactor verifier relies on
/// (docs/dev/refactor-verification.md §1).
mod seed_tests {
    use super::super::*;

    /// Run `source` with `seed` and return everything it printed.
    fn run_seeded(seed: u64, source: &str) -> Vec<String> {
        let mut env = Env::new();
        env.set_seed(seed);
        let pid = env.load_program(source).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        env.take_output()
    }

    const DRAWS: &str = "print(random(0, 1))\nprint(random(0, 1))\nprint(random(0, 1))\n";

    #[test]
    fn same_seed_gives_identical_sequence() {
        assert_eq!(run_seeded(7, DRAWS), run_seeded(7, DRAWS));
    }

    #[test]
    fn different_seeds_diverge() {
        assert_ne!(run_seeded(7, DRAWS), run_seeded(8, DRAWS));
    }

    /// Zero is not a legal xorshift state, so it maps to a constant — and must
    /// still be a *stable* stream, not a fresh clock seed each time.
    #[test]
    fn zero_seed_is_stable() {
        assert_eq!(run_seeded(0, DRAWS), run_seeded(0, DRAWS));
    }

    /// The seed is set once, not re-applied per frame: three `reset_stack`
    /// frames draw three *different* numbers, and the whole sequence replays.
    #[test]
    fn seed_governs_a_whole_multi_frame_session() {
        fn session(seed: u64) -> Vec<String> {
            let mut env = Env::new();
            env.set_seed(seed);
            let pid = env.load_program("print(random(0, 1))\n").unwrap();
            let sid = env.create_stack(pid).unwrap();
            for _ in 0..3 {
                env.run(sid).unwrap();
                env.reset_stack(sid).unwrap();
            }
            env.take_output()
        }
        let first = session(11);
        assert_eq!(first.len(), 3);
        assert_ne!(first[0], first[1], "frames must not repeat the same draw");
        assert_ne!(first[1], first[2]);
        assert_eq!(first, session(11));
    }
}

/// [`Env::set_echo`] — the embedder-level switch for whether `print` also
/// reaches the process's stdout. Off by default, so an embedder that reports
/// prints itself keeps its own output channel clean without asking; `petal
/// run` turns it on.
mod echo_tests {
    use super::super::*;

    fn ctx_echo(env: &Env) -> Vec<bool> {
        env.contexts.values().map(|c| c.echo).collect()
    }

    #[test]
    fn set_echo_reaches_every_context_and_output_is_collected_either_way() {
        let mut env = Env::new();
        assert!(ctx_echo(&env).iter().all(|&e| !e), "silent by default");

        // A silent echo does not silence the buffer: take_output still
        // returns what the program printed.
        let pid = env.load_program("print(\"hi\")\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), vec!["hi".to_string()]);

        env.set_echo(true);
        assert!(ctx_echo(&env).iter().all(|&e| e), "every context echoing");

        env.set_echo(false);
        assert!(ctx_echo(&env).iter().all(|&e| !e), "and it turns back off");
    }

    /// A fork made after `set_echo(true)` echoes too: the setting is the
    /// host's, not something a later context resets.
    #[test]
    fn a_fork_of_an_echoing_env_echoes() {
        let mut env = Env::new();
        env.set_echo(true);
        let pid = env.load_program("print(1)\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.fork_execution(sid).unwrap();
        assert!(env.contexts.len() > 1, "the fork has its own context");
        assert!(ctx_echo(&env).iter().all(|&e| e));
    }
}

/// The execution trace buffer holds `Value`s across runs *weakly*: it is not a
/// GC root (a 20 MB diagnostic ring must not pin a frame's garbage), so an
/// event can outlive the object it recorded. See docs/tasks/id-system-improvement.md.
mod trace_liveness_tests {
    use super::super::*;

    #[test]
    fn explain_does_not_show_a_recycled_slot_s_new_contents() {
        let mut env = Env::new();
        env.trace_mut().enable();
        let pid = env.load_program("let a = [1, 2, 3]\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        env.trace_mut().enabled = false;

        let term = env.get_program(pid).unwrap().find_term("a").unwrap();
        let old = match env.trace().last_for_term(term).unwrap().result {
            Value::List(id) => id,
            other => panic!("expected a list, got {other:?}"),
        };
        let explain_value = |env: &Env| {
            let p = env.get_program(pid).unwrap();
            env.trace().explain(p, env.heap(), term, 1).entries[0]
                .value
                .clone()
        };
        assert_eq!(explain_value(&env).as_deref(), Some("[1, 2, 3]"));

        // Drop the only root and collect, then allocate until the list's slot
        // is handed out again.
        env.drop_fork(sid);
        let ck = env.default_context;
        env.collect_garbage(ck);
        let mut keep = Vec::new();
        loop {
            let new = env.heap_mut().alloc_list(vec![Value::Int(99)]);
            keep.push(new);
            if new.index() == old.index() {
                assert_ne!(new, old, "a reused slot must mint a distinct id");
                break;
            }
            assert!(keep.len() < 100_000, "slot was never reused");
        }

        assert_eq!(explain_value(&env).as_deref(), Some("<collected>"));
    }
}

/// `restore_execution` swaps a snapshot's heap in under the live context's
/// key, so ids a host read before the restore must never alias an object
/// allocated after it.
mod restore_generation_tests {
    use super::super::*;
    use crate::heap::ListId;

    #[test]
    fn an_id_from_before_a_restore_never_matches_one_allocated_after() {
        let mut env = Env::new();
        let pid = env.load_program("let a = [1, 2, 3]\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let snapshot = env.fork_execution(sid).unwrap();
        let ck = env.default_context;

        // Grow the live heap past the snapshot and reuse its slots once, so
        // the held ids carry generations the snapshot never issued.
        // A host holds both generations: the first batch was collected (a
        // stale id a weak holder still has), the second is live.
        let mut held: Vec<ListId> = (0..16)
            .map(|_| env.heap_mut().alloc_list(vec![]))
            .collect();
        env.collect_garbage(ck);
        held.extend((0..16).map(|_| env.heap_mut().alloc_list(vec![Value::Int(7)])));
        assert!(held.iter().any(|id| id.generation() > 0));

        env.restore_execution(sid, snapshot).unwrap();
        env.collect_garbage(ck);
        let max_index = held.iter().map(|id| id.index()).max().unwrap();
        for _ in 0..1_000 {
            let id = env.heap_mut().alloc_list(vec![]);
            assert!(!held.contains(&id), "restore reissued {id:?}");
            if id.index() > max_index {
                break;
            }
        }
        for id in &held {
            assert!(!env.heap().is_live(Value::List(*id)));
        }
    }

    /// `+` on two lists is the JS/Python habit; the error names `++`.
    #[test]
    fn adding_two_lists_points_at_concatenation() {
        let mut env = Env::new();
        let err = env.run_source("let a = [1]\nlet b = [2]\na + b\n").unwrap_err();
        assert!(err.to_string().contains("use ++ to concatenate lists"), "{err}");
    }

    #[test]
    fn element_origins_are_weak_and_pruned_by_a_collection() {
        let mut env = Env::new();
        env.enable_element_trace(true);
        let program = env.load_program("state keep = <kept />\n[<a />, <b />]\n").unwrap();
        let stack = env.create_stack(program).unwrap();
        env.run(stack).unwrap();
        assert_eq!(env.element_origins().count(), 3);
        // The returned list is held by nothing once the run is over; the
        // state slot's element is.
        let ck = env.stacks.get(&stack).unwrap().context;
        env.stacks.get_mut(&stack).unwrap().reset_execution();
        env.collect_garbage(ck);
        let left: Vec<_> = env.element_origins().map(|(id, _)| id).collect();
        assert_eq!(left.len(), 1, "only the element in state survives");
        assert!(env.heap().is_live(Value::Element(left[0])));
    }
}
