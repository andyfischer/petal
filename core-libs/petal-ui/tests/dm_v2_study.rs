//! Direct manipulation v2, milestone M5: the examples ladder and the
//! complexity study (docs/dev/direct-manipulation-v2-plan.md §5, M5).
//!
//! Every example under `examples/direct-manipulation-v2/` carries its drag
//! goals in header comments (the format is in that directory's README). For
//! each goal the harness:
//!
//! 1. loads the example headlessly (`Headless`, with Garden's panel stubs
//!    when the file says `// dm-panel`, the RNG seeded), runs `dm-frames`
//!    frames and takes the stack after the last one as the checkpoint;
//! 2. calls `propose_edits_v2` with the goal (and the goal's client filter,
//!    if it has one), timing it;
//! 3. runs the top alternative as overrides of the original program in a
//!    fresh session, then writes it as a text edit into a copy of the
//!    example that was run to the same checkpoint, hot-reloads the copy
//!    (`Env::reload_program`), and requires the reloaded frame to draw
//!    exactly what the override run drew (in the full study, a restart of
//!    the edited copy is recorded too);
//! 4. records whether that worked (the top alternative is verified, the
//!    reloaded frame matches and is within tolerance, and the changed
//!    literals are the ones the goal's `expect=` names), and the cost.
//!
//! The full study adds a scaling sweep, which generates programs of growing
//! size (rows of N shapes, trees of depth D, simulations of S steps,
//! programs with K shared literals, N items under `map`) with one goal each,
//! and an external pass, which drags every headless-runnable app under the
//! `DM_STUDY_EXTRA` directories with auto-generated goals.
//!
//! Run it (from `core-libs/petal-ui`; the command lines are in the study
//! doc's §7):
//!
//! ```text
//! cargo test --release --test dm_v2_study ladder -- --nocapture       # asserted
//! DM_STUDY_WRITE=1 DM_STUDY_EXTRA=<dir>[:<dir>…] \
//!   cargo test --release --test dm_v2_study full_study -- --ignored --nocapture
//! DM_STUDY_ONLY=l07,a0   only the examples whose file name contains one of these
//! DM_STUDY_GOAL=<name>   only that goal
//! DM_STUDY_DUMP=1        print each example's draw list (to write goals)
//! DM_STUDY_VERBOSE=1     print alternatives, scores, columns, re-run checks
//! DM_STUDY_EXPLAIN=1     print the solver's explain log
//! DM_STUDY_DIR=<dir>     run a ladder from another directory
//! ```
//!
//! `ladder` is a normal test: every goal not marked `xfail=` must succeed.
//! `full_study` (ignored) runs everything and, with `DM_STUDY_WRITE`,
//! rewrites docs/dev/direct-manipulation-v2-study.{md,json}.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use petal::dm_v2::observe::hash_str;
use petal::dm_v2::params::{ParamFilter, ParamSelector};
use petal::dm_v2::spec::{ObsValue, SpecConfig, SpecSession, StateMode};
use petal::dm_v2::{
    Alternative, DmError, DmGoal, DmRequest, DmResponse, EmitKey, Observable, PathStep, Prefer,
    apply_edits_multi, program_files, propose_edits_v2,
};
use petal_ui::harness::Headless;
use serde_json::{Value as Json, json};

// ── the example format ───────────────────────────────────────────────────

/// `tag#k.arg[.field]`: raw native argument `arg` of the `k`-th (0-based)
/// emitted draw command whose tag is `tag` (`*` counts every command).
#[derive(Debug, Clone)]
struct Addr {
    tag: String,
    k: usize,
    arg: usize,
    path: Vec<PathStep>,
}

