//! Provenance — resolving a runtime call site back to the code that wrote it.
//!
//! [`crate::observe`] answers "what is `grid` right now?"; [`crate::trace`]
//! answers "how did it get that way?". This module answers a third question a
//! *direct-manipulation* editor asks: **"which code drew this?"** — and its
//! sequel, **"which literal do I edit to move it?"**
//!
//! # Where the trace comes from
//!
//! Nowhere new. The bytecode lowerer already stamps every instruction with the
//! [`TermId`] it was lowered from (`cur_origin`), and the VM hands that to each
//! native as [`PetalCxt::origin`](crate::native_fn::PetalCxt::origin). So a
//! native that emits into a buffered-output channel — every `draw_*` in
//! `petal-ui` — can record *which call site emitted this command* for one
//! `Copy` id per emit, gated off by default
//! ([`ExecutionContext::trace_emit`](crate::execution_context::ExecutionContext::trace_emit)).
//!
//! Everything in this module is then **derived lazily from that one id**. That
//! is the design's whole point, and worth stating plainly because two other
//! designs suggest themselves and are worse:
//!
//! - *Re-run the program with tracing bindings.* Needs the second run to
//!   reproduce the first exactly — same `random()`, same clock, same input — so
//!   it is only as sound as the program is deterministic.
//! - *Register extra callbacks per native in trace mode.* Sound, but it makes
//!   the hot draw path pay dispatch for a feature almost no run uses.
//!
//! Recording an id the VM already computed costs a push, and no run that has
//! tracing off pays even that.
//!
//! # From an id to editable source
//!
//! A call's [`TermId`] resolves through [`Program::source_map`] to the span of
//! the call, and through [`Term::inputs`] to each *argument's* term — and so to
//! each argument's own span. When an argument's term is a literal constant, the
//! span is a number in the source file and the value is known, which is exactly
//! what an editor needs to drag a shape and write the movement back into the
//! code. [`CallSite::resolve`] returns both in one pass.

use crate::constant_table::ConstantValue;
use crate::program::{Program, TermId, TermOp, base_fn_name};
use crate::source_map::{FileId, SourceSpan};
use crate::static_value::StaticValue;

/// A literal number written in the source, and therefore directly editable: the
/// argument value together with how it was spelled, so a rewrite can put back
/// `12` rather than `12.0` (or vice versa) and leave the file idiomatic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Literal {
    /// The value as a float, whichever way it was written.
    pub value: f64,
    /// Whether the source wrote it as an integer. A rewrite should preserve
    /// this: silently turning `10` into `10.0` across a drag churns the diff.
    pub is_int: bool,
}

/// How directly an argument traces back to something an editor may rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// The argument *is* a literal in the call — `draw_circle(120, ...)`. The
    /// safest thing to edit: the span belongs to this call and nothing else.
    Literal,
    /// The argument is a name (or an identity copy of one) that resolves to a
    /// literal elsewhere — `let r = 30 … draw_circle(x, y, r, …)`. Editable, but
    /// the definition may feed other calls, so a rewrite changes more than the
    /// shape being dragged. An editor should say so rather than surprise anyone.
    Binding,
    /// The argument is computed (arithmetic, a call, a field read). Not
    /// rewritable by editing one number; a drag has to refuse, or solve.
    Computed,
}

/// One argument of a traced call, resolved back to source.
#[derive(Debug, Clone)]
pub struct ArgSite {
    /// 0-based position in the call's argument list (the receiver of a method
    /// call is *not* counted — see [`CallSite::resolve`]).
    pub index: usize,
    /// The term that produced this argument's value.
    pub term: TermId,
    /// Where the argument expression sits in the source, when known.
    pub span: Option<SourceSpan>,
    /// How directly it traces to something editable.
    pub kind: ArgKind,
    /// The literal it resolves to, when it resolves to a *numeric* one. Present
    /// for both [`ArgKind::Literal`] and [`ArgKind::Binding`]; the difference
    /// between them is what [`span`](Self::span) points at — the call for the
    /// first, the definition for the second. Non-numeric constants (strings,
    /// bools, `nil`) appear in [`value`](Self::value) instead — this field keeps
    /// the number-specific spelling data (`is_int`) a drag needs.
    pub literal: Option<Literal>,
    /// The constant this argument resolves to, for *any* constant type: a
    /// string, bool, or `nil` as well as a number. `Some` exactly when
    /// [`kind`](Self::kind) is [`ArgKind::Literal`] or [`ArgKind::Binding`];
    /// what a goal-based edit compares against and replaces.
    pub value: Option<StaticValue>,
    /// For [`ArgKind::Binding`], the term the literal was actually found on (the
    /// definition), whose span is what a rewrite must edit. Equal to
    /// [`term`](Self::term) for a plain literal, `None` when nothing resolved.
    pub literal_term: Option<TermId>,
}

