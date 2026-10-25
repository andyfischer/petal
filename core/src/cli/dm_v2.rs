//! `propose-edit --v2`: the command-line surface of direct manipulation v2
//! (docs/dev/direct-manipulation-v2-plan.md §4.2).
//!
//! The pieces here are public so that every binary that can host a script
//! (the core `petal`, and `petal-ui-run` for scripts that draw) shares one
//! flag vocabulary and one `--json` shape:
//!
//! - [`V2Parser`] reads the goal and filter flags out of an argument list;
//!   [`V2Opts`] is what it builds.
//! - [`propose`] resolves the goals against a speculative baseline of the
//!   checkpoint, runs [`crate::dm_v2::DmSession`], applies the chosen
//!   alternative when asked, and returns the report as JSON.
//! - [`print_text`] renders that report for people.
//!
//! The JSON shape is frozen by `core/tests/cli_propose_edit_v2.rs` (M5's
//! study harness reads it); add fields, do not rename them.

use std::path::PathBuf;

use serde_json::{Value as Json, json};

use crate::dm_v2::params::{ParamFilter, ParamSelector};
use crate::dm_v2::spec::{SpecConfig, SpecHost, SpecSession};
use crate::dm_v2::{
    Alternative, CandidateReport, ColumnQuality, DmGoal, DmRequest, DmResponse, DmSession, EmitKey, Engine,
    Hint, ObsValue, Observable, PathStep, Prefer, apply_edits_multi, program_files,
};
use crate::env::Env;
use crate::program::{Program, ProgramId, TermId};
use crate::source_map::{ENTRY_FILE, SourceSpan};
use crate::stack::StackKey;
use crate::static_value::StaticValue;

/// What a goal addresses within its emit.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalAddr {
    /// `--arg k`: argument `k` of the user-file call that made the emit
    /// (v1's addressing), observed as a watched term.
    Arg(usize),
    /// `--native-arg k`: the emitting native's raw argument `k`.
    NativeArg(usize),
    /// `--value`: the emitted value itself.
    Value,
}

/// Where a goal wants its number to go.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalValue {
    /// `--to v`: an absolute value, as source-ish text (`55`, `2.5`, or a
    /// non-number, which v2 hands to v1).
    To(String),
    /// `--by d`: relative to the baseline value.
    By(f64),
}

/// One goal of a `propose-edit --v2` invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalSpec {
    pub channel: String,
    pub emit: usize,
    pub addr: GoalAddr,
    /// `--field a.b.0`: a path into the addressed value.
    pub path: Vec<PathStep>,
    pub value: GoalValue,
}

/// The parsed v2 options.
#[derive(Debug, Clone, PartialEq)]
pub struct V2Opts {
    pub json: bool,
    pub goals: Vec<GoalSpec>,
    /// `--allow` selectors, in the spelling of [`parse_selector`]
    /// (`--configurable x` adds `x`).
    pub allow: Vec<String>,
    /// `--deny` selectors (`--static x` adds `x`).
    pub deny: Vec<String>,
    /// `--config-strict`: v1's `config let` rule ([`Prefer::ConfigStrict`]).
    pub config_strict: bool,
    pub alternatives: usize,
    pub explain: bool,
    pub apply: bool,
    /// `--pick N`, 1-based.
    pub pick: Option<usize>,
    pub tol: Option<f64>,
    pub max_runs: Option<usize>,
    pub max_screen: Option<usize>,
    pub allow_respell: bool,
    /// `--seed`; `None` is `PETAL_SEED`, else 0 ([`effective_seed`]).
    pub seed: Option<u64>,
}

impl Default for V2Opts {
    fn default() -> Self {
        V2Opts {
            json: false,
            goals: Vec::new(),
            allow: Vec::new(),
            deny: Vec::new(),
            config_strict: false,
            alternatives: 3,
            explain: false,
            apply: false,
            pick: None,
            tol: None,
            max_runs: None,
            max_screen: None,
            allow_respell: false,
            seed: None,
        }
    }
}

/// The seed a v2 invocation runs with: `--seed`, else `PETAL_SEED`, else 0.
/// It is echoed in the report, so `run --trace-emits` and `propose-edit`
/// can be made to agree on which shape "emit 7" is.
pub fn effective_seed(flag: Option<u64>) -> u64 {
    flag.or_else(|| std::env::var("PETAL_SEED").ok()?.trim().parse().ok()).unwrap_or(0)
}

/// Parse an `--allow`/`--deny` selector:
///
/// | spelling | selects |
/// |---|---|
/// | `NAME` | the binding `NAME` (base or module-qualified name) |
/// | `line:N` | literals on line `N` of the entry file |
/// | `fn:NAME` | literals in the body of function `NAME` |
/// | `config` | `config let` and `tunable` literals |
/// | `user` | every literal in user code |
/// | `lib` | screen library literals for the report (never edited) |
/// | `path-deps` | let local path dependencies be edited |
/// | `state-init` | let `state` initializers move |
/// | `structural` | let literals that only choose control flow move |
/// | `rng` | let literals that only feed random draws move |
pub fn parse_selector(s: &str) -> Result<ParamSelector, String> {
    if let Some(n) = s.strip_prefix("line:") {
        return n
            .parse()
            .map(ParamSelector::Line)
            .map_err(|_| format!("selector `{s}`: line:N takes a 1-based line number"));
    }
    if let Some(name) = s.strip_prefix("fn:") {
        if name.is_empty() {
            return Err(format!("selector `{s}`: fn:NAME needs a function name"));
        }
        return Ok(ParamSelector::Function(name.to_string()));
    }
    Ok(match s {
        // Every knob: `config let` and `tunable` literals, one tier.
        "config" | "knob" => ParamSelector::ConfigOnly,
        "user" => ParamSelector::AllUserCode,
        "lib" => ParamSelector::ReportLibrary,
        "path-deps" => ParamSelector::AllowPathDeps,
        "state-init" => ParamSelector::IncludeStateInit,
        "structural" => ParamSelector::IncludeStructural,
        "rng" => ParamSelector::IncludeThroughRng,
        "" => return Err("empty selector".into()),
        name if name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ':') => {
            ParamSelector::Variable(name.to_string())
        }
        other => return Err(format!("unknown selector `{other}`")),
    })
}

