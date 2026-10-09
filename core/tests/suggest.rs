// `petal suggest` — the type-annotation suggestion channel
// (docs/dev/suggestions-plan.md).
//
// The unit tests in `src/typecheck/infer.rs` cover how evidence resolves into
// a type. These cover the command end to end: that real source produces the
// suggestion, that the insertion lands in the right place, and — the ones that
// matter most — that the cases which *should* stay silent do.

use petal::suggest::{SuggestOptions, apply, suggest_source};
use petal::typecheck::infer::Slot;

/// Suggestions for `src`, as `("fn/arity", "slot", "type")` triples.
fn suggestions(src: &str) -> Vec<(String, String, String)> {
    let out = suggest_source(src, None, &SuggestOptions::default()).expect("suggest");
    out.suggestions
        .iter()
        .map(|s| {
            let slot = match s.slot {
                Slot::Return => "return".to_string(),
                Slot::Param(_) => s.param.clone(),
            };
            (format!("{}/{}", s.function.0, s.function.1), slot, s.ty.clone())
        })
        .collect()
}

/// The source with every suggestion applied.
fn applied(src: &str) -> String {
    let out = suggest_source(src, None, &SuggestOptions::default()).expect("suggest");
    apply(src, &out.suggestions)
}

#[test]
fn a_body_tail_gives_the_return_type() {
    // `len` is `int` whatever it is handed, so the tail types on the first
    // pass without needing the parameter annotated first.
    let src = "fn count(xs)\n  len(xs)\nend\nprint(count([1, 2]))\n";
    let got = suggestions(src);
    assert!(
        got.contains(&("count/1".into(), "return".into(), "int".into())),
        "{got:?}"
    );
}

/// Suggestions compound: an applied annotation is evidence for the next pass.
/// `n * 2` types as `any` while `n` is un-annotated, so the return type is
/// only inferable once `n: num` has been written — and `num * int` must stay
/// `num` rather than collapsing to `any`, or the chain would stop here.
#[test]
fn a_second_pass_infers_what_the_first_pass_made_visible() {
    let src = "fn twice(n)\n  n * 2\nend\nprint(twice(2))\n";
    let first = applied(src);
    assert_eq!(first, "fn twice(n: num)\n  n * 2\nend\nprint(twice(2))\n");
    let second = applied(&first);
    assert_eq!(
        second,
        "fn twice(n: num) -> num\n  n * 2\nend\nprint(twice(2))\n"
    );
    // And a third pass has nothing left to say.
    assert_eq!(applied(&second), second);
}

/// A parameter's numeric type is `num` whatever this compile's callers
/// happened to pass — see the precondition/promise split in `infer.rs`.
#[test]
fn a_numeric_call_site_suggests_num_for_the_parameter() {
    let got = suggestions("fn f(a)\n  a\nend\nprint(f(1))\nprint(f(2))\n");
    assert!(
        got.contains(&("f/1".into(), "a".into(), "num".into())),
        "{got:?}"
    );
}

/// The declared type of the slot a parameter is forwarded into. This is the
/// direction that compounds: one annotation makes its callers inferable.
#[test]
fn a_parameter_forwarded_to_an_annotated_slot_takes_that_type() {
    let src = "fn inner(s: string)\n  s\nend\nfn outer(x)\n  inner(x)\nend\nprint(outer(\"a\"))\n";
    let got = suggestions(src);
    assert!(
        got.contains(&("outer/1".into(), "x".into(), "string".into())),
        "{got:?}"
    );
}

/// Field reads prove record-shaped, not the class that happens to declare
/// those fields — a plain record has them too and is not assignable to a
/// class. The report names the class; the annotation does not.
#[test]
fn field_reads_suggest_record_and_name_the_matching_class_in_the_reason() {
    let src = "fn area(r)\n  r.w * r.h\nend\nprint(area(rect(0, 0, 2, 3)))\n";
    let out = suggest_source(src, None, &SuggestOptions::default()).expect("suggest");
    let r = out
        .suggestions
        .iter()
        .find(|s| s.param == "r")
        .expect("a suggestion for `r`");
    assert_eq!(r.ty, "record", "{:?}", r.because);
    assert!(r.because.contains("Rect"), "{}", r.because);
}

#[test]
fn a_slot_the_author_already_annotated_is_never_suggested_for() {
    let got = suggestions("fn f(a: int) -> int\n  a\nend\nprint(f(1))\n");
    assert!(got.is_empty(), "{got:?}");
}

/// Disagreeing callers are the commonest reason to stay silent, and staying
/// silent is the right answer: there is no one type to write.
#[test]
fn disagreeing_call_sites_suggest_nothing() {
    let got = suggestions("fn f(a)\n  a\nend\nprint(f(1))\nprint(f(\"two\"))\n");
    assert!(
        !got.iter().any(|(_, slot, _)| slot == "a"),
        "{got:?}"
    );
}

/// An overload is a separate declaration with its own arity, and its own
/// suggestions.
#[test]
fn overloads_are_suggested_for_separately() {
    let src = "fn f(a)\n  f(a, \"x\")\nend\nfn f(a, b)\n  b\nend\nprint(f(1))\n";
    let got = suggestions(src);
    assert!(
        got.contains(&("f/2".into(), "b".into(), "string".into())),
        "{got:?}"
    );
}

#[test]
fn the_insertion_lands_after_the_parameter_name_and_after_the_paren() {
    let src = "fn count(xs)\n  len(xs)\nend\nprint(count([1, 2]))\n";
    assert_eq!(
        applied(src),
        "fn count(xs: list) -> int\n  len(xs)\nend\nprint(count([1, 2]))\n"
    );
}