impl std::fmt::Display for Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}.{}", self.tag, self.k, self.arg)?;
        for p in &self.path {
            match p {
                PathStep::Field(s) => write!(f, ".{s}")?,
                PathStep::Index(i) => write!(f, "[{i}]")?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum Op {
    By(f64),
    To(f64),
}

#[derive(Debug, Clone)]
struct Target {
    addr: Addr,
    op: Op,
}

#[derive(Debug, Clone, Default)]
struct GoalSpec {
    name: String,
    targets: Vec<Target>,
    expect: Option<Vec<String>>,
    allow: Option<Vec<ParamSelector>>,
    allow_text: Option<String>,
    deny: Vec<ParamSelector>,
    strict: bool,
    respell: bool,
    tol: Option<f64>,
    alts: Option<usize>,
    max_runs: Option<usize>,
    xfail: Option<String>,
}

#[derive(Debug, Clone)]
struct Example {
    /// The entry file.
    file: PathBuf,
    /// What gets copied for the from-scratch re-run: the file, or its
    /// directory for a multi-file example.
    root: PathBuf,
    id: String,
    level: String,
    title: String,
    panel: bool,
    frames: usize,
    size: (i32, i32),
    fresh_state: bool,
    goals: Vec<GoalSpec>,
}

fn parse_addr(s: &str) -> Result<Addr, String> {
    let (tag, rest) = s.split_once('#').ok_or_else(|| format!("`{s}`: want tag#k.arg"))?;
    let mut parts = rest.split('.');
    let k = parts.next().unwrap().parse().map_err(|_| format!("`{s}`: bad k"))?;
    let arg = parts.next().ok_or_else(|| format!("`{s}`: no arg"))?.parse().map_err(|_| format!("`{s}`: bad arg"))?;
    let path = parts.map(|p| PathStep::Field(p.to_string())).collect();
    Ok(Addr { tag: tag.to_string(), k, arg, path })
}

fn parse_selector(s: &str) -> Result<ParamSelector, String> {
    Ok(match s.split_once(':') {
        Some(("var", v)) => ParamSelector::Variable(v.into()),
        Some(("fn", v)) => ParamSelector::Function(v.into()),
        Some(("line", v)) => ParamSelector::Line(v.parse().map_err(|_| format!("bad line `{v}`"))?),
        None if s == "config" => ParamSelector::ConfigOnly,
        None if s == "all" => ParamSelector::AllUserCode,
        None if s == "state-init" => ParamSelector::IncludeStateInit,
        None if s == "structural" => ParamSelector::IncludeStructural,
        None if s == "rng" => ParamSelector::IncludeThroughRng,
        None if s == "path-deps" => ParamSelector::AllowPathDeps,
        _ => return Err(format!("unknown selector `{s}`")),
    })
}

/// `// dm-goal <name>: <tag#k.arg> (+=|-=|=) <n>[, ...] [| opts]`
fn parse_goal(line: &str) -> Result<GoalSpec, String> {
    let (name, rest) = line.split_once(':').ok_or("dm-goal: want `name: targets`")?;
    let (targets, opts) = match rest.split_once('|') {
        Some((t, o)) => (t, o.trim()),
        None => (rest, ""),
    };
    let mut g = GoalSpec { name: name.trim().to_string(), ..Default::default() };
    for t in targets.split(',') {
        let t = t.trim();
        let (lhs, op, rhs) = if let Some((l, r)) = t.split_once("+=") {
            (l, '+', r)
        } else if let Some((l, r)) = t.split_once("-=") {
            (l, '-', r)
        } else if let Some((l, r)) = t.split_once('=') {
            (l, '=', r)
        } else {
            return Err(format!("target `{t}`: want `addr += n`"));
        };
        let v: f64 = rhs.trim().parse().map_err(|_| format!("target `{t}`: bad number"))?;
        let op = match op {
            '+' => Op::By(v),
            '-' => Op::By(-v),
            _ => Op::To(v),
        };
        g.targets.push(Target { addr: parse_addr(lhs.trim())?, op });
    }
    let mut rest = opts;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("xfail=") {
            g.xfail = Some(r.trim().to_string());
            break;
        }
        let (tok, r) = rest.split_once(' ').unwrap_or((rest, ""));
        rest = r.trim_start();
        match tok.split_once('=') {
            Some(("expect", v)) => g.expect = Some(v.split(',').map(str::to_string).collect()),
            Some(("allow", v)) => {
                g.allow_text = Some(v.to_string());
                g.allow = Some(v.split(',').map(parse_selector).collect::<Result<_, _>>()?);
            }
            Some(("deny", v)) => g.deny = v.split(',').map(parse_selector).collect::<Result<_, _>>()?,
            Some(("tol", v)) => g.tol = Some(v.parse().map_err(|_| "bad tol")?),
            Some(("alts", v)) => g.alts = Some(v.parse().map_err(|_| "bad alts")?),
            Some(("runs", v)) => g.max_runs = Some(v.parse().map_err(|_| "bad runs")?),
            None if tok == "strict" => g.strict = true,
            None if tok == "respell" => g.respell = true,
            _ => return Err(format!("unknown option `{tok}`")),
        }
    }
    Ok(g)
}

fn parse_example(file: &Path, root: &Path) -> Result<Example, String> {
    let source = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let id = if root.is_dir() {
        root.file_name().unwrap().to_string_lossy().to_string()
    } else {
        file.file_stem().unwrap().to_string_lossy().to_string()
    };
    let mut ex = Example {
        file: file.to_path_buf(),
        root: root.to_path_buf(),
        level: id.split('-').next().unwrap_or("").to_uppercase(),
        id,
        title: String::new(),
        panel: false,
        frames: 1,
        size: (800, 600),
        fresh_state: false,
        goals: Vec::new(),
    };
    for line in source.lines() {
        let Some(c) = line.trim().strip_prefix("//") else { continue };
        let c = c.trim();
        if let Some(r) = c.strip_prefix("dm-goal ") {
            ex.goals.push(parse_goal(r).map_err(|e| format!("{}: {e}", file.display()))?);
        } else if let Some(r) = c.strip_prefix("dm-title:") {
            ex.title = r.trim().to_string();
        } else if let Some(r) = c.strip_prefix("dm-level:") {
            ex.level = r.trim().to_string();
        } else if let Some(r) = c.strip_prefix("dm-frames:") {
            ex.frames = r.trim().parse().map_err(|_| "bad dm-frames")?;
        } else if let Some(r) = c.strip_prefix("dm-size:") {
            let (w, h) = r.trim().split_once('x').ok_or("bad dm-size")?;
            ex.size = (w.parse().map_err(|_| "bad w")?, h.parse().map_err(|_| "bad h")?);
        } else if c == "dm-panel" {
            ex.panel = true;
        } else if c == "dm-state: fresh" {
            ex.fresh_state = true;
        }
    }
    Ok(ex)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn ladder_examples() -> Vec<Example> {
    let dir = std::env::var_os("DM_STUDY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("examples/direct-manipulation-v2"));
    let mut out = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    let only = std::env::var("DM_STUDY_ONLY").ok();
    for p in entries {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if only.as_deref().is_some_and(|o| !o.split(',').any(|o| name.contains(o))) {
            continue;
        }
        if p.is_dir() && p.join("app.ptl").exists() {
            out.push(parse_example(&p.join("app.ptl"), &p).unwrap());
        } else if p.extension().is_some_and(|e| e == "ptl") {
            out.push(parse_example(&p, &p).unwrap());
        }
    }
    out
}

// ── running ──────────────────────────────────────────────────────────────

const TAGS: &[&str] = &[
    "circle", "rect", "text", "line", "triangle", "ellipse", "ellipse_outline", "rect_outline",
    "rect_gradient", "rects", "image", "clip", "clip_push", "clip_pop", "clip_none", "clear", "layer",
    "rect_rounded", "rect_rounded_outline", "circle_outline", "arc", "poly", "polygon", "fan",
];

fn tag_name(v: &ObsValue) -> &'static str {
    if let ObsValue::Variant { tag, .. } = v {
        for t in TAGS {
            if hash_str(t) == *tag {
                return t;
            }
        }
    }
    "?"
}

/// One baseline emit, as the harness addresses it.
#[derive(Debug, Clone)]
struct Emit {
    key: EmitKey,
    tag: &'static str,
    args: Vec<ObsValue>,
}

/// A loaded example, run up to its checkpoint.
fn load(ex: &Example, file: &Path) -> Result<Headless, String> {
    let mut ui = if ex.panel {
        Headless::panel_from_file_with_paths(file, ex.size.0, ex.size.1, &[])?
    } else {
        Headless::from_file_with_size(file, ex.size.0, ex.size.1)?
    };
    // A fixed seed, as a host that seeds its `random()` stream would have:
    // the default is the clock, and two loads would draw different numbers.
    ui.set_seed(7);
    ui.frames(ex.frames.max(1))?;
    let sid = ui.stack_id();
    ui.env.reset_stack(sid)?;
    Ok(ui)
}

fn spec_config(ex: &Example) -> SpecConfig {
    SpecConfig { state: if ex.fresh_state { StateMode::Fresh } else { StateMode::Frozen }, ..SpecConfig::default() }
}

fn baseline(ui: &mut Headless, cfg: &SpecConfig) -> Result<Vec<Emit>, String> {
    let sid = ui.stack_id();
    let (env, mut host) = ui.spec_parts();
    let s = SpecSession::begin(env, &mut host, sid, &[], cfg.clone(), None).map_err(|e| format!("{e:?}"))?;
    let out = s
        .baseline()
        .emits
        .iter()
        .map(|e| Emit { key: e.key, tag: tag_name(&e.value), args: e.native_args.to_vec() })
        .collect();
    s.end(env);
    Ok(out)
}

fn find<'a>(emits: &'a [Emit], a: &Addr) -> Option<&'a Emit> {
    emits.iter().filter(|e| a.tag == "*" || e.tag == a.tag).nth(a.k)
}

fn read(emits: &[Emit], a: &Addr) -> Option<f64> {
    let e = find(emits, a)?;
    let mut v = e.args.get(a.arg)?;
    for p in &a.path {
        v = match (v, p) {
            (ObsValue::Record(fs), PathStep::Field(n)) => &fs.iter().find(|(k, _)| k == n)?.1,
            (ObsValue::List(xs), PathStep::Field(n)) => {
                let i = ["x", "y", "z"].iter().position(|c| c == n)?;
                xs.get(i)?
            }
            (ObsValue::List(xs), PathStep::Index(i)) => xs.get(*i)?,
            _ => return None,
        };
    }
    v.as_f64()
}

/// Static size of an example: lines, functions, loops, `var`s, `state`s,
/// literals (numeric, in user files), and dynamic: emits.
#[derive(Debug, Clone, Default)]
struct Metrics {
    files: usize,
    lines: usize,
    fns: usize,
    loops: usize,
    vars: usize,
    states: usize,
    configs: usize,
    hints: usize,
    emits: usize,
    reads_clock: bool,
    reads_rng: bool,
    terms: usize,
}

fn metrics(ui: &Headless, ex: &Example, emits: usize) -> Metrics {
    let program = ui.env.get_program(ui.program_id()).unwrap();
    let root = if ex.root.is_dir() { ex.root.clone() } else { ex.root.parent().unwrap().to_path_buf() };
    let root = root.canonicalize().unwrap_or(root);
    let mut m = Metrics { emits, ..Default::default() };
    let mut user_files = Vec::new();
    for (i, f) in program.source_map.files.iter().enumerate() {
        let user = i == 0
            || f.origin.as_ref().is_some_and(|o| o.canonicalize().unwrap_or(o.clone()).starts_with(&root));
        if !user {
            continue;
        }
        user_files.push(i as u16);
        m.files += 1;
        for line in f.source.lines() {
            let t = line.trim_start();
            if t.is_empty() || t.starts_with("//") {
                continue;
            }
            m.lines += 1;
            let starts = |w: &str| t.starts_with(w);
            if starts("fn ") || starts("pub fn ") {
                m.fns += 1;
            }
            if starts("for ") || starts("while ") || t.contains(" for ") {
                m.loops += 1;
            }
            if starts("var ") || starts("state var ") {
                m.vars += 1;
            }
            if starts("state ") {
                m.states += 1;
            }
            if starts("config let ") {
                m.configs += 1;
            }
            m.hints += t.matches("tunable(").count() + t.matches("rigid(").count();
            m.reads_clock |= t.contains("time()") || t.contains("dt()");
            m.reads_rng |= t.contains("random") || t.contains("noise(") || t.contains("choose(");
        }
    }
    m.terms = program
        .terms
        .iter()
        .filter(|t| program.source_map.get(t.id).is_some_and(|s| user_files.contains(&s.file.0)))
        .count();
    m
}

