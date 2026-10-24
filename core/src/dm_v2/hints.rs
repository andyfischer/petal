//! Reading the direct-manipulation hints `tunable` and `rigid` from the IR
//! (docs/dev/direct-manipulation-v2-plan.md §2.6,
//! docs/proposals/dm-v2-hints.md).
//!
//! At run time both builtins are the identity on their first argument. What
//! they mean is static, and this module is where it is read:
//!
//! - `tunable(x)`, `tunable(x, min, max)`, `tunable(x, min:, max:, step:)`
//!   mark the literal `x` resolves to as a preferred target, with an optional
//!   box constraint and step;
//! - `rigid(x)` marks it as never moving. The call is also a feedback
//!   barrier: discovery stops crediting what is upstream of it
//!   ([`super::params`]), and a speculative run replays its baseline answer.
//!
//! `x` resolves the way `provenance::resolve_arg` resolves an argument: a
//! literal in place (`tunable(40)`, `tunable(-5)`), or a name whose identity
//! chain ends in one (`tunable(radius)`, which also covers the bare-statement
//! form `tunable(radius, 10, 80)`).
//!
//! A `BuiltinCall` names its native by a string, so a name alone does not
//! make a hint: a host that registers its own `rigid` (a rigid-body
//! constructor) would otherwise have every call read as one. A call is a
//! hint only while its name still resolves to the id the core gave the hint
//! ([`NativeFnTable::hint_rigid`]); when a host has registered over it,
//! hints are off for that name and [`Hints::warnings`] says so.

use std::collections::{HashMap, HashSet};

use crate::constant_table::ConstantValue;
use crate::native_fn::NativeFnTable;
use crate::program::{Program, TermId, TermOp};
use crate::provenance;
use crate::source_map::SourceSpan;

/// What a hint says about one literal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hint {
    /// A preferred target, kept within `[min, max]` and to multiples of
    /// `step` (from `min`, or from 0) when given.
    Tunable { min: Option<f64>, max: Option<f64>, step: Option<f64> },
    /// Never moved; nothing upstream receives feedback through the call.
    Rigid,
}

/// Which hint a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintKind {
    Tunable,
    Rigid,
}

/// The lint name of a `tunable` whose value is computed (it marks nothing).
pub const NOT_LITERAL: &str = "dm-hint-not-literal";
/// The lint name of a `tunable` bound that is not a literal, or of `min > max`.
pub const BOUNDS: &str = "dm-hint-bounds";
/// The name of the warning that a host replaced a hint builtin.
pub const DISABLED: &str = "dm-hint-disabled";

/// Something about the hints that a tool should tell the author.
#[derive(Debug, Clone, PartialEq)]
pub struct HintWarning {
    pub rule: &'static str,
    /// The hint call, when there is one.
    pub term: Option<TermId>,
    pub span: Option<SourceSpan>,
    pub message: String,
}

/// Every hint of a program.
#[derive(Debug, Clone, Default)]
pub struct Hints {
    /// The hinted literal terms (the `Constant` term, also under a unary
    /// minus) and what the hint says. When one literal is both, `rigid` wins.
    pub marks: HashMap<TermId, Hint>,
    /// The literal terms written as a `tunable` bound: never editable
    /// (§2.5.1), since editing a range to reach a goal would defeat it.
    pub bounds: HashSet<TermId>,
    /// Every hint call, by its `BuiltinCall` term.
    pub calls: HashMap<TermId, HintKind>,
    /// The hint names a host has registered over (hints are off for them).
    pub disabled: Vec<&'static str>,
    pub warnings: Vec<HintWarning>,
}

impl Hints {
    /// The hint on literal `term`, if any.
    pub fn get(&self, term: TermId) -> Option<&Hint> {
        self.marks.get(&term)
    }

    /// Which hint the call `term` is, if it is one.
    pub fn call_kind(&self, term: TermId) -> Option<HintKind> {
        self.calls.get(&term).copied()
    }
}

/// The hint builtin `name`, while `natives` still has the core's native under
/// that name. `Err` when a host has registered over it.
fn kind_of(natives: &NativeFnTable, name: &str) -> Option<Result<HintKind, ()>> {
    let (kind, core) = match name {
        "tunable" => (HintKind::Tunable, natives.hint_tunable),
        "rigid" => (HintKind::Rigid, natives.hint_rigid),
        _ => return None,
    };
    let core = core?;
    Some(if natives.lookup_name(name) == Some(core) { Ok(kind) } else { Err(()) })
}

/// The hint call `term` is, if it is one (checked against `natives`, see the
/// module docs).
pub fn hint_call(program: &Program, natives: &NativeFnTable, term: TermId) -> Option<HintKind> {
    let t = program.terms.get(term.0 as usize)?;
    let TermOp::BuiltinCall(name) = t.op else { return None };
    let ConstantValue::String(name) = program.constants.get(name) else { return None };
    kind_of(natives, name)?.ok()
}

