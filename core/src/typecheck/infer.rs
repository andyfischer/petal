//! Evidence for `petal suggest`: what a program's own call sites and function
//! bodies say about types nobody wrote down.
//!
//! This is the *reading* half. [`Checker`](super::Checker) records evidence as
//! it walks (behind the `collect` flag, so an ordinary compile pays nothing),
//! and [`Inferences::resolve`] turns the pile into suggestions. The rewriting
//! half — rendering, and splicing an accepted suggestion into the source — is
//! `crate::suggest`.
//!
//! Everything here is a *suggestion*, never a warning: nothing it produces
//! reaches `petal check`, and a wrong guess costs a rejected proposal rather
//! than a spurious diagnostic. That still leaves a high bar, because
//! `suggest --apply` writes source: a suggestion has to be one a careful
//! author would have written by hand.
//!
//! ## What counts as evidence
//!
//! For a **parameter**, three things, all of which pin a type without
//! guessing:
//!
//! - [`Evidence::CallSite`] — a caller passed an argument this pass could
//!   type. Literals make this common in un-annotated code: `ease_flag(true,
//!   18.0)` says `bool` and `float` outright.
//! - [`Evidence::FlowsInto`] — the parameter is handed straight to a call
//!   whose corresponding parameter *is* annotated (or is a class field, which
//!   is the same thing for a constructor). This is what makes annotating a
//!   library pay compound interest: each annotation becomes evidence for its
//!   callers' parameters.
//! - [`Evidence::FieldRead`] — the parameter is read with `.name`, which
//!   proves it is **record-shaped**, and no more than that. It is tempting to
//!   go further: reading `.x`, `.y`, `.w` and `.h` picks out `Rect` uniquely,
//!   so why not conclude `Rect`? Because a plain `{x, y, w, h}` record has
//!   those fields too and is *not* assignable to `Rect`, so the annotation
//!   would warn on working code. (`petal-ui`'s `point_in(px, py, r)` is
//!   exactly this case, and it is called both ways.) Field reads therefore
//!   conclude `record`; a call site that actually passes a class instance is
//!   what narrows the answer to that class.
//!
//! For a **return type**, the body's tail expression and every explicit
//! `return`.
//!
//! ## What was deliberately rejected
//!
//! **Arithmetic.** `x * 2.0` looks like it proves `x: num`, and it does not:
//! `vec2` and `dual` are additive and multiplicative too (`vec2(1,2) + 2.0`
//! is a `vec2`), so `num` would be a wrong annotation on working code. Petal's
//! `+` is numeric-only in the sense that it rejects strings and lists, which
//! is not the same as proving `num`.
//!
//! **Truthiness.** `if p then …`, `!p` and `p && q` accept any value and
//! return one (`0 || 42` is `42`), so none of them proves `bool`.
//!
//! ## Widening: a parameter is a precondition, a return type is a promise
//!
//! The two slots want opposite defaults, so they get them.
//!
//! **Numeric evidence about a parameter always concludes `num`**, never `int`
//! or `float`. The callers this compile happens to see are not the callers
//! there are: one call passing `18.0` does not make the parameter a `float`,
//! and three passing `18` do not make it an `int`. `num` is the contract the
//! arithmetic actually has, and it is what a careful author writes.
//!
//! **A return type keeps the precise type** the body produces. `-> float` on a
//! function whose tail is a `float` is exactly true, and narrowing it to `num`
//! would throw away what the callers can rely on. Only a body that returns two
//! different numeric types widens, because then `num` is the true answer.
//!
//! Non-numeric evidence has no such ladder either way: one type, or no
//! suggestion.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::classes::ClassTable;
use crate::source_map::SourceSpan;
use crate::types::Type;

/// A function, as the checker keys them: name and arity. Petal overloads on
/// arity alone, so this names exactly one declaration.
pub type FnKey = (String, usize);

/// One observation about one slot.
#[derive(Debug, Clone)]
pub enum Evidence {
    /// A call passed an argument of this type to the slot.
    CallSite { span: SourceSpan, ty: Type },
    /// The parameter was passed on to `callee`, whose matching parameter is
    /// declared `ty`.
    FlowsInto {
        span: SourceSpan,
        callee: String,
        ty: Type,
    },
    /// The parameter was read with field `field`.
    FieldRead { span: SourceSpan, field: String },
    /// The function body's tail expression has this type.
    Tail { span: SourceSpan, ty: Type },
    /// An explicit `return` of this type.
    Return { span: SourceSpan, ty: Type },
}