#[derive(Debug, Clone, Default)]
struct GoalResult {
    example: String,
    level: String,
    goal: String,
    targets: String,
    status: &'static str,
    why: String,
    success: bool,
    any_verified: bool,
    top_verified: bool,
    expect_ok: Option<bool>,
    rerun_error: Option<f64>,
    /// Whether the hot-reloaded edited program draws exactly what the
    /// top alternative's override run of the original drew.
    edit_matches_override: Option<bool>,
    /// The same for a restart of the edited program from scratch (differs
    /// for edits that change how `state` evolves).
    restart_matches: Option<bool>,
    max_error_tau: Option<f64>,
    runs: usize,
    wall_ms: f64,
    discovered: usize,
    eligible: usize,
    screened: usize,
    measured: usize,
    truncated: bool,
    alternatives: usize,
    verified_alternatives: usize,
    top_edits: Vec<String>,
    other_alternatives: Vec<String>,
    aids: Vec<&'static str>,
    stateful: bool,
    regime_changed: bool,
    rng_order_changed: bool,
    budget_exhausted: bool,
    iterations: usize,
    collateral: usize,
    xfail: Option<String>,
}

fn change_matches(c: &petal::dm_v2::ParamChange, tok: &str) -> bool {
    if let Some(t) = tok.strip_prefix("lit:") {
        return c.old_text == t;
    }
    if let Some(f) = tok.strip_prefix("fn:") {
        return c.function.as_deref() == Some(f);
    }
    if let Some(l) = tok.strip_prefix('L').and_then(|l| l.parse::<u32>().ok()) {
        return c.file.0 == 0 && c.span.start.line == l;
    }
    if tok == "config" {
        return c.config;
    }
    c.binding.as_deref().is_some_and(|b| b == tok || b.rsplit("::").next() == Some(tok))
}

fn describe(a: &Alternative) -> String {
    let e: Vec<String> = a
        .changes
        .iter()
        .map(|c| {
            let name = c.binding.clone().or_else(|| c.function.as_ref().map(|f| format!("in {f}"))).unwrap_or_default();
            format!("{}→{} ({name} L{})", c.old_text, c.new_text, c.span.start.line)
        })
        .collect();
    format!("{}{}", e.join(", "), if a.verified { "" } else { " [unverified]" })
}

/// A copy of the example on disk, loaded and run to the same checkpoint as
/// the original: what edits are hot-reloaded into. One per example; after
/// each goal its files are restored and reloaded back.
struct LiveCopy {
    dir: PathBuf,
    entry: PathBuf,
    ui: Headless,
}

impl LiveCopy {
    fn new(ex: &Example, tag: &str) -> Result<LiveCopy, String> {
        let dir = scratch_dir(&format!("{}-{tag}", ex.id));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let entry = if ex.root.is_dir() {
            copy_dir(&ex.root, &dir).map_err(|e| e.to_string())?;
            dir.join(ex.file.strip_prefix(&ex.root).unwrap())
        } else {
            let entry = dir.join(ex.file.file_name().unwrap());
            std::fs::copy(&ex.file, &entry).map_err(|e| e.to_string())?;
            entry
        };
        let ui = load(ex, &entry)?;
        Ok(LiveCopy { dir, entry, ui })
    }

    /// Write `files` into the copy and reload its running stack.
    fn reload(&mut self, files: &[(PathBuf, String)]) -> Result<(), String> {
        for (path, text) in files {
            std::fs::write(path, text).map_err(|e| e.to_string())?;
        }
        let entry_text = std::fs::read_to_string(&self.entry).map_err(|e| e.to_string())?;
        let sid = self.ui.stack_id();
        let report =
            self.ui.env.reload_program(sid, &entry_text, Some(&self.entry)).map_err(|e| format!("reload: {e}"))?;
        if std::env::var_os("DM_STUDY_VERBOSE").is_some() {
            println!(
                "     reload: {} (state preserved {:?}, dropped {:?})",
                report.outcome.label(),
                report.state_preserved,
                report.state_dropped
            );
        }
        self.ui.env.reset_stack(sid)?;
        Ok(())
    }
}

impl Drop for LiveCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Apply `alt` to the example's live copy and run it again, two ways:
///
/// - *hot reload*: the edited files are written over the copy, which was run
///   to the same checkpoint as the original, its running stack is reloaded
///   (`Env::reload_program`, state carried over), and the next frame is read.
///   This is what a host applying the edit live shows, and what the
///   speculative runs model (frozen state). The copy is then restored and
///   reloaded back for the next goal.
/// - *restart* (with [`restart_check`]): the edited copy is loaded fresh and
///   run for the same frames. It differs from the hot reload when the edit
///   changes how the state evolves (a `state` animation's speed) or which
///   random numbers are drawn.
fn rerun(
    ex: &Example,
    ui: &Headless,
    live: &mut LiveCopy,
    alt: &Alternative,
    cfg: &SpecConfig,
) -> Result<(Vec<Emit>, Option<Vec<Emit>>), String> {
    let program = ui.env.get_program(ui.program_id()).unwrap();
    let originals = program_files(program);
    let mut files = originals.clone();
    apply_edits_multi(&mut files, &alt.edits).map_err(|e| e.to_string())?;
    // Where each file of the original program is in the copy.
    let root = if ex.root.is_dir() { Some(ex.root.canonicalize().unwrap()) } else { None };
    let place = |fid: petal::source_map::FileId| -> Option<PathBuf> {
        if fid.0 == 0 {
            return Some(live.entry.clone());
        }
        let root = root.as_ref()?;
        let f = &program.source_map.files[fid.0 as usize];
        let rel = f.origin.as_ref()?.canonicalize().ok()?.strip_prefix(root).ok()?.to_path_buf();
        Some(live.dir.join(rel))
    };
    let mut edited: Vec<(PathBuf, String)> = Vec::new();
    let mut restore: Vec<(PathBuf, String)> = Vec::new();
    for ((fid, text), (_, old)) in files.iter().zip(&originals) {
        if text == old {
            continue;
        }
        let Some(path) = place(*fid) else {
            return Err(format!("an edit lands outside the example: {}", program.source_map.files[fid.0 as usize].name));
        };
        edited.push((path.clone(), text.clone()));
        restore.push((path, old.clone()));
    }
    let result = (|| {
        live.reload(&edited)?;
        let hot = baseline(&mut live.ui, cfg)?;
        let restart = if restart_check() {
            let mut fresh = load(ex, &live.entry)?;
            Some(baseline(&mut fresh, cfg)?)
        } else {
            None
        };
        Ok((hot, restart))
    })();
    live.reload(&restore)?;
    result
}

/// Whether to also restart each edited program from scratch (recorded in
/// the report, not required for success): on for the full study and when
/// writing the report, off for the asserted ladder, where it would double
/// the time of a debug build.
fn restart_check() -> bool {
    RESTART.with(|r| r.get()) || std::env::var_os("DM_STUDY_WRITE").is_some()
}

