//! Opt-in runtime audit of the static-value classifier: what the code it
//! calls static was *observed* reading, held against the claim that it reads
//! nothing that changes between runs.
//!
//! [`crate::static_classify`] calls a top-level term static when its value is
//! a function of the source text alone. A later stage keeps such a value
//! across runs, so a wrong answer there is a stale value on screen — and the
//! answer rests on things only a run can check: that a native registered
//! [`CONST`](crate::native_fn::NativeEffects::CONST) really reads nothing,
//! and that the classifier resolved every callee. This audit is the oracle
//! for both, the way [`crate::effect_audit`] is the oracle for effect rows.
//!
//! With the audit on, the VM knows at every moment which instruction of the
//! top level is in progress (the root frame's, however deep the call it
//! made). While that instruction's term is one the classifier called static,
//! it watches the **dynamic inputs** and records a [`Finding`] for each one
//! touched:
//!
//! - the run's activity counters ([`crate::run_deps::Activity`]) around every
//!   native call: a binding read (`mouse_x`, `time`, the screen size, a
//!   font…), a host-data read, a resource-table read, an emit, an effect;
//! - the random stream, around every native call;
//! - every `state` instruction (declare, read, write), in any frame;
//! - every call of a native that is not constant
//!   ([`crate::native_fn::NativeConstness::Dynamic`]), whatever the counters
//!   say — `noise`, `font` and the like move no counter.
//!
//! A finding names the declaration whose initializer was running, the input,
//! and the call that touched it.
//!
//! # Two modes
//!
//! - [`StaticAuditMode::Watch`] runs the program as usual and audits the
//!   static terms as they go by. It needs the host the program was written
//!   for, and it is the mode a host's own frame loop uses.
//! - [`StaticAuditMode::StaticOnly`] runs *only* the top-level statements the
//!   classifier called static and skips every other one — a build of the
//!   static declarations and nothing else, which is what a later stage will
//!   do once and keep. Since static code may only call constant natives, it
//!   needs no host: `petal run --static-only game.ptl` builds the tables of a
//!   program whose host is not there. Anything that goes wrong is a finding:
//!   a touched input as above, or the run raising (a "static" statement that
//!   needed a value from a skipped one, or called a native nothing
//!   registered).
//!
//! # What it does not see
//!
//! A read of a `var` cell or of a value another statement mutates moves no
//! counter. Those are covered by comparing values between runs
//! (`core-libs/petal-ui/tests/static_classify.rs`), not here. A call the memo
//! replays is not made, so run the audit with memoization off to see every
//! call.
//!
//! Like the effect audit this is a runtime switch on the
//! [`Env`](crate::env::Env): off, the cost is one branch per native call and
//! per `state` instruction. Turn it on or off between runs, not inside one —
//! it re-lowers the program.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::backend::bytecode::isa::{BytecodeProgram, Inst, Label};
use crate::native_fn::{NativeConstness, NativeFnId, NativeFnTable};
use crate::program::{BlockId, Program, ProgramId, TermId};
use crate::run_deps::Activity;
use crate::static_classify::{DeclClass, StaticAnalysis, TermClass};
use crate::symbol::{SymbolId, SymbolTable};

/// How the audit runs a program. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StaticAuditMode {
    #[default]
    Off,
    /// Run everything; audit the static terms as they execute.
    Watch,
    /// Run only the static top-level statements, and audit all of it.
    StaticOnly,
}

/// A dynamic input, as the audit names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Input {
    /// A host→script binding was read (by name when known).
    Binding(String),
    /// Host data outside the binding table was read.
    HostData,
    /// The resource table was consulted.
    Resource,
    /// A value was pushed into an output buffer.
    Emit,
    /// Something no replay could reproduce: a print, a counter, a resource.
    Effect,
    /// The random stream was advanced.
    Random,
    /// `state` was declared, read or written.
    State,
    /// A native that is not registered constant was called.
    NonConstNative,
    /// The static-only build raised an error.
    Raised(String),
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Input::Binding(name) => write!(f, "read the binding `{name}`"),
            Input::HostData => f.write_str("read host data"),
            Input::Resource => f.write_str("read the resource table"),
            Input::Emit => f.write_str("emitted output"),
            Input::Effect => f.write_str("had an effect"),
            Input::Random => f.write_str("drew from the random stream"),
            Input::State => f.write_str("touched state"),
            Input::NonConstNative => f.write_str("called a native that is not constant"),
            Input::Raised(msg) => write!(f, "raised: {msg}"),
        }
    }
}

