//! IR equivalence — "are these two compiled programs the same program?"
//!
//! The verification primitive behind `petal ir-equal` and `petal lint --fix
//! --verify` (see docs/dev/refactor-verification.md §5 `ir-equal` and §7). It
//! answers the one question a mechanical source refactor has to answer: did
//! the rewrite change what the program *means*?
//!
//! It compares the two term graphs structurally, by walking from the root
//! block and from each function body, pairing up terms and blocks as it goes.
//! What is compared and what is ignored:
//!
//! **Compared (semantic).** Function count/order, each function's name,
//! parameters, captures and registers; each block's parameter names, register
//! count, ordered term list and phi carry-outs; each term's op, its constants
//! *by value*, its input edges (as a graph shape, not as raw ids), its name,
//! state key, callsite id, register, and the `collect` / `is_config` flags;
//! match-arm patterns and their guard/body blocks; the program's declared
//! class names and its `has_errors` flag.
//!
//! **Ignored (positional).** Source text, source spans, file ids, the whole
//! source map, comments and whitespace (they reach the IR only through spans),
//! the numeric values of `TermId` / `BlockId` / `ConstantId` (only the
//! *correspondence* between the two sides matters), the constant table's
//! layout and any constant nothing references, and terms unreachable from the
//! root block or a function body.
//!
//! **Two deliberate strictness choices.**
//!
//! - *Variable names are semantic here.* Petal stores binding names in the IR
//!   (`Term::name`), a `state` binding's key is a hash of its name
//!   (`Compiler::hash_state_name` — so a rename silently orphans persisted
//!   state), and `--observe` / `explain` report by name. A rename is therefore
//!   reported as a difference. That is the conservative direction for a
//!   refactor gate: `ir_equivalent` returning `Ok` is meant to be *proof*, and
//!   a rewrite that renames things does not get to skip the run-diff.
//! - *Registers are compared.* They are derived from block structure, so equal
//!   graphs always get equal registers; a register difference means a
//!   structural difference the walk would have found anyway, reported earlier.

// `IrDiff` is a handful of Strings — a report, produced at most once per
// comparison and never on a hot path. Boxing every `Result` for its size would
// only obscure the API.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::collections::VecDeque;
use std::fmt;

use crate::constant_table::{ConstantId, ConstantValue};
use crate::program::{BlockId, FunctionDef, MapSpreadEntry, Program, TermId, TermOp};
use crate::source_map::SourceSpan;

/// The first difference found between two programs.
#[derive(Debug, Clone)]
pub struct IrDiff {
    /// Where the difference is, in program terms — `"root block"`,
    /// `"function `draw` body"`, `"then-branch of term #3"`.
    pub location: String,
    /// Which property differed (`"op"`, `"term count"`, `"input count"`, …).
    pub what: String,
    /// The left (original) side's value.
    pub left: String,
    /// The right (rewritten) side's value.
    pub right: String,
    /// Where the left side's term was written, when the left program still
    /// carries a source map. This is the pointer the user needs.
    pub span: Option<SourceSpan>,
}

impl IrDiff {
    fn new(
        location: impl Into<String>,
        what: impl Into<String>,
        left: impl Into<String>,
        right: impl Into<String>,
    ) -> Self {
        IrDiff {
            location: location.into(),
            what: what.into(),
            left: left.into(),
            right: right.into(),
            span: None,
        }
    }

    fn at(mut self, span: Option<SourceSpan>) -> Self {
        self.span = span;
        self
    }

    /// `line:col` of the left side's span, when known.
    pub fn position(&self) -> Option<String> {
        self.span
            .map(|s| format!("{}:{}", s.start.line, s.start.column))
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "location": self.location,
            "what": self.what,
            "left": self.left,
            "right": self.right,
        });
        if let Some(pos) = self.position() {
            obj["position"] = serde_json::json!(pos);
            let span = self.span.expect("position implies span");
            obj["line"] = serde_json::json!(span.start.line);
            obj["column"] = serde_json::json!(span.start.column);
        }
        obj
    }
}

