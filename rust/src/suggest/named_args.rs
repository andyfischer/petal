//! The named-argument refactor: find calls that pass three or more arguments
//! by position to a callee whose parameter names are known, and propose
//! writing them as `name: value`.
//!
//! `draw_rect(0, 0, 320, 48, panel)` says nothing about which number is which;
//! `draw_rect(x: 0, y: 0, w: 320, h: 48, c: panel)` does. The rewrite is only
//! ever proposed where it is provably the same call.
//!
//! # Where the callee comes from
//!
//! From the compiled IR, not from the source text: each call expression is
//! matched to its call term, and [`CallResolver`] follows that term's callee
//! edge back to the function it must be. That is what makes "the callee
//! cannot be rebound or shadowed at this site" true by construction — the IR
//! is SSA, so a rebound name is a different term — and it is the same
//! resolution the `--apply` proof runs afterwards
//! ([`crate::ir_equiv::ir_equivalent_modulo_named_args`]).
//!
//! Resolved: a `fn` and each of its overload variants, a lambda held in a
//! `let`, a class constructor, a function of an imported module or of a
//! prelude, a method call the type checker pinned to one class, and a native
//! that declares its parameter names. Left alone: a parameter or any other
//! value that merely holds a function, a method dispatched on its receiver at
//! runtime, and a native with no declared names (the variadic ones —
//! `print`, `format`).
//!
//! # What makes a rewrite safe
//!
//! 1. The call resolves, as written, to exactly one variant.
//! 2. Rewritten, it selects the *same* variant under the name-aware overload
//!    rule (`backend::calls::resolve_overload`) — checked with the runtime's
//!    own `accepts_call`, against every variant of the set.
//! 3. Every argument fills the slot it filled before.
//! 4. The arguments stay in the order they were written, so they are
//!    evaluated in the same order.
//!
//! # Two shapes, one count
//!
//! Proving the rewrite changes nothing is not the same as proving the names
//! are *true*. An overload set may hold two variants that both take a call of
//! this many arguments — the `ui` prelude's `draw_line` takes seven as the
//! flat `(x1, y1, x2, y2, r, g, b)` and as `(x1, y1, x2, y2, c, a, width)`,
//! and the variant with exactly seven parameters sorts the two out by looking
//! at what it was handed. Writing that variant's names onto a flat call would
//! be the same program with `c: 255` in it. So a call is left alone whenever
//! the variants that accept it disagree about what its positional arguments
//! are called; when they agree (one shape declared at several lengths), the
//! names are the shape's and the call is named. The same rule holds a native
//! with several call forms.
//!
//! A single function that serves two shapes from one parameter list, with no
//! sibling declaration for the other, cannot be told apart this way: its
//! parameter names are all its declaration says, and they are what is
//! suggested.
//!
//! # Which arguments are named
//!
//! Positional arguments must precede named ones, so the choice is only where
//! the named part starts. It starts as early as it can, after:
//!
//! - **the unwritten leading argument** of a method call or a pipe
//!   (`v.scaled(…)`, `x |> f(…)`), which has no place to put a name;
//! - **placeholder parameters** — names that say nothing about the role, so
//!   naming them would add noise, not meaning ([`is_placeholder`]): a name
//!   starting with `_`, a run of adjacent parameters that only count through
//!   the alphabet from `a` (`a, b`, `a, b, c`), and a run of adjacent
//!   parameters numbered in sequence on one stem (`p1, p2`, `c0, c1`). Every
//!   argument up to the last placeholder stays positional;
//! - **the subject**, when the first parameter is the thing being operated on
//!   ([`is_subject`]: the role words and their usual short forms, a leading
//!   `r` for a rect among them): `clamp(v, lo: 0, hi: 1)`,
//!   `slice(xs, start: 1, end: 3)`, `button(r, label: "OK", style: s)`;
//! - **arguments already spelled like their parameter** — `rect(x, y, w: 10,
//!   h: 4)` rather than `rect(x: x, y: y, …)` — for as long as the run lasts.
//!
//! Arguments that are already named are left exactly as written. A call with
//! nothing left to name gets no suggestion.
//!
//! # Calls not worth naming
//!
//! A rewrite can be safe and still read worse than what it replaces. Three
//! shapes are left alone ([`is_noise`]), each because the names would repeat
//! what the call already says:
//!
//! - **a bare colour** — a call whose arguments are colour channels and
//!   nothing else, `(r, g, b)` or `(r, g, b, a)`: `clear(r: 18, g: 20, b: 28)`.
//!   Three numbers handed to such a function already read as a colour. The
//!   channels at the end of a longer call (`draw_rect(x: 0, y: 0, w: 320,
//!   h: 48, r: 20, g: 25, b: 45)`) are named with the rest;
//! - **a function literal under a one-letter name** — `reduce(xs, initial: 0,
//!   f: fn(a, b) -> a + b)`: the literal is visibly the function;
//! - **mostly echoes** — more of the names would repeat their own argument
//!   than add to it: `hash(ix: ix + 1, iy: iy, seed: seed)`.