/// The `Constant` term of the literal `id` denotes (unwrapping a unary
/// minus), or `None`.
pub(crate) fn literal_term(program: &Program, id: TermId) -> Option<TermId> {
    provenance::literal_at(program, id)?;
    let t = program.terms.get(id.0 as usize)?;
    match t.op {
        TermOp::Constant(_) => Some(id),
        TermOp::Neg => t.inputs.first().copied(),
        _ => None,
    }
}

/// The literal `arg` resolves to: in place, or at the end of its identity
/// chain (as `provenance::resolve_arg` resolves an argument).
fn resolve_literal(program: &Program, arg: TermId) -> Option<TermId> {
    let mut cur = arg;
    for _ in 0..16 {
        if let Some(lit) = literal_term(program, cur) {
            return Some(lit);
        }
        cur = provenance::alias_target(program, cur)?;
    }
    None
}

/// Read every hint of `program`. `natives` is the table the program runs
/// against, which decides whether a call named `rigid` is the hint.
pub fn read_hints(program: &Program, natives: &NativeFnTable) -> Hints {
    let mut hints = Hints::default();
    let span = |t: TermId| program.source_map.get(t).copied();
    for term in &program.terms {
        let TermOp::BuiltinCall(name) = term.op else { continue };
        let ConstantValue::String(name) = program.constants.get(name) else { continue };
        let kind = match kind_of(natives, name) {
            None => continue,
            Some(Ok(kind)) => kind,
            Some(Err(())) => {
                let name: &'static str = if name == "rigid" { "rigid" } else { "tunable" };
                if !hints.disabled.contains(&name) {
                    hints.disabled.push(name);
                    hints.warnings.push(HintWarning {
                        rule: DISABLED,
                        term: Some(term.id),
                        span: span(term.id),
                        message: format!(
                            "`{name}` is registered by the host, so it is not read as a \
                             direct-manipulation hint"
                        ),
                    });
                }
                continue;
            }
        };
        hints.calls.insert(term.id, kind);
        let Some(&arg) = term.inputs.first() else { continue };
        let literal = resolve_literal(program, arg);
        match kind {
            HintKind::Rigid => {
                // On a computed value `rigid` is still a barrier, and marks
                // nothing: that is allowed.
                if let Some(lit) = literal {
                    hints.marks.insert(lit, Hint::Rigid);
                }
            }
            HintKind::Tunable => {
                let (min, max, step) = read_bounds(program, term, &mut hints);
                match literal {
                    Some(lit) => {
                        if !matches!(hints.marks.get(&lit), Some(Hint::Rigid)) {
                            hints.marks.insert(lit, Hint::Tunable { min, max, step });
                        }
                    }
                    None => hints.warnings.push(HintWarning {
                        rule: NOT_LITERAL,
                        term: Some(term.id),
                        span: span(term.id),
                        message: "`tunable` marks a literal or a name bound to one; its \
                                  argument is computed, so nothing is marked. Wrap the \
                                  literal itself"
                            .to_string(),
                    }),
                }
                if let (Some(lo), Some(hi)) = (min, max)
                    && lo > hi
                {
                    hints.warnings.push(HintWarning {
                        rule: BOUNDS,
                        term: Some(term.id),
                        span: span(term.id),
                        message: format!("`tunable` range is empty: min {lo} is above max {hi}"),
                    });
                }
            }
        }
    }
    hints
}

/// The `min`, `max` and `step` of the `tunable` call `term`. Each must be a
/// literal; a computed one is ignored, with a warning.
fn read_bounds(
    program: &Program,
    term: &crate::program::Term,
    hints: &mut Hints,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    const NAMES: [&str; 3] = ["min", "max", "step"];
    let mut out = [None, None, None];
    for (j, &input) in term.inputs.iter().enumerate().skip(1) {
        // Named arguments go by name, positional ones by position.
        let slot = match term.arg_names.get(j).copied().flatten() {
            Some(c) => match program.constants.get(c) {
                ConstantValue::String(n) => NAMES.iter().position(|x| x == n),
                _ => None,
            },
            None => Some(j - 1),
        };
        let Some(slot) = slot.filter(|&s| s < 3) else { continue };
        // An omitted optional argument arrives as `nil`.
        if let Some(t) = program.terms.get(input.0 as usize)
            && let TermOp::Constant(c) = t.op
            && matches!(program.constants.get(c), ConstantValue::Nil)
        {
            continue;
        }
        match provenance::literal_at(program, input) {
            Some(lit) => {
                out[slot] = Some(lit.value);
                if let Some(lt) = literal_term(program, input) {
                    hints.bounds.insert(lt);
                }
            }
            None => hints.warnings.push(HintWarning {
                rule: BOUNDS,
                term: Some(term.id),
                span: program.source_map.get(input).copied(),
                message: format!(
                    "`tunable` bound `{}` must be a literal so tools can read it without \
                     running the program",
                    NAMES[slot]
                ),
            }),
        }
    }
    (out[0], out[1], out[2])
}
