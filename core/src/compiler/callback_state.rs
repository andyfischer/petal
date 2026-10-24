//! Report `state` that a higher-order builtin's callback is sure to reach.
//!
//! A `state` or `state var` declaration executed while a callback of `map`,
//! `filter`, `reduce`, `forEach`, `sort_by`, `sort` (with a comparator),
//! `any`, `all` or `find` is running is a runtime error
//! (docs/proposals/state-in-callbacks.md): the callback is a step of a
//! computation over a list, not a place in the program, so its state would
//! have no slot of its own. That error is authoritative. This pass reports,
//! with error severity, the cases provable without running anything, so
//! `petal check` fails on them, `petal run` refuses to start and the language
//! server shows them:
//!
//! - a lambda passed to a callback builtin whose body declares `state`;
//! - a named function passed to one that declares `state`, itself or through
//!   the functions it calls.
//!
//! "Declares, or calls something that declares" is a closure over the calls
//! whose callee resolves statically: a function value bound by `fn` or
//! `let`, a capture of one, a hoisted forward reference, the function's own
//! name. A callback that arrives through a parameter or a `var`, or a call
//! through one, is left to the runtime error: the pass reports only what it
//! can prove. So is anything a function runs only sometimes: `state`, a
//! call or a hand-off under an `if`, a `match`, a loop or a short-circuit, or
//! after a branch that can `return`, is not counted as reached. A function
//! that hands one of its parameters straight to a
//! callback builtin (`std`'s `count` calls `filter(xs, pred)`) makes that
//! parameter a callback slot too, so `count(xs, stateful)` is reported at the
//! user's line.
//!
//! The pass reads the compiled IR rather than the AST, so the compiler's
//! name resolution decides what a name means: a `BuiltinCall` is only ever
//! emitted for a builtin nothing shadows. `memo` is not state here: it is
//! keyed by value, not by place.
//!
//! A first-party bundled package is not exempt: it has no reason to hold such
//! code, and a callback of the user's that reaches a bundled widget's state
//! is reported at the user's callback.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::diagnostic::Diagnostic;
use crate::native_fn::NativeFnTable;
use crate::program::{FunctionId, Program, Term, TermId, TermOp, base_fn_name};
use crate::program_analysis::CellIndex;
use crate::source_map::SourceSpan;

/// The rule id the error carries.
pub(crate) const RULE: &str = "state-in-callback";

/// The builtins that run a Petal callback, and the argument that is the
/// callback (the receiver counts as argument 0 in the method form).
const CALLBACK_BUILTINS: &[(&str, usize)] = &[
    ("map", 1),
    ("filter", 1),
    ("reduce", 2),
    ("forEach", 1),
    ("sort_by", 1),
    ("sort", 1),
    ("any", 1),
    ("all", 1),
    ("find", 1),
];

/// What the compiler knows about a `state` declaration that the IR does not
/// carry: where it is written, and how.
pub(super) struct StateDecl {
    /// From `state` to the end of its initializer's first line.
    pub span: SourceSpan,
    /// `state n` or `state var n`.
    pub text: String,
}

/// The error for each callback builtin call whose callback provably reaches a
/// `state` declaration. `decls` holds every non-memo `StateInit`'s
/// declaration, by term.
pub(super) fn states_in_callbacks(
    program: &Program,
    decls: &HashMap<TermId, StateDecl>,
    natives: &NativeFnTable,
) -> Vec<Diagnostic> {
    // Nothing to reach: the common case costs one scan of the map.
    if decls.is_empty() {
        return Vec::new();
    }
    let pass = Pass::new(program, decls, natives);
    if pass.direct.iter().all(Option::is_none) {
        return Vec::new();
    }
    pass.report()
}

/// What a term statically is, as far as this pass needs to know.
#[derive(Clone, Copy, PartialEq)]
enum Val {
    /// A closure of this function (an index into `program.functions`).
    Fn(usize),
    /// Parameter `.1` of function `.0`.
    Param(usize, usize),
}