impl Evidence {
    fn ty(&self) -> Option<Type> {
        match self {
            Evidence::CallSite { ty, .. }
            | Evidence::FlowsInto { ty, .. }
            | Evidence::Tail { ty, .. }
            | Evidence::Return { ty, .. } => Some(*ty),
            Evidence::FieldRead { .. } => None,
        }
    }

    pub fn span(&self) -> SourceSpan {
        match self {
            Evidence::CallSite { span, .. }
            | Evidence::FlowsInto { span, .. }
            | Evidence::FieldRead { span, .. }
            | Evidence::Tail { span, .. }
            | Evidence::Return { span, .. } => *span,
        }
    }
}

/// Everything one or more compiles observed, accumulated across every module.
///
/// Keys are `(name, arity)` with no module in them, matching the checker's own
/// signature table — so a name two modules both declare would mix their
/// evidence. [`Inferences::declared_in`] records who declared what, and
/// [`Inferences::resolve`] drops every key more than one module claims rather
/// than suggesting from a mixture.
#[derive(Debug, Default)]
pub struct Inferences {
    params: HashMap<(FnKey, usize), Vec<Evidence>>,
    returns: HashMap<FnKey, Vec<Evidence>>,
    declared_in: HashMap<FnKey, BTreeSet<String>>,
    /// Keys whose declaration already carries the annotation, per slot, so a
    /// slot the author already wrote is never suggested for.
    annotated_params: BTreeSet<(FnKey, usize)>,
    annotated_returns: BTreeSet<FnKey>,
}

impl Inferences {
    pub fn is_empty(&self) -> bool {
        self.params.is_empty() && self.returns.is_empty()
    }

    pub fn note_param(&mut self, key: FnKey, index: usize, ev: Evidence) {
        self.params.entry((key, index)).or_default().push(ev);
    }

    pub fn note_return(&mut self, key: FnKey, ev: Evidence) {
        self.returns.entry(key).or_default().push(ev);
    }

    /// Record that `module` declares `key`, and which of its slots the author
    /// already annotated. Called once per declaration seen.
    pub fn note_declaration(
        &mut self,
        key: FnKey,
        module: &str,
        annotated_params: &[bool],
        annotated_return: bool,
    ) {
        self.declared_in
            .entry(key.clone())
            .or_default()
            .insert(module.to_string());
        for (i, done) in annotated_params.iter().enumerate() {
            if *done {
                self.annotated_params.insert((key.clone(), i));
            }
        }
        if annotated_return {
            self.annotated_returns.insert(key);
        }
    }

    /// Fold another compile's observations in. `petal suggest --from app.ptl`
    /// compiles extra entry points purely for their call sites, and this is
    /// how their evidence reaches the file being suggested for.
    pub fn merge(&mut self, other: Inferences) {
        for (k, v) in other.params {
            self.params.entry(k).or_default().extend(v);
        }
        for (k, v) in other.returns {
            self.returns.entry(k).or_default().extend(v);
        }
        for (k, v) in other.declared_in {
            self.declared_in.entry(k).or_default().extend(v);
        }
        self.annotated_params.extend(other.annotated_params);
        self.annotated_returns.extend(other.annotated_returns);
    }

    /// Every slot this evidence pins down, keyed by function. Slots the author
    /// already annotated, and functions whose name is claimed by more than one
    /// module, are left out.
    pub fn resolve(&self, classes: &ClassTable) -> BTreeMap<FnKey, Vec<Resolved>> {
        let mut out: BTreeMap<FnKey, Vec<Resolved>> = BTreeMap::new();
        let unambiguous = |key: &FnKey| {
            self.declared_in
                .get(key)
                .is_none_or(|mods| mods.len() == 1)
        };

        for ((key, index), evidence) in &self.params {
            if !unambiguous(key) || self.annotated_params.contains(&(key.clone(), *index)) {
                continue;
            }
            if let Some(ty) = conclude(evidence, Slot::Param(*index), classes) {
                out.entry(key.clone()).or_default().push(Resolved {
                    slot: Slot::Param(*index),
                    ty,
                    evidence: evidence.clone(),
                });
            }
        }
        for (key, evidence) in &self.returns {
            if !unambiguous(key) || self.annotated_returns.contains(key) {
                continue;
            }
            if let Some(ty) = conclude(evidence, Slot::Return, classes) {
                out.entry(key.clone()).or_default().push(Resolved {
                    slot: Slot::Return,
                    ty,
                    evidence: evidence.clone(),
                });
            }
        }
        for slots in out.values_mut() {
            slots.sort_by_key(|r| match r.slot {
                Slot::Param(i) => (0, i),
                Slot::Return => (1, 0),
            });
        }
        out
    }
}

/// Which part of a declaration a suggestion is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Param(usize),
    Return,
}

