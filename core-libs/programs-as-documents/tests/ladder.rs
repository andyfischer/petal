//! Steps 3 to 6 of the resolution ladder: a shared site's scopes (the shared
//! literal, the data entry, a host override) and the policy that picks one;
//! solving a computed field with direct manipulation (holding other fields,
//! the entry file only, an unreachable target writing nothing); writing an
//! output out as source with the caller's consent; and the reasons given
//! when nothing applies.

mod common;

use std::rc::Rc;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::{
    Choice, Document, DocumentRun, DocumentSession, Intent, OutputKey, OverrideEdit, OverrideHook, OverrideRequest,
    Resolution, ResolveOptions, ResolvedEdit, Runner, Scope, ScopePolicy, Step, concretize, concretize_and_apply,
    resolve,
};

fn set(target: OutputKey, field: &str, value: StaticValue) -> Intent {
    Intent::SetField { target, field: field.into(), value }
}

fn resolve_with(runner: &Runner, src: &str, opts: &ResolveOptions, intent: impl Fn(&DocumentRun) -> Intent) -> Resolution {
    let r = run(runner, src);
    let i = intent(&r);
    resolve(runner, &r, &i, opts)
}

fn resolve_src(runner: &Runner, src: &str, intent: impl Fn(&DocumentRun) -> Intent) -> Resolution {
    resolve_with(runner, src, &ResolveOptions::default(), intent)
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

/// The value of `field` on the `i`-th `tag` of `src`'s run.
fn field(runner: &Runner, src: &str, tag: &str, i: usize, field: &str) -> StaticValue {
    let r = run(runner, src);
    let n = tagged(&r, tag)[i];
    let v = r.node_value(n).unwrap();
    programs_as_documents::intent::field_of(&v, field).cloned().unwrap_or(StaticValue::Nil)
}

// ── step 3: a shared site ──────────────────────────────────────────────────

/// Writes an override as a named argument on the use site: `card(2)` →
/// `card(2, w: 50)` (the helper has a defaulted `w`).
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

const CARDS: &str = "fn card(x, w = 40)
  <card x={x} w={w} />
end
[card(1), card(2)]
";

#[test]
fn a_shared_site_offers_its_scopes_and_a_policy_picks_one() {
    let r = value_runner();
    // `w` comes from a default the call left out, so it is solved for: the
    // default moves, and the edit says the other card changes too.
    let opts = ResolveOptions::default();
    let e = edit(resolve_with(&r, CARDS, &opts, |r| set(key_of(r, "card", 1), "w", StaticValue::Int(50))));
    assert_eq!(e.step, Step::Solve);
    assert_eq!(e.source, CARDS.replace("w = 40", "w = 50"));
    assert_eq!(e.also_changes.len(), 1, "{:?}", e.also_changes);

    // A literal several outputs share: editing it is a choice.
    let src = "fn card(x)\n  <card x={x} h={40} />\nend\n[card(1), card(2)]\n";
    let cs = choices(resolve_with(&r, src, &opts, |r| set(key_of(r, "card", 1), "h", StaticValue::Int(50))));
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].scope, Scope::EditShared);
    let shared = cs[0].edit.as_ref().unwrap();
    assert_eq!(shared.source, src.replace("h={40}", "h={50}"));
    assert_eq!(shared.step, Step::Scope(Scope::EditShared));
    assert_eq!(shared.also_changes.len(), 1, "{:?}", shared.also_changes);

    // With the host's override hook, the override is a second scope.
    let opts = ResolveOptions { overrides: Some(Rc::new(NamedArg)), ..ResolveOptions::default() };
    let cs = choices(resolve_with(&r, CARDS, &opts, |r| set(key_of(r, "card", 1), "w", StaticValue::Int(50))));
    let scopes: Vec<Scope> = cs.iter().map(|c| c.scope).collect();
    assert_eq!(scopes, vec![Scope::EditShared, Scope::Override]);
    assert_eq!(cs[0].edit.as_ref().unwrap().step, Step::Solve);
    let o = cs[1].edit.as_ref().unwrap();
    assert_eq!(o.source, CARDS.replace("card(2)]", "card(2, w: 50)]"));
    assert!(o.also_changes.is_empty());

    // A policy takes the first scope that verifies.
    let prefer = |order: Vec<Scope>| ResolveOptions {
        overrides: Some(Rc::new(NamedArg)),
        scope_policy: ScopePolicy::Prefer(order),
        ..ResolveOptions::default()
    };
    let e = edit(resolve_with(&r, CARDS, &prefer(vec![Scope::EditData, Scope::Override]), |r| {
        set(key_of(r, "card", 1), "w", StaticValue::Int(50))
    }));
    assert_eq!(e.step, Step::Scope(Scope::Override));
    let e = edit(resolve_with(&r, CARDS, &prefer(vec![Scope::EditShared]), |r| {
        set(key_of(r, "card", 1), "w", StaticValue::Int(50))
    }));
    assert_eq!(e.source, CARDS.replace("w = 40", "w = 50"));
    // A policy with no scope that applies still asks.
    assert!(matches!(
        resolve_with(&r, CARDS, &prefer(vec![Scope::EditData]), |r| set(key_of(r, "card", 1), "w", StaticValue::Int(50))),
        Resolution::NeedsChoice { .. }
    ));
}

