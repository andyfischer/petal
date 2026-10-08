//! `petal suggest` — suggest safe refactors for a file.
//!
//! A *suggestion channel*, deliberately separate from `petal check` (which
//! warns) and `petal lint` (which normalizes). Nothing here ever fires during
//! an ordinary compile, nothing here can fail a build, and applying a
//! suggestion is always an explicit act. See docs/dev/suggestions-plan.md.
//!
//! Three kinds of suggestion ([`Kinds`]). Two are a set of text insertions,
//! each behind a proof that `--apply` runs before writing it:
//!
//! - **type annotations** the program already implies (this file);
//! - **named arguments** for calls that pass three or more arguments by
//!   position ([`named_args`]).
//!
//! The third is a comment, for what can be noticed but not rewritten:
//!
//! - **advice** ([`advice`]) — a heuristic remark about a piece of code ("this
//!   looks like a hand-written sort"). No edit, so nothing to apply or prove.
//!
//! # Type annotations
//!
//! The analysis is `crate::typecheck::infer`, which records evidence while the
//! type checker walks each module. This file is the rest of the command:
//!
//! 1. compile the target (and any `--from` entry points) with collection on;
//! 2. keep only the suggestions about functions the *target file* declares —
//!    evidence is compilation-wide, but a rewrite must never touch a file the
//!    user did not name;
//! 3. locate the insertion point for each one in the source text;
//! 4. render, or splice and write.
//!
//! **Locating by text, not by span.** [`Param`](crate::ast::Param) carries no
//! span, so an insertion point is found by scanning the declaration's own
//! source. Every located edit is re-validated against the text it claims to
//! cover before it is accepted, the way `lint`'s cast rule validates its
//! spans: a scan that goes wrong costs a suggestion, never a corrupted file.
//!
//! # Which host
//!
//! A script is compiled the way `petal check` compiles it: for a host
//! ([`HostEnv`]), with that host's prelude imported implicitly. That is what
//! lets a call to `draw_rect` resolve to the `ui` prelude's overloads rather
//! than look like an unknown global.

pub mod advice;
pub mod named_args;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ast::{Stmt, StmtKind};
use crate::classes::ClassTable;
use crate::native_fn::NativeSignature;
use crate::typecheck::globals::{self, HostProfile};
use crate::typecheck::infer::{Evidence, FnKey, Inferences, Resolved, Slot};
use crate::types::Type;

pub use advice::Advice;
pub use named_args::NamedArgs;

/// Which kinds of suggestion to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kinds {
    /// Type annotations the program already implies.
    pub types: bool,
    /// Named arguments for calls of three or more positional ones.
    pub named_args: bool,
    /// Heuristic comments that carry no rewrite.
    pub advice: bool,
}

impl Default for Kinds {
    fn default() -> Self {
        Kinds {
            types: true,
            named_args: true,
            advice: true,
        }
    }
}

impl Kinds {
    /// The `--only` spelling of each kind, with the aliases it accepts.
    pub const NAMES: &'static str = "'types', 'named-args' or 'advice'";

    /// Parse an `--only` list (`types,named-args`).
    pub fn parse(list: &str) -> Result<Kinds, String> {
        const NONE: Kinds = Kinds {
            types: false,
            named_args: false,
            advice: false,
        };
        let mut kinds = NONE;
        for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            match name {
                "types" | "type-annotations" | "annotations" => kinds.types = true,
                "named-args" | "named-arguments" => kinds.named_args = true,
                "advice" | "hints" => kinds.advice = true,
                other => {
                    return Err(format!(
                        "Unknown suggestion kind '{other}' (expected {})",
                        Self::NAMES
                    ));
                }
            }
        }
        if kinds == NONE {
            return Err(format!("--only needs a kind: {}", Self::NAMES));
        }
        Ok(kinds)
    }
}

