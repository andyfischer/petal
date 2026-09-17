//! Opt-in runtime audit of native effect declarations: what each native was
//! *observed* doing, held against what it *declared*.
//!
//! A native registered with a [`NativeEffects`] row is taken at its word by
//! the reactive layers (the memo classifies the call from the row and never
//! looks at the activity counters). That makes an under-declared row a silent
//! staleness bug: a native that reaches host state without saying so looks
//! pure, and every memoized scope that calls it replays without calling it.
//! One registered without a row is classified by inference around each call,
//! which is correct but is the fallback step 5 of the declarative-effect task
//! removes (`docs/tasks/declarative-effect-refactoring.md`).
//!
//! With the audit on, the VM snapshots the activity counters around *every*
//! native call — declared or not — and accumulates the deltas here, per
//! native. [`EffectAudit::findings`] then reports:
//!
//! - **under-declared**: a declared native was seen doing something its row
//!   does not cover. A bug; the row must grow.
//! - **undeclared**: a native with no row, and what it was seen doing. Some
//!   of these are pure and want `NativeEffects::PURE`; the rest want the
//!   observed facets. Either way the row must be written before step 5.
//! - **over-declared**: a declared facet the corpus never exercised. Not a
//!   bug — a row is the union over every path, and the corpus may not have
//!   taken the path — but worth a look when the row was a guess.
//!
//! `petal run --effect-audit` and `petal-ui-run --effect-audit` print the
//! report to stderr; `petal-ui/tests/effect_audit.rs` runs it over the whole
//! in-tree corpus and fails on any under-declaration.
//!
//! Like [`crate::profile`], this is a runtime switch on the [`Env`]: off, the
//! only cost is one branch per native call.
//!
//! [`Env`]: crate::env::Env
//! [`NativeEffects`]: crate::native_fn::NativeEffects

use std::fmt;

use crate::native_fn::{InputClasses, NativeEffects, NativeFnId, NativeFnTable};
use crate::run_deps::Activity;
use crate::symbol::{SymbolId, SymbolTable};

/// How many distinct binding symbols to remember per native. A native reads
/// one or two bindings by construction; `binding(sym)` is the exception and
/// reads whatever it is handed, which is exactly what the cap is for.
const MAX_BINDINGS: usize = 8;

/// What one native was seen doing, summed over every audited call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// Calls that reached the native (a `Pending` argument intercepted
    /// before the call does not count).
    pub calls: u64,
    /// Calls that read at least one host→script binding.
    pub binding_reads: u64,
    /// Calls that reported a host-data read (`note_host_read`).
    pub host_reads: u64,
    /// Calls that consulted the resource table.
    pub resource_reads: u64,
    /// Calls that pushed into an output buffer.
    pub emits: u64,
    /// Calls that reported an effect no replay could reproduce.
    pub effects: u64,
    /// The bindings read, in first-seen order, capped at [`MAX_BINDINGS`].
    pub bindings: Vec<SymbolId>,
    /// Whether the cap on `bindings` was hit.
    pub more_bindings: bool,
}

impl Observed {
    /// Nothing observed beyond the call itself: the native looks pure.
    pub fn is_silent(&self) -> bool {
        self.binding_reads == 0
            && self.host_reads == 0
            && self.resource_reads == 0
            && self.emits == 0
            && self.effects == 0
    }
}

/// One line of the report: a native whose declaration and behavior differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub native: NativeFnId,
    pub name: String,
    pub kind: FindingKind,
    /// The facets in question, as short words (`effect`, `host_read`,
    /// `binding mouse_x`, …).
    pub facets: Vec<String>,
    pub observed: Observed,
}

/// What kind of gap a [`Finding`] reports. Ordered by severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FindingKind {
    /// A declared native was seen doing something its row does not cover.
    UnderDeclared,
    /// A native with no row; `facets` is what it was seen doing (empty if it
    /// looked pure).
    Undeclared,
    /// A declared facet the audited runs never exercised.
    OverDeclared,
}

impl fmt::Display for FindingKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FindingKind::UnderDeclared => "under-declared",
            FindingKind::Undeclared => "undeclared",
            FindingKind::OverDeclared => "over-declared",
        })
    }
}