/// Parse a `--field` path: dot-separated, a number is a list index
/// (`center.0`, `points.2.x`).
pub fn parse_path(s: &str) -> Vec<PathStep> {
    s.split('.')
        .filter(|p| !p.is_empty())
        .map(|p| match p.parse::<usize>() {
            Ok(i) => PathStep::Index(i),
            Err(_) => PathStep::Field(p.to_string()),
        })
        .collect()
}

/// Reads the v2 flags of `propose-edit` one at a time, so a host binary can
/// mix them with its own (`--frames`, `--size`, …).
#[derive(Debug, Default)]
pub struct V2Parser {
    pub opts: V2Opts,
    channel: Option<String>,
    emit: Option<usize>,
    pending: Option<GoalAddr>,
    pending_path: Vec<PathStep>,
    /// The v2-only flags seen (for "this needs --v2" errors under `--v1`).
    pub v2_only: Vec<String>,
    /// More than one `(channel, emit)` was addressed.
    pub multi_emit: bool,
}

impl V2Parser {
    pub fn new() -> V2Parser {
        V2Parser::default()
    }

    /// Consume `args[*i]` when it is a propose-edit flag, leaving `*i` on the
    /// last token consumed (the flag, or its value), as the core CLI's flag
    /// loops expect. `Ok(false)` leaves it for the caller.
    pub fn flag(&mut self, args: &[String], i: &mut usize) -> Result<bool, String> {
        let flag = args[*i].clone();
        let value = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("Expected a value after {flag}."))
        };
        fn index(flag: &str, v: &str) -> Result<usize, String> {
            v.parse().map_err(|_| format!("{flag} takes a 0-based index, not `{v}`"))
        }
        fn number(flag: &str, v: &str) -> Result<f64, String> {
            v.parse().map_err(|_| format!("{flag} takes a number, not `{v}`"))
        }
        let v2_only = [
            "--native-arg",
            "--value",
            "--field",
            "--by",
            "--allow",
            "--deny",
            "--config-strict",
            "--alternatives",
            "--explain",
            "--pick",
            "--tol",
            "--max-runs",
            "--max-screen",
            "--allow-respell",
            "--seed",
        ];
        match flag.as_str() {
            "--json" => self.opts.json = true,
            "--apply" => self.opts.apply = true,
            "--channel" => {
                let c = value(i)?;
                self.address(Some(c), None);
            }
            "--emit" => {
                let v = value(i)?;
                let e = index("--emit", &v)?;
                self.address(None, Some(e));
            }
            "--arg" | "--native-arg" | "--value" => {
                if self.pending.is_some() {
                    return Err(format!("{flag} given before the previous goal's --to/--by"));
                }
                self.pending = Some(match flag.as_str() {
                    "--arg" => GoalAddr::Arg(index(&flag, &value(i)?)?),
                    "--native-arg" => GoalAddr::NativeArg(index(&flag, &value(i)?)?),
                    _ => GoalAddr::Value,
                });
                self.pending_path.clear();
            }
            "--field" => {
                if self.pending.is_none() {
                    return Err("--field needs an --arg, --native-arg or --value before it".into());
                }
                self.pending_path = parse_path(&value(i)?);
            }
            "--to" | "--by" => {
                let Some(addr) = self.pending.take() else {
                    return Err(format!("{flag} needs an --arg, --native-arg or --value before it"));
                };
                let (Some(channel), Some(emit)) = (self.channel.clone(), self.emit) else {
                    return Err(format!("{flag} needs --channel and --emit before it"));
                };
                let v = value(i)?;
                let value = if flag == "--to" { GoalValue::To(v) } else { GoalValue::By(number("--by", &v)?) };
                self.opts.goals.push(GoalSpec {
                    channel,
                    emit,
                    addr,
                    path: std::mem::take(&mut self.pending_path),
                    value,
                });
            }
            "--allow" | "--configurable" => self.opts.allow.push(value(i)?),
            "--deny" | "--static" => self.opts.deny.push(value(i)?),
            "--config-strict" => self.opts.config_strict = true,
            "--alternatives" => {
                let v = value(i)?;
                self.opts.alternatives = v
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("--alternatives takes a positive count, not `{v}`"))?;
            }
            "--explain" => self.opts.explain = true,
            "--pick" => {
                let v = value(i)?;
                self.opts.pick =
                    Some(v.parse().ok().filter(|n| *n > 0).ok_or_else(|| format!("--pick takes a 1-based rank, not `{v}`"))?);
            }
            "--tol" => self.opts.tol = Some(number("--tol", &value(i)?)?.abs()),
            "--max-runs" => self.opts.max_runs = Some(index("--max-runs", &value(i)?)?),
            "--max-screen" => self.opts.max_screen = Some(index("--max-screen", &value(i)?)?),
            "--allow-respell" => self.opts.allow_respell = true,
            "--seed" => {
                let v = value(i)?;
                self.opts.seed = Some(v.parse().map_err(|_| format!("--seed takes an integer, not `{v}`"))?);
            }
            _ => return Ok(false),
        }
        if v2_only.contains(&flag.as_str()) && !self.v2_only.contains(&flag) {
            self.v2_only.push(flag);
        }
        Ok(true)
    }

    fn address(&mut self, channel: Option<String>, emit: Option<usize>) {
        let had = self.opts.goals.last().map(|g| (g.channel.clone(), g.emit));
        if let Some(c) = channel {
            self.channel = Some(c);
        }
        if let Some(e) = emit {
            self.emit = Some(e);
        }
        if let (Some((c, e)), Some(nc), Some(ne)) = (had, &self.channel, self.emit)
            && (c != *nc || e != ne)
        {
            self.multi_emit = true;
        }
    }

    /// Finish: the options, or why they are incomplete.
    pub fn finish(self) -> Result<V2Opts, String> {
        if self.pending.is_some() {
            return Err("a goal address without a matching --to or --by".into());
        }
        if self.opts.goals.is_empty() {
            return Err("at least one goal is required: --channel <name> --emit <n> \
                        (--arg|--native-arg <k> | --value) [--field <path>] (--to <v> | --by <d>)"
                .into());
        }
        for s in self.opts.allow.iter().chain(&self.opts.deny) {
            parse_selector(s)?;
        }
        Ok(self.opts)
    }
}

/// A failure of [`propose`]: the message and the `--json` error phase.
#[derive(Debug, Clone, PartialEq)]
pub struct CliDmError {
    pub message: String,
    pub phase: &'static str,
}

fn fail(phase: &'static str, message: impl Into<String>) -> CliDmError {
    CliDmError { message: message.into(), phase }
}

