//! The corpus over which `text_wrap_rows` and its Petal specification
//! (`core-runtime/text-layout/src/wrap_reference.ptl`) must answer the same
//! rows. Shared by `tests/text_wrap_reference.rs` (the check) and
//! `examples/bench_text_wrap.rs` (which also times the two). Included with
//! `#[path]`; not a test target of its own.

#![allow(dead_code)]

/// The width `text_area` wraps to in the timed scripts: a 560 px box less its
/// 6 px inset on each side.
pub const AREA_W: i32 = 560;
pub const WRAP_W: i32 = AREA_W - 12;

/// A Petal string literal for `s`.
pub fn lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Advance ratios for ASCII, roughly a UI sans: narrow punctuation and `il`,
/// wide `mw`, capitals wider than lowercase. Anything else takes the
/// fallback ratio.
pub fn ratios() -> Vec<f64> {
    (0u32..128)
        .map(|cp| {
            let c = char::from_u32(cp).unwrap();
            match c {
                ' ' => 0.27,
                '\t' => 1.08,
                'i' | 'l' | 'j' | '.' | ',' | ';' | ':' | '!' | '|' | '\'' => 0.24,
                'f' | 't' | 'r' | '(' | ')' | '-' => 0.36,
                'm' | 'w' => 0.83,
                'M' | 'W' => 0.92,
                'a'..='z' => 0.55,
                'A'..='Z' => 0.67,
                '0'..='9' => 0.56,
                _ => 0.5,
            }
        })
        .collect()
}

/// A deterministic document of about `n` characters: paragraphs of words of
/// mixed length, the odd blank line, double space, tab and over-long token
/// (a URL a wrap has to break).
pub fn document(n: usize, seed: u64) -> String {
    const WORDS: &[&str] = &[
        "the",
        "of",
        "and",
        "a",
        "to",
        "in",
        "is",
        "you",
        "that",
        "it",
        "he",
        "was",
        "for",
        "on",
        "are",
        "as",
        "with",
        "his",
        "they",
        "I",
        "at",
        "be",
        "this",
        "have",
        "from",
        "or",
        "one",
        "had",
        "by",
        "word",
        "but",
        "not",
        "what",
        "all",
        "were",
        "we",
        "when",
        "your",
        "can",
        "said",
        "there",
        "use",
        "an",
        "each",
        "which",
        "she",
        "do",
        "how",
        "their",
        "if",
        "will",
        "up",
        "other",
        "about",
        "out",
        "many",
        "then",
        "them",
        "these",
        "so",
        "some",
        "her",
        "would",
        "make",
        "like",
        "him",
        "into",
        "time",
        "has",
        "look",
        "two",
        "more",
        "write",
        "go",
        "see",
        "number",
        "no",
        "way",
        "could",
        "people",
        "my",
        "than",
        "first",
        "water",
        "been",
        "call",
        "who",
        "oil",
        "its",
        "now",
        "find",
        "long",
        "down",
        "day",
        "did",
        "get",
        "come",
        "made",
        "may",
        "part",
        "Borough",
        "budget",
        "Wednesday,",
        "committee.",
        "reconciliation",
        "infrastructure",
        "(approximately)",
        "well-known",
        "Mr.",
        "e.g.",
    ];
    let mut s = String::with_capacity(n + 64);
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut next = || {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) as usize
    };
    let mut in_para = 0usize;
    while s.chars().count() < n {
        let r = next();
        if in_para > 40 && r % 23 == 0 {
            s.push('\n');
            if r % 5 == 0 {
                s.push('\n');
            }
            in_para = 0;
            continue;
        }
        if in_para > 0 {
            s.push_str(match r % 61 {
                0 => "  ",
                1 => "\t",
                _ => " ",
            });
        }
        if r % 97 == 3 {
            s.push_str("https://example.org/a/really/long/path/that/never/fits/in/one/row/of/the/box/index.html");
        } else {
            s.push_str(WORDS[r % WORDS.len()]);
        }
        in_para += 1;
    }
    s.chars().take(n).collect()
}

/// The documents: edge cases (blanks, CRLF, tabs, non-ASCII, over-wide
/// words, characters a Petal literal escapes) and generated prose.
pub fn corpus() -> Vec<String> {
    let mut corpus: Vec<String> = [
        "",
        " ",
        "   ",
        "a",
        "aaa bbb ccc",
        "  leading blanks then words",
        "trailing blanks   ",
        "double  space and\ttab\t\tinside",
        "a\nb",
        "a\n\nb\n",
        "\n\n",
        "crlf line\r\nnext line\r\n",
        "supercalifragilisticexpialidocious",
        "   supercalifragilisticexpialidocious after leading blanks",
        "x supercalifragilisticexpialidocious y",
        "h\u{e9}llo w\u{f6}rld\n\n  \u{65e5}\u{672c}\u{8a9e}\u{306e}\u{6587}\u{7ae0}",
        "quotes \"and\" {braces} and \\ backslash",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (i, n) in [200usize, 1000, 5000].iter().enumerate() {
        corpus.push(document(*n, 7 + i as u64));
    }
    corpus
}

/// A script that wraps every document of `corpus()` at 11 widths (−5 to 2000
/// px) and 4 styles with both the native and the Petal specification, and
/// leaves `checked` (cases), `bad` (mismatches) and `first` (the first
/// mismatch, described) in its state.
pub fn equivalence_source() -> String {
    let corpus = corpus();
    let docs: Vec<String> = corpus.iter().map(|d| lit(d)).collect();
    let src = format!(
        "import ui\nimport text_layout/wrap_reference: wrap_rows_reference\n\
         let docs = [{docs}]\n\
         let widths = [-5, 0, 1, 5, 37, 80.5, 120, 200, 333.3, {WRAP_W}, 2000]\n\
         let styles = [{{size: 14}}, {{size: 11, spacing: 0.5}}, {{size: 20, weight: 700}}, 13]\n\
         state checked = 0\n\
         state bad = 0\n\
         state first = \"\"\n\
         var n = 0\n\
         var nbad = 0\n\
         var msg = \"\"\n\
         for d in docs do\n  \
           for ts in styles do\n    \
             for w in widths do\n      \
               let a = text_wrap_rows(d, ts, float(w))\n      \
               let b = wrap_rows_reference(d, ts, w)\n      \
               set n = n + 1\n      \
               if a != b then\n        \
                 set nbad = nbad + 1\n        \
                 if msg == \"\" then\n          \
                   var k = 0\n          \
                   while k < len(a) && k < len(b) && a[k] == b[k] do\n            \
                     set k = k + 1\n          \
                   end\n          \
                   let ra = if k < len(a) then str(a[k]) else \"(none)\" end\n          \
                   let rb = if k < len(b) then str(b[k]) else \"(none)\" end\n          \
                   set msg = \"style \" ++ str(ts) ++ \" width \" ++ str(w) ++ \" row \" ++ str(k) ++ \" of \" ++ str(len(a)) ++ \"/\" ++ str(len(b)) ++ \" native \" ++ ra ++ \" petal \" ++ rb ++ \" doc \" ++ char_slice(d, 0, 60)\n        \
                 end\n      \
               end\n    \
             end\n  \
           end\n\
         end\n\
         checked = n\n\
         bad = nbad\n\
         first = msg\n",
        docs = docs.join(", "),
    );
    src
}