thread_local! {
    static RESTART: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The largest distance from the goals' targets in `emits`, reading each
/// goal by its position (`tag#k`), or the first goal gone.
fn miss(emits: &[Emit], want: &[(Addr, f64, EmitKey)]) -> Result<f64, String> {
    let mut worst: f64 = 0.0;
    for (a, target, _) in want {
        match read(emits, a) {
            Some(v) => worst = worst.max((v - target).abs()),
            None => return Err(a.to_string()),
        }
    }
    Ok(worst)
}

/// The same, reading each goal by its baseline emit's key (valid in runs of
/// the original program only).
fn miss_by_key(emits: &[Emit], want: &[(Addr, f64, EmitKey)]) -> Result<f64, String> {
    let mut worst: f64 = 0.0;
    for (a, target, key) in want {
        let v = emits.iter().position(|e| e.key == *key).and_then(|i| {
            let e = &emits[i];
            let k = emits[..i].iter().filter(|x| x.tag == e.tag).count();
            read(emits, &Addr { tag: e.tag.to_string(), k, ..a.clone() })
        });
        match v {
            Some(v) => worst = worst.max((v - target).abs()),
            None => return Err(a.to_string()),
        }
    }
    Ok(worst)
}

/// Whether two runs drew the same list: the same commands in the same
/// order with the same numbers (to 1e-6, relative).
fn same_draws(a: &[Emit], b: &[Emit]) -> bool {
    fn same(x: &ObsValue, y: &ObsValue) -> bool {
        match (x.as_f64(), y.as_f64()) {
            (Some(p), Some(q)) => (p - q).abs() <= 1e-6 * p.abs().max(q.abs()).max(1.0),
            _ => match (x, y) {
                (ObsValue::List(p), ObsValue::List(q)) => p.len() == q.len() && p.iter().zip(q).all(|(p, q)| same(p, q)),
                (ObsValue::Record(p), ObsValue::Record(q)) => {
                    p.len() == q.len() && p.iter().zip(q).all(|((k, p), (l, q))| k == l && same(p, q))
                }
                _ => x == y,
            },
        }
    }
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| x.tag == y.tag && x.args.len() == y.args.len() && x.args.iter().zip(&y.args).all(|(p, q)| same(p, q)))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    for e in std::fs::read_dir(from)? {
        let p = e?.path();
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            std::fs::create_dir_all(&dest)?;
            copy_dir(&p, &dest)?;
        } else {
            std::fs::copy(&p, &dest)?;
        }
    }
    Ok(())
}

