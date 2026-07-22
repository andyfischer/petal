//! Source → rendered text for each compilation stage — the stable embedding
//! surface for tools that want to show what a Petal program *compiles to*
//! (Garden's IR inspector is the first user).
//!
//! Every renderer here is the same one the `show-*` CLI subcommands use (see
//! [`crate::cli`]'s `handle_show_*` handlers); this module just packages them
//! behind a `source -> String` API so an embedder never has to reach into
//! private [`Env`] internals or reproduce the fiddly bytecode opt-flag lowering.
//! Each call compiles independently and retains nothing.

use crate::backend::bytecode::{
    analyze_escapes, apply_last_use, disasm, lower_program_opt, InPlaceSet,
};
use crate::env::Env;
use crate::ir_display::display_program_with;
use crate::program::ProgramId;
use crate::source_map::ENTRY_FILE;

/// One inspectable representation of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// The parsed statement tree (`Vec<Stmt>`, rendered with `{:#?}`).
    Ast,
    /// The term-graph IR ([`display_program_with`]), phantom builtin `Copy`
    /// terms hidden — the primary "IR" view.
    Ir,
    /// The lowered bytecode the VM executes ([`disasm::render_text`]), using the
    /// same opt-flag lowering a real run would.
    Bytecode,
}

/// The stages a menu can offer, each with a short human label, in display order.
/// The label is what [`stage_from_label`] resolves (case-insensitively).
pub fn stages() -> &'static [(&'static str, Stage)] {
    &[
        ("IR", Stage::Ir),
        ("Bytecode", Stage::Bytecode),
        ("AST", Stage::Ast),
    ]
}

/// Resolve a stage label (as returned by [`stages`], case-insensitive) to a
/// [`Stage`]. `None` for an unknown label.
pub fn stage_from_label(label: &str) -> Option<Stage> {
    stages()
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(label))
        .map(|(_, stage)| *stage)
}

/// Render `source` at `stage` to display text, or `Err` with the compile/parse
/// error message (which a caller can show in place of the text rather than
/// treating it as fatal).
pub fn render(source: &str, stage: Stage) -> Result<String, String> {
    match stage {
        Stage::Ast => render_ast(source),
        Stage::Ir => render_ir(source),
        Stage::Bytecode => render_bytecode(source),
    }
}

fn render_ast(source: &str) -> Result<String, String> {
    let (_tree, stmts) = crate::cst::parse_source(source, ENTRY_FILE)?;
    let mut out = String::new();
    for stmt in &stmts {
        out.push_str(&format!("{stmt:#?}\n"));
    }
    Ok(out)
}

fn render_ir(source: &str) -> Result<String, String> {
    let program = Env::new().compile_program(ProgramId(0), source)?;
    Ok(display_program_with(&program, true))
}

fn render_bytecode(source: &str) -> Result<String, String> {
    let program = Env::new().compile_program(ProgramId(0), source)?;
    // Mirror the runtime defaults so the disassembly shows the opcodes a run
    // would actually execute (see `cli::handlers::handle_show_bytecode`).
    let flags = Env::opt_flags_from_env();
    let in_place = if flags.in_place_mutation {
        analyze_escapes(&program)
    } else {
        InPlaceSet::default()
    };
    let mut bc = lower_program_opt(&program, &in_place)
        .map_err(|e| format!("bytecode lowering failed: {e}"))?;
    if flags.in_place_straight_line {
        apply_last_use(&mut bc, &program);
    }
    Ok(disasm::render_text(&bc, &program))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNIPPET: &str = "let a = 1\nlet b = a + 2\nprint(b)\n";

    #[test]
    fn renders_every_stage_to_nonempty_text() {
        for (_, stage) in stages() {
            let out = render(SNIPPET, *stage)
                .unwrap_or_else(|e| panic!("stage {stage:?} failed: {e}"));
            assert!(!out.trim().is_empty(), "stage {stage:?} rendered empty");
        }
    }

    #[test]
    fn ir_mentions_the_add_op() {
        let ir = render(SNIPPET, Stage::Ir).unwrap();
        assert!(ir.contains("Add"), "IR should show the Add term:\n{ir}");
    }

    #[test]
    fn labels_round_trip() {
        for (label, stage) in stages() {
            assert_eq!(stage_from_label(label), Some(*stage));
            assert_eq!(stage_from_label(&label.to_lowercase()), Some(*stage));
        }
        assert_eq!(stage_from_label("nonsense"), None);
    }

    #[test]
    fn a_syntax_error_is_reported_not_panicked() {
        // Unbalanced paren — compilation/parse should return Err, not panic.
        let err = render("let a = (1 + \n", Stage::Ir);
        assert!(err.is_err(), "expected a compile error, got {err:?}");
    }
}
