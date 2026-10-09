//! Differential fuzzer for in-place writes through `var` cells
//! (`super::cells`).
//!
//! The general fuzzer (`super::fuzz`) keeps its functions closed over nothing
//! and its cells scalar, so it never reaches the case this pass exists for: a
//! container in a cell, written from a function that did not declare it. This
//! generator does little else. Its programs hold lists, grids and records in
//! module-level cells (`var` or `state var`) and in a function's local `var`s,
//! and write them through top-level helpers and nested closures, with the
//! aliasing that could expose a wrong in-place write mixed in on purpose:
//!
//! * snapshots (`let s = get xs`) taken before later writes and printed after;
//! * one cell's container stored into another's (`set g[0] = get xs`), and an
//!   inner container promoted to a cell's own (`set xs = get g[1]`);
//! * helpers whose last statement is a `set`, called both as statements (the
//!   result is dropped, the write stays in place) and for their value (the
//!   result is the container, which must then stop being written in place);
//! * helper calls inside the value being written (`set xs[i] = h(…)`), where
//!   the callee writes the same cell.
//!
//! Every program is run for several frames on one stack under the
//! clone-and-alloc baseline and under each configuration that can write in
//! place, and all of them must print the same thing and leave the same state.

use crate::backend::OptFlags;
use crate::env::Env;
use crate::policy::RunPolicy;

/// Small deterministic PRNG (xorshift64*), seeded per program.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 33) % n
    }

    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

/// What a cell holds. Every list keeps at least three elements and every
/// grid and record list at least two, so the generated indices stay in range
/// and a run that errors is the exception rather than the rule.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// `[int, int, int, …]`
    Ints,
    /// `[[int; 3]; 2]`
    Grid,
    /// `{a, b, x}`
    Rec,
    /// `[{a, b, x}, {a, b, x}, …]`
    Recs,
}

#[derive(Clone)]
struct Cell {
    name: String,
    kind: Kind,
    /// Declared in the scope being generated, so a bare read is legal there.
    local: bool,
}

#[derive(Clone)]
struct Callable {
    name: String,
    /// Its result is an int, so a call can stand in an int expression.
    int_result: bool,
}

struct Gen {
    rng: Rng,
    src: String,
    indent: usize,
    cells: Vec<Cell>,
    calls: Vec<Callable>,
    /// Int-valued names in scope (parameters, loop counters).
    ints: Vec<String>,
    /// Snapshots and call results bound in each open block, printed as the
    /// block closes — after whatever writes followed them.
    held: Vec<Vec<String>>,
    next_id: usize,
    budget: i64,
}

impl Gen {
    fn new(seed: u64) -> Gen {
        Gen {
            rng: Rng::new(seed),
            src: String::new(),
            indent: 0,
            cells: Vec::new(),
            calls: Vec::new(),
            ints: Vec::new(),
            held: Vec::new(),
            next_id: 0,
            budget: 70,
        }
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.src.push_str("  ");
        }
        self.src.push_str(s);
        self.src.push('\n');
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn cell(&mut self, kind: Kind) -> Option<Cell> {
        let of_kind: Vec<&Cell> = self.cells.iter().filter(|c| c.kind == kind).collect();
        if of_kind.is_empty() {
            return None;
        }
        Some(of_kind[self.rng.below(of_kind.len() as u64) as usize].clone())
    }

    /// A read of `cell`'s contents: `get c`, or the bare name where that is
    /// legal (the declaring scope), so both spellings are exercised.
    fn read(&mut self, cell: &Cell) -> String {
        if cell.local && self.rng.chance(50) {
            cell.name.clone()
        } else {
            format!("get {}", cell.name)
        }
    }

    /// An index into something of length `n`: a literal, or computed.
    fn index(&mut self, n: u64, depth: u64) -> String {
        if depth == 0 || self.rng.chance(70) {
            self.rng.below(n).to_string()
        } else {
            format!("({}) % {n}", self.int_expr(depth - 1))
        }
    }

