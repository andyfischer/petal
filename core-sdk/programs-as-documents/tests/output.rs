//! Reading a document's output: the output calls a runner registers, which
//! one a script made, and `Runner::evaluate`'s plain JSON.

use programs_as_documents::{OutputCall, OutputSource, Runner};
use serde_json::json;

const DEMO: OutputCall = OutputCall { native: "demo_output", call: "demo(a, b)", arity: 2 };
const OTHER: OutputCall = OutputCall { native: "other_output", call: "other(a)", arity: 1 };

fn runner(output: OutputSource) -> Runner {
    Runner::new("/doc/t.ptl", |env| {
        let pkg = |name: &str, src: &'static str| {
            [(name.to_string(), src.to_string()), ("petal.toml".into(), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"))]
        };
        env.register_package("demo", pkg("demo", "native fn demo_output(a, b)\n\npub fn demo(a, b)\n  demo_output(a, b)\n"))
            .map_err(|e| e.to_string())?;
        env.register_package("other", pkg("other", "native fn other_output(a)\n\npub fn other(a)\n  other_output(a * 2)\n"))
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .with_output(output)
}

#[test]
fn reads_the_output_call() {
    let src = "import demo\nprint(\"hi\")\ndemo({w: 2}, [<a x={1} />])\n";
    let e = runner(OutputSource::Call(DEMO)).evaluate(src).unwrap();
    let el = json!({"type": "element", "tag": "a", "props": {"x": 1}, "children": []});
    assert_eq!(e.outputs, vec![json!({"w": 2}), json!([el])]);
    assert_eq!(e.log, vec!["hi".to_string()]);
    assert_eq!(e.output_call, Some(DEMO));
    // A traced run reads the same output, and maps it.
    let run = runner(OutputSource::Call(DEMO)).run(src).unwrap();
    assert_eq!(run.plain_outputs().unwrap(), e.outputs);
    assert_eq!(run.output_call, Some(DEMO));
    assert!(!run.map.nodes.is_empty());
}

#[test]
fn says_when_the_call_is_missing() {
    let err = runner(OutputSource::Call(DEMO)).evaluate("import demo\nlet x = 1\n").unwrap_err();
    assert_eq!(err, "the script never called demo(a, b)");
}

#[test]
fn says_which_output_call_a_script_made() {
    let any = || runner(OutputSource::AnyCall(vec![DEMO, OTHER]));
    let e = any().evaluate("import other\nother(21)\n").unwrap();
    assert_eq!((e.output_call, e.outputs), (Some(OTHER), vec![json!(42)]));
    assert_eq!(any().evaluate("import demo\ndemo(1, 2)\n").unwrap().output_call, Some(DEMO));
    let err = any().evaluate("let x = 1\n").unwrap_err();
    assert_eq!(err, "the script never called demo(a, b) or other(a)");
}

#[test]
fn refuses_an_output_call_made_twice() {
    let r = runner(OutputSource::Call(DEMO));
    let err = r.evaluate("import demo\ndemo(1, [])\nlet x = 2\ndemo(x, [])\n").unwrap_err();
    assert_eq!(err, "demo(a, b) is called twice, on line 2 and again on line 4: a document makes its output call once");
    // A traced run refuses it too, before running.
    assert_eq!(r.run("import demo\ndemo(1, [])\nlet x = 2\ndemo(x, [])\n").err(), Some(err));
    let err = r.evaluate("import demo\ndemo(1, [])\ndemo(2, [])\ndemo(3, [])\n").unwrap_err();
    assert!(err.starts_with("demo(a, b) is called 3 times, on line 2 and again on line 3"), "{err}");
}

#[test]
fn refuses_an_output_call_in_a_loop() {
    let r = runner(OutputSource::Call(DEMO));
    let err = r.evaluate("import demo\nfor i in [1, 2] do\n  demo(i, [])\nend\n").unwrap_err();
    assert_eq!(err, "demo(a, b) is called inside a loop (line 3), so it runs more than once: a document makes its output call once");
    let err = r.evaluate("import demo\nlet xs = for i in [1] do demo(i, []) end\n").unwrap_err();
    assert!(err.contains("inside a loop (line 2)"), "{err}");
}

#[test]
fn an_output_call_in_each_branch_is_one_call() {
    let r = runner(OutputSource::Call(DEMO));
    let src = "import demo\nlet big = true\nif big then\n  demo(1, [])\nelse\n  demo(2, [])\nend\n";
    assert_eq!(r.evaluate(src).unwrap().outputs, vec![json!(1), json!([])]);
    // ... but a call after the `if` is a second one.
    let err = r.evaluate(&format!("{src}demo(3, [])\n")).unwrap_err();
    assert!(err.contains("called twice, on line 4 and again on line 8"), "{err}");
}

#[test]
fn checks_each_of_several_output_calls() {
    let any = runner(OutputSource::AnyCall(vec![DEMO, OTHER]));
    assert!(any.evaluate("import demo\nimport other\ndemo(1, 2)\nother(3)\n").is_ok());
    let err = any.evaluate("import other\nother(1)\nother(2)\n").unwrap_err();
    assert!(err.starts_with("other(a) is called twice"), "{err}");
}

#[test]
fn a_call_the_document_binds_itself_is_not_the_output_call() {
    let src = "import demo as d\nfn demo(a)\n  a\nend\ndemo(1)\ndemo(2)\nd.demo(1, [])\n";
    assert!(programs_as_documents::check_output_calls(src, &[DEMO]).is_ok());
}

#[test]
fn a_probe_runs_with_the_output_call_registered() {
    let r = runner(OutputSource::Call(DEMO));
    let src = format!("import demo\ndemo({}(1 + 2), [])\n", programs_as_documents::run::PROBE);
    assert_eq!(r.probe(&src).unwrap(), vec![petal::static_value::StaticValue::Int(3)]);
}
