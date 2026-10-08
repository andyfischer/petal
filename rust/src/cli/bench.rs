//! `petal bench <file> --fn <name>...` — what a call of a named function
//! costs, measured over the calls the script itself makes.
//!
//! The file is run the way `petal run` runs it, again and again until a time
//! budget is spent, with a [`CallBench`] installed on each run's `Env` (see
//! `crate::profile`). Every run is a fresh `Env`, so each one starts from the
//! same empty `state` and an empty memo table, exactly as a `petal run`
//! would. The whole measurement is then repeated with the optimizer off, and
//! the report sets the two side by side.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::backend::OptFlags;
use crate::policy::RunPolicy;
use crate::profile::{CallBench, CallStats, commas};
use crate::program::{FunctionDef, FunctionId, Program, TermOp, base_fn_name};

use super::handlers::{eprint_warnings, load_into, make_env};
use super::{BenchOpts, SourceInput, die, die_error, print_json};

/// How long each of the two measurements (optimizer on, optimizer off) keeps
/// re-running the file when `--iters` does not pin the count.
const BUDGET: Duration = Duration::from_secs(1);

/// One function a `--fn` name resolved to.
struct Target {
    /// The `--fn` argument that selected it.
    query: String,
    func: FunctionId,
    /// Source name, without the internal `#arity` overload suffix.
    name: String,
    /// Where it is declared: the file (`None` for the entry file) and line.
    file: Option<String>,
    line: Option<u32>,
}

/// One of the two measurements.
struct Variant {
    policy: RunPolicy,
    /// Measured runs (the warm-up is not one of them).
    runs: u32,
    /// Wall time of each measured run's execution, with compiling and
    /// lowering excluded.
    run_ns: Vec<u64>,
    /// Wall time of each measured run's lowering to bytecode — the step the
    /// optimizer's passes run in, so the one the two variants differ by
    /// before a single instruction executes.
    lower_ns: Vec<u64>,
    /// Instructions one run retired (the last one's; they agree unless the
    /// script is nondeterministic).
    run_insts: u64,
    bench: CallBench,
}

pub(super) fn handle_bench(
    opts: &BenchOpts,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
) {
    let json = opts.json;
    if opts.fns.is_empty() {
        die(
            json,
            "bench needs at least one function to measure: petal bench <file> --fn <name>",
            "bench",
        );
    }

    // Compile once up front: to report load errors and warnings the way `run`
    // does, and to turn the `--fn` names into function ids. Ids are assigned
    // by the compiler in source order, so they are the same in every later
    // compile of the same source.
    let mut probe = make_env(include_dirs);
    let pid = match load_into(&mut probe, source, source_input) {
        Ok(pid) => pid,
        Err(e) => die_error(json, &e, serde_json::Value::Null, source),
    };
    let program = probe.get_program(pid).expect("just loaded");
    eprint_warnings(program);
    let targets = match resolve_targets(program, &opts.fns) {
        Ok(t) => t,
        Err(e) => die(json, &e, "bench"),
    };
    let funcs: Vec<FunctionId> = targets.iter().map(|t| t.func).collect();
    let ambient = probe.policy();
    drop(probe);

    // The two sides differ in the optimizer and nothing else. The rest of the
    // policy is the ambient one (`PETAL_POLICY`), but an ambient policy that
    // turns the optimizer off (`PETAL_OPT=off`, `PETAL_POLICY=baseline`)
    // cannot be the "opt" side: that would compare the baseline with itself.
    let base = if ambient.opts == OptFlags::none().preserving(ambient.opts) {
        ambient.with_opts(OptFlags::default_on().preserving(ambient.opts))
    } else {
        ambient
    };
    let no_opt = base.with_opts(OptFlags::none());
    let measure = |policy: RunPolicy| {
        match run_variant(policy, opts, source, source_input, include_dirs, &funcs) {
            Ok(v) => v,
            Err(e) => die(json, &e, "runtime"),
        }
    };
    let opt = measure(base);
    let noopt = measure(no_opt);
    let overhead_ns = CallBench::timer_overhead_ns();

    let file = match source_input {
        SourceInput::File(path) => path.clone(),
        SourceInput::Inline(_) => "-e".to_string(),
    };
    if json {
        print_json(&report_json(&file, &targets, &opt, &noopt, overhead_ns));
    } else {
        print!("{}", report_text(&file, &targets, &opt, &noopt, overhead_ns));
    }
}

