// `petal suggest`'s named-argument refactor (`src/suggest/named_args.rs`).
//
// The unit tests beside the code cover the placeholder rule, and
// `src/named_calls.rs` covers callee resolution. These cover the suggestion
// end to end: which calls are rewritten and how, the calls that must be left
// alone, that applying is idempotent and keeps everything it did not touch,
// and — the one that matters — that every rewrite passes the proof `--apply`
// holds it to, while a rewrite that is *not* the same call fails it.

use std::process::Command;

use petal::suggest::{Kinds, SuggestOptions, apply_all, suggest_source, verify_named_args};
use petal::typecheck::globals::HostProfile;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

fn opts(host: HostProfile) -> SuggestOptions {
    SuggestOptions {
        host,
        kinds: Kinds {
            types: false,
            named_args: true,
            return_types: false,
            advice: false,
        },
        ..Default::default()
    }
}

/// Each suggested call in `src`, as it would read.
fn rewrites_for(src: &str, host: HostProfile) -> Vec<String> {
    let out = suggest_source(src, None, &opts(host)).expect("suggest");
    out.named_args.iter().map(|n| n.after.clone()).collect()
}

fn rewrites(src: &str) -> Vec<String> {
    rewrites_for(src, HostProfile::Core)
}

/// `src` with every named-argument suggestion applied — after checking that
/// the result is provably the same program, as `--apply` does.
fn applied_for(src: &str, host: HostProfile) -> String {
    let out = suggest_source(src, None, &opts(host)).expect("suggest");
    let rewritten = apply_all(src, &[], &[], &out.named_args);
    if rewritten != src {
        verify_named_args(src, &rewritten, None, &opts(host))
            .unwrap_or_else(|e| panic!("{e}\n--- rewritten ---\n{rewritten}"));
    }
    rewritten
}

fn applied(src: &str) -> String {
    applied_for(src, HostProfile::Core)
}

const BOX: &str = "fn box(x, y, w, h)\n  x + y + w + h\nend\n";

// --- what is suggested -------------------------------------------------------

#[test]
fn a_call_with_three_or_more_positional_arguments_is_named() {
    let src = format!("{BOX}print(box(1, 2, 3, 4))\n");
    assert_eq!(rewrites(&src), ["box(x: 1, y: 2, w: 3, h: 4)"]);
}

#[test]
fn fewer_than_three_arguments_is_left_alone() {
    let src = "fn pair(left, right)\n  left + right\nend\nprint(pair(1, 2))\n";
    assert!(rewrites(src).is_empty());
}

#[test]
fn the_variant_the_count_selects_lends_its_names() {
    let src = "fn area(width, height, depth)\n  width * height * depth\nend\n\
               fn area(width, height, depth, scale)\n  width * height * depth * scale\nend\n\
               print(area(1, 2, 3))\nprint(area(1, 2, 3, 4))\n";
    assert_eq!(
        rewrites(src),
        [
            "area(width: 1, height: 2, depth: 3)",
            "area(width: 1, height: 2, depth: 3, scale: 4)"
        ]
    );
}

#[test]
fn a_defaulted_parameter_left_out_stays_left_out() {
    let src =
        "fn frame(x, y, w, h, pad = 2)\n  x + y + w + h + pad\nend\nprint(frame(1, 2, 3, 4))\n";
    assert_eq!(rewrites(src), ["frame(x: 1, y: 2, w: 3, h: 4)"]);
}

#[test]
fn constructors_lambdas_and_later_declarations_resolve() {
    let src = "class Point3\n  x, y, z\nend\n\
               let mix = fn(from, to, amount) from + (to - from) * amount end\n\
               fn early()\n  late(1, 2, 3)\nend\n\
               fn late(first, second, third)\n  first + second + third\nend\n\
               print(Point3(1, 2, 3))\nprint(mix(0, 10, 0.5))\nprint(early())\n";
    assert_eq!(
        rewrites(src),
        [
            "late(first: 1, second: 2, third: 3)",
            "Point3(x: 1, y: 2, z: 3)",
            "mix(from: 0, to: 10, amount: 0.5)"
        ]
    );
}