fn scratch_dir(tag: &str) -> PathBuf {
    let base = std::env::var_os("DM_STUDY_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("petal-dm-v2-study"));
    base.join(format!("{}-{tag}", std::process::id()))
}

fn run_goal(ex: &Example, g: &GoalSpec, ui: &mut Headless, live: &mut Option<LiveCopy>, emits: &[Emit]) -> GoalResult {
    let mut r = GoalResult {
        example: ex.id.clone(),
        level: ex.level.clone(),
        goal: g.name.clone(),
        targets: g
            .targets
            .iter()
            .map(|t| match t.op {
                Op::By(d) => format!("{} {}= {}", t.addr, if d < 0.0 { '-' } else { '+' }, d.abs()),
                Op::To(v) => format!("{} = {v}", t.addr),
            })
            .collect::<Vec<_>>()
            .join(", "),
        xfail: g.xfail.clone(),
        ..Default::default()
    };
    if g.allow.is_some() {
        r.aids.push("client");
    }
    if g.strict {
        r.aids.push("strict");
    }
    let mut goals = Vec::new();
    let mut want = Vec::new();
    for t in &g.targets {
        let (Some(e), Some(now)) = (find(emits, &t.addr), read(emits, &t.addr)) else {
            r.status = "error";
            r.why = format!("no emit {}", t.addr);
            return r;
        };
        let target = match t.op {
            Op::By(d) => now + d,
            Op::To(v) => v,
        };
        want.push((t.addr.clone(), target, e.key));
        let obs = Observable::EmitArg { key: e.key, arg: t.addr.arg, path: t.addr.path.clone() };
        let mut goal = DmGoal::to(obs, target);
        if let Some(tol) = g.tol {
            goal = goal.within(tol);
        }
        goals.push(goal);
    }
    let mut req = DmRequest::new(goals);
    req.spec = spec_config(ex);
    req.filter = ParamFilter { allow: g.allow.clone(), deny: g.deny.clone() };
    if g.strict {
        req.prefer = Prefer::ConfigStrict;
    }
    req.allow_respell = g.respell;
    if let Some(n) = g.alts {
        req.max_alternatives = n;
    }
    if let Some(n) = g.max_runs {
        req.max_runs = n;
    }
    req.max_wall = Duration::from_secs(60);
    req.explain = std::env::var_os("DM_STUDY_EXPLAIN").is_some();
    let sid = ui.stack_id();
    let t0 = Instant::now();
    let resp: Result<DmResponse, DmError> = {
        let (env, mut host) = ui.spec_parts();
        propose_edits_v2(env, &mut host, sid, &req)
    };
    r.wall_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let resp = match resp {
        Ok(resp) => resp,
        Err(e) => {
            r.status = "no-proposal";
            r.why = e.to_string();
            return finish(r);
        }
    };
    let d = &resp.diagnostics;
    r.runs = d.runs;
    r.discovered = d.discovered;
    r.eligible = d.eligible;
    r.screened = d.screened;
    r.measured = d.measured;
    r.truncated = d.truncated;
    r.budget_exhausted = d.budget_exhausted;
    r.alternatives = resp.alternatives.len();
    r.verified_alternatives = resp.alternatives.iter().filter(|a| a.verified).count();
    r.any_verified = r.verified_alternatives > 0;
    r.other_alternatives = resp.alternatives.iter().skip(1).map(describe).collect();
    if let Some(log) = &resp.explain {
        for l in &log.lines {
            println!("     | {l}");
        }
    }
    if std::env::var_os("DM_STUDY_VERBOSE").is_some() {
        println!("   [{}] {} runs, d/e/s/m {}/{}/{}/{}", g.name, d.runs, d.discovered, d.eligible, d.screened, d.measured);
        for a in &resp.alternatives {
            println!("     alt {:.3} {:?} {:?} err {:.3} {}", a.score, a.kind, a.score_parts, a.max_error, describe(a));
        }
        for c in &resp.candidates {
            println!(
                "     cand {:>8} {:<10} w {:.2} {:?} col {:?} {:?}",
                c.param.value,
                c.param.binding.as_deref().unwrap_or(""),
                c.weight,
                c.eligible,
                c.column,
                c.quality
            );
        }
    }
    let Some(top) = resp.alternatives.first() else {
        r.status = "no-proposal";
        r.why = "no alternatives".into();
        return finish(r);
    };
    r.top_verified = top.verified;
    r.top_edits = top.changes.iter().map(|c| format!("{}→{}", c.old_text, c.new_text)).collect();
    r.max_error_tau = Some(top.max_error);
    r.stateful = top.stateful;
    r.regime_changed = top.regime_changed;
    r.rng_order_changed = top.rng_order_changed;
    r.iterations = top.iterations;
    r.collateral = top.collateral.items.len();
    if top.changes.iter().any(|c| c.config) {
        r.aids.push("config");
    }
    let hinted = top.changes.iter().any(|c| {
        resp.candidates.iter().any(|cand| cand.param.term == c.term && cand.param.hint.is_some())
    });
    if hinted {
        r.aids.push("hint");
    }
    if let Some(exp) = &g.expect {
        let ok = if exp.iter().any(|e| e == "none") {
            top.changes.is_empty()
        } else {
            !top.changes.is_empty() && top.changes.iter().all(|c| exp.iter().any(|e| change_matches(c, e)))
        };
        r.expect_ok = Some(ok);
    }
    let cfg = spec_config(ex);
    // The top alternative applied as overrides to the original program, in
    // a fresh session: what the edited program must draw, emit for emit.
    let overridden: Option<Vec<Emit>> = {
        use petal::dm_v2::spec::{OverrideValue, Overrides};
        let terms: Vec<_> = top.changes.iter().map(|c| c.term).collect();
        let mut ov = Overrides::none();
        for c in &top.changes {
            let value = match resp.candidates.iter().find(|cand| cand.param.term == c.term) {
                // A negated literal's term holds the unsigned value.
                Some(cand) => cand.param.override_for(c.new),
                None if c.new.fract() == 0.0 && !c.new_text.contains('.') => OverrideValue::Int(c.new as i64),
                None => OverrideValue::Num(c.new),
            };
            ov = ov.with(c.term, value);
        }
        let (env, mut host) = ui.spec_parts();
        SpecSession::begin(env, &mut host, sid, &terms, cfg.clone(), None).ok().map(|mut s| {
            let run = s.run(env, &mut host, &ov);
            let out = run.emits.iter().map(|e| Emit { key: e.key, tag: tag_name(&e.value), args: e.native_args.to_vec() }).collect();
            s.end(env);
            out
        })
    };
    let tol = g.tol.unwrap_or(0.5);
    if live.is_none() {
        match LiveCopy::new(ex, "live") {
            Ok(l) => *live = Some(l),
            Err(e) => {
                r.why = format!("re-run failed: {e}");
                r.status = "fail";
                return finish(r);
            }
        }
    }
    match rerun(ex, ui, live.as_mut().unwrap(), top, &cfg) {
        Ok((hot, restart)) => {
            // An edit renumbers terms, so the edited program's emit keys are
            // not the original's: the goal is found in the override run by
            // its key, and the edited program must draw the same list.
            let same = overridden.as_ref().map(|o| same_draws(o, &hot));
            r.edit_matches_override = same;
            let found = match (&overridden, same) {
                (Some(o), Some(true)) => miss_by_key(o, &want),
                _ => miss(&hot, &want),
            };
            match found {
                Ok(worst) => {
                    r.rerun_error = Some(worst);
                    if same == Some(false) {
                        r.why = "the edited program draws something else than the verified run did".into();
                    } else if worst > tol {
                        r.why = format!("after a hot reload the goal misses by {worst:.3} (tolerance {tol})");
                    }
                }
                Err(m) => r.why = format!("after the edit, {m} is gone"),
            }
            r.restart_matches = match (&overridden, &restart) {
                (Some(o), Some(rs)) => Some(same_draws(o, rs)),
                _ => None,
            };
            if std::env::var_os("DM_STUDY_VERBOSE").is_some() {
                println!(
                    "     re-run: hot reload error {:?}, draws = override run: {:?}; restart draws the same: {:?}",
                    r.rerun_error, r.edit_matches_override, r.restart_matches
                );
            }
        }
        Err(e) => r.why = format!("re-run failed: {e}"),
    }
    let rerun_ok = r.rerun_error.is_some_and(|e| e <= tol) && r.edit_matches_override != Some(false);
    r.success = r.top_verified && rerun_ok && r.expect_ok != Some(false);
    if !r.top_verified {
        r.why = format!("best effort only, top alternative not verified (error {:.1} τ)", top.max_error);
    }
    if !r.success && r.why.is_empty() {
        r.why = if !r.top_verified {
            "top alternative not verified".into()
        } else {
            format!("top alternative edits {:?}, expected ⊆ {:?}", r.top_edits, g.expect.clone().unwrap_or_default())
        };
    }
    r.status = if r.success { "ok" } else { "fail" };
    finish(r)
}

fn finish(mut r: GoalResult) -> GoalResult {
    if r.xfail.is_some() {
        r.status = if r.success { "xpass" } else { "xfail" };
    }
    r
}

struct ExampleResult {
    ex: Example,
    metrics: Metrics,
    load_error: Option<String>,
    goals: Vec<GoalResult>,
}

fn run_example(ex: &Example) -> ExampleResult {
    let mut out = ExampleResult { ex: ex.clone(), metrics: Metrics::default(), load_error: None, goals: Vec::new() };
    let mut ui = match load(ex, &ex.file) {
        Ok(ui) => ui,
        Err(e) => {
            out.load_error = Some(e);
            return out;
        }
    };
    let cfg = spec_config(ex);
    let emits = match baseline(&mut ui, &cfg) {
        Ok(e) => e,
        Err(e) => {
            out.load_error = Some(format!("baseline: {e}"));
            return out;
        }
    };
    out.metrics = metrics(&ui, ex, emits.len());
    if std::env::var_os("DM_STUDY_DUMP").is_some() {
        println!("── {} ({} emits)", ex.id, emits.len());
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for e in &emits {
            let k = counts.entry(e.tag).or_default();
            let args: Vec<String> = e.args.iter().map(|a| match a.as_f64() {
                Some(f) => format!("{f:.2}"),
                None => "…".into(),
            }).collect();
            println!("   {}#{} [{}]", e.tag, k, args.join(", "));
            *k += 1;
        }
    }
    let only_goal = std::env::var("DM_STUDY_GOAL").ok();
    let mut live: Option<LiveCopy> = None;
    for g in &ex.goals {
        if only_goal.as_deref().is_some_and(|o| o != g.name) {
            continue;
        }
        let r = run_goal(ex, g, &mut ui, &mut live, &emits);
        println!(
            "{:<5} {:<28} {:<22} {:<6} runs {:>4} {:>8.1} ms  top {:?}  {}",
            r.level, r.example, r.goal, r.status, r.runs, r.wall_ms, r.top_edits, r.why
        );
        out.goals.push(r);
    }
    out
}

// ── the scaling sweep ────────────────────────────────────────────────────

/// A generated program and the goal to run on it.
struct Generated {
    family: &'static str,
    size: usize,
    source: String,
    goal: &'static str,
    frames: usize,
}

fn generated() -> Vec<Generated> {
    let mut out = Vec::new();
    // A row of N rects; drag the last one right. Expect gap.
    for n in [5, 20, 100, 400, 1600] {
        out.push(Generated {
            family: "row",
            size: n,
            source: format!(
                "import ui\nlet x0 = 10\nlet gap = 2.5\nlet w = 4\nfor i in range(0, {n}) do\n  draw_rect(x0 + i * (w + gap), 50, w, 20, #4080c0)\nend\n"
            ),
            goal: "drag: rect#LAST.0 += 10, rect#0.0 += 0 | expect=gap",
            frames: 1,
        });
    }
    // A binary tree of depth D (2^D leaves); drag the rightmost leaf.
    for d in [2, 4, 6, 8, 10, 12] {
        out.push(Generated {
            family: "tree-depth",
            size: d,
            source: format!(
                "import ui\nlet trunk = 60.0\nlet shrink = 0.72\nlet spread = 0.4\nfn branch(x, y, a, len, depth)\n  let x2 = x + cos(a) * len\n  let y2 = y - sin(a) * len\n  draw_line(x, y, x2, y2, #806040, 255, 1)\n  if depth == 0 then\n    draw_circle(x2, y2, 2, #40c040)\n  else\n    branch(x2, y2, a + spread, len * shrink, depth - 1)\n    branch(x2, y2, a - spread, len * shrink, depth - 1)\n  end\nend\nbranch(400, 580, pi() / 2, trunk, {d})\n"
            ),
            goal: "drag: circle#LAST.0 += 15 | expect=spread,shrink,trunk",
            frames: 1,
        });
    }
    // A projectile simulated for S Euler steps; drag the landing x.
    for s in [10, 100, 1000, 10000, 100000] {
        out.push(Generated {
            family: "sim-steps",
            size: s,
            source: format!(
                "import ui\nlet v0 = 40.0\nlet angle = 0.9\nlet g = 9.8\nlet steps = {s}\nlet dt_sim = 6.0 / steps\nfn simulate()\n  var x = 0.0\n  var y = 0.0\n  var vx = v0 * cos(angle)\n  var vy = v0 * sin(angle)\n  for i in range(0, steps) do\n    set x = x + vx * dt_sim\n    set y = y + vy * dt_sim\n    set vy = vy - g * dt_sim\n  end\n  {{x: x, y: y}}\nend\nlet p = simulate()\ndraw_circle(rigid(20) + p.x, 400 - p.y, 5, #e04040)\n"
            ),
            goal: "drag: circle#0.0 += 20 | expect=v0,angle",
            frames: 1,
        });
    }
    // K literals that each add to one emit (`k_i * 1.0` summed): many
    // equally good parameters, the worst case for screening and ranking.
    for k in [4, 16, 64, 256, 1024] {
        let mut src = String::from("import ui\nlet parts = [");
        for i in 0..k {
            let _ = write!(src, "{}{}.5", if i > 0 { ", " } else { "" }, i % 7);
        }
        src.push_str("]\nlet total = sum(parts)\ndraw_circle(total, 100, 4, #ffffff)\n");
        out.push(Generated { family: "shared-sum", size: k, source: src, goal: "drag: circle#0.0 += 10", frames: 1 });
    }
    // N items in a `map` layout: each item's x is computed by a helper;
    // the emit count grows with N while the parameter count stays fixed.
    for n in [10, 100, 1000] {
        out.push(Generated {
            family: "map-items",
            size: n,
            source: format!(
                "import ui\nlet margin = 12\nlet pitch = 7.5\nfn place(i)\n  {{x: margin + i * pitch, y: 40 + (i % 10) * 3}}\nend\nlet pts = map(range(0, {n}), fn(i) place(i) end)\nfor p in pts do\n  draw_circle(p.x, p.y, 2, #a0a0ff)\nend\n"
            ),
            goal: "drag: circle#3.0 += 6, circle#0.0 += 0 | expect=pitch",
            frames: 1,
        });
    }
    out
}

fn run_generated(gens: &[Generated]) -> Vec<(String, usize, ExampleResult)> {
    let dir = scratch_dir("table");
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = Vec::new();
    for g in gens {
        let file = dir.join(format!("{}-{}.ptl", g.family, g.size));
        std::fs::write(&file, &g.source).unwrap();
        let mut ex = parse_example(&file, &file).unwrap();
        ex.id = format!("{}-{}", g.family, g.size);
        ex.level = "scale".into();
        ex.frames = g.frames;
        // Resolve `LAST` against the baseline once it is known.
        let mut ui = match load(&ex, &file) {
            Ok(ui) => ui,
            Err(e) => {
                out.push((g.family.to_string(), g.size, ExampleResult { ex, metrics: Metrics::default(), load_error: Some(e), goals: vec![] }));
                continue;
            }
        };
        let emits = baseline(&mut ui, &spec_config(&ex)).unwrap_or_default();
        let mut goal = g.goal.to_string();
        for tag in ["rect", "circle"] {
            let n = emits.iter().filter(|e| e.tag == tag).count();
            goal = goal.replace(&format!("{tag}#LAST"), &format!("{tag}#{}", n.saturating_sub(1)));
        }
        ex.goals = vec![parse_goal(&goal).unwrap()];
        drop(ui);
        let r = run_example(&ex);
        out.push((g.family.to_string(), g.size, r));
    }
    let _ = std::fs::remove_dir_all(&dir);
    out
}

// ── the extra pass (external projects, auto-generated goals) ─────────────

fn extra_examples(dir: &Path) -> Vec<Example> {
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap_or_default();
        if !src.contains("draw_") && !src.contains("import ui") {
            continue;
        }
        let root = f.parent().unwrap().to_path_buf();
        let Ok(mut ex) = parse_example(&f, &root) else { continue };
        ex.id = f.strip_prefix(dir).unwrap_or(&f).display().to_string();
        ex.level = "extra".into();
        ex.panel = true;
        if let Some((w, h)) = headless_size(&root) {
            ex.size = (w, h);
        }
        ex.frames = 2;
        ex.goals.clear();
        out.push(ex);
    }
    out
}