/// Resolve each `--fn` name to the functions it names: every function whose
/// source name is exactly that, plus every method of that name (`area`
/// selects `Rect.area`; `Rect.area` selects only it). A name that selects
/// nothing is an error naming the functions there are.
fn resolve_targets(program: &Program, names: &[String]) -> Result<Vec<Target>, String> {
    let mut targets: Vec<Target> = Vec::new();
    for query in names {
        let before = targets.len();
        for def in &program.functions {
            let Some(name) = def.name.as_deref().map(base_fn_name) else {
                continue;
            };
            let method = crate::classes::split_qualified_method_name(name).map(|(_, m)| m);
            if name != query && method != Some(query.as_str()) {
                continue;
            }
            if targets.iter().any(|t| t.func == def.id) {
                continue;
            }
            let (file, line) = fn_location(program, def);
            targets.push(Target {
                query: query.clone(),
                func: def.id,
                name: name.to_string(),
                file,
                line,
            });
        }
        if targets.len() == before && !targets.iter().any(|t| &t.query == query) {
            let mut known: Vec<&str> = program
                .functions
                .iter()
                .filter_map(|d| d.name.as_deref().map(base_fn_name))
                .collect();
            known.sort_unstable();
            known.dedup();
            let hint = if known.is_empty() {
                "the program defines no named functions".to_string()
            } else {
                let more = known.len().saturating_sub(20);
                let mut list = known.into_iter().take(20).collect::<Vec<_>>().join(", ");
                if more > 0 {
                    list.push_str(&format!(", ... ({more} more)"));
                }
                format!("functions defined: {list}")
            };
            return Err(format!("no function named '{query}' ({hint})"));
        }
    }
    Ok(targets)
}

/// The file and line a function is declared at: the position of the term
/// that creates it (its `fn` keyword), or, for a function no term in the
/// program creates, the first term of its body that has a source position.
fn fn_location(program: &Program, def: &FunctionDef) -> (Option<String>, Option<u32>) {
    let located = |t: crate::program::TermId| program.source_map.get(t).filter(|s| s.start.line > 0);
    let span = program
        .terms
        .iter()
        .find(|t| matches!(t.op, TermOp::MakeClosure(f) if f == def.id))
        .and_then(|t| located(t.id))
        .or_else(|| {
            program
                .block_terms
                .get(&def.body_block)
                .and_then(|terms| terms.iter().find_map(|&t| located(t)))
        });
    match span {
        Some(span) => (
            program
                .source_map
                .file_name_for_span(span)
                .map(|f| f.rsplit('/').next().unwrap_or(f).to_string()),
            Some(span.start.line as u32),
        ),
        None => (None, None),
    }
}