/// One thing the audit caught: a dynamic input touched while a static term
/// was being evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The static declaration whose initializer was running, by
    /// module-qualified name; `None` for a free-standing static statement.
    pub declaration: Option<String>,
    /// Where that declaration or statement is: `file:line`.
    pub at: Option<String>,
    pub input: Input,
    /// The native that touched it, when a native did.
    pub native: Option<String>,
    /// Where the touching call or instruction is: `file:line`.
    pub site: Option<String>,
    /// How many times it was seen.
    pub count: u64,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.declaration, &self.at) {
            (Some(name), Some(at)) => write!(f, "{name} ({at})")?,
            (Some(name), None) => write!(f, "{name}")?,
            (None, Some(at)) => write!(f, "statement at {at}")?,
            (None, None) => f.write_str("a top-level statement")?,
        }
        write!(f, " {}", self.input)?;
        match (&self.native, &self.site) {
            (Some(n), Some(s)) => write!(f, ": {n} at {s}")?,
            (Some(n), None) => write!(f, ": {n}")?,
            (None, Some(s)) => write!(f, " at {s}")?,
            (None, None) => {}
        }
        if self.count > 1 {
            write!(f, " ({} times)", self.count)?;
        }
        Ok(())
    }
}

/// A finished audit, printable.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub mode: StaticAuditMode,
    /// Static data declarations of the audited programs.
    pub static_declarations: usize,
    /// Top-level statements a static-only build ran, and skipped.
    pub statements_run: usize,
    pub statements_skipped: usize,
    /// Native calls made while a static term was being evaluated.
    pub native_calls: u64,
    pub findings: Vec<Finding>,
}

impl Report {
    /// Whether any static term touched a dynamic input: the condition
    /// `--static-audit` fails on.
    pub fn has_failures(&self) -> bool {
        !self.findings.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.findings.len();
        let failures = if n == 1 { "1 failure".to_string() } else { format!("{n} failures") };
        match self.mode {
            StaticAuditMode::StaticOnly => writeln!(
                f,
                "static audit: built {} static declarations alone ({} top-level statements run, {} skipped), {} native calls under them, {}",
                self.static_declarations,
                self.statements_run,
                self.statements_skipped,
                self.native_calls,
                failures,
            )?,
            _ => writeln!(
                f,
                "static audit: {} static declarations, {} native calls under static terms, {}",
                self.static_declarations, self.native_calls, failures,
            )?,
        }
        for finding in &self.findings {
            writeln!(f, "  {finding}")?;
        }
        Ok(())
    }
}