    /// A non-negative int expression. Sums are reduced so repeated frames of
    /// a `state var` program cannot overflow.
    fn int_expr(&mut self, depth: u64) -> String {
        let pick = if depth == 0 { self.rng.below(3) } else { self.rng.below(11) };
        match pick {
            0 => self.rng.below(10).to_string(),
            1 => match self.ints.len() {
                0 => self.rng.below(10).to_string(),
                n => self.ints[self.rng.below(n as u64) as usize].clone(),
            },
            2 | 3 => match self.cell(Kind::Ints) {
                Some(c) => {
                    let (r, i) = (self.read(&c), self.index(3, depth.saturating_sub(1)));
                    format!("{r}[{i}]")
                }
                None => "1".into(),
            },
            4 => match self.cell(Kind::Grid) {
                Some(c) => {
                    let r = self.read(&c);
                    let (i, j) = (self.index(2, 0), self.index(3, 0));
                    format!("{r}[{i}][{j}]")
                }
                None => "2".into(),
            },
            5 => match self.cell(Kind::Rec) {
                Some(c) => {
                    let f = ["a", "b", "x"][self.rng.below(3) as usize];
                    format!("{}.{f}", self.read(&c))
                }
                None => "3".into(),
            },
            6 => match self.cell(Kind::Recs) {
                Some(c) => {
                    let (r, i) = (self.read(&c), self.index(2, 0));
                    format!("{r}[{i}].x")
                }
                None => "4".into(),
            },
            7 => match self.cell(Kind::Ints) {
                Some(c) => format!("len({}) % 5", self.read(&c)),
                None => "5".into(),
            },
            // A call in value position: the callee may write the very cell
            // the enclosing statement is about to write.
            8 => {
                let ints: Vec<Callable> =
                    self.calls.iter().filter(|c| c.int_result).cloned().collect();
                match ints.len() {
                    0 => "6".into(),
                    n => {
                        let f = ints[self.rng.below(n as u64) as usize].name.clone();
                        let (a, b) = (self.int_expr(0), self.int_expr(0));
                        format!("{f}({a}, {b})")
                    }
                }
            }
            // A call between reading a cell and indexing what was read: the
            // callee's writes must not show in the element.
            9 => {
                let ints: Vec<Callable> =
                    self.calls.iter().filter(|c| c.int_result).cloned().collect();
                match (self.cell(Kind::Ints), ints.len()) {
                    (Some(c), n) if n > 0 => {
                        let f = ints[self.rng.below(n as u64) as usize].name.clone();
                        let (r, a) = (self.read(&c), self.int_expr(0));
                        format!("{r}[{f}({a}, 1) % 3]")
                    }
                    _ => "7".into(),
                }
            }
            _ => {
                let (a, b) = (self.int_expr(depth - 1), self.int_expr(depth - 1));
                format!("({a} + {b}) % 97")
            }
        }
    }

    fn open(&mut self) {
        self.indent += 1;
        self.held.push(Vec::new());
    }

    /// Close a block, first printing what it bound — so a snapshot is read
    /// after every write that followed it in the block.
    fn close(&mut self) {
        let held = self.held.pop().unwrap_or_default();
        if !held.is_empty() {
            self.line(&format!("print(\"held\", {})", held.join(", ")));
        }
        self.indent -= 1;
    }

    fn hold(&mut self, name: String) {
        if let Some(top) = self.held.last_mut() {
            top.push(name);
        }
    }

    fn block(&mut self, max: u64) {
        let n = self.rng.below(max) + 1;
        for _ in 0..n {
            if self.budget <= 0 {
                break;
            }
            self.stmt();
        }
    }