/// The `// headless-size: WxH` of an app's `layout.ptl`, if it has one.
fn headless_size(dir: &Path) -> Option<(i32, i32)> {
    let text = std::fs::read_to_string(dir.join("layout.ptl")).ok()?;
    let line = text.lines().find_map(|l| l.trim().strip_prefix("// headless-size:"))?;
    let (w, h) = line.trim().split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Entry files under `dir`: an `app.ptl` (or `main.ptl`) where a directory
/// has one, else every `.ptl` file of a directory without one (screens,
/// sketches); `layout.ptl` launchers and `test`/`dev` directories are left
/// out.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    let has_app = entries.iter().any(|p| p.file_name().is_some_and(|n| n == "app.ptl" || n == "main.ptl"));
    for p in entries {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if p.is_dir() {
            if !["target", "node_modules", "tests", "test", "dev", "packages", "lib", "vendor"].contains(&name.as_str()) {
                walk(&p, out);
            }
        } else if name.ends_with(".ptl") && name != "layout.ptl" && (!has_app || name == "app.ptl" || name == "main.ptl") {
            out.push(p);
        }
    }
}

/// Auto goals for an external program: "move emit k's first numeric arg by
/// +10" for up to `n` sampled circle/rect/text emits. No `expect=`.
fn auto_goals(emits: &[Emit], n: usize) -> Vec<GoalSpec> {
    let mut cands: Vec<(String, usize, usize)> = Vec::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for e in emits {
        let k = *counts.entry(e.tag).and_modify(|c| *c += 1).or_insert(0);
        let arg = match e.tag {
            "circle" | "rect" | "line" | "rect_outline" | "ellipse" => 0,
            "text" => 1,
            _ => continue,
        };
        if e.args.get(arg).and_then(ObsValue::as_f64).is_some() {
            cands.push((e.tag.to_string(), k, arg));
        }
    }
    if cands.is_empty() {
        return vec![];
    }
    let step = (cands.len() as f64 / n as f64).max(1.0);
    (0..n.min(cands.len()))
        .map(|i| {
            let (tag, k, arg) = &cands[(i as f64 * step) as usize];
            GoalSpec {
                name: format!("auto{i}"),
                targets: vec![Target { addr: Addr { tag: tag.clone(), k: *k, arg: *arg, path: vec![] }, op: Op::By(10.0) }],
                ..Default::default()
            }
        })
        .collect()
}

fn run_extra(dir: &Path, per_example: usize) -> Vec<ExampleResult> {
    let mut out = Vec::new();
    for mut ex in extra_examples(dir) {
        let mut ui = match load(&ex, &ex.file) {
            Ok(ui) => ui,
            Err(e) => {
                println!("extra {:<40} not runnable headless: {}", ex.id, first_line(&e));
                out.push(ExampleResult { ex, metrics: Metrics::default(), load_error: Some(first_line(&e)), goals: vec![] });
                continue;
            }
        };
        let emits = match baseline(&mut ui, &spec_config(&ex)) {
            Ok(e) => e,
            Err(e) => {
                out.push(ExampleResult { ex, metrics: Metrics::default(), load_error: Some(first_line(&e)), goals: vec![] });
                continue;
            }
        };
        ex.goals = auto_goals(&emits, per_example);
        drop(ui);
        out.push(run_example(&ex));
    }
    out
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(160).collect()
}

// ── the report ───────────────────────────────────────────────────────────

fn goal_json(r: &GoalResult) -> Json {
    json!({
        "goal": r.goal, "targets": r.targets, "status": r.status, "success": r.success,
        "why": r.why, "top_verified": r.top_verified, "any_verified": r.any_verified,
        "expect_ok": r.expect_ok, "rerun_error": r.rerun_error, "edit_matches_override": r.edit_matches_override, "restart_matches": r.restart_matches, "max_error_tau": r.max_error_tau,
        "runs": r.runs, "wall_ms": (r.wall_ms * 10.0).round() / 10.0,
        "candidates": {"discovered": r.discovered, "eligible": r.eligible, "screened": r.screened,
                       "measured": r.measured, "truncated": r.truncated},
        "alternatives": r.alternatives, "verified_alternatives": r.verified_alternatives,
        "top_edits": r.top_edits, "other_alternatives": r.other_alternatives, "aids": r.aids,
        "stateful": r.stateful, "regime_changed": r.regime_changed,
        "rng_order_changed": r.rng_order_changed, "budget_exhausted": r.budget_exhausted,
        "iterations": r.iterations, "collateral_items": r.collateral, "xfail": r.xfail,
    })
}