/// What the VM needs to know about one program's root code.
#[derive(Debug, Clone, Default)]
struct Table {
    /// Per root instruction: is the term it was lowered from one the
    /// classifier called static (so a dynamic input touched under it is a
    /// finding)?
    audited: Vec<bool>,
    /// Per root instruction: the root-block statement it belongs to.
    stmt: Vec<Option<TermId>>,
    /// Root-block statement -> index into `owners`: the declaration whose
    /// initializer it is part of.
    owner_of: HashMap<TermId, usize>,
    /// (qualified name, `file:line`).
    owners: Vec<(String, Option<String>)>,
    static_declarations: usize,
    statements_run: usize,
    statements_skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RawKey {
    program: ProgramId,
    stmt: Option<TermId>,
    site: Option<TermId>,
    native: Option<NativeFnId>,
    input: RawInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RawInput {
    Binding(Option<SymbolId>),
    HostData,
    Resource,
    Emit,
    Effect,
    Random,
    State,
    NonConstNative,
    Raised(String),
}

/// The audit's accumulator. Lives on the [`Env`](crate::env::Env) and sums
/// over every run on it until the mode is set again.
#[derive(Debug, Clone, Default)]
pub struct StaticAudit {
    /// Master switch, read by the VM. Set exactly when the mode is not `Off`.
    pub enabled: bool,
    mode: StaticAuditMode,
    tables: HashMap<ProgramId, Table>,
    /// The program whose code the VM is running.
    active: Option<ProgramId>,
    /// Findings in first-seen order, with a count each.
    findings: Vec<(RawKey, u64)>,
    index: HashMap<RawKey, usize>,
    native_calls: u64,
    /// Print each new finding on stderr as the run that made it ends
    /// (`PETAL_STATIC_AUDIT`); how many have been printed so far.
    announce: bool,
    announced: usize,
    /// In [`StaticAuditMode::StaticOnly`], let the static-value cache keep
    /// what the build computes (see [`Env::set_static_only_keeps`]).
    ///
    /// [`Env::set_static_only_keeps`]: crate::env::Env::set_static_only_keeps
    pub(crate) keeps: bool,
    /// Check the values the cache keeps for function bodies instead of
    /// trusting them (see [`Env::set_static_check`]). Independent of the
    /// mode: the cache stays on.
    ///
    /// [`Env::set_static_check`]: crate::env::Env::set_static_check
    pub(crate) check_kept: bool,
    /// The check was asked for by `PETAL_STATIC_CHECK`: print each mismatch
    /// on stderr as it is found, since no test reads them.
    pub(crate) announce_check: bool,
}

impl StaticAudit {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> StaticAuditMode {
        self.mode
    }

    /// Set the mode and forget everything observed and every table built.
    /// The caller drops its lowered bytecode, which the tables index.
    pub(crate) fn set_mode(&mut self, mode: StaticAuditMode) {
        *self = StaticAudit {
            enabled: mode != StaticAuditMode::Off,
            mode,
            announce: self.announce,
            keeps: self.keeps,
            check_kept: self.check_kept,
            announce_check: self.announce_check,
            ..Default::default()
        };
    }

    /// Whether this audit is the static-only build with its values kept:
    /// the one case where the static-value cache stays on under it.
    pub(crate) fn keeps_static_values(&self) -> bool {
        self.keeps && self.mode == StaticAuditMode::StaticOnly
    }

    /// The audit the process environment asks for: `PETAL_STATIC_AUDIT=watch`
    /// (or `1`) starts every `Env` in [`StaticAuditMode::Watch`] and prints
    /// each finding on stderr, prefixed `static audit failure:`, when the run
    /// that made it ends. It is how a whole test suite is run under the
    /// audit without touching a test: run it with the variable set and
    /// search the output.
    pub(crate) fn from_env() -> Self {
        let mut audit = StaticAudit::default();
        // `PETAL_STATIC_CHECK=1`: every `Env` checks the values kept for
        // function bodies (`Env::set_static_check`).
        audit.check_kept = matches!(std::env::var("PETAL_STATIC_CHECK").as_deref(), Ok("1"));
        audit.announce_check = audit.check_kept;
        if matches!(std::env::var("PETAL_STATIC_AUDIT").as_deref(), Ok("watch" | "1")) {
            audit.announce = true;
            audit.set_mode(StaticAuditMode::Watch);
        }
        audit
    }

    /// Whether findings are printed as runs end, and how many are new.
    pub(crate) fn unannounced(&mut self) -> Option<usize> {
        if !self.announce || self.announced == self.findings.len() {
            return None;
        }
        let from = self.announced;
        self.announced = self.findings.len();
        Some(from)
    }

    /// Tell the audit which program the VM is about to run code of.
    #[inline]
    pub(crate) fn enter(&mut self, pid: ProgramId) {
        self.active = Some(pid);
    }

    /// Forget the table of a program whose lowering was dropped.
    pub(crate) fn forget(&mut self, pid: ProgramId) {
        self.tables.remove(&pid);
    }

    /// Whether the root instruction at `ip` is being audited.
    #[inline]
    pub(crate) fn watches(&self, ip: usize) -> bool {
        self.active
            .and_then(|pid| self.tables.get(&pid))
            .and_then(|t| t.audited.get(ip).copied())
            .unwrap_or(false)
    }

    fn note(&mut self, ip: usize, site: Option<TermId>, native: Option<NativeFnId>, input: RawInput) {
        let Some(program) = self.active else { return };
        let stmt = self
            .tables
            .get(&program)
            .and_then(|t| t.stmt.get(ip).copied())
            .flatten();
        let key = RawKey {
            program,
            stmt,
            site,
            native,
            input,
        };
        match self.index.get(&key) {
            Some(&i) => self.findings[i].1 += 1,
            None => {
                self.index.insert(key.clone(), self.findings.len());
                self.findings.push((key, 1));
            }
        }
    }

    /// A native call finished while the audited root instruction `ip` was in
    /// progress. `before`/`after` bracket the call; `bindings` is what it
    /// read; `rng_moved` whether the random stream advanced.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn native_call(
        &mut self,
        ip: usize,
        site: Option<TermId>,
        nid: NativeFnId,
        natives: &NativeFnTable,
        before: Activity,
        after: Activity,
        bindings: &[SymbolId],
        rng_moved: bool,
    ) {
        self.native_calls += 1;
        let n = Some(nid);
        if after.binding_reads != before.binding_reads {
            if bindings.is_empty() {
                self.note(ip, site, n, RawInput::Binding(None));
            }
            let mut seen: Vec<SymbolId> = Vec::new();
            for sym in bindings {
                if !seen.contains(sym) {
                    seen.push(*sym);
                    self.note(ip, site, n, RawInput::Binding(Some(*sym)));
                }
            }
        }
        if after.host_reads != before.host_reads {
            self.note(ip, site, n, RawInput::HostData);
        }
        if after.resource_reads != before.resource_reads {
            self.note(ip, site, n, RawInput::Resource);
        }
        if after.emits != before.emits {
            self.note(ip, site, n, RawInput::Emit);
        }
        if after.effects != before.effects {
            self.note(ip, site, n, RawInput::Effect);
        }
        if rng_moved {
            self.note(ip, site, n, RawInput::Random);
        }
        // The classifier answers these two from the program's own enum
        // declarations, not from their rows.
        if natives.constness(nid) == NativeConstness::Dynamic
            && !matches!(natives.get_name(nid), "__enum_from" | "__enum_all")
        {
            self.note(ip, site, n, RawInput::NonConstNative);
        }
    }

    /// A `state` instruction ran while the audited root instruction `ip` was
    /// in progress.
    pub(crate) fn state_access(&mut self, ip: usize, site: Option<TermId>) {
        self.note(ip, site, None, RawInput::State);
    }

    /// A static-only build ended in an error while root instruction `ip` was
    /// in progress.
    pub(crate) fn raised(&mut self, ip: Option<usize>, message: &str) {
        if self.mode != StaticAuditMode::StaticOnly {
            return;
        }
        let first_line = message.lines().next().unwrap_or(message).to_string();
        self.note(ip.unwrap_or(usize::MAX), None, None, RawInput::Raised(first_line));
    }

    /// Build the table for `pid` from its lowering, and — in
    /// [`StaticAuditMode::StaticOnly`] — rewrite the root code so that every
    /// top-level statement that is not static is jumped over.
    pub(crate) fn prepare(
        &mut self,
        pid: ProgramId,
        program: &Program,
        bc: &mut BytecodeProgram,
        analysis: &StaticAnalysis,
    ) -> Result<(), String> {
        let table = build_table(program, bc, analysis, self.mode)?;
        self.tables.insert(pid, table);
        Ok(())
    }

    /// What was observed, with names and positions resolved. `program` looks
    /// up the program a finding was recorded against.
    pub fn report<'p>(
        &self,
        program: impl Fn(ProgramId) -> Option<&'p Program>,
        natives: &NativeFnTable,
        symbols: &SymbolTable,
    ) -> Report {
        self.report_from(0, program, natives, symbols)
    }

