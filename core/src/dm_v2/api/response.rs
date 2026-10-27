//! The response: ranked alternatives, each a set of source edits, and the
//! diagnostics of how they were found (§6.3).

use std::path::PathBuf;
use std::time::Duration;

use crate::direct_manipulation::SourceEdit;
use crate::dm_v2::jacobian::ColumnQuality;
use crate::dm_v2::observe::{EmitKey, PathStep, path_text};
use crate::dm_v2::params::{BudgetLimit, Param};
use crate::dm_v2::rank::{ScoreParts, SubsetKind};
use crate::program::TermId;
use crate::source_map::{FileId, SourceSpan};

use super::report::is_value_arg;

/// One text replacement and the file it is in (R14).
#[derive(Debug, Clone, PartialEq)]
pub struct FileEdit {
    pub file: FileId,
    /// Where that file came from, when it is on disk.
    pub path: Option<PathBuf>,
    pub edit: SourceEdit,
}

/// One literal changed by an alternative.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamChange {
    pub term: TermId,
    pub span: SourceSpan,
    pub file: FileId,
    /// The binding it defines, if any.
    pub binding: Option<String>,
    /// The function it is written in, if any.
    pub function: Option<String>,
    /// A knob ([`Param::is_knob`]): `config let`, `tunable(...)`, or both.
    pub config: bool,
    /// The value as written before and after.
    pub old: f64,
    pub new: f64,
    pub old_text: String,
    pub new_text: String,
    /// An int literal written as a float now (§2.4.4).
    pub respelled: bool,
}

/// One other emit an alternative moved.
#[derive(Debug, Clone, PartialEq)]
pub struct CollateralItem {
    pub key: EmitKey,
    pub channel: String,
    /// The native that made the emit.
    pub callee: Option<String>,
    /// The line of the emit's call in user code (1-based).
    pub line: Option<u32>,
    /// The largest movement of any of its numbers (`inf`: it is gone).
    pub max_move: f64,
    /// Which emit on its channel this is (0-based, baseline order): the
    /// `--emit` that addresses it.
    pub emit: usize,
    /// The number that moved most: the raw native argument and the path
    /// into it (`lives`' `2.x` is item 2's `x` of a list of records).
    pub arg: usize,
    pub path: Vec<PathStep>,
    /// How many of its numbers moved.
    pub moved: usize,
    /// Its channel's collateral weight ([`DmRequest::collateral_weights`](super::DmRequest::collateral_weights)).
    pub weight: f64,
}

impl CollateralItem {
    /// Where in the emit the largest move is, as a `--field` path
    /// (`2.x`), with `arg N:` in front unless it is `push_output`'s value.
    pub fn where_text(&self) -> String {
        let path = path_text(&self.path);
        let value_arg = is_value_arg(self.callee.as_deref(), self.arg);
        match (value_arg, path.is_empty()) {
            (true, true) => String::new(),
            (true, false) => path,
            (false, true) => format!("arg {}", self.arg),
            (false, false) => format!("arg {}: {path}", self.arg),
        }
    }

    /// `lives[0].2.x`: the channel, which emit on it, and [`Self::where_text`],
    /// then `+N` when N more of its numbers moved too.
    pub fn label(&self) -> String {
        let w = self.where_text();
        let head = format!("{}[{}]", self.channel, self.emit);
        let mut s = if w.is_empty() {
            head
        } else if w.starts_with("arg ") {
            format!("{head} {w}")
        } else {
            format!("{head}.{w}")
        };
        if self.moved > 1 {
            s.push_str(&format!(" +{}", self.moved - 1));
        }
        s
    }
}

/// What else an alternative moved, measured by its verification run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Collateral {
    /// `‖Δ other rows‖ / ‖Δ goals‖`.
    pub norm: f64,
    /// How many other emits moved, each counted by how far it moved
    /// relative to the goals, at most 1
    /// ([`weighted_collateral`](crate::dm_v2::rank::weighted_collateral)): what the
    /// score charges. Under channel weights, each emit counts `min(w, 1)`
    /// times its share.
    pub shapes: f64,
    /// What channel weights above 1 add to the score, uncompressed
    /// ([`crate::dm_v2::rank::WeightedCollateral::excess`]); 0 without them.
    pub excess: f64,
    pub items: Vec<CollateralItem>,
}

/// Which engine produced an alternative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    V2,
    /// The v1 fallback for non-numeric goals; not re-run.
    V1,
}