/// One goal resolved against the baseline.
struct Resolved {
    observable: Observable,
    key: EmitKey,
    /// The watched argument term of an `--arg` goal.
    watch: Option<TermId>,
    callee: Option<String>,
    line: Option<u32>,
}

/// Run `propose-edit --v2` on the program `pid`, from `checkpoint` (a fresh
/// stack on the CLI), and return the report. With `opts.apply`, the chosen
/// alternative is written to every file it touches; `entry_path` is where
/// the entry file is on disk (`None` for inline code, which cannot be
/// applied).
///
/// The caller sets the env's seed ([`effective_seed`]) before creating the
/// checkpoint's stack.
pub fn propose(
    env: &mut Env,
    host: &mut dyn SpecHost,
    pid: ProgramId,
    checkpoint: StackKey,
    opts: &V2Opts,
    entry_path: Option<PathBuf>,
) -> Result<Json, CliDmError> {
    let seed = effective_seed(opts.seed);
    let spec_cfg = SpecConfig { session_seed: seed, ..SpecConfig::default() };

    // Resolve each goal's emit to its key against one baseline.
    let resolved = resolve_goals(env, host, pid, checkpoint, opts, &spec_cfg)?;

    let mut filter = ParamFilter::default();
    if !opts.allow.is_empty() {
        filter.allow =
            Some(opts.allow.iter().map(|s| parse_selector(s)).collect::<Result<_, _>>().map_err(|e| fail("goal", e))?);
    }
    filter.deny = opts.deny.iter().map(|s| parse_selector(s)).collect::<Result<_, _>>().map_err(|e| fail("goal", e))?;

    let mut req = DmRequest::default();
    for (g, r) in opts.goals.iter().zip(&resolved) {
        let mut goal = match &g.value {
            GoalValue::To(text) => match parse_goal_value(text) {
                StaticValue::Int(n) => DmGoal::to(r.observable.clone(), n as f64),
                StaticValue::Float(f) => DmGoal::to(r.observable.clone(), f),
                other => DmGoal::value(r.observable.clone(), other),
            },
            // The target is set from the baseline when the session finishes.
            GoalValue::By(_) => DmGoal::to(r.observable.clone(), f64::NAN),
        };
        goal.tolerance = opts.tol;
        req.goals.push(goal);
        req.holds.push(matches!(g.value, GoalValue::By(d) if d == 0.0));
    }
    req.filter = filter;
    req.prefer = if opts.config_strict { Prefer::ConfigStrict } else { Prefer::Soft };
    req.max_alternatives = opts.alternatives.max(1);
    if let Some(n) = opts.max_runs {
        req.max_runs = n;
    }
    if let Some(n) = opts.max_screen {
        req.max_screen = n;
    }
    req.allow_respell = opts.allow_respell;
    req.explain = opts.explain;
    req.spec = spec_cfg;
    req.spec.capture.watch = resolved.iter().filter_map(|r| r.watch).collect();

    let session = DmSession::open(env, host, checkpoint, &req).map_err(|e| fail("goal", e.to_string()))?;
    let baseline: Vec<f64> = session.baseline_goals().to_vec();
    let targets: Vec<f64> = opts
        .goals
        .iter()
        .enumerate()
        .map(|(i, g)| match (&g.value, &req.goals[i].target) {
            (GoalValue::By(d), _) => baseline.get(i).copied().unwrap_or(f64::NAN) + d,
            (_, crate::dm_v2::GoalTarget::Num(t)) => *t,
            _ => f64::NAN,
        })
        .collect();
    let response = session.finish(env, host, &targets).map_err(|e| fail("goal", e.to_string()))?;

    let program = env.get_program(pid).ok_or_else(|| fail("goal", "the program is gone"))?;
    let applied = if opts.apply { Some(apply(program, &response, opts.pick, entry_path)?) } else { None };
    Ok(report(program, opts, seed, &resolved, &baseline, &targets, &response, applied))
}

// ── embedding: a JSON request, a host's natives ────────────────────────────

/// The keys a JSON request ([`opts_from_json`]) may carry for one goal.
const GOAL_KEYS: [&str; 8] = ["channel", "emit", "arg", "native_arg", "value", "field", "to", "by"];