    /// [`report`](Self::report) of the findings first seen after the first
    /// `skip`.
    pub(crate) fn report_from<'p>(
        &self,
        skip: usize,
        program: impl Fn(ProgramId) -> Option<&'p Program>,
        natives: &NativeFnTable,
        symbols: &SymbolTable,
    ) -> Report {
        let mut report = Report {
            mode: self.mode,
            native_calls: self.native_calls,
            ..Default::default()
        };
        for table in self.tables.values() {
            report.static_declarations += table.static_declarations;
            report.statements_run += table.statements_run;
            report.statements_skipped += table.statements_skipped;
        }
        for (key, count) in self.findings.iter().skip(skip) {
            let p = program(key.program);
            let table = self.tables.get(&key.program);
            let owner = key
                .stmt
                .and_then(|s| table.and_then(|t| t.owner_of.get(&s).map(|&i| &t.owners[i])));
            let at = match owner {
                Some((_, at)) => at.clone(),
                None => key.stmt.and_then(|s| p.and_then(|p| position(p, s))),
            };
            let input = match &key.input {
                RawInput::Binding(Some(sym)) => {
                    Input::Binding(symbols.name(*sym).unwrap_or("?").to_string())
                }
                RawInput::Binding(None) => Input::Binding("?".to_string()),
                RawInput::HostData => Input::HostData,
                RawInput::Resource => Input::Resource,
                RawInput::Emit => Input::Emit,
                RawInput::Effect => Input::Effect,
                RawInput::Random => Input::Random,
                RawInput::State => Input::State,
                RawInput::NonConstNative => Input::NonConstNative,
                RawInput::Raised(m) => Input::Raised(m.clone()),
            };
            let finding = Finding {
                declaration: owner.map(|(name, _)| name.clone()),
                at,
                input,
                native: key.native.map(|n| natives.get_name(n).to_string()),
                site: key.site.and_then(|s| p.and_then(|p| position(p, s))),
                count: *count,
            };
            // One declaration's initializer is several root statements (its
            // operands are statements of their own): one line for all.
            let same = report.findings.iter_mut().find(|f| {
                f.declaration == finding.declaration
                    && f.at == finding.at
                    && f.input == finding.input
                    && f.native == finding.native
                    && f.site == finding.site
            });
            match same {
                Some(f) => f.count += finding.count,
                None => report.findings.push(finding),
            }
        }
        report
    }
}