use std::collections::HashMap;

use crate::ast::{Expr, ExprKind, ExprVisitor, Stmt, walk_expr};
use crate::backend::calls::accepts_call;
use crate::lexer::Token;
use crate::named_calls::{CallResolver, NativeSignatures, fn_slots, native_slots, select_variant};
use crate::native_fn::NativeSignature;
use crate::program::{FunctionDef, Program, TermId, TermOp, base_fn_name};
use crate::source_map::{ENTRY_FILE, SourceSpan};

/// A call needs at least this many written arguments before naming them is
/// worth a suggestion.
pub const MIN_ARGS: usize = 3;

/// First-parameter names that mark the argument as the subject of the call —
/// the value a method-style reading would put before the dot. The role words
/// the builtins use for it (`rust/src/builtins/params.rs`), plus the receiver
/// names a class method declares, and the short forms scripts write for
/// the same roles (`s`, `str`, `txt`, `v`, `val`, `xs`, `lst`, `arr`,
/// `items`). `r` is one more, with a condition — see [`is_subject`].
pub const SUBJECT_PARAMS: &[&str] = &[
    "self",
    "this",
    "value",
    "list",
    "collection",
    "string",
    "record",
    "array",
    "text",
    "rect",
    "s",
    "str",
    "txt",
    "v",
    "val",
    "xs",
    "lst",
    "arr",
    "items",
];

/// Whether the first of `params` is the subject of the call. A leading `r`
/// is a rect (`button(r, label, style)`) unless a `g` follows it, which makes
/// it the red of a colour.
pub fn is_subject(params: &[String]) -> bool {
    match params.first().map(String::as_str) {
        Some("r") => params.get(1).is_none_or(|p| p != "g"),
        Some(first) => SUBJECT_PARAMS.contains(&first),
        None => false,
    }
}

/// One text insertion: `text` goes in at character offset `at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub at: usize,
    pub text: String,
}

/// One call that could name its arguments.
#[derive(Debug, Clone)]
pub struct NamedArgs {
    /// The callee as the call spells it: `draw_rect`, `shapes.area`,
    /// `v.scaled`.
    pub callee: String,
    /// 1-based position of the call.
    pub line: usize,
    pub column: usize,
    /// The names this adds, in argument order.
    pub names: Vec<String>,
    /// One insertion per name: `"x: "` ahead of its argument.
    pub edits: Vec<Edit>,
    /// The call as written, and as it would read.
    pub before: String,
    pub after: String,
    /// What the callee was resolved to, in one line.
    pub because: String,
}

/// Whether `params[i]` is a placeholder: a name that carries no role. See
/// the module docs for the rule.
pub fn is_placeholder(params: &[String], i: usize) -> bool {
    let name = params[i].as_str();
    name.starts_with('_') || in_letter_run(params, i) || in_numbered_run(params, i)
}

/// `a, b[, c…]`: adjacent single-letter parameters counting up from `a`.
fn in_letter_run(params: &[String], i: usize) -> bool {
    let letter = |p: &String| match p.as_bytes() {
        [c] if c.is_ascii_lowercase() => Some(*c),
        _ => None,
    };
    let follows = |lo: usize, hi: usize| match (letter(&params[lo]), letter(&params[hi])) {
        (Some(a), Some(b)) => b == a + 1,
        _ => false,
    };
    if letter(&params[i]).is_none() {
        return false;
    }
    let mut start = i;
    while start > 0 && follows(start - 1, start) {
        start -= 1;
    }
    let mut end = i;
    while end + 1 < params.len() && follows(end, end + 1) {
        end += 1;
    }
    end > start && params[start] == "a"
}

