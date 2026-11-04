//! What a reload costs when it follows the diff and when it rebuilds every
//! static value, for a program whose host is not here to run the rest.
//!
//! ```text
//! cargo run --release --example reload_follow -- <file.ptl> [cycles]
//! ```
//!
//! Like `static_frame`, this runs the static-only build of the file with its
//! values kept: the static part of a frame as a host with the cache runs it.
//! It then makes three edits, each several times over (edit, frame, undo,
//! frame), and reports the reload, the lowering (which a reload that
//! follows the diff does itself, and one that does not leaves to the next
//! frame) and the first frame after them:
//!
//! - **dynamic**: a function added to the entry file. A recompile that
//!   reaches no constant.
//! - **table, shape**: one literal inside a kept declaration rewritten as
//!   `n + 1`, in whichever source file declares it. A recompile that reaches
//!   that declaration and what is built from it.
//! - **table, value**: the same literal changed to another number. No
//!   recompile.
//!
//! Each is run with `Env::set_static_follow` off (every static value is
//! rebuilt by the frame after the reload, as before) and on. The frame's
//! instruction count is exact; the times are the minimum over the cycles.

#![allow(clippy::disallowed_methods, reason = "examples run natively")]

use std::path::{Path, PathBuf};

use petal::env::{Env, ReloadOutcome, StaticSlotState};
use petal::policy::RunPolicy;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::static_audit::StaticAuditMode;

struct World {
    env: Env,
    pid: ProgramId,
    sid: StackKey,
}

