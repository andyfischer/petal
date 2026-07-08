//! `petal lint` — source normalization (see docs/dev/linter-plan.md).
//!
//! Two passes, split by mechanism so neither ever reprints from the AST:
//!
//! 1. **Formatting** ([`reindent`]) — token-driven 2-space re-indentation.
//!    Nesting depth is computed from block-opening/-closing tokens and
//!    delimiters, and only the *leading whitespace* of each line is rewritten
//!    (plus trailing-whitespace trim and a single trailing newline).
//!    Everything else on a line — including comments — is copied verbatim, and
//!    any line that starts or ends inside a multi-line token (raw strings, JSX
//!    text) is left untouched, so the pass is comment- and content-safe by
//!    construction. Petal is newline-significant but not
//!    indentation-significant, so this cannot change semantics.
//!
//! 2. **Rebind** ([`find_rebinds`]) — the semantics-preserving idiom rewrite
//!    `x = f(x)` → `f(@x)`. Candidates are detected on the AST and applied as
//!    two minimal string splices (delete the `x = ` prefix, insert `@` before
//!    the matching argument) — no reprinting, so comments inside the call
//!    survive.
//!
//! Because rebind changes tokens (not just whitespace), [`lint_source`] gates
//! it behind an **IR-equivalence check**: the pre- and post-lint sources must
//! compile to structurally identical IR (modulo source text and spans). If
//! the original doesn't compile (e.g. imports unresolvable here), rebinds are
//! skipped and only formatting applies; if the gate ever reports a real
//! difference, lint refuses to produce output — that's a linter bug, not a
//! user error.

use std::path::PathBuf;

use crate::ast::{self, AssignTarget, Expr, ExprKind, ExprVisitor, Stmt, StmtKind};
use crate::env::Env;
use crate::lexer::{Lexer, Token};

/// Context the IR-equivalence gate needs to compile the source the same way
/// `petal run` would: module search dirs and the file's own path (imports
/// resolve relative to it).
#[derive(Default)]
pub struct LintOptions {
    pub include_dirs: Vec<PathBuf>,
    pub origin: Option<PathBuf>,
}

/// The result of linting one source text.
pub struct LintOutcome {
    /// The normalized source.
    pub output: String,
    /// Lines whose text changed in the formatting pass.
    pub reindented_lines: usize,
    /// Rebind rewrites applied (post-gate).
    pub rebinds: usize,
    /// Human-readable notes (e.g. rebinds skipped because the IR gate was
    /// unavailable).
    pub notes: Vec<String>,
}

impl LintOutcome {
    pub fn changed(&self, original: &str) -> bool {
        self.output != original
    }
}

/// Normalize `source`: apply rebind rewrites (IR-gated), then re-indent.
/// Errors if the source doesn't parse, or if a rewrite fails the equivalence
/// gate outright (which indicates a lint bug and refuses all output).
pub fn lint_source(source: &str, opts: &LintOptions) -> Result<LintOutcome, String> {
    // Lint operates on valid programs only.
    let (_tree, stmts) = crate::rewrite::parse_ast(source)?;

    let mut notes = Vec::new();
    let chars: Vec<char> = source.chars().collect();
    let candidates = find_rebinds(&stmts, &chars);
    let mut rebinds = candidates.len();
    let mut rebound = if candidates.is_empty() {
        source.to_string()
    } else {
        apply_rebinds(&chars, &candidates)
    };

    if rebinds > 0 {
        match ir_gate(source, &rebound, opts) {
            Gate::Equivalent => {}
            Gate::Different(detail) => {
                return Err(format!(
                    "lint bug: the rebind rewrite changed the compiled IR — refusing to \
                     produce output ({detail})"
                ));
            }
            Gate::Unavailable(reason) => {
                notes.push(format!(
                    "skipped {rebinds} rebind rewrite(s): can't verify IR equivalence ({reason})"
                ));
                rebinds = 0;
                rebound = source.to_string();
            }
        }
    }

    let output = reindent(&rebound)?;
    let reindented_lines = count_changed_lines(&rebound, &output);
    Ok(LintOutcome { output, reindented_lines, rebinds, notes })
}

// ---------------------------------------------------------------------------
// Pass 1 — re-indentation
// ---------------------------------------------------------------------------

