//! Human-readable text formatting for the IR, plus the filtered "user-only"
//! JSON view behind `show-ir --user-only`.
//!
//! Two kinds of noise are hidden by default (restored with `--all`):
//!
//! - **Phantom terms** — synthetic `Copy` terms with no source location: one
//!   per registered builtin in the root block, plus the param/capture/self
//!   binding terms at the head of each function body ([`is_phantom`]).
//! - **Prelude / module terms** — everything whose source lives in a
//!   non-entry file (`source_map` file index ≠ 0): the auto-loaded `std`
//!   prelude and explicit imports. See [`IrFilter`].

use std::collections::HashMap;
use std::fmt::Write;

use crate::ast::{Literal, Pattern};
use crate::constant_table::{ConstantId, ConstantValue};
use crate::program::{
    Block, BlockId, FunctionId, MapSpreadEntry, Program, Term, TermId, TermOp, base_fn_name,
};
use crate::source_map::{ENTRY_FILE, FileId};

/// Returns true for synthetic builtin terms that have no source location.
/// These flood the output of `show-ir` / `show-graph` on small programs (the
/// builtin function table alone is ~70 phantom Copy terms), so display
/// helpers hide them by default.
pub fn is_phantom(program: &Program, term: &Term) -> bool {
    if !crate::ir_validate::is_binding_phantom(term) {
        return false;
    }
    match program.source_map.get(term.id) {
        Some(span) => span.start.line == 0,
        None => true,
    }
}

// ---------------------------------------------------------------------------
// Entry-file / prelude classification
// ---------------------------------------------------------------------------

/// Classifies every term and block of a program as entry-file or
/// prelude/module content, so display paths can hide the auto-loaded `std`
/// prelude (and imports) by default.
///
/// A term's file is its span's file when it has one; spanless terms (compiler
/// synthesized) inherit their block's file. A block's file comes from its
/// parent term, or — for function bodies, which have no parent term — from
/// the `MakeClosure` site of the function that owns it.
pub struct IrFilter {
    term_prelude: Vec<bool>,
    block_prelude: Vec<bool>,
}

struct FileResolve<'a> {
    program: &'a Program,
    /// Function body block -> owning function.
    body_fn: HashMap<BlockId, FunctionId>,
    /// Function -> a `MakeClosure` term that creates it (its definition site).
    fn_site: HashMap<FunctionId, TermId>,
    memo: Vec<Option<FileId>>,
}

impl FileResolve<'_> {
    fn block_file(&mut self, b: BlockId) -> FileId {
        if let Some(f) = self.memo[b.0 as usize] {
            return f;
        }
        // Cycle guard; parent chains are acyclic in practice.
        self.memo[b.0 as usize] = Some(ENTRY_FILE);
        let block = self.program.get_block(b);
        let f = if b == self.program.root_block {
            ENTRY_FILE
        } else if let Some(pt) = block.parent_term_id {
            self.term_file(pt)
        } else if let Some(&fid) = self.body_fn.get(&b) {
            match self.fn_site.get(&fid).copied() {
                Some(tid) => self.term_file(tid),
                None => ENTRY_FILE,
            }
        } else {
            ENTRY_FILE
        };
        self.memo[b.0 as usize] = Some(f);
        f
    }

    fn term_file(&mut self, t: TermId) -> FileId {
        let term = self.program.get_term(t);
        match self.program.source_map.get(t) {
            // Line 0 marks a synthetic span (builtin phantoms) — no file info.
            Some(span) if span.start.line != 0 => span.file,
            _ => self.block_file(term.block_id),
        }
    }
}

impl IrFilter {
    pub fn new(program: &Program) -> Self {
        let mut body_fn = HashMap::new();
        for func in &program.functions {
            body_fn.insert(func.body_block, func.id);
        }
        let mut fn_site = HashMap::new();
        for term in &program.terms {
            if let TermOp::MakeClosure(fid) = term.op {
                fn_site.entry(fid).or_insert(term.id);
            }
        }
        let mut resolve = FileResolve {
            program,
            body_fn,
            fn_site,
            memo: vec![None; program.blocks.len()],
        };
        let term_prelude = (0..program.terms.len())
            .map(|i| resolve.term_file(TermId(i as u32)) != ENTRY_FILE)
            .collect();
        let block_prelude = (0..program.blocks.len())
            .map(|i| resolve.block_file(BlockId(i as u32)) != ENTRY_FILE)
            .collect();
        IrFilter {
            term_prelude,
            block_prelude,
        }
    }

    /// True when the term's source lives in a non-entry file (prelude/import).
    pub fn term_is_prelude(&self, t: TermId) -> bool {
        self.term_prelude
            .get(t.0 as usize)
            .copied()
            .unwrap_or(false)
    }