    /// One statement that writes a cell — also what a helper may end on.
    fn write_stmt(&mut self) -> String {
        for _ in 0..8 {
            let s = match self.rng.below(13) {
                0 | 1 => self.cell(Kind::Ints).map(|c| {
                    let (i, e) = (self.index(3, 1), self.int_expr(2));
                    format!("set {}[{i}] = {e}", c.name)
                }),
                2 => self.cell(Kind::Ints).map(|c| {
                    let (i, e) = (self.index(3, 1), self.int_expr(1));
                    format!("set {}[{i}] += {e}", c.name)
                }),
                3 => self.cell(Kind::Ints).map(|c| {
                    let (r, e) = (self.read(&c), self.int_expr(1));
                    format!("set {} = append({r}, {e})", c.name)
                }),
                4 => self.cell(Kind::Grid).map(|c| {
                    let (i, j, e) = (self.index(2, 1), self.index(3, 1), self.int_expr(2));
                    format!("set {}[{i}][{j}] = {e}", c.name)
                }),
                5 => self.cell(Kind::Recs).map(|c| {
                    let (i, e) = (self.index(2, 1), self.int_expr(2));
                    let f = ["a", "x"][self.rng.below(2) as usize];
                    let op = ["=", "+="][self.rng.below(2) as usize];
                    format!("set {}[{i}].{f} {op} {e}", c.name)
                }),
                6 => self.cell(Kind::Rec).map(|c| {
                    let f = ["a", "b", "x"][self.rng.below(3) as usize];
                    let op = ["=", "+="][self.rng.below(2) as usize];
                    format!("set {}.{f} {op} {}", c.name, self.int_expr(2))
                }),
                // One cell's list becomes a row of another's grid.
                7 => match (self.cell(Kind::Grid), self.cell(Kind::Ints)) {
                    (Some(g), Some(xs)) => {
                        let (i, r) = (self.index(2, 0), self.read(&xs));
                        Some(format!("set {}[{i}] = {r}", g.name))
                    }
                    _ => None,
                },
                // A grid's row becomes another cell's own list.
                8 => match (self.cell(Kind::Ints), self.cell(Kind::Grid)) {
                    (Some(xs), Some(g)) => {
                        let (r, i) = (self.read(&g), self.index(2, 0));
                        Some(format!("set {} = {r}[{i}]", xs.name))
                    }
                    _ => None,
                },
                // Two cells share one list.
                9 => match (self.cell(Kind::Ints), self.cell(Kind::Ints)) {
                    (Some(a), Some(b)) => Some(format!("set {} = {}", a.name, self.read(&b))),
                    _ => None,
                },
                10 => match (self.cell(Kind::Recs), self.cell(Kind::Rec)) {
                    (Some(ps), Some(r)) => {
                        let (p, r) = (self.read(&ps), self.read(&r));
                        Some(format!("set {} = append({p}, {r})", ps.name))
                    }
                    _ => None,
                },
                11 => match (self.cell(Kind::Rec), self.cell(Kind::Recs)) {
                    (Some(r), Some(ps)) => {
                        let (p, i) = (self.read(&ps), self.index(2, 0));
                        Some(format!("set {} = {p}[{i}]", r.name))
                    }
                    _ => None,
                },
                _ => self.cell(Kind::Grid).map(|c| {
                    let (i, e) = (self.index(2, 0), self.int_expr(1));
                    format!("set {}[{i}] = [{e}, 0, 1]", c.name)
                }),
            };
            if let Some(s) = s {
                return s;
            }
        }
        "nil".into()
    }