/// `file:line` of a term, when the compiler recorded one.
pub(crate) fn position(program: &Program, term: TermId) -> Option<String> {
    let span = program.source_map.get(term).filter(|s| s.start.line > 0)?;
    Some(match program.source_map.file_name_for_span(span) {
        Some(file) => format!("{file}:{}", span.start.line),
        None => format!("line {}", span.start.line),
    })
}

/// For every block, the control term it hangs off: a child block's parent,
/// or the match an arm's guard or body block belongs to.
pub(crate) fn block_parents(program: &Program) -> Vec<Option<TermId>> {
    let mut parent: Vec<Option<TermId>> = program.blocks.iter().map(|b| b.parent_term_id).collect();
    let mut set = |b: BlockId, t: TermId| {
        if let Some(slot) = parent.get_mut(b.0 as usize) {
            slot.get_or_insert(t);
        }
    };
    for term in &program.terms {
        for &b in &term.child_blocks {
            set(b, term.id);
        }
    }
    for (term, arms) in &program.match_arms {
        for arm in arms {
            if let Some(g) = arm.guard_block {
                set(g, *term);
            }
            set(arm.body_block, *term);
        }
    }
    parent
}

/// The root-block term `term` sits in (itself, when it is in the root
/// block); `None` for a term inside a function body.
pub(crate) fn root_statement(
    program: &Program,
    parents: &[Option<TermId>],
    term: TermId,
) -> Option<TermId> {
    let mut t = term;
    for _ in 0..10_000 {
        let b = program.terms.get(t.0 as usize)?.block_id;
        if b == program.root_block {
            return Some(t);
        }
        t = parents.get(b.0 as usize).copied().flatten()?;
    }
    None
}

