//! The little TOML `core/build.rs` reads: the table headers and `key = value`
//! lines of a `Cargo.toml` or `petal.toml`, and the string fields of an inline
//! table. Not a TOML parser — just enough for the [features] table, the
//! [package.metadata.petal.bundled] table and a package's [dependencies],
//! which keeps a real parser out of the build dependencies (none is in the
//! lock file). A value that spans lines is skipped up to its closing bracket.

/// `key = raw value` lines under the table header `table` (exactly, e.g.
/// `features` or `package.metadata.petal.bundled`), in file order. Quoted keys
/// lose their quotes; values keep their raw text, trimmed.
pub fn table_entries(text: &str, table: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    let mut depth = 0i32;
    for line in text.lines() {
        let line = strip_comment(line).trim();
        if depth > 0 {
            depth += bracket_balance(line);
            continue;
        }
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            let header = line.trim_start_matches('[').trim_end_matches(']').trim();
            inside = header == table;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        depth = bracket_balance(value);
        if inside {
            let key = key.trim().trim_matches('"').to_string();
            out.push((key, value.trim().to_string()));
        }
    }
    out
}

/// The string fields of an inline table: `{ dir = "x", kind = "y" }` →
/// `[("dir", "x"), ("kind", "y")]`. `None` if `value` is not one, or a field
/// is not a plain quoted string.
pub fn inline_strings(value: &str) -> Option<Vec<(String, String)>> {
    let body = value.trim().strip_prefix('{')?.strip_suffix('}')?;
    let mut out = Vec::new();
    for field in body.split(',') {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let (key, value) = field.split_once('=')?;
        let value = value.trim().strip_prefix('"')?.strip_suffix('"')?;
        out.push((key.trim().to_string(), value.to_string()));
    }
    Some(out)
}

/// `line` up to a `#` outside a string.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Opening minus closing brackets and braces outside strings.
fn bracket_balance(text: &str) -> i32 {
    let mut in_string = false;
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '"' => in_string = !in_string,
            '[' | '{' if !in_string => depth += 1,
            ']' | '}' if !in_string => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// Every table header in `text`, in file order, without its brackets
/// (`[dependencies.text_layout]` → `dependencies.text_layout`).
pub fn headers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    for line in text.lines() {
        let line = strip_comment(line).trim();
        if depth > 0 {
            depth += bracket_balance(line);
        } else if line.starts_with('[') {
            out.push(
                line.trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim()
                    .to_string(),
            );
        } else if let Some((_, value)) = line.split_once('=') {
            depth = bracket_balance(value);
        }
    }
    out
}