/// Layout inside the parameter list survives: the scan inserts, it never
/// reprints.
#[test]
fn an_awkwardly_laid_out_parameter_list_is_spliced_not_reprinted() {
    let src = "fn f(\n  a,\n  b\n)\n  a\nend\nprint(f(1, \"x\"))\n";
    let out = applied(src);
    assert!(out.starts_with("fn f(\n  a: num,\n  b: string\n)"), "{out}");
}

/// A lambda has no annotatable return slot and no name to key on, so nothing
/// inside one is suggested for.
#[test]
fn lambdas_are_left_alone() {
    let got = suggestions("let double = fn(n) -> n * 2\nprint(double(2))\n");
    assert!(got.is_empty(), "{got:?}");
}

/// An annotation is compile-time only, so applying suggestions must leave the
/// program's IR untouched. (The one documented exception is an annotation that
/// lets the compiler pin a method call to a class; nothing here does that.)
#[test]
fn applying_suggestions_does_not_change_the_ir() {
    let src = "fn twice(n)\n  n * 2\nend\nfn label(s)\n  \"x\" ++ s\nend\n\
               print(twice(2))\nprint(label(\"a\"))\n";
    let out = applied(src);
    assert_ne!(out, src, "the fixture should produce suggestions");

    let verdict = petal::ir_equiv::sources_equivalent(src, &out, &[], None).expect("both compile");
    assert!(verdict.is_ok(), "IR changed: {:?}", verdict.err());
}

// ── Advice: a comment about the code, with no rewrite ───────────────────────

/// `(rule, subject, line)` for each piece of advice about `src`.
fn advice(src: &str) -> Vec<(String, String, usize)> {
    let out = suggest_source(src, None, &SuggestOptions::default()).expect("suggest");
    out.advice
        .iter()
        .map(|a| (a.rule.to_string(), a.subject.clone(), a.line))
        .collect()
}

const INSERTION_SORT: &str = "fn less(a, b)
  a.k < b.k
end
fn sorted(rows)
  var out = []
  for r in rows do
    var placed = false
    var res = []
    for x in out do
      if !placed && less(r, x) then
        set res = append(res, r)
        set placed = true
      end
      set res = append(res, x)
    end
    if !placed then set res = append(res, r) end
    set out = res
  end
  out
end
print(sorted([{k: 2}, {k: 1}]))
";

#[test]
fn a_hand_written_insertion_sort_gets_advice() {
    assert_eq!(
        advice(INSERTION_SORT),
        [("hand-written-sort".to_string(), "fn sorted".to_string(), 6)]
    );
}

/// When the insertion test is `less(r, x)`, that function is the comparator
/// `sort` wants, and the advice says so by name.
#[test]
fn the_advice_names_a_comparator_it_can_see() {
    let out = suggest_source(INSERTION_SORT, None, &SuggestOptions::default()).expect("suggest");
    let message = &out.advice[0].message;
    assert!(message.contains("looks like"), "{message}");
    assert!(message.contains("`sort(list, less)`"), "{message}");

    // An inline test has no function to name; the advice stays general.
    let inline = INSERTION_SORT.replace("less(r, x)", "r.k < x.k");
    let out = suggest_source(&inline, None, &SuggestOptions::default()).expect("suggest");
    assert_eq!(out.advice.len(), 1);
    assert!(!out.advice[0].message.contains("is likely the whole loop"));
}

/// Advice is a comment: it is not a suggestion with an edit, so nothing about
/// it can reach `--apply`.
#[test]
fn advice_carries_no_edit() {
    let out = suggest_source(INSERTION_SORT, None, &SuggestOptions::default()).expect("suggest");
    assert_eq!(out.advice.len(), 1);
    let annotated = apply(INSERTION_SORT, &out.suggestions);
    let again = suggest_source(&annotated, None, &SuggestOptions::default()).expect("suggest");
    assert_eq!(again.advice.len(), 1, "applying the rest leaves the advice standing");
}

#[test]
fn advice_can_be_selected_or_left_out() {
    use petal::suggest::Kinds;
    let only = |list: &str| SuggestOptions {
        kinds: Kinds::parse(list).expect("kinds"),
        ..SuggestOptions::default()
    };
    let out = suggest_source(INSERTION_SORT, None, &only("advice")).expect("suggest");
    assert_eq!(out.advice.len(), 1);
    assert!(out.suggestions.is_empty() && out.named_args.is_empty());
    let out = suggest_source(INSERTION_SORT, None, &only("types,named-args")).expect("suggest");
    assert!(out.advice.is_empty());
}

/// Nested loops that build a list are everywhere; only the insertion step —
/// the outer element appended under a test against the inner one, and the
/// inner one kept — is a sort.
#[test]
fn ordinary_nested_loops_get_no_advice() {
    for src in [
        // A cross product.
        "var out = []\nfor a in [1, 2] do\n  for b in [3, 4] do\n    set out = append(out, a * b)\n  end\nend\nprint(out)\n",
        // A filter against another list: appends the outer, never the inner.
        "var out = []\nfor a in [1, 2] do\n  for b in [2, 3] do\n    if a == b then set out = append(out, a) end\n  end\nend\nprint(out)\n",
        // A flatten: appends the inner, never tests against the outer.
        "var out = []\nfor row in [[1], [2]] do\n  for x in row do\n    set out = append(out, x)\n  end\nend\nprint(out)\n",
        // The builtin, which is the point.
        "print(sort([2, 1], fn(a, b) -> a - b))\n",
    ] {
        assert!(advice(src).is_empty(), "{src}");
    }
}