/// The open-construct stack: one entry per unclosed construct, holding the
/// indent its *contents* get — the display indent of the line that opened it,
/// plus one. Keying content indent to the opening line (rather than raw stack
/// depth) makes several delimiters opened on one line (`column([`) indent
/// their contents once, and their closers (`])`) realign with the opening
/// line.
///
/// Constructs and their closers: `end` closes fn/enum/if/for/while/match,
/// block lambdas, and `when … do` arms; `)` `]` `}` close their delimiters;
/// `</tag>` closes a JSX element's children. Closers just pop the innermost
/// entry — lint runs on parseable source, so they always correspond.
type OpenStack = Vec<usize>;

/// Re-indent `source` to 2-space indentation, trim trailing whitespace, and
/// end with exactly one newline. Only whitespace outside tokens is touched;
/// lines that start inside a multi-line token (raw string, JSX text) are
/// copied verbatim. Works from the token stream alone, so it needs the source
/// to lex but not to parse.
pub fn reindent(source: &str) -> Result<String, String> {
    if source.is_empty() {
        return Ok(String::new());
    }
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let tokens = &lexer.tokens;
    let spans = &lexer.token_spans;
    let chars: Vec<char> = source.chars().collect();

    // Line table: (start, end) char offsets, `end` at the `\n` (or EOF).
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            lines.push((start, i));
            start = i + 1;
        }
    }
    lines.push((start, chars.len()));

    let mut stack: OpenStack = Vec::new();
    // A `for`/`while` header's `do` belongs to the construct already opened at
    // the keyword; only a `do` with no pending header opens a block itself
    // (a `when … do` match arm).
    let mut pending_do = false;
    // Inside `when pattern [if guard]` the `if` is a guard, not an opener.
    // Cleared at the arm's `do`/`->` (or a newline, defensively).
    let mut when_header = false;
    // Stack of unterminated JSX opening tags, each recording the delimiter
    // depth at its `<`: the `>` that ends the tag is the one seen back at that
    // depth (a `>` inside an `attr={a > b}` brace sits deeper).
    let mut open_tags: Vec<usize> = Vec::new();

    let mut out_lines: Vec<String> = Vec::with_capacity(lines.len());
    let mut ti = 0usize; // next token index
    let mut covered_end = 0usize; // max token end seen so far

    for &(ls, le) in &lines {
        // Tokens starting on this line (the Newline terminator included).
        let first_ti = ti;
        while ti < tokens.len() && (spans[ti].start.offset as usize) <= le {
            covered_end = covered_end.max(spans[ti].end.offset as usize);
            ti += 1;
        }
        let line_tokens = first_ti..ti;

        // A line beginning inside a token that started earlier (multi-line raw
        // string, JSX text) is content, not layout — copy it verbatim. The
        // check uses tokens *before* this line, so recompute from `ti` bounds.
        let starts_inside = spans[..first_ti]
            .iter()
            .any(|s| (s.start.offset as usize) < ls && (s.end.offset as usize) > ls);

        // Leading-whitespace run.
        let mut ws_end = ls;
        while ws_end < le && chars[ws_end] != '\n' && chars[ws_end].is_whitespace() {
            ws_end += 1;
        }
        // A token starting within the leading run means that "whitespace" is
        // token content (JSX text beginning at line start) — leave it alone.
        let token_in_leading_ws = line_tokens
            .clone()
            .any(|k| { let s = spans[k].start.offset as usize; s >= ls && s < ws_end });

        // Trailing trim is safe only when no token spills past the line end
        // (the spill means the tail is inside a multi-line token).
        let spills_past_end = line_tokens
            .clone()
            .any(|k| (spans[k].end.offset as usize) > le + 1);

        // Display indent. A line opening no new construct sits at the
        // innermost open construct's content indent. A run of closers at the
        // start of the line realigns with the line that opened the outermost
        // construct the run closes; `else`/`elsif` realign with their `if`.
        let sig: Vec<usize> = line_tokens
            .clone()
            .filter(|&k| !matches!(tokens[k], Token::Newline | Token::Eof))
            .collect();
        let mut dedent = 0usize;
        let mut si = 0usize;
        while si < sig.len() {
            match tokens[sig[si]] {
                Token::End | Token::RParen | Token::RBracket | Token::RBrace => {
                    dedent += 1;
                    si += 1;
                }
                Token::JsxCloseStart => {
                    dedent += 1;
                    si += 1;
                    if si < sig.len() && matches!(tokens[sig[si]], Token::JsxTagName(_)) {
                        si += 1;
                    }
                }
                _ => break,
            }
        }
        let indent = if dedent > 0 {
            // Align with the opener of the outermost construct the run closes.
            stack
                .len()
                .checked_sub(dedent)
                .and_then(|i| stack.get(i))
                .map_or(0, |content| content.saturating_sub(1))
        } else if matches!(sig.first().map(|&k| &tokens[k]), Some(Token::Else | Token::Elsif)) {
            stack.last().map_or(0, |content| content.saturating_sub(1))
        } else {
            stack.last().copied().unwrap_or(0)
        };

        // Render the line.
        if starts_inside || token_in_leading_ws {
            out_lines.push(chars[ls..le].iter().collect());
        } else {
            let mut content_end = le;
            if !spills_past_end {
                while content_end > ws_end && chars[content_end - 1].is_whitespace() {
                    content_end -= 1;
                }
            }
            if content_end == ws_end {
                out_lines.push(String::new()); // blank line
            } else {
                let mut line = "  ".repeat(indent);
                line.extend(chars[ws_end..content_end].iter());
                out_lines.push(line);
            }
        }

        // Update depth with this line's tokens (done for every line — a
        // verbatim line can still contain tokens that open or close blocks).
        // Constructs opened on this line indent their contents one past this
        // line's own indent, however many of them open here.
        for k in line_tokens {
            match &tokens[k] {
                Token::Newline => when_header = false,
                Token::When => when_header = true,
                Token::Arrow => when_header = false,
                Token::Do => {
                    when_header = false;
                    if pending_do {
                        pending_do = false;
                    } else {
                        stack.push(indent + 1); // `when … do` arm body
                    }
                }
                Token::For | Token::While => {
                    pending_do = true;
                    stack.push(indent + 1);
                }
                Token::If => {
                    if !when_header {
                        stack.push(indent + 1);
                    }
                }
                Token::Match | Token::Enum => stack.push(indent + 1),
                Token::Fn => {
                    if fn_takes_end(tokens, k) {
                        stack.push(indent + 1);
                    }
                }
                Token::LParen | Token::LBracket | Token::LBrace => stack.push(indent + 1),
                Token::End | Token::RParen | Token::RBracket | Token::RBrace => {
                    stack.pop();
                }
                Token::JsxOpenStart => open_tags.push(stack.len()),
                Token::JsxSelfClose => {
                    open_tags.pop();
                }
                Token::Gt => {
                    if open_tags.last() == Some(&stack.len()) {
                        open_tags.pop();
                        stack.push(indent + 1); // children until `</tag>`
                    }
                }
                Token::JsxCloseStart => {
                    stack.pop();
                }
                _ => {}
            }
        }
    }

    // Single trailing newline: drop blank lines at EOF, end with exactly one.
    while out_lines.last().is_some_and(|l| l.is_empty()) {
        out_lines.pop();
    }
    if out_lines.is_empty() {
        return Ok(String::new());
    }
    let mut out = out_lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// Whether the `fn` at token index `k` opens an `end`-terminated block: a