/// One way to meet the goals (§6.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Alternative {
    pub edits: Vec<FileEdit>,
    pub changes: Vec<ParamChange>,
    /// Each goal's value after the edit (from the verification run).
    pub achieved: Vec<f64>,
    /// What the linear model predicted for each goal before any run.
    pub predicted: Vec<f64>,
    /// Each goal's distance from what it asks for, in its own units
    /// ([`GoalRow::miss`](crate::dm_v2::solve::GoalRow::miss): a range's is how far outside it).
    pub misses: Vec<f64>,
    /// `max_j |achieved - target| / τ_j`.
    pub max_error: f64,
    pub verified: bool,
    pub collateral: Collateral,
    /// Some change feeds back through `state` from earlier frames: the
    /// frozen-state measurement covers one frame only.
    pub stateful: bool,
    /// Every change is a knob (`config let` or `tunable`, one tier).
    pub config_only: bool,
    /// The edit took another branch or re-initialized state somewhere.
    pub regime_changed: bool,
    /// Under sequential random draws the edited program draws differently.
    pub rng_order_changed: bool,
    /// The first difference the sequential-draw run showed.
    pub rng_difference: Option<String>,
    pub score: f64,
    pub score_parts: ScoreParts,
    pub kind: SubsetKind,
    /// Verification steps the solve took.
    pub iterations: usize,
    pub engine: Engine,
    pub description: String,
    /// Why runs this alternative's solve tried were rejected: each raised an
    /// error (an `assert` failing is the usual one, and the way a program
    /// states a constraint between literals). Grouped by error, in order.
    pub rejections: Vec<Rejection>,
}

/// Speculative runs rejected because they raised the same error.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    /// The error's message, without its position (`assertion failed`).
    pub message: String,
    /// The file it was raised in, and its display name.
    pub file: Option<FileId>,
    pub file_name: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// For a failing `assert(...)`: the condition as written (`x1 >= x0`).
    pub condition: Option<String>,
    /// How many runs raised it.
    pub runs: usize,
    /// The literals the first of those runs had moved, and their values.
    pub tried: Vec<TriedLiteral>,
}

/// A literal a rejected run had moved.
#[derive(Debug, Clone, PartialEq)]
pub struct TriedLiteral {
    pub term: TermId,
    pub line: u32,
    pub old_text: String,
    pub value: f64,
}

impl Rejection {
    /// `assertion failed at beetle.ptl:3 (x1 >= x0)`, and an assert's
    /// message after it: `…(x1 >= x0): a patrol ends after it starts`.
    pub fn summary(&self) -> String {
        let (head, note) = match self.message.split_once(": ") {
            Some((h, n)) if h == "assertion failed" => (h, Some(n)),
            _ => (self.message.as_str(), None),
        };
        let mut s = head.to_string();
        if let Some(line) = self.line {
            match &self.file_name {
                Some(f) => s.push_str(&format!(" at {f}:{line}")),
                None => s.push_str(&format!(" at line {line}")),
            }
        }
        if let Some(c) = &self.condition {
            s.push_str(&format!(" ({c})"));
        }
        if let Some(n) = note {
            s.push_str(&format!(": {n}"));
        }
        s
    }
}

/// One discovered literal, and what became of it (§2.7).
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateReport {
    pub param: Param,
    /// May it be edited; `Err` says why not.
    pub eligible: Result<(), &'static str>,
    /// Its goal-row column, when measured.
    pub column: Option<Vec<f64>>,
    pub quality: Option<ColumnQuality>,
    /// Its cost weight `d_i`.
    pub weight: f64,
    pub clock_coupled: bool,
    /// The baseline evaluated it.
    pub loaded: bool,
}

/// What the request cost and anything worth telling the client.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DmDiagnostics {
    /// Speculative runs made (baselines not counted).
    pub runs: usize,
    pub wall: Option<Duration>,
    /// Natives a speculative run did not call.
    pub suppressed: Vec<String>,
    pub budget_exhausted: bool,
    /// Which limit ended the session when [`budget_exhausted`](Self::budget_exhausted):
    /// the run count or the wall clock (`max_wall`). `None` when it finished
    /// its work within both.
    pub stopped_by: Option<BudgetLimit>,
    pub warnings: Vec<String>,
    /// Literals found by discovery, eligible to screen, screened, measured.
    pub discovered: usize,
    pub eligible: usize,
    pub screened: usize,
    pub measured: usize,
    pub truncated: bool,
    /// Subsets solved linearly, and sent to the verification loop.
    pub subsets_predicted: usize,
    pub subsets_verified: usize,
    /// Speculative runs of the solve, rounding and verification that raised
    /// an error (each rejected the values it tried), and why, grouped.
    pub rejected_runs: usize,
    pub rejections: Vec<Rejection>,
    /// The goals' baseline values and tolerances.
    pub baseline: Vec<f64>,
    pub tolerances: Vec<f64>,
    /// Per goal: it already holds at the baseline (a range the baseline is
    /// inside, or a point target within tolerance). Filled by `finish`.
    pub holds_at_baseline: Vec<bool>,
    pub engine: Option<Engine>,
}

/// The `explain` trace: what each stage did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExplainLog {
    pub lines: Vec<String>,
}

/// The answer (§6.3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DmResponse {
    /// Ranked, best first.
    pub alternatives: Vec<Alternative>,
    pub candidates: Vec<CandidateReport>,
    pub diagnostics: DmDiagnostics,
    pub explain: Option<ExplainLog>,
}

impl DmResponse {
    /// The best alternative, if it verified.
    pub fn best_verified(&self) -> Option<&Alternative> {
        self.alternatives.first().filter(|a| a.verified)
    }
}