/// An [`Env`](crate::env::Env) set up the way a host sets its own up, as far
/// as this process can: the module search path, and the host's implicit
/// prelude. Every compile `petal suggest` does — the analysis and the
/// `--apply` proofs alike — goes through one of these, so they agree on what
/// each name means.
pub struct HostEnv {
    pub env: crate::env::Env,
    host: HostProfile,
    /// Whether the host's natives can be trusted to be what a bare call
    /// reaches: false when the host has a prelude this build cannot see, since
    /// a prelude function of the same name would have shadowed the native.
    host_natives: bool,
}

impl HostEnv {
    pub fn new(include_dirs: &[PathBuf], host: HostProfile) -> Self {
        let mut env = crate::env::Env::new();
        for dir in include_dirs {
            env.add_module_path(dir.clone());
        }
        // Garden registers its packages for every panel; see `petal check`.
        if host == HostProfile::Garden
            && let Some(libs) = globals::garden_packages_dir()
            && libs.is_dir()
            && !include_dirs.contains(&libs)
        {
            env.add_module_path(libs);
        }
        let mut host_natives = true;
        if host.uses_ui_prelude() {
            match globals::ui_prelude_source() {
                Some(src) => {
                    env.register_module(globals::UI_MODULE, src);
                    env.set_implicit_imports(&[globals::UI_MODULE]);
                }
                None => host_natives = false,
            }
        }
        HostEnv {
            env,
            host,
            host_natives,
        }
    }

    /// The parameter lists native `name` declares: the `Env`'s own when it
    /// registers one, else the host's when this process carries them (the
    /// petal-ui set). `None` for a name that is not a known native.
    pub fn native_signatures(&self, name: &str) -> Option<Vec<NativeSignature>> {
        if let Some(sigs) = self.env.native_signatures(name) {
            return Some(sigs.to_vec());
        }
        if self.host_natives && self.host.natives().any(|n| n == name) {
            return globals::host_native_signatures(name);
        }
        None
    }
}

/// What the command was asked to do.
#[derive(Default)]
pub struct SuggestOptions {
    /// Module search directories, as `-I` gives them.
    pub include_dirs: Vec<PathBuf>,
    /// Extra entry points to compile purely for their call sites. A library
    /// module compiled on its own has no callers, so its parameters have no
    /// call-site evidence; pointing at an app that uses it supplies them.
    pub from: Vec<PathBuf>,
    /// The host the script is written for, as `petal check --host` names it.
    pub host: HostProfile,
    /// Which kinds of suggestion to produce.
    pub kinds: Kinds,
}

/// One proposed annotation, resolved down to a single text insertion.
#[derive(Debug, Clone)]
pub struct Suggestion {
    /// The declared function, as `(name, arity)`.
    pub function: FnKey,
    /// Which slot: a parameter (with its name) or the return type.
    pub slot: Slot,
    /// The parameter's name, for the report. Empty for a return type.
    pub param: String,
    /// The type to write.
    pub ty: String,
    /// Byte offset in the source where `text` is inserted.
    pub at: usize,
    /// Exactly what to insert: `": num"` or `" -> float"`.
    pub text: String,
    /// 1-based line of the declaration, for the report.
    pub line: usize,
    /// Why, in one line.
    pub because: String,
    /// The evidence itself, for `--json`.
    pub evidence: Vec<EvidenceLine>,
}

/// One piece of evidence, flattened for reporting.
#[derive(Debug, Clone)]
pub struct EvidenceLine {
    pub kind: &'static str,
    pub detail: String,
    pub line: usize,
}

/// Everything one `petal suggest` run produced.
pub struct SuggestOutcome {
    /// Proposed type annotations.
    pub suggestions: Vec<Suggestion>,
    /// Calls that could name their arguments.
    pub named_args: Vec<NamedArgs>,
    /// Heuristic comments: places worth a second look, with no rewrite to
    /// offer. Never applied.
    pub advice: Vec<Advice>,
    /// How many functions the target file declares, for the summary line.
    pub functions: usize,
    /// Notes worth printing above the suggestions (a `--from` that would not
    /// compile, say) — never fatal.
    pub notes: Vec<String>,
}