    /// True when the block belongs to a non-entry file (prelude/import).
    pub fn block_is_prelude(&self, b: BlockId) -> bool {
        self.block_prelude
            .get(b.0 as usize)
            .copied()
            .unwrap_or(false)
    }

    /// Per-constant visibility for the filtered views: a constant stays
    /// visible when a visible term references it (or when nothing references
    /// it at all — better to over-show than to hide something real).
    pub fn visible_constants(&self, program: &Program) -> Vec<bool> {
        let n = program.constants.values().len();
        let mut used_any = vec![false; n];
        let mut used_visible = vec![false; n];
        for term in &program.terms {
            let hidden = self.term_is_prelude(term.id) || is_phantom(program, term);
            for cid in term.op.constant_ids() {
                if let Some(slot) = used_any.get_mut(cid.0 as usize) {
                    *slot = true;
                }
                if !hidden && let Some(slot) = used_visible.get_mut(cid.0 as usize) {
                    *slot = true;
                }
            }
        }
        (0..n).map(|i| used_visible[i] || !used_any[i]).collect()
    }
}

// ---------------------------------------------------------------------------
// Constant / pattern rendering helpers
// ---------------------------------------------------------------------------

/// Resolve a constant to a compact literal for display (strings quoted).
/// Shares `ConstantValue::display_compact` with the bytecode disassembler.
fn kconst(program: &Program, k: ConstantId) -> String {
    program.constants.get(k).display_compact()
}

/// Resolve a constant that names something (a field, method, class, enum
/// variant, element tag): string constants render bare, everything else
/// falls back to [`kconst`].
fn kname(program: &Program, k: ConstantId) -> String {
    match program.constants.get(k) {
        ConstantValue::String(s) => s.clone(),
        _ => kconst(program, k),
    }
}

fn format_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool(b) => b.to_string(),
        Literal::Int(n) => n.to_string(),
        Literal::Float(f) => f.to_string(),
        Literal::String(s) => format!("{:?}", s),
    }
}

