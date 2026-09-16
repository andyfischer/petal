//! How a run executes, as one named value.
//!
//! Three independent switches decide what a frame of Petal does, and every
//! one of them is also a differential oracle — the output must not change when
//! it is flipped:
//!
//! - the optimizer passes the bytecode is lowered with ([`OptFlags`]);
//! - whether user-function calls are memoized and replayed ([`crate::memo`]);
//! - whether a host skips a frame the frame gate says would reproduce the last
//!   one ([`Env::run_needed`](crate::env::Env::run_needed)).
//!
//! [`RunPolicy`] carries all three, and the combinations anyone actually asks
//! for have names:
//!
//! | Name       | Optimizer | Memo | Gate | For |
//! |------------|-----------|------|------|-----|
//! | `fast`     | on        | on   | on   | the default — what a shipped host runs |
//! | `baseline` | off       | off  | off  | the correctness oracle every other policy must reproduce |
//! | `explain`  | on, preserving every traced instruction | off | off | tools that read per-instruction results: every frame runs every instruction |
//! | `replay`   | on        | on   | off  | every frame runs, and memoized scopes replay — the memo under test without the gate |
//!
//! A name may be followed by modifiers that switch one thing on or off:
//! `fast-memo` is the gate alone, `baseline+gate` is the gate over unoptimized
//! code, `replay-opt` is the memo over unoptimized code. So a differential is
//! one word per side (`petal-ui-run --policy baseline` against
//! `--policy fast-memo`), not a set of `--no-*` flags plus a comment saying
//! which combination they spell.
//!
//! The gate is only a *policy* here: the runtime answers `run_needed` whatever
//! it says, and a host frame driver (petal-ui's `FrameCore`, the SDL game
//! loop) reads [`RunPolicy::gate`] to decide whether to act on the answer.

use crate::backend::OptFlags;

/// Which optimizations, memoization and frame gating a run uses. See the
/// module docs for the named policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunPolicy {
    /// The optimizer passes bytecode is lowered with.
    pub opts: OptFlags,
    /// Memoize user-function calls: a call whose arguments, captures and
    /// recorded reads are what they were last time is replayed from its
    /// record instead of run (`crate::memo`). A runtime switch; a call whose
    /// result the caller mutates in place is never memoized
    /// (`Inst::Call::no_memo`), since a record would share that result.
    pub memo: bool,
    /// Let a host frame driver skip a frame the frame gate says would
    /// reproduce the last one.
    pub gate: bool,
}

/// The names [`RunPolicy::parse`] accepts, in the order `name()` prefers them.
const NAMED: [(&str, RunPolicy); 4] = [
    ("fast", RunPolicy::FAST),
    ("baseline", RunPolicy::BASELINE),
    ("explain", RunPolicy::EXPLAIN),
    ("replay", RunPolicy::REPLAY),
];

impl RunPolicy {
    /// Everything on: what a shipped host runs, and the default.
    pub const FAST: RunPolicy = RunPolicy { opts: OptFlags::default_on(), memo: true, gate: true };

    /// Everything off: every frame runs, every call runs, and the bytecode is
    /// the clone-and-alloc lowering. The oracle the others must reproduce.
    pub const BASELINE: RunPolicy = RunPolicy { opts: OptFlags::none(), memo: false, gate: false };

    /// For tools that read what every instruction computed (`explain`,
    /// provenance, direct manipulation, observation): the optimizer stays on
    /// but keeps every instruction the trace would record, and nothing is
    /// skipped or replayed.
    pub const EXPLAIN: RunPolicy = RunPolicy {
        opts: OptFlags { preserve_observations: true, preserve_trace: true, ..OptFlags::default_on() },
        memo: false,
        gate: false,
    };

    /// Every frame runs, and memoized scopes replay: the memo exercised on
    /// every frame rather than only on the frames the gate lets through.
    pub const REPLAY: RunPolicy = RunPolicy { gate: false, ..RunPolicy::FAST };

    /// This policy with the optimizer set to `opts`.
    pub const fn with_opts(self, opts: OptFlags) -> RunPolicy {
        RunPolicy { opts, ..self }
    }

    /// This policy with memoization on or off.
    pub const fn with_memo(self, memo: bool) -> RunPolicy {
        RunPolicy { memo, ..self }
    }

    /// This policy with frame gating on or off.
    pub const fn with_gate(self, gate: bool) -> RunPolicy {
        RunPolicy { gate, ..self }
    }