#[test]
fn a_builtin_that_declares_its_parameters_is_named() {
    let src = "print(map_range(5, 0, 10, 0, 100))\nprint(hsv(0.5, 1.0, 1.0))\n";
    assert_eq!(
        rewrites(src),
        [
            "map_range(5, in_lo: 0, in_hi: 10, out_lo: 0, out_hi: 100)",
            "hsv(h: 0.5, s: 1.0, v: 1.0)"
        ]
    );
}

// --- calls not worth naming --------------------------------------------------

#[test]
fn a_bare_colour_is_left_alone() {
    let src = "fn tint(r, g, b)\n  r + g + b\nend\n\
               fn glass(r, g, b, a)\n  r + g + b + a\nend\n\
               print(tint(1, 2, 3))\nprint(glass(1, 2, 3, 4))\n";
    assert_eq!(rewrites(src), [] as [&str; 0]);
    // Channels that are only part of a call are named with the rest of it.
    let src = "fn fill(rect, r, g, b)\n  rect + r + g + b\nend\n\
               fn dot(x, y, r, g, b)\n  x + y + r + g + b\nend\n\
               print(fill(0, 1, 2, 3))\nprint(dot(0, 0, 1, 2, 3))\n";
    assert_eq!(
        rewrites(src),
        [
            "fill(0, r: 1, g: 2, b: 3)",
            "dot(x: 0, y: 0, r: 1, g: 2, b: 3)"
        ]
    );
}

#[test]
fn a_function_literal_is_not_labelled_with_one_letter() {
    let src = "fn each(count, step, f)\n  f(count * step)\nend\n\
               fn button(label, width, on_click)\n  on_click(label)\nend\n\
               print(reduce([1, 2], 0, fn(a, b) -> a + b))\n\
               print(each(3, 2, fn(n) -> n))\n\
               let double = fn(n) -> n * 2\nprint(each(3, 2, double))\n\
               print(button(\"ok\", 80, fn(l) -> l))\n";
    assert_eq!(
        rewrites(src),
        [
            "each(count: 3, step: 2, f: double)",
            "button(label: \"ok\", width: 80, on_click: fn(l) -> l)"
        ]
    );
}

#[test]
fn a_call_that_would_mostly_echo_its_arguments_is_left_alone() {
    let src = "fn hash(ix, iy, seed)\n  ix + iy + seed\nend\n\
               let ix = 1\nlet iy = 2\nlet seed = 3\n\
               print(hash(ix + 1, iy, seed))\nprint(hash(ix + 1, iy + 1, seed))\n";
    // Two echoes of three is noise; one of three still says more than it repeats.
    assert_eq!(rewrites(src), ["hash(ix: ix + 1, iy: iy + 1, seed: seed)"]);
}

#[test]
fn a_subject_under_its_short_name_stays_positional() {
    let src = "fn tx(s, x, y, style)\n  s\nend\n\
               fn iclamp(v, lo, hi)\n  v + lo + hi\nend\n\
               fn pill(r, label, active)\n  r\nend\n\
               fn dot(r, g, b, size)\n  r + g + b + size\nend\n\
               print(tx(\"hi\", 1, 2, 3))\nprint(iclamp(7, 0, 5))\n\
               print(pill(1, \"Fit\", false))\nprint(dot(1, 2, 3, 4))\n";
    // A leading `r` is a rect, and stays positional — unless it is red.
    assert_eq!(
        rewrites(src),
        [
            "tx(\"hi\", x: 1, y: 2, style: 3)",
            "iclamp(7, lo: 0, hi: 5)",
            "pill(1, label: \"Fit\", active: false)",
            "dot(r: 1, g: 2, b: 3, size: 4)"
        ]
    );
}

#[test]
fn a_pinned_method_call_names_what_it_writes() {
    let src = "class V\n  x, y\nend\n\
               fn V.moved(self, dx, dy, scale)\n  V(self.x + dx * scale, self.y + dy * scale)\nend\n\
               let v: V = V(1, 2)\nprint(v.moved(3, 4, 2))\n";
    assert_eq!(rewrites(src), ["v.moved(dx: 3, dy: 4, scale: 2)"]);
}

#[test]
fn a_piped_call_names_only_the_arguments_in_its_parentheses() {
    let src = format!("{BOX}print(1 |> box(2, 3, 4))\n");
    assert_eq!(rewrites(&src), ["box(y: 2, w: 3, h: 4)"]);
}