/// Read a JSON request into [`V2Opts`], for hosts that drive propose-edit
/// without a command line (the C bridge's `pb_vm_propose_edit`, a petal-ui
/// host). The keys are the CLI flags', snake_cased:
///
/// ```json
/// {"channel": "shapes", "emit": 2, "native_arg": 1, "by": 25,
///  "allow": ["config"], "deny": ["x"], "tol": 0.5, "explain": true}
/// ```
///
/// A goal is `channel`, `emit`, one of `arg` / `native_arg` (a 0-based
/// index) / `value: true`, an optional `field` (`"center.0"` or
/// `["center", 0]`), and one of `to` (a number, or any JSON scalar for a
/// non-numeric goal) / `by` (a number). One goal may sit at the top level;
/// several go in `goals: [...]`, each inheriting the top-level `channel` and
/// `emit` it leaves out. The options are `allow` / `deny` (a selector or a
/// list of them; `pins` is `deny`'s alias), `config_strict`, `alternatives`,
/// `explain`, `apply`, `pick` (1-based), `tol`, `max_runs`, `max_screen`,
/// `allow_respell` and `seed`; `json` is accepted and ignored. An unknown key
/// is an error, so a misspelt option is not silently dropped.
///
/// The request is translated into the flag list and read by [`V2Parser`],
/// so its rules are the CLI's exactly.
pub fn opts_from_json(req: &Json) -> Result<V2Opts, String> {
    let obj = req.as_object().ok_or("the request must be a JSON object")?;
    const OPTION_KEYS: [&str; 15] = [
        "goals", "allow", "deny", "pins", "config_strict", "alternatives", "explain", "apply", "pick", "tol",
        "max_runs", "max_screen", "allow_respell", "seed", "json",
    ];
    for k in obj.keys() {
        if !GOAL_KEYS.contains(&k.as_str()) && !OPTION_KEYS.contains(&k.as_str()) {
            return Err(format!("unknown request key `{k}`"));
        }
    }
    fn scalar(key: &str, v: &Json) -> Result<String, String> {
        match v {
            Json::Number(n) => Ok(n.to_string()),
            Json::String(s) => Ok(s.clone()),
            Json::Bool(b) => Ok(b.to_string()),
            Json::Null => Ok("nil".into()),
            _ => Err(format!("`{key}` takes a number or a string, not {v}")),
        }
    }
    fn flag_on(obj: &serde_json::Map<String, Json>, key: &str) -> Result<bool, String> {
        match obj.get(key) {
            None | Some(Json::Null) => Ok(false),
            Some(Json::Bool(b)) => Ok(*b),
            Some(v) => Err(format!("`{key}` takes true or false, not {v}")),
        }
    }
    let mut args: Vec<String> = Vec::new();
    // `nested`: a member of `goals`, whose keys are checked here (the top
    // level's were checked above, and it also carries the options).
    let mut push_goal = |g: &serde_json::Map<String, Json>, nested: bool| -> Result<(), String> {
        for k in g.keys().filter(|_| nested) {
            if !GOAL_KEYS.contains(&k.as_str()) {
                return Err(format!("unknown goal key `{k}`"));
            }
        }
        for key in ["channel", "emit"] {
            if let Some(v) = g.get(key).or_else(|| obj.get(key)) {
                args.push(format!("--{key}"));
                args.push(scalar(key, v)?);
            }
        }
        match (g.get("arg"), g.get("native_arg"), flag_on(g, "value")?) {
            (Some(k), None, false) => args.extend(["--arg".into(), scalar("arg", k)?]),
            (None, Some(k), false) => args.extend(["--native-arg".into(), scalar("native_arg", k)?]),
            (None, None, true) => args.push("--value".into()),
            (None, None, false) => return Err("a goal needs one of `arg`, `native_arg` or `value: true`".into()),
            _ => return Err("a goal takes only one of `arg`, `native_arg` and `value: true`".into()),
        }
        if let Some(f) = g.get("field") {
            let path = match f {
                Json::Array(steps) => steps.iter().map(|s| scalar("field", s)).collect::<Result<Vec<_>, _>>()?.join("."),
                other => scalar("field", other)?,
            };
            args.extend(["--field".into(), path]);
        }
        match (g.get("to"), g.get("by")) {
            (Some(v), None) => args.extend(["--to".into(), scalar("to", v)?]),
            (None, Some(Json::Number(d))) => args.extend(["--by".into(), d.to_string()]),
            (None, Some(v)) => return Err(format!("`by` takes a number, not {v}")),
            (None, None) => return Err("a goal needs `to` or `by`".into()),
            (Some(_), Some(_)) => return Err("a goal takes `to` or `by`, not both".into()),
        }
        Ok(())
    };
    let top_goal = ["arg", "native_arg", "value", "field", "to", "by"].iter().any(|k| obj.contains_key(*k));
    if top_goal {
        push_goal(obj, false)?;
    }
    match obj.get("goals") {
        None | Some(Json::Null) => {}
        Some(Json::Array(goals)) => {
            for g in goals {
                push_goal(g.as_object().ok_or("each of `goals` must be an object")?, true)?;
            }
        }
        Some(v) => return Err(format!("`goals` takes a list of goal objects, not {v}")),
    }
    for (key, flag) in [("allow", "--allow"), ("deny", "--deny"), ("pins", "--deny")] {
        match obj.get(key) {
            None | Some(Json::Null) => {}
            Some(Json::Array(xs)) => {
                for x in xs {
                    args.extend([flag.to_string(), scalar(key, x)?]);
                }
            }
            Some(v) => args.extend([flag.to_string(), scalar(key, v)?]),
        }
    }
    for (key, flag) in [
        ("config_strict", "--config-strict"),
        ("explain", "--explain"),
        ("apply", "--apply"),
        ("allow_respell", "--allow-respell"),
    ] {
        if flag_on(obj, key)? {
            args.push(flag.into());
        }
    }
    for (key, flag) in [
        ("alternatives", "--alternatives"),
        ("pick", "--pick"),
        ("tol", "--tol"),
        ("max_runs", "--max-runs"),
        ("max_screen", "--max-screen"),
        ("seed", "--seed"),
    ] {
        if let Some(v) = obj.get(key).filter(|v| !v.is_null()) {
            args.extend([flag.to_string(), scalar(key, v)?]);
        }
    }
    let mut parser = V2Parser::new();
    let mut i = 0;
    while i < args.len() {
        if !parser.flag(&args, &mut i)? {
            return Err(format!("internal: `{}` is not a propose-edit flag", args[i]));
        }
        i += 1;
    }
    let mut opts = parser.finish()?;
    opts.json = true;
    Ok(opts)
}

/// The JSON error object the CLI prints for a failed `propose-edit --json`
/// (`{"error": true, "phase": …, "message": …, …}`).
pub fn error_json(e: &CliDmError) -> Json {
    super::error_json_value(&e.message, e.phase)
}

/// [`propose`] for a JSON request ([`opts_from_json`]), answering in the
/// CLI's `--json` shape either way: the report, or [`error_json`] (phase
/// `"request"` when the request itself does not parse). This is the
/// embedding entry point: a host that already has `env` (its natives
/// registered, the program loaded and run to `checkpoint`) passes its
/// [`SpecHost`] and gets the same document `petal propose-edit --json`
/// prints. Natives whose effects are not contained are not called by the
/// speculative runs (see [`crate::dm_v2::spec`]).
pub fn propose_json(
    env: &mut Env,
    host: &mut dyn SpecHost,
    pid: ProgramId,
    checkpoint: StackKey,
    request: &Json,
    entry_path: Option<PathBuf>,
) -> Json {
    let opts = match opts_from_json(request) {
        Ok(o) => o,
        Err(e) => return error_json(&fail("request", e)),
    };
    match propose(env, host, pid, checkpoint, &opts, entry_path) {
        Ok(report) => report,
        Err(e) => error_json(&e),
    }
}

/// `petal propose-edit --v2` with the host's natives: load `source` (from
/// `path` when given, so imports and `--apply` resolve against it) in a
/// fresh `Env` that `register` has first filled with the host's natives and
/// packages, and run [`propose`] on a fresh stack. What a host binary with
/// no frame loop of its own needs; one that runs frames before it drags
/// (petal-ui's `petal-ui-propose-edit`) calls [`propose`] on its own
/// checkpoint instead.
pub fn propose_with_natives(
    source: &str,
    path: Option<&std::path::Path>,
    include_dirs: &[PathBuf],
    opts: &V2Opts,
    host: &mut dyn SpecHost,
    register: &mut dyn FnMut(&mut Env),
) -> Result<Json, CliDmError> {
    let mut env = Env::new();
    env.set_echo(false);
    register(&mut env);
    for dir in include_dirs {
        env.add_module_path(dir.clone());
    }
    env.set_seed(effective_seed(opts.seed));
    let pid = match path {
        Some(p) => env.load_program_at(source, p),
        None => env.load_program(source),
    }
    .map_err(|e| fail("compile", e))?;
    let sid = env.create_stack(pid).map_err(|e| fail("runtime", e))?;
    propose(&mut env, host, pid, sid, opts, path.map(|p| p.to_path_buf()))
}

