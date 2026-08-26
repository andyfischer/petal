//! The two name-derived ids that key `state` slots, and the compile-time
//! bookkeeping that produces them.
//!
//! A slot is `(declaration id, call path)`: [`Compiler::state_key_for`] mints
//! the declaration id from the declaration's full name path, and
//! [`Compiler::call_site_for`] mints the callsite id the VM pushes onto a
//! frame's path per call. Both are derived from *names and structure*, never
//! from `TermId`s or source spans, which is what lets a hot reload match slots
//! across an edit. See docs/dev/state-call-paths.md §3.1.

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::ast::{Expr, ExprKind};
use crate::program::{StateKey, TermId, TermOp};

use super::Compiler;

/// The canonical text of a callee expression, for the callsite id
/// (`Compiler::call_site_for`): the source spelling with trivia stripped, so
/// `f`, `obj.method` and `m::f` each render as themselves regardless of how the
/// call was laid out. Structure-derived rather than span-derived, which is what
/// lets a callsite id survive edits elsewhere in the file.
///
/// Shapes with no name to render — a call through an expression
/// (`(pick())(x)`), an interpolation, a literal — collapse to `<expr>`. They
/// are still separated from each other by the ordinal, so two such callsites in
/// one function keep distinct slots; only their *stability* is weaker, and a
/// callee with no name has nothing more stable to offer.
pub(crate) fn callee_text(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Ident(name) => name.clone(),
        ExprKind::CellGet(name) => name.clone(),
        ExprKind::FieldAccess { object, field } => format!("{}.{}", callee_text(object), field),
        ExprKind::OptionalAccess(inner) => callee_text(inner),
        ExprKind::IndexAccess { object, .. } => format!("{}[]", callee_text(object)),
        _ => "<expr>".to_string(),
    }
}

/// Append the next occurrence ordinal for `base` to it — `#1`, `#2`, …, with
/// the first occurrence left bare — and bump the counter in `counts`.
///
/// The one numbering rule behind everything that separates identically-spelled
/// things by order of appearance: the two name-derived ids
/// ([`Compiler::state_key_for`] and [`Compiler::call_site_for`]) and the
/// display labels host state dumps put on callsites (`env::state_json`).
pub(crate) fn append_ordinal(counts: &mut HashMap<String, u32>, base: &mut String) {
    let next = counts.entry(base.clone()).or_insert(0);
    let ordinal = std::mem::replace(next, *next + 1);
    if ordinal > 0 {
        base.push_str(&format!("#{ordinal}"));
    }
}