/// declaration (`fn name(…) … end`) or a block-bodied lambda (`fn(…) … end`).
/// Only an arrow lambda (`fn(…) -> expr`) doesn't consume an `end`.
fn fn_takes_end(tokens: &[Token], k: usize) -> bool {
    let mut i = k + 1;
    while i < tokens.len() && matches!(tokens[i], Token::Newline) {
        i += 1;
    }
    match tokens.get(i) {
        Some(Token::Ident(_)) => true, // declaration
        Some(Token::LParen) => {
            // Lambda: skip the parameter list to its matching `)`.
            let mut depth = 0usize;
            while i < tokens.len() {
                match tokens[i] {
                    Token::LParen => depth += 1,
                    Token::RParen => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            i += 1;
            while i < tokens.len() && matches!(tokens[i], Token::Newline) {
                i += 1;
            }
            !matches!(tokens.get(i), Some(Token::Arrow))
        }
        _ => false,
    }
}

fn count_changed_lines(before: &str, after: &str) -> usize {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let common = a.len().min(b.len());
    let mut n = (0..common).filter(|&i| a[i] != b[i]).count();
    n += a.len().max(b.len()) - common;
    n
}

// ---------------------------------------------------------------------------
// Pass 2 — rebind (`x = f(x)` → `f(@x)`)
// ---------------------------------------------------------------------------

/// One rebind rewrite, as two minimal char-offset edits: delete the
/// `x = ` prefix `[prefix_start, prefix_end)` and insert `@` at `at_pos`
/// (the start of the matching argument). No reprinting, so comments and
/// layout inside the call survive.
#[derive(Debug, Clone, Copy)]
struct Rebind {
    prefix_start: usize,
    prefix_end: usize,
    at_pos: usize,
}

/// Detect rebind candidates. A statement `x = f(a, …)` qualifies when:
/// - the target is a plain name and the value is a call whose callee is a
///   plain identifier (not `x` itself, and not a field/method chain),
/// - exactly one argument is exactly `Ident(x)`,
/// - `x` appears nowhere else in the whole value (no `x = g(x, x)` ambiguity —
///   this also rejects `x` captured in nested lambdas/blocks, conservatively),
/// - and the value contains no `@` already (a second marker on the same call
///   would stop the desugarer from lifting either).
///
/// The walk mirrors [`crate::desugar`]'s recursion *exactly*: candidates are
/// only collected in statement scopes the desugarer lifts `@` from (top level,
/// fn/for/while/if/block/lambda bodies) — never inside match arms or `while`
/// conditions, where an `@` would survive to the compiler as an error.
///
/// The desugarer rewrites statement-level `f(@x)` back to exactly `x = f(x)`,
/// so the rewrite is semantics-preserving by construction; [`lint_source`]
/// verifies it against the compiled IR anyway.
fn find_rebinds(stmts: &[Stmt], chars: &[char]) -> Vec<Rebind> {
    let mut out = Vec::new();
    let mut finder = Rebinder { chars, out: &mut out };
    for stmt in stmts {
        finder.visit_stmt(stmt);
    }
    out
}

/// Walks the statement scopes the desugarer lifts `@` from, collecting rebind
/// candidates. Its traversal mirrors [`crate::desugar`] exactly by overriding
/// the two nodes where the desugarer stops short of a total walk: a `while`
/// condition (re-evaluated each iteration) and `match` arms (conditional
/// bodies) are never lifted into, so no candidates are collected there.
struct Rebinder<'a> {
    chars: &'a [char],
    out: &'a mut Vec<Rebind>,
}

impl ExprVisitor for Rebinder<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let StmtKind::Assign { target: AssignTarget::Name(x), value } = &stmt.kind
            && let Some(rebind) = rebind_candidate(stmt, x, value, self.chars)
        {
            self.out.push(rebind);
        }
        match &stmt.kind {
            StmtKind::Let { value, .. }
            | StmtKind::Expr(value)
            | StmtKind::Return(Some(value)) => self.visit_expr(value),
            StmtKind::Assign { target, value } => {
                self.visit_expr(value);
                match target {
                    AssignTarget::Name(_) => {}
                    AssignTarget::Field(obj, _) => self.visit_expr(obj),
                    AssignTarget::Index(obj, idx) => {
                        self.visit_expr(obj);
                        self.visit_expr(idx);
                    }
                }
            }
            StmtKind::State { init, key, .. } => {
                self.visit_expr(init);
                if let Some(k) = key {
                    self.visit_expr(k);
                }
            }
            StmtKind::FnDecl { body, .. } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                for s in body {
                    self.visit_stmt(s);
                }
            }
            // The desugarer never lifts from a `while` condition (it
            // re-evaluates each iteration), so recurse the body only.
            StmtKind::While { body, .. } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::Return(None)
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::EnumDecl { .. }
            | StmtKind::Import(_) => {}
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        // The desugarer lifts only the subject of a `match`, not the arms.
        if let ExprKind::Match { subject, .. } = &expr.kind {
            self.visit_expr(subject);
            return;
        }
        ast::walk_expr(self, expr);
    }
}

