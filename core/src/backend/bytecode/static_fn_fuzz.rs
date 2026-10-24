//! Differential fuzzer for static values inside function bodies
//! (`crate::static_cache`, the `in_fn` module).
//!
//! `super::static_fuzz` writes top-level tables and its functions are a
//! fixed prelude. This generator writes the functions. Each body mixes, on
//! purpose, what may be kept with what looks the same and may not:
//!
//! * tables and records of literals, of kept top-level values, and built
//!   by pure calls, loops, `map` and `sort` over those: constant;
//! * the same shapes with a parameter, a captured top-level value that
//!   changes every run, a loop variable, a pattern variable or a `state`
//!   somewhere inside: not;
//! * names rebound by a branch or a loop that depends on a parameter, and
//!   by one that does not;
//! * every way of writing to a value that may be kept: by index, by field,
//!   by `append`, through an alias, through a function that writes its
//!   parameter, in a loop, inside the function and by the caller on what
//!   the function returned;
//! * recursion and early returns in front of the constants, closures that
//!   capture a constant and closures that capture a parameter, a function
//!   that returns a closure made per call;
//! * reloads: the same program with some literals changed (patched in
//!   place), then with a declaration added (recompiled), then the first
//!   text again.
//!
//! Every program is driven through the same frames and reloads on one
//! stack three ways, under each policy the cache runs under: the cache on,
//! the cache off, and the cache on and checking itself
//! ([`Env::set_static_check`]: the code runs on every call and what it
//! built is compared with what is kept). All three must print the same,
//! raise the same and leave the same state, and the check must find
//! nothing.

use std::collections::HashSet;

use crate::env::{Env, StaticSlotState};
use crate::policy::RunPolicy;

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

/// A list has at least three ints, a grid two rows of three, a record the
/// fields `a`, `b`, `x`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Int,
    Ints,
    Grid,
    Rec,
}

#[derive(Clone)]
struct Name {
    name: String,
    kind: Kind,
}

#[derive(Clone, Default)]
struct Edit {
    literals: HashSet<usize>,
    extra: bool,
}

const PRELUDE: &str = "\
fn scale(xs, k)
  for x in xs do (x * k) % 97 end
end
fn total(xs)
  let t = 0
  for x in xs do t = (t + x) % 997 end
  t
end
fn mk(a, b)
  {a: a, b: b, x: (a + b) % 97}
end
fn bump(xs, v)
  xs[0] = v
  xs
end
fn poke(r, v)
  r.x = v
  r
end
";

struct Gen {
    rng: Rng,
    src: String,
    /// Top-level names a function body may read: constant ones and ones
    /// that change every run.
    globals: Vec<Name>,
    /// The parameters and locals of the body being written.
    locals: Vec<Name>,
    /// Functions written so far, by name: each takes two ints and returns
    /// a list of at least three ints.
    functions: Vec<String>,
    /// The functions that declare `state`, themselves or through a call: a
    /// builtin's callback may not run one, so the calls loop over them instead.
    stateful: HashSet<String>,
    /// Whether the body being written declares `state` or calls a function
    /// that does.
    body_stateful: bool,
    next_id: usize,
    literals: usize,
    edit: Edit,
    indent: usize,
}

macro_rules! emit {
    ($g:expr, $($fmt:tt)*) => {{
        let text = format!($($fmt)*);
        $g.line(&text);
    }};
}