/// Analyze `source` (which lives at `origin`) and return what could be
/// annotated. Compiling is the expensive part and happens once per entry
/// point.
pub fn suggest_source(
    source: &str,
    origin: Option<&Path>,
    opts: &SuggestOptions,
) -> Result<SuggestOutcome, String> {
    let (_tree, stmts) = crate::rewrite::parse_ast(source)?;
    let mut notes = Vec::new();
    let host = HostEnv::new(&opts.include_dirs, opts.host);
    let (program, mut inferences, classes) = host
        .env
        .compile_collecting_program(source, origin)
        .map_err(|e| e.to_string())?;
    let declared = declarations(&stmts);

    let mut suggestions = Vec::new();
    if opts.kinds.types {
        gather_from(&host, opts, &mut inferences, &mut notes);
        let resolved = inferences.resolve(&classes);

        // Only the target file's own declarations. Evidence is
        // compilation-wide by design — that is how a caller in another module
        // informs this one — but a rewrite must stay inside the file the user
        // named.
        let chars: Vec<char> = source.chars().collect();
        for (key, slots) in &resolved {
            let Some(decl) = declared.get(key) else {
                continue;
            };
            for r in slots {
                if let Some(s) = locate(key, decl, r, &chars, &classes) {
                    suggestions.push(s);
                }
            }
        }
        suggestions.sort_by_key(|s| s.at);
    }

    let named_args = if opts.kinds.named_args {
        named_args::find(source, &stmts, &program, &|name| {
            host.native_signatures(name)
        })
    } else {
        Vec::new()
    };
    let advice = if opts.kinds.advice {
        advice::find(&stmts)
    } else {
        Vec::new()
    };
    Ok(SuggestOutcome {
        suggestions,
        named_args,
        advice,
        functions: declared.len(),
        notes,
    })
}

/// Compile every `--from` entry and merge what each observed into the
/// target's own evidence. A `--from` entry contributes call sites and nothing
/// else; its class table is discarded with it.
fn gather_from(
    host: &HostEnv,
    opts: &SuggestOptions,
    inferences: &mut Inferences,
    notes: &mut Vec<String>,
) {
    for extra in &opts.from {
        match std::fs::read_to_string(extra) {
            Ok(text) => match host.env.compile_collecting(&text, Some(extra)) {
                Ok((more, _)) => inferences.merge(more),
                Err(e) => notes.push(format!(
                    "--from {}: skipped, it does not compile ({e})",
                    extra.display()
                )),
            },
            Err(e) => notes.push(format!("--from {}: {e}", extra.display())),
        }
    }
}

/// The target file's own `fn` declarations, by key.
fn declarations(stmts: &[Stmt]) -> BTreeMap<FnKey, &Stmt> {
    let mut out = BTreeMap::new();
    for stmt in stmts {
        if let StmtKind::FnDecl { name, params, .. } = &stmt.kind {
            out.insert((name.clone(), params.len()), stmt);
        }
    }
    out
}

/// Turn a resolved slot into a concrete insertion, or `None` when the source
/// text does not look the way the declaration says it should.
fn locate(
    key: &FnKey,
    decl: &Stmt,
    r: &Resolved,
    chars: &[char],
    classes: &ClassTable,
) -> Option<Suggestion> {
    let StmtKind::FnDecl { params, .. } = &decl.kind else {
        return None;
    };
    let ty = r.ty.display(classes).into_owned();
    let list = param_list(decl, chars)?;

    let (at, text, param) = match r.slot {
        Slot::Param(i) => {
            let name = params.get(i)?.name.clone();
            let (start, end) = *list.params.get(i)?;
            // The slot must be exactly the bare name: anything else means the
            // scan drifted, or an annotation is already there.
            let written: String = chars[start..end].iter().collect();
            if written.trim() != name {
                return None;
            }
            let insert = start + written.trim_end().len();
            (insert, format!(": {ty}"), name)
        }
        Slot::Return => {
            // Nothing may already sit between the `)` and the body.
            let mut i = list.close + 1;
            while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                i += 1;
            }
            if chars.get(i) == Some(&'-') && chars.get(i + 1) == Some(&'>') {
                return None;
            }
            (list.close + 1, format!(" -> {ty}"), String::new())
        }
    };

    Some(Suggestion {
        function: key.clone(),
        slot: r.slot,
        param,
        ty,
        at,
        text,
        line: decl.span.start.line as usize,
        because: because(&r.evidence, classes),
        evidence: r
            .evidence
            .iter()
            .map(|e| evidence_line(e, classes))
            .collect(),
    })
}

