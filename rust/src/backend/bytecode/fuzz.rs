//! Differential fuzzer: generate random (but always-terminating) Petal
//! programs from a seeded grammar and require the graph engine and the
//! bytecode VM to agree exactly — same result value, same print output, and
//! same error text when a program faults at runtime.
//!
//! The hand-written differential tests in `tests.rs` cover the constructs we
//! thought of; this covers the compositions we didn't. The subtlest lowering
//! bugs (break/continue running enclosing phi carry-outs, match-arm binding
//! registers, loop-key state) live exactly in deep nesting combinations that
//! hand-written cases under-sample.
//!
//! Determinism: seeds are fixed, so a failure reproduces by seed. The default
//! iteration count keeps `cargo test` fast; scale up with
//! `PETAL_FUZZ_ITERS=20000 cargo test fuzz -- --nocapture` for a soak run.

use crate::backend::Backend;
use crate::env::Env;
use crate::value;

// ── Deterministic RNG (xorshift64*) ─────────────────────────────

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        // Avoid the all-zeros fixed point.
        Rng(seed.wrapping_mul(2685821657736338717).max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(2685821657736338717)
    }

    /// Uniform in `0..n` (n > 0).
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// True with probability `pct`/100.
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

// ── Program generator ───────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Int,
    List,
    Rec,
}

struct Var {
    name: String,
    kind: Kind,
    /// Loop counters / loop variables must not be rebound by generated
    /// statements or loop termination could break.
    frozen: bool,
}

struct Gen {
    rng: Rng,
    src: String,
    indent: usize,
    /// In-scope variables, innermost last; `scopes` records the stack depth at
    /// each block entry so leaving a block truncates to it.
    vars: Vec<Var>,
    scopes: Vec<usize>,
    /// Defined functions: (name, arity). Bodies only see their params, so
    /// calls never capture — generation order makes every call target defined.
    funcs: Vec<(String, usize)>,
    next_id: usize,
    loop_depth: usize,
    /// Global cap on emitted statements, so nesting can't run away.
    stmt_budget: i64,
}

impl Gen {
    fn new(seed: u64) -> Gen {
        Gen {
            rng: Rng::new(seed),
            src: String::new(),
            indent: 0,
            vars: Vec::new(),
            scopes: Vec::new(),
            funcs: Vec::new(),
            next_id: 0,
            loop_depth: 0,
            stmt_budget: 60,
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.src.push_str("  ");
        }
        self.src.push_str(s);
        self.src.push('\n');
    }

    fn enter_scope(&mut self) {
        self.scopes.push(self.vars.len());
    }

    fn exit_scope(&mut self) {
        let depth = self.scopes.pop().unwrap();
        self.vars.truncate(depth);
    }

    /// A random in-scope variable of `kind`, if any.
    fn pick(&mut self, kind: Kind) -> Option<String> {
        let matching: Vec<&Var> = self.vars.iter().filter(|v| v.kind == kind).collect();
        if matching.is_empty() {
            return None;
        }
        let i = self.rng.below(matching.len() as u64) as usize;
        Some(matching[i].name.clone())
    }

    /// A random rebindable (non-frozen) int variable, if any.
    fn pick_mutable_int(&mut self) -> Option<String> {
        let matching: Vec<&Var> = self
            .vars
            .iter()
            .filter(|v| v.kind == Kind::Int && !v.frozen)
            .collect();
        if matching.is_empty() {
            return None;
        }
        let i = self.rng.below(matching.len() as u64) as usize;
        Some(matching[i].name.clone())
    }

    // ── Expressions ─────────────────────────────────────────────

    /// An integer-valued expression of bounded depth.
    fn int_expr(&mut self, depth: u64) -> String {
        if depth == 0 || self.rng.chance(35) {
            return self.int_leaf();
        }
        match self.rng.below(7) {
            0 | 1 => {
                let a = self.int_expr(depth - 1);
                let b = self.int_expr(depth - 1);
                let op = ["+", "-", "*"][self.rng.below(3) as usize];
                format!("({a} {op} {b})")
            }
            2 => {
                // Modulo by a nonzero literal — safe, and the workhorse for
                // making match subjects and conditions vary.
                let a = self.int_expr(depth - 1);
                let m = self.rng.below(5) + 1;
                format!("({a} % {m})")
            }
            3 => {
                let list = self.pick(Kind::List);
                match list {
                    // Reads use indices 0..3: initial literals are 3 long and
                    // appends only grow, so this is always in bounds.
                    Some(xs) => format!("{xs}[{}]", self.rng.below(3)),
                    None => self.int_leaf(),
                }
            }
            4 => {
                let rec = self.pick(Kind::Rec);
                match rec {
                    Some(r) => format!("{r}.{}", ["a", "b"][self.rng.below(2) as usize]),
                    None => self.int_leaf(),
                }
            }
            5 => {
                let call = self.call_expr(depth);
                call.unwrap_or_else(|| self.int_leaf())
            }
            _ => {
                // Conditional expression (if/else both required for a value).
                let c = self.cond_expr(depth - 1);
                let a = self.int_expr(depth - 1);
                let b = self.int_expr(depth - 1);
                format!("(if {c} then {a} else {b} end)")
            }
        }
    }