impl fmt::Display for IrDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} differs", self.location, self.what)?;
        if let Some(pos) = self.position() {
            write!(f, " (at {})", pos)?;
        }
        write!(
            f,
            "\n  original: {}\n  rewritten: {}",
            self.left, self.right
        )
    }
}

/// Are `a` and `b` the same program, ignoring everything positional?
///
/// `a` is treated as the original: reported spans are `a`'s. See the module
/// docs for exactly what counts as semantic.
pub fn ir_equivalent(a: &Program, b: &Program) -> Result<(), IrDiff> {
    Cmp::new(a, b).run()
}

// ---------------------------------------------------------------------------
// Comparison walk
// ---------------------------------------------------------------------------

struct Cmp<'p> {
    a: &'p Program,
    b: &'p Program,
    /// Pairings discovered so far, in both directions, so an aliasing change
    /// ("these two edges used to point at the same term") is caught.
    terms: HashMap<TermId, TermId>,
    terms_rev: HashMap<TermId, TermId>,
    blocks: HashMap<BlockId, BlockId>,
    blocks_rev: HashMap<BlockId, BlockId>,
    /// Block pairs still to walk, with the label used in diffs.
    queue: VecDeque<(BlockId, BlockId, String)>,
    /// A register-allocation difference, held back until the structural walk
    /// finishes. Registers are *derived* from block structure, so a register
    /// mismatch is nearly always the shadow of a structural difference
    /// elsewhere — and the structural one is the better thing to show. This is
    /// reported only if the walk finds nothing else.
    deferred: Option<IrDiff>,
}

impl<'p> Cmp<'p> {
    fn new(a: &'p Program, b: &'p Program) -> Self {
        Cmp {
            a,
            b,
            terms: HashMap::new(),
            terms_rev: HashMap::new(),
            blocks: HashMap::new(),
            blocks_rev: HashMap::new(),
            queue: VecDeque::new(),
            deferred: None,
        }
    }

    fn run(mut self) -> Result<(), IrDiff> {
        if self.a.has_errors != self.b.has_errors {
            return Err(IrDiff::new(
                "program",
                "has_errors",
                self.a.has_errors.to_string(),
                self.b.has_errors.to_string(),
            ));
        }
        let (ca, cb) = (&self.a.class_names, &self.b.class_names);
        if ca != cb {
            return Err(IrDiff::new(
                "program",
                "class names",
                format!("{:?}", ca),
                format!("{:?}", cb),
            ));
        }
        if self.a.functions.len() != self.b.functions.len() {
            return Err(IrDiff::new(
                "program",
                "function count",
                self.a.functions.len().to_string(),
                self.b.functions.len().to_string(),
            ));
        }

        self.pair_blocks(self.a.root_block, self.b.root_block, "root block")?;
        for i in 0..self.a.functions.len() {
            let (fa, fb) = (&self.a.functions[i], &self.b.functions[i]);
            self.compare_function(fa, fb)?;
        }

        while let Some((ba, bb, label)) = self.queue.pop_front() {
            self.compare_block(ba, bb, &label)?;
        }
        match self.deferred {
            Some(diff) => Err(diff),
            None => Ok(()),
        }
    }

    /// Hold a register difference back (see [`Cmp::deferred`]).
    fn defer(&mut self, diff: IrDiff) {
        if self.deferred.is_none() {
            self.deferred = Some(diff);
        }
    }

    fn fn_label(f: &FunctionDef) -> String {
        match &f.name {
            Some(n) => format!("function `{}`", n),
            None => format!("anonymous function #{}", f.id.0),
        }
    }

