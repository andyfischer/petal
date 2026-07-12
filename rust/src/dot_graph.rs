//! DOT-format graph rendering of a program's dataflow IR.

use std::fmt::Write;

use crate::ir_display::is_phantom;
use crate::program::{Program, TermOp};

/// Generate a DOT-format graph representation of the program's dataflow.
pub fn program_to_dot(program: &Program, hide_phantoms: bool) -> String {
    let mut dot = String::new();
    writeln!(dot, "digraph dataflow {{").unwrap();
    writeln!(dot, "  rankdir=TB;").unwrap();
    writeln!(
        dot,
        "  node [shape=box, fontname=\"monospace\", fontsize=10];"
    )
    .unwrap();
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

        writeln!(
            dot,
            "  t{} [label=\"{}\", style=filled, fillcolor={}];",
            term.id.0, label, color
        )
        .unwrap();

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
                writeln!(
                    dot,
                    "  t{} -> t{} [style=dashed, color=gray];",
                    term.id.0, entry.0
                )
                .unwrap();
            }
        }
    }

    writeln!(dot, "}}").unwrap();
    dot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::register_builtins;
    use crate::compiler::Compiler;
    use crate::native_fn::NativeFnTable;
    use crate::program::ProgramId;
    use crate::source_map::ENTRY_FILE;

    fn compile(source: &str) -> Program {
        let (_, stmts) = crate::cst::parse_source(source, ENTRY_FILE).expect("parse");
        let mut natives = NativeFnTable::new();
        register_builtins(&mut natives);
        Compiler::new().compile(&stmts, source.to_string(), ProgramId(0), &natives)
    }

    #[test]
    fn wraps_output_in_digraph_block() {
        let dot = program_to_dot(&compile("let x = 1"), true);
        assert!(dot.starts_with("digraph dataflow {"));
        assert!(dot.trim_end().ends_with('}'));
    }

    #[test]
    fn emits_term_nodes_and_dataflow_edges() {
        let dot = program_to_dot(&compile("let a = 1\nlet b = a + 1"), true);
        assert!(dot.contains("(Add)"), "expected an Add term node:\n{dot}");
        // At least one dataflow edge "tN -> tM;" (not a dashed control edge).
        assert!(
            dot.lines()
                .any(|l| l.contains("->") && !l.contains("dashed")),
            "expected a dataflow edge:\n{dot}"
        );
    }

    #[test]
    fn colors_state_terms() {
        let dot = program_to_dot(&compile("state x = 0"), true);
        assert!(dot.contains("lightyellow"), "expected state color:\n{dot}");
    }

    #[test]
    fn hide_phantoms_drops_phantom_nodes() {
        let program = compile("let a = 1\nlet b = a + 1");
        let with_phantoms = program_to_dot(&program, false);
        let without_phantoms = program_to_dot(&program, true);
        let count = |dot: &str| dot.lines().filter(|l| l.contains("[label=")).count();
        assert!(
            count(&with_phantoms) > count(&without_phantoms),
            "hiding phantoms should remove nodes: {} vs {}",
            count(&with_phantoms),
            count(&without_phantoms)
        );
    }
}