/// Check one `x = value` assignment against the rebind rule, with textual
/// sanity checks so a surprising span can only ever make us *skip*, never
/// produce a wrong edit.
fn rebind_candidate(stmt: &Stmt, x: &str, value: &Expr, chars: &[char]) -> Option<Rebind> {
    let ExprKind::Call { function, args } = &value.kind else { return None };
    // Callee must be a plain identifier other than `x` — a field/method chain
    // could evaluate `x` on its own, which the occurrence count below would
    // catch, but a plain-ident callee is also what the `@` sugar reads best on.
    let ExprKind::Ident(callee) = &function.kind else { return None };
    if callee == x {
        return None;
    }
    if count_ident(value, x) != 1 {
        return None;
    }
    // An `@` already present anywhere in the value could end up as a second
    // marker on the same call, which the desugarer refuses to lift.
    if contains_atvar(value) {
        return None;
    }
    let matching: Vec<&Expr> = args
        .iter()
        .filter(|a| matches!(&a.kind, ExprKind::Ident(n) if n == x))
        .collect();
    let [arg] = matching[..] else { return None };

    let prefix_start = stmt.span.start.offset as usize;
    let prefix_end = value.span.start.offset as usize;
    let at_pos = arg.span.start.offset as usize;
    if prefix_start >= prefix_end || at_pos < prefix_end || at_pos >= chars.len() {
        return None;
    }
    // The deleted prefix must be exactly `x`, `=`, and whitespace — same line.
    let prefix: String = chars[prefix_start..prefix_end].iter().collect();
    let rest = prefix.strip_prefix(x)?;
    if rest.trim() != "=" || prefix.contains('\n') {
        return None;
    }
    // The insertion point must sit on the identifier itself (`f((x))` gives
    // the inner ident's span, but be safe against span drift).
    if !chars[at_pos..].starts_with(&x.chars().collect::<Vec<_>>()[..]) {
        return None;
    }
    Some(Rebind { prefix_start, prefix_end, at_pos })
}