/// The character offsets of a declaration's parameter list: each parameter's
/// own slice, and the index of the closing `)`.
struct ParamList {
    params: Vec<(usize, usize)>,
    close: usize,
}

/// Scan a `fn` declaration's parameter list out of the source text.
///
/// A parameter is `name`, `name: type`, or either followed by `= default`,
/// and a default is an arbitrary expression — brackets, commas and strings
/// included — so the declaration is re-lexed rather than split on characters.
/// Each parameter's slice stops before its `=`: an annotation is inserted
/// after the name, never after the default. Returns `None` rather than
/// guessing whenever the text surprises it.
fn param_list(decl: &Stmt, chars: &[char]) -> Option<ParamList> {
    use crate::lexer::Token;

    let start = decl.span.start.offset as usize;
    let end = (decl.span.end.offset as usize).min(chars.len());
    if start >= end {
        return None;
    }
    let text: String = chars[start..end].iter().collect();
    let mut lexer = crate::lexer::Lexer::new(&text);
    lexer.tokenize().ok()?;
    let tokens: Vec<(Token, usize, usize)> = lexer
        .tokens_with_spans()
        .map(|(t, s)| {
            (
                t.clone(),
                start + s.start.offset as usize,
                start + s.end.offset as usize,
            )
        })
        .collect();
    let open = tokens
        .iter()
        .position(|(t, ..)| matches!(t, Token::LParen))?;

    let mut params = Vec::new();
    let mut depth = 1usize;
    // The slice of the parameter being read, up to (not including) its `=`.
    let mut item: Option<(usize, usize)> = None;
    let mut in_default = false;
    for (token, tok_start, tok_end) in &tokens[open + 1..] {
        match token {
            Token::LParen | Token::LBracket | Token::LBrace => depth += 1,
            Token::RParen | Token::RBracket | Token::RBrace => {
                depth -= 1;
                if depth == 0 {
                    params.extend(item.take());
                    return Some(ParamList {
                        params,
                        close: *tok_start,
                    });
                }
            }
            Token::Comma if depth == 1 => {
                params.extend(item.take());
                in_default = false;
                continue;
            }
            Token::Assign if depth == 1 => {
                in_default = true;
                continue;
            }
            Token::Newline => continue,
            _ => {}
        }
        if !in_default {
            item = Some(match item {
                None => (*tok_start, *tok_end),
                Some((first, _)) => (first, *tok_end),
            });
        }
    }
    None
}