/// `p1, p2` / `c0, c1`: adjacent parameters on one stem, numbered in sequence.
fn in_numbered_run(params: &[String], i: usize) -> bool {
    let split = |p: &String| -> Option<(String, u32)> {
        let digits = p.chars().rev().take_while(char::is_ascii_digit).count();
        if digits == 0 || digits == p.len() {
            return None;
        }
        let (stem, number) = p.split_at(p.len() - digits);
        Some((stem.to_string(), number.parse().ok()?))
    };
    let follows = |lo: usize, hi: usize| match (split(&params[lo]), split(&params[hi])) {
        (Some((s1, n1)), Some((s2, n2))) => s1 == s2 && n2 == n1 + 1,
        _ => false,
    };
    (i > 0 && follows(i - 1, i)) || (i + 1 < params.len() && follows(i, i + 1))
}

/// Whether writing `names` onto a call would add noise rather than meaning —
/// see "Calls not worth naming" in the module docs. `names[i]` is the label
/// argument `i` would get, `echo[i]` whether that argument is already an
/// identifier of the same spelling, `literal_fn[i]` whether it is a function
/// literal, and `all_written` whether these are all the arguments the call
/// passes (none stays positional, none was named before).
pub fn is_noise(names: &[String], echo: &[bool], literal_fn: &[bool], all_written: bool) -> bool {
    let letter = |n: &String| n.chars().count() == 1;
    let bare_colour = all_written
        && matches!(
            names.iter().map(String::as_str).collect::<Vec<_>>()[..],
            ["r", "g", "b"] | ["r", "g", "b", "a"]
        );
    let lettered_fn = names.iter().zip(literal_fn).any(|(n, f)| *f && letter(n));
    let echoes = echo.iter().filter(|e| **e).count();
    bare_colour || lettered_fn || echoes * 2 > names.len()
}

/// A parameter name that can be written as an argument label and read back
/// by every tool: a plain identifier.
fn is_label(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Every call in `stmts` (the parse of `source`) that could name its
/// arguments, in source order. `program` is the compile of that same source.
pub fn find(
    source: &str,
    stmts: &[Stmt],
    program: &Program,
    natives: NativeSignatures,
) -> Vec<NamedArgs> {
    let Some(finder) = Finder::new(source, program, natives) else {
        return Vec::new();
    };
    let mut sites = Sites(Vec::new());
    for stmt in stmts {
        sites.visit_stmt(stmt);
    }
    let mut out: Vec<NamedArgs> = sites.0.iter().filter_map(|s| finder.suggest(s)).collect();
    out.sort_by_key(|s| s.edits.first().map(|e| e.at));
    out
}

/// What the rewrite needs to know about one call expression.
struct Site {
    span: SourceSpan,
    /// Where the callee expression starts.
    callee_start: usize,
    /// `recv.name(…)` — a method call or a call through a module namespace.
    callee_is_field: bool,
    /// The callee's name, when it is a bare one.
    callee_ident: Option<String>,
    args: Vec<Arg>,
    names: Vec<Option<String>>,
}

struct Arg {
    span: SourceSpan,
    /// The argument's name, when it is a bare identifier.
    ident: Option<String>,
    /// `@x` — rewritten into an assignment before compilation; left alone.
    at_var: bool,
    /// `fn(…) -> …` written in place.
    literal_fn: bool,
}

struct Sites(Vec<Site>);

impl ExprVisitor for Sites {
    fn visit_expr(&mut self, e: &Expr) {
        if let ExprKind::Call {
            function,
            args,
            arg_names,
        } = &e.kind
        {
            self.0.push(Site {
                span: e.span,
                callee_start: function.span.start.offset as usize,
                callee_is_field: matches!(function.kind, ExprKind::FieldAccess { .. }),
                callee_ident: match &function.kind {
                    ExprKind::Ident(name) => Some(name.clone()),
                    _ => None,
                },
                args: args
                    .iter()
                    .map(|a| Arg {
                        span: a.span,
                        ident: match &a.kind {
                            ExprKind::Ident(name) => Some(name.clone()),
                            _ => None,
                        },
                        at_var: matches!(a.kind, ExprKind::AtVar(_)),
                        literal_fn: matches!(a.kind, ExprKind::Lambda { .. }),
                    })
                    .collect(),
                names: arg_names.clone(),
            });
        }
        walk_expr(self, e);
    }
}

/// What one call term resolved to: enough to choose the names and to check
/// that writing them changes nothing.
struct Target<'p> {
    /// How many leading arguments the call does not write between its
    /// parentheses: the receiver of a method call, or a piped value.
    lead: usize,
    /// The written name of each argument, as the callee sees them.
    names: Vec<Option<String>>,
    /// The parameters of the variant the call runs.
    params: Vec<String>,
    kind: TargetKind<'p>,
}

