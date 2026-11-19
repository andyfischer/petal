//! Reference intents (`Intent::SetFieldExpr`): a field written as an
//! expression, through the ladder, with names checked in scope and the
//! value verified by a probe; and freshness of resolved edits.

mod common;

use std::rc::Rc;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::{
    Choice, Document, DocumentRun, DocumentSession, Intent, OutputKey, OverrideEdit, OverrideHook, OverrideRequest,
    Resolution, ResolveOptions, ResolvedEdit, Runner, Scope, ScopePolicy, StaticExpr, Step, resolve, source_hash,
};

fn bind(target: OutputKey, field: &str, expr: &str) -> Intent {
    Intent::SetFieldExpr { target, field: field.into(), expr: StaticExpr::parse(expr).unwrap() }
}

fn resolve_src(runner: &Runner, src: &str, opts: &ResolveOptions, intent: impl Fn(&DocumentRun) -> Intent) -> Resolution {
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

fn choices(res: Resolution) -> Vec<Choice> {
    match res {
        Resolution::NeedsChoice { options, .. } => options,
        other => panic!("expected a choice, got {other:?}"),
    }
}

fn refused(res: Resolution) -> String {
    match res {
        Resolution::Refused { reason } | Resolution::Unresolved { reason, .. } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

const TOKENS: &str = "let tok = {color: {ink: {r: 10, g: 10, b: 14}, paper: {r: 255, g: 255, b: 255}}, space: {m: 16}}
";

#[test]
fn a_literal_attribute_becomes_the_reference() {
    let src = format!("{TOKENS}[\n  <rect id=\"a\" fill={{:ff0000}} x={{4}} />, # the box\n  <rect id=\"b\" fill={{:00ff00}} />,\n]\n");
    let e = edit(resolve_src(&value_runner(), &src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "fill", "tok.color.ink")
    }));
    assert_eq!(e.step, Step::DirectLiteral);
    assert!(e.source.contains("<rect id=\"a\" fill={tok.color.ink} x={4} />, # the box\n"), "{}", e.source);
    assert!(e.description.contains("tok.color.ink"), "{}", e.description);
    assert!(e.also_changes.is_empty());
    assert_eq!(e.base, Some(source_hash(&src)));
    // Written that way already: nothing to do.
    let again = resolve_src(&value_runner(), &e.source, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "fill", "tok.color.ink")
    });
    assert_eq!(again, Resolution::AlreadyHolds);
}

#[test]
fn a_field_not_written_yet_is_added_as_the_reference() {
    let src = format!("{TOKENS}[<rect id=\"a\" x={{4}} />]\n");
    let e = edit(resolve_src(&value_runner(), &src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "stroke", "tok.color.paper")
    }));
    assert_eq!(e.step, Step::SingleSite);
    assert!(e.source.contains("<rect id=\"a\" x={4} stroke={tok.color.paper} />"), "{}", e.source);
}

#[test]
fn a_reference_read_from_another_name_is_rewritten_on_the_element() {
    // `fill` reads `tok.color.ink` through `C`: the value comes from `tok`'s
    // literal, but the reference is written where the element reads it.
    let src = format!("{TOKENS}let C = tok.color\n[<rect id=\"a\" fill={{C.ink}} />]\n");
    let e = edit(resolve_src(&value_runner(), &src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "fill", "C.paper")
    }));
    assert!(e.source.contains("<rect id=\"a\" fill={C.paper} />"), "{}", e.source);
}

#[test]
fn a_call_with_named_arguments_is_written_named() {
    let src = "fn shadow(c, radius = 0, y = 0)
  {c: c, radius: radius, y: y}
end
let ink = {r: 1, g: 2, b: 3}
[<rect id=\"a\" shadow={shadow(ink, 4)} />]
";
    let e = edit(resolve_src(&value_runner(), src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "shadow", "shadow(ink, radius: 4, y: 3)")
    }));
    assert!(e.source.contains("shadow={shadow(ink, radius: 4, y: 3)}"), "{}", e.source);
}

#[test]
fn a_name_not_in_scope_at_the_site_is_refused_with_its_name() {
    let src = format!("{TOKENS}[<rect id=\"a\" fill={{:ff0000}} />]\n");
    let why = refused(resolve_src(&value_runner(), &src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "fill", "theme.ink")
    }));
    assert!(why.contains("`theme` is not in scope"), "{why}");
    // A function's parameter is in scope only inside it: `row.x` cannot go
    // into the top-level constant the field flows from, so it is written on
    // the element literal, inside `card`, instead.
    let src = "let w = 40
fn card(row)
  <card id={row.id} w={w} />