    fn int_leaf(&mut self) -> String {
        if self.rng.chance(55) {
            if let Some(v) = self.pick(Kind::Int) {
                return v;
            }
        }
        format!("{}", self.rng.below(20) as i64 - 5)
    }

    /// A boolean-valued expression (comparison, optionally && / || chained —
    /// short-circuit coverage).
    fn cond_expr(&mut self, depth: u64) -> String {
        let a = self.int_expr(depth.min(1));
        let b = self.int_expr(depth.min(1));
        let cmp = ["<", ">", "<=", ">=", "==", "!="][self.rng.below(6) as usize];
        let base = format!("{a} {cmp} {b}");
        if depth > 0 && self.rng.chance(30) {
            let rest = self.cond_expr(depth - 1);
            let op = ["&&", "||"][self.rng.below(2) as usize];
            return format!("({base} {op} {rest})");
        }
        base
    }

    /// A call to an already-defined function, if one exists.
    fn call_expr(&mut self, depth: u64) -> Option<String> {
        if self.funcs.is_empty() {
            return None;
        }
        let i = self.rng.below(self.funcs.len() as u64) as usize;
        let (name, arity) = (self.funcs[i].0.clone(), self.funcs[i].1);
        let args: Vec<String> = (0..arity).map(|_| self.int_expr(depth - 1)).collect();
        Some(format!("{name}({})", args.join(", ")))
    }

    /// A match expression over an int subject: literal arms, an optional
    /// guarded binding arm, and a wildcard.
    fn match_expr(&mut self) -> String {
        let subject = self.int_expr(1);
        let mut s = format!("match ({subject}) % 4\n");
        let pad = "  ".repeat(self.indent + 1);
        let arms = self.rng.below(3) + 1;
        for lit in 0..arms {
            let body = self.int_expr(1);
            s.push_str(&format!("{pad}when {lit} -> {body}\n"));
        }
        if self.rng.chance(50) {
            let g = self.fresh("g");
            let body = self.int_expr(1);
            s.push_str(&format!("{pad}when {g} if {g} > 2 -> {body}\n"));
        }
        let body = self.int_expr(1);
        s.push_str(&format!("{pad}when _ -> {body}\n"));
        s.push_str(&format!("{}end", "  ".repeat(self.indent)));
        s
    }

    // ── Statements ──────────────────────────────────────────────

    fn block(&mut self, max_stmts: u64) {
        self.enter_scope();
        let n = self.rng.below(max_stmts) + 1;
        for _ in 0..n {
            if self.stmt_budget <= 0 {
                break;
            }
            self.stmt();
        }
        self.exit_scope();
    }

    fn stmt(&mut self) {
        self.stmt_budget -= 1;
        match self.rng.below(14) {
            0 | 1 => {
                let name = self.fresh("v");
                let e = self.int_expr(2);
                self.line(&format!("let {name} = {e}"));
                self.vars.push(Var { name, kind: Kind::Int, frozen: false });
            }
            2 => {
                if let Some(v) = self.pick_mutable_int() {
                    let e = self.int_expr(2);
                    self.line(&format!("{v} = {e}"));
                }
            }
            3 => {
                if let Some(v) = self.pick_mutable_int() {
                    let e = self.int_expr(1);
                    let op = ["+=", "-=", "*="][self.rng.below(3) as usize];
                    self.line(&format!("{v} {op} {e}"));
                }
            }
            4 => self.if_stmt(),
            5 => {
                if self.loop_depth < 2 {
                    self.for_stmt();
                }
            }
            6 => {
                if self.loop_depth < 2 {
                    self.while_stmt();
                }
            }
            7 => {
                let name = self.fresh("m");
                let e = self.match_expr();
                self.line(&format!("let {name} = {e}"));
                self.vars.push(Var { name, kind: Kind::Int, frozen: false });
            }
            8 => {
                let name = self.fresh("xs");
                let a = self.int_expr(1);
                let b = self.int_expr(1);
                let c = self.int_expr(1);
                self.line(&format!("let {name} = [{a}, {b}, {c}]"));
                self.vars.push(Var { name, kind: Kind::List, frozen: false });
            }
            9 => {
                if let Some(xs) = self.pick(Kind::List) {
                    let e = self.int_expr(1);
                    self.line(&format!("{xs} = append({xs}, {e})"));
                }
            }
            10 => {
                if let Some(xs) = self.pick(Kind::List) {
                    let i = self.rng.below(3);
                    let e = self.int_expr(1);
                    self.line(&format!("{xs}[{i}] = {e}"));
                }
            }
            11 => {
                let name = self.fresh("r");
                let a = self.int_expr(1);
                let b = self.int_expr(1);
                self.line(&format!("let {name} = {{ a: {a}, b: {b} }}"));
                self.vars.push(Var { name, kind: Kind::Rec, frozen: false });
            }
            12 => {
                if let Some(r) = self.pick(Kind::Rec) {
                    let f = ["a", "b"][self.rng.below(2) as usize];
                    let e = self.int_expr(1);
                    self.line(&format!("{r}.{f} = {e}"));
                }
            }
            _ => {
                if let Some(v) = self.pick(Kind::Int) {
                    self.line(&format!("print(\"t\", {v})"));
                }
            }
        }
    }