impl Gen {
    fn line(&mut self, s: &str) {
        for l in s.lines() {
            for _ in 0..self.indent {
                self.src.push_str("  ");
            }
            self.src.push_str(l);
            self.src.push('\n');
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    /// An int literal in a value position: what a literal edit changes.
    fn lit(&mut self) -> String {
        let v = self.rng.below(10);
        let n = self.literals;
        self.literals += 1;
        if self.edit.literals.contains(&n) { (v + 11).to_string() } else { v.to_string() }
    }

    fn pick(&mut self, kind: Kind) -> Option<String> {
        let of: Vec<&Name> = self.locals.iter().chain(&self.globals).filter(|n| n.kind == kind).collect();
        if of.is_empty() {
            return None;
        }
        Some(of[self.rng.below(of.len() as u64) as usize].name.clone())
    }

    fn int(&mut self, depth: u64) -> String {
        let top = if depth == 0 { 4 } else { 10 };
        match self.rng.below(top) {
            0 | 1 => self.lit(),
            2 | 3 => self.pick(Kind::Int).unwrap_or_else(|| self.lit()),
            4 => match self.pick(Kind::Ints) {
                Some(t) => format!("{t}[{}]", self.rng.below(3)),
                None => self.lit(),
            },
            5 => match self.pick(Kind::Grid) {
                Some(g) => format!("{g}[{}][{}]", self.rng.below(2), self.rng.below(3)),
                None => self.lit(),
            },
            6 => match self.pick(Kind::Rec) {
                Some(r) => format!("{r}.{}", ["a", "b", "x"][self.rng.below(3) as usize]),
                None => self.lit(),
            },
            7 => match self.pick(Kind::Ints) {
                Some(t) => match self.rng.below(2) {
                    0 => format!("total({t})"),
                    _ => format!("len({t})"),
                },
                None => self.lit(),
            },
            8 if !self.functions.is_empty() => {
                // A call of an earlier function on constants, or not.
                let f = self.functions[self.rng.below(self.functions.len() as u64) as usize].clone();
                self.body_stateful |= self.stateful.contains(&f);
                format!("{f}({}, {})[{}]", self.int(0), self.int(0), self.rng.below(3))
            }
            _ => {
                let (a, b) = (self.int(depth - 1), self.int(depth - 1));
                format!("({a} + {b}) % 97")
            }
        }
    }

    fn ints(&mut self) -> String {
        match self.rng.below(11) {
            0..=2 => {
                let n = 3 + self.rng.below(2);
                let items: Vec<String> = (0..n).map(|_| self.int(1)).collect();
                format!("[{}]", items.join(", "))
            }
            3 => self.pick(Kind::Ints).unwrap_or_else(|| "[1, 2, 3]".into()),
            4 => match self.pick(Kind::Ints) {
                Some(t) => format!("scale({t}, {})", self.int(0)),
                None => format!("scale([{}, {}, 3], 2)", self.lit(), self.lit()),
            },
            5 => match self.pick(Kind::Ints) {
                Some(t) => format!("map({t}, fn(x) (x + {}) % 97 end)", self.int(0)),
                None => format!("[{}, 1, 2]", self.lit()),
            },
            6 => match self.pick(Kind::Ints) {
                Some(t) => format!("for x in {t} do (x + {}) % 97 end", self.int(0)),
                None => format!("for i in range(0, 4) do (i * {}) % 97 end", self.lit()),
            },
            7 => format!("for i in range(0, 3) do (i * {} + {}) % 97 end", self.lit(), self.int(0)),
            8 => match self.pick(Kind::Grid) {
                Some(g) => format!("{g}[{}]", self.rng.below(2)),
                None => format!("[{}, 0, 1]", self.lit()),
            },
            9 if !self.functions.is_empty() => {
                let f = self.functions[self.rng.below(self.functions.len() as u64) as usize].clone();
                self.body_stateful |= self.stateful.contains(&f);
                format!("{f}({}, {})", self.int(0), self.int(0))
            }
            _ => match self.pick(Kind::Ints) {
                Some(t) => format!("sort(concat({t}, [{}, {}]))", self.lit(), self.int(0)),
                None => format!("sort([{}, {}, {}])", self.lit(), self.lit(), self.lit()),
            },
        }
    }

    fn rec(&mut self) -> String {
        match self.rng.below(4) {
            0 => format!("mk({}, {})", self.int(1), self.int(0)),
            1 => match self.pick(Kind::Rec) {
                Some(r) => format!("{{...{r}, a: {}}}", self.int(0)),
                None => format!("mk({}, 1)", self.lit()),
            },
            _ => format!("{{a: {}, b: {}, x: {}}}", self.int(1), self.int(0), self.lit()),
        }
    }

    fn value(&mut self, kind: Kind) -> String {
        match kind {
            Kind::Int => self.int(2),
            Kind::Ints => self.ints(),
            Kind::Grid => match self.rng.below(2) {
                0 => format!("[{}, {}]", self.ints(), self.ints()),
                _ => {
                    let cells: Vec<String> = (0..6).map(|_| self.int(0)).collect();
                    format!("[[{}], [{}]]", cells[..3].join(", "), cells[3..].join(", "))
                }
            },
            Kind::Rec => self.rec(),
        }
    }

    fn any_kind(&mut self) -> Kind {
        [Kind::Int, Kind::Ints, Kind::Ints, Kind::Grid, Kind::Rec][self.rng.below(5) as usize]
    }

    fn local(&mut self, name: &str, kind: Kind) {
        self.locals.push(Name { name: name.to_string(), kind });
    }

    /// One statement of a function body.
    fn body_statement(&mut self) {
        match self.rng.below(16) {
            0..=5 => {
                let kind = self.any_kind();
                let value = self.value(kind);
                let name = self.fresh("v");
                emit!(self, "let {name} = {value}");
                self.local(&name, kind);
            }
            // A write to a local that may hold a constant.
            6 | 7 => {
                let Some(t) = self.pick_local(Kind::Ints) else { return };
                let v = self.int(1);
                match self.rng.below(5) {
                    0 => emit!(self, "{t}[{}] = {v}", self.rng.below(3)),
                    1 => emit!(self, "{t} = append({t}, {v})"),
                    2 => {
                        let m = self.fresh("w");
                        emit!(self, "let {m} = bump({t}, {v})");
                        self.local(&m, Kind::Ints);
                    }
                    3 => {
                        let m = self.fresh("w");
                        emit!(self, "let {m} = {t}");
                        emit!(self, "{m}[{}] = {v}", self.rng.below(3));
                        self.local(&m, Kind::Ints);
                    }
                    _ => emit!(self, "for i in range(0, 3) do\n  {t}[i] = ({t}[i] + {v}) % 97\nend"),
                }
            }
            8 => {
                let Some(r) = self.pick_local(Kind::Rec) else { return };
                let v = self.int(1);
                match self.rng.below(2) {
                    0 => emit!(self, "{r}.{} = {v}", ["a", "b", "x"][self.rng.below(3) as usize]),
                    _ => {
                        let m = self.fresh("w");
                        emit!(self, "let {m} = poke({r}, {v})");
                        self.local(&m, Kind::Rec);
                    }
                }
            }
            // A name a branch rebinds.
            9 => {
                let name = self.fresh("b");
                let (first, cond, second) = (self.ints(), self.int(1), self.ints());
                emit!(self, "let {name} = {first}");
                emit!(self, "if ({cond}) % 2 == 0 then\n  {name} = {second}\nend");
                self.local(&name, Kind::Ints);
            }
            // An accumulator, over a constant range or one a parameter sets.
            10 => {
                let name = self.fresh("acc");
                let (start, bound, step) = (self.int(0), self.int(0), self.int(1));
                emit!(self, "let {name} = {start}");
                emit!(self, "for i in range(0, ({bound}) % 4) do\n  {name} = ({name} + i + {step}) % 97\nend");
                self.local(&name, Kind::Int);
            }
            // A list grown in a loop from a literal start.
            11 => {
                let name = self.fresh("g");
                let (bound, cell) = (self.int(0), self.int(1));
                emit!(self, "let {name} = [0, 1, {}]", self.lit());
                emit!(self, "for i in range(0, ({bound}) % 3) do\n  {name} = append({name}, [i, {cell}][1])\nend");
                self.local(&name, Kind::Ints);
            }
            // A match that binds pattern variables.
            12 => {
                let name = self.fresh("m");
                let (subject, extra) = (self.ints(), self.int(0));
                emit!(
                    self,
                    "let {name} = match {subject}\n  when [a, b, ...rest] -> [b, a, {extra}]\n  when _ -> [0, 0, 0]\nend"
                );
                self.local(&name, Kind::Ints);
            }
            // A closure that captures what is in scope, called here.
            13 => {
                let name = self.fresh("h");
                let (inner, arg) = (self.int(1), self.int(0));
                let table = self.pick(Kind::Ints).unwrap_or_else(|| "[1, 2, 3]".into());
                emit!(self, "let {name}f = fn(x) [x, ({inner}) % 97, {table}[1]] end");
                emit!(self, "let {name} = {name}f({arg})");
                self.local(&name, Kind::Ints);
            }
            // A `var`, written in a loop.
            14 => {
                let name = self.fresh("c");
                let (start, step) = (self.int(0), self.int(1));
                let table = self.pick(Kind::Ints).unwrap_or_else(|| "[1, 2, 3]".into());
                emit!(self, "var {name}v = {start}");
                emit!(self, "for x in {table} do\n  set {name}v = ({name}v + x * {step}) % 97\nend");
                emit!(self, "let {name} = {name}v");
                self.local(&name, Kind::Int);
            }
            _ => {
                let name = self.fresh("s");
                emit!(self, "state {name} = {}", self.lit());
                emit!(self, "{name} = ({name} + 1) % 97");
                self.body_stateful = true;
                self.local(&name, Kind::Int);
            }
        }
    }

    fn pick_local(&mut self, kind: Kind) -> Option<String> {
        let of: Vec<&Name> = self.locals.iter().filter(|n| n.kind == kind && n.name != "p" && n.name != "q").collect();
        if of.is_empty() {
            return None;
        }
        Some(of[self.rng.below(of.len() as u64) as usize].name.clone())
    }

    /// `fn NAME(p, q) ... end`, returning a list of at least three ints.
    fn function(&mut self) {
        let name = self.fresh("f");
        emit!(self, "fn {name}(p, q)");
        self.body_stateful = false;
        self.indent = 1;
        self.locals = vec![
            Name { name: "p".into(), kind: Kind::Int },
            Name { name: "q".into(), kind: Kind::Int },
        ];
        // Sometimes a constant ahead of an early return, sometimes behind.
        let early = self.rng.below(4);
        if early == 0 {
            emit!(self, "if p > 60 then return [p, q, {}] end", self.lit());
        }
        let n = 2 + self.rng.below(6);
        for i in 0..n {
            self.body_statement();
            if early == 1 && i == 0 {
                // Recursion behind the first statement.
                emit!(self, "if p > 3 then return {name}(p - 4, q) end");
            }
        }
        // The result: a local list as it is (a constant may leave the
        // function), or a list of ints read from everything in scope.
        let result = match self.rng.below(3) {
            0 => self.pick_local(Kind::Ints),
            _ => None,
        };
        match result {
            Some(t) => emit!(self, "{t}"),
            None => {
                let items: Vec<String> = (0..3 + self.rng.below(2)).map(|_| self.int(2)).collect();
                emit!(self, "[{}]", items.join(", "));
            }
        }
        self.indent = 0;
        self.line("end");
        self.locals.clear();
        if self.body_stateful {
            self.stateful.insert(name.clone());
        }
        self.functions.push(name);
    }

    /// A function that returns a closure made per call: what the closure
    /// builds from its capture is per call, what it builds from literals
    /// and kept values is not.
    fn maker(&mut self) -> String {
        let name = self.fresh("make");
        emit!(self, "fn {name}(p, q)");
        self.indent = 1;
        self.locals = vec![
            Name { name: "p".into(), kind: Kind::Int },
            Name { name: "q".into(), kind: Kind::Int },
        ];
        if self.rng.chance(50) {
            self.body_statement();
        }
        self.line("fn(x)");
        self.indent = 2;
        self.locals.push(Name { name: "x".into(), kind: Kind::Int });
        for _ in 0..1 + self.rng.below(3) {
            self.body_statement();
        }
        let items: Vec<String> = (0..3).map(|_| self.int(2)).collect();
        emit!(self, "[{}]", items.join(", "));
        self.indent = 1;
        self.line("end");
        self.indent = 0;
        self.line("end");
        self.locals.clear();
        name
    }

    fn program(mut self) -> (String, usize) {
        self.src.push_str(PRELUDE);
        self.line("state n = 0");
        self.line("n = n + 1");
        // What function bodies capture: kept tables, and values of the run.
        for kind in [Kind::Ints, Kind::Grid, Kind::Rec, Kind::Int] {
            let name = self.fresh("K");
            let value = self.value(kind);
            emit!(self, "let {name} = {value}");
            self.globals.push(Name { name, kind });
        }
        let c = self.fresh("C");
        emit!(self, "config let {c} = {}", self.lit());
        self.globals.push(Name { name: c, kind: Kind::Int });
        self.line("let d1 = (n * 7) % 97");
        self.line("let d2 = [n, (n * 3) % 97, 5]");
        self.globals.push(Name { name: "d1".into(), kind: Kind::Int });
        self.globals.push(Name { name: "d2".into(), kind: Kind::Ints });
        if self.edit.extra {
            // The declaration a structural edit adds, in front of every
            // function, with a function of its own.
            self.line("let X1 = [3, 1, 4]");
            self.line("fn xfn(p, q)\n  let t = [X1[0], 2, 7]\n  [t[0] + p, t[1], q]\nend");
        }
        let count = 2 + self.rng.below(4);
        for _ in 0..count {
            self.function();
        }
        let maker = self.maker();
        // The calls: constants and values of the run as arguments, results
        // printed, written to, handed to a function that writes, kept in
        // `state`, and called from a closure.
        let globals = std::mem::take(&mut self.globals);
        for f in self.functions.clone() {
            let (a, b) = (self.lit(), self.lit());
            emit!(self, "print(\"{f}\", {f}(n, {a}), {f}({b}, n % 5), {f}({a}, {b}))");
            match self.rng.below(6) {
                0 => {
                    let r = self.fresh("r");
                    emit!(self, "let {r} = {f}({a}, {b})\n{r}[0] = n\n{r}[1] = n + 1\nprint(\"{r}\", {r}, {f}({a}, {b}))");
                }
                1 => emit!(self, "print(\"bump\", bump({f}({a}, {b}), n), {f}({a}, {b}))"),
                2 => {
                    let r = self.fresh("r");
                    emit!(self, "let {r} = append({f}({a}, {b}), n)\nprint(\"{r}\", append({r}, n), {f}({a}, {b}))");
                }
                3 => {
                    let s = self.fresh("st");
                    emit!(self, "state {s} = {f}({a}, {b})\n{s}[0] = ({s}[0] + n) % 97\nprint(\"{s}\", {s}, {f}({a}, {b}))");
                }
                4 if self.stateful.contains(&f) => {
                    emit!(self, "print(\"map\", for i in [1, 2, 3] do {f}(i, n % 3)[0] end)")
                }
                4 => emit!(self, "print(\"map\", map([1, 2, 3], fn(i) {f}(i, n % 3)[0] end))"),
                _ => {}
            }
        }
        let (a, b) = (self.lit(), self.lit());
        emit!(self, "let c1 = {maker}(n, {a})\nlet c2 = {maker}({b}, {a})\nprint(\"closures\", c1(1), c2(2), c1(n), c2(1))");
        if self.edit.extra {
            self.line("print(\"x\", xfn(n, 1), xfn(2, 2), X1)");
        }
        self.globals = globals;
        (self.src, self.literals)
    }
}

/// The three texts of a seed: the plain program, the same with a third of
/// its literals changed, and that with a declaration and a function added.
fn versions(seed: u64) -> [String; 3] {
    let gen_with = |edit: Edit| {
        Gen {
            rng: Rng::new(seed),
            src: String::new(),
            globals: Vec::new(),
            locals: Vec::new(),
            functions: Vec::new(),
            stateful: HashSet::new(),
            body_stateful: false,
            next_id: 0,
            literals: 0,
            edit,
            indent: 0,
        }
        .program()
    };
    let (plain, literals) = gen_with(Edit::default());
    let mut rng = Rng::new(seed ^ 0xA5A5_5A5A);
    let mut edit = Edit::default();
    for i in 0..literals {
        if rng.chance(34) {
            edit.literals.insert(i);
        }
    }
    if edit.literals.is_empty() && literals > 0 {
        edit.literals.insert(rng.below(literals as u64) as usize);
    }
    let (patched, _) = gen_with(edit.clone());
    edit.extra = true;
    let (grown, _) = gen_with(edit);
    [plain, patched, grown]
}

struct Driven {
    transcript: Vec<String>,
    /// Values kept for function bodies when the run ended, and how many
    /// skips the stack took in all.
    kept: usize,
    skips: u64,
    mismatches: Vec<String>,
    reloads: Vec<String>,
    errors: usize,
}

fn drive(texts: &[String; 3], policy: RunPolicy, check: bool) -> Driven {
    let mut env = Env::new();
    env.set_policy(policy);
    env.set_echo(false);
    env.set_static_check(check);
    let mut out = Driven {
        transcript: Vec::new(),
        kept: 0,
        skips: 0,
        mismatches: Vec::new(),
        reloads: Vec::new(),
        errors: 0,
    };
    let pid = match env.load_program(&texts[0]) {
        Ok(pid) => pid,
        Err(e) => {
            out.transcript.push(format!("compile error {e}"));
            out.errors += 1;
            return out;
        }
    };
    let sid = env.create_stack(pid).expect("stack");
    let run = |env: &mut Env, out: &mut Driven, reset: bool| {
        if reset {
            env.reset_stack(sid).expect("reset");
        }
        let result = env.run(sid);
        out.transcript.extend(env.take_output());
        match result {
            Ok(v) => out
                .transcript
                .push(format!("=> {}", crate::value::value_to_display_string(&v, env.heap()))),
            Err(e) => {
                out.errors += 1;
                out.transcript.push(format!("error {e}"));
            }
        }
    };
    for _ in 0..3 {
        run(&mut env, &mut out, true);
    }
    for next in [1, 2, 0] {
        match env.reload_program(sid, &texts[next], None) {
            Ok(report) => {
                let how = format!("{:?}", report.outcome);
                out.transcript.push(format!("reload {how}"));
                out.reloads.push(how);
            }
            Err(e) => {
                out.errors += 1;
                out.transcript.push(format!("reload error {e}"));
            }
        }
        // The run straight after a reload is not reset, as a host's is not.
        run(&mut env, &mut out, false);
        run(&mut env, &mut out, true);
        run(&mut env, &mut out, true);
    }
    let state = env.get_state_json(pid, sid);
    let mut pairs: Vec<String> = state.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    out.transcript.push(pairs.join(","));
    if let Some(values) = env.static_values(sid) {
        out.skips = values.stats.skips;
        out.kept = values
            .in_functions
            .iter()
            .filter(|(_, s)| matches!(s, StaticSlotState::Filled(_)))
            .count();
        out.mismatches = values.mismatches;
    }
    out
}

const POLICIES: [&str; 4] = ["replay", "replay-memo", "fast", "baseline"];

#[track_caller]
fn assert_parity(seed: u64) -> Driven {
    let texts = versions(seed);
    let mut first = None;
    for name in POLICIES {
        let base = RunPolicy::parse(name).unwrap();
        let on = drive(&texts, base.with_static_cache(true), false);
        let off = drive(&texts, base.with_static_cache(false), false);
        let checked = drive(&texts, base.with_static_cache(true), true);
        for (what, side) in [("on", &on), ("checking itself", &checked)] {
            if side.transcript != off.transcript {
                let at = side
                    .transcript
                    .iter()
                    .zip(&off.transcript)
                    .position(|(a, b)| a != b)
                    .unwrap_or(side.transcript.len().min(off.transcript.len()));
                panic!(
                    "divergence at seed {seed} under {name} with the cache {what}, line {at} of the transcript:\n  \
                     cache: {:?}\n  off:   {:?}\n--- program ---\n{}\n--- with literals changed ---\n{}\n\
                     --- with a declaration added ---\n{}",
                    side.transcript.get(at),
                    off.transcript.get(at),
                    texts[0],
                    texts[1],
                    texts[2],
                );
            }
        }
        assert!(
            checked.mismatches.is_empty(),
            "seed {seed} under {name}: a kept value was built different on a later call: {:?}\n{}",
            checked.mismatches,
            texts[0]
        );
        first.get_or_insert(on);
    }
    first.unwrap()
}

fn iters() -> u64 {
    std::env::var("PETAL_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100)
}

#[test]
fn static_in_functions_differential_fuzz() {
    for seed in 0..iters() {
        assert_parity(seed);
    }
}

/// The soak proves nothing unless the programs compile, run, keep values
/// in function bodies and are reloaded both ways.
#[test]
fn the_generator_reaches_function_constants() {
    let (mut clean, mut kept, mut patched, mut recompiled, mut total) = (0, 0, 0, 0, 0);
    for seed in 0..60 {
        let texts = versions(seed);
        let d = drive(&texts, RunPolicy::REPLAY, false);
        assert!(
            !d.transcript[0].starts_with("compile error"),
            "seed {seed} does not compile: {}\n{}",
            d.transcript[0],
            texts[0]
        );
        clean += (d.errors == 0) as u32;
        kept += (d.kept >= 2 && d.skips >= 12) as u32;
        total += d.kept;
        patched += (d.reloads.first().map(String::as_str) == Some("Patched")) as u32;
        recompiled += (d.reloads.get(1).map(String::as_str) == Some("Recompiled")) as u32;
    }
    assert!(clean >= 50, "only {clean}/60 generated programs run without an error");
    assert!(kept >= 45, "only {kept}/60 kept values in function bodies and loaded them ({total} in all)");
    assert!(patched >= 45, "only {patched}/60 literal edits were patched in place");
    assert!(recompiled >= 50, "only {recompiled}/60 added declarations were a recompile");
}