end
[card({id: \"a\", x: 7})]
";
    let e = edit(resolve_src(&value_runner(), src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "w", "row.x")
    }));
    assert_eq!(e.step, Step::SingleSite);
    assert!(e.source.starts_with("let w = 40\n") && e.source.contains("w={row.x}"), "{}", e.source);
    // Nowhere in scope: refused, naming it.
    let why = refused(resolve_src(&value_runner(), src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "w", "rows.x")
    }));
    assert!(why.contains("`rows` is not in scope"), "{why}");
}

#[test]
fn a_parameter_of_the_enclosing_function_is_in_scope_and_verified_by_probe() {
    // The probe reads the expression's value where it is written, inside
    // `card`, so a parameter works.
    let src = "fn card(row)
  <card id={row.id} w={40} />
end
[card({id: \"a\", x: 7})]
";
    let e = edit(resolve_src(&value_runner(), src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "w", "row.x")
    }));
    assert!(e.source.contains("<card id={row.id} w={row.x} />"), "{}", e.source);
}

#[test]
fn a_shared_site_offers_its_scopes() {
    let src = format!("{TOKENS}fn card(x)\n  <card x={{x}} fill={{:ff0000}} />\nend\n[card(1), card(2)]\n");
    let opts = ResolveOptions::default();
    let options = choices(resolve_src(&value_runner(), &src, &opts, |r| bind(key_of(r, "card", 0), "fill", "tok.color.ink")));
    assert_eq!(options[0].scope, Scope::EditShared);
    let shared = options[0].edit.as_ref().unwrap();
    assert!(shared.source.contains("fill={tok.color.ink}"), "{}", shared.source);
    assert_eq!(shared.also_changes.len(), 1, "{:?}", shared.also_changes);
}

/// Writes an override as a named argument at the use site: `card(2)` →
/// `card(2, fill: tok.color.ink)`.
struct NamedArg;

impl OverrideHook for NamedArg {
    fn write_override(&self, req: &OverrideRequest<'_>) -> Option<Result<OverrideEdit, String>> {
        let Intent::SetFieldExpr { field, expr, .. } = req.intent else { return None };
        let site = req.use_site.as_ref()?;
        let src = &req.run.source;
        let (a, b) = (site.span.start.offset as usize, site.span.end.offset as usize);
        let text = &src[a..b];
        let new_text = format!("{}, {field}: {expr})", text.strip_suffix(')')?);
        let source = format!("{}{new_text}{}", &src[..a], &src[b..]);
        Some(Ok(OverrideEdit { source, span: site.span, description: format!("override `{field}` on `{text}`") }))
    }
}

#[test]
fn an_override_of_a_reference_is_verified_against_the_value_at_top_level() {
    let src = format!("{TOKENS}fn card(x, fill = :ff0000)\n  <card x={{x}} fill={{fill}} />\nend\n[card(1), card(2)]\n");
    let opts = ResolveOptions {
        overrides: Some(Rc::new(NamedArg)),
        scope_policy: ScopePolicy::Prefer(vec![Scope::Override]),
        ..ResolveOptions::default()
    };
    let e = edit(resolve_src(&value_runner(), &src, &opts, |r| bind(key_of(r, "card", 1), "fill", "tok.color.ink")));
    assert_eq!(e.step, Step::Scope(Scope::Override));
    assert!(e.source.contains("[card(1), card(2, fill: tok.color.ink)]"), "{}", e.source);
}

#[test]
fn a_data_entry_can_take_a_reference() {
    let src = format!(
        "{TOKENS}fn card(row)\n  <card id={{row.id}} fill={{row.fill}} />\nend\nlet rows = [\n  {{id: \"a\", fill: :ff0000}},\n  {{id: \"b\", fill: :00ff00}},\n]\nrows.map(card)\n"
    );
    let e = edit(resolve_src(&value_runner(), &src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("b".into()), "fill", "tok.color.paper")
    }));
    assert_eq!(e.step, Step::Scope(Scope::EditData));
    assert!(e.source.contains("{id: \"b\", fill: tok.color.paper},"), "{}", e.source);
}

#[test]
fn a_dotted_field_inside_a_computed_record_is_refused() {
    let src = format!("{TOKENS}fn st()\n  {{fill: :ff0000}}\nend\n[<rect id=\"a\" style={{st()}} />]\n");
    let why = refused(resolve_src(&value_runner(), &src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "style.fill", "tok.color.ink")
    }));
    assert!(why.contains("set the whole `style`"), "{why}");
}

// ── freshness ──────────────────────────────────────────────────────────────

fn open(src: &str) -> Document {
    let path = std::env::temp_dir().join(format!("pad-fresh-{}.ptl", std::process::id()));
    Document::new(DocumentSession::new(path, src.to_string()), value_runner())
}