/// Run the file under `policy`: one warm-up run that is thrown away, then
/// `--iters` runs, or as many as fit in [`BUDGET`] (always at least one).
fn run_variant(
    policy: RunPolicy,
    opts: &BenchOpts,
    source: &str,
    source_input: &SourceInput,
    include_dirs: &[PathBuf],
    funcs: &[FunctionId],
) -> Result<Variant, String> {
    let mut v = Variant {
        policy,
        runs: 0,
        run_ns: Vec::new(),
        lower_ns: Vec::new(),
        run_insts: 0,
        bench: CallBench::new(funcs),
    };
    let one_run = |v: &mut Variant| -> Result<(u64, u64, u64), String> {
        let mut env = make_env(include_dirs);
        env.set_policy(policy);
        // The script's prints would otherwise repeat once per run.
        env.set_echo(false);
        env.set_heap_stats(true);
        if let Some(seed) = opts.seed {
            env.set_seed(seed);
        }
        let pid = load_into(&mut env, source, source_input).map_err(|e| e.to_string())?;
        let sid = env.create_stack(pid)?;
        // A run lowers the program on its first step; do it here instead so
        // its cost is reported on its own rather than inside the run's.
        let started = Instant::now();
        env.ensure_bytecode(pid)?;
        let lower_ns = started.elapsed().as_nanos() as u64;
        env.profile_mut().bench = std::mem::take(&mut v.bench);
        let started = Instant::now();
        let result = env.run(sid);
        let ns = started.elapsed().as_nanos() as u64;
        v.bench = std::mem::take(&mut env.profile_mut().bench);
        result?;
        Ok((ns, lower_ns, env.stack(sid).map_or(0, |s| s.insts)))
    };

    one_run(&mut v)?;
    v.bench.clear();
    let started = Instant::now();
    loop {
        let (ns, lower_ns, insts) = one_run(&mut v)?;
        v.runs += 1;
        v.run_ns.push(ns);
        v.lower_ns.push(lower_ns);
        v.run_insts = insts;
        let done = match opts.iters {
            Some(n) => v.runs >= n,
            None => started.elapsed() >= BUDGET,
        };
        if done {
            return Ok(v);
        }
    }
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

/// One function's per-call figures under one variant. Everything per-call is
/// `None` when the function was never called.
struct Row {
    calls: u64,
    outer_calls: u64,
    replayed: u64,
    calls_per_run: f64,
    insts: Option<f64>,
    insts_self: Option<f64>,
    insts_min: Option<u64>,
    insts_max: Option<u64>,
    ms: Option<f64>,
    ms_self: Option<f64>,
    ms_min: Option<f64>,
    ms_median: Option<f64>,
    ms_p95: Option<f64>,
    ms_max: Option<f64>,
    allocs: Option<f64>,
    copies: Option<f64>,
    copy_bytes: Option<f64>,
    collections: Option<f64>,
    gc_ms: Option<f64>,
    /// The percentiles are over a sample of the calls, not all of them.
    sampled: bool,
}

const NS_PER_MS: f64 = 1e6;

impl Row {
    fn new(s: &CallStats, runs: u32) -> Row {
        // Inclusive figures are per outermost call, self figures per call —
        // see `CallStats`.
        let outer = (s.outer_calls > 0).then_some(s.outer_calls as f64);
        let all = (s.calls > 0).then_some(s.calls as f64);
        let per_outer = |total: u64| outer.map(|n| total as f64 / n);
        let ms = |ns: Option<u64>| ns.map(|ns| ns as f64 / NS_PER_MS);
        Row {
            calls: s.calls,
            outer_calls: s.outer_calls,
            replayed: s.replayed,
            calls_per_run: s.calls as f64 / runs.max(1) as f64,
            insts: per_outer(s.incl_insts),
            insts_self: all.map(|n| s.self_insts as f64 / n),
            insts_min: outer.map(|_| s.min_insts),
            insts_max: outer.map(|_| s.max_insts),
            ms: per_outer(s.incl_ns).map(|ns| ns / NS_PER_MS),
            ms_self: all.map(|n| s.self_ns as f64 / n / NS_PER_MS),
            ms_min: ms(outer.map(|_| s.min_ns)),
            ms_median: ms(s.quantile_ns(0.5)),
            ms_p95: ms(s.quantile_ns(0.95)),
            ms_max: ms(outer.map(|_| s.max_ns)),
            allocs: per_outer(s.allocs),
            copies: per_outer(s.copies),
            copy_bytes: per_outer(s.copy_bytes),
            collections: per_outer(s.collections),
            gc_ms: per_outer(s.gc_ns).map(|ns| ns / NS_PER_MS),
            sampled: s.sampled(),
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "calls": self.calls,
            "calls_per_run": self.calls_per_run,
            "outermost_calls": self.outer_calls,
            "replayed_calls": self.replayed,
            "instructions_per_call": {
                "inclusive": self.insts,
                "self": self.insts_self,
                "min": self.insts_min,
                "max": self.insts_max,
            },
            "ms_per_call": {
                "inclusive": self.ms,
                "self": self.ms_self,
                "min": self.ms_min,
                "median": self.ms_median,
                "p95": self.ms_p95,
                "max": self.ms_max,
                "percentiles_sampled": self.sampled,
            },
            "allocations_per_call": self.allocs,
            "copies_per_call": self.copies,
            "copied_bytes_per_call": self.copy_bytes,
            "gc_collections_per_call": self.collections,
            "gc_ms_per_call": self.gc_ms,
        })
    }
}

/// The optimizer's effect on one figure, in percent: how much lower (negative)
/// or higher the optimized value is than the unoptimized one. `None` when
/// either side is missing or the unoptimized value is zero.
fn delta_pct(opt: Option<f64>, noopt: Option<f64>) -> Option<f64> {
    match (opt, noopt) {
        (Some(a), Some(b)) if b != 0.0 => Some((a - b) * 100.0 / b),
        _ => None,
    }
}

fn median_ns(ns: &[u64]) -> u64 {
    let mut sorted = ns.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or(0)
}

fn variant_json(v: &Variant) -> serde_json::Value {
    serde_json::json!({
        "policy": v.policy.name(),
        "runs": v.runs,
        "ms_per_run": {
            "min": v.run_ns.iter().min().map(|&ns| ns as f64 / NS_PER_MS),
            "median": median_ns(&v.run_ns) as f64 / NS_PER_MS,
        },
        "lower_ms": median_ns(&v.lower_ns) as f64 / NS_PER_MS,
        "instructions_per_run": v.run_insts,
    })
}