fn load(file: &Path, source: &str, follow: bool) -> World {
    let mut env = Env::new();
    env.set_echo(false);
    env.set_check_only(true);
    env.set_policy(RunPolicy::FAST.with_static_cache(true));
    env.set_static_audit(StaticAuditMode::StaticOnly);
    env.set_static_only_keeps(true);
    env.set_static_follow(follow);
    env.set_static_debounce(None);
    let pid = env.load_program_diag(source, Some(file)).unwrap_or_else(|e| panic!("{e}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, pid, sid }
}

/// One frame: (instructions, microseconds).
fn frame(w: &mut World, reset: bool) -> (u64, f64) {
    if reset {
        w.env.reset_stack(w.sid).unwrap();
    }
    let before = w.env.instructions_retired(w.sid).unwrap_or(0);
    let t = std::time::Instant::now();
    if let Err(e) = w.env.run(w.sid) {
        panic!("the static-only build raised: {e}");
    }
    let micros = t.elapsed().as_secs_f64() * 1e6;
    w.env.take_output();
    (w.env.instructions_retired(w.sid).unwrap_or(0) - before, micros)
}

/// A kept declaration with a whole-number literal on its first line: the
/// file it is in, and that file's text with the literal replaced by `with`.
struct Target {
    name: String,
    file: PathBuf,
    is_entry: bool,
    original: String,
    line: usize,
    span: (usize, usize),
    number: i64,
}

impl Target {
    fn edited(&self, with: &str) -> String {
        let mut lines: Vec<String> = self.original.lines().map(str::to_string).collect();
        let l = &mut lines[self.line];
        l.replace_range(self.span.0..self.span.1, with);
        let mut out = lines.join("\n");
        if self.original.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

/// The first number in `line` after its `=`, as (byte range, value): one a
/// literal edit can change without changing its type.
fn number_after_equals(line: &str) -> Option<((usize, usize), i64)> {
    let from = line.find('=')? + 1;
    let bytes = line.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let prev = bytes[i - 1];
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let next = bytes.get(i).copied().unwrap_or(b' ');
            let plain = !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.' || prev == b'"' || prev == b'#')
                && !(next.is_ascii_alphanumeric() || next == b'_' || next == b'.' || next == b'"');
            if plain && let Ok(n) = line[start..i].parse::<i64>() {
                return Some(((start, i), n));
            }
        } else if bytes[i] == b'"' {
            // Skip a string.
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            i += 1;
        } else if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            break;
        } else {
            i += 1;
        }
    }
    None
}

/// Pick the kept declaration to edit: named, a table (not a scalar), with a
/// number on its first line; the one with the most slots depending on it
/// would be the worst case, the middle one of the candidates is a fair one.
fn pick(w: &World, entry: &Path, entry_source: &str, want: Option<&str>) -> Option<Target> {
    let values = w.env.static_values(w.sid)?;
    let paths = w.env.program_source_paths(w.pid, None);
    let mut found = Vec::new();
    for (info, state) in &values.slots {
        let (Some(name), Some(at)) = (info.name.as_deref(), info.at.as_deref()) else { continue };
        if name.starts_with("enum data of ") {
            continue;
        }
        if !matches!(state, StaticSlotState::Filled(v) if !matches!(v, petal::value::Value::Int(_) | petal::value::Value::Float(_) | petal::value::Value::Bool(_) | petal::value::Value::String(_) | petal::value::Value::Nil))
        {
            continue;
        }
        if want.is_some_and(|w| w != name) {
            continue;
        }
        // `file.ptl:12` for a module, `line 12` for the entry file.
        let (file, line, is_entry) = match at.rsplit_once(':') {
            Some((f, l)) => {
                let Some(path) = paths.iter().find(|p| p.ends_with(f)) else { continue };
                (path.clone(), l.parse::<usize>().ok()?, false)
            }
            None => (entry.to_path_buf(), at.strip_prefix("line ")?.parse::<usize>().ok()?, true),
        };
        let original = if is_entry {
            entry_source.to_string()
        } else {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            text
        };
        let Some(text) = original.lines().nth(line - 1) else { continue };
        let Some((span, number)) = number_after_equals(text) else { continue };
        found.push(Target { name: name.to_string(), file, is_entry, original, line: line - 1, span, number });
    }
    let n = found.len();
    (n > 0).then(|| found.swap_remove(n / 2))
}

struct Measured {
    outcome: ReloadOutcome,
    reload_us: f64,
    /// Lowering the new program, when the reload did not.
    lower_us: f64,
    frame_us: f64,
    instructions: u64,
    kept: usize,
    rebuilt: usize,
}

/// Edit, frame, undo, frame, `cycles` times; the edit direction measured.
fn cycle(
    w: &mut World,
    file: &Path,
    entry_source: &str,
    apply: &dyn Fn(&mut World, bool) -> String,
    cycles: usize,
) -> Measured {
    let mut best: Option<Measured> = None;
    for _ in 0..cycles {
        let mut one = None;
        for edited in [true, false] {
            let text = apply(w, edited);
            let entry = if text.is_empty() { entry_source.to_string() } else { text };
            let t = std::time::Instant::now();
            let report = w.env.reload_program(w.sid, &entry, Some(file)).unwrap_or_else(|e| panic!("{e}"));
            let reload_us = t.elapsed().as_secs_f64() * 1e6;
            // A reload that follows the diff lowers the new program itself;
            // one that does not leaves it to the next frame. Time it apart
            // from the frame either way.
            let t = std::time::Instant::now();
            w.env.lower_program(w.pid).unwrap();
            let lower_us = t.elapsed().as_secs_f64() * 1e6;
            let (instructions, frame_us) = frame(w, false);
            frame(w, true);
            if edited {
                let (kept, rebuilt) = report.statics.as_ref().map_or((0, 0), |s| (s.kept, s.rebuilt));
                one = Some(Measured { outcome: report.outcome, reload_us, lower_us, frame_us, instructions, kept, rebuilt });
            }
        }
        let one = one.unwrap();
        best = Some(match best {
            None => one,
            Some(b) => Measured {
                reload_us: b.reload_us.min(one.reload_us),
                lower_us: b.lower_us.min(one.lower_us),
                frame_us: b.frame_us.min(one.frame_us),
                ..one
            },
        });
    }
    best.unwrap()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(file) = args.next().map(PathBuf::from) else {
        eprintln!("usage: reload_follow <file.ptl> [cycles] [declaration]");
        std::process::exit(2);
    };
    let cycles: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5).max(1);
    let want = args.next();
    let source = std::fs::read_to_string(&file).expect("script file");
    println!("{}: the static-only build, {cycles} cycles per edit", file.display());
    println!("| edit | follow | outcome | reload | lowering | first frame after | its instructions | kept | rebuilt |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for follow in [false, true] {
        let mut w = load(&file, &source, follow);
        frame(&mut w, true);
        let (steady, _) = frame(&mut w, true);
        let slots = w.env.static_values(w.sid).map_or(0, |v| v.slots.len());
        let target = pick(&w, &file, &source, want.as_deref());
        let row = |edit: &str, m: &Measured| {
            println!(
                "| {edit} | {} | {} | {:.2} ms | {:.2} ms | {:.0} µs | {} | {} | {} |",
                if follow { "on" } else { "off" },
                m.outcome.label(),
                m.reload_us / 1000.0,
                m.lower_us / 1000.0,
                m.frame_us,
                m.instructions,
                m.kept,
                m.rebuilt,
            );
        };
        let grown = format!("{}\nfn __reload_follow_added(x) x + 1 end\n", source.trim_end());
        let m = cycle(&mut w, &file, &source, &|_, edited| if edited { grown.clone() } else { String::new() }, cycles);
        row("dynamic: a function added", &m);
        if let Some(t) = &target {
            for (label, with) in [
                ("table, shape", format!("{} + 1", t.number)),
                ("table, value", format!("{}", t.number + 1)),
            ] {
                let new = t.edited(&with);
                let apply = |w: &mut World, edited: bool| -> String {
                    let text = if edited { new.clone() } else { t.original.clone() };
                    if t.is_entry {
                        text
                    } else {
                        w.env.override_file_source(&t.file, &text);
                        String::new()
                    }
                };
                let m = cycle(&mut w, &file, &source, &apply, cycles);
                row(&format!("{label}: `{}` in {}", t.name, t.file.file_name().unwrap().to_string_lossy()), &m);
            }
        } else {
            println!("| (no kept table with a number on its first line) | | | | | | | | |");
        }
        if follow {
            println!("\n{slots} slots; a frame with every value kept is {steady} instructions");
        }
    }
}