#[test]
fn a_prelude_function_is_named_through_the_implicit_import() {
    let src = "draw_rect(0, 0, 320, 48, 20, 24, 32)\n";
    assert_eq!(
        rewrites_for(src, HostProfile::Ui),
        ["draw_rect(x: 0, y: 0, w: 320, h: 48, r: 20, g: 24, b: 32)"]
    );
    // Without the host's prelude the name is an unknown global: nothing to
    // resolve, nothing suggested.
    assert!(rewrites(src).is_empty());
}

// --- which arguments stay positional ----------------------------------------

#[test]
fn the_subject_stays_positional() {
    let src = "let xs = [1, 2, 3, 4]\nprint(slice(xs, 1, 3))\nprint(clamp(7, 0, 5))\n";
    assert_eq!(
        rewrites(src),
        ["slice(xs, start: 1, end: 3)", "clamp(7, lo: 0, hi: 5)"]
    );
}

#[test]
fn placeholder_parameters_stay_positional() {
    // `a, b` only count; `t` means something.
    assert_eq!(
        rewrites("print(lerp(0, 10, 0.5))\n"),
        ["lerp(0, 10, t: 0.5)"]
    );
    // Nothing but placeholders: nothing to name.
    let src = "fn sum3(a, b, c)\n  a + b + c\nend\nfn pick(_, p1, p2)\n  p1\nend\n\
               print(sum3(1, 2, 3))\nprint(pick(0, 1, 2))\n";
    assert!(rewrites(src).is_empty(), "{:?}", rewrites(src));
}

#[test]
fn an_argument_spelled_like_its_parameter_stays_positional_while_it_can() {
    let src = format!(
        "{BOX}let x = 1\nlet y = 2\nlet h = 9\nprint(box(x, y, 3, 4))\nprint(box(1, y, 3, h))\n\
         print(box(x, y, 3, h))\n"
    );
    assert_eq!(
        rewrites(&src),
        [
            // The leading run needs no label…
            "box(x, y, w: 3, h: 4)",
            // …but positional arguments cannot follow a named one, so `y`
            // and `h` are labelled here.
            "box(x: 1, y: y, w: 3, h: h)",
            "box(x, y, w: 3, h: h)"
        ]
    );
}

#[test]
fn arguments_already_named_are_kept_as_written() {
    let src = format!("{BOX}print(box(1, 2, h: 4, w: 3))\n");
    assert_eq!(rewrites(&src), ["box(x: 1, y: 2, h: 4, w: 3)"]);
    let done = format!("{BOX}print(box(x: 1, y: 2, w: 3, h: 4))\n");
    assert!(rewrites(&done).is_empty());
}

// --- what is left alone -----------------------------------------------------

#[test]
fn an_opaque_callee_is_left_alone() {
    // A parameter, a record field, a name rebound under a branch, a `var`
    // written twice, and a method dispatched on its receiver at runtime.
    let src = format!(
        "{BOX}fn apply(f)\n  f(1, 2, 3, 4)\nend\n\
         let ops = {{run: box}}\nprint(ops.run(1, 2, 3, 4))\n\
         let g = box\nif len([1]) > 0 then\n  g = fn(p, q, r, s) p end\nend\nprint(g(1, 2, 3, 4))\n\
         var h = box\nset h = fn(p, q, r, s) q end\nprint(h(1, 2, 3, 4))\n\
         print([1, 2, 3, 4].slice(0, 2, 1))\nprint(apply(box))\n"
    );
    assert!(rewrites(&src).is_empty(), "{:?}", rewrites(&src));
}

#[test]
fn a_rebound_name_takes_the_names_of_what_it_holds_at_the_call() {
    let src = "fn one(first, second, third)\n  first\nend\n\
               fn two(red, green, blue)\n  blue\nend\n\
               fn main()\n  let k = one\n  let run = fn() k(1, 2, 3) end\n  k = two\n  run() + k(4, 5, 6)\nend\n\
               print(main())\n";
    assert_eq!(
        rewrites(src),
        [
            // The lambda captured `k` while it held `one`.
            "k(first: 1, second: 2, third: 3)",
            "k(red: 4, green: 5, blue: 6)"
        ]
    );
}

#[test]
fn a_variadic_builtin_is_left_alone() {
    assert!(rewrites("print(1, 2, 3, 4)\nprint(max(1, 2))\n").is_empty());
}