fn report_json(
    file: &str,
    targets: &[Target],
    opt: &Variant,
    noopt: &Variant,
    overhead_ns: f64,
) -> serde_json::Value {
    let functions: Vec<serde_json::Value> = targets
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let a = Row::new(&opt.bench.stats()[i], opt.runs);
            let b = Row::new(&noopt.bench.stats()[i], noopt.runs);
            serde_json::json!({
                "name": t.name,
                "query": t.query,
                "file": t.file,
                "line": t.line,
                "opt": a.json(),
                "no_opt": b.json(),
                // Optimized relative to unoptimized, in percent.
                "delta_pct": {
                    "instructions_per_call": delta_pct(a.insts, b.insts),
                    "ms_per_call": delta_pct(a.ms, b.ms),
                    "allocations_per_call": delta_pct(a.allocs, b.allocs),
                    "copies_per_call": delta_pct(a.copies, b.copies),
                },
            })
        })
        .collect();
    serde_json::json!({
        "file": file,
        "host": "core",
        "runs": {
            "opt": variant_json(opt),
            "no_opt": variant_json(noopt),
        },
        "timer_overhead_ns": overhead_ns,
        "functions": functions,
    })
}

/// A count or a per-call average: whole numbers get thousands separators,
/// fractional ones as many decimals as their size leaves meaningful.
fn num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return commas(v as u64);
    }
    if v >= 100.0 {
        let whole = v.trunc();
        format!("{}.{}", commas(whole as u64), ((v - whole) * 10.0) as u64)
    } else if v >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.4}")
    }
}

/// Milliseconds, down to nanosecond resolution for the small ones.
fn ms_text(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.1}")
    } else if v >= 1.0 {
        format!("{v:.3}")
    } else {
        format!("{v:.6}")
    }
}

