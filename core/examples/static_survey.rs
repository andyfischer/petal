//! What a program builds inside function bodies that could only ever come
//! out the same, counted from its text: the invariant expressions
//! (`petal::static_survey`), and what the static-value cache makes of them
//! (`petal::static_cache`, the `in_fn` module).
//!
//! ```text
//! cargo run --release --example static_survey -- <file.ptl>...
//! ```
//!
//! Nothing is run, so a script written for a host works here: it is
//! compiled without linking. A native its host would register is not
//! registered, so it counts as dynamic and the numbers are a floor on what
//! that host would see. How often each of these runs is the other half of
//! the question, and needs the program running: for the panel corpus that
//! is `core-libs/petal-ui/tests/static_in_functions.rs`.

use std::path::PathBuf;

use petal::backend::OptFlags;
use petal::env::Env;
use petal::static_survey::survey;

fn main() {
    let files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if files.is_empty() {
        eprintln!("usage: static_survey <file.ptl>...");
        std::process::exit(2);
    }
    println!("| file | functions | invariant expressions | terms in them | allocating terms | kept: slots | stretches | instructions | allocating instructions |");
    println!("|---|---|---|---|---|---|---|---|---|");
    let mut total = [0usize; 8];
    let mut failed = 0;
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else {
            failed += 1;
            continue;
        };
        let mut env = Env::new();
        env.set_echo(false);
        env.set_check_only(true);
        let Ok(pid) = env.load_program_diag(&source, Some(file)) else {
            failed += 1;
            continue;
        };
        let program = env.get_program(pid).expect("the program");
        let analysis = env.static_analysis(program);
        let found = survey(program, &analysis);
        let Ok(bc) = env.lower_with(program, OptFlags::default_on()) else {
            failed += 1;
            continue;
        };
        let (slots, stretches) = bc.statics.function_slots();
        let runs = &bc.statics.runs[bc.statics.root_runs..];
        let row = [
            program.functions.len(),
            found.sites.len(),
            found.sites.iter().map(|s| s.terms as usize).sum(),
            found.sites.iter().map(|s| s.allocations as usize).sum(),
            slots,
            stretches,
            runs.iter().map(|r| r.instructions).sum(),
            runs.iter().map(|r| r.allocations).sum(),
        ];
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            file.display(),
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            row[5],
            row[6],
            row[7]
        );
        for (t, r) in total.iter_mut().zip(row) {
            *t += r;
        }
    }
    println!(
        "| {} files ({failed} did not compile) | {} | {} | {} | {} | {} | {} | {} | {} |",
        files.len(),
        total[0],
        total[1],
        total[2],
        total[3],
        total[4],
        total[5],
        total[6],
        total[7]
    );
}
