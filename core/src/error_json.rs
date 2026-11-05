//! The structured JSON form of an error message: the `{"error": true, "phase",
//! "message", "line", "column", "caused_by", "stack"}` object that the CLI's
//! `--json` output and propose-edit's report both use. Kept out of `cli` so
//! the library half (propose-edit, used by embedders) can build without the
//! `tooling` feature.

/// Parse an error string into a structured JSON object.
/// Extracts `[line N, column M]`, `Caused by:` (provenance), and
/// `Stack trace:` suffixes produced by the evaluator, lexer, and parser.
pub fn error_json_value(err: &str, phase: &str) -> serde_json::Value {
    // Split off stack trace first (always last)
    let (head, stack) = match err.split_once("\nStack trace:") {
        Some((h, rest)) => (h.to_string(), split_indented_lines(rest)),
        None => (err.to_string(), Vec::new()),
    };

    // Split off provenance ("Caused by:") next
    let (head, caused_by) = match head.split_once("\nCaused by:") {
        Some((h, rest)) => (h.to_string(), split_indented_lines(rest)),
        None => (head, Vec::new()),
    };

    // Extract [line N, column M] from the primary message line
    let (message, line, column) = parse_line_column(&head);

    serde_json::json!({
        "error": true,
        "phase": phase,
        "message": message,
        "line": line,
        "column": column,
        "caused_by": caused_by,
        "stack": stack,
    })
}

fn split_indented_lines(s: &str) -> Vec<String> {
    s.lines()
        .map(|l| l.trim_start().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Extract a `[line N, column M]` (entry file) or `[file.ptl line N, column M]`
/// (imported module) suffix from an error message. Returns
/// (message, line, column) — the file name, when present, is left in the
/// message (a structured `file` field can follow in a later diagnostics pass).
pub fn parse_line_column(s: &str) -> (String, Option<u32>, Option<u32>) {
    if let Some(open) = s.rfind(" [line ") {
        let rest = &s[open + 7..];
        if let Some((line, col)) = parse_position_body(rest) {
            return (s[..open].to_string(), Some(line), Some(col));
        }
    }
    // Module-file variant: find the last "[...]" group whose body ends with
    // "line N, column M" after a file name.
    if let Some(open) = s.rfind(" [")
        && let Some(rel_line) = s[open..].find(" line ")
        && let Some((line, col)) = parse_position_body(&s[open + rel_line + 6..])
    {
        return (s[..open].to_string(), Some(line), Some(col));
    }
    (s.to_string(), None, None)
}

/// Parse `N, column M]...` into (N, M).
fn parse_position_body(rest: &str) -> Option<(u32, u32)> {
    let close = rest.find(']')?;
    let (l, c) = rest[..close].split_once(", column ")?;
    Some((l.trim().parse().ok()?, c.trim().parse().ok()?))
}