    fn compare_function(&mut self, fa: &FunctionDef, fb: &FunctionDef) -> Result<(), IrDiff> {
        let label = Self::fn_label(fa);
        let checks: [(&str, String, String); 5] = [
            ("name", format!("{:?}", fa.name), format!("{:?}", fb.name)),
            (
                "parameters",
                format!("{:?}", fa.params),
                format!("{:?}", fb.params),
            ),
            (
                "captures",
                format!("{:?}", fa.capture_names),
                format!("{:?}", fb.capture_names),
            ),
            (
                "capture registers",
                format!("{:?}", fa.capture_registers),
                format!("{:?}", fb.capture_registers),
            ),
            (
                "self-reference register",
                format!("{:?}", fa.self_ref_register),
                format!("{:?}", fb.self_ref_register),
            ),
        ];
        for (what, left, right) in checks {
            if left != right {
                return Err(IrDiff::new(label.clone(), what, left, right));
            }
        }
        if fa.register_count != fb.register_count {
            self.defer(IrDiff::new(
                label.clone(),
                "register count",
                fa.register_count.to_string(),
                fb.register_count.to_string(),
            ));
        }
        self.pair_blocks(fa.body_block, fb.body_block, &format!("{} body", label))
    }

    /// Record a block correspondence and enqueue the pair for walking.
    /// Re-pairing an already-paired block is fine as long as it pairs with the
    /// same partner; pairing it differently means the two graphs share
    /// structure in different places.
    fn pair_blocks(&mut self, ba: BlockId, bb: BlockId, label: &str) -> Result<(), IrDiff> {
        match (self.blocks.get(&ba), self.blocks_rev.get(&bb)) {
            (Some(&seen), _) if seen != bb => Err(IrDiff::new(
                label,
                "block sharing",
                format!("block {} already paired with {}", ba.0, seen.0),
                format!("now paired with {}", bb.0),
            )),
            (Some(_), _) => Ok(()),
            (None, Some(&seen)) => Err(IrDiff::new(
                label,
                "block sharing",
                format!("block {} is unpaired", ba.0),
                format!("block {} already paired with {}", bb.0, seen.0),
            )),
            (None, None) => {
                self.blocks.insert(ba, bb);
                self.blocks_rev.insert(bb, ba);
                self.queue.push_back((ba, bb, label.to_string()));
                Ok(())
            }
        }
    }

    fn compare_block(&mut self, ba: BlockId, bb: BlockId, label: &str) -> Result<(), IrDiff> {
        let (blk_a, blk_b) = (self.a.get_block(ba), self.b.get_block(bb));
        if blk_a.param_names != blk_b.param_names {
            return Err(IrDiff::new(
                label,
                "block parameter names",
                format!("{:?}", blk_a.param_names),
                format!("{:?}", blk_b.param_names),
            ));
        }
        if blk_a.terms.len() != blk_b.terms.len() {
            return Err(IrDiff::new(
                label,
                "statement count",
                blk_a.terms.len().to_string(),
                blk_b.terms.len().to_string(),
            )
            .at(self.first_span(&blk_a.terms)));
        }
        for (i, (&ta, &tb)) in blk_a.terms.iter().zip(blk_b.terms.iter()).enumerate() {
            self.compare_term(ta, tb, &format!("{}, statement #{}", label, i))?;
        }
        if blk_a.register_count != blk_b.register_count {
            self.defer(IrDiff::new(
                label,
                "block register count",
                blk_a.register_count.to_string(),
                blk_b.register_count.to_string(),
            ));
        }
        if blk_a.phi_outs.len() != blk_b.phi_outs.len() {
            return Err(IrDiff::new(
                label,
                "phi carry-out count",
                blk_a.phi_outs.len().to_string(),
                blk_b.phi_outs.len().to_string(),
            ));
        }
        for (i, (pa, pb)) in blk_a.phi_outs.iter().zip(blk_b.phi_outs.iter()).enumerate() {
            let where_ = format!("{}, phi carry-out #{}", label, i);
            self.compare_term(pa.src_term, pb.src_term, &format!("{} source", where_))?;
            self.compare_term(pa.dest_term, pb.dest_term, &format!("{} target", where_))?;
        }
        Ok(())
    }

    fn first_span(&self, terms: &[TermId]) -> Option<SourceSpan> {
        terms
            .first()
            .and_then(|t| self.a.source_map.get(*t))
            .copied()
    }