#[test]
fn two_shapes_on_one_count_are_left_alone() {
    // Both variants take four arguments; the four-parameter one is chosen,
    // but its names are only one of the two readings. Three or five are only
    // the first.
    let src = "fn mark(x, y, size, tint = 0, alpha = 255)\n  x + y + size\nend\n\
               fn mark(pos, size, tint, alpha)\n  size\nend\n\
               print(mark(1, 2, 3, 4))\nprint(mark(1, 2, 3))\n";
    assert_eq!(rewrites(src), ["mark(x: 1, y: 2, size: 3)"]);
    // The `ui` prelude's `draw_line` is the case this exists for: seven
    // arguments are the flat form *and* `(x1, y1, x2, y2, c, a, width)`.
    let flat = "draw_line(0, 0, 10, 10, 255, 0, 0)\n";
    assert!(rewrites_for(flat, HostProfile::Ui).is_empty());
    // Eight are only the flat form.
    let with_alpha = "draw_line(0, 0, 10, 10, 255, 0, 0, 128)\n";
    assert_eq!(
        rewrites_for(with_alpha, HostProfile::Ui),
        ["draw_line(x1: 0, y1: 0, x2: 10, y2: 10, r: 255, g: 0, b: 0, a: 128)"]
    );
}

#[test]
fn variants_that_agree_on_the_names_are_named_and_keep_their_variant() {
    // Both take three arguments and call them the same thing, so there is
    // one reading. The call ran the three-parameter variant and still does:
    // the proof inside `applied` would refuse anything else.
    let src = "fn f(x, y, z)\n  1\nend\nfn f(x, y, z, w = 0)\n  2\nend\nprint(f(1, 2, 3))\n";
    let out = applied(src);
    assert!(out.contains("f(x: 1, y: 2, z: 3)"), "{out}");
}

// --- applying ---------------------------------------------------------------

#[test]
fn applying_keeps_comments_layout_and_everything_outside_the_arguments() {
    let src = "// header\nfn box(x, y, w, h)\n  x + y + w + h   // sum\nend\n\n\
               print(box(\n  1,   // left\n  (2 + 3) * 4,\n  [5, 6][0],\n  box(1, 2, 3, 4),  // nested\n))\n";
    let expected = "// header\nfn box(x, y, w, h)\n  x + y + w + h   // sum\nend\n\n\
               print(box(\n  x: 1,   // left\n  y: (2 + 3) * 4,\n  w: [5, 6][0],\n  h: box(x: 1, y: 2, w: 3, h: 4),  // nested\n))\n";
    assert_eq!(applied(src), expected);
}

#[test]
fn applying_is_idempotent() {
    let src = format!(
        "{BOX}let x = 1\nprint(box(x, 2, 3, 4))\nprint(lerp(0, 10, 0.5))\nprint(slice([1, 2, 3], 0, 2))\n"
    );
    let once = applied(&src);
    assert_ne!(once, src);
    assert_eq!(applied(&once), once);
    assert!(rewrites(&once).is_empty(), "{:?}", rewrites(&once));
}

#[test]
fn non_ascii_text_ahead_of_a_call_does_not_shift_the_edit() {
    let src = format!("{BOX}let s = \"héllo → wörld\"\nprint(box(1, 2, 3, 4)) // ✓\n");
    assert_eq!(
        applied(&src),
        format!("{BOX}let s = \"héllo → wörld\"\nprint(box(x: 1, y: 2, w: 3, h: 4)) // ✓\n")
    );
}