impl ArgSite {
    /// The span a rewrite of this argument must edit: the literal's own span
    /// where one was found, else the argument expression's. `None` when neither
    /// is mapped.
    pub fn editable_span(&self, program: &Program) -> Option<SourceSpan> {
        match self.literal_term {
            Some(t) => program.source_map.get(t).copied().or(self.span),
            None => self.span,
        }
    }
}

/// A call site resolved back to source: where the call is written, what it
/// calls, and where each of its arguments came from.
#[derive(Debug, Clone)]
pub struct CallSite {
    /// The term the runtime attributed the emit to.
    pub term: TermId,
    /// The span of the whole call expression, when the program carries one.
    /// `None` for IR compiled without a source map (imported IR).
    pub span: Option<SourceSpan>,
    /// The callee's name when the op names one statically (`draw_circle` for a
    /// `BuiltinCall`, the method name for a `MethodCall`). `None` for a dynamic
    /// `Call` through a value.
    pub callee: Option<String>,
    /// One entry per argument, in call order.
    pub args: Vec<ArgSite>,
}

/// Choose which frame of an emit's call chain to attribute a value to: the
/// innermost one whose source span belongs to `file`.
///
/// This is what makes attribution mean what a person means by it. `draw_circle`
/// is typically a Petal function in a prelude that wraps a native, so the
/// innermost call site is a line of *library* code — true, and useless to
/// someone looking at their own sketch. Walking out to the first frame in the
/// file being shown lands on the line they wrote. A helper the user defined in
/// that same file still resolves to the `draw_*` call inside the helper, which
/// is also what they mean: it is their code, and it is where the shape is made.
///
/// Falls back to the chain's leaf when no frame is in `file` (an emit from
/// library code with no user frame above it, which is real and shouldn't be
/// silently dropped) and to `None` for an empty chain.
pub fn pick_frame(program: &Program, chain: &[TermId], file: FileId) -> Option<TermId> {
    chain
        .iter()
        .copied()
        .find(|t| {
            program
                .source_map
                .get(*t)
                .is_some_and(|s| s.file == file && s.start.line > 0)
        })
        .or_else(|| chain.first().copied())
}

/// How far [`CallSite::resolve`] will walk a chain of identity copies looking
/// for the literal behind a name. Chains are short in practice; the bound just
/// means a malformed or cyclic graph can't spin.
const MAX_ALIAS_HOPS: usize = 16;

impl CallSite {
    /// Resolve the call `term` against `program`.
    ///
    /// Returns `None` only when `term` is out of range for this program — which
    /// is what a *stale* id looks like, and the reason this takes the program
    /// rather than trusting the caller: an origin recorded before a live reload
    /// refers to a graph that no longer exists, and must be discarded rather
    /// than resolved against whatever now sits at that index.
    ///
    /// The receiver of a `MethodCall` and the callable of a dynamic `Call` are
    /// not arguments, so they are skipped: `args[0]` is the first thing written
    /// inside the parentheses in every case.
    pub fn resolve(program: &Program, term: TermId) -> Option<CallSite> {
        let t = program.terms.get(term.0 as usize)?;
        let (callee, skip) = match &t.op {
            TermOp::BuiltinCall(name) => (string_constant(program, *name), 0),
            TermOp::MethodCall { name, .. } => (string_constant(program, *name), 1),
            // A dynamic call through a value — which is what calling a *Petal*
            // function looks like, including every `draw_*` the `petal-ui`
            // prelude defines. The callee has no name constant, so recover it
            // from the term the callable resolves to: a name reference compiles
            // to a `Copy` of the binding, and the binding carries the name.
            TermOp::Call => (t.inputs.first().and_then(|c| callable_name(program, *c)), 1),
            // Not a call at all. Still resolvable as a site — an emitting native
            // can be reached through ops this doesn't enumerate — just with no
            // callee name and every input treated as an argument.
            _ => (None, 0),
        };

        let args = t
            .inputs
            .iter()
            .skip(skip)
            .enumerate()
            .map(|(index, &arg)| resolve_arg(program, index, arg))
            .collect();

        Some(CallSite {
            term,
            span: program.source_map.get(term).copied(),
            callee,
            args,
        })
    }
}