/// The audit's accumulator. Lives on the [`Env`](crate::env::Env) and sums
/// over every run on it until [`reset`](EffectAudit::reset).
#[derive(Debug, Clone, Default)]
pub struct EffectAudit {
    /// Master switch. The VM snapshots activity around every native call
    /// only while this is set.
    pub enabled: bool,
    /// Indexed by `NativeFnId`; grown on demand.
    by_native: Vec<Observed>,
}

impl EffectAudit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Turn the audit on or off for subsequent runs. Turning it on clears
    /// whatever was already observed.
    pub fn set_enabled(&mut self, on: bool) {
        if on {
            self.reset();
        }
        self.enabled = on;
    }

    /// Forget everything observed so far.
    pub fn reset(&mut self) {
        self.by_native.clear();
    }

    /// One native call finished. `before` and `after` bracket the call;
    /// `bindings` is every binding the call read, in read order.
    #[inline]
    pub fn record(
        &mut self,
        nid: NativeFnId,
        before: Activity,
        after: Activity,
        bindings: &[SymbolId],
    ) {
        let i = nid.0 as usize;
        if i >= self.by_native.len() {
            self.by_native.resize(i + 1, Observed::default());
        }
        let o = &mut self.by_native[i];
        o.calls += 1;
        if after.binding_reads != before.binding_reads {
            o.binding_reads += 1;
            for sym in bindings {
                if o.bindings.contains(sym) {
                    continue;
                }
                if o.bindings.len() < MAX_BINDINGS {
                    o.bindings.push(*sym);
                } else {
                    o.more_bindings = true;
                }
            }
        }
        if after.host_reads != before.host_reads {
            o.host_reads += 1;
        }
        if after.resource_reads != before.resource_reads {
            o.resource_reads += 1;
        }
        if after.emits != before.emits {
            o.emits += 1;
        }
        if after.effects != before.effects {
            o.effects += 1;
        }
    }

    /// What `nid` was seen doing, if it was called at all.
    pub fn observed(&self, nid: NativeFnId) -> Option<&Observed> {
        self.by_native.get(nid.0 as usize).filter(|o| o.calls > 0)
    }

    /// Every native that was called, with what it was seen doing.
    pub fn all(&self) -> impl Iterator<Item = (NativeFnId, &Observed)> {
        self.by_native
            .iter()
            .enumerate()
            .filter(|(_, o)| o.calls > 0)
            .map(|(i, o)| (NativeFnId(i as u32), o))
    }

    /// The observed behavior of every called native held against its row.
    /// Sorted most severe first, then by name.
    pub fn findings(&self, natives: &NativeFnTable, symbols: &SymbolTable) -> Vec<Finding> {
        let mut out = Vec::new();
        for (nid, observed) in self.all() {
            let name = natives.get_name(nid).to_string();
            let seen = observed_facets(observed, symbols);
            match natives.effects(nid) {
                None => out.push(Finding {
                    native: nid,
                    name,
                    kind: FindingKind::Undeclared,
                    facets: seen,
                    observed: observed.clone(),
                }),
                Some(row) => {
                    let missing = under_declared(observed, row, symbols);
                    if !missing.is_empty() {
                        out.push(Finding {
                            native: nid,
                            name: name.clone(),
                            kind: FindingKind::UnderDeclared,
                            facets: missing,
                            observed: observed.clone(),
                        });
                    }
                    let unused = over_declared(observed, row);
                    if !unused.is_empty() {
                        out.push(Finding {
                            native: nid,
                            name,
                            kind: FindingKind::OverDeclared,
                            facets: unused,
                            observed: observed.clone(),
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// The findings as a report for stderr: one line per finding, grouped by
    /// kind, with a summary line first. `None` if nothing was called.
    pub fn report(&self, natives: &NativeFnTable, symbols: &SymbolTable) -> Report {
        Report {
            called: self.all().count(),
            findings: self.findings(natives, symbols),
        }
    }
}

/// The classes a binding read is declared through: any class the memo would
/// treat as a probe (everything but the three that have deps of their own).
const PROBE_CLASSES: InputClasses = InputClasses(
    InputClasses::POINTER.0
        | InputClasses::KEYBOARD.0
        | InputClasses::CLOCK.0
        | InputClasses::VIEWPORT.0
        | InputClasses::BINDINGS.0,
);

fn binding_facet(observed: &Observed, symbols: &SymbolTable) -> String {
    let mut names: Vec<&str> = observed
        .bindings
        .iter()
        .map(|s| symbols.name(*s).unwrap_or("?"))
        .collect();
    if observed.more_bindings {
        names.push("…");
    }
    if names.is_empty() {
        "binding".to_string()
    } else {
        format!("binding {}", names.join(","))
    }
}

/// Every facet the native was seen exercising, as report words.
fn observed_facets(observed: &Observed, symbols: &SymbolTable) -> Vec<String> {
    let mut v = Vec::new();
    if observed.effects > 0 {
        v.push("effect".to_string());
    }
    if observed.host_reads > 0 {
        v.push("host_read".to_string());
    }
    if observed.resource_reads > 0 {
        v.push("resource_read".to_string());
    }
    if observed.emits > 0 {
        v.push("emit".to_string());
    }
    if observed.binding_reads > 0 {
        v.push(binding_facet(observed, symbols));
    }
    v
}

/// The facets `observed` shows that `row` does not cover. Mirrors the
/// memo's two classifiers (`memo_note_native` against
/// `memo_note_declared_native`): each observed counter maps to the field of
/// the row that would have produced the same dep.
fn under_declared(observed: &Observed, row: NativeEffects, symbols: &SymbolTable) -> Vec<String> {
    let mut v = Vec::new();
    if observed.effects > 0 && !row.effect {
        v.push("effect".to_string());
    }
    if observed.host_reads > 0 && !row.reads.contains(InputClasses::HOST_DATA) {
        v.push("host_read".to_string());
    }
    if observed.resource_reads > 0 && !row.reads.contains(InputClasses::RESOURCES) {
        v.push("resource_read".to_string());
    }
    if observed.emits > 0 && !row.emits {
        v.push("emit".to_string());
    }
    if observed.binding_reads > 0 && (row.reads.0 & PROBE_CLASSES.0) == 0 {
        v.push(binding_facet(observed, symbols));
    }
    v
}

/// The facets `row` declares that `observed` never showed. Only the facets
/// inference can see: a declared probe class the corpus never read is
/// reported as `binding`, since the counters do not say which class moved.
fn over_declared(observed: &Observed, row: NativeEffects) -> Vec<String> {
    let mut v = Vec::new();
    if row.effect && observed.effects == 0 {
        v.push("effect".to_string());
    }
    if row.reads.contains(InputClasses::HOST_DATA) && observed.host_reads == 0 {
        v.push("host_read".to_string());
    }
    if row.reads.contains(InputClasses::RESOURCES) && observed.resource_reads == 0 {
        v.push("resource_read".to_string());
    }
    if row.emits && observed.emits == 0 {
        v.push("emit".to_string());
    }
    if (row.reads.0 & PROBE_CLASSES.0) != 0 && observed.binding_reads == 0 {
        v.push("binding".to_string());
    }
    v
}

/// A finished audit, printable.
#[derive(Debug, Clone)]
pub struct Report {
    /// Natives that were called at least once.
    pub called: usize,
    pub findings: Vec<Finding>,
}

impl Report {
    /// Whether any declared native did more than it declared. The condition
    /// the corpus test and `petal-ui-run --effect-audit` fail on.
    pub fn has_under_declared(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.kind == FindingKind::UnderDeclared)
    }

    fn count(&self, kind: FindingKind) -> usize {
        self.findings.iter().filter(|f| f.kind == kind).count()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "effect audit: {} natives called, {} under-declared, {} undeclared, {} over-declared",
            self.called,
            self.count(FindingKind::UnderDeclared),
            self.count(FindingKind::Undeclared),
            self.count(FindingKind::OverDeclared),
        )?;
        for finding in &self.findings {
            let facets = if finding.facets.is_empty() {
                "(silent)".to_string()
            } else {
                finding.facets.join(" ")
            };
            writeln!(
                f,
                "  {:<15} {:<28} {:>7} calls  {}",
                finding.kind, finding.name, finding.observed.calls, facets
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_fn::{NativeClass, PetalCxt};

    fn act(binding_reads: u32, host: u32, res: u32, emits: u32, effects: u32) -> Activity {
        Activity {
            binding_reads,
            host_reads: host,
            resource_reads: res,
            emits,
            effects,
        }
    }

    fn noop(_: &mut PetalCxt) -> crate::native_fn::NativeResult {
        Ok(0)
    }

    #[test]
    fn a_declared_native_seen_doing_more_is_under_declared() {
        let mut natives = NativeFnTable::new();
        let symbols = SymbolTable::new();
        let pure = natives.register_with("looks_pure", noop, NativeEffects::PURE);
        let mut audit = EffectAudit::new();
        audit.set_enabled(true);
        audit.record(pure, act(0, 0, 0, 0, 0), act(0, 1, 0, 0, 1), &[]);
        let findings = audit.findings(&natives, &symbols);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::UnderDeclared);
        assert_eq!(findings[0].facets, ["effect", "host_read"]);
        assert!(audit.report(&natives, &symbols).has_under_declared());
    }

    #[test]
    fn an_undeclared_native_is_listed_with_what_it_did_or_as_silent() {
        let mut natives = NativeFnTable::new();
        let mut symbols = SymbolTable::new();
        let mouse_x = symbols.intern("mouse_x");
        let silent = natives.register("silent", noop);
        let reader = natives.register("reader", noop);
        let mut audit = EffectAudit::new();
        audit.set_enabled(true);
        audit.record(silent, act(0, 0, 0, 0, 0), act(0, 0, 0, 0, 0), &[]);
        audit.record(reader, act(0, 0, 0, 0, 0), act(1, 0, 0, 0, 0), &[mouse_x]);
        let findings = audit.findings(&natives, &symbols);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.kind == FindingKind::Undeclared));
        let reader = findings.iter().find(|f| f.name == "reader").unwrap();
        assert_eq!(reader.facets, ["binding mouse_x"]);
        let silent = findings.iter().find(|f| f.name == "silent").unwrap();
        assert!(silent.facets.is_empty());
        assert!(!audit.report(&natives, &symbols).has_under_declared());
    }

    #[test]
    fn a_declared_facet_never_exercised_is_over_declared_and_a_match_is_nothing() {
        let mut natives = NativeFnTable::new();
        let symbols = SymbolTable::new();
        let row = NativeEffects::probe(InputClasses::POINTER)
            .with_effect()
            .with_pending(NativeClass::Effectful);
        let id = natives.register_with("probe_and_effect", noop, row);
        let never_called = natives.register_with("never_called", noop, NativeEffects::EFFECT);
        let mut audit = EffectAudit::new();
        audit.set_enabled(true);
        audit.record(id, act(0, 0, 0, 0, 0), act(1, 0, 0, 0, 0), &[]);
        let findings = audit.findings(&natives, &symbols);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::OverDeclared);
        assert_eq!(findings[0].facets, ["effect"]);
        assert!(audit.observed(never_called).is_none());
        // Once the effect path is exercised too, the row matches exactly.
        audit.record(id, act(1, 0, 0, 0, 0), act(2, 0, 0, 0, 1), &[]);
        assert!(audit.findings(&natives, &symbols).is_empty());
    }

    #[test]
    fn enabling_clears_the_previous_audit() {
        let mut natives = NativeFnTable::new();
        let symbols = SymbolTable::new();
        let id = natives.register("n", noop);
        let mut audit = EffectAudit::new();
        audit.set_enabled(true);
        audit.record(id, act(0, 0, 0, 0, 0), act(0, 0, 0, 0, 1), &[]);
        assert_eq!(audit.findings(&natives, &symbols).len(), 1);
        audit.set_enabled(true);
        assert!(audit.findings(&natives, &symbols).is_empty());
    }
}