fn build_table(
    program: &Program,
    bc: &mut BytecodeProgram,
    analysis: &StaticAnalysis,
    mode: StaticAuditMode,
) -> Result<Table, String> {
    let parents = block_parents(program);
    let n = bc.root.code.len();
    let is_static = |t: TermId| analysis.term_class(t) != TermClass::Dynamic;

    // The statement each root instruction belongs to. An instruction with no
    // origin of its own (a loop's bookkeeping, a phi's move) belongs with the
    // one before it.
    // A `StaticSkip` (the static-only build with its values kept) has no
    // origin and stands in front of the run it guards: it belongs with the
    // run's first statement, not with whatever precedes it.
    let own_of = |ip: usize| {
        bc.root
            .origins
            .get(ip)
            .copied()
            .flatten()
            .and_then(|t| root_statement(program, &parents, t))
    };
    let mut stmt: Vec<Option<TermId>> = Vec::with_capacity(n);
    for ip in 0..n {
        let own = match bc.root.code[ip] {
            Inst::StaticSkip { .. } => (ip + 1..n).find_map(own_of),
            _ => own_of(ip),
        };
        let prev = stmt.last().copied().flatten();
        stmt.push(own.or(prev));
    }
    // Leading instructions with no origin go with the first statement.
    if let Some(first) = stmt.iter().copied().flatten().next() {
        for s in stmt.iter_mut().take_while(|s| s.is_none()) {
            *s = Some(first);
        }
    }

    let mut table = Table {
        audited: vec![false; n],
        stmt,
        ..Default::default()
    };

    // Which declaration each root statement is part of: the declaration's
    // own term, and the unnamed root-block temporaries that feed it.
    let decl_terms: HashSet<TermId> = analysis.report.declarations.iter().map(|d| d.term).collect();
    for d in &analysis.report.declarations {
        if d.class == DeclClass::Static {
            table.static_declarations += 1;
        }
        let at = match (&d.file, d.line) {
            (Some(f), Some(l)) => Some(format!("{f}:{l}")),
            (None, Some(l)) => Some(format!("line {l}")),
            _ => None,
        };
        let idx = table.owners.len();
        table.owners.push((d.qualified.clone(), at));
        let mut work = vec![d.term];
        while let Some(t) = work.pop() {
            let Some(term) = program.terms.get(t.0 as usize) else { continue };
            if term.block_id != program.root_block || table.owner_of.contains_key(&t) {
                continue;
            }
            if t != d.term && (term.name.is_some() || decl_terms.contains(&t)) {
                continue;
            }
            table.owner_of.insert(t, idx);
            work.extend(term.inputs.iter().copied());
        }
    }

    match mode {
        StaticAuditMode::Off => {}
        StaticAuditMode::Watch => {
            for ip in 0..n {
                table.audited[ip] = bc
                    .root
                    .origins
                    .get(ip)
                    .copied()
                    .flatten()
                    .is_some_and(&is_static);
            }
        }
        StaticAuditMode::StaticOnly => slice_to_static(program, bc, &is_static, &mut table)?,
    }
    Ok(table)
}

/// Rewrite the root code so only the static top-level statements run.
///
/// The root code is cut into segments, each a maximal run of instructions
/// lowered from one top-level statement. A statement is usually one segment;
/// it is several when the lowering put another statement's instruction in
/// the middle of it (a constant materialized where a loop body first uses
/// it). The first instruction of every segment of a statement that is not
/// static becomes a jump to the segment's end. Fails, changing nothing, if a
/// kept statement branches into the middle of another statement's segment,
/// which that scheme could not skip around.
fn slice_to_static(
    program: &Program,
    bc: &mut BytecodeProgram,
    is_static: &dyn Fn(TermId) -> bool,
    table: &mut Table,
) -> Result<(), String> {
    let n = bc.root.code.len();
    let keep_stmt = |s: Option<TermId>| s.is_some_and(is_static);

    let mut starts = vec![false; n + 1];
    starts[n] = true;
    for (ip, start) in starts.iter_mut().enumerate().take(n) {
        *start = ip == 0 || table.stmt[ip - 1] != table.stmt[ip];
    }
    for ip in 0..n {
        if !keep_stmt(table.stmt[ip]) {
            continue;
        }
        // (`branch_target_mut` is the public accessor; nothing is changed.)
        if let Some(to) = bc.root.code[ip].branch_target_mut().map(|l| *l) {
            let to = to as usize;
            if to > n || !(starts[to] || table.stmt.get(to) == Some(&table.stmt[ip])) {
                return Err(format!(
                    "static audit: the top-level statement at {} branches into another statement",
                    table.stmt[ip]
                        .and_then(|t| position(program, t))
                        .unwrap_or_else(|| "an unknown position".into())
                ));
            }
        }
    }

    let mut kept: HashSet<Option<TermId>> = HashSet::new();
    let mut skipped: HashSet<Option<TermId>> = HashSet::new();
    let mut ip = 0;
    while ip < n {
        let mut end = ip + 1;
        while !starts[end] {
            end += 1;
        }
        if keep_stmt(table.stmt[ip]) {
            kept.insert(table.stmt[ip]);
            for a in &mut table.audited[ip..end] {
                *a = true;
            }
        } else {
            skipped.insert(table.stmt[ip]);
            bc.root.code[ip] = Inst::Jump { to: end as Label };
        }
        ip = end;
    }
    table.statements_run = kept.len();
    table.statements_skipped = skipped.len();
    // The root's result is whatever its last statement left, which may now
    // be a statement that never ran.
    bc.root.result_reg = None;
    Ok(())
}