/// A call that runs one of its arguments as a builtin's callback.
struct Site {
    /// The call term.
    term: TermId,
    /// The callback argument.
    arg: TermId,
    /// What the call names: the builtin, or the function that hands the
    /// argument on to one.
    called: String,
    /// The builtin that runs it.
    builtin: &'static str,
}

struct Pass<'a> {
    p: &'a Program,
    decls: &'a HashMap<TermId, StateDecl>,
    natives: &'a NativeFnTable,
    cells: CellIndex,
    fn_index: HashMap<FunctionId, usize>,
    /// Block → the function whose body it is part of; `None` at the top level.
    owner: Vec<Option<usize>>,
    /// Function → its body block.
    body_fn: HashMap<crate::program::BlockId, usize>,
    /// Function → every `MakeClosure` of it.
    closures: Vec<Vec<TermId>>,
    /// Function → the first `state` it is sure to declare in its own body.
    direct: Vec<Option<TermId>>,
    /// Function → the functions it is sure to call, where the callee resolves.
    calls: Vec<Vec<usize>>,
    /// Function → its parameters that it is sure to hand to a callback
    /// builtin, and the builtin each reaches.
    forwards: Vec<HashMap<usize, &'static str>>,
    /// Method names the method form must not be read as a builtin under: a
    /// user method, a built-in class's method, or a record field of the name
    /// can take the call first.
    taken_method_names: HashSet<String>,
    /// Term → whether the function it is in runs it on every call (see
    /// `Pass::new`); false at the top level.
    sure: Vec<bool>,
}