fn resolve_goals(
    env: &mut Env,
    host: &mut dyn SpecHost,
    pid: ProgramId,
    checkpoint: StackKey,
    opts: &V2Opts,
    cfg: &SpecConfig,
) -> Result<Vec<Resolved>, CliDmError> {
    let channels: Vec<_> = opts.goals.iter().map(|g| env.intern_symbol(&g.channel)).collect();
    let session =
        SpecSession::begin(env, host, checkpoint, &[], cfg.clone(), None).map_err(|e| fail("runtime", e.to_string()))?;
    let emits = session.baseline().emits.clone();
    let run_error = session.baseline().error.clone();
    session.end(env);
    let program = env.get_program(pid).ok_or_else(|| fail("goal", "the program is gone"))?;

    let mut out = Vec::new();
    for (g, &sym) in opts.goals.iter().zip(&channels) {
        let on_channel: Vec<_> = emits.iter().filter(|e| e.channel == sym).collect();
        if on_channel.is_empty() {
            let mut known: Vec<String> = Vec::new();
            for e in &emits {
                if let Some(n) = env.symbol_name(e.channel)
                    && !known.iter().any(|k| k == n)
                {
                    known.push(n.to_string());
                }
            }
            let mut msg = format!(
                "channel '{}' recorded no emits; channels with emits: {}",
                g.channel,
                if known.is_empty() { "none".to_string() } else { known.join(", ") }
            );
            if let Some(e) = &run_error {
                msg.push_str(&format!(" (the run failed: {e:?})"));
            }
            return Err(fail("goal", msg));
        }
        let Some(e) = on_channel.get(g.emit) else {
            return Err(fail(
                "goal",
                format!("channel '{}' has {} emit(s), no index {}", g.channel, on_channel.len(), g.emit),
            ));
        };
        let frame = crate::provenance::pick_frame(program, &e.chain.chain, ENTRY_FILE);
        let site = frame.and_then(|t| crate::provenance::CallSite::resolve(program, t));
        let line = site.as_ref().and_then(|s| s.span).map(|s| s.start.line);
        let callee = site.as_ref().and_then(|s| s.callee.clone());
        let (observable, watch) = match g.addr {
            GoalAddr::NativeArg(k) => {
                if k >= e.native_args.len() {
                    return Err(fail(
                        "goal",
                        format!("emit {} of '{}' was made by a native with {} argument(s), no index {k}", g.emit, g.channel, e.native_args.len()),
                    ));
                }
                (Observable::EmitArg { key: e.key, arg: k, path: g.path.clone() }, None)
            }
            GoalAddr::Value => (Observable::EmitValue { key: e.key, path: g.path.clone() }, None),
            GoalAddr::Arg(k) => {
                let Some(site) = &site else {
                    return Err(fail("goal", "the emit carries no attributable call chain"));
                };
                let Some(a) = site.args.get(k) else {
                    return Err(fail(
                        "goal",
                        format!("the call `{}` has {} argument(s), no index {k}", site.callee.as_deref().unwrap_or("?"), site.args.len()),
                    ));
                };
                (Observable::Watch { term: a.term, at: e.key, path: g.path.clone() }, Some(a.term))
            }
        };
        out.push(Resolved { observable, key: e.key, watch, callee, line });
    }
    Ok(out)
}

/// Write the picked alternative to disk; returns the `applied` report.
fn apply(program: &Program, response: &DmResponse, pick: Option<usize>, entry_path: Option<PathBuf>) -> Result<Json, CliDmError> {
    let alts = &response.alternatives;
    if alts.is_empty() {
        return Err(fail("apply", "there is no alternative to apply"));
    }
    let rank = match pick {
        Some(n) => n,
        None if alts.len() == 1 => 1,
        None => {
            return Err(fail(
                "apply",
                format!("{} alternatives; choose one with --pick N (or narrow with --allow/--deny)", alts.len()),
            ));
        }
    };
    let Some(alt) = alts.get(rank - 1) else {
        return Err(fail("apply", format!("--pick {rank}: there are only {} alternatives", alts.len())));
    };
    if !alt.verified {
        return Err(fail("apply", format!("alternative {rank} is not verified; refusing to write it")));
    }
    let mut files = program_files(program);
    apply_edits_multi(&mut files, &alt.edits).map_err(|e| fail("apply", e.to_string()))?;
    let mut written: Vec<String> = Vec::new();
    let mut touched: Vec<_> = alt.edits.iter().map(|e| (e.file, e.path.clone())).collect();
    touched.sort_by_key(|(f, _)| f.0);
    touched.dedup_by_key(|(f, _)| f.0);
    // Check every path first, so a refusal writes nothing.
    let mut plan = Vec::new();
    for (file, path) in touched {
        let path = if file == ENTRY_FILE { entry_path.clone().or(path) } else { path };
        let Some(path) = path else {
            return Err(fail("apply", "--apply needs a file path (not inline code or stdin)"));
        };
        let text = files.iter().find(|(f, _)| *f == file).map(|(_, t)| t.clone()).unwrap_or_default();
        plan.push((path, text));
    }
    for (path, text) in plan {
        std::fs::write(&path, text).map_err(|e| fail("apply", format!("writing '{}': {e}", path.display())))?;
        written.push(path.display().to_string());
    }
    Ok(json!({ "pick": rank, "files": written }))
}

/// Parse a `--to` value as a config file would: int, float, `true`/`false`/
/// `nil`, else a string.
pub fn parse_goal_value(to: &str) -> StaticValue {
    if let Ok(n) = to.parse::<i64>() {
        return StaticValue::Int(n);
    }
    if let Ok(f) = to.parse::<f64>() {
        return StaticValue::Float(f);
    }
    match to {
        "true" => StaticValue::Bool(true),
        "false" => StaticValue::Bool(false),
        "nil" => StaticValue::Nil,
        s => StaticValue::Str(s.to_string()),
    }
}

