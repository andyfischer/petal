//! The refusals of the standard-library helpers added for the Moses debrief
//! (`fill`, `argmin`, `top_indices`, `group_by_keys`, `interp`, `ord`, `chr`,
//! the hashes, record `lerp`), and what a program's own function of one of
//! those names does. What they compute is tested in Petal:
//! test/ptl/stdlib_helpers.ptl.

use petal::env::Env;

fn run(src: &str) -> Result<String, String> {
    let mut env = Env::new();
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

fn out(src: &str) -> String {
    run(src).unwrap_or_else(|e| panic!("run failed for {src:?}: {e}"))
}

fn err(src: &str) -> String {
    match run(src) {
        Ok(o) => panic!("expected an error for {src:?}, got output {o:?}"),
        Err(e) => e,
    }
}

#[track_caller]
fn refuses(src: &str, what: &str) {
    let e = err(src);
    assert!(e.contains(what), "{src:?} should fail with {what:?}, got: {e}");
}

#[test]
fn fill_wants_an_int_count_of_a_sane_size() {
    refuses("fill(2.5, 0)", "fill(count, value) expects an int count, got float");
    refuses("fill(\"3\", 0)", "expects an int count, got string");
    refuses("fill(1000000000, 0)", "would be too large");
    refuses("fill(3)", "fill() expects 2 arguments, got 1");
}

#[test]
fn the_selections_want_numbers() {
    refuses("argmin([1, \"b\", 3])", "argmin() expects a list of numbers, got string at index 1");
    refuses("argmax(5)", "argmax() expects a list of numbers, got int");
    refuses("top_indices([1, nil], 1)", "top_indices() expects a list of numbers, got nil at index 1");
    refuses("top_indices([1, 2], 1.5)", "top_indices(list, k) expects an int k, got float");
    // The keyed forms say so through the native they are built on.
    refuses("min_by([1, 2], fn(x) -> \"k\")", "argmin() expects a list of numbers");
    refuses("top_k([\"a\", \"b\"], 1)", "top_indices() expects a list of numbers");
}

#[test]
fn group_by_keys_wants_a_key_per_element() {
    refuses(
        "group_by_keys([1, 2, 3], [\"a\"])",
        "needs one key per element, got 3 elements and 1 keys",
    );
    refuses("group_by_keys(1, 2)", "group_by_keys(list, keys) expects two lists");
}

#[test]
fn interp_wants_a_table_of_pairs() {
    refuses("interp([], 1)", "needs a table with at least one entry");
    refuses("interp(5, 1)", "expects a list of [x, y] entries");
    refuses("interp([[0, 1], [2]], 1)", "expects each entry of the table to be [x, y], got entry 1");
    refuses("interp([[0, 1], \"x\"], 5)", "got entry 1");
    refuses("interp([[0, 1]], \"x\")", "");
}

#[test]
fn ord_and_chr_name_what_they_cannot_convert() {
    refuses("ord(\"\")", "ord() of an empty string");
    refuses("ord(65)", "ord() expects a string, got int");
    refuses("chr(-1)", "chr() of -1, which is not a Unicode code point");
    refuses("chr(55296)", "chr() of 55296, which is not a Unicode code point");
    refuses("chr(1114112)", "not a Unicode code point");
    refuses("chr(\"a\")", "chr() expects an int, got string");
}

#[test]
fn the_hashes_want_numbers_and_a_string() {
    refuses("hash_int()", "hash_int() expects 1 to 4 arguments");
    refuses("hash_int(1, 2, 3, 4, 5)", "hash_int() expects 1 to 4 arguments");
    refuses("hash_int(\"1\")", "hash_int() expects numbers, got string");
    refuses("hash01(nil)", "hash01() expects numbers, got nil");
    refuses("hash_str(12)", "hash_str() expects a string, got int");
    refuses("hash_str(\"a\", \"b\")", "hash_str() expects numbers, got string");
    // A nil salt is no salt.
    assert_eq!(out("print(hash_str(\"a\", nil) == hash_str(\"a\"))"), "true");
}

#[test]
fn lerp_of_a_record_and_something_else_is_refused() {
    refuses("import creative\nlerp({a: 1}, 5, 0.5)", "two lists of numbers or two records");
    refuses("import creative\nlerp([1], {a: 1}, 0.5)", "two lists of numbers or two records");
}

/// A record of a class keeps its class through `lerp`.
#[test]
fn lerp_keeps_the_first_records_class() {
    let src = "import creative
class Look
  sepia: num,
  id: string
end
fn Look.label(l: Look)
  \"{l.id} {l.sepia}\"
end
let m = lerp(Look(0.0, \"a\"), Look(1.0, \"b\"), 0.25)
print(m.label())";
    assert_eq!(out(src), "a 0.25");
}

/// The Petal ones live in the std prelude, whose bindings are weak: a
/// program's own function of the same name is the one its calls reach.
#[test]
fn a_programs_own_helper_shadows_the_preludes() {
    let src = "fn plural(n, one, many)
  if n == 1 then one else many end
end
fn group_by(xs, key)
  \"mine\"
end
print(plural(2, \"ox\", \"oxen\"))
print(group_by([1], fn(x) -> x))";
    assert_eq!(out(src), "oxen\nmine");
    // And a native's name is free for a program to use too.
    let src = "fn fill(n, v)
  \"my fill\"
end
fn hash_int(a)
  7
end
print(fill(2, 0), hash_int(1))";
    assert_eq!(out(src), "my fill 7");
}

/// The hashes are constant functions: a table built from them is a `const`.
#[test]
fn the_new_natives_are_constant() {
    let src = "const JITTER = for i in range(0, 4) do hash_int(7, i) % 10 end
const ZEROS = fill(3, 0.0)
const AT = interp([[0, 0], [10, 5]], 4)
const A = ord(\"A\")
print(JITTER, ZEROS, AT, A)";
    assert_eq!(out(src), "[8, 5, 7, 1] [0.0, 0.0, 0.0] 2.0 65");
}

/// `{x:.1f}` is not a hole; the error says what to write instead.
#[test]
fn a_format_spec_in_a_hole_points_at_the_formatting_builtins() {
    for src in ["let x = 1.5\nprint(\"{x:.1f}\")", "let n = 1500\nprint(\"{n:,}\")"] {
        let e = err(src);
        assert!(e.contains("takes no format spec"), "{e}");
        assert!(e.contains("{fixed(x, 1)}") && e.contains("{commas(n)}"), "{e}");
    }
    // A record literal in a hole is still a record literal.
    assert_eq!(out("print(\"{ {a: 1}.a }\")"), "1");
    // JSON in a string keeps the message about the brace.
    let e = err("print(\"{\\\"a\\\": 1}\")");
    assert!(e.contains("write `\\{` for a literal brace") && !e.contains("format spec"), "{e}");
}
