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
