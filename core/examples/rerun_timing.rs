//! What one run of a script costs when the stack is kept between runs, the
//! way a host runs a frame.
//!
//! ```text
//! cargo run --release --example rerun_timing -- <file.ptl> [<file.ptl>]...
//! ```
//!
//! `petal bench` starts every run from scratch, so it cannot see work a run
//! only does (or only skips) because an earlier run left something behind:
//! the per-run declaration of enum constant data is the case this was written
//! for. Each file is loaded once and run `RUNS` times on one stack, under the
//! default policy (`PETAL_POLICY` overrides it); the report is the minimum and
//! median time of a run.

use std::path::PathBuf;
use std::time::Instant;

use petal::env::Env;

const WARMUP: usize = 200;
const RUNS: usize = 20_000;

fn main() {
    let files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if files.is_empty() {
        eprintln!("usage: rerun_timing <file.ptl> [<file.ptl>]...");
        std::process::exit(2);
    }
    for file in files {
        let source = std::fs::read_to_string(&file).expect("script file");
        let mut env = Env::new();
        env.set_echo(false);
        let pid = env
            .load_program_diag(&source, Some(&file))
            .unwrap_or_else(|e| panic!("{e}"));
        let sid = env.create_stack(pid).unwrap();
        let run = |env: &mut Env| {
            env.reset_stack(sid).unwrap();
            let t = Instant::now();
            env.run(sid).unwrap_or_else(|e| panic!("{e}"));
            let elapsed = t.elapsed();
            env.take_output();
            elapsed
        };
        for _ in 0..WARMUP {
            run(&mut env);
        }
        let mut times: Vec<f64> = (0..RUNS).map(|_| run(&mut env).as_secs_f64() * 1e6).collect();
        times.sort_by(|a, b| a.total_cmp(b));
        println!(
            "{}: {RUNS} runs on one stack: min {:.2} µs, median {:.2} µs per run",
            file.display(),
            times[0],
            times[RUNS / 2],
        );
    }
}
