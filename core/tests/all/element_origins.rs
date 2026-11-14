//! Element-origin tracing: with `Env::enable_element_trace` on, every element
//! a run builds records the `AllocElement` term that built it and the call
//! chain that reached it, and `provenance::trace_value` follows a prop back to
//! the literal it was copied from.

use petal::env::Env;
use petal::execution_context::ElementOrigin;
use petal::program::{Program, TermId, TermOp};
use petal::provenance::{self, TraceEnd, TraceHop};
use petal::static_value::StaticValue;
use petal::value::Value;

/// Run `src` with element tracing on and return the env, the program id and
/// the elements of the list it evaluates to.
fn run_traced(src: &str) -> (Env, petal::program::ProgramId, Vec<Value>) {
    let mut env = Env::new();
    env.enable_element_trace(true);
    let pid = load(&mut env, src);
    let sid = env.create_stack(pid).unwrap();
    let v = env.run(sid).unwrap();
    let items = match v {
        Value::List(id) => env.heap().get_list(id).to_vec(),
        other => panic!("expected a list, got {other:?}"),
    };
    (env, pid, items)
}

fn origin(env: &Env, v: Value) -> ElementOrigin {
    let Value::Element(id) = v else { panic!("not an element: {v:?}") };
    env.element_origin(id).cloned().expect("element has an origin")
}

/// The source text a term covers.
fn text(program: &Program, term: TermId) -> String {
    provenance::term_text(program, term).expect("term has a span")
}

/// Load from a path, so the program's source map carries the file text.
fn load(env: &mut Env, src: &str) -> petal::program::ProgramId {
    env.load_program_at(src, std::path::Path::new("doc.ptl")).unwrap()
}

fn prop_trace(env: &Env, pid: petal::program::ProgramId, v: Value, prop: &str) -> provenance::ValueTrace {
    let program = env.get_program(pid).unwrap();
    let o = origin(env, v);
    let input = provenance::element_prop_term(program, o.term, prop).expect("prop is written");
    provenance::trace_value(program, input, &o.chain)
}

#[test]
fn top_level_literal_records_its_site_and_an_empty_chain() {
    let (env, pid, items) = run_traced("let items = [<rect x={10} />, <circle r={5} />]\nitems");
    let program = env.get_program(pid).unwrap();
    let a = origin(&env, items[0]);
    let b = origin(&env, items[1]);
    assert!(matches!(program.get_term(a.term).op, TermOp::AllocElement { .. }));
    assert!(a.chain.is_empty() && b.chain.is_empty(), "top-level code has no call site");
    assert_ne!(a.term, b.term);
    assert_eq!(text(program, a.term), "<rect x={10} />");
    assert_eq!(env.element_site_count(a.term), 1);

    let t = prop_trace(&env, pid, items[0], "x");
    assert!(t.is_direct());
    assert_eq!(t.constant(), Some(&StaticValue::Int(10)));
    assert_eq!(text(program, t.end), "10");
    assert_eq!(provenance::element_prop_term(program, a.term, "y"), None);
    assert_eq!(
        provenance::element_prop_terms(program, a.term).iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        ["x"]
    );
}