impl Compiler {
    /// Compute a stable hash for a state variable name. This ensures state
    /// keys are based on name, not declaration order, so reordering state
    /// declarations doesn't break hot reload.
    pub fn hash_state_name(name: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut hasher);
        hasher.finish()
    }

    /// The *declaration id* for a `state` declaration of `name` in the current
    /// compilation context: a hash of everything that identifies the
    /// declaration site by name rather than by position, so it survives the
    /// edits hot reload has to tolerate. The parts, in order:
    ///
    /// - the module qualifier (`"ui::"`), absent in the entry file;
    /// - the enclosing function-name chain (`"draw/row/"`), empty at module
    ///   scope — this is what stops two functions that declare the same state
    ///   name from silently sharing one slot;
    /// - the variable name;
    /// - a shadow ordinal (`"#1"`, `"#2"`, … — omitted for the first), which
    ///   separates repeated declarations of one name in one function.
    ///
    /// Top-level declarations therefore hash exactly the string they always
    /// did — `"scroll"` in the entry file, `"ui::scroll"` in a module — so
    /// existing programs' persisted state survives this change untouched.
    ///
    /// The declaration id is only *half* of a slot. At runtime it is the `base`
    /// of a [`RuntimeStateKey`](crate::stack::RuntimeStateKey), whose `path` is
    /// the live chain of callsites and loop iterations that reached the
    /// declaration ([`PathPart`](crate::stack::PathPart), composed per frame by
    /// the VM out of the ids [`call_site_for`](Self::call_site_for) hands the
    /// call terms). So one declaration owns *many* slots — one per call path —
    /// and this id is the static part they all share, which is exactly why hot
    /// reload can match on it alone (`transfer_stack_state`) and treat the path
    /// as an opaque tail. A declaration at module scope, outside every loop,
    /// runs on the empty path and so owns exactly one slot for the whole
    /// program; that is the case every existing embedder inspects by name.
    ///
    /// Consequence (documented in docs/module-system.md): moving a `state`
    /// decl between files or functions, or renaming a module or an enclosing
    /// function, changes its key and drops that state on reload — the same
    /// class of event as renaming the variable.
    ///
    /// Must be called exactly once per compiled declaration: it bumps the
    /// shadow ordinal.
    pub(super) fn state_key_for(&mut self, name: &str) -> StateKey {
        let mut base = String::new();
        base.push_str(&self.lexical_scope_prefix());
        base.push_str(name);

        append_ordinal(&mut self.state_decl_ordinals, &mut base);
        StateKey(Self::hash_state_name(&base))
    }

    /// The lexical position code is being compiled at, as a string: the module
    /// qualifier (`"ui::"`, empty in the entry file) followed by each enclosing
    /// function's chain entry (`"draw/row/"`, empty at module scope). Shared by
    /// the two name-derived ids — [`state_key_for`](Self::state_key_for) and
    /// [`call_site_for`](Self::call_site_for) — so both are scoped the same way.
    fn lexical_scope_prefix(&self) -> String {
        let mut out = String::new();
        if let Some(m) = &self.current_module {
            out.push_str(m);
            out.push_str("::");
        }
        for f in &self.fn_name_chain {
            out.push_str(f);
            // Neither identifiers nor module names contain '/', so a nested
            // declaration's string can never collide with a top-level one's.
            out.push('/');
        }
        out
    }

    /// The *callsite id* for a call being compiled: a hash of the canonical
    /// callee text (`f`, `obj.method`, `m::f` — see [`callee_text`]) plus its
    /// ordinal among identically-spelled callees in the same function, all
    /// qualified by the enclosing module and function chain.
    ///
    /// The callee frame pushes this onto its path as
    /// [`PathPart::Call`](crate::stack::PathPart::Call), so each callsite of a
    /// function reaches its own `state` slots (docs/dev/state-call-paths.md
    /// §2.1). Like the declaration id it is derived from *names*, never from
    /// `TermId`s or spans, so an edit elsewhere in the file leaves it alone.
    /// Renaming the callee, or inserting an earlier call to the same callee in
    /// the same function, does change it —
    /// and drops that callsite's subtree of state on reload, the same accepted
    /// loss as renaming a state variable (docs/program-modification.md).
    ///
    /// Must be called exactly once per compiled call term: it bumps the ordinal.
    pub(super) fn call_site_for(&mut self, callee: &str) -> u64 {
        // The leading marker keeps the callsite namespace disjoint from the
        // declaration-id one: a space cannot appear in an identifier, so no
        // callsite string can ever equal a `state_key_for` string.
        let mut base = format!("call {}{}", self.lexical_scope_prefix(), callee);
        append_ordinal(&mut self.call_site_ordinals, &mut base);
        Self::hash_state_name(&base)
    }

    /// Emit a call term (`Call`/`MethodCall`/`BuiltinCall`) carrying its
    /// callsite id, derived from `callee`'s canonical text. The single place a
    /// call term is created, so no call can silently reach the runtime without
    /// a path part.
    pub(super) fn emit_call_term(
        &mut self,
        op: TermOp,
        inputs: SmallVec<[TermId; 4]>,
        callee: &str,
    ) -> TermId {
        let site = self.call_site_for(callee);
        let tid = self.emit_term(op, inputs, None);
        self.terms[tid.0 as usize].call_site = Some(site);
        tid
    }

    /// The name the function about to be compiled contributes to the
    /// declaration-id chain (see `state_key_for`): its own name if it has one,
    /// else the binding name of the `let` it is the initializer of, else an
    /// ordinal among the unnamed lambdas of the enclosing function.
    pub(super) fn push_fn_name_chain(&mut self, name: Option<&str>) {
        let pending = self.pending_lambda_name.take();
        let entry = name.map(str::to_string).or(pending).unwrap_or_else(|| {
            let count = self
                .lambda_counts
                .last_mut()
                .expect("lambda_counts always holds the module-scope entry");
            let ordinal = std::mem::replace(count, *count + 1);
            format!("<lambda {ordinal}>")
        });
        self.fn_name_chain.push(entry);
        self.lambda_counts.push(0);
    }

    pub(super) fn pop_fn_name_chain(&mut self) {
        self.fn_name_chain.pop();
        self.lambda_counts.pop();
    }

    /// Compile the initializer of `let name = value`, remembering the binding
    /// name for a lambda so it names that lambda in the declaration-id chain.
    pub(super) fn compile_bound_expr(&mut self, name: &str, value: &Expr) -> TermId {
        if matches!(value.kind, ExprKind::Lambda { .. }) {
            self.pending_lambda_name = Some(name.to_string());
        }
        let tid = self.compile_expr(value);
        self.pending_lambda_name = None;
        tid
    }
}