/// Whether any `@var` marker appears anywhere in `e` (nested bodies included).
fn contains_atvar(e: &Expr) -> bool {
    let mut found = false;
    ast::for_each_expr(e, &mut |e| found |= matches!(e.kind, ExprKind::AtVar(_)));
    found
}

/// Occurrences of `name` as an identifier or `@name` anywhere in `e`, nested
/// statement bodies included. (Nested statements *assigning* `name` don't
/// matter: they are identical text on both sides of the rewrite.)
fn count_ident(e: &Expr, name: &str) -> usize {
    let mut n = 0;
    ast::for_each_expr(e, &mut |e| match &e.kind {
        ExprKind::Ident(id) | ExprKind::AtVar(id) if id == name => n += 1,
        _ => {}
    });
    n
}


/// Apply rebind edits to the source, highest offset first so earlier
/// positions stay valid.
fn apply_rebinds(chars: &[char], rebinds: &[Rebind]) -> String {
    let mut edits: Vec<(usize, usize, &str)> = Vec::with_capacity(rebinds.len() * 2);
    for r in rebinds {
        edits.push((r.at_pos, r.at_pos, "@"));
        edits.push((r.prefix_start, r.prefix_end, ""));
    }
    edits.sort_by_key(|&(start, _, _)| std::cmp::Reverse(start));
    let mut out: Vec<char> = chars.to_vec();
    for (start, end, text) in edits {
        out.splice(start..end, text.chars());
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------
// IR-equivalence gate
// ---------------------------------------------------------------------------

enum Gate {
    Equivalent,
    Different(String),
    Unavailable(String),
}

/// Compile both sources and compare the entry programs' full serialized IR —
/// term ids, blocks, registers, constants, everything except the source text
/// and the source map (whitespace edits move spans). No structural slack:
/// statement-level `f(@x)` desugars to exactly `x = f(x)`
/// ([`crate::desugar`]), so the rebind rewrite must produce an identical
/// program.
fn ir_gate(before: &str, after: &str, opts: &LintOptions) -> Gate {
    let pre = match compile_ir(before, opts) {
        Ok(v) => v,
        Err(e) => return Gate::Unavailable(e),
    };
    let post = match compile_ir(after, opts) {
        Ok(v) => v,
        Err(e) => return Gate::Different(format!("rewritten source fails to compile: {e}")),
    };
    if pre == post {
        Gate::Equivalent
    } else {
        Gate::Different("compiled IR differs".to_string())
    }
}

fn compile_ir(source: &str, opts: &LintOptions) -> Result<serde_json::Value, String> {
    let mut env = Env::new();
    for dir in &opts.include_dirs {
        env.add_module_path(dir.clone());
    }
    let pid = match &opts.origin {
        Some(path) => env.load_program_at(source, path)?,
        None => env.load_program(source)?,
    };
    let program = env
        .get_program(pid)
        .ok_or_else(|| "compiled program missing".to_string())?;
    let mut json = serde_json::to_value(program).map_err(|e| e.to_string())?;
    if let serde_json::Value::Object(map) = &mut json {
        map.remove("source");
        map.remove("source_map");
    }
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint(src: &str) -> LintOutcome {
        lint_source(src, &LintOptions::default()).expect("lint_source")
    }

    // ---- Pass 1: re-indentation ----

    #[test]
    fn reindents_fn_if_for_to_two_spaces() {
        let src = "fn f(a)\nif a > 1 then\nreturn a\nend\nfor i in [1, 2] do\nprint(i)\nend\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "fn f(a)\n  if a > 1 then\n    return a\n  end\n  for i in [1, 2] do\n    print(i)\n  end\nend\n"
        );
    }

    #[test]
    fn reindents_else_elsif_at_block_level() {
        let src = "if a then\n      x\n   elsif b then\n y\nelse\n  z\n      end\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "if a then\n  x\nelsif b then\n  y\nelse\n  z\nend\n");
    }

    #[test]
    fn reindents_match_with_do_arms_house_style() {
        // `when` at match+1, do-arm bodies one deeper, arm `end` back at `when`.
        let src = "let r = match e\nwhen Add(t) do\nitems = append(items, t)\ntrue\nend\nwhen None() -> false\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "let r = match e\n  when Add(t) do\n    items = append(items, t)\n    true\n  end\n  when None() -> false\nend\n"
        );
    }

    #[test]
    fn when_guard_if_is_not_an_opener() {
        let src = "match s\nwhen Red if t >= 5 do\nx = 1\nend\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "match s\n  when Red if t >= 5 do\n    x = 1\n  end\nend\n");
    }

    #[test]
    fn multiline_collections_indent_one_level() {
        let src = "let xs = [\n1,\n2\n]\nlet r = {\na: 1\nb: 2\n}\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "let xs = [\n  1,\n  2\n]\nlet r = {\n  a: 1\n  b: 2\n}\n");
    }

    #[test]
    fn leading_closer_run_dedents_by_run_length() {
        let src = "layout(\ncolumn([\neditor()\n])\n)\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "layout(\n  column([\n    editor()\n  ])\n)\n");
    }

    #[test]
    fn arrow_lambda_does_not_open_a_block() {
        let src = "let ys = map(xs, fn(x) -> x * 2)\nlet z = 1\n";
        assert_eq!(reindent(src).unwrap(), src);
    }

    #[test]
    fn block_lambda_opens_and_end_closes() {
        let src = "let f = fn(x)\nx * 2\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "let f = fn(x)\n  x * 2\nend\n");
    }

    #[test]
    fn enum_variants_indent() {
        let src = "enum Event\nNone()\nAdd(text)\nend\n";
        assert_eq!(reindent(src).unwrap(), "enum Event\n  None()\n  Add(text)\nend\n");
    }

    #[test]
    fn jsx_children_indent_and_close_tag_dedents() {
        let src = "let e = <div class=\"x\">\n<p>hi</p>\n<br/>\n</div>\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "let e = <div class=\"x\">\n  <p>hi</p>\n  <br/>\n</div>\n");
    }

    #[test]
    fn gt_inside_jsx_attr_brace_is_not_a_tag_end() {
        let src = "let e = <div a={x > 1}>\n<p>y</p>\n</div>\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "let e = <div a={x > 1}>\n  <p>y</p>\n</div>\n");
    }

    #[test]
    fn raw_string_interior_lines_are_untouched(){
        // Lines inside a multi-line raw string are content, not layout.
        let src = "if a then\nlet s = \"\"\"\n   keep   me\n\"\"\"\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "if a then\n  let s = \"\"\"\n   keep   me\n\"\"\"\nend\n");
    }

    #[test]
    fn comments_reindent_with_their_block() {
        let src = "fn f()\n// leading\nlet x = 1 // trailing\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "fn f()\n  // leading\n  let x = 1 // trailing\nend\n");
    }

    #[test]
    fn trims_trailing_whitespace_and_ensures_single_final_newline() {
        let src = "let x = 1   \n\n\nlet y = 2\t\n\n\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "let x = 1\n\n\nlet y = 2\n");
    }

    #[test]
    fn adds_missing_final_newline() {
        assert_eq!(reindent("let x = 1").unwrap(), "let x = 1\n");
    }

    #[test]
    fn empty_source_stays_empty() {
        assert_eq!(reindent("").unwrap(), "");
    }

    #[test]
    fn inline_if_and_string_interp_are_neutral() {
        let src = "let x = if c then 1 else 2 end\nprint(\"sum = {2 + (3)} done\")\nlet y = 1\n";
        assert_eq!(reindent(src).unwrap(), src);
    }

    // ---- Pass 2: rebind ----

    fn rebound(src: &str) -> String {
        lint(src).output
    }

    #[test]
    fn rebind_flagship_examples() {
        assert_eq!(
            rebound("let x = 1\nx = double(x)\n"),
            "let x = 1\ndouble(@x)\n"
        );
        assert_eq!(
            rebound("let nums = [1, 2, 3]\nnums = append(nums, 4)\nprint(nums)\n"),
            "let nums = [1, 2, 3]\nappend(@nums, 4)\nprint(nums)\n"
        );
    }

    #[test]
    fn rebind_keeps_trailing_comment() {
        assert_eq!(
            rebound("let n = [1]\nn = append(n, 2) // grow\n"),
            "let n = [1]\nappend(@n, 2) // grow\n"
        );
    }

    #[test]
    fn rebind_applies_inside_fn_bodies() {
        let out = rebound("fn f()\nlet a = [1]\na = append(a, 2)\na\nend\nf()\n");
        assert_eq!(out, "fn f()\n  let a = [1]\n  append(@a, 2)\n  a\nend\nf()\n");
    }

    #[test]
    fn rebind_skips_ambiguous_and_non_candidates() {
        // x used twice in the call.
        let src = "let x = 1\nx = add(x, x)\n";
        assert_eq!(rebound(src), src);
        // `let` introduces a new binding; RHS x is a different variable.
        let src = "let x = 1\nlet y = double(x)\nprint(y)\n";
        assert_eq!(rebound(src), src);
        // x also used outside the arg position.
        let src = "let x = 1\nx = add(x, x + 1)\n";
        assert_eq!(rebound(src), src);
        // Callee is x itself.
        let src = "let x = double\nx = x(1)\n";
        assert_eq!(rebound(src), src);
        // RHS is not a call.
        let src = "let x = 1\nx = x + 1\n";
        assert_eq!(rebound(src), src);
    }

    #[test]
    fn lint_is_idempotent_on_its_own_output() {
        let src = "fn f()\nlet a = [1]\na = append(a, 2)\nend\n";
        let once = rebound(src);
        assert_eq!(rebound(&once), once);
    }

    // ---- IR gate + corpus property test ----

    fn collect_ptl(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "node_modules" || n == "target") {
                    continue;
                }
                collect_ptl(&path, out);
            } else if path.extension().is_some_and(|e| e == "ptl") {
                out.push(path);
            }
        }
    }

    /// The linter-plan safeguard, as a property test over the whole repo
    /// corpus: for every program that compiles, `lint` output must compile to
    /// structurally identical IR. (Programs that parse but don't compile in
    /// isolation — e.g. import-dependent files — get formatting only, which
    /// `lint_source` guarantees by skipping unverifiable rebinds.)
    #[test]
    fn lint_preserves_ir_over_repo_corpus() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root");
        let mut files = Vec::new();
        collect_ptl(repo_root, &mut files);
        let mut checked = 0;
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else { continue };
            let opts = LintOptions { include_dirs: vec![], origin: Some(path.clone()) };
            let Ok(outcome) = lint_source(&src, &opts) else { continue };
            if compile_ir(&src, &opts).is_err() {
                continue; // formatting-only file; nothing to compare
            }
            match ir_gate(&src, &outcome.output, &opts) {
                Gate::Equivalent => {}
                Gate::Different(d) => {
                    panic!("lint changed IR for {}: {}", path.display(), d)
                }
                Gate::Unavailable(e) => {
                    panic!("IR gate unavailable for {}: {}", path.display(), e)
                }
            }
            // And linting again must be a fixed point.
            let again = lint_source(&outcome.output, &opts).expect("relint");
            assert_eq!(
                again.output,
                outcome.output,
                "lint not idempotent for {}",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 50, "expected a real corpus, checked {checked}");
    }
}
