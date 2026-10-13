//! Differential fuzzer for lists that begin as `Enum.all`.
//!
//! `Enum.all` yields the one list the stack keeps for the enum
//! (`EnumTable::all`): every read hands out the same heap object, and no
//! reader may write through it. A read that is the init of a phi is the
//! exception: it copies, and the escape analysis takes it for a fresh root
//! (`escape::Analysis::copies_enum_all`), so the loop that follows may rewrite
//! the copy in place. The two fuzzers beside this one (`super::fuzz`,
//! `super::cell_fuzz`) declare no enum, so neither reaches either half of
//! that: a shared list an in-place route might write through, or a copy the
//! analysis counted on that the lowering did not make.
//!
//! This generator does little else. Its programs start their lists from
//! `K.all`, from a function that returns it, and from containers that hold
//! it, in `let`s, `var`s, `state`, parameters, record fields and nested
//! lists, and then rewrite them by every route the optimizer has (index
//! writes, `append` accumulators, cell writes) inside loops and branches,
//! with the aliasing that could expose a wrong write mixed in on purpose:
//!
//! * a variable rebound to the shared list in one arm of a branch or on one
//!   iteration of a loop, and written on the next;
//! * snapshots (`let s = xs`, `let s = K.all`) taken before later writes and
//!   printed after;
//! * the shared list passed to a helper that writes its parameter;
//! * the shared list stored as a row of a nested list or a field of a
//!   record, and written through that.
//!
//! Every program is run for several frames on one stack under the
//! clone-and-alloc baseline and under each configuration that can write in
//! place, with the static-value cache on and off, and all of them must print
//! the same thing and leave the same state. Each frame's last line prints
//! `K.all` and `J.all` themselves, which must be the declared variants
//! whatever the rest of the program did.

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

    fn pick<'a>(&mut self, of: &[&'a str]) -> &'a str {
        of[self.below(of.len() as u64) as usize]
    }
}

/// What a variable holds. Every flat list keeps at least three elements (the
/// smaller enum has three variants, and nothing here shortens a list), every
/// nest at least two rows, so the generated indices stay in range.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// A list that began as some `Enum.all`.
    Flat,
    /// `[flat, flat, …]`
    Nest,
    /// `{xs: flat, n: int}`
    Rec,
}

#[derive(Clone, Copy, PartialEq)]
enum Decl {
    /// `let`, a parameter or `state`: rebound with `x = …`.
    Let,
    /// `var` or `state var`: written with `set x = …`.
    Var,
}

#[derive(Clone)]
struct Var {
    name: String,
    kind: Kind,
    decl: Decl,
}

impl Var {
    fn set(&self) -> &'static str {
        if self.decl == Decl::Var { "set " } else { "" }
    }
}

struct Gen {
    rng: Rng,
    src: String,
    indent: usize,
    vars: Vec<Var>,
    /// Helpers `h(xs, a)` that write their list parameter and return a list.
    helpers: Vec<String>,
    /// Non-negative int names in scope (parameters, loop counters).
    ints: Vec<String>,
    /// Snapshots bound in each open block, printed as the block closes,
    /// after whatever writes followed them.
    held: Vec<Vec<String>>,
    /// How many of `vars` were in scope as each open block began.
    scopes: Vec<usize>,
    next_id: usize,
    budget: i64,
}

const PRELUDE: &str = "\
enum K
  a(v: 1),
  b(v: 2),
  c(v: 3),
  d(v: 4),
end
enum J
  p,
  q,
  r,
end
fn all_k()
  K.all
end
";

/// The line every frame must end on, whatever the program did.
const PRISTINE: &str = "end [K.a, K.b, K.c, K.d] [J.p, J.q, J.r] [K.a, K.b, K.c, K.d]";