enum TargetKind<'p> {
    Function {
        variants: Vec<&'p FunctionDef>,
        chosen: usize,
    },
    Native {
        name: String,
        sigs: Vec<NativeSignature>,
        form: usize,
    },
}

impl Target<'_> {
    /// The slot each argument fills when the call is written with `names`,
    /// or `None` when it would no longer be the same call.
    fn slots(&self, names: &[Option<String>]) -> Option<Vec<usize>> {
        let names: Vec<Option<&str>> = names.iter().map(|n| n.as_deref()).collect();
        match &self.kind {
            TargetKind::Function { variants, chosen } => {
                if select_variant(variants, names.len(), &names) != Some(*chosen) {
                    return None;
                }
                fn_slots(variants[*chosen], &names)
            }
            TargetKind::Native { name, sigs, .. } => {
                if names.iter().all(Option::is_none) {
                    Some((0..names.len()).collect())
                } else {
                    native_slots(name, sigs, &names)
                }
            }
        }
    }

    /// The callee, as a signature.
    fn describe(&self) -> String {
        match &self.kind {
            TargetKind::Function { variants, chosen } => {
                let func = variants[*chosen];
                let required = func.required_params();
                let params: Vec<String> = func
                    .params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        if i < required {
                            p.clone()
                        } else {
                            format!("{p} = …")
                        }
                    })
                    .collect();
                match func.name.as_deref().map(base_fn_name) {
                    Some(name) => format!("`{name}({})`", params.join(", ")),
                    None => format!("the lambda `fn({})`", params.join(", ")),
                }
            }
            TargetKind::Native { name, sigs, form } => {
                let sig = &sigs[*form];
                let params: Vec<String> = sig
                    .params()
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        if i < sig.required() {
                            p.clone()
                        } else {
                            format!("{p}?")
                        }
                    })
                    .collect();
                format!("the builtin `{name}({})`", params.join(", "))
            }
        }
    }
}

struct Finder<'a> {
    chars: Vec<char>,
    /// The source's tokens, without newlines: `(token, start, end)`.
    tokens: Vec<(Token, usize, usize)>,
    /// End offset of each `)` → its index in `tokens`.
    close_at: HashMap<usize, usize>,
    /// `(start, end)` of each call expression in the entry file → its terms.
    calls_at: HashMap<(usize, usize), Vec<TermId>>,
    program: &'a Program,
    resolver: CallResolver<'a>,
    natives: NativeSignatures<'a>,
}

impl<'a> Finder<'a> {
    fn new(source: &str, program: &'a Program, natives: NativeSignatures<'a>) -> Option<Self> {
        let mut lexer = crate::lexer::Lexer::new(source);
        lexer.tokenize().ok()?;
        let tokens: Vec<(Token, usize, usize)> = lexer
            .tokens_with_spans()
            .filter(|(t, _)| !matches!(t, Token::Newline | Token::Eof))
            .map(|(t, s)| (t.clone(), s.start.offset as usize, s.end.offset as usize))
            .collect();
        let close_at = tokens
            .iter()
            .enumerate()
            .filter(|(_, (t, ..))| matches!(t, Token::RParen))
            .map(|(i, (_, _, end))| (*end, i))
            .collect();
        let mut calls_at: HashMap<(usize, usize), Vec<TermId>> = HashMap::new();
        for term in &program.terms {
            if !matches!(
                term.op,
                TermOp::Call | TermOp::BuiltinCall(_) | TermOp::MethodCall { .. }
            ) {
                continue;
            }
            let Some(span) = program.source_map.get(term.id) else {
                continue;
            };
            if span.file == ENTRY_FILE {
                calls_at
                    .entry((span.start.offset as usize, span.end.offset as usize))
                    .or_default()
                    .push(term.id);
            }
        }
        Some(Finder {
            chars: source.chars().collect(),
            tokens,
            close_at,
            calls_at,
            program,
            resolver: CallResolver::new(program),
            natives,
        })
    }