// ── the report ─────────────────────────────────────────────────────────────

fn num(f: f64) -> Json {
    if f.is_finite() { json!(f) } else { Json::Null }
}

fn span(s: &SourceSpan) -> Json {
    json!({
        "file": s.file.0,
        "start": { "line": s.start.line, "column": s.start.column, "offset": s.start.offset },
        "end": { "line": s.end.line, "column": s.end.column, "offset": s.end.offset },
    })
}

/// An [`EmitKey`] as JSON. The hashes are 64-bit, beyond a JSON number's
/// exact range, so they are hex strings.
pub fn key_json(k: &EmitKey) -> Json {
    json!({
        "chain_hash": format!("{:016x}", k.chain_hash),
        "path_hash": format!("{:016x}", k.path_hash),
        "occurrence": k.occurrence,
    })
}

/// An [`ObsValue`] as JSON: strings and enum tags are recorded by hash, so
/// they appear as `{"str": "<hash>"}` / `{"tag": "<hash>", "data": [...]}`;
/// an opaque value is `{"opaque": true}`.
pub fn obs_json(v: &ObsValue) -> Json {
    match v {
        ObsValue::Num(f) => num(*f),
        ObsValue::Int(n) => json!(n),
        ObsValue::Bool(b) => json!(b),
        ObsValue::Str(h) => json!({ "str": format!("{h:016x}") }),
        ObsValue::Nil => Json::Null,
        ObsValue::List(xs) => Json::Array(xs.iter().map(obs_json).collect()),
        ObsValue::Record(fs) => Json::Object(fs.iter().map(|(k, v)| (k.clone(), obs_json(v))).collect()),
        ObsValue::Variant { tag, data } => {
            json!({ "tag": format!("{tag:016x}"), "data": data.iter().map(obs_json).collect::<Vec<_>>() })
        }
        ObsValue::Opaque => json!({ "opaque": true }),
    }
}

/// The emits of a speculative baseline of `checkpoint`, per channel name, in
/// emit order: each one's [`EmitKey`] and the emitting native's raw
/// arguments. `run --trace-emits` merges these into its report (§4.2); the
/// stack is not changed. `None` when the speculative run cannot start.
pub fn baseline_emits(env: &mut Env, host: &mut dyn SpecHost, checkpoint: StackKey, seed: u64) -> Option<Vec<(String, Json)>> {
    let cfg = SpecConfig { session_seed: seed, ..SpecConfig::default() };
    let session = SpecSession::begin(env, host, checkpoint, &[], cfg, None).ok()?;
    let emits = session.baseline().emits.clone();
    session.end(env);
    Some(
        emits
            .iter()
            .map(|e| {
                let name = env.symbol_name(e.channel).unwrap_or("<unnamed>").to_string();
                let args: Vec<Json> = e.native_args.iter().map(obs_json).collect();
                (name, json!({ "key": key_json(&e.key), "native_args": args }))
            })
            .collect(),
    )
}

fn path_json(path: &[PathStep]) -> Json {
    Json::Array(
        path.iter()
            .map(|p| match p {
                PathStep::Index(i) => json!(i),
                PathStep::Field(f) => json!(f),
            })
            .collect(),
    )
}

fn engine_name(e: Engine) -> &'static str {
    match e {
        Engine::V2 => "v2",
        Engine::V1 => "v1",
    }
}