impl<'a> Pass<'a> {
    fn new(
        p: &'a Program,
        decls: &'a HashMap<TermId, StateDecl>,
        natives: &'a NativeFnTable,
    ) -> Pass<'a> {
        let nfns = p.functions.len();
        let fn_index: HashMap<FunctionId, usize> = p
            .functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id, i))
            .collect();
        let body_fn = p
            .functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.body_block, i))
            .collect();

        let mut owner = vec![None; p.blocks.len()];
        for (fi, f) in p.functions.iter().enumerate() {
            let mut stack = vec![f.body_block];
            while let Some(b) = stack.pop() {
                let Some(slot) = owner.get_mut(b.0 as usize) else {
                    continue;
                };
                if slot.is_some() {
                    continue;
                }
                *slot = Some(fi);
                for &t in &p.get_block(b).terms {
                    stack.extend(p.get_term(t).child_blocks.iter().copied());
                }
            }
        }

        // The terms each function is sure to run once it is called: its
        // body's own, up to the first that can return early. A term under a
        // branch, a loop or a short-circuit runs only sometimes, so `state`
        // there, or a call from there, is left to the runtime error.
        let mut sure = vec![false; p.terms.len()];
        for f in &p.functions {
            for &t in &p.get_block(f.body_block).terms {
                sure[t.0 as usize] = true;
                let term = p.get_term(t);
                if matches!(term.op, TermOp::Return) || returns_within(p, &term.child_blocks) {
                    break;
                }
            }
        }

        let mut closures = vec![Vec::new(); nfns];
        let mut direct: Vec<Option<TermId>> = vec![None; nfns];
        for term in &p.terms {
            match term.op {
                TermOp::MakeClosure(fid) => {
                    if let Some(&fi) = fn_index.get(&fid) {
                        closures[fi].push(term.id);
                    }
                }
                TermOp::StateInit
                    if !term.is_memo
                        && sure[term.id.0 as usize]
                        && decls.contains_key(&term.id) =>
                {
                    if let Some(fi) = owner[term.block_id.0 as usize] {
                        let first = direct[fi].map(|d| decls[&d].span.start.offset);
                        if first.is_none_or(|o| decls[&term.id].span.start.offset < o) {
                            direct[fi] = Some(term.id);
                        }
                    }
                }
                _ => {}
            }
        }

        let mut taken_method_names: HashSet<String> = p
            .dispatch_targets()
            .keys()
            .filter_map(|c| p.get_string_constant(*c))
            .map(str::to_string)
            .collect();
        for i in 0..natives.count() {
            let name = natives.get_name(crate::native_fn::NativeFnId(i as u32));
            if let Some((_, method)) = name.rsplit_once('.') {
                taken_method_names.insert(method.to_string());
            }
        }
        for term in &p.terms {
            let fields: Vec<_> = match &term.op {
                TermOp::AllocMap { fields, .. } => fields.clone(),
                TermOp::AllocMapSpread { entries } => entries
                    .iter()
                    .filter_map(|e| match e {
                        crate::program::MapSpreadEntry::Named(c, _) => Some(*c),
                        crate::program::MapSpreadEntry::Spread(_) => None,
                    })
                    .collect(),
                TermOp::SetField(c) => vec![*c],
                // `r["map"] = f` writes the field as `r.map = f` does.
                TermOp::SetIndex => match term.inputs.get(1).and_then(|&k| constant_of(p, k)) {
                    Some(c) => vec![c],
                    None => continue,
                },
                _ => continue,
            };
            for c in fields {
                if let Some(s) = p.get_string_constant(c) {
                    taken_method_names.insert(s.to_string());
                }
            }
        }

        let mut pass = Pass {
            p,
            decls,
            natives,
            cells: p.cell_index(),
            fn_index,
            owner,
            body_fn,
            closures,
            direct,
            calls: vec![Vec::new(); nfns],
            forwards: vec![HashMap::new(); nfns],
            taken_method_names,
            sure,
        };
        for term in &p.terms {
            if let TermOp::Call = term.op
                && pass.sure[term.id.0 as usize]
                && let Some(fi) = pass.owner_of(term)
                && let Some(&callee) = term.inputs.first()
                && let Some(Val::Fn(g)) = pass.resolve(callee)
                && !pass.calls[fi].contains(&g)
            {
                pass.calls[fi].push(g);
            }
        }
        // Which parameters each function hands on to a callback builtin,
        // directly or through another such function: a fixpoint, since a
        // forwarding function may be declared after its caller.
        loop {
            let mut found = Vec::new();
            for term in &p.terms {
                let Some(fi) = pass.owner_of(term) else {
                    continue;
                };
                if !pass.sure[term.id.0 as usize] {
                    continue;
                }
                for site in pass.sites(term) {
                    if let Some(Val::Param(fj, param)) = pass.resolve(site.arg)
                        && fj == fi
                        && !pass.forwards[fi].contains_key(&param)
                    {
                        found.push((fi, param, site.builtin));
                    }
                }
            }
            if found.is_empty() {
                break;
            }
            for (fi, param, builtin) in found {
                pass.forwards[fi].entry(param).or_insert(builtin);
            }
        }
        pass
    }

    fn owner_of(&self, term: &Term) -> Option<usize> {
        self.owner.get(term.block_id.0 as usize).copied().flatten()
    }

    /// What `t` is, when it is a function value or a parameter that can be
    /// told without running anything.
    fn resolve(&self, mut t: TermId) -> Option<Val> {
        for _ in 0..64 {
            let term = self.p.get_term(t);
            match term.op {
                TermOp::MakeClosure(fid) => return self.fn_index.get(&fid).map(|&fi| Val::Fn(fi)),
                TermOp::Copy if term.inputs.len() == 1 => t = term.inputs[0],
                // A binding phantom: a parameter, a capture, the function's
                // own name. Anything else (a loop or pattern variable, a
                // top-level native) is not a known function.
                TermOp::Copy if term.inputs.is_empty() => {
                    let &fi = self.body_fn.get(&term.block_id)?;
                    let f = &self.p.functions[fi];
                    if f.self_ref_register == Some(term.register) {
                        return Some(Val::Fn(fi));
                    }
                    if let Some(slot) = f.capture_registers.iter().position(|r| *r == term.register)
                    {
                        if self.cells.decl_of(t).is_some() {
                            return None;
                        }
                        let [site] = self.closures[fi].as_slice() else {
                            return None;
                        };
                        t = *self.p.get_term(*site).inputs.get(slot)?;
                        continue;
                    }
                    let r = term.register.0 as usize;
                    return (r < f.params.len()).then_some(Val::Param(fi, r));
                }
                // A hoisted function read through its forward-reference
                // cell: an unnamed top-level cell written once, at the top
                // level. A user's `var` is named, and is not followed.
                TermOp::CellRead => {
                    let decl = self.cells.decl_of(*term.inputs.first()?)?;
                    let d = self.p.get_term(decl);
                    if !matches!(d.op, TermOp::CellNew)
                        || d.block_id != self.p.root_block
                        || d.name.is_some()
                    {
                        return None;
                    }
                    let [w] = self.cells.writes_of(decl) else {
                        return None;
                    };
                    let w = self.p.get_term(*w);
                    if w.block_id != self.p.root_block {
                        return None;
                    }
                    t = *w.inputs.get(1)?;
                }
                _ => return None,
            }
        }
        None
    }

    /// The callback arguments `term` hands to a builtin, if it is a call that
    /// does.
    fn sites(&self, term: &Term) -> Vec<Site> {
        let positional = term.arg_names.iter().all(Option::is_none);
        let builtin = |name: &str| CALLBACK_BUILTINS.iter().find(|(b, _)| *b == name).copied();
        match &term.op {
            TermOp::BuiltinCall(c) => {
                let Some((name, index)) = self.p.get_string_constant(*c).and_then(builtin) else {
                    return Vec::new();
                };
                match term.inputs.get(index) {
                    Some(&arg) if positional => vec![Site {
                        term: term.id,
                        arg,
                        called: name.to_string(),
                        builtin: name,
                    }],
                    _ => Vec::new(),
                }
            }
            // `xs.map(f)` falls back to the builtin with the receiver as its
            // first argument, unless a record field, a user method or a
            // built-in class's method of that name takes the call first.
            TermOp::MethodCall { name, .. } => {
                let Some(method) = self.p.get_string_constant(*name) else {
                    return Vec::new();
                };
                let Some((name, index)) = builtin(method) else {
                    return Vec::new();
                };
                if !positional
                    || self.taken_method_names.contains(name)
                    || self.natives.lookup_name(name).is_none()
                {
                    return Vec::new();
                }
                match term.inputs.get(index) {
                    Some(&arg) => vec![Site {
                        term: term.id,
                        arg,
                        called: name.to_string(),
                        builtin: name,
                    }],
                    None => Vec::new(),
                }
            }
            TermOp::Call => {
                let Some((&callee, args)) = term.inputs.split_first() else {
                    return Vec::new();
                };
                let Some(Val::Fn(g)) = self.resolve(callee) else {
                    return Vec::new();
                };
                let f = &self.p.functions[g];
                let mut out = Vec::new();
                for (&param, &builtin) in &self.forwards[g] {
                    let at = if positional {
                        Some(param)
                    } else {
                        // By name, or by position ahead of the first named
                        // argument.
                        let named = term.arg_names.iter().position(|n| {
                            n.and_then(|c| self.p.get_string_constant(c))
                                == f.params.get(param).map(String::as_str)
                        });
                        let first_named = term.arg_names.iter().position(Option::is_some);
                        named.or((first_named.is_some_and(|n| param < n)).then_some(param))
                    };
                    if let Some(&arg) = at.and_then(|i| args.get(i)) {
                        out.push(Site {
                            term: term.id,
                            arg,
                            called: display_name(f.name.as_deref()),
                            builtin,
                        });
                    }
                }
                out.sort_by_key(|s| s.arg.0);
                out
            }
            _ => Vec::new(),
        }
    }

    /// The shortest chain of calls from `f` to a function that declares
    /// `state`, `f` first and the declaring one last.
    fn path_to_state(&self, f: usize) -> Option<Vec<usize>> {
        let mut from: HashMap<usize, usize> = HashMap::new();
        let mut queue = VecDeque::from([f]);
        let mut seen = HashSet::from([f]);
        while let Some(g) = queue.pop_front() {
            if self.direct[g].is_some() {
                let mut path = vec![g];
                while let Some(&prev) = from.get(path.last().unwrap()) {
                    path.push(prev);
                }
                path.reverse();
                return Some(path);
            }
            for &h in &self.calls[g] {
                if seen.insert(h) {
                    from.insert(h, g);
                    queue.push_back(h);
                }
            }
        }
        None
    }

    fn report(&self) -> Vec<Diagnostic> {
        let mut out: Vec<Diagnostic> = Vec::new();
        for term in &self.p.terms {
            for site in self.sites(term) {
                let Some(Val::Fn(cb)) = self.resolve(site.arg) else {
                    continue;
                };
                let Some(path) = self.path_to_state(cb) else {
                    continue;
                };
                if let Some(d) = self.diagnostic(&site, &path)
                    && !out.contains(&d)
                {
                    out.push(d);
                }
            }
        }
        out
    }

    fn diagnostic(&self, site: &Site, path: &[usize]) -> Option<Diagnostic> {
        let holder = *path.last()?;
        let decl = &self.decls[&self.direct[holder]?];
        let call_span = *self.p.source_map.get(site.term)?;
        let cb = &self.p.functions[path[0]];
        let through: Vec<String> = path[1..]
            .iter()
            .map(|&g| format!("`{}`", display_name(self.p.functions[g].name.as_deref())))
            .collect();
        // Worded like the runtime error: named after the builtin that runs
        // the callback, with the user's call (into `count`, say) cited.
        let head = format!(
            "`{}` cannot be used inside a callback of `{}`",
            decl.text, site.builtin
        );
        let runs = if site.called == site.builtin {
            format!("`{}` runs", site.builtin)
        } else {
            format!("`{}` runs, through `{}`,", site.called, site.builtin)
        };
        let callback = match &cb.name {
            Some(n) => format!("`{}`", display_name(Some(n))),
            None => "the callback".to_string(),
        };
        let per = if site.builtin == "sort" {
            "comparison"
        } else {
            "element"
        };
        let reaches = if through.is_empty() {
            String::new()
        } else {
            format!(", which reaches it through {}", through.join(" → "))
        };
        if decl.span.file == call_span.file {
            return Some(Diagnostic::error_citing(
                decl.span,
                format!(
                    "{head}: {runs} {callback} on line {{line}}, once per {per}{reaches}. \
                     {HELP} [{RULE}]"
                ),
                vec![call_span],
            ));
        }
        // Declared in another file: the error goes on the callback, where the
        // reader of this file can act on it.
        let arg_span = self
            .p
            .source_map
            .get(site.arg)
            .copied()
            .unwrap_or(call_span);
        let file = self
            .p
            .source_map
            .file(decl.span.file)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        Some(Diagnostic::error(
            arg_span,
            format!(
                "{head} (declared in {file}): {runs} {callback} here, once per {per}{reaches}. \
                 {HELP} [{RULE}]"
            ),
        ))
    }
}