/// One line saying why, built by summarizing rather than listing: "7 call
/// sites pass `float`" is what makes a suggestion acceptable at a glance,
/// where seven separate lines would not.
fn because(evidence: &[Evidence], classes: &ClassTable) -> String {
    let mut parts = Vec::new();

    let mut call_sites: BTreeMap<String, usize> = BTreeMap::new();
    let mut flows: BTreeMap<String, String> = BTreeMap::new();
    let mut fields: Vec<&str> = Vec::new();
    let mut returns: BTreeMap<String, usize> = BTreeMap::new();
    let mut tail = None;

    for e in evidence {
        match e {
            Evidence::CallSite { ty, .. } if *ty != Type::Any => {
                *call_sites.entry(ty.display(classes).into_owned()).or_default() += 1;
            }
            Evidence::CallSite { .. } => {}
            Evidence::FlowsInto { callee, ty, .. } => {
                flows.insert(callee.clone(), ty.display(classes).into_owned());
            }
            Evidence::FieldRead { field, .. } => {
                if !fields.contains(&field.as_str()) {
                    fields.push(field);
                }
            }
            Evidence::Tail { ty, .. } => tail = Some(ty.display(classes).into_owned()),
            Evidence::Return { ty, .. } => {
                *returns.entry(ty.display(classes).into_owned()).or_default() += 1;
            }
        }
    }

    if let Some(t) = tail {
        parts.push(format!("the body's tail expression is `{t}`"));
    }
    for (ty, n) in &returns {
        parts.push(format!("{n} `return`{} of `{ty}`", plural(*n)));
    }
    for (ty, n) in &call_sites {
        parts.push(format!(
            "{n} call site{} `{ty}`",
            if *n == 1 { " passes" } else { "s pass" }
        ));
    }
    for (callee, ty) in &flows {
        parts.push(format!("passed to `{callee}`, which declares `{ty}`"));
    }
    if !fields.is_empty() {
        let mut list: Vec<String> = fields.iter().map(|f| format!(".{f}")).collect();
        list.sort();
        // Naming the class that matches is the useful part: `record` is what
        // the reads prove, but the class is usually what the author means, and
        // only the author can say whether a plain record is also passed.
        match crate::typecheck::infer::class_matching_field_reads(evidence, classes) {
            Some(class) => parts.push(format!(
                "read with {} (all fields of `{class}` — narrow it by hand if \
                 a plain record is never passed)",
                list.join(", ")
            )),
            None => parts.push(format!("read with {}", list.join(", "))),
        }
    }
    if parts.is_empty() {
        "inferred".to_string()
    } else {
        parts.join("; ")
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn evidence_line(e: &Evidence, classes: &ClassTable) -> EvidenceLine {
    let line = e.span().start.line as usize;
    match e {
        Evidence::CallSite { ty, .. } => EvidenceLine {
            kind: "call-site",
            detail: format!("argument is `{}`", ty.display(classes)),
            line,
        },
        Evidence::FlowsInto { callee, ty, .. } => EvidenceLine {
            kind: "flows-into",
            detail: format!("passed to `{callee}`, which declares `{}`", ty.display(classes)),
            line,
        },
        Evidence::FieldRead { field, .. } => EvidenceLine {
            kind: "field-read",
            detail: format!("read with `.{field}`"),
            line,
        },
        Evidence::Tail { ty, .. } => EvidenceLine {
            kind: "tail",
            detail: format!("body tail is `{}`", ty.display(classes)),
            line,
        },
        Evidence::Return { ty, .. } => EvidenceLine {
            kind: "return",
            detail: format!("returns `{}`", ty.display(classes)),
            line,
        },
    }
}

/// Apply every type annotation to `source`. The result is only ever written
/// by the caller after it re-compiles clean — see `handle_suggest`.
pub fn apply(source: &str, suggestions: &[Suggestion]) -> String {
    apply_all(source, suggestions, &[])
}

/// Apply suggestions of both kinds to `source` in one pass. Every offset is
/// relative to the original text, and the two kinds never insert at the same
/// place: one writes inside a declaration's parameter list, the other inside
/// a call's.
pub fn apply_all(source: &str, suggestions: &[Suggestion], named: &[NamedArgs]) -> String {
    let annotations = suggestions.iter().map(|s| (s.at, s.text.as_str()));
    let names = named
        .iter()
        .flat_map(|n| n.edits.iter().map(|e| (e.at, e.text.as_str())));
    named_args::apply_edits(source, annotations.chain(names))
}

/// One source text compiled for a host: what the `--apply` proofs compare.
/// Owns its [`HostEnv`], since a `Program` is borrowed from the `Env` that
/// loaded it.
pub struct Compiled {
    host: HostEnv,
    pid: crate::program::ProgramId,
}

impl Compiled {
    /// Compile `source` the way a run for `opts.host` would. `Err` is the
    /// compile error.
    pub fn new(source: &str, origin: Option<&Path>, opts: &SuggestOptions) -> Result<Self, String> {
        let mut host = HostEnv::new(&opts.include_dirs, opts.host);
        let pid = match origin {
            Some(path) => host.env.load_program_at(source, path),
            None => host.env.load_program(source),
        }?;
        Ok(Compiled { host, pid })
    }

    pub fn program(&self) -> &crate::program::Program {
        self.host
            .env
            .get_program(self.pid)
            .expect("a loaded program stays loaded")
    }

    /// The type-checker warnings this compile produced, as messages.
    pub fn warnings(&self) -> Vec<String> {
        self.program()
            .warnings
            .iter()
            .map(|d| d.message.clone())
            .collect()
    }

    /// The warnings this compile has that `before` did not. By message, not
    /// by count: a rewrite can legitimately *remove* a warning while adding a
    /// different one, and that is still a regression.
    pub fn warnings_gained_over(&self, before: &Compiled) -> Vec<String> {
        let mut remaining = before.warnings();
        let mut gained = Vec::new();
        for w in self.warnings() {
            match remaining.iter().position(|b| *b == w) {
                Some(i) => {
                    remaining.remove(i);
                }
                None => gained.push(w),
            }
        }
        gained
    }

    /// The proof behind a named-argument rewrite: is `rewritten` the program
    /// this is — every term, block and function paired, with the one licence
    /// that two calls may write their argument names differently where both
    /// provably bind the same values to the same parameters of the same
    /// function ([`crate::ir_equiv::ir_equivalent_modulo_named_args`])?
    #[allow(clippy::result_large_err)]
    pub fn same_program_modulo_named_args(
        &self,
        rewritten: &Compiled,
    ) -> Result<(), crate::ir_equiv::IrDiff> {
        crate::ir_equiv::ir_equivalent_modulo_named_args(
            self.program(),
            rewritten.program(),
            &|name| rewritten.host.native_signatures(name),
        )
    }
}

/// [`Compiled::same_program_modulo_named_args`] from two source texts, with
/// the reason for a failure already worded for the user.
pub fn verify_named_args(
    original: &str,
    rewritten: &str,
    origin: Option<&Path>,
    opts: &SuggestOptions,
) -> Result<(), String> {
    match sources_equivalent_modulo_named_args(original, rewritten, origin, opts) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(diff)) => Err(not_the_same_program(&diff)),
        Err(e) => Err(e),
    }
}

/// One line for a named-argument rewrite the IR comparison rejected.
pub fn not_the_same_program(diff: &crate::ir_equiv::IrDiff) -> String {
    let at = diff.position().map(|p| format!(" at {p}")).unwrap_or_default();
    format!(
        "the rewritten source is not provably the same program ({}: {} differs{at})",
        diff.location, diff.what
    )
}

/// Compile both texts for `opts.host` and compare them, accepting calls that
/// differ only in how their arguments are written. Shaped like
/// [`crate::ir_equiv::sources_equivalent`]: `Ok(Err(diff))` is "compiled, not
/// equivalent"; `Err(msg)` is "one of them didn't compile".
#[allow(clippy::result_large_err)]
pub fn sources_equivalent_modulo_named_args(
    original: &str,
    rewritten: &str,
    origin: Option<&Path>,
    opts: &SuggestOptions,
) -> Result<Result<(), crate::ir_equiv::IrDiff>, String> {
    let a = Compiled::new(original, origin, opts)
        .map_err(|e| format!("original does not compile: {e}"))?;
    let b = Compiled::new(rewritten, origin, opts)
        .map_err(|e| format!("rewritten does not compile: {e}"))?;
    Ok(a.same_program_modulo_named_args(&b))
}