    fn suggest(&self, site: &Site) -> Option<NamedArgs> {
        // Locate the argument list in the text. The AST says where each
        // argument's *value* is, but not where its slot starts — a
        // parenthesized argument's span is the inside of the parentheses — so
        // the slots are cut out of the token stream and every one is then
        // checked against the argument it is supposed to hold.
        let close = *self.close_at.get(&(site.span.end.offset as usize))?;
        let open = self.matching_open(close)?;
        let slots = self.split_arguments(open, close)?;
        if slots.len() < MIN_ARGS {
            return None;
        }
        if site.callee_start > self.tokens[open].1 {
            return None;
        }
        // A piped call's first argument is written ahead of the callee.
        let piped = site.args.len().checked_sub(slots.len())?;
        if piped > 1 || (piped == 1 && site.args[0].span.end.offset as usize > site.callee_start) {
            return None;
        }
        for (j, &(first, last)) in slots.iter().enumerate() {
            let arg = &site.args[piped + j];
            let (start, end) = (self.tokens[first].1, self.tokens[last].2);
            if arg.at_var
                || (arg.span.start.offset as usize) < start
                || (arg.span.end.offset as usize) > end
            {
                return None;
            }
            let labelled = last > first && matches!(self.tokens[first + 1].0, Token::Colon);
            let named = site.names.get(piped + j).is_some_and(Option::is_some);
            if labelled != named {
                return None;
            }
        }

        // One call expression is normally one call term. Where the compiler
        // emitted more, they must all tell the same story.
        let terms = self.calls_at.get(&(
            site.span.start.offset as usize,
            site.span.end.offset as usize,
        ))?;
        let mut target: Option<Target> = None;
        for &term in terms {
            let next = self.target(site, term, slots.len(), piped)?;
            if let Some(prev) = &target
                && (prev.lead != next.lead
                    || prev.names != next.names
                    || prev.params != next.params)
            {
                return None;
            }
            target = Some(next);
        }
        let target = target?;
        let Target {
            lead,
            names,
            params,
            ..
        } = &target;
        if names.len() > params.len() {
            return None;
        }

        // Where the named part starts — see the module docs.
        let positional = names.iter().take_while(|n| n.is_none()).count();
        let mut start = *lead;
        for i in *lead..positional {
            if is_placeholder(params, i) {
                start = i + 1;
            }
        }
        while start < positional {
            let arg = &site.args[piped + (start - lead)];
            let subject = start == 0 && is_subject(params);
            if !subject && arg.ident.as_deref() != Some(params[start].as_str()) {
                break;
            }
            start += 1;
        }
        if start >= positional || !params[start..positional].iter().all(|p| is_label(p)) {
            return None;
        }
        let naming = || (start..positional).map(|i| &site.args[piped + (i - lead)]);
        let echo: Vec<bool> = (start..positional)
            .zip(naming())
            .map(|(i, arg)| arg.ident.as_deref() == Some(params[i].as_str()))
            .collect();
        let literal_fn: Vec<bool> = naming().map(|arg| arg.literal_fn).collect();
        let all_written = start == *lead && positional == names.len();
        if is_noise(&params[start..positional], &echo, &literal_fn, all_written) {
            return None;
        }

        // The rewritten call must be the same call: same variant, every
        // argument in the slot it had.
        let mut renamed = names.clone();
        for (i, name) in renamed.iter_mut().enumerate().take(positional).skip(start) {
            *name = Some(params[i].clone());
        }
        let before_slots = target.slots(names)?;
        if target.slots(&renamed)? != before_slots {
            return None;
        }

        let edits: Vec<Edit> = (start..positional)
            .map(|i| Edit {
                at: self.tokens[slots[i - lead].0].1,
                text: format!("{}: ", params[i]),
            })
            .collect();
        let (from, to) = (site.callee_start, self.tokens[close].2);
        let before: String = self.chars[from..to].iter().collect();
        let after = apply_edits(
            &before,
            edits.iter().map(|e| (e.at - from, e.text.as_str())),
        );
        let callee: String = self.chars[from..self.tokens[open].1].iter().collect();
        let mut because = format!("{} is {}", quote(callee.trim()), target.describe());
        if start > *lead {
            let kept: Vec<String> = params[*lead..start]
                .iter()
                .map(|p| format!("`{p}`"))
                .collect();
            because.push_str(&format!(
                "; {} stay{} positional",
                kept.join(", "),
                if kept.len() == 1 { "s" } else { "" }
            ));
        }
        Some(NamedArgs {
            callee: callee.trim().to_string(),
            line: site.span.start.line as usize,
            column: site.span.start.column as usize,
            names: params[start..positional].to_vec(),
            edits,
            before,
            after,
            because,
        })
    }