#[test]
fn a_choice_made_after_the_document_changed_is_refused() {
    let src = "fn card(x)\n  <card x={x} w={40} />\nend\n[card(1), card(2), <other id=\"o\" y={1} />]\n";
    let mut doc = open(src);
    let key = key_of(doc.run().unwrap(), "card", 0);
    let options = match doc.apply(&Intent::SetField { target: key, field: "w".into(), value: StaticValue::Int(50) }, None) {
        Resolution::NeedsChoice { options, .. } => options,
        other => panic!("{other:?}"),
    };
    let chosen = options[0].edit.clone().unwrap();
    // Meanwhile another edit lands.
    let r = doc.apply(&Intent::SetField { target: OutputKey::Id("o".into()), field: "y".into(), value: StaticValue::Int(9) }, None);
    assert!(matches!(r, Resolution::Edit(_)), "{r:?}");
    let after = doc.session.source().to_string();
    let err = doc.commit(&chosen, None).unwrap_err();
    assert!(err.contains("changed since"), "{err}");
    assert_eq!(doc.session.source(), after, "nothing was undone");
    // Resolved again, it commits.
    let key = key_of(doc.run().unwrap(), "card", 0);
    let options = match doc.apply(&Intent::SetField { target: key, field: "w".into(), value: StaticValue::Int(50) }, None) {
        Resolution::NeedsChoice { options, .. } => options,
        other => panic!("{other:?}"),
    };
    assert_eq!(doc.commit(options[0].edit.as_ref().unwrap(), None), Ok(true));
    assert!(doc.session.source().contains("w={50}") && doc.session.source().contains("y={9}"));
    // A host's own edit carries no base and is not checked.
    let own = ResolvedEdit { source: "[]\n".into(), step: Step::DirectLiteral, span: chosen.span, description: String::new(), also_changes: vec![], base: None };
    assert_eq!(doc.commit(&own, None), Ok(true));
}

// ── a token from another file ──────────────────────────────────────────────

const TOKS: &str = "pub let tok = {ink: {r: 10, g: 10, b: 14}, paper: {r: 255, g: 255, b: 255}}
";

fn token_runner() -> Runner {
    Runner::new("doc.ptl", |env| {
        setup(env)?;
        let files = [("toks", TOKS), (petal::package::MANIFEST_FILE, "[package]\nname = \"toks\"\nversion = \"0.1.0\"\n")];
        env.register_package("toks", files).map_err(|e| e.to_string())?;
        env.provides_package("toks")
    })
}

#[test]
fn a_field_bound_to_an_imported_token_is_rebound_on_the_element() {
    // `fill`'s value comes from the token package's literal, which is not
    // the document's to edit; the reference is the element's attribute.
    let src = "import toks\n[<rect id=\"a\" fill={tok.ink} />, <rect id=\"b\" fill={tok.ink} />]\n";
    let e = edit(resolve_src(&token_runner(), src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "fill", "tok.paper")
    }));
    assert_eq!(e.source, "import toks\n[<rect id=\"a\" fill={tok.paper} />, <rect id=\"b\" fill={tok.ink} />]\n");
    assert!(e.also_changes.is_empty(), "{:?}", e.also_changes);
    // And a literal to a token.
    let src = "import toks\n[<rect id=\"a\" fill={:ff0000} />]\n";
    let e = edit(resolve_src(&token_runner(), src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "fill", "tok.ink")
    }));
    assert_eq!(e.source, "import toks\n[<rect id=\"a\" fill={tok.ink} />]\n");
}

#[test]
fn a_name_that_means_something_else_at_the_site_is_refused() {
    // `tok` is the document's tokens at the top, but `swatch`'s parameter
    // inside it: `fill={tok.color.ink}` written in `swatch` would read the
    // parameter. The value at the site differs from the value at the top,
    // so the edit is refused rather than binding to the wrong thing.
    let src = format!(
        "{TOKENS}fn swatch(tok)\n  <rect id=\"a\" w={{10}} />\nend\n[swatch({{color: {{ink: {{r: 1, g: 1, b: 1}}}}}})]\n"
    );
    let why = refused(resolve_src(&value_runner(), &src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "fill", "tok.color.ink")
    }));
    assert!(why.contains("`tok.color.ink`") && why.contains("something else"), "{why}");
    // The same expression where `tok` is the document's: written.
    let src = format!("{TOKENS}fn swatch(c)\n  <rect id=\"a\" w={{10}} />\nend\n[swatch(1)]\n");
    let e = edit(resolve_src(&value_runner(), &src, &ResolveOptions::default(), |_| {
        bind(OutputKey::Id("a".into()), "fill", "tok.color.ink")
    }));
    assert!(e.source.contains("<rect id=\"a\" w={10} fill={tok.color.ink} />"), "{}", e.source);
}