#[test]
fn a_value_a_map_reads_per_item_is_edited_in_the_data() {
    let r = value_runner();
    // A named helper passed to `map`.
    let src = "let rows = [
  {title: \"one\", n: 1},
  {title: \"two\", n: 2},
  {title: \"three\", n: 3},
]
fn card(row)
  <card title={row.title} n={row.n * 10} />
end
rows.map(card)
";
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "card", 1), "title", StaticValue::str("TWO"))));
    assert_eq!(e.step, Step::Scope(Scope::EditData));
    assert_eq!(e.source, src.replace("\"two\"", "\"TWO\""));
    assert!(e.also_changes.is_empty());
    assert!(e.description.contains("[1].title"), "{}", e.description);
    // A lambda, and a helper called from it with the field.
    let src = "let rows = [{t: \"a\"}, {t: \"b\"}]
fn card(title)
  <card title={title} />
end
rows.map(fn(r) -> card(r.t))
";
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "card", 0), "title", StaticValue::str("z"))));
    assert_eq!(e.source, src.replace("{t: \"a\"}", "{t: \"z\"}"));
    // Data the element's computation reads is solved in the data, too.
    let src = "let rows = [{x: 1}, {x: 2}]\nrows.map(fn(r) -> <dot x={r.x * 10} />)\n";
    // A computation over the data is solved (the solver may move the data
    // or the factor).
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "dot", 1), "x", StaticValue::Int(50))));
    assert_eq!(e.step, Step::Solve);
    assert_eq!(field(&r, &e.source, "dot", 1, "x"), StaticValue::Int(50), "{}", e.source);
}

#[test]
fn a_value_a_for_loop_reads_per_item_is_edited_in_the_data() {
    let r = value_runner();
    let src = "let rows = [{t: \"a\"}, {t: \"b\"}, {t: \"c\"}]
let out = for row in rows do
  <card title={row.t} />
end
out
";
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "card", 2), "title", StaticValue::str("C"))));
    assert_eq!(e.step, Step::Scope(Scope::EditData));
    assert_eq!(e.source, src.replace("{t: \"c\"}", "{t: \"C\"}"));
    // Data in another file is not written.
    let src = "import shapes\nlet out = for s in [3, 4] do\n  badge(\"b\", s)\nend\ndoc(out)\n";
    let e = edit(resolve_src(&emit_runner(), src, |r| set(key_of(r, "badge", 1), "x", StaticValue::Int(9))));
    assert_eq!(e.source, src.replace("[3, 4]", "[3, 9]"));
}

// ── step 4: solve ──────────────────────────────────────────────────────────

#[test]
fn a_computed_field_is_solved_holding_the_others() {
    let r = value_runner();
    let src = "let m = 50.0\n[<dot x={m * 2.0} y={m} />]\n";
    // Holding `y`, only the factor can move.
    let opts = ResolveOptions { hold: vec!["y".into()], ..ResolveOptions::default() };
    let e = edit(resolve_with(&r, src, &opts, |r| set(key_of(r, "dot", 0), "x", StaticValue::Int(120))));
    assert_eq!(e.step, Step::Solve);
    assert_eq!(field(&r, &e.source, "dot", 0, "x"), StaticValue::Float(120.0));
    assert_eq!(field(&r, &e.source, "dot", 0, "y"), StaticValue::Float(50.0), "{}", e.source);
    assert!(e.also_changes.is_empty(), "{:?}", e.also_changes);
    // Moving the loop's second card moves the step between cards.
    let src = "let cards = for i in range(3) do\n  <card x={100 + i * 200} y={300} />\nend\ncards\n";
    let opts = ResolveOptions { hold: vec!["y".into()], ..ResolveOptions::default() };
    let e = edit(resolve_with(&r, src, &opts, |r| set(key_of(r, "card", 1), "x", StaticValue::Int(450))));
    assert_eq!(field(&r, &e.source, "card", 1, "x"), StaticValue::Int(450), "{}", e.source);
    assert_eq!(field(&r, &e.source, "card", 1, "y"), StaticValue::Int(300));
}

