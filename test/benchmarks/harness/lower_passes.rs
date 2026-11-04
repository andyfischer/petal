//! What each optimizer pass costs to lower a real program.
//!
//! ```text
//! cargo run --release --example lower_passes -- <entry.ptl> [rounds]
//! ```
//!
//! Loads the program without linking (so a game whose host is not here still
//! loads), then lowers it with no passes, with each pass alone and with every
//! pass, and prints the minimum time of `rounds` lowerings (default 9) for
//! each, with the static instruction count of the code it produced. This is
//! the table in docs/dev/performance.md, "Lowering cost".
//! Nothing is run.

#![allow(clippy::disallowed_methods, reason = "examples run natively")]

use std::path::PathBuf;
use std::time::Instant;

use petal::backend::OptFlags;
use petal::env::Env;

fn main() {
    let mut args = std::env::args().skip(1);
    let entry = PathBuf::from(args.next().expect("usage: lower_passes <entry.ptl> [rounds]"));
    let rounds: usize = args.next().map_or(9, |n| n.parse().expect("rounds is a number"));
    let source = std::fs::read_to_string(&entry).expect("entry file");

    let mut env = Env::new();
    env.set_echo(false);
    env.set_check_only(true);
    let pid = env
        .load_program_diag(&source, Some(&entry))
        .unwrap_or_else(|e| panic!("{e}"));
    let program = env.get_program(pid).unwrap();
    let lines: usize = if program.source_map.files.is_empty() {
        program.source.lines().count()
    } else {
        program.source_map.files.iter().map(|f| f.source.lines().count()).sum()
    };
    println!(
        "{}: {} files, {lines} lines, {} terms (min of {rounds})",
        entry.display(),
        program.source_map.files.len().max(1),
        program.terms.len()
    );

    let none = OptFlags::none();
    let all = OptFlags::default_on();
    let rows: [(&str, OptFlags); 9] = [
        ("no passes", none),
        ("escape analysis only (in_place_mutation)", OptFlags { in_place_mutation: true, ..none }),
        ("last-use rewriting only (in_place_straight_line)", OptFlags { in_place_straight_line: true, ..none }),
        ("the cell plan only (in_place_cells)", OptFlags { in_place_cells: true, ..none }),
        ("copy propagation only", OptFlags { copy_propagation: true, ..none }),
        ("match dispatch only", OptFlags { match_dispatch: true, ..none }),
        ("intrinsics only", OptFlags { intrinsics: true, ..none }),
        ("all passes, static cache off", OptFlags { static_cache: false, ..all }),
        ("all passes (the default)", all),
    ];
    for (label, flags) in rows {
        let mut best = f64::INFINITY;
        let mut insts = 0;
        for _ in 0..rounds {
            let t = Instant::now();
            let bc = env.lower_with(program, flags).unwrap_or_else(|e| panic!("{label}: {e}"));
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
            insts = bc.root.code.len() + bc.fns.iter().map(|f| f.code.len()).sum::<usize>();
        }
        println!("{label:<50} {best:>9.2} ms {insts:>9} instructions");
    }
}