    fn stmt(&mut self) {
        self.budget -= 1;
        match self.rng.below(23) {
            0..=8 => {
                let s = self.write_stmt();
                self.line(&s);
            }
            // A write in value position: a collecting loop keeps every
            // iteration's container, an `if` expression yields its arm's.
            20 => {
                let (name, k, s) = (self.fresh("t"), self.fresh("k"), self.write_stmt());
                self.line(&format!("let {name} = for {k} in range(0, 2) do"));
                self.indent += 1;
                self.line(&s);
                self.indent -= 1;
                self.line("end");
                self.hold(name);
            }
            21 => {
                let (name, c) = (self.fresh("t"), self.int_expr(1));
                let (s1, s2) = (self.write_stmt(), self.write_stmt());
                self.line(&format!("let {name} = if ({c}) % 2 == 0 then"));
                self.indent += 1;
                self.line(&s1);
                self.indent -= 1;
                self.line("else");
                self.indent += 1;
                self.line(&s2);
                self.indent -= 1;
                self.line("end");
                self.hold(name);
            }
            // A snapshot of a cell's contents, read back at the end of the
            // block: later writes to the cell must not show through it.
            9 | 10 => {
                let kind = [Kind::Ints, Kind::Grid, Kind::Rec, Kind::Recs]
                    [self.rng.below(4) as usize];
                if let Some(c) = self.cell(kind) {
                    let (name, r) = (self.fresh("s"), self.read(&c));
                    self.line(&format!("let {name} = {r}"));
                    self.hold(name);
                }
            }
            // A helper called for its effect: its result is dropped.
            11 | 12 => {
                if !self.calls.is_empty() {
                    let f = self.calls[self.rng.below(self.calls.len() as u64) as usize]
                        .name
                        .clone();
                    let (a, b) = (self.int_expr(1), self.int_expr(1));
                    self.line(&format!("{f}({a}, {b})"));
                }
            }
            // The same helpers called for their value, which for one ending
            // in a `set` is the cell's container.
            13 | 22 => {
                if !self.calls.is_empty() {
                    let f = self.calls[self.rng.below(self.calls.len() as u64) as usize]
                        .name
                        .clone();
                    let (name, a, b) = (self.fresh("t"), self.int_expr(1), self.int_expr(1));
                    self.line(&format!("let {name} = {f}({a}, {b})"));
                    self.hold(name);
                }
            }
            14 | 15 => {
                let c = self.int_expr(1);
                self.line(&format!("if ({c}) % 2 == 0 then"));
                self.open();
                self.block(3);
                self.close();
                if self.rng.chance(35) {
                    self.line("else");
                    self.open();
                    self.block(2);
                    self.close();
                }
                self.line("end");
            }
            16 | 17 => {
                let k = self.fresh("k");
                self.line(&format!("for {k} in range(0, 2) do"));
                self.open();
                self.ints.push(k);
                self.block(3);
                self.ints.pop();
                self.close();
                self.line("end");
            }
            _ => {
                let e = self.int_expr(2);
                self.line(&format!("print(\"p\", {e})"));
            }
        }
    }

    /// A function body: statements, then a tail that is a write (so the
    /// function's value is a cell's container), a cell read, or an int.
    /// Returns whether the result is an int.
    fn body(&mut self, max: u64) -> bool {
        self.block(max);
        match self.rng.below(10) {
            0..=3 => {
                let s = self.write_stmt();
                // Printed first: a block's held values close before its tail.
                let held = std::mem::take(self.held.last_mut().unwrap());
                if !held.is_empty() {
                    self.line(&format!("print(\"held\", {})", held.join(", ")));
                }
                self.line(&s);
                false
            }
            4 => match self.cell(Kind::Ints) {
                Some(c) => {
                    let r = self.read(&c);
                    let held = std::mem::take(self.held.last_mut().unwrap());
                    if !held.is_empty() {
                        self.line(&format!("print(\"held\", {})", held.join(", ")));
                    }
                    self.line(&r);
                    false
                }
                None => {
                    self.line("0");
                    true
                }
            },
            _ => {
                let e = self.int_expr(2);
                let held = std::mem::take(self.held.last_mut().unwrap());
                if !held.is_empty() {
                    self.line(&format!("print(\"held\", {})", held.join(", ")));
                }
                self.line(&e);
                true
            }
        }
    }

    /// A top-level helper over the module's cells.
    fn helper(&mut self) {
        let name = self.fresh("h");
        self.line(&format!("fn {name}(a, b)"));
        let outer_ints = std::mem::replace(&mut self.ints, vec!["a".into(), "b".into()]);
        self.open();
        let int_result = self.body(4);
        self.indent -= 1;
        self.held.pop();
        self.ints = outer_ints;
        self.line("end");
        self.calls.push(Callable { name, int_result });
    }