#[test]
fn a_rewritten_program_runs_as_it_did() {
    let src = "fn frame(x, y, w, h, pad = w / 2)\n  [x, y, w, h, pad]\nend\n\
               fn log(tag, value, extra)\n  print(tag)\n  value + extra\nend\n\
               print(frame(log(\"a\", 1, 0), log(\"b\", 2, 0), log(\"c\", 3, 0), 4))\n\
               print(clamp(log(\"d\", 9, 1), 0, 5))\n";
    let out = applied(src);
    assert_ne!(out, src);
    let run = |code: &str| {
        let o = Command::new(PETAL)
            .args(["run", "-e", code])
            .output()
            .expect("run petal");
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    assert_eq!(run(src), run(&out));
}

// --- the proof ----------------------------------------------------------------

#[test]
fn the_proof_rejects_a_rewrite_that_binds_differently() {
    let o = opts(HostProfile::Core);
    let before = format!("{BOX}print(box(1, 2, 3, 4))\n");
    // The right names in the wrong slots.
    let swapped = format!("{BOX}print(box(y: 1, x: 2, w: 3, h: 4))\n");
    assert!(verify_named_args(&before, &swapped, None, &o).is_err());
    // The same slots, written in another order: the arguments are evaluated
    // in a different order, so it is not the same program either.
    let reordered = format!("{BOX}print(box(x: 1, y: 2, h: 4, w: 3))\n");
    assert!(verify_named_args(&before, &reordered, None, &o).is_err());
    // A name that moves the call to another overload variant.
    let two = "fn f(x, y, z)\n  1\nend\nfn f(p, q, r, s = 0)\n  2\nend\n";
    let a = format!("{two}print(f(1, 2, 3))\n");
    let b = format!("{two}print(f(p: 1, q: 2, r: 3))\n");
    assert!(verify_named_args(&a, &b, None, &o).is_err());
    // And it accepts the real thing.
    let named = format!("{BOX}print(box(x: 1, y: 2, w: 3, h: 4))\n");
    verify_named_args(&before, &named, None, &o).expect("the same call");
}

#[test]
fn the_proof_refuses_a_callee_it_cannot_resolve() {
    let o = opts(HostProfile::Core);
    let a = format!("{BOX}fn apply(f)\n  f(1, 2, 3, 4)\nend\nprint(apply(box))\n");
    let b = format!("{BOX}fn apply(f)\n  f(x: 1, y: 2, w: 3, h: 4)\nend\nprint(apply(box))\n");
    // True of this program as it stands, but only because of what `apply` is
    // handed — which is not something a call site can be rewritten on.
    assert!(verify_named_args(&a, &b, None, &o).is_err());
}

#[test]
fn plain_ir_equality_still_tells_named_from_positional() {
    let a = format!("{BOX}print(box(1, 2, 3, 4))\n");
    let b = format!("{BOX}print(box(x: 1, y: 2, w: 3, h: 4))\n");
    let verdict = petal::ir_equiv::sources_equivalent(&a, &b, &[], None).expect("both compile");
    assert!(verdict.is_err(), "argument names are semantic to ir-equal");
}

// --- the command ----------------------------------------------------------------

fn petal(args: &[&str]) -> (String, String, Option<i32>) {
    let out = Command::new(PETAL)
        .args(args)
        .output()
        .expect("failed to run petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

/// A scratch file holding `content`, removed when dropped.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str, content: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("petal-suggest-{}-{name}.ptl", std::process::id()));
        std::fs::write(&path, content).expect("write scratch file");
        Scratch(path)
    }
    fn path(&self) -> &str {
        self.0.to_str().expect("utf-8 temp path")
    }
    fn read(&self) -> String {
        std::fs::read_to_string(&self.0).expect("read scratch file")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const PROGRAM: &str = "fn box(x, y, w, h)\n  x + y + w + h\nend\nprint(box(1, 2, 3, 4))\n";

#[test]
fn cli_reports_both_kinds_and_only_filters_them() {
    let file = Scratch::new("report", PROGRAM);
    let (stdout, _, code) = petal(&["suggest", file.path()]);
    assert_eq!(code, Some(0));
    assert!(stdout.contains("suggest: x: num"), "{stdout}");
    assert!(stdout.contains("call box"), "{stdout}");
    assert!(
        stdout.contains("suggest: box(x: 1, y: 2, w: 3, h: 4)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("because: `box` is `box(x, y, w, h)`"),
        "{stdout}"
    );

    let (stdout, _, _) = petal(&["suggest", "--only", "named-args", file.path()]);
    assert!(!stdout.contains("x: num"), "{stdout}");
    assert!(
        stdout.contains("1 call that could name its arguments."),
        "{stdout}"
    );

    let (stdout, _, _) = petal(&["suggest", "--only", "types", file.path()]);
    assert!(!stdout.contains("call box"), "{stdout}");
    assert!(
        stdout.contains("type annotations across 1 function."),
        "{stdout}"
    );

    let (_, stderr, code) = petal(&["suggest", "--only", "nope", file.path()]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("Unknown suggestion kind 'nope'"),
        "{stderr}"
    );
    // Reporting never writes.
    assert_eq!(file.read(), PROGRAM);
}

#[test]
fn cli_json_lists_each_suggestion_with_its_kind_and_edits() {
    let file = Scratch::new("json", PROGRAM);
    let (stdout, _, code) = petal(&["suggest", "--json", file.path()]);
    assert_eq!(code, Some(0));
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let items = json["suggestions"].as_array().expect("suggestions");
    let named: Vec<&serde_json::Value> =
        items.iter().filter(|s| s["kind"] == "named-args").collect();
    assert_eq!(named.len(), 1, "{stdout}");
    assert_eq!(named[0]["callee"], "box");
    assert_eq!(named[0]["call"], "box(1, 2, 3, 4)");
    assert_eq!(named[0]["rewrite"], "box(x: 1, y: 2, w: 3, h: 4)");
    assert_eq!(named[0]["names"], serde_json::json!(["x", "y", "w", "h"]));
    assert_eq!(named[0]["edits"].as_array().unwrap().len(), 4);
    assert_eq!(named[0]["edits"][0]["insert_text"], "x: ");
    // The annotations keep the fields they always had.
    let typed = items
        .iter()
        .find(|s| s["kind"] == "type-annotation")
        .expect("an annotation");
    assert!(
        typed["insert_at"].is_u64() && typed["function"] == "box",
        "{typed}"
    );
}

#[test]
fn cli_apply_writes_proven_rewrites_and_a_second_run_finds_nothing() {
    let file = Scratch::new("apply", PROGRAM);
    let (stdout, stderr, code) =
        petal(&["suggest", "--only", "named-args", "--apply", file.path()]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("1 named-argument rewrite proven IR-equal"),
        "{stderr}"
    );
    assert!(stdout.contains("Applied 1 of 1"), "{stdout}");
    assert_eq!(
        file.read(),
        "fn box(x, y, w, h)\n  x + y + w + h\nend\nprint(box(x: 1, y: 2, w: 3, h: 4))\n"
    );
    let (stdout, _, _) = petal(&["suggest", "--only", "named-args", file.path()]);
    assert!(stdout.contains("no suggestions"), "{stdout}");
}

#[test]
fn cli_apply_writes_both_kinds_together() {
    let file = Scratch::new("both", PROGRAM);
    let (_, stderr, code) = petal(&["suggest", "--apply", file.path()]);
    assert_eq!(code, Some(0), "{stderr}");
    let text = file.read();
    assert!(
        text.starts_with("fn box(x: num, y: num, w: num, h: num)"),
        "{text}"
    );
    assert!(
        text.contains("print(box(x: 1, y: 2, w: 3, h: 4))"),
        "{text}"
    );
}

#[test]
fn cli_verify_proves_without_writing() {
    let file = Scratch::new("verify", PROGRAM);
    let (_, stderr, code) = petal(&["suggest", "--only", "named-args", "--verify", file.path()]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("proven IR-equal"), "{stderr}");
    assert_eq!(file.read(), PROGRAM);
}

#[test]
fn cli_ir_equal_accepts_named_arguments_only_when_asked() {
    let before = Scratch::new("ir-a", PROGRAM);
    let after = Scratch::new(
        "ir-b",
        "fn box(x, y, w, h)\n  x + y + w + h\nend\nprint(box(x: 1, y: 2, w: 3, h: 4))\n",
    );
    let (stdout, _, code) = petal(&["ir-equal", before.path(), after.path()]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("argument names differs"), "{stdout}");
    let (stdout, _, code) = petal(&["ir-equal", "--named-args", before.path(), after.path()]);
    assert_eq!(code, Some(0), "{stdout}");
    // Still a comparison: a different binding is a difference.
    let swapped = Scratch::new(
        "ir-c",
        "fn box(x, y, w, h)\n  x + y + w + h\nend\nprint(box(y: 1, x: 2, w: 3, h: 4))\n",
    );
    let (_, _, code) = petal(&["ir-equal", "--named-args", before.path(), swapped.path()]);
    assert_eq!(code, Some(1));
}

#[test]
fn a_parenthesized_callee_is_shown_whole() {
    let src = "fn box(left, top, wide, high)\n  left + top + wide + high\nend\n\
               var h = box\nprint((get h)(1, 2, 3, 4))\n";
    assert_eq!(
        rewrites(src),
        ["(get h)(left: 1, top: 2, wide: 3, high: 4)"]
    );
    assert!(applied(src).contains("print((get h)(left: 1, top: 2, wide: 3, high: 4))"));
}