/// Resolve one argument term: is it a literal here, a name that resolves to one,
/// or something computed?
fn resolve_arg(program: &Program, index: usize, arg: TermId) -> ArgSite {
    let span = program.source_map.get(arg).copied();
    let mut site = ArgSite {
        index,
        term: arg,
        span,
        kind: ArgKind::Computed,
        literal: None,
        literal_term: None,
        value: None,
    };

    // A constant written right here in the call — a number, a string, a bool,
    // or `nil`.
    if let Some(value) = constant_at(program, arg) {
        site.kind = ArgKind::Literal;
        site.literal = literal_at(program, arg);
        site.literal_term = Some(arg);
        site.value = Some(value);
        return site;
    }

    // Otherwise follow identity copies (a name reference compiles to `Copy` of
    // the term that bound it) to see whether a constant sits at the end.
    let mut cur = arg;
    for _ in 0..MAX_ALIAS_HOPS {
        let Some(next) = alias_target(program, cur) else {
            break;
        };
        if let Some(value) = constant_at(program, next) {
            site.kind = ArgKind::Binding;
            site.literal = literal_at(program, next);
            // `next` is the term the constant sits on — a `Neg` when the source
            // wrote a negative, whose span covers the `-` as well as the digits,
            // so replacing that range keeps the sign.
            site.literal_term = Some(next);
            site.value = Some(value);
            return site;
        }
        cur = next;
    }

    site
}

/// The term an identity op forwards from, or `None` if this op isn't one.
/// `Phi` is deliberately *not* followed: its value depends on control flow, so
/// the literal reached through one is not the value this call saw.
///
/// The direct-manipulation hint `tunable(x, …)` is the identity on `x`, so it
/// is followed too, and a drag of `draw(tunable(40))` edits the `40`
/// (docs/proposals/dm-v2-hints.md). `rigid(x)` is not: it says the literal
/// must not move, and v1 refusing the computed argument is what that means.
pub(crate) fn alias_target(program: &Program, id: TermId) -> Option<TermId> {
    let t = program.terms.get(id.0 as usize)?;
    match t.op {
        TermOp::Copy | TermOp::StateInit => t.inputs.first().copied(),
        TermOp::BuiltinCall(name)
            if string_constant(program, name).as_deref() == Some(TUNABLE) =>
        {
            t.inputs.first().copied()
        }
        _ => None,
    }
}

/// The name of the `tunable` hint builtin.
const TUNABLE: &str = "tunable";

/// The constant `id` denotes — of any type, unwrapping a unary minus on a
/// number — or `None` when `id` is not a constant. The generic counterpart of
/// [`literal_at`], for arguments a drag doesn't cover but a goal-based edit
/// does: strings, bools, `nil`.
pub fn constant_at(program: &Program, id: TermId) -> Option<StaticValue> {
    if let Some(lit) = literal_at(program, id) {
        return Some(if lit.is_int {
            StaticValue::Int(lit.value as i64)
        } else {
            StaticValue::Float(lit.value)
        });
    }
    let t = program.terms.get(id.0 as usize)?;
    let TermOp::Constant(c) = t.op else {
        return None;
    };
    match program.constants.get(c) {
        ConstantValue::String(s) => Some(StaticValue::Str(s.clone())),
        ConstantValue::Bool(b) => Some(StaticValue::Bool(*b)),
        ConstantValue::Nil => Some(StaticValue::Nil),
        // Numbers were already covered by `literal_at` above.
        _ => None,
    }
}

/// The numeric literal `id` denotes, or `None`.
pub fn literal_at(program: &Program, id: TermId) -> Option<Literal> {
    let t = program.terms.get(id.0 as usize)?;
    match t.op {
        TermOp::Constant(c) => {
            number_constant(program, c).map(|(value, is_int)| Literal { value, is_int })
        }
        _ => None,
    }
}

/// The name a dynamic call's callable resolves to, following identity copies to
/// the binding that carries it. `None` for a genuinely anonymous callable (a
/// closure expression called in place), which has no name to report.
fn callable_name(program: &Program, id: TermId) -> Option<String> {
    let mut cur = id;
    for _ in 0..MAX_ALIAS_HOPS {
        let t = program.terms.get(cur.0 as usize)?;
        if let Some(name) = &t.name {
            // Names arrive module-qualified (`ui::draw_circle`) and overload
            // variants carry an internal `#arity`; the bare tail is what the
            // user typed and what an editor should show.
            let tail = name.rsplit("::").next().unwrap_or(name);
            return Some(base_fn_name(tail).to_string());
        }
        cur = alias_target(program, cur)?;
    }
    None
}