fn example_json(e: &ExampleResult) -> Json {
    let m = &e.metrics;
    json!({
        "id": e.ex.id, "level": e.ex.level, "title": e.ex.title,
        "file": e.ex.file.strip_prefix(repo_root()).unwrap_or(&e.ex.file).display().to_string(),
        "load_error": e.load_error,
        "metrics": {"files": m.files, "lines": m.lines, "fns": m.fns, "loops": m.loops, "vars": m.vars,
                    "states": m.states, "configs": m.configs, "hints": m.hints, "emits": m.emits,
                    "terms": m.terms, "reads_clock": m.reads_clock, "reads_rng": m.reads_rng},
        "goals": e.goals.iter().map(goal_json).collect::<Vec<_>>(),
    })
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn summary(rs: &[&GoalResult]) -> Json {
    let n = rs.len();
    let ok = rs.iter().filter(|r| r.success).count();
    let tv = rs.iter().filter(|r| r.top_verified).count();
    let av = rs.iter().filter(|r| r.any_verified).count();
    json!({
        "goals": n, "success": ok, "top_verified": tv, "any_verified": av,
        "success_rate": if n > 0 { ok as f64 / n as f64 } else { 0.0 },
        "top_verified_rate": if n > 0 { tv as f64 / n as f64 } else { 0.0 },
        "median_runs": median(rs.iter().map(|r| r.runs as f64).collect()),
        "median_wall_ms": median(rs.iter().map(|r| r.wall_ms).collect()),
        "max_runs": rs.iter().map(|r| r.runs).max().unwrap_or(0),
    })
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn md_goal_rows(out: &mut String, rs: &[ExampleResult]) {
    out.push_str("| Example | Goal | Status | Top edit | Runs | ms | Cands d/e/s/m | Alts (verified) | Aids | Error |\n");
    out.push_str("|---|---|---|---|---:|---:|---|---|---|---|\n");
    for e in rs {
        if let Some(err) = &e.load_error {
            let _ = writeln!(out, "| `{}` | — | load error | {} | | | | | | |", e.ex.id, md_escape(err));
            continue;
        }
        for r in &e.goals {
            let status = match r.status {
                "ok" => "ok".to_string(),
                s => format!("**{s}**"),
            };
            let edits = if r.top_edits.is_empty() { "—".into() } else { format!("`{}`", r.top_edits.join("`, `")) };
            let err = r.rerun_error.map(|e| format!("{e:.2}")).unwrap_or_else(|| "—".into());
            let mut note = String::new();
            if !r.why.is_empty() && r.status != "ok" {
                let why: String = r.why.chars().take(140).collect();
                let more = if r.why.chars().count() > 140 { "…" } else { "" };
                note = format!(" ({}{more})", md_escape(&why));
            }
            if let Some(x) = &r.xfail {
                note = format!(" (expected: {})", md_escape(x));
            }
            let _ = writeln!(
                out,
                "| `{}` | {} | {}{} | {} | {} | {:.0} | {}/{}/{}/{}{} | {} ({}) | {} | {} |",
                e.ex.id,
                md_escape(&r.goal),
                status,
                note,
                edits,
                r.runs,
                r.wall_ms,
                r.discovered,
                r.eligible,
                r.screened,
                r.measured,
                if r.truncated { "+" } else { "" },
                r.alternatives,
                r.verified_alternatives,
                if r.aids.is_empty() { "—".into() } else { r.aids.join(", ") },
                err,
            );
        }
    }
}

fn write_report(ladder: &[ExampleResult], scaling: &[(String, usize, ExampleResult)], extra: &[ExampleResult], extra_dir: Option<&str>) {
    let root = repo_root();
    let all_ladder: Vec<&GoalResult> = ladder.iter().flat_map(|e| &e.goals).collect();
    let expected: Vec<&GoalResult> = all_ladder.iter().copied().filter(|r| r.xfail.is_none()).collect();
    let all_extra: Vec<&GoalResult> = extra.iter().flat_map(|e| &e.goals).collect();
    let doc = json!({
        "generated": "core-libs/petal-ui/tests/dm_v2_study.rs (DM_STUDY_WRITE=1)",
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "ladder": {
            "summary": summary(&all_ladder),
            "summary_expected": summary(&expected),
            "examples": ladder.iter().map(example_json).collect::<Vec<_>>(),
        },
        "scaling": scaling.iter().map(|(f, n, e)| json!({"family": f, "size": n, "result": example_json(e)})).collect::<Vec<_>>(),
        "extra": {
            "dir": extra_dir.map(|d| d.replace(&(root.display().to_string() + "/"), "")),
            "summary": summary(&all_extra),
            "examples": extra.iter().map(example_json).collect::<Vec<_>>(),
        },
    });
    std::fs::write(
        root.join("docs/dev/direct-manipulation-v2-study.json"),
        serde_json::to_string_pretty(&doc).unwrap() + "\n",
    )
    .unwrap();

    // The generated tables go between markers in the study doc, so the
    // hand-written analysis around them survives a re-run.
    let mut table = String::new();
    let s = summary(&all_ladder);
    let se = summary(&expected);
    let _ = writeln!(
        table,
        "Build: {}. Ladder: {} goals over {} examples; {} succeed ({:.0}%), top alternative verified on {} ({:.0}%), some alternative verified on {}. Goals not marked as expected failures: {}/{} succeed. Median {} runs and {:.0} ms per goal, at most {} runs.\n",
        if cfg!(debug_assertions) { "debug" } else { "release" },
        s["goals"], ladder.len(), s["success"], s["success_rate"].as_f64().unwrap() * 100.0,
        s["top_verified"], s["top_verified_rate"].as_f64().unwrap() * 100.0, s["any_verified"],
        se["success"], se["goals"], s["median_runs"], s["median_wall_ms"].as_f64().unwrap(), s["max_runs"],
    );
    table.push_str("#### Examples\n\n| Example | Level | Lines | Fns | Loops | var | state | config | hints | Emits | Clock | RNG |\n|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---|\n");
    for e in ladder {
        let m = &e.metrics;
        let _ = writeln!(
            table,
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            e.ex.id, e.ex.level, m.lines, m.fns, m.loops, m.vars, m.states, m.configs, m.hints, m.emits,
            if m.reads_clock { "yes" } else { "" }, if m.reads_rng { "yes" } else { "" }
        );
    }
    table.push_str("\n#### Goals\n\n");
    md_goal_rows(&mut table, ladder);
    table.push_str("\n#### Scaling sweep\n\n| Family | Size | Emits | Status | Top edit | Runs | ms | Cands d/e/s/m | Note |\n|---|---:|---:|---|---|---:|---:|---|---|\n");
    for (f, n, e) in scaling {
        let Some(r) = e.goals.first() else {
            let _ = writeln!(table, "| {f} | {n} | | load error | | | | | {} |", md_escape(e.load_error.as_deref().unwrap_or("")));
            continue;
        };
        let _ = writeln!(
            table,
            "| {f} | {n} | {} | {} | {} | {} | {:.0} | {}/{}/{}/{}{} | {} |",
            e.metrics.emits, r.status,
            if r.top_edits.is_empty() { "—".into() } else { format!("`{}`", r.top_edits.join("`, `")) },
            r.runs, r.wall_ms, r.discovered, r.eligible, r.screened, r.measured, if r.truncated { "+" } else { "" },
            md_escape(&r.why)
        );
    }
    if let Some(d) = extra_dir {
        let s = summary(&all_extra);
        let runnable = extra.iter().filter(|e| e.load_error.is_none()).count();
        let home = repo_root().display().to_string() + "/";
        let dirs = d.replace(&home, "");
        let _ = writeln!(
            table,
            "\n#### External projects (auto-generated goals)\n\nDirectories: `{}`.\n\n{} entry files, {} runnable headless. {} auto goals (\"move emit k's x by +10\"): top alternative verified on {} ({:.0}%), and the edit re-run within tolerance on {}; median {} runs, {:.0} ms.\n",
            dirs.replace(':', "`, `"),
            extra.len(), runnable, s["goals"], s["top_verified"], s["top_verified_rate"].as_f64().unwrap() * 100.0,
            s["success"], s["median_runs"], s["median_wall_ms"].as_f64().unwrap()
        );
        table.push_str("| Project | Lines | Emits | Goals | Verified | Re-run ok | Median runs | Median ms |\n|---|---:|---:|---:|---:|---:|---:|---:|\n");
        for e in extra.iter().filter(|e| e.load_error.is_none() && !e.goals.is_empty()) {
            let gs: Vec<&GoalResult> = e.goals.iter().collect();
            let sm = summary(&gs);
            let _ = writeln!(
                table,
                "| `{}` | {} | {} | {} | {} | {} | {} | {:.0} |",
                e.ex.id, e.metrics.lines, e.metrics.emits, sm["goals"], sm["top_verified"], sm["success"], sm["median_runs"],
                sm["median_wall_ms"].as_f64().unwrap()
            );
        }
        table.push_str("\nGoals that did not succeed:\n\n");
        md_goal_rows(&mut table, &extra.iter().filter(|e| e.goals.iter().any(|g| !g.success)).map(|e| ExampleResult {
            ex: e.ex.clone(),
            metrics: e.metrics.clone(),
            load_error: None,
            goals: e.goals.iter().filter(|g| !g.success).cloned().collect(),
        }).collect::<Vec<_>>());
        let mut why: BTreeMap<String, usize> = BTreeMap::new();
        for e in extra.iter().filter(|e| e.load_error.is_some()) {
            let w = e.load_error.as_deref().unwrap_or("");
            let key = if w.contains("cannot find module") || w.contains("requires the host interface") {
                "needs its embedder's modules (cheesecake)".to_string()
            } else if w.contains("needs native fn") || w.contains("needs a native fn") {
                format!("needs native fns of another host ({})", w.split("module '").nth(1).and_then(|r| r.split('\'').next()).unwrap_or("?"))
            } else if w.contains("Unknown function") {
                format!("calls a host function the headless runtime lacks ({})", w.split("Unknown function: ").nth(1).and_then(|r| r.split(' ').next()).unwrap_or("?"))
            } else {
                first_line(w).chars().take(80).collect()
            };
            *why.entry(key).or_default() += 1;
        }
        table.push_str("\nNot runnable headless:\n\n| Why | Files |\n|---|---:|\n");
        for (w, n) in why {
            let _ = writeln!(table, "| {} | {n} |", md_escape(&w));
        }
    }

    let path = root.join("docs/dev/direct-manipulation-v2-study.md");
    let doc = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        "# Direct Manipulation v2: complexity study\n\n<!-- study:begin -->\n<!-- study:end -->\n".to_string()
    });
    let (head, rest) = doc.split_once("<!-- study:begin -->").expect("study:begin marker");
    let (_, tail) = rest.split_once("<!-- study:end -->").expect("study:end marker");
    let out = format!("{head}<!-- study:begin -->\n<!-- Generated by core-libs/petal-ui/tests/dm_v2_study.rs; do not edit by hand. -->\n\n{table}\n<!-- study:end -->{tail}");
    std::fs::write(&path, out).unwrap();
    println!("wrote {} and the .json beside it", path.display());
}

