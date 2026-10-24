//! Direct-manipulation hints as text: marking a numeric literal `tunable` or
//! `rigid` by wrapping it in the language's own hint builtins
//! (`tunable(40)`, `rigid(40)`) and unwrapping it again.
//!
//! This is what the mouse toggles in Petal IDE's drag mode (see
//! docs/petal-ide-mode.md, "Marking numbers tunable or rigid"). Because the
//! result is ordinary source, the next drag's solver reads the hint the way it
//! reads one the author typed, and nothing in Garden has to remember it.
//!
//! Pure and line-local: a line of text and a column in, the edits out. A hint
//! call written across several lines is not recognized (it reads as a call the
//! literal is merely inside, and is left alone).

/// What a literal is marked as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HintState {
    /// No hint: the solver may move it, at the ordinary cost.
    Free,
    /// `tunable(…)`: preferred, kept within its bounds.
    Tunable,
    /// `rigid(…)`: never moved.
    Rigid,
}

impl HintState {
    /// The mouse cycle: free → tunable → rigid → free.
    pub(crate) fn next(self) -> HintState {
        match self {
            HintState::Free => HintState::Tunable,
            HintState::Tunable => HintState::Rigid,
            HintState::Rigid => HintState::Free,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            HintState::Free => "free",
            HintState::Tunable => "tunable",
            HintState::Rigid => "rigid",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<HintState> {
        match s {
            "free" | "none" => Some(HintState::Free),
            "tunable" => Some(HintState::Tunable),
            "rigid" => Some(HintState::Rigid),
            _ => None,
        }
    }

    /// The builtin's name, for a hinted state.
    fn builtin(self) -> Option<&'static str> {
        match self {
            HintState::Free => None,
            HintState::Tunable => Some("tunable"),
            HintState::Rigid => Some("rigid"),
        }
    }
}

/// A hint call around a literal: `tunable(` at `start`, its `)` ending at
/// `end` (exclusive). Columns are chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HintCall {
    pub kind: HintState,
    pub start: usize,
    pub end: usize,
}

/// A numeric literal on a line, its leading unary minus included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Literal {
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// The hint call whose first argument this literal is.
    pub hint: Option<HintCall>,
    /// A later argument of a hint call (a `tunable` bound): never toggled,
    /// since the solver never edits one either.
    pub bound: bool,
}

impl Literal {
    pub(crate) fn state(&self) -> HintState {
        self.hint.map_or(HintState::Free, |h| h.kind)
    }

    /// The columns a click on it may land in: the whole hint call, if any.
    pub(crate) fn outer(&self) -> (usize, usize) {
        self.hint.map_or((self.start, self.end), |h| (h.start, h.end))
    }
}

/// One replacement on a line: chars `start..end` become `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num,
    Punct(char),
    /// A string, or a `#rrggbb` color: never a number.
    Opaque,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}

/// Words after which a `-` starts a negative number rather than subtracting.
const UNARY_AFTER: &[&str] = &[
    "return", "then", "else", "do", "in", "and", "or", "not", "if", "elif", "while", "let", "var",
];

fn tokenize(chars: &[char]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    let n = chars.len();
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            break;
        }
        let start = i;
        if c == '"' || c == '\'' {
            i += 1;
            while i < n && chars[i] != c {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(n);
            out.push(Token { tok: Tok::Opaque, start, end: i });
            continue;
        }
        if c == '#' && chars.get(i + 1).is_some_and(|c| c.is_ascii_alphanumeric()) {
            i += 1;
            while i < n && is_ident(chars[i]) {
                i += 1;
            }
            out.push(Token { tok: Tok::Opaque, start, end: i });
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            while i < n && is_ident(chars[i]) {
                i += 1;
            }
            out.push(Token {
                tok: Tok::Ident(chars[start..i].iter().collect()),
                start,
                end: i,
            });
            continue;
        }
        if c.is_ascii_digit() {
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                i += 1;
            }
            // A fraction needs a digit after the dot: `0..5` is a range.
            if i + 1 < n && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                i += 1;
                while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                    i += 1;
                }
            }
            if i < n && (chars[i] == 'e' || chars[i] == 'E') {
                let sign = chars.get(i + 1).is_some_and(|c| *c == '+' || *c == '-');
                let d = if sign { i + 2 } else { i + 1 };
                if chars.get(d).is_some_and(char::is_ascii_digit) {
                    i = d;
                    while i < n && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            // `10px`, `2x`: an identifier that starts with digits is not a number.
            if i < n && is_ident(chars[i]) {
                while i < n && is_ident(chars[i]) {
                    i += 1;
                }
                out.push(Token { tok: Tok::Opaque, start, end: i });
                continue;
            }
            out.push(Token { tok: Tok::Num, start, end: i });
            continue;
        }
        i += 1;
        out.push(Token { tok: Tok::Punct(c), start, end: i });
    }
    out
}