    /// Parse a policy name with optional modifiers: `fast`, `baseline`,
    /// `explain`, `replay`, each optionally followed by any number of
    /// `+gate` / `-gate` / `+memo` / `-memo` / `+opt` / `-opt`.
    pub fn parse(spec: &str) -> Result<RunPolicy, String> {
        let spec = spec.trim();
        let split = spec.find(['+', '-']).unwrap_or(spec.len());
        let (name, mut rest) = spec.split_at(split);
        let mut policy = NAMED
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| *p)
            .ok_or_else(|| format!("unknown run policy '{name}' (expected {})", Self::names()))?;
        while let Some(sign) = rest.chars().next() {
            let on = sign == '+';
            let tail = &rest[1..];
            let end = tail.find(['+', '-']).unwrap_or(tail.len());
            match &tail[..end] {
                "gate" => policy.gate = on,
                "memo" => policy.memo = on,
                "opt" => {
                    policy.opts = if on { OptFlags::default_on() } else { OptFlags::none() }
                        .preserving(policy.opts)
                }
                other => {
                    return Err(format!(
                        "unknown run policy modifier '{sign}{other}' in '{spec}' (expected gate, memo or opt)"
                    ));
                }
            }
            rest = &tail[end..];
        }
        Ok(policy)
    }

    /// The policy's name, in the syntax [`parse`](Self::parse) accepts: a
    /// named policy when it is one, else the nearest name plus modifiers.
    /// `None` for an optimizer setting no name spells (a single pass isolated
    /// in a test, say).
    pub fn name(&self) -> Option<String> {
        if let Some((n, _)) = NAMED.iter().find(|(_, p)| p == self) {
            return Some((*n).to_string());
        }
        let (base, from) = NAMED
            .iter()
            .filter(|(_, p)| p.opts == self.opts)
            .min_by_key(|(_, p)| (p.memo != self.memo) as u8 + (p.gate != self.gate) as u8)?;
        let mut name = (*base).to_string();
        for (what, want, have) in [("memo", self.memo, from.memo), ("gate", self.gate, from.gate)] {
            if want != have {
                name.push(if want { '+' } else { '-' });
                name.push_str(what);
            }
        }
        Some(name)
    }

    /// The named policies, for usage and error text.
    pub fn names() -> String {
        NAMED.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
    }

    /// The policy the process environment asks for: `PETAL_POLICY=<policy>`,
    /// or the older `PETAL_OPT` (`off`/`none`/`0` is `baseline`; anything else
    /// is `fast`). An unparseable `PETAL_POLICY` is reported on stderr once
    /// per call and ignored, rather than failing every `Env::new` in the
    /// process.
    pub fn from_env() -> RunPolicy {
        if let Ok(spec) = std::env::var("PETAL_POLICY") {
            match RunPolicy::parse(&spec) {
                Ok(p) => return p,
                Err(e) => eprintln!("PETAL_POLICY: {e}; using fast"),
            }
        }
        match std::env::var("PETAL_OPT").ok().as_deref() {
            Some("none") | Some("0") | Some("off") => RunPolicy::BASELINE,
            _ => RunPolicy::FAST,
        }
    }
}

impl Default for RunPolicy {
    fn default() -> RunPolicy {
        RunPolicy::FAST
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_parses_to_itself_and_prints_back() {
        for (n, p) in NAMED {
            assert_eq!(RunPolicy::parse(n), Ok(p));
            assert_eq!(p.name().as_deref(), Some(n));
        }
    }

    #[test]
    fn modifiers_switch_one_thing() {
        let gate_only = RunPolicy::parse("fast-memo").unwrap();
        assert_eq!(gate_only, RunPolicy::FAST.with_memo(false));
        assert_eq!(RunPolicy::parse("fast-gate").unwrap(), RunPolicy::REPLAY);
        assert_eq!(
            RunPolicy::parse("baseline+gate+memo").unwrap(),
            RunPolicy::BASELINE.with_gate(true).with_memo(true)
        );
        assert_eq!(RunPolicy::parse("fast-opt").unwrap().opts, OptFlags::none());
        assert_eq!(RunPolicy::parse("baseline+opt").unwrap().opts, OptFlags::default_on());
    }

    #[test]
    fn opt_modifiers_keep_the_explain_preservation() {
        let p = RunPolicy::parse("explain-opt").unwrap();
        assert!(!p.opts.copy_propagation);
        assert!(p.opts.preserve_trace && p.opts.preserve_observations);
    }

    #[test]
    fn a_modified_policy_names_itself_by_its_nearest_name() {
        assert_eq!(RunPolicy::FAST.with_memo(false).name().as_deref(), Some("fast-memo"));
        assert_eq!(RunPolicy::BASELINE.with_gate(true).name().as_deref(), Some("baseline+gate"));
        let route_a = OptFlags { in_place_straight_line: true, ..OptFlags::none() };
        assert_eq!(RunPolicy::BASELINE.with_opts(route_a).name(), None);
    }

    #[test]
    fn bad_specs_say_what_was_expected() {
        assert!(RunPolicy::parse("quick").unwrap_err().contains("fast, baseline"));
        assert!(RunPolicy::parse("fast+speed").unwrap_err().contains("gate, memo or opt"));
    }
}