fn report_text(
    file: &str,
    targets: &[Target],
    opt: &Variant,
    noopt: &Variant,
    overhead_ns: f64,
) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "bench {file}");
    for (label, v) in [("opt", opt), ("no-opt", noopt)] {
        let _ = writeln!(
            s,
            "  {:<7} {:>4} run{}  {:>10} ms/run  {:>14} instructions/run  {:>9} ms to lower  \
             (policy {})",
            label,
            v.runs,
            if v.runs == 1 { " " } else { "s" },
            ms_text(median_ns(&v.run_ns) as f64 / NS_PER_MS),
            commas(v.run_insts),
            ms_text(median_ns(&v.lower_ns) as f64 / NS_PER_MS),
            v.policy.name().unwrap_or_else(|| "custom".into()),
        );
    }
    let _ = writeln!(
        s,
        "  timing costs about {overhead_ns:.0} ns per timed call (each call of a benched \
         function, and each\n  user function it calls directly); that is inside the times \
         below, not subtracted"
    );

    for (i, t) in targets.iter().enumerate() {
        let a = Row::new(&opt.bench.stats()[i], opt.runs);
        let b = Row::new(&noopt.bench.stats()[i], noopt.runs);
        let at = match (&t.file, t.line) {
            (Some(f), Some(l)) => format!("  {f}:{l}"),
            (None, Some(l)) => format!("  line {l}"),
            _ => String::new(),
        };
        let _ = writeln!(s, "\nfn {}{}", t.name, at);
        if a.calls == 0 && b.calls == 0 {
            let replayed = if a.replayed > 0 {
                format!(" ({} calls were replayed from the memo)", commas(a.replayed))
            } else {
                String::new()
            };
            let _ = writeln!(s, "  never called{replayed}");
            continue;
        }
        let _ = writeln!(
            s,
            "  {:<22} {:>14} {:>14}   {}",
            "", "opt", "no-opt", "opt vs no-opt"
        );
        let mut line = |label: &str, x: Option<String>, y: Option<String>, d: Option<f64>| {
            let dash = || "-".to_string();
            let _ = writeln!(
                s,
                "  {:<22} {:>14} {:>14}   {}",
                label,
                x.unwrap_or_else(dash),
                y.unwrap_or_else(dash),
                d.map(|d| format!("{d:+.1}%")).unwrap_or_default()
            );
        };
        let n = |v: Option<f64>| v.map(num);
        let m = |v: Option<f64>| v.map(ms_text);
        line("calls/run", Some(num(a.calls_per_run)), Some(num(b.calls_per_run)), None);
        let recursive = a.outer_calls != a.calls || b.outer_calls != b.calls;
        if recursive {
            line(
                "  outermost/run",
                Some(num(a.outer_calls as f64 / opt.runs.max(1) as f64)),
                Some(num(b.outer_calls as f64 / noopt.runs.max(1) as f64)),
                None,
            );
        }
        if a.replayed > 0 || b.replayed > 0 {
            line(
                "  replayed/run",
                Some(num(a.replayed as f64 / opt.runs.max(1) as f64)),
                Some(num(b.replayed as f64 / noopt.runs.max(1) as f64)),
                None,
            );
        }
        line("instructions/call", n(a.insts), n(b.insts), delta_pct(a.insts, b.insts));
        line("  self", n(a.insts_self), n(b.insts_self), delta_pct(a.insts_self, b.insts_self));
        line("ms/call", m(a.ms), m(b.ms), delta_pct(a.ms, b.ms));
        line("  min", m(a.ms_min), m(b.ms_min), delta_pct(a.ms_min, b.ms_min));
        line("  median", m(a.ms_median), m(b.ms_median), delta_pct(a.ms_median, b.ms_median));
        line("  p95", m(a.ms_p95), m(b.ms_p95), delta_pct(a.ms_p95, b.ms_p95));
        line("  self", m(a.ms_self), m(b.ms_self), delta_pct(a.ms_self, b.ms_self));
        line("allocations/call", n(a.allocs), n(b.allocs), delta_pct(a.allocs, b.allocs));
        line("copies/call", n(a.copies), n(b.copies), delta_pct(a.copies, b.copies));
        line(
            "  bytes copied/call",
            n(a.copy_bytes),
            n(b.copy_bytes),
            delta_pct(a.copy_bytes, b.copy_bytes),
        );
        line("collections/call", n(a.collections), n(b.collections), None);
        line("  gc ms/call", m(a.gc_ms), m(b.gc_ms), None);
        if recursive {
            let _ = writeln!(
                s,
                "  recursive: the per-call figures are per outermost call (everything under \
                 it counted once);\n  the self figures are per call, recursive ones included"
            );
        }
        if a.sampled || b.sampled {
            let _ = writeln!(
                s,
                "  median and p95 are over a random sample of {} calls",
                commas(crate::profile::BENCH_SAMPLE_CAP as u64)
            );
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(src: &str) -> (crate::env::Env, crate::program::ProgramId) {
        let mut env = crate::env::Env::new();
        let pid = env.load_program(src).expect("compiles");
        (env, pid)
    }

    #[test]
    fn a_name_selects_functions_and_methods() {
        let (env, pid) = program(
            "class Box\n  w: int,\n  h: int,\nend\n\
             fn Box.area(b: Box) -> int\n  b.w * b.h\nend\n\
             fn area(x)\n  return x\nend\n\
             fn other()\n  return 1\nend\n",
        );
        let p = env.get_program(pid).unwrap();
        let names = |q: &str| -> Vec<String> {
            resolve_targets(p, &[q.to_string()])
                .unwrap()
                .into_iter()
                .map(|t| t.name)
                .collect()
        };
        let mut both = names("area");
        both.sort();
        assert_eq!(both, ["Box.area", "area"]);
        assert_eq!(names("Box.area"), ["Box.area"]);
        assert_eq!(names("other"), ["other"]);
    }

    #[test]
    fn an_unknown_name_lists_what_there_is() {
        let (env, pid) = program("fn alpha()\n  return 1\nend\n");
        let err = resolve_targets(env.get_program(pid).unwrap(), &["beta".to_string()])
            .err()
            .expect("no such function");
        assert!(err.contains("no function named 'beta'"), "{err}");
        assert!(err.contains("alpha"), "{err}");
    }

    #[test]
    fn delta_is_optimized_relative_to_unoptimized() {
        assert_eq!(delta_pct(Some(50.0), Some(100.0)), Some(-50.0));
        assert_eq!(delta_pct(Some(1.0), Some(0.0)), None);
        assert_eq!(delta_pct(None, Some(1.0)), None);
    }

    #[test]
    fn numbers_format_by_size() {
        assert_eq!(num(21891.0), "21,891");
        assert_eq!(num(1234.56), "1,234.5");
        assert_eq!(num(2.5), "2.50");
        assert_eq!(num(0.125), "0.1250");
        assert_eq!(ms_text(0.000123), "0.000123");
        assert_eq!(ms_text(12.3456), "12.346");
    }
}