// ── the tests ────────────────────────────────────────────────────────────

/// The ladder: every goal not marked `xfail=` succeeds.
#[test]
fn ladder() {
    let results: Vec<ExampleResult> = ladder_examples().iter().map(run_example).collect();
    let mut failures = Vec::new();
    for e in &results {
        if let Some(err) = &e.load_error {
            failures.push(format!("{}: {err}", e.ex.id));
        }
        for r in &e.goals {
            if r.status != "ok" && r.status != "xfail" && r.status != "xpass" {
                failures.push(format!("{} / {}: {} — {} (top {:?})", r.example, r.goal, r.status, r.why, r.top_edits));
            }
            if r.status == "xpass" {
                println!("note: {} / {} passes although marked xfail ({})", r.example, r.goal, r.xfail.as_deref().unwrap_or(""));
            }
        }
    }
    assert!(failures.is_empty(), "{} goals failed:\n{}", failures.len(), failures.join("\n"));
}

/// The whole study: the ladder, the scaling sweep and (with
/// `DM_STUDY_EXTRA`) the external projects. Slow; it asserts nothing and
/// writes the report with `DM_STUDY_WRITE=1`.
#[test]
#[ignore]
fn full_study() {
    RESTART.with(|r| r.set(true));
    let ladder: Vec<ExampleResult> = ladder_examples().iter().map(run_example).collect();
    let scaling = run_generated(&generated());
    let extra_dir = std::env::var("DM_STUDY_EXTRA").ok();
    let per: usize = std::env::var("DM_STUDY_EXTRA_GOALS").ok().and_then(|n| n.parse().ok()).unwrap_or(6);
    let extra: Vec<ExampleResult> = extra_dir
        .as_deref()
        .map(|d| d.split(':').flat_map(|d| run_extra(Path::new(d), per)).collect())
        .unwrap_or_default();
    if std::env::var_os("DM_STUDY_WRITE").is_some() {
        write_report(&ladder, &scaling, &extra, extra_dir.as_deref());
    }
}

/// Debugging aid: one literal of an example run with an override two ways,
/// slot override and recompiled text, compared emit by emit.
/// `DM_STUDY_PROBE=<file>:<line>:<literal text>:<new value>`.
#[test]
#[ignore]
fn probe_override() {
    use petal::dm_v2::spec::{OverrideStrategy, OverrideValue, Overrides};
    use petal::program::TermOp;
    let Ok(spec) = std::env::var("DM_STUDY_PROBE") else { return };
    let parts: Vec<&str> = spec.rsplitn(4, ':').collect();
    let (value, text, line, file) = (parts[0].parse::<f64>().unwrap(), parts[1], parts[2].parse::<u32>().unwrap(), parts[3]);
    let file = PathBuf::from(file);
    let mut ex = parse_example(&file, file.parent().unwrap()).unwrap();
    ex.panel = true;
    ex.frames = 2;
    for strategy in [OverrideStrategy::Slots, OverrideStrategy::Recompile] {
        let mut ui = load(&ex, &file).unwrap();
        let (pid, sid) = (ui.program_id(), ui.stack_id());
        let term = {
            let program = ui.env.get_program(pid).unwrap();
            let src = &program.source_map.files[0].source;
            let chars: Vec<char> = src.chars().collect();
            program
                .terms
                .iter()
                .filter(|t| matches!(t.op, TermOp::Constant(_)))
                .find(|t| {
                    program.source_map.get(t.id).is_some_and(|s| {
                        s.file.0 == 0
                            && s.start.line == line
                            && chars.get(s.start.offset as usize..s.end.offset as usize).map(|c| c.iter().collect::<String>()).as_deref() == Some(text)
                    })
                })
                .map(|t| t.id)
                .expect("the literal")
        };
        let cfg = SpecConfig { strategy, ..SpecConfig::default() };
        let (env, mut host) = ui.spec_parts();
        let mut s = SpecSession::begin(env, &mut host, sid, &[term], cfg, None).unwrap();
        let run = s.run(env, &mut host, &Overrides::one(term, OverrideValue::Num(value)));
        println!("{strategy:?}: error {:?}, {} emits (baseline {})", run.error, run.emits.len(), s.baseline().emits.len());
        s.end(env);
    }
}