    /// A function with its own `var`s and closures over them — the nested
    /// shape, where the cell being written belongs to an enclosing frame.
    fn closure_host(&mut self) {
        let name = self.fresh("outer");
        self.line(&format!("fn {name}(a, b)"));
        let outer_ints = std::mem::replace(&mut self.ints, vec!["a".into(), "b".into()]);
        let outer_cells = self.cells.clone();
        let outer_calls = self.calls.clone();
        // The module's cells are not this function's: no bare reads of them.
        for c in &mut self.cells {
            c.local = false;
        }
        self.open();
        let (loc, lg) = (self.fresh("loc"), self.fresh("lg"));
        self.line(&format!("var {loc} = [a, b, 0, 1]"));
        self.line(&format!("var {lg} = [[a, 0, 1], [b, 2, 3]]"));
        let mine = [
            Cell { name: loc.clone(), kind: Kind::Ints, local: false },
            Cell { name: lg.clone(), kind: Kind::Grid, local: false },
        ];
        self.cells.extend(mine.iter().cloned());
        for _ in 0..(self.rng.below(2) + 1) {
            let c = self.fresh("c");
            self.line(&format!("let {c} = fn(a, b)"));
            self.open();
            let int_result = self.body(3);
            self.indent -= 1;
            self.held.pop();
            self.line("end");
            self.calls.push(Callable { name: c, int_result });
        }
        // The rest of the body is the declaring scope of `loc` and `lg`.
        for c in &mut self.cells {
            c.local = mine.iter().any(|m| m.name == c.name);
        }
        self.block(5);
        let held = std::mem::take(self.held.last_mut().unwrap());
        if !held.is_empty() {
            self.line(&format!("print(\"held\", {})", held.join(", ")));
        }
        self.line(&format!("print(\"locals\", {loc}, {lg})"));
        self.line(&format!("len({loc})"));
        self.indent -= 1;
        self.held.pop();
        self.line("end");
        self.ints = outer_ints;
        self.cells = outer_cells;
        self.calls = outer_calls;
        self.calls.push(Callable { name, int_result: true });
    }

    /// A whole program. `decl` is `var` or `state var`.
    fn program(mut self, decl: &str) -> String {
        let cells = [
            ("xs", Kind::Ints, "[1, 2, 3]"),
            ("ys", Kind::Ints, "[4, 5, 6, 7]"),
            ("g", Kind::Grid, "[[1, 2, 3], [4, 5, 6]]"),
            ("r", Kind::Rec, "{a: 1, b: 2, x: 3}"),
            ("ps", Kind::Recs, "[{a: 1, b: 1, x: 1}, {a: 2, b: 2, x: 2}]"),
        ];
        for (name, kind, init) in cells {
            self.line(&format!("{decl} {name} = {init}"));
            self.cells.push(Cell { name: name.into(), kind, local: false });
        }
        for _ in 0..(self.rng.below(3) + 1) {
            if self.rng.chance(35) {
                self.closure_host();
            } else {
                self.helper();
            }
        }
        // Module scope declares the cells, so bare reads are legal from here.
        for c in &mut self.cells {
            c.local = true;
        }
        self.held.push(Vec::new());
        let n = self.rng.below(8) + 5;
        for _ in 0..n {
            if self.budget <= 0 {
                break;
            }
            self.stmt();
        }
        let held = self.held.pop().unwrap_or_default();
        if !held.is_empty() {
            self.line(&format!("print(\"held\", {})", held.join(", ")));
        }
        self.line("print(\"end\", xs, ys, g, r, ps)");
        self.src
    }
}

/// What a program did over `FRAMES` runs of one stack: everything it printed
/// and the state it left, or the error that stopped it (with what it printed
/// up to then, since a write before the error still has to agree).
type Outcome = (Vec<String>, Result<String, String>);

const FRAMES: usize = 3;

