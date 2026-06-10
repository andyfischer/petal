//! Human-readable text formatting for the IR.

use std::fmt::Write;

use crate::constant_table::ConstantValue;
use crate::program::{MapSpreadEntry, Program, Term, TermOp};

/// Returns true for synthetic builtin terms that have no source location.
/// These flood the output of `show-ir` / `show-graph` on small programs (the
/// builtin function table alone is ~70 phantom Copy terms), so display
/// helpers hide them by default.
pub fn is_phantom(program: &Program, term: &Term) -> bool {
    if !matches!(term.op, TermOp::Copy) || !term.inputs.is_empty() || term.name.is_none() {
        return false;
    }
    match program.source_map.get(term.id) {
        Some(span) => span.start.line == 0,
        None => true,
    }
}

pub fn display_program(program: &Program) -> String {
    display_program_with(program, true)
}

pub fn display_program_with(program: &Program, hide_phantoms: bool) -> String {
    let mut out = String::new();

    // === Constants ===
    writeln!(out, "=== Constants ===").unwrap();
    for (i, val) in program.constants.values().iter().enumerate() {
        let val_str = match val {
            ConstantValue::Nil => "nil".to_string(),
            ConstantValue::Bool(b) => b.to_string(),
            ConstantValue::Int(n) => n.to_string(),
            ConstantValue::Float(bits) => format!("{}", f64::from_bits(*bits)),
            ConstantValue::String(s) => format!("{:?}", s),
        };
        writeln!(out, "  c{}: {}", i, val_str).unwrap();
    }
    writeln!(out).unwrap();

    // === Functions ===
    writeln!(out, "=== Functions ===").unwrap();
    for func in &program.functions {
        let name = func.name.as_deref().unwrap_or("<lambda>");
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

    // === Blocks ===
    writeln!(out, "=== Blocks ===").unwrap();
    for block in &program.blocks {
        let label = if block.id == program.root_block {
            " [root]".to_string()
        } else if let Some(parent) = block.parent_term_id {
            format!(" (parent: t{})", parent.0)
        } else {
            String::new()
        };

        let params_str = if block.param_names.is_empty() {
            String::new()
        } else {
            format!(" params={:?}", block.param_names)
        };

        writeln!(
            out,
            "block{}{}{} regs={}",
            block.id.0, label, params_str, block.register_count,
        )
        .unwrap();

        // Walk terms via linked list
        let mut tid = block.entry;
        while let Some(t) = tid {
            let term = program.get_term(t);
            if hide_phantoms && is_phantom(program, term) {
                tid = term.block_next;
                continue;
            }
            let op_str = format_op(&term.op);
            let inputs: Vec<String> = term.inputs.iter().map(|i| format!("t{}", i.0)).collect();
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

            let name_str = if let Some(ref name) = term.name {
                format!(" ; {}", name)
            } else {
                String::new()
            };

            writeln!(
                out,
                "  t{} r{} = {} [{}]{}{}",
                term.id.0,
                term.register.0,
                op_str,
                inputs.join(", "),
                blocks_str,
                name_str,
            )
            .unwrap();

            tid = term.block_next;
        }
        writeln!(out).unwrap();
    }

    out
}

fn format_op(op: &TermOp) -> String {
    match op {
        TermOp::Constant(cid) => format!("Constant(c{})", cid.0),
        TermOp::Error(cid) => format!("Error(c{})", cid.0),
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
        TermOp::StateInit => "StateInit".into(),
        TermOp::StateRead => "StateRead".into(),
        TermOp::StateWrite => "StateWrite".into(),
        TermOp::AllocList => "AllocList".into(),
        TermOp::AllocMap { fields } => {
            let ids: Vec<String> = fields.iter().map(|c| format!("c{}", c.0)).collect();
            format!("AllocMap({})", ids.join(", "))
        }
        TermOp::AllocMapSpread { entries } => {
            let parts: Vec<String> = entries.iter().map(|e| match e {
                MapSpreadEntry::Spread(idx) => format!("...#{}", idx),
                MapSpreadEntry::Named(cid, idx) => format!("c{}=#{}", cid.0, idx),
            }).collect();
            format!("AllocMapSpread({})", parts.join(", "))
        }
        TermOp::GetField(cid) => format!("GetField(c{})", cid.0),
        TermOp::SetField(cid) => format!("SetField(c{})", cid.0),
        TermOp::GetIndex => "GetIndex".into(),
        TermOp::SetIndex => "SetIndex".into(),
        TermOp::MakeEnumVariant(cid) => format!("MakeEnumVariant(c{})", cid.0),
        TermOp::MethodCall(cid) => format!("MethodCall(c{})", cid.0),
        TermOp::Match => "Match".into(),
        TermOp::AllocElement { tag, prop_keys } => {
            let keys: Vec<String> = prop_keys.iter().map(|c| format!("c{}", c.0)).collect();
            if keys.is_empty() {
                format!("AllocElement(c{})", tag.0)
            } else {
                format!("AllocElement(c{}, props=[{}])", tag.0, keys.join(", "))
            }
        }
    }
}