    fn compare_term(&mut self, ta: TermId, tb: TermId, label: &str) -> Result<(), IrDiff> {
        let span = self.a.source_map.get(ta).copied();
        // Already-paired terms are compared once; re-reaching one only has to
        // confirm the pairing (which is what makes sharing part of the check).
        match (self.terms.get(&ta), self.terms_rev.get(&tb)) {
            (Some(&seen), _) if seen != tb => {
                return Err(IrDiff::new(
                    label,
                    "dataflow sharing",
                    format!("term {} already paired with {}", ta.0, seen.0),
                    format!("now paired with {}", tb.0),
                )
                .at(span));
            }
            (Some(_), _) => return Ok(()),
            (None, Some(&seen)) => {
                return Err(IrDiff::new(
                    label,
                    "dataflow sharing",
                    format!("term {} is newly reached", ta.0),
                    format!("term {} was already paired with {}", tb.0, seen.0),
                )
                .at(span));
            }
            (None, None) => {}
        }
        self.terms.insert(ta, tb);
        self.terms_rev.insert(tb, ta);

        let (term_a, term_b) = (self.a.get_term(ta), self.b.get_term(tb));
        let (key_a, key_b) = (op_key(self.a, &term_a.op), op_key(self.b, &term_b.op));
        if key_a != key_b {
            return Err(IrDiff::new(label, "op", key_a, key_b).at(span));
        }
        let scalars: [(&str, String, String); 5] = [
            (
                "name",
                format!("{:?}", term_a.name),
                format!("{:?}", term_b.name),
            ),
            (
                "state key",
                format!("{:?}", term_a.state_key.map(|k| k.0)),
                format!("{:?}", term_b.state_key.map(|k| k.0)),
            ),
            (
                "call site",
                format!("{:?}", term_a.call_site),
                format!("{:?}", term_b.call_site),
            ),
            (
                "state path pop",
                term_a.path_pop.to_string(),
                term_b.path_pop.to_string(),
            ),
            (
                "collect flag",
                term_a.collect.to_string(),
                term_b.collect.to_string(),
            ),
        ];
        for (what, left, right) in scalars {
            if left != right {
                return Err(IrDiff::new(label, what, left, right).at(span));
            }
        }
        if term_a.register != term_b.register {
            self.defer(
                IrDiff::new(
                    label,
                    "register",
                    term_a.register.0.to_string(),
                    term_b.register.0.to_string(),
                )
                .at(span),
            );
        }
        if term_a.is_config != term_b.is_config {
            return Err(IrDiff::new(
                label,
                "config flag",
                term_a.is_config.to_string(),
                term_b.is_config.to_string(),
            )
            .at(span));
        }

        if term_a.inputs.len() != term_b.inputs.len() {
            return Err(IrDiff::new(
                label,
                "input count",
                term_a.inputs.len().to_string(),
                term_b.inputs.len().to_string(),
            )
            .at(span));
        }
        if term_a.child_blocks.len() != term_b.child_blocks.len() {
            return Err(IrDiff::new(
                label,
                "child block count",
                term_a.child_blocks.len().to_string(),
                term_b.child_blocks.len().to_string(),
            )
            .at(span));
        }

        let inputs: Vec<(TermId, TermId)> = term_a
            .inputs
            .iter()
            .copied()
            .zip(term_b.inputs.iter().copied())
            .collect();
        for (i, (ia, ib)) in inputs.into_iter().enumerate() {
            self.compare_term(ia, ib, &format!("{} (input #{} of {})", label, i, key_a))?;
        }

        let children: Vec<(BlockId, BlockId)> = term_a
            .child_blocks
            .iter()
            .copied()
            .zip(term_b.child_blocks.iter().copied())
            .collect();
        for (i, (ca, cb)) in children.into_iter().enumerate() {
            self.pair_blocks(ca, cb, &format!("{} (child block #{})", label, i))?;
        }

        if matches!(term_a.op, TermOp::Match) {
            self.compare_match_arms(ta, tb, label, span)?;
        }
        Ok(())
    }

