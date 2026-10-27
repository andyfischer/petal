//! The runs an `assert` (or another error) rejected, grouped by the check
//! that rejected them.

use crate::dm_v2::solve::FailedRun;
use crate::dm_v2::spec::RunError;
use crate::program::{Program, TermId};
use crate::source_map::FileId;

use super::response::{Rejection, TriedLiteral};

/// Group `failed` runs by their error (§ L3: an `assert` is a constraint).
/// `col` gives a Jacobian column's literal: term, baseline value, line, text.
pub(super) fn rejections(
    program: Option<&Program>,
    failed: &[FailedRun],
    col: impl Fn(usize) -> (TermId, f64, u32, String),
) -> Vec<Rejection> {
    let mut out: Vec<Rejection> = Vec::new();
    for f in failed {
        let r = rejection_of(program, &f.error);
        if let Some(seen) = out
            .iter_mut()
            .find(|o| o.message == r.message && o.file == r.file && o.line == r.line && o.column == r.column)
        {
            seen.runs += 1;
            continue;
        }
        let tried = f
            .cols
            .iter()
            .zip(&f.values)
            .filter_map(|(&c, &v)| {
                let (term, old, line, old_text) = col(c);
                (v != old).then_some(TriedLiteral { term, line, old_text, value: v })
            })
            .collect();
        out.push(Rejection { runs: 1, tried, ..r });
    }
    out
}

/// What a run's error says, and where: the message, the position tag the VM
/// appends (`[line N, column M]`, or `[m.ptl line N, column M]` in a module),
/// and for an `assert(...)` the condition's source text.
pub(super) fn rejection_of(program: Option<&Program>, e: &RunError) -> Rejection {
    let raw = e.to_string();
    let first = crate::dm_v2::solve::first_line(&raw);
    let mut r = Rejection {
        message: first.to_string(),
        file: None,
        file_name: None,
        line: None,
        column: None,
        condition: None,
        runs: 0,
        tried: Vec::new(),
    };
    if !matches!(e, RunError::Runtime(_)) {
        return r;
    }
    let Some(open) = first.rfind(" [").filter(|_| first.ends_with(']')) else { return r };
    let tag = &first[open + 2..first.len() - 1];
    let Some(at) = tag.rfind("line ") else { return r };
    let Some((line, column)) = tag[at + 5..].split_once(", column ") else { return r };
    let (Ok(line), Ok(column)) = (line.trim().parse::<u32>(), column.trim().parse::<u32>()) else { return r };
    let module = tag[..at].trim();
    r.message = first[..open].to_string();
    r.line = Some(line);
    r.column = Some(column);
    let Some(program) = program else { return r };
    let files = &program.source_map.files;
    let fid = if module.is_empty() {
        Some(crate::source_map::ENTRY_FILE)
    } else {
        files.iter().position(|f| f.name == module).map(|i| FileId(i as u16))
    };
    r.file = fid;
    let file = fid.and_then(|f| files.get(f.0 as usize));
    r.file_name = if module.is_empty() {
        file.and_then(|f| f.origin.as_ref().and_then(|o| o.file_name()).map(|n| n.to_string_lossy().into_owned()))
            .or_else(|| file.map(|f| f.name.clone()).filter(|n| !n.is_empty() && !n.starts_with('<')))
    } else {
        Some(module.to_string())
    };
    let source = file.map_or(program.source.as_str(), |f| f.source.as_str());
    if let Some(text) = source.lines().nth(line.saturating_sub(1) as usize) {
        let from: String = text.chars().skip(column.saturating_sub(1) as usize).collect();
        r.condition = assert_condition(&from);
    }
    r
}

/// The condition of the `assert(...)` call `text` starts with: everything
/// up to the first top-level comma (the message) or the closing paren.
pub(super) fn assert_condition(text: &str) -> Option<String> {
    let rest = text.strip_prefix("assert")?.trim_start().strip_prefix('(')?;
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, ch) in rest.char_indices() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') if depth > 0 => depth -= 1,
            (None, ')' | ',') if depth == 0 => {
                let c = rest[..i].trim();
                return (!c.is_empty()).then(|| c.to_string());
            }
            _ => {}
        }
    }
    None
}