#[test]
fn an_unreachable_solve_writes_nothing() {
    let r = value_runner();
    let src = "[<dot x={len(\"abc\")} />]\n";
    let mut doc = Document::new(DocumentSession::new("doc.ptl".into(), src.into()), r.clone());
    let key = key_of(doc.run().unwrap(), "dot", 0);
    match doc.apply(&set(key, "x", StaticValue::Int(10)), None) {
        Resolution::Unresolved { reason, at } => {
            assert!(reason.contains("computed") && reason.contains("by hand"), "{reason}");
            assert_eq!(at.unwrap().start.line, 1);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(doc.session.source(), src);
    assert!(!doc.session.can_undo());
    // A string target is not solved for.
    let src = "fn label(t)\n  <label text={t ++ \"!\"} />\nend\n[label(\"hi\")]\n";
    match resolve_src(&r, src, |r| set(key_of(r, "label", 0), "text", StaticValue::str("yo"))) {
        Resolution::Unresolved { reason, .. } => assert!(reason.contains("not a number"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_solve_moves_only_numbers_in_the_document() {
    let dir = std::env::temp_dir().join(format!("pad-ladder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("layout.ptl"), "pub fn spot()\n  20 + 2 * 30\nend\n").unwrap();
    let r = Runner::new(dir.join("doc.ptl"), setup);
    // Only the module's numbers move `x`: refused, naming where to edit.
    let src = "import ./layout: spot\n[<dot x={spot()} />]\n";
    match resolve_src(&r, src, |r| set(key_of(r, "dot", 0), "x", StaticValue::Int(95))) {
        Resolution::Unresolved { reason, at } => {
            assert!(reason.contains("no number in this document"), "{reason}");
            assert_eq!(at.unwrap().start.line, 2);
        }
        other => panic!("{other:?}"),
    }
    // Letting the solver look further finds the module's edit, which this
    // crate does not write.
    let opts = ResolveOptions { allow_other_files: true, ..ResolveOptions::default() };
    match resolve_with(&r, src, &opts, |r| set(key_of(r, "dot", 0), "x", StaticValue::Int(95))) {
        Resolution::Unresolved { reason, .. } => assert!(reason.contains("layout.ptl"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // With a number of its own, the document's number moves.
    let src = "import ./layout: spot\n[<dot x={spot() + 10} />]\n";
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "dot", 0), "x", StaticValue::Int(95))));
    assert_eq!(e.source, src.replace("+ 10", "+ 15"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── step 5: concretize ─────────────────────────────────────────────────────

#[test]
fn an_output_is_written_out_as_source_only_with_consent() {
    let r = value_runner();
    let src = "fn label(t)\n  <label text={t ++ \"!\"} size={12} />\nend\nlet items = [\n  label(\"hi\"),\n  label(\"yo\"),\n]\nitems\n";
    let k = |r: &DocumentRun| key_of(r, "label", 1);
    // Without consent: a reason.
    assert!(matches!(
        resolve_src(&r, src, |r| set(k(r), "text", StaticValue::str("hey"))),
        Resolution::Unresolved { .. }
    ));
    // With it: the call is replaced by its value, then edited.
    let opts = ResolveOptions { concretize: true, ..ResolveOptions::default() };
    let e = edit(resolve_with(&r, src, &opts, |r| set(k(r), "text", StaticValue::str("hey"))));
    assert_eq!(e.step, Step::Concretize);
    assert_eq!(e.source, src.replace("label(\"yo\")", "<label text=\"hey\" size={12} />"));
    assert!(e.description.starts_with("write out `label(\"yo\")` (line 6)"), "{}", e.description);
    // The host can ask for it directly on a shared literal, too.
    let rn = run(&r, src);
    let intent = set(k(&rn), "size", StaticValue::Int(20));
    assert!(matches!(resolve(&r, &rn, &intent, &ResolveOptions::default()), Resolution::NeedsChoice { .. }));
    let e = edit(concretize_and_apply(&r, &rn, &intent, &ResolveOptions::default()).unwrap());
    assert_eq!(e.source, src.replace("label(\"yo\")", "<label text=\"yo!\" size={20} />"));
}

#[test]
fn writing_out_keeps_field_order_numbers_colors_and_children() {
    let r = value_runner();
    let src = "fn chip(n)\n  <chip z={n} a={n * 1.5} fill={:ff8800} pad={{t: 1, b: 2.0}}>\n    <dot r={n} />\n  </chip>\nend\n[\n  chip(2),\n]\n";
    let rn = run(&r, src);
    let c = concretize(&r, &rn, tagged(&rn, "chip")[0]).unwrap();
    let written = c.source.split("[\n  ").nth(1).unwrap();
    assert!(
        written.starts_with("<chip z={2} a={3.0} fill={:ff8800} pad={{t: 1, b: 2.0}}>"),
        "{}",
        c.source
    );
    assert!(written.contains("<dot r={2} />"), "{}", c.source);
    // The written-out document produces the same result.
    let after = run(&r, &c.source);
    assert_eq!(after.static_outputs(), rn.static_outputs());
    // A value with no literal form is not written out.
    let src = "fn btn()\n  <btn on_click={fn() -> 1} />\nend\n[btn()]\n";
    let rn = run(&r, src);
    let err = concretize(&r, &rn, tagged(&rn, "btn")[0]).unwrap_err();
    assert!(err.contains("no literal form"), "{err}");
    // A call that makes more than the output is not replaced.
    let src = "fn pair()\n  [<a />, <b />]\nend\npair()\n";
    let rn = run(&r, src);
    assert!(concretize(&r, &rn, tagged(&rn, "a")[0]).is_err());
}
