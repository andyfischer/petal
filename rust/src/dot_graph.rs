//! DOT-format graph rendering of a program's dataflow IR.

use std::fmt::Write;

use crate::ir_display::is_phantom;
use crate::program::{Program, TermOp};

/// Generate a DOT-format graph representation of the program's dataflow.
pub fn program_to_dot(program: &Program, hide_phantoms: bool) -> String {
    let mut dot = String::new();
    writeln!(dot, "digraph dataflow {{").unwrap();
    writeln!(dot, "  rankdir=TB;").unwrap();
    writeln!(dot, "  node [shape=box, fontname=\"monospace\", fontsize=10];").unwrap();
    writeln!(dot, "  edge [fontname=\"monospace\", fontsize=8];").unwrap();

    for term in &program.terms {
        if hide_phantoms && is_phantom(program, term) {
            continue;
        }
        let label = if let Some(ref name) = term.name {
            format!("t{}: {} ({:?})", term.id.0, name, term.op)
        } else {
            format!("t{}: {:?}", term.id.0, term.op)
        };
        // Escape quotes in label
        let label = label.replace('"', "\\\"");

        // Color by operation type
        let color = match &term.op {
            TermOp::Constant(_) => "lightblue",
            TermOp::StateInit | TermOp::StateRead | TermOp::StateWrite => "lightyellow",
            TermOp::Call | TermOp::MethodCall(_) => "lightgreen",
            TermOp::Branch | TermOp::Match => "lightsalmon",
            TermOp::ForLoop | TermOp::NumericForLoop | TermOp::WhileLoop => "plum",
            TermOp::MakeClosure(_) => "lightcoral",
            _ => "white",
        };

        writeln!(dot, "  t{} [label=\"{}\", style=filled, fillcolor={}];",
            term.id.0, label, color).unwrap();

        // Dataflow edges (input -> term). Skip edges referencing phantom
        // builtins so the rendered graph matches the visible nodes.
        for input_id in &term.inputs {
            if hide_phantoms && is_phantom(program, program.get_term(*input_id)) {
                continue;
            }
            writeln!(dot, "  t{} -> t{};", input_id.0, term.id.0).unwrap();
        }

        // Control flow edges (term -> child blocks, dashed)
        for child_block in &term.child_blocks {
            let block = program.get_block(*child_block);
            if let Some(entry) = block.entry {
                writeln!(dot, "  t{} -> t{} [style=dashed, color=gray];",
                    term.id.0, entry.0).unwrap();
            }
        }
    }

    writeln!(dot, "}}").unwrap();
    dot
}
