//! A helper in a package, called once from the document with named
//! arguments for its defaulted parameters (petal-social-media's
//! `title_slide(post, "Hi", subtitle: "World", background: …)`): a field a
//! named argument sets is edited at that argument, and a field the helper
//! defaults or computes takes the host's override on the call when nothing
//! else applies.

mod common;

use std::rc::Rc;

use common::*;
use petal::env::Env;
use petal::static_value::StaticValue;
use programs_as_documents::{
    DocumentRun, Intent, OutputKey, OverrideEdit, OverrideHook, OverrideRequest, Resolution, ResolveOptions,
    ResolvedEdit, Runner, Scope, Step, resolve,
};

const SOCIAL: &str = "pub fn title_slide(title, subtitle = nil, size = 12)
  <slide title={title} subtitle={subtitle} size={size} pad={2 * 3} />
end
";

fn social_runner() -> Runner {
    Runner::new("doc.ptl", |env: &mut Env| {
        setup(env)?;
        let files = [("social", SOCIAL), (petal::package::MANIFEST_FILE, "[package]\nname = \"social\"\nversion = \"0.1.0\"\n")];
        env.register_package("social", files).map_err(|e| e.to_string())?;
        env.provides_package("social")
    })
}

fn set(target: OutputKey, field: &str, value: StaticValue) -> Intent {
    Intent::SetField { target, field: field.into(), value }
}

fn resolve_with(runner: &Runner, src: &str, opts: &ResolveOptions, intent: impl Fn(&DocumentRun) -> Intent) -> Resolution {
    let r = run(runner, src);
    let i = intent(&r);
    resolve(runner, &r, &i, opts)
}

fn edit(res: Resolution) -> ResolvedEdit {
    match res {
        Resolution::Edit(e) => e,
        other => panic!("expected an edit, got {other:?}"),
    }
}

/// Writes a named argument on the use site: `title_slide("Hi")` →
/// `title_slide("Hi", subtitle: "World")`.
struct NamedArg;

impl OverrideHook for NamedArg {
    fn write_override(&self, req: &OverrideRequest<'_>) -> Option<Result<OverrideEdit, String>> {
        let Intent::SetField { field, value, .. } = req.intent else { return None };
        let site = req.use_site.as_ref()?;
        let src = &req.run.source;
        let (a, b) = (site.span.start.offset as usize, site.span.end.offset as usize);
        let text = &src[a..b];
        let new_text = format!("{}, {field}: {})", text.strip_suffix(')')?, value.to_source());
        let source = format!("{}{new_text}{}", &src[..a], &src[b..]);
        Some(Ok(OverrideEdit { source, span: site.span, description: format!("override `{field}` on `{text}`") }))
    }
}

const NAMED: &str = "import social\n[title_slide(\"Hi\", subtitle: \"World\", size: 20)]\n";

#[test]
fn a_named_argument_to_a_defaulted_parameter_is_edited_at_the_argument() {
    let r = social_runner();
    let opts = ResolveOptions::default();
    let e = edit(resolve_with(&r, NAMED, &opts, |r| set(key_of(r, "slide", 0), "subtitle", StaticValue::str("Earth"))));
    assert_eq!(e.step, Step::DirectLiteral);
    assert_eq!(e.source, NAMED.replace("\"World\"", "\"Earth\""));
    assert_eq!(e.span.start.line, 2);
    let e = edit(resolve_with(&r, NAMED, &opts, |r| set(key_of(r, "slide", 0), "size", StaticValue::Int(30))));
    assert_eq!(e.source, NAMED.replace("size: 20", "size: 30"));
    // Passed positionally, the same.
    let src = "import social\n[title_slide(\"Hi\", \"World\")]\n";
    let e = edit(resolve_with(&r, src, &opts, |r| set(key_of(r, "slide", 0), "subtitle", StaticValue::str("Earth"))));
    assert_eq!(e.source, src.replace("\"World\"", "\"Earth\""));
}

#[test]
fn a_helper_used_once_takes_the_hooks_override_before_a_refusal() {
    let r = social_runner();
    let src = "import social\n[title_slide(\"Hi\")]\n";
    let k = |r: &DocumentRun| key_of(r, "slide", 0);
    // The subtitle is the package's default: without a hook, nothing applies.
    let res = resolve_with(&r, src, &ResolveOptions::default(), |r| set(k(r), "subtitle", StaticValue::str("World")));
    assert!(matches!(res, Resolution::Unresolved { .. } | Resolution::Refused { .. }), "{res:?}");
    // With one, the override on the call, verified.
    let opts = ResolveOptions { overrides: Some(Rc::new(NamedArg)), ..ResolveOptions::default() };
    let e = edit(resolve_with(&r, src, &opts, |r| set(k(r), "subtitle", StaticValue::str("World"))));
    assert_eq!(e.step, Step::Scope(Scope::Override));
    assert_eq!(e.source, src.replace("(\"Hi\")", "(\"Hi\", subtitle: \"World\")"));
    assert!(e.also_changes.is_empty());
    // An override that does not make the intent hold is not taken: `pad` is
    // computed in the package and is not a parameter.
    let res = resolve_with(&r, src, &opts, |r| set(k(r), "pad", StaticValue::Int(50)));
    assert!(!matches!(res, Resolution::Edit(_)), "{res:?}");
    // One that changes another output is not taken either.
    let src2 = "import social\nlet s = title_slide(\"Hi\")\n[s, s]\n";
    let res = resolve_with(&r, src2, &opts, |r| set(k(r), "subtitle", StaticValue::str("World")));
    assert!(!matches!(res, Resolution::Edit(_)), "{res:?}");
}