fn run_frames(code: &str, policy: RunPolicy, observe: bool) -> Outcome {
    let mut env = Env::new();
    env.set_policy(policy);
    if observe {
        env.observations_mut().enable();
    }
    let pid = match env.load_program(code) {
        Ok(pid) => pid,
        Err(e) => return (Vec::new(), Err(e)),
    };
    let sid = match env.create_stack(pid) {
        Ok(sid) => sid,
        Err(e) => return (Vec::new(), Err(e)),
    };
    let mut output = Vec::new();
    for frame in 0..FRAMES {
        if frame > 0 {
            if let Err(e) = env.reset_stack(sid) {
                return (output, Err(e));
            }
        }
        let ran = env.run(sid);
        output.extend(env.take_output());
        if let Err(e) = ran {
            return (output, Err(e));
        }
    }
    let state = env.get_state_json(pid, sid);
    let mut pairs: Vec<String> = state.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    (output, Ok(pairs.join(",")))
}

/// The cell pass alone, so a failure is attributed to it rather than to an
/// interaction with the other two in-place routes or with memoization.
const CELLS_ONLY: RunPolicy = RunPolicy::BASELINE.with_opts(OptFlags {
    in_place_cells: true,
    ..OptFlags::none()
});

#[track_caller]
fn assert_cell_parity(seed: u64, decl: &str, code: &str) {
    let oracle = run_frames(code, RunPolicy::BASELINE, false);
    let configs: [(&str, RunPolicy, bool); 5] = [
        ("cell pass alone", CELLS_ONLY, false),
        ("all passes, no memo", RunPolicy::FAST.with_memo(false), false),
        ("all passes, memoized", RunPolicy::REPLAY, false),
        ("fast", RunPolicy::FAST, false),
        ("all passes, memoized, observed", RunPolicy::REPLAY, true),
    ];
    for (what, policy, observe) in configs {
        let got = run_frames(code, policy, observe);
        assert_eq!(
            oracle, got,
            "in-place cell write divergence at seed {seed} ({what}) — reproduce with \
             Gen::new({seed}).program({decl:?})\n--- program ---\n{code}"
        );
    }
}

fn iters() -> u64 {
    std::env::var("PETAL_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200)
}

#[test]
fn cell_differential_fuzz() {
    for seed in 0..iters() {
        let code = Gen::new(seed).program("var");
        assert_cell_parity(seed, "var", &code);
    }
}

/// The same programs with their cells in `state`, so the containers — and
/// each cell's ownership of one — carry over from frame to frame.
#[test]
fn state_cell_differential_fuzz() {
    for seed in 0..iters() {
        let code = Gen::new(seed).program("state var");
        assert_cell_parity(seed, "state var", &code);
    }
}

/// The soak proves nothing unless the programs run and the pass fires on
/// them. Guards the generator against drifting into programs that all fail to
/// compile, or that no longer contain a write the pass can take.
#[test]
fn the_generator_reaches_in_place_cell_writes() {
    let (mut ran, mut fired, mut tail, mut nested) = (0, 0, 0, 0);
    for seed in 0..100 {
        let code = Gen::new(seed).program("var");
        if run_frames(&code, RunPolicy::BASELINE, false).1.is_ok() {
            ran += 1;
        }
        let program = Env::new()
            .compile_program(crate::program::ProgramId(0), &code)
            .unwrap_or_else(|e| panic!("seed {seed} does not compile: {e}\n{code}"));
        let plan = super::cells::analyze(&program);
        if plan.in_place_writes() > 0 {
            fired += 1;
        }
        let bc = super::lower_with_flags(&program, OptFlags::default_on()).expect("lower");
        if super::disasm::render_text(&bc, &program).contains("cell_put_tail") {
            tail += 1;
        }
        if code.contains("= fn(a, b)") {
            nested += 1;
        }
    }
    assert!(ran > 80, "only {ran}/100 generated programs run to completion");
    assert!(fired > 90, "only {fired}/100 generated programs have an in-place cell write");
    assert!(tail > 30, "only {tail}/100 generated programs end a function on a cell write");
    assert!(nested > 20, "only {nested}/100 generated programs write through a closure");
}