/// Whether the `-` at token `k` is a unary minus (it negates what follows).
fn is_unary_minus(toks: &[Token], k: usize) -> bool {
    if toks[k].tok != Tok::Punct('-') {
        return false;
    }
    match k.checked_sub(1).map(|p| &toks[p].tok) {
        None => true,
        Some(Tok::Punct(c)) => !matches!(c, ')' | ']' | '}'),
        Some(Tok::Ident(w)) => UNARY_AFTER.contains(&w.as_str()),
        Some(Tok::Num) | Some(Tok::Opaque) => false,
    }
}

/// The literals of a token stream as (first token, last token) index pairs.
fn literal_tokens(toks: &[Token]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (k, t) in toks.iter().enumerate() {
        if t.tok != Tok::Num {
            continue;
        }
        let first = match k.checked_sub(1) {
            Some(m) if toks[m].end == t.start && is_unary_minus(toks, m) => m,
            _ => k,
        };
        out.push((first, k));
    }
    out
}

/// Every numeric literal on `line`, in order, with its hint call.
pub(crate) fn literals(line: &str) -> Vec<Literal> {
    let chars: Vec<char> = line.chars().collect();
    let toks = tokenize(&chars);
    let lits = literal_tokens(&toks);

    // Hint calls: `tunable(` / `rigid(` (not a method, `.rigid(`), closed on
    // this line.
    struct Call {
        kind: HintState,
        start: usize,
        end: usize,
        /// Token range of the first argument, and of every argument.
        first: (usize, usize),
        args: (usize, usize),
    }
    let mut calls: Vec<Call> = Vec::new();
    for (k, t) in toks.iter().enumerate() {
        let kind = match &t.tok {
            Tok::Ident(w) if w == "tunable" => HintState::Tunable,
            Tok::Ident(w) if w == "rigid" => HintState::Rigid,
            _ => continue,
        };
        if k > 0 && toks[k - 1].tok == Tok::Punct('.') {
            continue;
        }
        if toks.get(k + 1).map(|t| &t.tok) != Some(&Tok::Punct('(')) {
            continue;
        }
        let mut depth = 0i32;
        let mut first_end = None;
        let mut close = None;
        for (j, u) in toks.iter().enumerate().skip(k + 1) {
            match u.tok {
                Tok::Punct('(' | '[' | '{') => depth += 1,
                Tok::Punct(')' | ']' | '}') => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
                Tok::Punct(',') if depth == 1 && first_end.is_none() => first_end = Some(j),
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        let first_end = first_end.unwrap_or(close);
        calls.push(Call {
            kind,
            start: t.start,
            end: toks[close].end,
            first: (k + 2, first_end),
            args: (k + 2, close),
        });
    }

    lits.into_iter()
        .map(|(a, b)| {
            // The innermost call whose first argument is exactly this literal.
            let hint = calls
                .iter()
                .filter(|c| c.first == (a, b + 1))
                .min_by_key(|c| c.end - c.start)
                .map(|c| HintCall {
                    kind: c.kind,
                    start: c.start,
                    end: c.end,
                });
            let bound = hint.is_none()
                && calls
                    .iter()
                    .any(|c| a >= c.args.0 && b < c.args.1 && !(a >= c.first.0 && b < c.first.1));
            Literal {
                start: toks[a].start,
                end: toks[b].end,
                text: chars[toks[a].start..toks[b].end].iter().collect(),
                hint,
                bound,
            }
        })
        .collect()
}

/// The literal a click at `col` means: one whose text, or whose hint call,
/// contains the column (a click just past its last char counts too). Bounds
/// are skipped.
pub(crate) fn literal_at(line: &str, col: usize) -> Option<Literal> {
    let lits: Vec<Literal> = literals(line).into_iter().filter(|l| !l.bound).collect();
    let inside = |l: &&Literal| l.start <= col && col < l.end;
    let in_call = |l: &&Literal| {
        let (s, e) = l.outer();
        s <= col && col < e
    };
    let just_after = |l: &&Literal| col == l.end;
    lits.iter()
        .find(inside)
        .or_else(|| lits.iter().find(in_call))
        .or_else(|| lits.iter().find(just_after))
        .cloned()
}

/// The edits that mark `lit` as `to`. Empty when it already is. Changing a
/// `tunable` with bounds to `rigid` drops the bounds (`rigid` takes none).
pub(crate) fn mark(lit: &Literal, to: HintState) -> Vec<LineEdit> {
    if lit.state() == to {
        return Vec::new();
    }
    let edit = |start, end, text: &str| LineEdit {
        start,
        end,
        text: text.to_string(),
    };
    match (lit.hint, to.builtin()) {
        (None, Some(name)) => vec![
            edit(lit.start, lit.start, &format!("{name}(")),
            edit(lit.end, lit.end, ")"),
        ],
        (Some(h), None) => vec![edit(h.start, lit.start, ""), edit(lit.end, h.end, "")],
        (Some(h), Some(name)) => vec![
            edit(h.start, lit.start, &format!("{name}(")),
            edit(lit.end, h.end, ")"),
        ],
        (None, None) => Vec::new(),
    }
}

/// `line` with `edits` applied (they must not overlap).
#[cfg(test)]
pub(crate) fn apply(line: &str, edits: &[LineEdit]) -> String {
    let mut chars: Vec<char> = line.chars().collect();
    let mut sorted: Vec<&LineEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse((e.start, e.end)));
    for e in sorted {
        let (s, t) = (e.start.min(chars.len()), e.end.min(chars.len()));
        chars.splice(s..t, e.text.chars());
    }
    chars.into_iter().collect()
}

/// Cycle the literal at `col`: its new state and the edits, or `None` when
/// no literal is there.
#[cfg(test)]
pub(crate) fn cycle_at(line: &str, col: usize) -> Option<(Literal, HintState, Vec<LineEdit>)> {
    let lit = literal_at(line, col)?;
    let to = lit.state().next();
    let edits = mark(&lit, to);
    Some((lit, to, edits))
}

/// What a line-level toggle acts on: the line's literals that overlap one of
/// `candidates` (char column ranges of the drag's candidate numbers on this
/// line), else the literal initializer of a `let` / `config let` / `var` /
/// `const` binding. Empty when neither.
pub(crate) fn line_targets(line: &str, candidates: &[(usize, usize)]) -> Vec<Literal> {
    let lits: Vec<Literal> = literals(line).into_iter().filter(|l| !l.bound).collect();
    let hit: Vec<Literal> = lits
        .iter()
        .filter(|l| candidates.iter().any(|&(s, e)| s < l.end && l.start < e))
        .cloned()
        .collect();
    if !hit.is_empty() {
        return hit;
    }
    let chars: Vec<char> = line.chars().collect();
    let toks = tokenize(&chars);
    let mut k = 0;
    if matches!(&toks.first().map(|t| &t.tok), Some(Tok::Ident(w)) if w == "config") {
        k = 1;
    }
    let is_kw = matches!(toks.get(k).map(|t| &t.tok), Some(Tok::Ident(w)) if matches!(w.as_str(), "let" | "var" | "const"));
    let is_name = matches!(toks.get(k + 1).map(|t| &t.tok), Some(Tok::Ident(_)));
    let is_eq = toks.get(k + 2).map(|t| &t.tok) == Some(&Tok::Punct('='));
    if !(is_kw && is_name && is_eq) {
        return Vec::new();
    }
    let Some(rest) = toks.get(k + 3) else {
        return Vec::new();
    };
    let rest_end = toks.last().map_or(rest.end, |t| t.end);
    lits.into_iter()
        .filter(|l| l.outer() == (rest.start, rest_end))
        .collect()
}

/// Cycle a whole line (see [`line_targets`]): every target goes to the state
/// after the first target's. `None` when the line has nothing to toggle.
pub(crate) fn cycle_line(line: &str, candidates: &[(usize, usize)]) -> Option<(Vec<Literal>, HintState, Vec<LineEdit>)> {
    let targets = line_targets(line, candidates);
    let to = targets.first()?.state().next();
    let edits = targets.iter().flat_map(|l| mark(l, to)).collect();
    Some((targets, to, edits))
}

/// Where column `col` lands once `edits` (in the line's original columns)
/// are applied. `start_side`: the column starts a range, so an insertion at
/// exactly `col` pushes it right; an end column stays left of one.
pub(crate) fn map_col(col: usize, edits: &[LineEdit], start_side: bool) -> usize {
    let mut delta: isize = 0;
    let mut clamp: Option<usize> = None;
    for e in edits {
        let grow = e.text.chars().count() as isize - (e.end - e.start) as isize;
        if e.start == e.end {
            if col > e.start || (start_side && col == e.start) {
                delta += grow;
            }
        } else if col >= e.end {
            delta += grow;
        } else if col > e.start {
            // Inside a replaced range: pin to the start of its replacement.
            clamp = Some(e.start);
        }
    }
    let col = clamp.unwrap_or(col);
    (col as isize + delta).max(0) as usize
}

/// A range `start..end` after `edits`.
pub(crate) fn map_range((start, end): (usize, usize), edits: &[LineEdit]) -> (usize, usize) {
    let s = map_col(start, edits, true);
    let e = map_col(end, edits, false).max(s);
    (s, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycled(line: &str, col: usize) -> (String, HintState) {
        let (_, to, edits) = cycle_at(line, col).expect("a literal");
        (apply(line, &edits), to)
    }

    #[test]
    fn wraps_a_free_literal_in_tunable() {
        let line = "let gap = 40";
        assert_eq!(cycled(line, 10), ("let gap = tunable(40)".to_string(), HintState::Tunable));
    }

    #[test]
    fn cycles_free_tunable_rigid_free() {
        let mut line = "draw_rect(40, 10, 5, 5, #101418)".to_string();
        let mut states = Vec::new();
        for _ in 0..3 {
            let col = line.find("40").unwrap();
            let (next, to) = cycled(&line, col);
            line = next;
            states.push((line.clone(), to));
        }
        assert_eq!(states[0].0, "draw_rect(tunable(40), 10, 5, 5, #101418)");
        assert_eq!(states[1].0, "draw_rect(rigid(40), 10, 5, 5, #101418)");
        assert_eq!(states[2].0, "draw_rect(40, 10, 5, 5, #101418)");
        assert_eq!(
            states.iter().map(|s| s.1).collect::<Vec<_>>(),
            [HintState::Tunable, HintState::Rigid, HintState::Free]
        );
    }

    #[test]
    fn unwraps_rigid_keeping_the_rest_of_the_line() {
        let line = "let g = rigid(9.8)   // gravity";
        assert_eq!(cycled(line, 15), ("let g = 9.8   // gravity".to_string(), HintState::Free));
        // A click on the builtin's name means its literal too.
        assert_eq!(cycled(line, 9).0, "let g = 9.8   // gravity");
    }

    #[test]
    fn negative_numbers_keep_their_sign_inside_the_hint() {
        let line = "let a = offset(center, -60, 0)";
        let col = line.find("60").unwrap();
        assert_eq!(cycled(line, col).0, "let a = offset(center, tunable(-60), 0)");
        // A click on the minus itself is the same literal.
        assert_eq!(cycled(line, col - 1).0, "let a = offset(center, tunable(-60), 0)");
        let wrapped = "let a = offset(center, rigid(-60), 0)";
        assert_eq!(cycled(wrapped, wrapped.find("60").unwrap()).0, line);
    }

    #[test]
    fn binary_minus_is_not_part_of_the_literal() {
        let line = "x0 + i * gap - 5";
        let lits = literals(line);
        assert_eq!(lits.len(), 1);
        assert_eq!(lits[0].text, "5");
        assert_eq!(cycled(line, line.len() - 1).0, "x0 + i * gap - tunable(5)");
    }

    #[test]
    fn a_literal_inside_an_expression() {
        let line = "  draw_rect(x0 + i * (size + 12), 100, size, size, #c08040)";
        let col = line.find("12").unwrap();
        assert_eq!(
            cycled(line, col).0,
            "  draw_rect(x0 + i * (size + tunable(12)), 100, size, size, #c08040)"
        );
    }

    #[test]
    fn already_wrapped_with_range_args() {
        let line = "let shrink = tunable(0.7, 0.5, 0.8)";
        let lits = literals(line);
        assert_eq!(lits.len(), 3);
        assert_eq!(lits[0].state(), HintState::Tunable);
        assert!(lits[1].bound && lits[2].bound, "the bounds are never toggled");
        // tunable → rigid drops the bounds (rigid takes one argument).
        assert_eq!(cycled(line, line.find("0.7").unwrap()).0, "let shrink = rigid(0.7)");
        // A click on a bound means the hinted literal (the call contains it).
        assert_eq!(cycled(line, line.find("0.8").unwrap()).0, "let shrink = rigid(0.7)");
        // Named bounds too.
        let named = "let r = tunable(30, min: 0, max: 50)";
        assert_eq!(mark(&literals(named)[0], HintState::Free).len(), 2);
        assert_eq!(apply(named, &mark(&literals(named)[0], HintState::Free)), "let r = 30");
    }

    #[test]
    fn strings_colors_comments_and_identifiers_hold_no_literals() {
        let line = r#"draw_text("40 px", x2, 7, #101418) // 99"#;
        let lits = literals(line);
        assert_eq!(lits.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["7"]);
        assert!(literal_at(line, line.find("99").unwrap()).is_none());
        // A range is two integers, not a float.
        let range = "for i in range(0, 6) do";
        assert_eq!(literals(range).iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["0", "6"]);
        assert_eq!(literals("for i in 0..5 do").len(), 2);
        assert_eq!(literals("let e = 1.5e-3").first().map(|l| l.text.clone()), Some("1.5e-3".into()));
    }

    #[test]
    fn a_method_named_rigid_is_not_a_hint() {
        let line = "let b = body.rigid(40)";
        assert_eq!(literals(line)[0].state(), HintState::Free);
    }

    #[test]
    fn line_toggle_prefers_candidates_then_the_let_initializer() {
        let line = "  draw_circle(x0 + 16, 60, 8, 255, 255, 255)";
        let at = |s: &str| {
            let c = line.find(s).unwrap();
            (c, c + s.len())
        };
        let targets = line_targets(line, &[at("16"), at("60")]);
        assert_eq!(targets.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["16", "60"]);
        let (_, to, edits) = cycle_line(line, &[at("16"), at("60")]).unwrap();
        assert_eq!(to, HintState::Tunable);
        assert_eq!(
            apply(line, &edits),
            "  draw_circle(x0 + tunable(16), tunable(60), 8, 255, 255, 255)"
        );
        // No candidates: a binding's literal initializer.
        assert_eq!(line_targets("config let gap = 30", &[])[0].text, "30");
        assert_eq!(line_targets("let g = rigid(9.8)", &[])[0].state(), HintState::Rigid);
        let (_, to, edits) = cycle_line("let g = rigid(9.8)", &[]).unwrap();
        assert_eq!((apply("let g = rigid(9.8)", &edits), to), ("let g = 9.8".into(), HintState::Free));
        // A computed initializer, or a plain statement, has nothing to toggle.
        assert!(line_targets("let x = a + 30", &[]).is_empty());
        assert!(cycle_line(line, &[]).is_none());
    }

    #[test]
    fn mixed_states_on_a_line_follow_the_first() {
        let line = "f(rigid(1), 2)";
        let (targets, to, edits) = cycle_line(line, &[(8, 9), (12, 13)]).unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(to, HintState::Free);
        assert_eq!(apply(line, &edits), "f(1, 2)");
    }

    #[test]
    fn ranges_map_through_a_wrap_and_an_unwrap() {
        let line = "f(40, 7)";
        let lit = literal_at(line, 2).unwrap();
        let edits = mark(&lit, HintState::Tunable);
        let out = apply(line, &edits);
        assert_eq!(out, "f(tunable(40), 7)");
        // The literal itself moves inside the call; the later one shifts by 9.
        assert_eq!(map_range((2, 4), &edits), (10, 12));
        assert_eq!(&out[10..12], "40");
        assert_eq!(map_range((6, 7), &edits), (15, 16));
        assert_eq!(&out[15..16], "7");
        let back = mark(&literal_at(&out, 10).unwrap(), HintState::Free);
        assert_eq!(apply(&out, &back), line);
        assert_eq!(map_range((10, 12), &back), (2, 4));
        // A replacement of the same width (a drag's 30 → 36) moves nothing.
        let swap = [LineEdit { start: 2, end: 4, text: "36".into() }];
        assert_eq!(map_range((2, 4), &swap), (2, 4));
        let grow = [LineEdit { start: 2, end: 4, text: "100".into() }];
        assert_eq!(map_range((2, 4), &grow), (2, 5));
        assert_eq!(map_range((6, 7), &grow), (7, 8));
    }
}