/// A string constant's text, if `id` names one.
fn string_constant(program: &Program, id: crate::constant_table::ConstantId) -> Option<String> {
    match program.constants.get(id) {
        ConstantValue::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// A numeric constant as `(value, was_written_as_int)`, if `id` names one.
fn number_constant(
    program: &Program,
    id: crate::constant_table::ConstantId,
) -> Option<(f64, bool)> {
    match program.constants.get(id) {
        ConstantValue::Int(n) => Some((*n as f64, true)),
        ConstantValue::Float(bits) => Some((f64::from_bits(*bits), false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;

    /// Compile `source` and return its program.
    fn compile(source: &str) -> (Env, crate::program::ProgramId) {
        let mut env = Env::new();
        let id = env.load_program(source).expect("compiles");
        (env, id)
    }

    /// The line/column a span starts at, for readable assertions.
    fn start(span: &SourceSpan) -> (u32, u32) {
        (span.start.line, span.start.column)
    }

    /// A builtin call resolves to its own name and its literal arguments, with
    /// each literal carrying the value the source wrote.
    #[test]
    fn resolves_a_builtin_call_and_its_literal_args() {
        let (env, pid) = compile("print(10, 20.5)\n");
        let program = env.get_program(pid).unwrap();

        // Find the call term by op rather than by index — term numbering is a
        // compiler detail this test has no business depending on.
        let call = program
            .terms
            .iter()
            .find(|t| matches!(t.op, TermOp::BuiltinCall(_)))
            .expect("a builtin call");

        let site = CallSite::resolve(program, call.id).expect("resolves");
        assert_eq!(site.callee.as_deref(), Some("print"));
        assert_eq!(site.args.len(), 2);

        assert_eq!(site.args[0].kind, ArgKind::Literal);
        let lit = site.args[0].literal.unwrap();
        assert_eq!(lit.value, 10.0);
        assert!(lit.is_int, "10 was written as an int");

        assert_eq!(site.args[1].kind, ArgKind::Literal);
        let lit = site.args[1].literal.unwrap();
        assert_eq!(lit.value, 20.5);
        assert!(!lit.is_int, "20.5 was written as a float");
    }

    /// An argument that names a binding resolves to the literal behind the name,
    /// and is reported as a `Binding` so an editor can warn that the definition
    /// may be shared.
    #[test]
    fn resolves_an_argument_through_a_binding() {
        let (env, pid) = compile("let r = 30\nprint(r)\n");
        let program = env.get_program(pid).unwrap();
        let call = program
            .terms
            .iter()
            .find(|t| matches!(t.op, TermOp::BuiltinCall(_)))
            .expect("a builtin call");

        let site = CallSite::resolve(program, call.id).expect("resolves");
        assert_eq!(site.args.len(), 1);
        assert_eq!(site.args[0].kind, ArgKind::Binding);
        assert_eq!(site.args[0].literal.unwrap().value, 30.0);

        // The span to edit is the definition's `30` on line 1, not the use on
        // line 2 — that distinction is the whole reason `literal_term` exists.
        let span = site.args[0].editable_span(program).expect("a span");
        assert_eq!(start(&span).0, 1);
    }

    /// A computed argument reports `Computed` and offers no literal: a drag has
    /// to refuse rather than rewrite something arbitrary.
    #[test]
    fn a_computed_argument_is_not_editable() {
        let (env, pid) = compile("let a = 1\nprint(a + 2)\n");
        let program = env.get_program(pid).unwrap();
        let call = program
            .terms
            .iter()
            .find(|t| matches!(t.op, TermOp::BuiltinCall(_)))
            .expect("a builtin call");

        let site = CallSite::resolve(program, call.id).expect("resolves");
        assert_eq!(site.args[0].kind, ArgKind::Computed);
        assert!(site.args[0].literal.is_none());
    }

    /// Non-numeric constants — strings, bools, `nil` — resolve too, through
    /// `value`, both written in place and through a binding. This is what a
    /// goal-based edit of `draw_text(x, y, "hello")` reads.
    #[test]
    fn resolves_string_and_bool_constants() {
        let (env, pid) = compile("let label = \"hi\"\nprint(\"lo\", true, label)\n");
        let program = env.get_program(pid).unwrap();
        let call = program
            .terms
            .iter()
            .find(|t| matches!(t.op, TermOp::BuiltinCall(_)))
            .expect("a builtin call");

        let site = CallSite::resolve(program, call.id).expect("resolves");
        assert_eq!(site.args[0].kind, ArgKind::Literal);
        assert_eq!(site.args[0].value, Some(StaticValue::Str("lo".into())));
        assert!(site.args[0].literal.is_none(), "not a number");

        assert_eq!(site.args[1].kind, ArgKind::Literal);
        assert_eq!(site.args[1].value, Some(StaticValue::Bool(true)));

        assert_eq!(site.args[2].kind, ArgKind::Binding);
        assert_eq!(site.args[2].value, Some(StaticValue::Str("hi".into())));
        // The span to edit is the definition's string on line 1.
        let span = site.args[2].editable_span(program).expect("a span");
        assert_eq!(start(&span).0, 1);
    }

    /// Numeric args carry both the generic `value` and the spelling-preserving
    /// `literal` — the two views must agree.
    #[test]
    fn numeric_value_and_literal_agree() {
        let (env, pid) = compile("print(10, 2.5)\n");
        let program = env.get_program(pid).unwrap();
        let call = program
            .terms
            .iter()
            .find(|t| matches!(t.op, TermOp::BuiltinCall(_)))
            .expect("a builtin call");
        let site = CallSite::resolve(program, call.id).expect("resolves");
        assert_eq!(site.args[0].value, Some(StaticValue::Int(10)));
        assert_eq!(site.args[1].value, Some(StaticValue::Float(2.5)));
    }

    /// A negative literal is one constant, sign included.
    #[test]
    fn resolves_a_negative_literal() {
        let (env, pid) = compile("print(-7)\n");
        let program = env.get_program(pid).unwrap();
        let call = program
            .terms
            .iter()
            .find(|t| matches!(t.op, TermOp::BuiltinCall(_)))
            .expect("a builtin call");

        let site = CallSite::resolve(program, call.id).expect("resolves");
        let lit = site.args[0].literal.expect("a literal");
        assert_eq!(lit.value, -7.0);
        assert_eq!(site.args[0].kind, ArgKind::Literal);
    }

    /// `tunable(x)` is the identity, so a literal under it is still the
    /// literal the call saw; `rigid(x)` is a barrier and stays computed.
    #[test]
    fn looks_through_tunable_but_not_rigid() {
        let (env, pid) = compile("let r = tunable(30, 0, 50)\nprint(tunable(4), r, rigid(5))\n");
        let program = env.get_program(pid).unwrap();
        let call = program
            .terms
            .iter()
            .find(|t| matches!(t.op, TermOp::BuiltinCall(c) if string_constant(program, c).as_deref() == Some("print")))
            .expect("the print call");
        let site = CallSite::resolve(program, call.id).expect("resolves");
        assert_eq!(site.args[0].kind, ArgKind::Binding);
        assert_eq!(site.args[0].literal.unwrap().value, 4.0);
        assert_eq!(site.args[1].kind, ArgKind::Binding);
        assert_eq!(site.args[1].literal.unwrap().value, 30.0);
        assert_eq!(site.args[2].kind, ArgKind::Computed);
    }

    /// A stale id — one recorded against a program that has since been replaced
    /// — resolves to `None` rather than to whatever now occupies that index.
    #[test]
    fn an_out_of_range_term_does_not_resolve() {
        let (env, pid) = compile("print(1)\n");
        let program = env.get_program(pid).unwrap();
        assert!(CallSite::resolve(program, TermId(u32::MAX)).is_none());
    }
}

// ── Elements and value traces ──────────────────────────────────────────
//
// The readers of an [`ElementOrigin`](crate::execution_context::ElementOrigin):
// from the `AllocElement` term that built an element to the term each prop
// came from, and from there back through plain data flow to the literal (if
// any) the value was copied from unchanged.

/// The input term of prop `prop` on the element literal `element_term` (an
/// `AllocElement`), or `None` when the term is not an element literal or the
/// literal does not write that prop.
pub fn element_prop_term(program: &Program, element_term: TermId, prop: &str) -> Option<TermId> {
    let t = program.terms.get(element_term.0 as usize)?;
    let TermOp::AllocElement { prop_keys, .. } = &t.op else {
        return None;
    };
    // A prop written twice keeps its last value, as `alloc_element` does.
    let i = prop_keys
        .iter()
        .rposition(|k| string_constant(program, *k).as_deref() == Some(prop))?;
    t.inputs.get(i).copied()
}

/// The props an element literal writes, in source order, each with its input
/// term. Empty when `element_term` is not an `AllocElement`.
pub fn element_prop_terms(program: &Program, element_term: TermId) -> Vec<(String, TermId)> {
    let Some(t) = program.terms.get(element_term.0 as usize) else {
        return Vec::new();
    };
    let TermOp::AllocElement { prop_keys, .. } = &t.op else {
        return Vec::new();
    };
    prop_keys
        .iter()
        .zip(t.inputs.iter())
        .filter_map(|(k, i)| Some((string_constant(program, *k)?, *i)))
        .collect()
}

/// The input terms of an element literal's children, in source order (a
/// `{…}` hole is one child term whatever it evaluates to). Empty when
/// `element_term` is not an `AllocElement`.
pub fn element_child_terms(program: &Program, element_term: TermId) -> &[TermId] {
    let Some(t) = program.terms.get(element_term.0 as usize) else {
        return &[];
    };
    match &t.op {
        TermOp::AllocElement { prop_keys, .. } => t.inputs.get(prop_keys.len()..).unwrap_or(&[]),
        _ => &[],
    }
}

/// The function whose body `term` is in (through nested blocks), or `None`
/// for top-level code.
pub fn enclosing_function(program: &Program, term: TermId) -> Option<crate::program::FunctionId> {
    let mut block = program.terms.get(term.0 as usize)?.block_id;
    for _ in 0..256 {
        if let Some(f) = program.functions.iter().find(|f| f.body_block == block) {
            return Some(f.id);
        }
        let parent = program.blocks.iter().find(|b| b.id == block)?.parent_term_id?;
        block = program.terms.get(parent.0 as usize)?.block_id;
    }
    None
}

/// One step a [`trace_value`] walk took.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceHop {
    /// An identity: a name reference (`Copy`) or `tunable(x)`.
    Alias { from: TermId, to: TermId },
    /// A function parameter, resolved to the argument the call in the chain
    /// passed for it.
    Param { param: TermId, call: TermId, arg: TermId },
    /// A captured variable, resolved to the value its closure captured.
    Capture { capture: TermId, closure: TermId, value: TermId },
    /// A field read of a record literal (`r.x` where `r = {x: …}`).
    Field { read: TermId, record: TermId, field: String, value: TermId },
    /// An index read of a list literal with a constant index.
    Index { read: TermId, list: TermId, index: usize, value: TermId },
}

/// Where a [`trace_value`] walk stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceEnd {
    /// A constant written in the source: a number, string, bool or `nil`.
    /// The value flows unchanged from it, so editing it edits the value.
    Constant(StaticValue),
    /// A record, list or element literal (`AllocMap`, `AllocMapSpread`,
    /// `AllocList`, `AllocElement`). The value is that literal's.
    Literal,
    /// Anything else: arithmetic, a call result, a loop variable, a `Phi`, a
    /// parameter whose call is not in the chain.
    Computed,
}

/// The result of [`trace_value`]: how a value reached the term it was read
/// at, back through plain data flow.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueTrace {
    /// The term the walk started at.
    pub start: TermId,
    /// The term it stopped at: the literal, or the first term it could not
    /// see through.
    pub end: TermId,
    /// The call chain in effect at [`end`](Self::end), innermost first (a
    /// suffix of the chain the walk was given), or `None` when a captured
    /// variable led out of every frame on the chain.
    pub end_chain: Option<Vec<TermId>>,
    /// Every step taken, in walk order.
    pub hops: Vec<TraceHop>,
    pub end_kind: TraceEnd,
}

impl ValueTrace {
    /// The value is a constant written right at the start term.
    pub fn is_direct(&self) -> bool {
        self.hops.is_empty() && matches!(self.end_kind, TraceEnd::Constant(_))
    }

    /// The constant the value flows from unchanged, if it does.
    pub fn constant(&self) -> Option<&StaticValue> {
        match &self.end_kind {
            TraceEnd::Constant(v) => Some(v),
            _ => None,
        }
    }

    /// Whether the walk passed a function parameter: the literal is at a call
    /// site, so editing it changes only what that call built.
    pub fn crosses_call(&self) -> bool {
        self.hops.iter().any(|h| matches!(h, TraceHop::Param { .. }))
    }
}

/// How many steps [`trace_value`] takes before giving up.
const MAX_TRACE_HOPS: usize = 64;

/// Follow `term`'s value back through plain data flow to where it was
/// written: through name references, `tunable(x)`, the parameters of the
/// calls in `chain` (the call site of each enclosing frame, innermost first,
/// as an [`ElementOrigin`](crate::execution_context::ElementOrigin) or an
/// [`EmitSite`](crate::execution_context::EmitSite) past its leaf records),
/// captured variables, and field and constant-index reads of record and list
/// literals.
///
/// It stops at a constant ([`TraceEnd::Constant`]: the value flows unchanged
/// from that literal), at a record, list or element literal
/// ([`TraceEnd::Literal`]), or at the first term it cannot see through
/// ([`TraceEnd::Computed`]). It does not follow a `Phi` (the value depends on
/// control flow), a loop variable, a `state` slot, or a parameter of a call
/// made by a higher-order builtin (`map`'s callback): none of those is one
/// literal's value.
///
/// Static and sound for the run the chain was recorded in: the chain says
/// which call supplied each parameter, so a helper called twice traces to two
/// different literals.
pub fn trace_value(program: &Program, term: TermId, chain: &[TermId]) -> ValueTrace {
    let mut hops = Vec::new();
    let (end, ctx) = walk_value(program, term, Some(0), chain, &mut hops, 0);
    let end_kind = match program.terms.get(end.0 as usize).map(|t| &t.op) {
        Some(TermOp::AllocMap { .. } | TermOp::AllocMapSpread { .. } | TermOp::AllocList | TermOp::AllocElement { .. }) => {
            TraceEnd::Literal
        }
        _ => match constant_at(program, end) {
            Some(v) => TraceEnd::Constant(v),
            None => TraceEnd::Computed,
        },
    };
    ValueTrace {
        start: term,
        end,
        end_chain: ctx.map(|i| chain.get(i..).unwrap_or(&[]).to_vec()),
        hops,
        end_kind,
    }
}

/// One walk of [`trace_value`]: returns the term it stopped at and the chain
/// position in effect there. `ctx = Some(i)` means `chain[i]` called the
/// function the current term is in (past the end: top-level code).
fn walk_value(
    program: &Program,
    mut cur: TermId,
    mut ctx: Option<usize>,
    chain: &[TermId],
    hops: &mut Vec<TraceHop>,
    depth: usize,
) -> (TermId, Option<usize>) {
    for _ in 0..MAX_TRACE_HOPS {
        if depth > 8 {
            break;
        }
        let Some(t) = program.terms.get(cur.0 as usize) else { break };
        match &t.op {
            TermOp::Copy if !t.inputs.is_empty() => {
                hops.push(TraceHop::Alias { from: cur, to: t.inputs[0] });
                cur = t.inputs[0];
            }
            TermOp::BuiltinCall(name)
                if string_constant(program, *name).as_deref() == Some(TUNABLE) && !t.inputs.is_empty() =>
            {
                hops.push(TraceHop::Alias { from: cur, to: t.inputs[0] });
                cur = t.inputs[0];
            }
            TermOp::Copy => {
                // A binding phantom: a parameter or a capture of the function
                // whose body it is in.
                let Some(f) = program.functions.iter().find(|f| f.body_block == t.block_id) else {
                    break;
                };
                let name = t.name.as_deref();
                if let Some(k) = name.and_then(|n| f.params.iter().position(|p| p == n)) {
                    let Some(i) = ctx else { break };
                    let Some(&call) = chain.get(i) else { break };
                    let Some(arg) = call_arg_for_param(program, call, &f.params, k) else {
                        break;
                    };
                    hops.push(TraceHop::Param { param: cur, call, arg });
                    cur = arg;
                    ctx = Some(i + 1);
                } else if let Some(s) = f
                    .capture_registers
                    .iter()
                    .position(|r| *r == t.register)
                    .or_else(|| name.and_then(|n| f.capture_names.iter().position(|c| c == n)))
                {
                    // Only a closure made at one place has one captured value.
                    let mut makers = program
                        .terms
                        .iter()
                        .filter(|m| matches!(m.op, TermOp::MakeClosure(id) if id == f.id));
                    let (Some(maker), None) = (makers.next(), makers.next()) else { break };
                    let Some(&value) = maker.inputs.get(s) else { break };
                    hops.push(TraceHop::Capture { capture: cur, closure: maker.id, value });
                    ctx = ctx.and_then(|i| context_of(program, maker.id, chain, i));
                    cur = value;
                } else {
                    break;
                }
            }
            TermOp::Branch => {
                // A parameter with a default lowers to
                // `p = if p#given then p else <default>`: with the call in the
                // chain, the branch it took is known statically.
                let Some(next) = default_param_branch(program, t, ctx, chain) else { break };
                hops.push(TraceHop::Alias { from: cur, to: next });
                cur = next;
            }
            TermOp::GetField(field) => {
                let Some(field) = string_constant(program, *field) else { break };
                let Some(&obj) = t.inputs.first() else { break };
                let mut sub = Vec::new();
                let (rec, rctx) = walk_value(program, obj, ctx, chain, &mut sub, depth + 1);
                let Some(value) = record_field(program, rec, &field) else { break };
                hops.extend(sub);
                hops.push(TraceHop::Field { read: cur, record: rec, field, value });
                cur = value;
                ctx = rctx;
            }
            TermOp::GetIndex => {
                let (Some(&obj), Some(&ix)) = (t.inputs.first(), t.inputs.get(1)) else { break };
                let Some(StaticValue::Int(index)) = constant_at(program, ix) else { break };
                let Ok(index) = usize::try_from(index) else { break };
                let mut sub = Vec::new();
                let (list, lctx) = walk_value(program, obj, ctx, chain, &mut sub, depth + 1);
                let Some(lt) = program.terms.get(list.0 as usize) else { break };
                if !matches!(lt.op, TermOp::AllocList) {
                    break;
                }
                let Some(&value) = lt.inputs.get(index) else { break };
                hops.extend(sub);
                hops.push(TraceHop::Index { read: cur, list, index, value });
                cur = value;
                ctx = lctx;
            }
            _ => break,
        }
    }
    (cur, ctx)
}

/// For the `if p#given then p else <default>` branch the compiler writes for
/// parameter `p` with a default: when the call at `chain[ctx]` passes `p`
/// (positionally or by name), the then-arm, which reads the parameter and so
/// leads on to the argument. `None` when the call omits it (the value is the
/// default), for any other branch, or when the call is not known.
fn default_param_branch(program: &Program, branch: &crate::program::Term, ctx: Option<usize>, chain: &[TermId]) -> Option<TermId> {
    let f = program.functions.iter().find(|f| f.body_block == branch.block_id)?;
    // The condition is a reference to the hidden `p#given` flag.
    let mut cond = *branch.inputs.first()?;
    for _ in 0..4 {
        let c = program.terms.get(cond.0 as usize)?;
        match (&c.op, c.inputs.first()) {
            (TermOp::Copy, Some(&next)) => cond = next,
            _ => break,
        }
    }
    let flag = program.terms.get(cond.0 as usize)?;
    if !matches!(flag.op, TermOp::Copy) || !flag.inputs.is_empty() || flag.block_id != f.body_block {
        return None;
    }
    let flag_name = flag.name.as_deref()?;
    let k = f.params.iter().position(|p| crate::ast::arg_given_name(p) == flag_name)?;
    let then_b = *branch.child_blocks.first()?;
    let then_v = program.blocks.get(then_b.0 as usize).and_then(|b| b.terms.last().copied())?;
    // The then-arm reads the parameter itself.
    let tv = program.terms.get(then_v.0 as usize)?;
    let reads_param = matches!(tv.op, TermOp::Copy)
        && tv.inputs.first().and_then(|i| program.terms.get(i.0 as usize)).is_some_and(|p| {
            matches!(p.op, TermOp::Copy) && p.inputs.is_empty() && p.name.as_deref() == Some(f.params[k].as_str())
        });
    if !reads_param {
        return None;
    }
    let call = *chain.get(ctx?)?;
    if !matches!(program.terms.get(call.0 as usize)?.op, TermOp::Call) {
        return None;
    }
    // An omitted argument leaves the default, which every call that omits
    // it shares: that stays a stop (the branch), as before.
    call_arg_for_param(program, call, &f.params, k).map(|_| then_v)
}

/// The input of record literal `rec` for field `field`, when it is plainly
/// there: a named field of an `AllocMap`, or of an `AllocMapSpread` with no
/// spread after it.
fn record_field(program: &Program, rec: TermId, field: &str) -> Option<TermId> {
    let t = program.terms.get(rec.0 as usize)?;
    match &t.op {
        TermOp::AllocMap { fields, .. } => {
            let i = fields
                .iter()
                .rposition(|k| string_constant(program, *k).as_deref() == Some(field))?;
            t.inputs.get(i).copied()
        }
        TermOp::AllocMapSpread { entries } => {
            let mut found = None;
            for e in entries {
                match e {
                    crate::program::MapSpreadEntry::Spread(_) => found = None,
                    crate::program::MapSpreadEntry::Named(k, i) => {
                        if string_constant(program, *k).as_deref() == Some(field) {
                            found = t.inputs.get(*i).copied();
                        }
                    }
                }
            }
            found
        }
        _ => None,
    }
}

/// The argument `call` passes for parameter `k` (of `params`), when `call` is
/// a direct `Call` that passes it: positionally, or by name.
fn call_arg_for_param(program: &Program, call: TermId, params: &[String], k: usize) -> Option<TermId> {
    let t = program.terms.get(call.0 as usize)?;
    if !matches!(t.op, TermOp::Call) {
        return None;
    }
    let args = t.inputs.get(1..)?;
    if t.arg_names.is_empty() {
        return args.get(k).copied();
    }
    let want = params.get(k)?;
    let mut positional = 0;
    for (arg, name) in args.iter().zip(t.arg_names.iter()) {
        match name {
            Some(c) => {
                if string_constant(program, *c).as_deref() == Some(want.as_str()) {
                    return Some(*arg);
                }
            }
            None => {
                if positional == k {
                    return Some(*arg);
                }
                positional += 1;
            }
        }
    }
    None
}

/// The chain position in effect for code at `term` (in function `G`, or
/// top level), searching outward from position `from`: the frame of `G` is
/// the one whose call site follows a call made from inside `G`.
fn context_of(program: &Program, term: TermId, chain: &[TermId], from: usize) -> Option<usize> {
    let Some(g) = enclosing_function(program, term) else {
        return Some(chain.len());
    };
    (from..chain.len())
        .find(|&j| enclosing_function(program, chain[j]) == Some(g))
        .map(|j| j + 1)
}

/// The text of source file `file` of `program`, and its display name: the
/// file table's entry, or, for a single-file program (whose table is empty),
/// the program's own source as file 0, the entry file.
pub fn file_source(program: &Program, file: FileId) -> Option<(&str, &str)> {
    if let Some(f) = program.source_map.files.get(file.0 as usize) {
        return Some((&f.source, &f.name));
    }
    (file.0 == 0 && !program.source.is_empty()).then_some((program.source.as_str(), ""))
}

/// The source text `span` covers. Span offsets count characters, so this is
/// right for a file that is not ASCII too.
pub fn span_text(program: &Program, span: &SourceSpan) -> Option<String> {
    let (source, _) = file_source(program, span.file)?;
    let (start, end) = (span.start.offset as usize, span.end.offset as usize);
    if source.is_ascii() {
        return source.get(start..end).map(str::to_string);
    }
    Some(source.chars().skip(start).take(end.saturating_sub(start)).collect())
}

/// The source text of term `term`, when it has a span.
pub fn term_text(program: &Program, term: TermId) -> Option<String> {
    span_text(program, program.source_map.get(term)?)
}