/// Render a match pattern back to source-like text.
pub fn format_pattern(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Literal(lit) => format_literal(lit),
        Pattern::Variable(name) => name.clone(),
        Pattern::Variant { name, fields } => {
            if fields.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = fields.iter().map(format_pattern).collect();
                format!("{}({})", name, inner.join(", "))
            }
        }
        Pattern::List { elements, rest } => {
            let mut parts: Vec<String> = elements.iter().map(format_pattern).collect();
            if let Some(rest) = rest {
                parts.push(format!("...{}", rest));
            }
            format!("[{}]", parts.join(", "))
        }
        Pattern::Record(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(k, p)| format!("{}: {}", k, format_pattern(p)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

// ---------------------------------------------------------------------------
// Text rendering
// ---------------------------------------------------------------------------

pub fn display_program(program: &Program) -> String {
    display_program_with(program, true)
}

/// Render the program as text. With `hide_noise` (the default), phantom
/// terms and prelude/module content are omitted — ids are never renumbered,
/// their lines are simply not printed. `--all` passes `false`.
pub fn display_program_with(program: &Program, hide_noise: bool) -> String {
    let filter = IrFilter::new(program);
    let const_visible = filter.visible_constants(program);
    let state_names = state_name_index(program);
    let guard_of = guard_index(program);
    let body_fn: HashMap<BlockId, &crate::program::FunctionDef> = program
        .functions
        .iter()
        .map(|f| (f.body_block, f))
        .collect();
    // Phantom binding terms per block, in id order (they are not on the
    // block's linked list).
    let mut block_phantoms: HashMap<BlockId, Vec<&Term>> = HashMap::new();
    for term in &program.terms {
        if is_phantom(program, term) {
            block_phantoms.entry(term.block_id).or_default().push(term);
        }
    }

    let mut out = String::new();

    // === Constants ===
    writeln!(out, "=== Constants ===").unwrap();
    for i in 0..program.constants.values().len() {
        if hide_noise && !const_visible[i] {
            continue;
        }
        writeln!(out, "  c{}: {}", i, kconst(program, ConstantId(i as u32))).unwrap();
    }
    writeln!(out).unwrap();

    // === Functions ===
    writeln!(out, "=== Functions ===").unwrap();
    for func in &program.functions {
        if hide_noise && filter.block_is_prelude(func.body_block) {
            continue;
        }
        let name = func.name.as_deref().map(base_fn_name).unwrap_or("<lambda>");
        let params: Vec<&str> = func.params.iter().map(|s| s.as_str()).collect();
        let captures: Vec<&str> = func.capture_names.iter().map(|s| s.as_str()).collect();
        writeln!(
            out,
            "  fn{}: {} params={:?} body=block{} captures={:?}",
            func.id.0, name, params, func.body_block.0, captures,
        )
        .unwrap();
    }
    writeln!(out).unwrap();

    // === Blocks === (root tree first, then each function body's tree)
    writeln!(out, "=== Blocks ===").unwrap();
    for &bid in &block_order(program) {
        if hide_noise && filter.block_is_prelude(bid) {
            continue;
        }
        let block = program.get_block(bid);
        render_block(
            &mut out,
            program,
            block,
            hide_noise,
            &filter,
            &state_names,
            &guard_of,
            &body_fn,
            block_phantoms
                .get(&bid)
                .map(|v| v.as_slice())
                .unwrap_or(&[]),
        );
    }

    out
}

/// Map each state key to the name of its (named) state term, so
/// `StateRead`/`StateWrite` lines can show which state they touch.
fn state_name_index(program: &Program) -> HashMap<u64, String> {
    let mut names = HashMap::new();
    for (key, name) in program.state_terms() {
        if let Some(name) = name {
            names.entry(key.0).or_insert_with(|| name.clone());
        }
    }
    names
}

/// Map each guard block to `(match term, arm index)` for header labels.
fn guard_index(program: &Program) -> HashMap<BlockId, (TermId, usize)> {
    let mut guards = HashMap::new();
    for (&match_term, arms) in &program.match_arms {
        for (i, arm) in arms.iter().enumerate() {
            if let Some(gb) = arm.guard_block {
                guards.insert(gb, (match_term, i));
            }
        }
    }
    guards
}

/// Blocks in tree order: the root block, then its descendants depth-first
/// (child blocks in term order; a match arm's guard block just before its
/// body). Function bodies follow as their own top-level trees, in function
/// order. Any block left over (defensive) is appended in id order.
fn block_order(program: &Program) -> Vec<BlockId> {
    let mut order = Vec::with_capacity(program.blocks.len());
    let mut visited = vec![false; program.blocks.len()];
    visit_block(program, program.root_block, &mut order, &mut visited);
    for func in &program.functions {
        visit_block(program, func.body_block, &mut order, &mut visited);
    }
    for i in 0..program.blocks.len() {
        if !visited[i] {
            order.push(BlockId(i as u32));
        }
    }
    order
}

fn visit_block(program: &Program, bid: BlockId, order: &mut Vec<BlockId>, visited: &mut Vec<bool>) {
    let idx = bid.0 as usize;
    if idx >= visited.len() || visited[idx] {
        return;
    }
    visited[idx] = true;
    order.push(bid);
    let block = program.get_block(bid);
    let mut tid = block.entry;
    while let Some(t) = tid {
        let term = program.get_term(t);
        if let Some(arms) = program.match_arms.get(&t) {
            for arm in arms {
                if let Some(gb) = arm.guard_block {
                    visit_block(program, gb, order, visited);
                }
                visit_block(program, arm.body_block, order, visited);
            }
        } else {
            for &cb in &term.child_blocks {
                visit_block(program, cb, order, visited);
            }
        }
        tid = term.block_next;
    }
}

#[allow(clippy::too_many_arguments)]
fn render_block(
    out: &mut String,
    program: &Program,
    block: &Block,
    hide_noise: bool,
    filter: &IrFilter,
    state_names: &HashMap<u64, String>,
    guard_of: &HashMap<BlockId, (TermId, usize)>,
    body_fn: &HashMap<BlockId, &crate::program::FunctionDef>,
    phantoms: &[&Term],
) {
    let func = body_fn.get(&block.id).copied();
    let label = if block.id == program.root_block {
        " [root]".to_string()
    } else if let Some(f) = func {
        format!(
            " (body of fn{} {})",
            f.id.0,
            f.name.as_deref().map(base_fn_name).unwrap_or("<lambda>")
        )
    } else if let Some(&(match_term, arm)) = guard_of.get(&block.id) {
        format!(" (guard for t{} arm{})", match_term.0, arm)
    } else if let Some(parent) = block.parent_term_id {
        format!(" (parent: t{})", parent.0)
    } else {
        String::new()
    };

    writeln!(
        out,
        "block{}{} regs={}",
        block.id.0, label, block.register_count
    )
    .unwrap();

    // Bindings header: params/captures/self with their term ids and
    // registers, so the (hidden) phantom binding terms leave no dangling
    // tNNN references in the visible term inputs.
    let by_name: HashMap<&str, &Term> = phantoms
        .iter()
        .filter_map(|t| t.name.as_deref().map(|n| (n, *t)))
        .collect();
    let mut consumed: Vec<TermId> = Vec::new();
    let bind = |name: &str, consumed: &mut Vec<TermId>| -> String {
        match by_name.get(name) {
            Some(t) => {
                consumed.push(t.id);
                format!("{}=t{}:r{}", name, t.id.0, t.register.0)
            }
            None => name.to_string(),
        }
    };
    let mut segments: Vec<String> = Vec::new();
    let param_names: &[String] = match func {
        Some(f) => &f.params,
        None => &block.param_names,
    };
    if !param_names.is_empty() {
        let parts: Vec<String> = param_names.iter().map(|n| bind(n, &mut consumed)).collect();
        segments.push(format!("params: {}", parts.join(", ")));
    }
    if let Some(f) = func {
        if !f.capture_names.is_empty() {
            let parts: Vec<String> = f
                .capture_names
                .iter()
                .map(|n| bind(n, &mut consumed))
                .collect();
            segments.push(format!("captures: {}", parts.join(", ")));
        }
        if let Some(self_reg) = f.self_ref_register
            && let Some(t) = phantoms.iter().find(|t| t.register == self_reg)
        {
            consumed.push(t.id);
            segments.push(format!(
                "self: {}=t{}:r{}",
                t.name.as_deref().map(base_fn_name).unwrap_or("<self>"),
                t.id.0,
                t.register.0
            ));
        }
    }
    // Any remaining phantom bindings — match-arm pattern variables, mostly —
    // get their own segment so no visible term references an invisible one.
    // The root block is exempt: its phantoms are the builtin table.
    if block.id != program.root_block {
        let leftover: Vec<String> = phantoms
            .iter()
            .filter(|t| !consumed.contains(&t.id))
            .filter_map(|t| {
                t.name
                    .as_deref()
                    .map(|n| format!("{}=t{}:r{}", n, t.id.0, t.register.0))
            })
            .collect();
        if !leftover.is_empty() {
            segments.push(format!("binds: {}", leftover.join(", ")));
        }
    }
    if !segments.is_empty() {
        writeln!(out, "  {}", segments.join("  ")).unwrap();
    }

    // With --all, the phantom binding terms print ahead of the linked list
    // (they are not on it).
    if !hide_noise {
        for term in phantoms {
            render_term(out, program, term, hide_noise, filter, state_names);
        }
    }

    let mut tid = block.entry;
    while let Some(t) = tid {
        let term = program.get_term(t);
        if hide_noise && (is_phantom(program, term) || filter.term_is_prelude(t)) {
            tid = term.block_next;
            continue;
        }
        render_term(out, program, term, hide_noise, filter, state_names);
        tid = term.block_next;
    }

    for phi in &block.phi_outs {
        let dest_name = program
            .get_term(phi.dest_term)
            .name
            .as_deref()
            .map(|n| format!(" ({})", n))
            .unwrap_or_default();
        writeln!(
            out,
            "  phi-out: t{} -> t{}{}",
            phi.src_term.0, phi.dest_term.0, dest_name
        )
        .unwrap();
    }

    writeln!(out).unwrap();
}

fn render_term(
    out: &mut String,
    program: &Program,
    term: &Term,
    hide_noise: bool,
    filter: &IrFilter,
    state_names: &HashMap<u64, String>,
) {
    let op_str = format_op(program, term, state_names);
    // An input that points at a hidden term gets its name inlined —
    // `Copy [t162(std::sum)]` — so nothing visible dangles. Same-block
    // binding phantoms are exempt: the block header already names them.
    let fmt_input = |i: &TermId| -> String {
        let input = program.get_term(*i);
        if hide_noise && let Some(name) = input.name.as_deref() {
            let phantom = is_phantom(program, input);
            let hidden = phantom || filter.term_is_prelude(*i);
            let header_covers =
                phantom && input.block_id == term.block_id && input.block_id != program.root_block;
            if hidden && !header_covers {
                return format!("t{}({})", i.0, name);
            }
        }
        format!("t{}", i.0)
    };
    // A named argument of a call prints as `limit: t7`; a positional one, and
    // every input of every other op, is unchanged.
    let arg_offset = term.op.arg_offset().unwrap_or(0);
    let inputs: Vec<String> = term
        .inputs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let rendered = fmt_input(t);
            match i
                .checked_sub(arg_offset)
                .and_then(|a| term.arg_names.get(a))
                .and_then(|n| n.as_ref())
            {
                Some(cid) => format!("{}: {}", kname(program, *cid), rendered),
                None => rendered,
            }
        })
        .collect();
    let blocks: Vec<String> = term
        .child_blocks
        .iter()
        .map(|b| format!("block{}", b.0))
        .collect();

    let blocks_str = if blocks.is_empty() {
        String::new()
    } else {
        format!(" -> {}", blocks.join(", "))
    };

    let name_str = match &term.name {
        // Overload variants carry the internal `box#1`; show the source name,
        // as the function and closure headers above already do.
        Some(name) => format!(" ; {}", base_fn_name(name)),
        None => String::new(),
    };

    // Compact source location: `@line:col`, prefixed with the file's display
    // name for non-entry files (`@std:21:1`).
    let loc_str = match program.source_map.get(term.id) {
        Some(span) if span.start.line > 0 => match program.source_map.file_name_for_span(span) {
            Some(file) => format!(" @{}:{}:{}", file, span.start.line, span.start.column),
            None => format!(" @{}:{}", span.start.line, span.start.column),
        },
        _ => String::new(),
    };

    writeln!(
        out,
        "  t{} r{} = {} [{}]{}{}{}",
        term.id.0,
        term.register.0,
        op_str,
        inputs.join(", "),
        blocks_str,
        name_str,
        loc_str,
    )
    .unwrap();

    // Match-arm metadata: one line per arm, pattern rendered source-like.
    if let Some(arms) = program.match_arms.get(&term.id) {
        for (i, arm) in arms.iter().enumerate() {
            let guard = match arm.guard_block {
                Some(gb) => format!(" if block{}", gb.0),
                None => String::new(),
            };
            writeln!(
                out,
                "    arm{}: when {}{} -> block{}",
                i,
                format_pattern(&arm.pattern),
                guard,
                arm.body_block.0
            )
            .unwrap();
        }
    }
}