fn snake(debug: String) -> String {
    let mut out = String::new();
    for (i, c) in debug.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn quality_json(q: &ColumnQuality) -> Json {
    match q {
        ColumnQuality::Smooth => json!("smooth"),
        ColumnQuality::Kink { left, right } => json!({ "kind": "kink", "left": num(*left), "right": num(*right) }),
        ColumnQuality::BranchFlip { side } => json!({ "kind": "branch_flip", "side": snake(format!("{side:?}")) }),
        ColumnQuality::Step { secant_h } => json!({ "kind": "step", "secant_h": num(*secant_h) }),
        ColumnQuality::Discrete => json!("discrete"),
        ColumnQuality::Zero => json!("zero"),
        ColumnQuality::Errored(e) => json!({ "kind": "errored", "error": e }),
    }
}

fn file_path(program: &Program, file: crate::source_map::FileId) -> Json {
    program
        .source_map
        .files
        .get(file.0 as usize)
        .and_then(|f| f.origin.as_ref())
        .map(|p| json!(p.display().to_string()))
        .unwrap_or(Json::Null)
}

fn alternative_json(program: &Program, rank: usize, a: &Alternative, targets: &[f64]) -> Json {
    let changes: Vec<Json> = a
        .changes
        .iter()
        .map(|c| {
            json!({
                "term": c.term.0,
                "file": c.file.0,
                "path": file_path(program, c.file),
                "line": c.span.start.line,
                "span": span(&c.span),
                "binding": c.binding,
                "function": c.function,
                "config": c.config,
                "old": num(c.old),
                "new": num(c.new),
                "old_text": c.old_text,
                "new_text": c.new_text,
                "respelled": c.respelled,
            })
        })
        .collect();
    let edits: Vec<Json> = a
        .edits
        .iter()
        .map(|e| {
            json!({
                "file": e.file.0,
                "path": e.path.as_ref().map(|p| p.display().to_string()),
                "span": span(&e.edit.span),
                "new_text": e.edit.new_text,
            })
        })
        .collect();
    let collateral: Vec<Json> = a
        .collateral
        .items
        .iter()
        .map(|c| {
            json!({
                "channel": c.channel,
                "callee": c.callee,
                "line": c.line,
                "max_move": num(c.max_move),
                "key": key_json(&c.key),
                "emit": c.emit,
                "arg": c.arg,
                "path": path_json(&c.path),
                "moved": c.moved,
                "label": c.label(),
            })
        })
        .collect();
    let p = &a.score_parts;
    json!({
        "rank": rank,
        "engine": engine_name(a.engine),
        "verified": a.verified,
        "description": a.description,
        "kind": snake(format!("{:?}", a.kind)),
        "score": num(a.score),
        "score_parts": {
            "edit": num(p.edit), "collateral": num(p.collateral), "count": num(p.count),
            "respelled": num(p.respelled), "regime": num(p.regime), "rng": num(p.rng), "error": num(p.error),
        },
        // In units of each goal's tolerance (`max_j |g_j - t_j| / τ_j`);
        // `misses` is the same in the goals' own units.
        "max_error": num(a.max_error),
        "misses": a
            .achieved
            .iter()
            .zip(targets)
            .map(|(v, t)| num((v - t).abs()))
            .collect::<Vec<_>>(),
        "achieved": a.achieved.iter().map(|v| num(*v)).collect::<Vec<_>>(),
        "predicted": a.predicted.iter().map(|v| num(*v)).collect::<Vec<_>>(),
        "iterations": a.iterations,
        "config_only": a.config_only,
        "stateful": a.stateful,
        "regime_changed": a.regime_changed,
        "rng_order_changed": a.rng_order_changed,
        "rng_difference": a.rng_difference,
        "changes": changes,
        "edits": edits,
        "collateral": { "norm": num(a.collateral.norm), "items": collateral },
    })
}

fn candidate_json(program: &Program, c: &CandidateReport) -> Json {
    let p = &c.param;
    let function = p
        .function
        .and_then(|f| program.functions.get(f.0 as usize))
        .and_then(|d| d.name.as_deref())
        .map(|n| crate::program::base_fn_name(n).to_string());
    let hint = match p.hint {
        None => Json::Null,
        Some(Hint::Rigid) => json!({ "kind": "rigid" }),
        Some(Hint::Tunable { min, max, step }) => json!({
            "kind": "tunable",
            "min": min.map(num), "max": max.map(num), "step": step.map(num),
        }),
    };
    json!({
        "term": p.term.0,
        "file": p.file.0,
        "line": p.span.start.line,
        "span": span(&p.span),
        "value": num(p.value),
        "is_int": p.is_int,
        "binding": p.binding,
        "function": function,
        "config": p.config,
        "hint": hint,
        "origin": snake(format!("{:?}", p.origin)),
        "distance": p.distance,
        "focus": p.focus,
        "in_goal_frame": p.in_goal_frame,
        "flags": {
            "through_var": p.through_var,
            "through_state": p.through_state,
            "state_init": p.state_init,
            "through_rng": p.through_rng,
            "structural": p.structural,
            "hint_bound": p.hint_bound,
        },
        "loaded": c.loaded,
        "eligible": c.eligible.is_ok(),
        "ineligible_reason": c.eligible.err(),
        "quality": c.quality.as_ref().map(quality_json),
        "column": c.column.as_ref().map(|col| col.iter().map(|v| num(*v)).collect::<Vec<_>>()),
        "weight": num(c.weight),
        "clock_coupled": c.clock_coupled,
    })
}

#[allow(clippy::too_many_arguments)]
fn report(
    program: &Program,
    opts: &V2Opts,
    seed: u64,
    resolved: &[Resolved],
    baseline: &[f64],
    targets: &[f64],
    response: &DmResponse,
    applied: Option<Json>,
) -> Json {
    let goals: Vec<Json> = opts
        .goals
        .iter()
        .zip(resolved)
        .enumerate()
        .map(|(i, (g, r))| {
            let (kind, index) = match g.addr {
                GoalAddr::Arg(k) => ("arg", Some(k)),
                GoalAddr::NativeArg(k) => ("native_arg", Some(k)),
                GoalAddr::Value => ("value", None),
            };
            let (to, by) = match &g.value {
                GoalValue::To(t) => (Some(t.clone()), None),
                GoalValue::By(d) => (None, Some(num(*d))),
            };
            json!({
                "channel": g.channel,
                "emit": g.emit,
                "kind": kind,
                "index": index,
                "path": path_json(&g.path),
                "key": key_json(&r.key),
                "callee": r.callee,
                "line": r.line,
                "to": to,
                "by": by,
                "baseline": baseline.get(i).copied().map(num).unwrap_or(Json::Null),
                "target": targets.get(i).copied().map(num).unwrap_or(Json::Null),
                "tolerance": response.diagnostics.tolerances.get(i).copied().map(num).unwrap_or(Json::Null),
            })
        })
        .collect();
    let d = &response.diagnostics;
    json!({
        "engine": d.engine.map(engine_name).unwrap_or("v2"),
        "seed": seed,
        "goals": goals,
        "alternatives": response
            .alternatives
            .iter()
            .enumerate()
            .map(|(i, a)| alternative_json(program, i + 1, a, targets))
            .collect::<Vec<_>>(),
        "candidates": response.candidates.iter().map(|c| candidate_json(program, c)).collect::<Vec<_>>(),
        "diagnostics": {
            "runs": d.runs,
            "wall_ms": d.wall.map(|w| num(w.as_secs_f64() * 1000.0)),
            "suppressed": d.suppressed,
            "budget_exhausted": d.budget_exhausted,
            "warnings": d.warnings,
            "discovered": d.discovered,
            "eligible": d.eligible,
            "screened": d.screened,
            "measured": d.measured,
            "truncated": d.truncated,
            "subsets_predicted": d.subsets_predicted,
            "subsets_verified": d.subsets_verified,
        },
        "explain": response.explain.as_ref().map(|e| e.lines.clone()),
        "applied": applied,
    })
}

// ── text ───────────────────────────────────────────────────────────────────

fn fmt_num(v: &Json) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
        Some(f) => {
            let s = format!("{f:.4}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
        None => "?".into(),
    }
}

/// Print a [`propose`] report for people.
pub fn print_text(r: &Json) {
    let empty = Vec::new();
    let arr = |k: &str| r.get(k).and_then(|v| v.as_array()).unwrap_or(&empty).clone();
    let goals = arr("goals");
    let alts = arr("alternatives");
    let diag = &r["diagnostics"];
    println!(
        "engine {} · seed {} · {} run{}",
        r["engine"].as_str().unwrap_or("v2"),
        r["seed"],
        diag["runs"],
        if diag["runs"].as_u64() == Some(1) { "" } else { "s" }
    );
    for g in &goals {
        let what = match g["kind"].as_str() {
            Some("arg") => format!("arg {}", g["index"]),
            Some("native_arg") => format!("native-arg {}", g["index"]),
            _ => "value".into(),
        };
        let path: Vec<String> = g["path"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .map(|p| p.as_str().map(str::to_string).unwrap_or_else(|| p.to_string()))
            .collect();
        let path = if path.is_empty() { String::new() } else { format!(".{}", path.join(".")) };
        let at = match (g["callee"].as_str(), g["line"].as_u64()) {
            (Some(c), Some(l)) => format!(" ({c}, line {l})"),
            (Some(c), None) => format!(" ({c})"),
            _ => String::new(),
        };
        let target = if g["target"].is_null() {
            g["to"].as_str().unwrap_or("?").to_string()
        } else {
            fmt_num(&g["target"])
        };
        println!(
            "goal: {}[{}] {what}{path}{at}: {} -> {}",
            g["channel"].as_str().unwrap_or("?"),
            g["emit"],
            fmt_num(&g["baseline"]),
            target
        );
    }
    let suppressed = diag["suppressed"].as_array().unwrap_or(&empty);
    if !suppressed.is_empty() {
        let names: Vec<&str> = suppressed.iter().filter_map(|s| s.as_str()).collect();
        println!("suppressed in speculative runs: {}", names.join(", "));
    }
    for w in diag["warnings"].as_array().unwrap_or(&empty) {
        println!("warning: {}", w.as_str().unwrap_or(""));
    }
    if alts.is_empty() {
        println!("No alternative meets the goals.");
    } else {
        println!("{} alternative{}:", alts.len(), if alts.len() == 1 { "" } else { "s" });
    }
    for a in &alts {
        let mut tags = Vec::new();
        tags.push(if a["verified"].as_bool() == Some(true) { "verified" } else { "unverified" });
        if a["engine"].as_str() == Some("v1") {
            tags.push("v1");
        }
        if a["config_only"].as_bool() == Some(true) {
            tags.push("config");
        }
        if a["regime_changed"].as_bool() == Some(true) {
            tags.push("control flow changed");
        }
        if a["rng_order_changed"].as_bool() == Some(true) {
            tags.push("random draws reshuffled");
        }
        if a["stateful"].as_bool() == Some(true) {
            tags.push("feeds state");
        }
        println!("  {}. [{}] {}", a["rank"], tags.join(", "), a["description"].as_str().unwrap_or(""));
        for c in a["changes"].as_array().unwrap_or(&empty) {
            let name = c["binding"]
                .as_str()
                .map(|b| format!(" `{b}`"))
                .or_else(|| c["function"].as_str().map(|f| format!(" in fn {f}")))
                .unwrap_or_default();
            let file = c["path"]
                .as_str()
                .filter(|_| c["file"].as_u64() != Some(0))
                .map(|p| format!("{p}:"))
                .unwrap_or_else(|| "line ".into());
            println!(
                "       {file}{}{name}: {} -> {}",
                c["line"],
                c["old_text"].as_str().unwrap_or("?"),
                c["new_text"].as_str().unwrap_or("?")
            );
        }
        if a["changes"].as_array().is_none_or(|c| c.is_empty()) {
            for e in a["edits"].as_array().unwrap_or(&empty) {
                println!(
                    "       line {}: -> {}",
                    e["span"]["start"]["line"],
                    e["new_text"].as_str().unwrap_or("?")
                );
            }
        }
        let coll = &a["collateral"];
        let moved = coll["items"].as_array().map_or(0, |v| v.len());
        let achieved: Vec<String> = a["achieved"].as_array().unwrap_or(&empty).iter().map(fmt_num).collect();
        let worst = a["misses"].as_array().unwrap_or(&empty).iter().filter_map(|m| m.as_f64()).fold(0.0, f64::max);
        let mut facts = Vec::new();
        if !achieved.is_empty() {
            facts.push(format!("reaches {}", achieved.join(", ")));
            if a["max_error"].as_f64().is_some_and(|e| e > 1.0) {
                facts.push(format!("off by {}", fmt_num(&json!(worst))));
            }
        }
        if moved > 0 {
            let items = coll["items"].as_array().unwrap_or(&empty);
            let mut names: Vec<String> = items.iter().take(4).filter_map(|c| c["label"].as_str().map(str::to_string)).collect();
            if items.len() > 4 {
                names.push(format!("+{} more", items.len() - 4));
            }
            facts.push(format!(
                "moves {moved} other value{} (norm {}): {}",
                if moved == 1 { "" } else { "s" },
                fmt_num(&coll["norm"]),
                names.join(", ")
            ));
        }
        if !a["score"].is_null() && a["engine"].as_str() != Some("v1") {
            facts.push(format!("score {}", fmt_num(&a["score"])));
        }
        if !facts.is_empty() {
            println!("       {}", facts.join(" · "));
        }
    }
    if let Some(lines) = r["explain"].as_array() {
        println!();
        println!("explain:");
        for l in lines {
            println!("  {}", l.as_str().unwrap_or(""));
        }
        let cands = arr("candidates");
        if !cands.is_empty() {
            println!();
            println!("candidates ({}):", cands.len());
            for c in &cands {
                let name = c["binding"]
                    .as_str()
                    .map(|b| format!("`{b}`"))
                    .or_else(|| c["function"].as_str().map(|f| format!("fn {f}")))
                    .unwrap_or_else(|| "-".into());
                let status = if c["eligible"].as_bool() == Some(true) {
                    match &c["quality"] {
                        Json::Null => "eligible".to_string(),
                        Json::String(s) => s.clone(),
                        q => q["kind"].as_str().unwrap_or("?").to_string(),
                    }
                } else {
                    format!("excluded: {}", c["ineligible_reason"].as_str().unwrap_or("?"))
                };
                let column = c["column"]
                    .as_array()
                    .map(|col| format!(" d/dp [{}]", col.iter().map(fmt_num).collect::<Vec<_>>().join(", ")))
                    .unwrap_or_default();
                println!(
                    "  line {:>4} {:>10} {:<16} {status}{column} w {}",
                    c["line"],
                    fmt_num(&c["value"]),
                    name,
                    fmt_num(&c["weight"])
                );
            }
        }
    }
    match r.get("applied") {
        Some(a) if !a.is_null() => {
            let files: Vec<&str> = a["files"].as_array().unwrap_or(&empty).iter().filter_map(|f| f.as_str()).collect();
            println!("Applied alternative {} to {}.", a["pick"], files.join(", "));
        }
        _ if !alts.iter().any(|a| a["verified"].as_bool() == Some(true)) => {
            if alts.iter().any(|a| a["engine"].as_str() == Some("v1")) {
                println!("v1 answered (the goal is not a number); apply with --v1 --apply.");
            }
        }
        _ if alts.len() > 1 => {
            println!("Apply one with --apply --pick N; narrow with --allow/--deny; see why with --explain.")
        }
        _ => println!("Apply it with --apply."),
    }
}