/// Whether a `return` is written anywhere under `blocks` (closures aside:
/// their bodies are not child blocks).
fn returns_within(p: &Program, blocks: &[crate::program::BlockId]) -> bool {
    blocks.iter().any(|&b| {
        p.get_block(b).terms.iter().any(|&t| {
            let term = p.get_term(t);
            matches!(term.op, TermOp::Return) || returns_within(p, &term.child_blocks)
        })
    })
}

/// The constant `t` is, looking through copies.
fn constant_of(p: &Program, mut t: TermId) -> Option<crate::constant_table::ConstantId> {
    for _ in 0..64 {
        let term = p.get_term(t);
        match term.op {
            TermOp::Constant(c) => return Some(c),
            TermOp::Copy if term.inputs.len() == 1 => t = term.inputs[0],
            _ => return None,
        }
    }
    None
}

/// The runtime error's help, on one line.
const HELP: &str = "State needs a fixed place in the program, and a callback has none. For a \
     value per element that persists across runs, use a loop: `for x in xs do f(x) end` (add \
     `key …` to follow items). For a value carried from one element to the next, use reduce's \
     accumulator.";

/// A function's name as its source spells it: no module qualifier, no
/// overload suffix.
fn display_name(name: Option<&str>) -> String {
    match name {
        Some(n) => {
            let n = base_fn_name(n);
            n.rsplit_once("::").map_or(n, |(_, n)| n).to_string()
        }
        None => "the callback".to_string(),
    }
}