#[test]
fn helper_called_twice_records_two_chains_and_two_literals() {
    let src = "fn card(x, label)
  <rect x={x} label={label} w={40} />
end
let items = [card(100, \"a\"), card(x: 300, label: \"b\")]
items";
    let (env, pid, items) = run_traced(src);
    let program = env.get_program(pid).unwrap();
    let a = origin(&env, items[0]);
    let b = origin(&env, items[1]);
    assert_eq!(a.term, b.term, "one construction site");
    assert_eq!(a.chain.len(), 1);
    assert_eq!(b.chain.len(), 1);
    assert_ne!(a.chain, b.chain, "two call sites");
    assert_eq!(text(program, a.chain[0]), "card(100, \"a\")");
    assert_eq!(env.element_site_count(a.term), 2);

    // The prop flows from the argument of each element's own call.
    let ta = prop_trace(&env, pid, items[0], "x");
    let tb = prop_trace(&env, pid, items[1], "x");
    assert_eq!(ta.constant(), Some(&StaticValue::Int(100)));
    assert_eq!(tb.constant(), Some(&StaticValue::Int(300)), "named argument");
    assert!(ta.crosses_call() && tb.crosses_call());
    assert_eq!(ta.end_chain.as_deref(), Some(&[][..]), "the literal is in top-level code");
    let lb = prop_trace(&env, pid, items[1], "label");
    assert_eq!(lb.constant(), Some(&StaticValue::Str("b".into())));

    // A literal inside the helper is shared by both.
    let wa = prop_trace(&env, pid, items[0], "w");
    let wb = prop_trace(&env, pid, items[1], "w");
    assert_eq!(wa.end, wb.end);
    assert!(wa.is_direct());
    assert_eq!(wa.end_chain.as_deref(), Some(&a.chain[..]));
}

#[test]
fn for_loop_elements_share_one_site_and_chain() {
    let src = "let items = for i in range(3) do
  <rect x={i * 10} y={7} />
end
items";
    let (env, pid, items) = run_traced(src);
    assert_eq!(items.len(), 3);
    let o: Vec<ElementOrigin> = items.iter().map(|v| origin(&env, *v)).collect();
    assert!(o.iter().all(|x| x.term == o[0].term && x.chain == o[0].chain), "a loop pushes no frame");
    // Iterations are told apart by their occurrence.
    assert_eq!(o.iter().map(|x| x.occurrence).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert_eq!(env.element_site_count(o[0].term), 3);
    assert_eq!(prop_trace(&env, pid, items[1], "x").end_kind, TraceEnd::Computed);
    assert_eq!(prop_trace(&env, pid, items[1], "y").constant(), Some(&StaticValue::Int(7)));
}

#[test]
fn values_flow_through_bindings_and_record_fields() {
    let src = "let m = 96
let theme = {pad: 12, colors: {ink: \"#111\"}}
fn panel(t)
  <box x={m} pad={t.pad} fill={t.colors.ink} w={m + 1} />
end
[panel(theme)]";
    let (env, pid, items) = run_traced(src);
    let program = env.get_program(pid).unwrap();
    let x = prop_trace(&env, pid, items[0], "x");
    assert_eq!(x.constant(), Some(&StaticValue::Int(96)));
    assert!(!x.hops.is_empty());
    assert_eq!(text(program, x.end), "96");
    let pad = prop_trace(&env, pid, items[0], "pad");
    assert_eq!(pad.constant(), Some(&StaticValue::Int(12)));
    assert!(pad.hops.iter().any(|h| matches!(h, TraceHop::Field { field, .. } if field == "pad")));
    let fill = prop_trace(&env, pid, items[0], "fill");
    assert_eq!(fill.constant(), Some(&StaticValue::Str("#111".into())));
    assert_eq!(prop_trace(&env, pid, items[0], "w").end_kind, TraceEnd::Computed);
}

#[test]
fn map_callbacks_record_the_map_call_and_stop_at_their_parameter() {
    let src = "let items = [1, 2].map(fn(n) -> <dot n={n} />)\nitems";
    let (env, pid, items) = run_traced(src);
    let program = env.get_program(pid).unwrap();
    let o = origin(&env, items[0]);
    assert!(!o.chain.is_empty(), "the callback's frame has the map call as its call site");
    assert!(text(program, *o.chain.first().unwrap()).contains("map"));
    assert_eq!(prop_trace(&env, pid, items[0], "n").end_kind, TraceEnd::Computed);
}

#[test]
fn origins_survive_in_output_buffers() {
    let src = "import io
fn badge(t)
  <text>{t}</text>
end
push_output(symbol(\"doc\"), [badge(\"hi\"), <rule />])
nil";
    let mut env = Env::new();
    env.enable_element_trace(true);
    let pid = load(&mut env, src);
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let sym = env.intern_symbol("doc");
    let out = env.take_output_buffer(sym);
    let Value::List(list) = out[0] else { panic!() };
    let items = env.heap().get_list(list).to_vec();
    let program = env.get_program(pid).unwrap();
    let a = origin(&env, items[0]);
    assert_eq!(text(program, a.term), "<text>{t}</text>");
    assert_eq!(a.chain.len(), 1);
    let child = provenance::element_child_terms(program, a.term)[0];
    let t = provenance::trace_value(program, child, &a.chain);
    assert_eq!(t.constant(), Some(&StaticValue::Str("hi".into())));
    // `pick_frame` reads the full chain the way it reads an emit's.
    let file = program.source_map.get(a.term).unwrap().file;
    assert_eq!(provenance::pick_frame(program, &a.full_chain(), file), Some(a.term));
    assert!(origin(&env, items[1]).chain.is_empty());
}

#[test]
fn tracing_off_records_nothing() {
    let mut env = Env::new();
    assert!(!env.element_trace_enabled());
    let pid = env.load_program("fn f() <a /> end\n[f(), <b />]").unwrap();
    let sid = env.create_stack(pid).unwrap();
    let v = env.run(sid).unwrap();
    let Value::List(id) = v else { panic!() };
    for item in env.heap().get_list(id).to_vec() {
        let Value::Element(e) = item else { panic!() };
        assert!(env.element_origin(e).is_none());
    }
    assert_eq!(env.element_origins().count(), 0);
}

#[test]
fn reset_clears_origins_and_turning_tracing_off_drops_them() {
    let (mut env, pid, _items) = run_traced("[<a />]");
    assert_eq!(env.element_origins().count(), 1);
    let sid = env.create_stack(pid).unwrap();
    env.reset_stack(sid).unwrap();
    assert_eq!(env.element_origins().count(), 0);
    env.run(sid).unwrap();
    assert_eq!(env.element_origins().count(), 1);
    env.enable_element_trace(false);
    assert_eq!(env.element_origins().count(), 0);
}

#[test]
fn static_and_memo_caches_do_not_hide_elements_from_the_trace() {
    // Run twice on one stack: with the static cache or memo on, the second
    // run would reuse the first run's elements.
    let src = "fn row(i) <row i={i} /> end\n[row(1), row(1), <fixed v={1} />]";
    let mut env = Env::new();
    env.enable_element_trace(true);
    assert!(!env.static_cache_enabled());
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for _ in 0..2 {
        env.reset_stack(sid).unwrap();
        let v = env.run(sid).unwrap();
        let Value::List(id) = v else { panic!() };
        let items = env.heap().get_list(id).to_vec();
        for item in &items {
            let Value::Element(e) = *item else { panic!() };
            assert!(env.element_origin(e).is_some());
        }
        // A memo replay would hand the second `row(1)` the first one's element.
        assert_ne!(items[0], items[1]);
        assert_ne!(origin(&env, items[0]).chain, origin(&env, items[1]).chain);
    }
}

#[test]
fn captures_and_callbacks_resolve_against_their_own_frames() {
    // `x` is a parameter of `page`, captured by the callback `map` runs; the
    // callback's own parameter `n` comes from the list, not from a literal.
    let src = "fn page(x)
  [1, 2].map(fn(n) -> <a w={x} n={n} />)
end
fn apply(f)
  f(3)
end
let items = page(40) ++ [apply(fn(v) -> <b v={v} />)]
items";
    let (env, pid, items) = run_traced(src);
    let program = env.get_program(pid).unwrap();
    let a = origin(&env, items[0]);
    assert_eq!(a.chain.len(), 2, "the map call, then the call of page");
    assert!(text(program, a.chain[0]).contains("map"));
    assert_eq!(text(program, a.chain[1]), "page(40)");
    let w = prop_trace(&env, pid, items[0], "w");
    assert_eq!(w.constant(), Some(&StaticValue::Int(40)));
    assert!(w.hops.iter().any(|h| matches!(h, TraceHop::Capture { .. })));
    assert_eq!(prop_trace(&env, pid, items[0], "n").end_kind, TraceEnd::Computed);

    // A callback called directly by a Petal function: its call is `f(3)`.
    let b = origin(&env, items[2]);
    assert_eq!(text(program, b.chain[0]), "f(3)");
    assert_eq!(prop_trace(&env, pid, items[2], "v").constant(), Some(&StaticValue::Int(3)));
}

#[test]
fn a_props_record_passed_to_a_component_traces_per_field() {
    let src = "fn Card(props)
  <rect x={props.x} title={props.title} />
end
[Card({x: 5, title: \"one\"}), Card({x: 6, title: \"two\"})]";
    let (env, pid, items) = run_traced(src);
    let program = env.get_program(pid).unwrap();
    let x0 = prop_trace(&env, pid, items[0], "x");
    let x1 = prop_trace(&env, pid, items[1], "x");
    assert_eq!(x0.constant(), Some(&StaticValue::Int(5)));
    assert_eq!(x1.constant(), Some(&StaticValue::Int(6)));
    assert_ne!(x0.end, x1.end);
    assert_eq!(text(program, x1.end), "6");
    assert_eq!(prop_trace(&env, pid, items[1], "title").constant(), Some(&StaticValue::Str("two".into())));
}