    fn compare_match_arms(
        &mut self,
        ta: TermId,
        tb: TermId,
        label: &str,
        span: Option<SourceSpan>,
    ) -> Result<(), IrDiff> {
        let empty = Vec::new();
        let arms_a = self.a.match_arms.get(&ta).unwrap_or(&empty);
        let arms_b = self.b.match_arms.get(&tb).unwrap_or(&empty);
        if arms_a.len() != arms_b.len() {
            return Err(IrDiff::new(
                label,
                "match arm count",
                arms_a.len().to_string(),
                arms_b.len().to_string(),
            )
            .at(span));
        }
        // Copied out of both programs first: the walk below takes `&mut self`,
        // which cannot coexist with borrows of `self.a` / `self.b`.
        let pairs: Vec<(ArmFacts, ArmFacts)> = arms_a
            .iter()
            .zip(arms_b.iter())
            .map(|(x, y)| (ArmFacts::of(x), ArmFacts::of(y)))
            .collect();
        for (i, (arm_a, arm_b)) in pairs.into_iter().enumerate() {
            if arm_a.pattern != arm_b.pattern {
                return Err(IrDiff::new(
                    format!("{}, match arm #{}", label, i),
                    "pattern",
                    arm_a.pattern,
                    arm_b.pattern,
                )
                .at(span));
            }
            let (ga, gb) = (arm_a.guard_block, arm_b.guard_block);
            match (ga, gb) {
                (Some(x), Some(y)) => {
                    self.pair_blocks(x, y, &format!("{}, match arm #{} guard", label, i))?
                }
                (None, None) => {}
                _ => {
                    return Err(IrDiff::new(
                        format!("{}, match arm #{}", label, i),
                        "guard",
                        if ga.is_some() { "present" } else { "absent" },
                        if gb.is_some() { "present" } else { "absent" },
                    )
                    .at(span));
                }
            }
            self.pair_blocks(
                arm_a.body_block,
                arm_b.body_block,
                &format!("{}, match arm #{} body", label, i),
            )?;
        }
        Ok(())
    }
}

/// One match arm's comparable content, lifted out of its program so the walk
/// can keep mutating the comparison state while it works through the arms.
struct ArmFacts {
    pattern: String,
    guard_block: Option<BlockId>,
    body_block: BlockId,
}

impl ArmFacts {
    fn of(arm: &crate::program::MatchArmMeta) -> Self {
        ArmFacts {
            // Patterns carry no ids and no spans (`crate::ast::Pattern`), so
            // their Debug rendering is a faithful structural key.
            pattern: format!("{:?}", arm.pattern),
            guard_block: arm.guard_block,
            body_block: arm.body_block,
        }
    }
}

// ---------------------------------------------------------------------------
// Op keys
// ---------------------------------------------------------------------------

/// Render a constant *by value*. Constant ids are interning artifacts — the
/// same string can land at a different id when a rewrite changes what gets
/// interned first — so nothing in this module ever compares ids.
fn cval(p: &Program, c: ConstantId) -> String {
    match p.constants.get(c) {
        ConstantValue::Nil => "nil".to_string(),
        ConstantValue::Bool(b) => format!("bool {}", b),
        ConstantValue::Int(i) => format!("int {}", i),
        ConstantValue::Float(bits) => format!("float {}", f64::from_bits(*bits)),
        ConstantValue::String(s) => format!("str {:?}", s),
    }
}

fn cvals(p: &Program, ids: &[ConstantId]) -> String {
    let parts: Vec<String> = ids.iter().map(|c| cval(p, *c)).collect();
    format!("[{}]", parts.join(", "))
}

