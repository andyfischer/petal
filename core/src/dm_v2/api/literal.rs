//! A literal's text in the source, and a new value rendered in its place.

use std::path::PathBuf;

use crate::dm_v2::params::Param;
use crate::env::Env;
use crate::program::Program;

use super::response::ParamChange;

/// What a parameter looks like in the source.
#[derive(Debug, Clone)]
pub(super) struct ParamText {
    /// The literal as written, sign included.
    pub(super) text: String,
    /// The first non-blank character before it.
    pub(super) prev: Option<char>,
    pub(super) function: Option<String>,
    pub(super) path: Option<PathBuf>,
}

/// The literal's text and surroundings.
pub(super) fn param_text(program: &Program, p: &Param) -> ParamText {
    let file = program.source_map.files.get(p.file.0 as usize);
    let src: Vec<char> = file.map(|f| f.source.chars().collect()).unwrap_or_default();
    let (s, e) = (p.span.start.offset as usize, p.span.end.offset as usize);
    let text: String = src.get(s.min(src.len())..e.min(src.len())).map(|c| c.iter().collect()).unwrap_or_default();
    let prev = src[..s.min(src.len())].iter().rev().find(|c| !c.is_whitespace()).copied();
    let function = p.function.and_then(|f| program.functions.get(f.0 as usize)).and_then(|d| d.name.clone()).map(|n| {
        let n = crate::program::base_fn_name(&n).to_string();
        n.rsplit("::").next().unwrap_or(&n).to_string()
    });
    let path = file.and_then(|f| f.origin.clone()).filter(|o| !o.to_string_lossy().starts_with('<'));
    ParamText { text, prev, function, path }
}

/// `v` written the way `p` was (§2.4.4): an int stays an int when whole; a
/// float keeps its padded decimals (`3.50`), else the shortest spelling with
/// a decimal point. Returns the text and whether an int became a float.
pub(super) fn render(p: &Param, t: &ParamText, v: f64) -> (String, bool) {
    let respelled = p.is_int && v.fract() != 0.0;
    let v = if v == 0.0 { 0.0 } else { v };
    if p.is_int && !respelled {
        return (format!("{}", v as i64), false);
    }
    let model = t.text.trim_start_matches('-');
    (crate::literal_edit::float_like(v, if p.is_int { "" } else { model }), respelled)
}

/// Whether a negative replacement for the non-negative literal `p` must be
/// parenthesized: it would read `x - -3` (or `x + -3`), or the file would not
/// parse with it. A postfix operator needs none: `-2.f()` is `(-2).f()`.
/// [`render`], parenthesized when a negative value written where a
/// non-negative one stood would not parse as the same expression
/// (`needs_parens` decides, given the rendered text).
pub(super) fn render_in_place(p: &Param, t: &ParamText, v: f64, needs_parens: impl FnOnce(&str) -> bool) -> (String, bool) {
    let (text, respelled) = render(p, t, v);
    if v < 0.0 && !t.text.starts_with('-') && needs_parens(&text) {
        return (format!("({text})"), respelled);
    }
    (text, respelled)
}

pub(super) fn needs_parens(env: &Env, pid: crate::program::ProgramId, p: &Param, t: &ParamText, text: &str) -> bool {
    if needs_parens_by_neighbours(t) {
        return true;
    }
    let Some(program) = env.get_program(pid) else { return false };
    let Some(file) = program.source_map.files.get(p.file.0 as usize) else { return false };
    let edited = crate::rewrite::splice(&file.source, p.span, text);
    crate::cst::parse_source(&edited, p.file).is_err()
}

/// The half of [`needs_parens`] that reads only the literal's neighbours.
pub(super) fn needs_parens_by_neighbours(t: &ParamText) -> bool {
    matches!(t.prev, Some('-' | '+'))
}

pub(super) fn change_of(p: &Param, t: &ParamText, v: f64, (new_text, respelled): (String, bool)) -> ParamChange {
    ParamChange {
        term: p.term,
        span: p.span,
        file: p.file,
        binding: p.binding.as_ref().map(|b| b.rsplit("::").next().unwrap_or(b).to_string()),
        function: t.function.clone(),
        config: p.is_knob(),
        old: p.value,
        new: v,
        old_text: t.text.clone(),
        new_text,
        respelled,
    }
}
