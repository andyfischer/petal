use super::*;

fn fmt(src: &str) -> String {
    format_source(src).unwrap()
}

#[test]
fn formats_spacing_indent_comments_and_blank_lines() {
    let src = "\n\nfn f(a,b)\nlet x=a+b      // sum\nlet yy=a*b     // product\n\n\n\nreturn x+yy\nend\n\n\n";
    // The two comments were lined up, and stay lined up however the code
    // before them was respaced.
    assert_eq!(
        fmt(src),
        "fn f(a, b)\n  let x = a + b  // sum\n  let yy = a * b // product\n\n  return x + yy\nend\n"
    );
}

#[test]
fn is_idempotent_on_its_own_output() {
    let src = "state a=1 // x\nstate bbb   =  2 // y\nif(a>1)then\nprint( \"{a+1}\" )\nend\n";
    let once = fmt(src);
    assert_eq!(fmt(&once), once);
}

#[test]
fn ignore_next_line_keeps_it_verbatim() {
    let src = "// petal-fmt-ignore\nquad(x0,y0,z1,   x1,y0,z1)\nquad(x0,y0)\n";
    assert_eq!(
        fmt(src),
        "// petal-fmt-ignore\nquad(x0,y0,z1,   x1,y0,z1)\nquad(x0, y0)\n"
    );
}

#[test]
fn off_on_region_keeps_every_line() {
    let src =
        "let a=1\n// petal-fmt-off\nlet m = [\n  1,0,\n      0,1,\n]\n// petal-fmt-on\nlet b=2\n";
    assert_eq!(
        fmt(src),
        "let a = 1\n// petal-fmt-off\nlet m = [\n  1,0,\n      0,1,\n]\n// petal-fmt-on\nlet b = 2\n"
    );
}

#[test]
fn ignore_file_leaves_everything() {
    let src = "// petal-fmt-ignore-file\nlet a=1\n\n\n\nlet b  =  2";
    assert_eq!(fmt(src), src);
}

#[test]
fn refuses_a_file_that_does_not_parse() {
    assert!(format_source("fn (").is_err());
}

#[test]
fn directive_word_reads_the_first_word() {
    assert_eq!(
        directive_word("// petal-fmt-ignore"),
        Some("petal-fmt-ignore")
    );
    assert_eq!(
        directive_word("//petal-lint-ignore prefer-let"),
        Some("petal-lint-ignore")
    );
    assert_eq!(directive_word("// a note"), None);
}

/// The safety property, over every `.ptl` in the repo: formatting leaves the
/// compiled IR identical (it is whitespace, and Petal is not
/// whitespace-significant), and formatting is a fixed point.
#[test]
fn fmt_is_ir_equal_and_idempotent_over_repo_corpus() {
    use crate::ir_equiv::sources_equivalent;

    let files = crate::test_corpus::repo_ptl_files();
    let mut checked = 0;
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let formatted = match format_source(&src) {
            Ok(t) => t,
            // A file that does not parse (a negative test) is refused, which
            // is the correct outcome. Any other refusal is a formatter bug.
            Err(e) if e.starts_with("fmt bug") => panic!("{}: {}", path.display(), e),
            Err(_) => continue,
        };
        assert_eq!(
            format_source(&formatted).unwrap(),
            formatted,
            "fmt not idempotent for {}",
            path.display()
        );
        if crate::ir_equiv::compile_for_compare(&src, &[], Some(path)).is_err() {
            continue; // doesn't compile standalone; nothing to compare
        }
        match sources_equivalent(&src, &formatted, &[], Some(path)) {
            Ok(Ok(())) => {}
            Ok(Err(diff)) => panic!("fmt changed IR for {}:\n{}", path.display(), diff),
            Err(e) => panic!("fmt broke compilation for {}: {}", path.display(), e),
        }
        checked += 1;
    }
    assert!(checked > 150, "expected a real corpus, checked {checked}");
}

/// Re-indentation must undo any indentation, not just leave clean files alone:
/// every repo file is *mangled* first — three extra spaces on the front of
/// every non-empty line — and formatting the mangled text must give the same
/// result as formatting the original. Files with a multi-line token (raw
/// string, JSX text) are skipped: there the extra spaces would be content.
#[test]
fn fmt_undoes_mangled_indentation_over_repo_corpus() {
    let files = crate::test_corpus::repo_ptl_files();
    let mut checked = 0;
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(formatted) = format_source(&src) else {
            continue;
        };
        let Some(mangled) = mangle_indentation(&src) else {
            continue;
        };
        if mangled.contains(OFF) || mangled.contains(IGNORE_NEXT) {
            continue; // fenced lines keep the mangle, by design
        }
        let remangled = format_source(&mangled)
            .unwrap_or_else(|e| panic!("fmt failed on mangled {}: {}", path.display(), e));
        assert_eq!(
            remangled,
            formatted,
            "fmt did not undo the mangle for {}",
            path.display()
        );
        checked += 1;
    }
    assert!(
        checked > 150,
        "expected most of the corpus to be manglable, got {checked}"
    );
}

/// Add three spaces to the front of every non-empty line. Returns `None` when
/// the file has a token spanning more than one line.
fn mangle_indentation(src: &str) -> Option<String> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize().ok()?;
    let multiline = lexer.tokens_with_spans().any(|(token, span)| {
        !matches!(token, Token::Newline | Token::Eof) && span.end.line > span.start.line
    });
    if multiline {
        return None;
    }
    let mut out = String::with_capacity(src.len() + src.lines().count() * 3);
    for line in src.lines() {
        if !line.trim().is_empty() {
            out.push_str("   ");
        }
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}