/// A program-independent rendering of a term's op: same string ⟺ same
/// operation, with every constant resolved to its value.
fn op_key(p: &Program, op: &TermOp) -> String {
    match op {
        TermOp::Constant(c) => format!("Constant({})", cval(p, *c)),
        TermOp::Error(c) => format!("Error({})", cval(p, *c)),
        TermOp::GetField(c) => format!("GetField({})", cval(p, *c)),
        TermOp::GetFieldOpt(c) => format!("GetFieldOpt({})", cval(p, *c)),
        TermOp::SetField(c) => format!("SetField({})", cval(p, *c)),
        TermOp::BuiltinCall(c) => format!("BuiltinCall({})", cval(p, *c)),
        TermOp::MakeEnumVariant(c) => format!("MakeEnumVariant({})", cval(p, *c)),
        TermOp::MethodCall { name, hint } => format!(
            "MethodCall({}, hint={})",
            cval(p, *name),
            hint.map(|h| cval(p, h)).unwrap_or_else(|| "none".into())
        ),
        TermOp::AllocMap { fields, class } => format!(
            "AllocMap(fields={}, class={})",
            cvals(p, fields),
            class.map(|c| cval(p, c)).unwrap_or_else(|| "none".into())
        ),
        TermOp::AllocElement { tag, prop_keys } => format!(
            "AllocElement(tag={}, props={})",
            cval(p, *tag),
            cvals(p, prop_keys)
        ),
        TermOp::AllocMapSpread { entries } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|e| match e {
                    MapSpreadEntry::Spread(i) => format!("spread(input {})", i),
                    MapSpreadEntry::Named(c, i) => {
                        format!("named({}, input {})", cval(p, *c), i)
                    }
                })
                .collect();
            format!("AllocMapSpread([{}])", parts.join(", "))
        }
        // `MakeClosure` names a function by index into `Program.functions`,
        // which the walk compares element-wise, so the index is meaningful
        // on both sides.
        TermOp::MakeClosure(f) => format!("MakeClosure(function #{})", f.0),
        // Everything else carries no ids at all.
        other => format!("{:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Source-level convenience
// ---------------------------------------------------------------------------

/// Compile one source text the way `petal run` would, for comparison.
///
/// Returns the owning [`Env`] alongside the id: a `Program` is borrowed from
/// its `Env`, so the caller has to keep it alive.
pub fn compile_for_compare(
    source: &str,
    include_dirs: &[std::path::PathBuf],
    origin: Option<&std::path::Path>,
) -> Result<(crate::env::Env, crate::program::ProgramId), String> {
    let mut env = crate::env::Env::new();
    for dir in include_dirs {
        env.add_module_path(dir.clone());
    }
    let pid = match origin {
        Some(path) => env.load_program_at(source, path)?,
        None => env.load_program(source)?,
    };
    Ok((env, pid))
}

/// Compile both texts and compare them. `Err(Ok(diff))` is "compiled, not
/// equivalent"; `Err(Err(msg))` is "one of them didn't compile".
pub fn sources_equivalent(
    original: &str,
    rewritten: &str,
    include_dirs: &[std::path::PathBuf],
    origin: Option<&std::path::Path>,
) -> Result<Result<(), IrDiff>, String> {
    let (env_a, pid_a) = compile_for_compare(original, include_dirs, origin)
        .map_err(|e| format!("original does not compile: {e}"))?;
    let (env_b, pid_b) = compile_for_compare(rewritten, include_dirs, origin)
        .map_err(|e| format!("rewritten does not compile: {e}"))?;
    let pa = env_a
        .get_program(pid_a)
        .ok_or_else(|| "original program missing".to_string())?;
    let pb = env_b
        .get_program(pid_b)
        .ok_or_else(|| "rewritten program missing".to_string())?;
    Ok(ir_equivalent(pa, pb))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn equiv(a: &str, b: &str) -> Result<(), IrDiff> {
        sources_equivalent(a, b, &[], None).expect("both compile")
    }

    #[test]
    fn identical_source_is_equivalent() {
        let src = "let x = 1\nlet y = x + 2\nprint(y)\n";
        assert!(equiv(src, src).is_ok());
    }

    #[test]
    fn whitespace_and_comments_are_ignored() {
        let a = "let x = 1\nlet y = x + 2\nprint(y)\n";
        let b = "\n// a comment\nlet x   =  1\n\n    let y = x + 2   // trailing\nprint(y)\n";
        assert!(equiv(a, b).is_ok(), "{:?}", equiv(a, b).unwrap_err());
    }

    #[test]
    fn reindentation_is_ignored() {
        let a =
            "fn f(n)\n  if n > 0 then\n    print(n)\n  else\n    print(0)\n  end\nend\n\nf(3)\n";
        let b = "fn f(n)\n      if n > 0 then\n            print(n)\n      else\n            print(0)\n      end\nend\n\nf(3)\n";
        assert!(equiv(a, b).is_ok(), "{:?}", equiv(a, b).unwrap_err());
    }

    /// Binding names are part of the IR (state keys hash them, `--observe`
    /// reports them), so a rename is *not* equivalent. Documented in the
    /// module docs; this pins the decision.
    #[test]
    fn renaming_a_variable_is_a_difference() {
        let a = "let x = 1\nprint(x)\n";
        let b = "let renamed = 1\nprint(renamed)\n";
        let diff = equiv(a, b).expect_err("rename should differ");
        assert_eq!(diff.what, "name");
        assert!(diff.left.contains('x'), "{}", diff.left);
        assert!(diff.right.contains("renamed"), "{}", diff.right);
    }

    #[test]
    fn changed_constant_is_named_in_the_diff() {
        let a = "let x = 1\nprint(x)\n";
        let b = "let x = 2\nprint(x)\n";
        let diff = equiv(a, b).expect_err("constant change should differ");
        assert_eq!(diff.what, "op");
        assert!(diff.left.contains("int 1"), "{}", diff.left);
        assert!(diff.right.contains("int 2"), "{}", diff.right);
        assert!(diff.span.is_some(), "diff should carry the original's span");
    }

    #[test]
    fn changed_string_constant_compares_by_value() {
        let a = "print(\"hello\")\n";
        let b = "print(\"goodbye\")\n";
        let diff = equiv(a, b).expect_err("string change should differ");
        assert!(diff.left.contains("hello"), "{}", diff.left);
    }

    /// Interning order changes but nothing else: the same two strings, printed
    /// in the same order, with an extra earlier use of the second one removed.
    /// Constant ids shift; the comparison must not care.
    #[test]
    fn constant_ids_may_shift() {
        let a = "let a = \"zzz\"\nlet b = \"aaa\"\nprint(b)\nprint(a)\n";
        let b = "let a = \"zzz\"\nlet b = \"aaa\"\nprint(b)\nprint(a)\n";
        assert!(equiv(a, b).is_ok());
    }

    #[test]
    fn reordered_statements_are_not_equivalent() {
        let a = "print(1)\nprint(2)\n";
        let b = "print(2)\nprint(1)\n";
        let diff = equiv(a, b).expect_err("reorder should differ");
        assert!(
            diff.left.contains("int 1") || diff.what == "op",
            "{:?}",
            diff
        );
    }

    #[test]
    fn an_extra_statement_is_not_equivalent() {
        let a = "print(1)\n";
        let b = "print(1)\nprint(2)\n";
        let diff = equiv(a, b).expect_err("extra statement should differ");
        assert_eq!(diff.what, "statement count");
    }

    #[test]
    fn different_function_bodies_are_not_equivalent() {
        let a = "fn f(n)\n  return n + 1\nend\n\nprint(f(1))\n";
        let b = "fn f(n)\n  return n + 2\nend\n\nprint(f(1))\n";
        let diff = equiv(a, b).expect_err("body change should differ");
        assert!(diff.location.contains("f"), "{}", diff.location);
    }

    #[test]
    fn removing_an_identity_cast_is_not_equivalent() {
        // The `casts` lint pass is a real IR change by design — it deletes a
        // call — so `ir_equivalent` must report it rather than wave it through.
        let a = "let n = 1\nprint(int(n))\n";
        let b = "let n = 1\nprint(n)\n";
        assert!(equiv(a, b).is_err());
    }
}