    fn if_stmt(&mut self) {
        let c = self.cond_expr(1);
        self.line(&format!("if {c} then"));
        self.indent += 1;
        self.block(3);
        // break/continue coverage: from inside an if within a loop, the
        // lowering must run every enclosing region's phi carry-outs — the
        // single subtlest point in the whole lowering (see the status doc).
        if self.loop_depth > 0 && self.rng.chance(30) {
            let kw = ["break", "continue"][self.rng.below(2) as usize];
            self.line(kw);
        }
        self.indent -= 1;
        if self.rng.chance(40) {
            self.line("else");
            self.indent += 1;
            self.block(2);
            self.indent -= 1;
        }
        self.line("end");
    }

    fn for_stmt(&mut self) {
        let iv = self.fresh("i");
        let lo = self.rng.below(3);
        let hi = lo + self.rng.below(5) + 1;
        self.line(&format!("for {iv} in range({lo}, {hi}) do"));
        self.indent += 1;
        self.loop_depth += 1;
        self.enter_scope();
        self.vars.push(Var { name: iv, kind: Kind::Int, frozen: true });
        self.block(3);
        self.exit_scope();
        self.loop_depth -= 1;
        self.indent -= 1;
        self.line("end");
    }

    fn while_stmt(&mut self) {
        // Structurally terminating: fresh counter, decremented as the body's
        // first statement, and frozen so no generated statement can rebind it.
        let w = self.fresh("w");
        let n = self.rng.below(6) + 1;
        self.line(&format!("let {w} = {n}"));
        self.line(&format!("while {w} > 0 do"));
        self.indent += 1;
        self.loop_depth += 1;
        self.enter_scope();
        self.line(&format!("{w} = {w} - 1"));
        self.vars.push(Var { name: w.clone(), kind: Kind::Int, frozen: true });
        self.block(3);
        self.exit_scope();
        self.loop_depth -= 1;
        self.indent -= 1;
        self.line("end");
        self.vars.push(Var { name: w, kind: Kind::Int, frozen: true });
    }

    fn function_def(&mut self) {
        let name = self.fresh("f");
        let arity = self.rng.below(3) as usize + 1;
        let params: Vec<String> = (0..arity).map(|i| format!("p{i}")).collect();
        self.line(&format!("fn {name}({})", params.join(", ")));
        self.indent += 1;
        // Body sees only its params; swap the outer scope away for the body.
        let outer_vars = std::mem::take(&mut self.vars);
        let outer_scopes = std::mem::take(&mut self.scopes);
        for p in &params {
            self.vars.push(Var { name: p.clone(), kind: Kind::Int, frozen: false });
        }
        self.block(4);
        let result = self.int_expr(2);
        self.line(&result);
        self.vars = outer_vars;
        self.scopes = outer_scopes;
        self.indent -= 1;
        self.line("end");
        self.funcs.push((name, arity));
    }

    /// Generate a whole program.
    fn program(mut self) -> String {
        let nfuncs = self.rng.below(3);
        for _ in 0..nfuncs {
            self.function_def();
        }
        let nstmts = self.rng.below(6) + 4;
        for _ in 0..nstmts {
            if self.stmt_budget <= 0 {
                break;
            }
            self.stmt();
        }
        // Force every live int through the output so silent value divergence
        // can't hide.
        let live: Vec<String> = self
            .vars
            .iter()
            .filter(|v| v.kind == Kind::Int)
            .map(|v| v.name.clone())
            .collect();
        if !live.is_empty() {
            self.line(&format!("print(\"end\", {})", live.join(", ")));
        }
        self.src
    }
}

// ── Differential runner ─────────────────────────────────────────

/// Run `code` on `backend`: rendered result value + print output, or the full
/// annotated error text.
fn run(code: &str, backend: Backend) -> Result<(String, Vec<String>), String> {
    let mut env = Env::new();
    env.set_backend(backend);
    let v = env.run_source(code)?;
    let rendered = value::value_to_display_string(&v, env.heap());
    Ok((rendered, env.take_output()))
}

/// Require exact agreement, including error text (the M3 parity bar).
fn assert_exact_parity(seed: u64, code: &str) {
    let graph = run(code, Backend::Graph);
    let bytecode = run(code, Backend::Bytecode);
    assert_eq!(
        graph, bytecode,
        "backend divergence at seed {seed}; reproduce with \
         Gen::new({seed}).program()\n--- program ---\n{code}"
    );
}

#[test]
fn differential_fuzz() {
    // Default covers seed 431, the first real bug this fuzzer found (dead
    // rebind behind an always-taken continue corrupted the phi carry-out).
    let iters: u64 = std::env::var("PETAL_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    for seed in 0..iters {
        let code = Gen::new(seed).program();
        assert_exact_parity(seed, &code);
    }
}