    /// Resolve one call term of a call expression that writes `written`
    /// arguments between its parentheses, `piped` more ahead of the callee.
    fn target(&self, site: &Site, id: TermId, written: usize, piped: usize) -> Option<Target<'a>> {
        let term = self.program.get_term(id);
        let count = term.inputs.len().checked_sub(term.op.arg_offset()?)?;
        let lead = count.checked_sub(written)?;
        // The names the source writes, laid over the callee's argument list.
        let mut names: Vec<Option<String>> = vec![None; lead];
        names.extend((0..written).map(|j| site.names.get(piped + j).cloned().flatten()));
        match term.op {
            TermOp::Call => {
                // The one unwritten argument a `Call` may carry beyond a piped
                // value is the receiver of a method call the checker pinned.
                if lead != piped && !(piped == 0 && lead == 1 && site.callee_is_field) {
                    return None;
                }
                let in_ir: Vec<Option<String>> = self
                    .resolver
                    .arg_names(term)?
                    .into_iter()
                    .map(|n| n.map(str::to_string))
                    .collect();
                if in_ir != names {
                    return None;
                }
                let fns = self.resolver.resolve(*term.inputs.first()?)?;
                let variants: Vec<&FunctionDef> =
                    fns.iter().map(|f| self.resolver.function(*f)).collect();
                let as_written: Vec<Option<&str>> = names.iter().map(|n| n.as_deref()).collect();
                let chosen = select_variant(&variants, count, &as_written)?;
                // Two variants that both take this call are two readings of
                // it, and only one can lend its names — see "Two shapes, one
                // count" in the module docs.
                let positional = as_written.iter().take_while(|n| n.is_none()).count();
                let written: &[Option<&str>] = if positional == count {
                    &[]
                } else {
                    &as_written
                };
                let prefix =
                    &variants[chosen].params[..positional.min(variants[chosen].params.len())];
                if variants.iter().any(|v| {
                    accepts_call(v, count, written) && v.params.get(..prefix.len()) != Some(prefix)
                }) {
                    return None;
                }
                Some(Target {
                    lead,
                    names,
                    params: variants[chosen].params.clone(),
                    kind: TargetKind::Function { variants, chosen },
                })
            }
            TermOp::BuiltinCall(name) => {
                let name = self.program.get_string_constant(name)?;
                if lead != piped || site.callee_ident.as_deref() != Some(name) {
                    return None;
                }
                let sigs = (self.natives)(name)?;
                // A native reads its arguments by count, so the form a
                // positional call means is only knowable when every form that
                // takes this many names them alike.
                let mut fitting = (0..sigs.len()).filter(|&i| sigs[i].accepts_count(count));
                let form = fitting.next()?;
                let prefix = sigs[form].params().get(..count)?;
                if fitting.any(|i| sigs[i].params().get(..count) != Some(prefix)) {
                    return None;
                }
                Some(Target {
                    lead,
                    names,
                    params: sigs[form].params().to_vec(),
                    kind: TargetKind::Native {
                        name: name.to_string(),
                        sigs,
                        form,
                    },
                })
            }
            _ => None,
        }
    }

    /// The `(` that the `)` at token `close` closes.
    fn matching_open(&self, close: usize) -> Option<usize> {
        let mut depth = 0usize;
        for i in (0..=close).rev() {
            match self.tokens[i].0 {
                Token::RParen | Token::RBracket | Token::RBrace => depth += 1,
                Token::LParen | Token::LBracket | Token::LBrace => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.tokens[i].0, Token::LParen).then_some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Cut the tokens between a call's parentheses into one `(first, last)`
    /// token range per argument. `None` when a slot is empty — except after a
    /// trailing comma — since that is not an argument list this understands.
    fn split_arguments(&self, open: usize, close: usize) -> Option<Vec<(usize, usize)>> {
        let mut slots = Vec::new();
        let mut depth = 0usize;
        let mut first = open + 1;
        for i in open + 1..close {
            match self.tokens[i].0 {
                Token::LParen | Token::LBracket | Token::LBrace => depth += 1,
                Token::RParen | Token::RBracket | Token::RBrace => depth = depth.checked_sub(1)?,
                Token::Comma if depth == 0 => {
                    if first == i {
                        return None;
                    }
                    slots.push((first, i - 1));
                    first = i + 1;
                }
                _ => {}
            }
        }
        if first < close {
            slots.push((first, close - 1));
        }
        Some(slots)
    }
}

fn quote(s: &str) -> String {
    format!("`{s}`")
}

/// Insert each `(offset, text)` into `source`, offsets in characters and all
/// relative to the *original* text.
pub fn apply_edits<'e>(source: &str, edits: impl IntoIterator<Item = (usize, &'e str)>) -> String {
    let mut chars: Vec<char> = source.chars().collect();
    let mut ordered: Vec<(usize, &str)> = edits.into_iter().collect();
    // Back to front, so earlier offsets stay valid.
    ordered.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (at, text) in ordered {
        let at = at.min(chars.len());
        chars.splice(at..at, text.chars());
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn placeholders(names: &[&str]) -> Vec<bool> {
        let params = params(names);
        (0..params.len())
            .map(|i| is_placeholder(&params, i))
            .collect()
    }

    #[test]
    fn letter_runs_from_a_are_placeholders() {
        assert_eq!(placeholders(&["a", "b", "t"]), [true, true, false]);
        assert_eq!(placeholders(&["a", "b", "c"]), [true, true, true]);
        // `x, y` counts, but not from `a`; a lone `a` is alpha, not a
        // placeholder; `b, a` runs the wrong way.
        assert_eq!(placeholders(&["x", "y", "w", "h"]), [false; 4]);
        assert_eq!(placeholders(&["r", "g", "b", "a"]), [false; 4]);
        assert_eq!(placeholders(&["c", "a", "width"]), [false; 3]);
    }

    #[test]
    fn numbered_runs_on_one_stem_are_placeholders() {
        assert_eq!(placeholders(&["p1", "p2", "c"]), [true, true, false]);
        assert_eq!(
            placeholders(&["rect", "c0", "c1", "angle"]),
            [false, true, true, false]
        );
        // Coordinates interleave two stems: each number means something.
        assert_eq!(placeholders(&["x1", "y1", "x2", "y2"]), [false; 4]);
        assert_eq!(placeholders(&["edge0", "edge1", "x"]), [true, true, false]);
    }

    #[test]
    fn noise_is_bare_colours_lettered_fn_literals_and_echoes() {
        let noise = |names: &[&str], echo: &[bool], literal_fn: &[bool], all: bool| {
            is_noise(&params(names), echo, literal_fn, all)
        };
        let no = [false; 8];
        // `clear(r, g, b)`: colour channels and nothing else.
        assert!(noise(&["r", "g", "b"], &no[..3], &no[..3], true));
        assert!(noise(&["r", "g", "b", "a"], &no[..4], &no[..4], true));
        // Other letters, and channels that are only part of the call, are
        // worth their names.
        assert!(!noise(&["x", "y", "w", "h"], &no[..4], &no[..4], true));
        assert!(!noise(&["h", "s", "v"], &no[..3], &no[..3], true));
        assert!(!noise(&["r", "g", "b"], &no[..3], &no[..3], false));
        // `f: fn(…)` — but `on_click: fn(…)` says something.
        assert!(noise(&["initial", "f"], &no[..2], &[false, true], false));
        assert!(!noise(
            &["label", "on_click"],
            &no[..2],
            &[false, true],
            false
        ));
        // Echoes: a majority is noise, half is not.
        assert!(noise(
            &["ix", "iy", "seed"],
            &[false, true, true],
            &no[..3],
            true
        ));
        assert!(!noise(&["i", "s"], &[false, true], &no[..2], false));
    }

    #[test]
    fn a_leading_r_is_a_subject_unless_it_is_red() {
        assert!(is_subject(&params(&["r", "label", "style"])));
        assert!(is_subject(&params(&["s", "x", "y"])));
        assert!(!is_subject(&params(&["r", "g", "b"])));
        assert!(!is_subject(&params(&["x", "y", "r"])));
    }

    #[test]
    fn underscore_names_are_placeholders() {
        assert_eq!(placeholders(&["_", "x", "_unused"]), [true, false, true]);
    }
}
