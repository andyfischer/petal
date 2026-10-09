//! String offsets count code points: `len`, `slice`, `s[i]`, `s.length`,
//! `index_of` and the `char_*` synonyms all agree, on ASCII and non-ASCII
//! text alike. `byte_len` / `byte_slice` are the explicit byte-unit pair.
//!
//! The ASCII path (an index is the byte offset) and the non-ASCII path (a walk
//! from the nearest known position) are checked against one reference that
//! goes through `str::chars`, so the two cannot drift apart.

use std::time::{Duration, Instant};

use petal::env::Env;
use petal::policy::RunPolicy;

fn run_with(src: &str, policy: RunPolicy) -> Vec<String> {
    let mut env = Env::new();
    env.set_policy(policy);
    env.run_source(src)
        .unwrap_or_else(|e| panic!("run failed: {e}\n{src}"));
    env.take_output()
}

fn run(src: &str) -> Vec<String> {
    run_with(src, RunPolicy::FAST)
}

fn run_err(src: &str) -> String {
    Env::new()
        .run_source(src)
        .expect_err("expected a runtime error")
}

#[test]
fn len_counts_code_points() {
    assert_eq!(run(r#"print(len("Óscar"), "Óscar".length, char_len("Óscar"))"#), ["5 5 5"]);
    assert_eq!(run(r#"print(len(""), len("abc"), len("日本語"), len("a😀b"))"#), ["0 3 3 3"]);
    assert_eq!(run(r#"print(byte_len("Óscar"), byte_len("abc"), byte_len("😀"))"#), ["6 3 4"]);
}

#[test]
fn slice_cuts_between_code_points() {
    assert_eq!(run(r#"print(slice("Óscar", 0, 1))"#), ["Ó"]);
    assert_eq!(run(r#"print(slice("Óscar", 1))"#), ["scar"]);
    assert_eq!(run(r#"print(slice("Óscar", -3, -1))"#), ["ca"]);
    assert_eq!(run(r#"print(slice("a😀b✓c", 1, 4))"#), ["😀b✓"]);
    assert_eq!(run(r#"print(len(slice("Óscar", 4, 2)), len(slice("Óscar", 9)))"#), ["0 0"]);
    // `slice` and `char_slice` are the same operation on a string.
    assert_eq!(
        run(r#"print(slice("naïve café", 2, 8) == char_slice("naïve café", 2, 8))"#),
        ["true"]
    );
}

#[test]
fn byte_slice_keeps_whole_characters_inside_the_range() {
    // "Ó" is two bytes: a cut inside it moves inward, never splitting it.
    assert_eq!(run(r#"print(len(byte_slice("Óscar", 0, 1)))"#), ["0"]);
    assert_eq!(run(r#"print(byte_slice("Óscar", 0, 2), byte_slice("Óscar", 1, 4))"#), ["Ó sc"]);
    assert_eq!(run(r#"print(byte_slice("hello", 1, 3), byte_slice("hello", -2))"#), ["el lo"]);
    assert!(run_err("print(byte_slice([1, 2], 0))").contains("byte_slice() expects a string"));
    assert!(run_err("print(byte_len([1, 2]))").contains("byte_len() expects a string"));
}

#[test]
fn indexing_a_string_gives_one_character() {
    assert_eq!(run(r#"let s = "Óscar ✓"
print(s[0], s[1], s[-1], s[6], "abc"[1])"#), ["Ó s ✓ ✓ b"]);
    // Strict like a list index; `char_at` is the lenient form.
    assert!(run_err(r#"print("Óscar"[5])"#).contains("Index 5 out of bounds (len 5)"));
    assert!(run_err(r#"print("Óscar"[-6])"#).contains("out of bounds"));
    assert!(run_err(r#"print(""[0])"#).contains("out of bounds"));
    assert_eq!(run(r#"print(len(char_at("Óscar", 5)), char_at("Óscar", -5))"#), ["0 Ó"]);
}

#[test]
fn offsets_compose_across_builtins() {
    // The offset `index_of` returns is the one `slice` and `s[i]` take.
    let out = run(r#"let s = "clé=valeur"
let i = index_of(s, "=")
print(i, slice(s, 0, i), slice(s, i + 1), s[i])"#);
    assert_eq!(out, ["3 clé valeur ="]);
    // A scan by index visits every character exactly once.
    let out = run(r#"let s = "a😀é✓z"
let out = []
for i in range(len(s)) do
  out = push(out, s[i])
end
print(join(out, "|"), out == chars(s))"#);
    assert_eq!(out, ["a|😀|é|✓|z true"]);
}

/// What `slice(s, start, end)` must give, by way of `str::chars`.
fn ref_slice(s: &str, start: i64, end: i64) -> String {
    let n = s.chars().count() as i64;
    let clamp = |i: i64| if i < 0 { (n + i).max(0) } else { i.min(n) };
    let (a, b) = (clamp(start), clamp(end));
    if a >= b {
        return String::new();
    }
    s.chars().skip(a as usize).take((b - a) as usize).collect()
}

/// Every `slice`, `s[i]`, `char_at` and `index_of` of each sample, compared
/// with the reference. The samples put multi-byte characters at the start, the
/// end, adjacent to each other and alone, next to all-ASCII strings of the
/// same shape, so the ASCII path and the walking path face the same
/// questions. Run under both policies: `s[i]` has a fast-path and a slow-path
/// dispatch.
#[test]
fn every_offset_agrees_with_a_char_walk() {
    let samples = [
        "", "a", "abcdef", "hello world", "é", "éa", "aé", "Óscar", "日本語", "a😀b", "😀😀",
        "x✓y✓z", "né ✓ 😀!", "ascii then é", "é then ascii",
    ];
    for s in samples {
        let n = s.chars().count() as i64;
        let src = format!(
            r#"let s = "{s}"
let n = len(s)
print(n)
for i in range(-n - 2, n + 3) do
  for j in range(-n - 2, n + 3) do
    print("[" ++ slice(s, i, j) ++ "]")
  end
  print("<" ++ slice(s, i) ++ ">")
  print("(" ++ char_at(s, i) ++ ")")
end
for i in range(n - 1, -1, -1) do
  print(s[i] ++ s[i - n] ++ str(index_of(s, s[i])))
end"#
        );
        let mut want = vec![n.to_string()];
        let cs: Vec<char> = s.chars().collect();
        for i in -n - 2..n + 3 {
            for j in -n - 2..n + 3 {
                want.push(format!("[{}]", ref_slice(s, i, j)));
            }
            want.push(format!("<{}>", ref_slice(s, i, n)));
            let k = if i < 0 { n + i } else { i };
            let ch = if (0..n).contains(&k) { cs[k as usize].to_string() } else { String::new() };
            want.push(format!("({ch})"));
        }
        for i in (0..n as usize).rev() {
            let first = cs.iter().position(|c| *c == cs[i]).unwrap();
            want.push(format!("{}{}{}", cs[i], cs[i], first));
        }
        for policy in [RunPolicy::BASELINE, RunPolicy::FAST] {
            assert_eq!(run_with(&src, policy), want, "sample {s:?}");
        }
    }
}

/// Time `ops` rounds of `len`, `s[i]`, `slice` and `char_at` on an ASCII
/// string of `size` bytes. Building the string is inside the timed region, so
/// `ops` is large enough to dwarf it.
fn time_ascii_ops(size: usize, ops: usize) -> Duration {
    let src = format!(
        r#"let s = repeat("abcdefgh", {})
let n = len(s)
fn work(s, n)
  let acc = 0
  for i in range({ops}) do
    let k = (i * 7919) % (n - 4)
    acc = acc + len(s) + len(s[k]) + len(slice(s, k, k + 3)) + len(char_at(s, k))
  end
  acc
end
print(work(s, n))"#,
        size / 8
    );
    // Best of two fresh runs, so one scheduling hiccup does not decide it.
    let mut best = Duration::MAX;
    for _ in 0..2 {
        let mut env = Env::new();
        let t = Instant::now();
        env.run_source(&src).expect("run");
        best = best.min(t.elapsed());
        assert_eq!(env.take_output(), [(ops * (size + 5)).to_string()]);
    }
    best
}

/// `len`, `s[i]`, `slice` and `char_at` on ASCII text do not depend on the
/// string's length. A 4 MB string is 4,000 times the small one: any per-call
/// pass over the text, even an `is_ascii` scan, would cost thousands of times
/// more, where the bound allows 10.
#[test]
fn ascii_len_index_and_slice_are_constant_time() {
    const OPS: usize = 30_000;
    let small = time_ascii_ops(1 << 10, OPS);
    let big = time_ascii_ops(1 << 22, OPS);
    assert!(
        big < small * 10 + Duration::from_millis(50),
        "ASCII string ops scale with length: {small:?} at 1 KB, {big:?} at 4 MB"
    );
}

/// A left-to-right scan of non-ASCII text by index is linear overall: each
/// lookup resumes from the previous one instead of walking from the start.
#[test]
fn a_sequential_scan_of_non_ascii_text_is_linear() {
    let scan = |chars: usize| {
        let src = format!(
            r#"let s = repeat("añb✓", {})
fn work(s)
  let hits = 0
  for i in range(len(s)) do
    if s[i] == "✓" then hits = hits + 1 end
  end
  hits
end
print(work(s))"#,
            chars / 4
        );
        let mut env = Env::new();
        let t = Instant::now();
        env.run_source(&src).expect("run");
        assert_eq!(env.take_output(), [(chars / 4).to_string()]);
        t.elapsed()
    };
    let small = scan(2_000);
    let big = scan(64_000);
    // 32 times the text: linear is ~32x, a walk from the start per lookup ~1000x.
    assert!(
        big < small * 200 + Duration::from_millis(50),
        "scan is not linear: {small:?} for 2k chars, {big:?} for 64k"
    );
}