/// State-op suffix: the state's name (from its named init/copy term) and the
/// key value, so a write visibly links to its init.
fn state_id(term: &Term, state_names: &HashMap<u64, String>) -> String {
    match term.state_key {
        Some(key) => {
            let key_str = format!("key={:#x}", key.0);
            match term
                .name
                .as_deref()
                .or_else(|| state_names.get(&key.0).map(|s| s.as_str()))
            {
                Some(name) => format!("({}, {})", name, key_str),
                None => format!("({})", key_str),
            }
        }
        None => String::new(),
    }
}

fn format_op(program: &Program, term: &Term, state_names: &HashMap<u64, String>) -> String {
    let op = &term.op;
    match op {
        TermOp::Constant(cid) => format!("Constant({})", kconst(program, *cid)),
        TermOp::Error(cid) => format!("Error({})", kconst(program, *cid)),
        TermOp::Add => "Add".into(),
        TermOp::Sub => "Sub".into(),
        TermOp::Mul => "Mul".into(),
        TermOp::Div => "Div".into(),
        TermOp::Mod => "Mod".into(),
        TermOp::Neg => "Neg".into(),
        TermOp::Eq => "Eq".into(),
        TermOp::Ne => "Ne".into(),
        TermOp::Lt => "Lt".into(),
        TermOp::Le => "Le".into(),
        TermOp::Gt => "Gt".into(),
        TermOp::Ge => "Ge".into(),
        TermOp::Not => "Not".into(),
        TermOp::And => "And".into(),
        TermOp::Or => "Or".into(),
        TermOp::Coalesce => "Coalesce".into(),
        TermOp::Concat => "Concat".into(),
        TermOp::Copy => "Copy".into(),
        TermOp::Phi => "Phi".into(),
        TermOp::Branch => "Branch".into(),
        TermOp::ForLoop => "ForLoop".into(),
        TermOp::NumericForLoop => "NumericForLoop".into(),
        TermOp::WhileLoop => "WhileLoop".into(),
        TermOp::Break => "Break".into(),
        TermOp::Continue => "Continue".into(),
        TermOp::Return => "Return".into(),
        TermOp::MakeClosure(fid) => format!("MakeClosure(fn{})", fid.0),
        TermOp::MakeOverloadSet => "MakeOverloadSet".into(),
        TermOp::Call => "Call".into(),
        TermOp::StateInit => format!("StateInit{}", state_id(term, state_names)),
        TermOp::StateRead => format!("StateRead{}", state_id(term, state_names)),
        TermOp::StateWrite => format!("StateWrite{}", state_id(term, state_names)),
        TermOp::CellNew => "CellNew".into(),
        TermOp::CellRead => "CellRead".into(),
        TermOp::CellWrite => "CellWrite".into(),
        TermOp::AllocList => "AllocList".into(),
        TermOp::AllocMap { fields, class } => {
            let names: Vec<String> = fields.iter().map(|c| kname(program, *c)).collect();
            match class {
                Some(c) => format!(
                    "AllocMap{{{}}} class={}",
                    names.join(", "),
                    kname(program, *c)
                ),
                None => format!("AllocMap{{{}}}", names.join(", ")),
            }
        }
        TermOp::AllocMapSpread { entries } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|e| match e {
                    MapSpreadEntry::Spread(idx) => format!("...#{}", idx),
                    MapSpreadEntry::Named(cid, idx) => {
                        format!("{}=#{}", kname(program, *cid), idx)
                    }
                })
                .collect();
            format!("AllocMapSpread({})", parts.join(", "))
        }
        TermOp::GetField(cid) => format!("GetField(.{})", kname(program, *cid)),
        TermOp::GetFieldOpt(cid) => format!("GetFieldOpt(.{})", kname(program, *cid)),
        TermOp::SetField(cid) => format!("SetField(.{})", kname(program, *cid)),
        TermOp::GetIndex => "GetIndex".into(),
        TermOp::GetIndexOpt => "GetIndexOpt".into(),
        TermOp::SetIndex => "SetIndex".into(),
        TermOp::MakeEnumVariant(cid) => format!("MakeEnumVariant({})", kname(program, *cid)),
        TermOp::MethodCall { name, hint } => match hint {
            Some(h) => format!(
                "MethodCall(.{}, hint={})",
                kname(program, *name),
                kname(program, *h)
            ),
            None => format!("MethodCall(.{})", kname(program, *name)),
        },
        TermOp::BuiltinCall(cid) => format!("BuiltinCall({})", kconst(program, *cid)),
        TermOp::Match => "Match".into(),
        TermOp::AllocElement { tag, prop_keys } => {
            let keys: Vec<String> = prop_keys.iter().map(|c| kname(program, *c)).collect();
            if keys.is_empty() {
                format!("AllocElement({})", kname(program, *tag))
            } else {
                format!(
                    "AllocElement({}, props=[{}])",
                    kname(program, *tag),
                    keys.join(", ")
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// User-only JSON view (`show-ir --json --user-only`)
// ---------------------------------------------------------------------------

/// The Program JSON with builtin phantom terms, prelude/module terms, blocks,
/// functions, and prelude-only constants filtered out of the arrays. Ids are
/// preserved as-is (nothing is renumbered), which is why `constants.values`
/// becomes an id-keyed object here instead of an array. This is a debugging
/// VIEW — it is not loadable by `run --ir`, use the unfiltered
/// `show-ir --json` for the interchange format.
pub fn user_only_json(program: &Program) -> serde_json::Value {
    use serde_json::Value;

    let filter = IrFilter::new(program);
    let const_visible = filter.visible_constants(program);

    // Builtin phantoms (root block) go; the param/capture/self binding
    // phantoms of entry-file blocks stay — visible terms reference them, and
    // they carry the param/register mapping.
    let keep_term = |t: &Term| {
        !filter.term_is_prelude(t.id)
            && !(is_phantom(program, t) && t.block_id == program.root_block)
    };
    let kept_terms: std::collections::HashSet<u64> = program
        .terms
        .iter()
        .filter(|t| keep_term(t))
        .map(|t| t.id.0 as u64)
        .collect();

    let mut v = serde_json::to_value(program).expect("program serializes");
    let obj = v.as_object_mut().expect("program is a JSON object");

    if let Some(Value::Array(terms)) = obj.get_mut("terms") {
        terms.retain(|t| t["id"].as_u64().is_some_and(|id| kept_terms.contains(&id)));
    }
    if let Some(Value::Array(blocks)) = obj.get_mut("blocks") {
        blocks.retain(|b| {
            b["id"]
                .as_u64()
                .is_some_and(|id| !filter.block_is_prelude(BlockId(id as u32)))
        });
        // A kept block's ordered `terms` array may still list filtered terms
        // (the root block lists the prelude's top-level terms); drop those so
        // the view has no dangling ids.
        for b in blocks.iter_mut() {
            if let Some(Value::Array(terms)) = b.get_mut("terms") {
                terms.retain(|t| t.as_u64().is_some_and(|id| kept_terms.contains(&id)));
            }
        }
    }
    if let Some(Value::Array(functions)) = obj.get_mut("functions") {
        functions.retain(|f| {
            f["body_block"]
                .as_u64()
                .is_some_and(|b| !filter.block_is_prelude(BlockId(b as u32)))
        });
    }
    // Constants: an id-keyed object so filtering can't shift ids.
    if let Some(constants) = obj.get_mut("constants") {
        if let Some(Value::Array(values)) = constants.get_mut("values") {
            let kept: serde_json::Map<String, Value> = values
                .iter()
                .enumerate()
                .filter(|(i, _)| const_visible[*i])
                .map(|(i, val)| (i.to_string(), val.clone()))
                .collect();
            constants["values"] = Value::Object(kept);
        }
    }
    if let Some(source_map) = obj.get_mut("source_map")
        && let Some(Value::Object(spans)) = source_map.get_mut("term_spans")
    {
        spans.retain(|k, _| k.parse::<u64>().is_ok_and(|id| kept_terms.contains(&id)));
    }
    if let Some(Value::Object(arms)) = obj.get_mut("match_arms") {
        arms.retain(|k, _| k.parse::<u64>().is_ok_and(|id| kept_terms.contains(&id)));
    }

    v
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;
    use crate::program::ProgramId;

    fn compile(src: &str) -> Program {
        Env::new()
            .compile_program(ProgramId(0), src)
            .expect("test source compiles")
    }

    const STD_USER: &str = "print(sum([1, 2, 3]))";

    #[test]
    fn prelude_hidden_by_default() {
        let program = compile(STD_USER);
        let text = display_program(&program);
        // No prelude term lines (`; std::x` binding comments) — the only
        // `std::` allowed is the inline annotation on a hidden-input reference.
        assert!(
            !text.contains("; std::"),
            "prelude bindings leaked:\n{}",
            text
        );
        assert!(!text.contains("is_empty"), "prelude fn leaked:\n{}", text);
        assert!(
            !text.contains("\"reduce\""),
            "prelude-only constant leaked:\n{}",
            text
        );
        // User content is still there, with constants resolved inline.
        assert!(text.contains("BuiltinCall(\"print\")"), "{}", text);
        assert!(text.contains("Constant(3)"), "{}", text);
        // The reference to the hidden prelude binding is annotated by name.
        assert!(text.contains("(std::sum)]"), "{}", text);
    }

    #[test]
    fn all_restores_prelude_and_phantoms() {
        let program = compile(STD_USER);
        let text = display_program_with(&program, false);
        assert!(text.contains("std::sum"), "{}", text);
        assert!(text.contains("is_empty"), "{}", text);
        // Builtin phantom Copy terms print in the root block with --all.
        assert!(text.contains("Copy [] ; map"), "{}", text);
    }

    #[test]
    fn ids_stay_stable_when_hiding() {
        let program = compile(STD_USER);
        let hidden = display_program(&program);
        let full = display_program_with(&program, false);
        // The same user term line appears in both outputs (same ids/registers),
        // modulo tree ordering.
        let user_line = hidden
            .lines()
            .find(|l| l.contains("BuiltinCall(\"print\")"))
            .expect("print call rendered");
        assert!(full.contains(user_line), "{} not in full dump", user_line);
    }

    #[test]
    fn match_arms_and_guards_rendered() {
        let program = compile(
            "enum Shape\n  Circle(radius),\nend\n\
             let msg = match 5\n\
               when 0 -> \"zero\"\n\
               when n if n > 10 -> \"big\"\n\
               when [a, ...rest] -> \"list\"\n\
               when {x: xx} -> \"rec\"\n\
               when Circle(r) -> \"circle\"\n\
               when _ -> \"other\"\n\
             end\n\
             print(msg)",
        );
        let text = display_program(&program);
        assert!(text.contains("arm0: when 0 -> block"), "{}", text);
        assert!(text.contains("if block"), "{}", text);
        assert!(text.contains("when [a, ...rest] -> block"), "{}", text);
        assert!(text.contains("when {x: xx} -> block"), "{}", text);
        assert!(text.contains("when Circle(r) -> block"), "{}", text);
        assert!(text.contains("when _ -> block"), "{}", text);
        assert!(text.contains("(guard for t"), "{}", text);
        assert!(text.contains("arm1)"), "{}", text);
        // The pattern variable's binding phantom is named in the block header.
        assert!(text.contains("binds: n=t"), "{}", text);
    }

    #[test]
    fn phi_outs_rendered_with_dest_name() {
        let program = compile("let x = 1\nif x > 0 then\n  x = x + 10\nend\nprint(x)");
        let text = display_program(&program);
        assert!(text.contains("phi-out: t"), "{}", text);
        assert!(text.contains("(x)"), "{}", text);
    }

    #[test]
    fn constants_resolved_inline() {
        let program = compile("let r = {x: 1, y: 2}\nprint(r.x)\nlet e = <box width={3} />");
        let text = display_program(&program);
        assert!(text.contains("AllocMap{x, y}"), "{}", text);
        assert!(text.contains("GetField(.x)"), "{}", text);
        assert!(text.contains("Constant(1)"), "{}", text);
        assert!(
            text.contains("AllocElement(box, props=[width])"),
            "{}",
            text
        );
    }

    #[test]
    fn state_ops_show_identity() {
        let program = compile("state count = 0\ncount = count + 1\nprint(count)");
        let text = display_program(&program);
        assert!(text.contains("StateInit(count, key=0x"), "{}", text);
        assert!(text.contains("StateWrite(count, key=0x"), "{}", text);
    }

    #[test]
    fn block_headers_list_bindings() {
        let program = compile(
            "fn make_adder(n)\n  fn add(x)\n    if x > 100 then add(x - n) else x + n end\n  end\n  add\nend\nprint(make_adder(3)(5))",
        );
        let text = display_program(&program);
        assert!(text.contains("params: x=t"), "{}", text);
        assert!(text.contains("captures: n=t"), "{}", text);
        assert!(text.contains("self: add=t"), "{}", text);
        assert!(text.contains("(body of fn"), "{}", text);
    }

    #[test]
    fn term_lines_carry_source_locations() {
        let program = compile("let x = 41\nprint(x + 1)");
        let text = display_program(&program);
        assert!(text.contains("; x @1:"), "{}", text);
        assert!(text.contains("@2:"), "{}", text);
    }

    #[test]
    fn function_names_drop_arity_suffix() {
        let program = compile("fn f(a)\n  a\nend\nfn f(a, b)\n  a + b\nend\nprint(f(1))");
        let text = display_program(&program);
        let functions = text
            .split("=== Functions ===")
            .nth(1)
            .unwrap()
            .split("=== Blocks ===")
            .next()
            .unwrap();
        assert!(functions.contains("fn0: f "), "{}", functions);
        assert!(!functions.contains("f#"), "{}", functions);
        // Block headers use the stripped name too.
        assert!(text.contains("(body of fn0 f)"), "{}", text);
        assert!(text.contains("self: f=t"), "{}", text);
    }

    #[test]
    fn user_only_json_filters_noise_and_keeps_ids() {
        let program = compile(STD_USER);
        let full = serde_json::to_value(&program).unwrap();
        let view = user_only_json(&program);

        let full_terms = full["terms"].as_array().unwrap().len();
        let view_terms = view["terms"].as_array().unwrap();
        assert!(
            view_terms.len() < full_terms / 10,
            "expected a big cut: {} of {}",
            view_terms.len(),
            full_terms
        );
        // No prelude bindings, no builtin phantoms.
        assert!(
            view_terms
                .iter()
                .all(|t| t["name"].as_str().map_or(true, |n| !n.starts_with("std::"))),
        );
        // Ids preserved as-is: every kept term exists verbatim in the full dump.
        for t in view_terms {
            assert!(full["terms"].as_array().unwrap().contains(t));
        }
        // Prelude functions and blocks are gone; the root block stays.
        assert!(view["functions"].as_array().unwrap().is_empty());
        let root = view["root_block"].as_u64().unwrap();
        let blocks = view["blocks"].as_array().unwrap();
        assert!(blocks.iter().any(|b| b["id"].as_u64() == Some(root)));
        assert_eq!(blocks.len(), 1);
        // Constants: id-keyed map, prelude-only entries dropped.
        let consts = view["constants"]["values"].as_object().unwrap();
        assert!(
            consts
                .values()
                .any(|v| v.get("String").and_then(|s| s.as_str()) == Some("print"))
        );
        assert!(
            !consts
                .values()
                .any(|v| v.get("String").and_then(|s| s.as_str()) == Some("reduce"))
        );
        let full_consts = full["constants"]["values"].as_array().unwrap();
        for (k, val) in consts {
            let idx: usize = k.parse().unwrap();
            assert_eq!(&full_consts[idx], val, "constant id shifted");
        }
    }

    #[test]
    fn user_only_json_keeps_user_binding_phantoms() {
        let program = compile("fn double(x)\n  x * 2\nend\nprint(double(4))");
        let view = user_only_json(&program);
        let names: Vec<&str> = view["terms"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"x"), "param binding missing: {:?}", names);
        // Every input reference resolves within the view (no dangling ids).
        // `inputs` is omitted when empty (schema v0.2), so tolerate absence.
        let ids: std::collections::HashSet<u64> = view["terms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_u64().unwrap())
            .collect();
        for t in view["terms"].as_array().unwrap() {
            let Some(inputs) = t.get("inputs").and_then(|i| i.as_array()) else {
                continue;
            };
            for input in inputs {
                assert!(ids.contains(&input.as_u64().unwrap()), "dangling input");
            }
        }
        // Same for the kept blocks' terms arrays.
        for b in view["blocks"].as_array().unwrap() {
            let Some(terms) = b.get("terms").and_then(|t| t.as_array()) else {
                continue;
            };
            for t in terms {
                assert!(ids.contains(&t.as_u64().unwrap()), "dangling block term");
            }
        }
    }
}