/// One slot, the type its evidence agrees on, and the evidence itself — kept
/// so the report can say *why*, which is the whole difference between a
/// suggestion a reader can accept and one they have to re-derive.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub slot: Slot,
    pub ty: Type,
    pub evidence: Vec<Evidence>,
}

/// The type a slot's evidence agrees on, or `None` when it does not agree,
/// says nothing, or says only `any`.
///
/// Two independent tracks, because they answer differently:
///
/// - **Field reads** name a class. Exactly one class in scope must declare
///   every field read, or the reads say nothing.
/// - **Types** (call sites, flows-into, tail, return) must agree after
///   numeric widening. `Any` observations are dropped rather than counted as
///   disagreement: a caller this pass could not type is not evidence *against*
///   anything, it is simply silent.
///
/// If both tracks conclude, they must conclude the same thing.
///
/// `slot` decides how numbers are treated: see the module docs on
/// preconditions and promises.
fn conclude(evidence: &[Evidence], slot: Slot, classes: &ClassTable) -> Option<Type> {
    let from_fields = conclude_from_fields(evidence);

    let mut from_types: Option<Type> = None;
    for ty in evidence.iter().filter_map(Evidence::ty) {
        if ty == Type::Any {
            continue;
        }
        // A parameter's numeric type is `num` however consistently this
        // compile's callers happened to spell their arguments.
        let ty = match (slot, ty) {
            (Slot::Param(_), Type::Int | Type::Float) => Type::Num,
            _ => ty,
        };
        from_types = Some(match from_types {
            None => ty,
            Some(seen) => widen(seen, ty)?,
        });
    }

    match (from_fields, from_types) {
        // Field reads say `record`; a call site that passed an actual class
        // instance says which one. The class is the better answer, and it is
        // compatible with what the reads proved.
        (Some(_), Some(b)) if b.is_assignable_to(&Type::Record) => Some(b),
        // Read as a record *and* observed as something that is not one.
        (Some(_), Some(_)) => None,
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// What field reads prove: `record`, and nothing narrower. See the module
/// docs — a class is *not* implied, because a plain record with the same
/// fields is not assignable to one.
fn conclude_from_fields(evidence: &[Evidence]) -> Option<Type> {
    evidence
        .iter()
        .any(|e| matches!(e, Evidence::FieldRead { .. }))
        .then_some(Type::Record)
}

/// The one class in scope declaring every field the slot was read with, if
/// there is exactly one. Not a conclusion — [`conclude_from_fields`] explains
/// why — but worth naming in the report, because it is usually the annotation
/// the author wants and would write by hand.
pub fn class_matching_field_reads(evidence: &[Evidence], classes: &ClassTable) -> Option<String> {
    let fields: BTreeSet<&str> = evidence
        .iter()
        .filter_map(|e| match e {
            Evidence::FieldRead { field, .. } => Some(field.as_str()),
            _ => None,
        })
        .collect();
    if fields.is_empty() {
        return None;
    }
    let mut only = None;
    for (_, def) in classes.iter() {
        // Scope, not just existence: a module-private class cannot be named in
        // an annotation here, so it must not be named as a hint either.
        if classes.lookup(&def.name).is_none() {
            continue;
        }
        if fields.iter().all(|f| def.field(f).is_some()) {
            if only.is_some() {
                return None;
            }
            only = Some(def.name.clone());
        }
    }
    only
}

/// Combine two observations of the same slot. Identical types keep their type;
/// two different numeric types widen to `num`, which is the contract the
/// arithmetic actually has and the only honest answer when one caller passed
/// `18` and another `18.0`. Anything else is a disagreement, and a
/// disagreement means no suggestion.
fn widen(a: Type, b: Type) -> Option<Type> {
    if a == b {
        return Some(a);
    }
    let numeric = |t: Type| matches!(t, Type::Int | Type::Float | Type::Num);
    if numeric(a) && numeric(b) {
        return Some(Type::Num);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> SourceSpan {
        SourceSpan::default()
    }

    fn call(ty: Type) -> Evidence {
        Evidence::CallSite { span: span(), ty }
    }

    fn classes() -> ClassTable {
        ClassTable::new()
    }

    #[test]
    fn agreeing_evidence_concludes_and_disagreeing_evidence_does_not() {
        assert_eq!(
            conclude(&[call(Type::Bool), call(Type::Bool)], Slot::Return, &classes()),
            Some(Type::Bool)
        );
        assert_eq!(
            conclude(&[call(Type::Bool), call(Type::String)], Slot::Return, &classes()),
            None
        );
        assert_eq!(conclude(&[], Slot::Return, &classes()), None);
    }

    /// `any` is silence, not disagreement: a caller this pass could not type
    /// must not veto the callers it could.
    #[test]
    fn an_untypeable_call_site_is_silent_rather_than_contradictory() {
        assert_eq!(
            conclude(&[call(Type::Bool), call(Type::Any)], Slot::Return, &classes()),
            Some(Type::Bool)
        );
        assert_eq!(conclude(&[call(Type::Any)], Slot::Return, &classes()), None);
    }

    /// A return type keeps the precise type its body produces, and widens
    /// only when the body really does produce two.
    #[test]
    fn a_return_type_stays_precise_unless_the_body_disagrees_with_itself() {
        assert_eq!(
            conclude(
                &[call(Type::Int), call(Type::Float)],
                Slot::Return,
                &classes()
            ),
            Some(Type::Num)
        );
        assert_eq!(
            conclude(
                &[call(Type::Float), call(Type::Float)],
                Slot::Return,
                &classes()
            ),
            Some(Type::Float)
        );
    }

    /// A parameter is a precondition: however consistently this compile's
    /// callers spelled their arguments, the honest annotation is `num`. The
    /// callers this compile can see are not the callers there are.
    #[test]
    fn numeric_evidence_about_a_parameter_always_concludes_num() {
        for observed in [Type::Int, Type::Float] {
            assert_eq!(
                conclude(&[call(observed)], Slot::Param(0), &classes()),
                Some(Type::Num),
                "{observed:?}"
            );
        }
        // Non-numeric evidence is unaffected.
        assert_eq!(
            conclude(&[call(Type::String)], Slot::Param(0), &classes()),
            Some(Type::String)
        );
    }

    fn reads(names: &[&str]) -> Vec<Evidence> {
        names
            .iter()
            .map(|f| Evidence::FieldRead {
                span: span(),
                field: (*f).to_string(),
            })
            .collect()
    }

    /// Field reads prove record-shaped and nothing more. `Rect` is the only
    /// class declaring `.x/.y/.w/.h`, and concluding it anyway would warn on
    /// every caller that passes a plain `{x, y, w, h}` — which is what
    /// `petal-ui`'s `point_in` is actually called with.
    #[test]
    fn field_reads_conclude_record_not_the_class_that_happens_to_match() {
        let classes = classes();
        assert_eq!(
            conclude(&reads(&["x", "y", "w", "h"]), Slot::Param(0), &classes),
            Some(Type::Record)
        );
        // The class is still worth naming in the report.
        assert_eq!(
            class_matching_field_reads(&reads(&["x", "y", "w", "h"]), &classes).as_deref(),
            Some("Rect")
        );
        assert_eq!(
            class_matching_field_reads(&reads(&["definitely_not_a_field"]), &classes),
            None
        );
    }

    /// A call site that passed a real instance is what narrows `record` to the
    /// class — and a call site that passed something else entirely means the
    /// two tracks disagree, so nothing is suggested.
    #[test]
    fn a_call_site_narrows_record_to_a_class_but_only_a_compatible_one() {
        let classes = classes();
        let rect = Type::Class(classes.lookup("Rect").expect("built-in Rect"));

        let mut ev = reads(&["x", "y"]);
        ev.push(call(rect));
        assert_eq!(conclude(&ev, Slot::Param(0), &classes), Some(rect));

        let mut ev = reads(&["x", "y"]);
        ev.push(call(Type::Int));
        assert_eq!(conclude(&ev, Slot::Param(0), &classes), None);
    }

    /// A slot two modules both declare is dropped rather than suggested from a
    /// mixture of their call sites.
    #[test]
    fn an_ambiguous_name_yields_no_suggestion() {
        let key = ("f".to_string(), 1);
        let mut inf = Inferences::default();
        inf.note_declaration(key.clone(), "a", &[false], false);
        inf.note_param(key.clone(), 0, call(Type::Bool));
        assert_eq!(inf.resolve(&classes()).len(), 1);

        inf.note_declaration(key, "b", &[false], false);
        assert!(inf.resolve(&classes()).is_empty());
    }

    /// A slot the author already annotated is never suggested for, however
    /// much evidence exists about it.
    #[test]
    fn an_annotated_slot_is_left_alone() {
        let key = ("f".to_string(), 1);
        let mut inf = Inferences::default();
        inf.note_declaration(key.clone(), "a", &[true], true);
        inf.note_param(key.clone(), 0, call(Type::Bool));
        inf.note_return(key, Evidence::Tail {
            span: span(),
            ty: Type::Bool,
        });
        assert!(inf.resolve(&classes()).is_empty());
    }
}
