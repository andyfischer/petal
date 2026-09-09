//! `petal suggest` — propose type annotations the program already implies.
//!
//! A *suggestion channel*, deliberately separate from `petal check` (which
//! warns) and `petal lint` (which normalizes). Nothing here ever fires during
//! an ordinary compile, nothing here can fail a build, and applying a
//! suggestion is always an explicit act. See docs/dev/suggestions-plan.md.
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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ast::{Stmt, StmtKind};
use crate::classes::ClassTable;
use crate::typecheck::infer::{Evidence, FnKey, Inferences, Resolved, Slot};
use crate::types::Type;

/// What the command was asked to do.
#[derive(Default)]
pub struct SuggestOptions {
    /// Module search directories, as `-I` gives them.
    pub include_dirs: Vec<PathBuf>,
    /// Extra entry points to compile purely for their call sites. A library
    /// module compiled on its own has no callers, so its parameters have no
    /// call-site evidence; pointing at an app that uses it supplies them.
    pub from: Vec<PathBuf>,
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
    pub suggestions: Vec<Suggestion>,
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

    let (inferences, classes) = gather(source, origin, opts, &mut notes)?;
    let resolved = inferences.resolve(&classes);

    // Only the target file's own declarations. Evidence is compilation-wide by
    // design — that is how a caller in another module informs this one — but a
    // rewrite must stay inside the file the user named.
    let declared = declarations(&stmts);
    let chars: Vec<char> = source.chars().collect();
    let mut suggestions = Vec::new();
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
    Ok(SuggestOutcome {
        suggestions,
        functions: declared.len(),
        notes,
    })
}

/// Compile the target and every `--from` entry, merging what each observed.
/// The target's own compile also supplies the class table the suggestions are
/// spelled against.
fn gather(
    source: &str,
    origin: Option<&Path>,
    opts: &SuggestOptions,
    notes: &mut Vec<String>,
) -> Result<(Inferences, ClassTable), String> {
    let env = crate::suggest::make_env(&opts.include_dirs);
    let (mut inferences, classes) = env
        .compile_collecting(source, origin)
        .map_err(|e| e.to_string())?;

    for extra in &opts.from {
        match std::fs::read_to_string(extra) {
            Ok(text) => match env.compile_collecting(&text, Some(extra)) {
                // A `--from` entry contributes call sites and nothing else;
                // its class table is discarded with it.
                Ok((more, _)) => inferences.merge(more),
                Err(e) => notes.push(format!(
                    "--from {}: skipped, it does not compile ({e})",
                    extra.display()
                )),
            },
            Err(e) => notes.push(format!("--from {}: {e}", extra.display())),
        }
    }
    Ok((inferences, classes))
}

fn make_env(include_dirs: &[PathBuf]) -> crate::env::Env {
    let mut env = crate::env::Env::new();
    for dir in include_dirs {
        env.add_module_path(dir.clone());
    }
    env
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
/// Petal parameters are `name` or `name: type` — no defaults, no nesting — so
/// this only has to find the parens and split on commas. It still tracks depth
/// so a stray bracket cannot run the scan off the end of the declaration, and
/// returns `None` rather than guessing whenever the text surprises it.
fn param_list(decl: &Stmt, chars: &[char]) -> Option<ParamList> {
    let start = decl.span.start.offset as usize;
    let end = (decl.span.end.offset as usize).min(chars.len());
    if start >= end {
        return None;
    }
    let open = (start..end).find(|&i| chars[i] == '(')?;

    let mut params = Vec::new();
    let mut depth = 1usize;
    let mut item_start = open + 1;
    let mut i = open + 1;
    let close = loop {
        let c = *chars.get(i)?;
        if i >= end {
            return None;
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    break i;
                }
            }
            ',' if depth == 1 => {
                params.push((item_start, i));
                item_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    };
    // An empty list has no parameter slice at all; a non-empty one ends its
    // last parameter at the closing paren.
    if chars[item_start..close].iter().any(|c| !c.is_whitespace()) {
        params.push((item_start, close));
    }
    Some(ParamList { params, close })
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

/// Apply every suggestion to `source`, back to front so earlier offsets stay
/// valid. The result is only ever written by the caller after it re-compiles
/// clean — see `handle_suggest`.
pub fn apply(source: &str, suggestions: &[Suggestion]) -> String {
    let mut chars: Vec<char> = source.chars().collect();
    let mut ordered: Vec<&Suggestion> = suggestions.iter().collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.at));
    for s in ordered {
        let at = s.at.min(chars.len());
        let insert: Vec<char> = s.text.chars().collect();
        chars.splice(at..at, insert);
    }
    chars.into_iter().collect()
}