impl Gen {
    fn new(seed: u64) -> Gen {
        Gen {
            rng: Rng::new(seed),
            src: String::new(),
            indent: 0,
            vars: Vec::new(),
            helpers: Vec::new(),
            ints: Vec::new(),
            held: Vec::new(),
            scopes: Vec::new(),
            next_id: 0,
            budget: 60,
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

    fn var(&mut self, kind: Kind) -> Option<Var> {
        let of_kind: Vec<&Var> = self.vars.iter().filter(|v| v.kind == kind).collect();
        if of_kind.is_empty() {
            return None;
        }
        Some(of_kind[self.rng.below(of_kind.len() as u64) as usize].clone())
    }

    fn int(&mut self) -> String {
        if !self.ints.is_empty() && self.rng.chance(60) {
            return self.ints[self.rng.below(self.ints.len() as u64) as usize].clone();
        }
        self.rng.below(5).to_string()
    }

    /// An index below `n`.
    fn index(&mut self, n: u64) -> String {
        if !self.ints.is_empty() && self.rng.chance(45) {
            let i = self.ints[self.rng.below(self.ints.len() as u64) as usize].clone();
            return format!("{i} % {n}");
        }
        self.rng.below(n).to_string()
    }

    /// Something to store in a flat list.
    fn elem(&mut self) -> String {
        match self.rng.below(8) {
            0 | 1 => self.rng.pick(&["K.a", "K.b", "K.c", "K.d", "J.p", "J.r"]).to_string(),
            2 => format!("K.all[{}]", self.index(4)),
            3 => match self.var(Kind::Flat) {
                Some(v) => format!("{}[{}]", v.name, self.index(3)),
                None => "K.d".into(),
            },
            4 => match self.var(Kind::Flat) {
                Some(v) => format!("len({})", v.name),
                None => "7".into(),
            },
            5 => format!("K.all[{}].v", self.index(4)),
            _ => self.int(),
        }
    }

    /// A flat list: the shared one, by some route, more often than not.
    fn flat(&mut self) -> String {
        match self.rng.below(16) {
            0..=3 => "K.all".into(),
            4 => "J.all".into(),
            5 => "all_k()".into(),
            6 | 7 => match self.var(Kind::Flat) {
                Some(v) => v.name,
                None => "K.all".into(),
            },
            8 => match self.var(Kind::Nest) {
                Some(v) => format!("{}[{}]", v.name, self.index(2)),
                None => "J.all".into(),
            },
            9 => match self.var(Kind::Rec) {
                Some(v) => format!("{}.xs", v.name),
                None => "all_k()".into(),
            },
            10 => {
                let (c, other) = (self.int(), self.var(Kind::Flat).map(|v| v.name));
                let other = other.unwrap_or_else(|| "J.all".into());
                if self.rng.chance(50) {
                    format!("if {c} % 2 == 0 then K.all else {other} end")
                } else {
                    format!("if {c} % 2 == 0 then {other} else K.all end")
                }
            }
            11 => match self.var(Kind::Flat) {
                Some(v) => format!("concat({}, K.all)", v.name),
                None => "concat(K.all, J.all)".into(),
            },
            12 => format!("reverse({})", self.rng.pick(&["K.all", "J.all", "all_k()"])),
            13 if !self.helpers.is_empty() => {
                let h = self.helpers[self.rng.below(self.helpers.len() as u64) as usize].clone();
                let arg = match self.var(Kind::Flat) {
                    Some(v) if self.rng.chance(50) => v.name,
                    _ => "K.all".into(),
                };
                format!("{h}({arg}, {})", self.int())
            }
            14 => format!("append(K.all, {})", self.elem()),
            _ => "K.all".into(),
        }
    }

    fn value(&mut self, kind: Kind) -> String {
        match kind {
            Kind::Flat => self.flat(),
            Kind::Nest => {
                let (a, b) = (self.flat(), self.flat());
                if self.rng.chance(40) { "[K.all, K.all]".into() } else { format!("[{a}, {b}]") }
            }
            Kind::Rec => format!("{{ xs: {}, n: {} }}", self.flat(), self.int()),
        }
    }

    fn open(&mut self) {
        self.indent += 1;
        self.held.push(Vec::new());
        self.scopes.push(self.vars.len());
    }

    fn close(&mut self) {
        let held = self.held.pop().unwrap_or_default();
        if !held.is_empty() {
            self.line(&format!("print(\"held\", {})", held.join(", ")));
        }
        self.indent -= 1;
        // What the block declared is out of scope after it.
        if let Some(n) = self.scopes.pop() {
            self.vars.truncate(n);
        }
    }

    fn hold(&mut self, name: String) {
        if let Some(h) = self.held.last_mut() {
            h.push(name);
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

    /// One statement that writes or rebinds a variable.
    fn write_stmt(&mut self) -> String {
        for _ in 0..8 {
            let s = match self.rng.below(14) {
                0..=2 => self.var(Kind::Flat).map(|v| {
                    let (i, e) = (self.index(3), self.elem());
                    format!("{}{}[{i}] = {e}", v.set(), v.name)
                }),
                3..=5 => self.var(Kind::Flat).map(|v| {
                    let e = self.elem();
                    format!("{}{} = append({}, {e})", v.set(), v.name, v.name)
                }),
                // Rebound to another list, the shared one included: the
                // writes that follow must not reach it.
                6 | 7 => self.var(Kind::Flat).map(|v| {
                    let f = self.flat();
                    format!("{}{} = {f}", v.set(), v.name)
                }),
                8 => self.var(Kind::Nest).map(|v| {
                    let (i, j, e) = (self.index(2), self.index(3), self.elem());
                    format!("{}{}[{i}][{j}] = {e}", v.set(), v.name)
                }),
                9 => self.var(Kind::Nest).map(|v| {
                    let (i, f) = (self.index(2), self.flat());
                    format!("{}{}[{i}] = {f}", v.set(), v.name)
                }),
                10 => self.var(Kind::Nest).map(|v| {
                    let f = self.flat();
                    format!("{}{} = append({}, {f})", v.set(), v.name, v.name)
                }),
                11 => self.var(Kind::Rec).map(|v| {
                    let (i, e) = (self.index(3), self.elem());
                    format!("{}{}.xs[{i}] = {e}", v.set(), v.name)
                }),
                12 => self.var(Kind::Rec).map(|v| {
                    let e = self.elem();
                    format!("{}{}.xs = append({}.xs, {e})", v.set(), v.name, v.name)
                }),
                _ => self.var(Kind::Rec).map(|v| {
                    let f = self.flat();
                    format!("{}{}.xs = {f}", v.set(), v.name)
                }),
            };
            if let Some(s) = s {
                return s;
            }
        }
        "nil".into()
    }

    fn declare(&mut self) {
        let kind = match self.rng.below(10) {
            0..=6 => Kind::Flat,
            7 | 8 => Kind::Nest,
            _ => Kind::Rec,
        };
        let decl = if self.rng.chance(45) { Decl::Var } else { Decl::Let };
        let (name, value) = (self.fresh("v"), self.value(kind));
        self.line(&format!("{} {name} = {value}", if decl == Decl::Var { "var" } else { "let" }));
        self.vars.push(Var { name, kind, decl });
    }

    fn stmt(&mut self) {
        self.budget -= 1;
        match self.rng.below(27) {
            0..=9 => {
                let s = self.write_stmt();
                self.line(&s);
            }
            // A snapshot, read back at the end of the block: later writes to
            // the variable must not show through it.
            10 | 11 => {
                let kind = [Kind::Flat, Kind::Flat, Kind::Nest, Kind::Rec][self.rng.below(4) as usize];
                if let Some(v) = self.var(kind) {
                    let name = self.fresh("s");
                    self.line(&format!("let {name} = {}", v.name));
                    self.hold(name);
                }
            }
            // The shared list itself, held across the writes that follow.
            12 => {
                let (name, f) = (self.fresh("s"), self.flat());
                self.line(&format!("let {name} = {f}"));
                self.hold(name);
            }
            13 | 14 => {
                let c = self.int();
                self.line(&format!("if ({c}) % 2 == 0 then"));
                self.open();
                self.block(3);
                self.close();
                if self.rng.chance(50) {
                    self.line("else");
                    self.open();
                    self.block(2);
                    self.close();
                }
                self.line("end");
            }
            15..=17 => {
                let k = self.fresh("k");
                let n = self.rng.below(3) + 1;
                self.line(&format!("for {k} in range(0, {n}) do"));
                self.open();
                self.ints.push(k);
                self.block(3);
                self.ints.pop();
                self.close();
                self.line("end");
            }
            18 => {
                let w = self.fresh("w");
                self.line(&format!("var {w} = 0"));
                self.line(&format!("while {w} < 2 do"));
                self.open();
                self.block(3);
                self.line(&format!("set {w} = {w} + 1"));
                self.close();
                self.line("end");
            }
            19 => self.declare(),
            // The case the copying read exists for: a `let` that starts as
            // the shared list and is rebound in a loop or a branch, so the
            // read is the init of a phi.
            20 | 24..=26 => {
                let name = self.fresh("v");
                let all = self.rng.pick(&["K.all", "K.all", "J.all"]);
                self.line(&format!("let {name} = {all}"));
                let v = Var { name, kind: Kind::Flat, decl: Decl::Let };
                self.vars.push(v.clone());
                let rebind = |g: &mut Gen| {
                    let s = match g.rng.below(4) {
                        0 | 1 => format!("{} = append({}, {})", v.name, v.name, g.elem()),
                        2 => format!("{}[{}] = {}", v.name, g.index(3), g.elem()),
                        _ => format!("{} = {}", v.name, g.flat()),
                    };
                    g.line(&s);
                };
                if self.rng.chance(70) {
                    let (k, n) = (self.fresh("k"), self.rng.below(3) + 1);
                    self.line(&format!("for {k} in range(0, {n}) do"));
                    self.open();
                    self.ints.push(k);
                    rebind(self);
                    if self.rng.chance(50) {
                        self.block(2);
                    }
                    self.ints.pop();
                    self.close();
                } else {
                    let c = self.int();
                    self.line(&format!("if ({c}) % 2 == 0 then"));
                    self.open();
                    rebind(self);
                    self.close();
                }
                self.line("end");
            }
            // A write in value position: a collecting loop keeps every
            // iteration's list.
            21 => {
                let (name, k, s) = (self.fresh("t"), self.fresh("k"), self.write_stmt());
                self.line(&format!("let {name} = for {k} in range(0, 2) do"));
                self.indent += 1;
                self.line(&s);
                self.indent -= 1;
                self.line("end");
                self.hold(name);
            }
            _ => {
                if let Some(v) = self.var(Kind::Flat) {
                    self.line(&format!("print(\"p\", {}, K.all)", v.name));
                }
            }
        }
    }

    /// The value a function body or the module ends on: everything it
    /// holds, and the shared lists.
    fn finish(&mut self) -> String {
        let held = self.held.pop().unwrap_or_default();
        let mut all: Vec<String> = self.vars.iter().map(|v| v.name.clone()).collect();
        all.extend(held);
        all.push("K.all".into());
        all.push("J.all".into());
        format!("[{}]", all.join(", "))
    }

    /// `fn h(xs, a)`: writes its list parameter, returns a flat list.
    fn helper(&mut self) {
        let name = self.fresh("h");
        self.line(&format!("fn {name}(xs, a)"));
        self.indent += 1;
        let (vars, ints) = (std::mem::take(&mut self.vars), std::mem::take(&mut self.ints));
        self.vars.push(Var { name: "xs".into(), kind: Kind::Flat, decl: Decl::Let });
        self.ints.push("a".into());
        self.held.push(Vec::new());
        self.block(4);
        let held = self.held.pop().unwrap_or_default();
        if !held.is_empty() {
            self.line(&format!("print(\"held\", {})", held.join(", ")));
        }
        let tail = match self.rng.below(5) {
            0 | 1 => "xs".to_string(),
            2 => "K.all".to_string(),
            3 => format!("append(xs, {})", self.elem()),
            _ => self.vars[self.rng.below(self.vars.len() as u64) as usize].clone().name,
        };
        // A variable declared in the body may be a nest or a record.
        let tail = match self.vars.iter().find(|v| v.name == tail) {
            Some(v) if v.kind != Kind::Flat => "xs".to_string(),
            _ => tail,
        };
        self.line(&tail);
        self.indent -= 1;
        self.line("end");
        self.vars = vars;
        self.ints = ints;
        self.helpers.push(name);
    }

    /// `fn f(a)`: declares its own lists, rewrites them, returns them all.
    fn host(&mut self) -> String {
        let name = self.fresh("f");
        self.line(&format!("fn {name}(a)"));
        self.indent += 1;
        let (vars, ints) = (std::mem::take(&mut self.vars), std::mem::take(&mut self.ints));
        self.ints.push("a".into());
        self.held.push(Vec::new());
        for _ in 0..(self.rng.below(2) + 1) {
            self.declare();
        }
        self.block(8);
        let tail = self.finish();
        self.line(&tail);
        self.indent -= 1;
        self.line("end");
        self.vars = vars;
        self.ints = ints;
        name
    }

    fn program(mut self) -> String {
        self.src.push_str(PRELUDE);
        for _ in 0..self.rng.below(3) {
            self.helper();
        }
        let hosts: Vec<String> = (0..(self.rng.below(3) + 1)).map(|_| self.host()).collect();
        // Module scope: lists in `state`, whose containers carry over from
        // frame to frame, beside plain ones the static-value cache may keep.
        self.line("state n = 0");
        self.line("n = n + 1");
        self.ints.push("n".into());
        self.line("state kept = K.all");
        self.vars.push(Var { name: "kept".into(), kind: Kind::Flat, decl: Decl::Let });
        if self.rng.chance(60) {
            self.line("state var cell = all_k()");
            self.vars.push(Var { name: "cell".into(), kind: Kind::Flat, decl: Decl::Var });
        }
        if self.rng.chance(40) {
            self.line("state rows = [K.all, J.all]");
            self.vars.push(Var { name: "rows".into(), kind: Kind::Nest, decl: Decl::Let });
        }
        self.held.push(Vec::new());
        self.declare();
        self.block(8);
        let all = self.finish();
        self.line(&format!("print(\"top\", {all})"));
        for f in hosts {
            self.line(&format!("print(\"{f}\", {f}(n), {f}(2))"));
        }
        self.line("print(\"end\", K.all, J.all, all_k())");
        self.src
    }
}

/// What a program did over `FRAMES` runs of one stack: everything it printed
/// and the state it left, or the error that stopped it.
type Outcome = (Vec<String>, Result<String, String>);

const FRAMES: usize = 3;

fn run_frames(code: &str, policy: RunPolicy, observe: bool, check: bool) -> (Outcome, Vec<String>) {
    let mut env = Env::new();
    env.set_policy(policy);
    env.set_echo(false);
    env.set_static_check(check);
    if observe {
        env.observations_mut().enable();
    }
    let pid = match env.load_program(code) {
        Ok(pid) => pid,
        Err(e) => return ((Vec::new(), Err(e)), Vec::new()),
    };
    let sid = match env.create_stack(pid) {
        Ok(sid) => sid,
        Err(e) => return ((Vec::new(), Err(e)), Vec::new()),
    };
    let mut output = Vec::new();
    for frame in 0..FRAMES {
        if frame > 0 && let Err(e) = env.reset_stack(sid) {
            return ((output, Err(e)), Vec::new());
        }
        let ran = env.run(sid);
        output.extend(env.take_output());
        if let Err(e) = ran {
            return ((output, Err(e)), Vec::new());
        }
    }
    let state = env.get_state_json(pid, sid);
    let mut pairs: Vec<String> = state.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    let mismatches = env.static_values(sid).map(|v| v.mismatches).unwrap_or_default();
    ((output, Ok(pairs.join(","))), mismatches)
}

/// Each in-place route alone, so a failure is attributed to it.
const ESCAPE_ONLY: RunPolicy =
    RunPolicy::BASELINE.with_opts(OptFlags { in_place_mutation: true, ..OptFlags::none() });
const STRAIGHT_ONLY: RunPolicy =
    RunPolicy::BASELINE.with_opts(OptFlags { in_place_straight_line: true, ..OptFlags::none() });
const CELLS_ONLY: RunPolicy =
    RunPolicy::BASELINE.with_opts(OptFlags { in_place_cells: true, ..OptFlags::none() });

#[track_caller]
fn assert_enum_all_parity(seed: u64, code: &str) {
    let (oracle, _) = run_frames(code, RunPolicy::BASELINE, false, false);
    if oracle.1.is_ok() {
        let ends: Vec<&String> = oracle.0.iter().filter(|l| l.starts_with("end ")).collect();
        assert_eq!(ends.len(), FRAMES, "seed {seed}: a frame did not reach its end\n{code}");
        for end in ends {
            assert_eq!(end, PRISTINE, "seed {seed}: `Enum.all` was written through (baseline)\n{code}");
        }
    }
    let configs: [(&str, RunPolicy, bool, bool); 9] = [
        ("escape route alone", ESCAPE_ONLY, false, false),
        ("straight-line route alone", STRAIGHT_ONLY, false, false),
        ("cell route alone", CELLS_ONLY, false, false),
        ("all passes, no memo", RunPolicy::FAST.with_memo(false), false, false),
        ("all passes, no static cache", RunPolicy::FAST.with_static_cache(false), false, false),
        ("fast", RunPolicy::FAST, false, false),
        ("fast, checking kept values", RunPolicy::FAST, false, true),
        ("all passes, memoized", RunPolicy::REPLAY, false, false),
        ("all passes, memoized, observed", RunPolicy::REPLAY, true, false),
    ];
    for (what, policy, observe, check) in configs {
        let (got, mismatches) = run_frames(code, policy, observe, check);
        if oracle != got {
            // The first line that differs, not two whole transcripts.
            let at = oracle.0.iter().zip(&got.0).position(|(a, b)| a != b);
            let (want, have) = match at {
                Some(i) => (oracle.0[i].clone(), got.0[i].clone()),
                None => (
                    format!("{} lines, then {:?}", oracle.0.len(), oracle.1),
                    format!("{} lines, then {:?}", got.0.len(), got.1),
                ),
            };
            panic!(
                "divergence on a list that began as `Enum.all`, seed {seed} ({what}) — reproduce with \
                 Gen::new({seed}).program()\nbaseline: {want}\n     got: {have}\n--- program ---\n{code}"
            );
        }
        assert!(mismatches.is_empty(), "seed {seed} ({what}): {mismatches:?}\n--- program ---\n{code}");
    }
}

fn iters() -> u64 {
    std::env::var("PETAL_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(150)
}

/// The default covers seed 148, this fuzzer's first find, which was not in
/// the `Enum.all` path at all: a reference taken inside an `if`, in front of
/// a loop that rewrote the list in place, showed the rewritten list
/// (`inplace_does_not_fire_past_a_reference_taken_inside_the_guard` in
/// `tests.rs`).
#[test]
fn enum_all_differential_fuzz() {
    for seed in 0..iters() {
        let code = Gen::new(seed).program();
        assert_enum_all_parity(seed, &code);
    }
}

/// The soak proves nothing unless the programs run, read `Enum.all` where the
/// analysis makes it copy, and are rewritten in place by each route.
#[test]
fn the_generator_reaches_the_copying_read_and_every_in_place_route() {
    let (mut ran, mut copies, mut escape, mut straight, mut cells) = (0, 0, 0, 0, 0);
    let n = 100;
    for seed in 0..n {
        let code = Gen::new(seed).program();
        if run_frames(&code, RunPolicy::BASELINE, false, false).0.1.is_ok() {
            ran += 1;
        }
        let program = Env::new()
            .compile_program(crate::program::ProgramId(0), &code)
            .unwrap_or_else(|e| panic!("seed {seed} does not compile: {e}\n{code}"));
        let set = super::escape::analyze(&program);
        if program.terms.iter().any(|t| set.copies_enum_all(t.id)) {
            copies += 1;
        }
        if !set.is_empty() {
            escape += 1;
        }
        if super::cells::analyze(&program).in_place_writes() > 0 {
            cells += 1;
        }
        let count = |flags: OptFlags| {
            let bc = super::lower_with_flags(&program, flags).expect("lower");
            super::disasm::render_text(&bc, &program).matches("in_place").count()
        };
        if count(OptFlags { in_place_straight_line: true, ..OptFlags::none() }) > count(OptFlags::none()) {
            straight += 1;
        }
    }
    println!("ran {ran}, copies {copies}, escape {escape}, cells {cells}, straight {straight} of {n}");
    assert!(ran * 10 >= n * 9, "only {ran} of {n} programs run to the end");
    assert!(copies * 2 >= n, "only {copies} of {n} programs have an `Enum.all` read that copies");
    assert!(escape * 2 >= n, "only {escape} of {n} programs have a write the escape analysis takes");
    assert!(cells * 4 >= n, "only {cells} of {n} programs have an in-place cell write");
    assert!(straight * 4 >= n, "only {straight} of {n} programs have a straight-line in-place write");
}
