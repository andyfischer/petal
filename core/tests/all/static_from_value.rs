//! `StaticValue::from_value`: an evaluated value back to the literal that
//! would produce it.

use petal::env::Env;
use petal::static_value::StaticValue;

fn eval(src: &str) -> Option<StaticValue> {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let v = env.run(sid).unwrap();
    StaticValue::from_value(&v, env.heap())
}

#[test]
fn scalars_lists_and_records_convert() {
    assert_eq!(eval("1 + 2"), Some(StaticValue::Int(3)));
    assert_eq!(
        eval("[1.5, \"a\", nil, true]"),
        Some(StaticValue::List(vec![
            StaticValue::Float(1.5),
            StaticValue::str("a"),
            StaticValue::Nil,
            StaticValue::Bool(true)
        ]))
    );
    assert_eq!(
        eval("{x: 1, inner: {y: [2]}}"),
        Some(StaticValue::Record(vec![
            ("x".into(), StaticValue::Int(1)),
            ("inner".into(), StaticValue::Record(vec![("y".into(), StaticValue::List(vec![StaticValue::Int(2)]))])),
        ]))
    );
}

#[test]
fn an_element_converts_and_renders_as_source() {
    let v = eval("fn card(t) <card w={10 * 2}><title>{t}</title></card> end\ncard(\"hi\")").unwrap();
    assert_eq!(
        v,
        StaticValue::Element {
            tag: "card".into(),
            props: vec![("w".into(), StaticValue::Int(20))],
            children: vec![StaticValue::Element {
                tag: "title".into(),
                props: vec![],
                children: vec![StaticValue::str("hi")],
            }],
        }
    );
    assert!(v.to_source().starts_with("<card w={20}>"));
}

#[test]
fn a_function_has_no_literal() {
    assert_eq!(eval("fn f() 1 end\n[f]"), None);
}

#[test]
fn values_round_trip_through_their_source() {
    // Evaluate, write the value out as source, evaluate that: the same value,
    // with field order and int-versus-float kept.
    for src in [
        "{z: 1, a: 2.0, m: [3, 4.5], s: \"q\\\"uote\", n: nil}",
        "[round(2.6), 7 / 2, 1.0 * 3]",
        "fn chip(n) <chip z={n} a={n * 1.5} fill={:ff8800}><dot r={n} />{\"t\"}</chip> end\nchip(2)",
        "[<a />, {k: <b x={-1} />}]",
    ] {
        let v = eval(src).unwrap_or_else(|| panic!("no literal for {src}"));
        let again = eval(&v.to_source()).unwrap_or_else(|| panic!("no literal for {}", v.to_source()));
        assert_eq!(again, v, "{src} -> {}", v.to_source());
    }
    // A float that is a whole number stays a float.
    assert_eq!(eval("2.0 * 3").unwrap().to_source(), "6.0");
    // A color is the record it is at run time, so it round-trips as one.
    let c = eval(":ff8800").unwrap();
    assert_eq!(eval(&c.to_source()), Some(c));
}
