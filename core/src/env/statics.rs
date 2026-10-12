//! The `Env` side of the static-value cache (see [`crate::static_cache`]):
//! switching it, clearing it, and reading what a stack holds.

use super::*;

use crate::static_cache::{StaticSlotInfo, StaticStats};

/// What one slot of a stack's static-value table holds right now.
#[derive(Debug, Clone, PartialEq)]
pub enum StaticSlotState {
    /// Nothing yet: the run that fills it has not completed since the table
    /// was last emptied.
    Empty,
    Filled(Value),
    /// The value could not be kept, because it holds this; the code that
    /// builds it executes on every run.
    Rejected(&'static str),
    /// Nothing wrong with this value, but the same run builds a rejected
    /// one, and a run is kept whole or not at all.
    NotKept,
}

/// A stack's static-value table, slot by slot. See
/// [`Env::static_values`].
#[derive(Debug, Clone, Default)]
pub struct StaticValues {
    /// Each slot: which declaration it is, and what it holds.
    pub slots: Vec<(StaticSlotInfo, StaticSlotState)>,
    /// How many runs of static instructions the lowering can skip.
    pub runs: usize,
    /// Counters since the stack was created.
    pub stats: StaticStats,
}

/// The words of the warning for a value that could not be kept: `TABLE
/// (app.ptl:12) holds a function, so it is rebuilt on every run`. A `const`
/// says so, since its author asked for a constant.
fn rejection_warning(info: &StaticSlotInfo, what: &str) -> String {
    let name = info.name.as_deref().unwrap_or("a top-level value");
    let at = info.at.as_ref().map(|a| format!(" ({a})")).unwrap_or_default();
    if info.is_const {
        format!(
            "const {name}{at} holds {what}, which cannot be kept between runs, so it is rebuilt on every run"
        )
    } else {
        format!("{name}{at} holds {what}, so it is rebuilt on every run")
    }
}

impl StaticValues {
    /// One line per value that could not be kept, in the words of a
    /// warning: `TABLE (app.ptl:12) holds a function, so it is rebuilt on
    /// every run`.
    pub fn warnings(&self) -> Vec<String> {
        self.slots
            .iter()
            .filter_map(|(info, state)| {
                let StaticSlotState::Rejected(what) = state else { return None };
                Some(rejection_warning(info, what))
            })
            .collect()
    }
}

impl Env {
    /// Lower `program` under `flags` as this `Env` would to run it,
    /// [`OptFlags::static_cache`] included: the static-value cache lowers
    /// against the classifier's answer for this `Env`'s natives, which the
    /// free function [`lower_with_flags`](crate::backend::bytecode::lower_with_flags)
    /// does not have. Builtin names are left unresolved.
    pub fn lower_with(&self, program: &Program, flags: OptFlags) -> Result<BytecodeProgram, String> {
        if flags.static_cache {
            let analysis = crate::static_classify::analyze_kept(program, &self.native_fns);
            crate::backend::bytecode::lower_with_statics(program, flags, &analysis, &self.native_fns)
        } else {
            crate::backend::bytecode::lower_with_flags(program, flags)
        }
    }

    /// The top-level declarations of `program` that the static-value cache
    /// would keep in a slot: lowered as a run under the default optimizer
    /// would lower it, against this `Env`'s natives, with `analysis` the
    /// classifier's answer for the same pair. A static declaration that is
    /// not here is rebuilt on every run (too small to be worth a check,
    /// written to, or part of a statement that has to run) or is read only
    /// by other static declarations and built with them. Empty when the
    /// program does not lower. Nothing is cached and nothing changes: this
    /// is for a tool (hover), and costs one lowering.
    pub fn kept_declarations(
        &self,
        program: &Program,
        analysis: &crate::static_classify::StaticAnalysis,
    ) -> std::collections::HashSet<crate::program::TermId> {
        let flags = OptFlags::default_on().with_static_cache(true);
        crate::backend::bytecode::lower_with_statics(program, flags, analysis, &self.native_fns)
            .map(|bc| bc.statics.slots.iter().filter_map(|s| s.term).collect())
            .unwrap_or_default()
    }

    /// Switch the static-value cache on or off for subsequent runs: the
    /// same as setting a policy with
    /// [`RunPolicy::with_static_cache`](crate::policy::RunPolicy::with_static_cache).
    /// Each program is re-lowered on its next run, and a stack's kept values
    /// do not survive that.
    pub fn set_static_cache(&mut self, on: bool) {
        self.policy = self.policy.with_static_cache(on);
    }

    /// Whether runs keep static values right now: the policy asks for it
    /// and nothing that reads per-instruction results (observations, the
    /// trace, the static audit) is on.
    pub fn static_cache_enabled(&self) -> bool {
        self.effective_opt_flags().static_cache
    }

    /// Instructions retired on `stack_id` since it was created: an exact
    /// measure of how much a frame ran, for comparing two policies.
    pub fn instructions_retired(&self, stack_id: StackKey) -> Option<u64> {
        self.stacks.get(&stack_id).map(|s| s.insts)
    }

    /// Drop every static value `stack_id` holds; its next run builds them
    /// again. A reload does this itself. For a host that changed something
    /// a native registered `CONST` answers from, which is the one thing the
    /// cache cannot see.
    pub fn clear_static_values(&mut self, stack_id: StackKey) {
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.statics.clear();
            stack.run_deps.force();
        }
    }

    /// The warnings for static values `stack_id` found it could not keep
    /// since the last call (a value holding a function, a `var` cell, a
    /// handle or a pending result): one line each, as
    /// [`StaticValues::warnings`] words them. Each is handed out once per
    /// load and once more after a reload, so a host can print what this
    /// returns after every run and say each thing once. The run itself is
    /// unaffected: the value is built on every run, as it was before there
    /// was a cache.
    pub fn take_static_warnings(&mut self, stack_id: StackKey) -> Vec<String> {
        let Some(stack) = self.stacks.get_mut(&stack_id) else { return Vec::new() };
        let Some((_, bc)) = self.bytecode.get(&stack.program_id) else { return Vec::new() };
        stack
            .statics
            .take_unreported()
            .iter()
            .filter_map(|(slot, what)| {
                bc.statics.slots.get(*slot as usize).map(|info| rejection_warning(info, what))
            })
            .collect()
    }

    /// What `stack_id` keeps between runs: one entry per slot of its
    /// program's current lowering, in code order. `None` for an unknown
    /// stack or a program that has not been lowered yet (it has not run).
    /// A `Filled` value is an id into the stack's heap, valid until the
    /// next run or collection.
    pub fn static_values(&self, stack_id: StackKey) -> Option<StaticValues> {
        let stack = self.stacks.get(&stack_id)?;
        let (_, bc) = self.bytecode.get(&stack.program_id)?;
        let layout = &bc.statics;
        let table = &stack.statics;
        // A table filled under another lowering is about to be emptied.
        let current = table.len() == layout.slots.len();
        let slots = layout
            .slots
            .iter()
            .enumerate()
            .map(|(i, info)| {
                let i = i as u32;
                let state = if !current {
                    StaticSlotState::Empty
                } else if let Some(v) = table.get(i) {
                    StaticSlotState::Filled(v)
                } else if let Some((_, what)) = table.rejected().iter().find(|(s, _)| *s == i) {
                    StaticSlotState::Rejected(what)
                } else if table.is_rejected(i) {
                    StaticSlotState::NotKept
                } else {
                    StaticSlotState::Empty
                };
                (info.clone(), state)
            })
            .collect();
        Some(StaticValues {
            slots,
            runs: layout.runs.len(),
            stats: table.stats,
        })
    }
}
