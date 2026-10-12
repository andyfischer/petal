//! Differential fuzzer for the static-value cache (`crate::static_cache`).
//!
//! The general fuzzer (`super::fuzz`) runs its programs three times with the
//! cache on and off, but its top level is mostly dynamic and it never
//! reloads. This generator writes what the cache is for and what could
//! break it, on purpose:
//!
//! * top-level tables of literals, of other tables, and built by
//!   constructor functions, loops, `map`, `sort` and `concat`, declared with
//!   `let`, `const` and `config let`, with dynamic declarations between them
//!   so that runs start and stop everywhere;
//! * enums whose constant data is static, and one whose data is not;
//! * every way of writing to a value that may be kept: through an alias,
//!   through an element taken out of it, through a function that writes its
//!   parameter, through a `var` cell and through a `state` cell seeded from
//!   it, each followed by a print of both sides, and each also on a static
//!   expression written where it is used (`append([[1], [2]], n)`), which is
//!   a kept value nothing names;
//! * loops at top level, over static tables and with tables built in the
//!   body, and accumulators filled from static data (static inputs, but
//!   reassigned, so never kept);
//! * reloads: the same program with some literals changed (patched in place,
//!   nothing re-lowered), then with a declaration added (recompiled), then
//!   back to the first text.
//!
//! Every program is driven through the same frames and reloads on one stack
//! with the cache on and with it off, under each policy the cache runs
//! under, and both sides must print the same thing, raise the same errors,
//! report the same reloads and leave the same state.

use std::collections::HashSet;

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

/// What a name holds. A list has at least three elements, a grid two rows
/// of three, a record the fields `a`, `b`, `x`, and a record list two
/// records, so generated indices stay in range whatever was appended.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Int,
    Ints,
    Grid,
    Rec,
    Recs,
}

#[derive(Clone)]
struct Name {
    name: String,
    kind: Kind,
    /// Built from literals and other such names only, and never bound
    /// again: what the classifier calls static, so it may be a `const` and
    /// may feed one.
    fixed: bool,
}

/// What to change relative to the plain program of a seed.
#[derive(Clone, Default)]
pub(super) struct Edit {
    /// Ordinals of the literals to write with another value.
    literals: HashSet<usize>,
    /// The statement after which one more is added.
    extra_after: Option<usize>,
}

/// The helpers every program starts with. `scale`, `total`, `mk` and `rows`
/// are static-pure; `bump` and `poke` write their parameter; `tick` reads
/// `state`.
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
fn rows(n, k)
  let out = []
  let i = 0
  while i < n do
    out = append(out, [i, (i * k) % 97, 1])
    i = i + 1
  end
  out
end
fn bump(xs, v)
  xs[0] = v
  xs
end
fn poke(r, v)
  r.x = v
  r
end
fn tick()
  state t = 0
  t = t + 1
  t
end
";

/// `g.line(&format!(…))`, with the arguments evaluated first: they draw
/// from the generator's own random stream.
macro_rules! emit {
    ($g:expr, $($fmt:tt)*) => {{
        let text = format!($($fmt)*);
        $g.line(&text);
    }};
}

struct Gen {
    rng: Rng,
    src: String,
    names: Vec<Name>,
    /// Names the added statement of an [`Edit`] declared: printed at the
    /// end, never read by the rest (the rest must not depend on the edit).
    extras: Vec<String>,
    next_id: usize,
    literals: usize,
    statements: usize,
    edit: Edit,
    has_enum: bool,
}

impl Gen {
    fn new(seed: u64, edit: Edit) -> Gen {
        Gen {
            rng: Rng::new(seed),
            src: String::new(),
            names: Vec::new(),
            extras: Vec::new(),
            next_id: 0,
            literals: 0,
            statements: 0,
            edit,
            has_enum: false,
        }
    }

    fn line(&mut self, s: &str) {
        self.src.push_str(s);
        self.src.push('\n');
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    /// An int literal in a value position: the only thing a literal edit
    /// changes. Indices, moduli and loop bounds are written directly.
    fn lit(&mut self) -> String {
        let v = self.rng.below(10);
        let n = self.literals;
        self.literals += 1;
        if self.edit.literals.contains(&n) { (v + 11).to_string() } else { v.to_string() }
    }

    fn pick(&mut self, kind: Kind, fixed_only: bool) -> Option<Name> {
        let of: Vec<&Name> = self
            .names
            .iter()
            .filter(|n| n.kind == kind && (n.fixed || !fixed_only))
            .collect();
        if of.is_empty() {
            return None;
        }
        Some(of[self.rng.below(of.len() as u64) as usize].clone())
    }

    /// An int expression. With `fixed`, one that reads no dynamic data.
    fn int(&mut self, fixed: bool, depth: u64) -> String {
        let top = if depth == 0 { 3 } else { 9 };
        match self.rng.below(top) {
            0 => self.lit(),
            1 if !fixed => "n".into(),
            1 | 2 => match self.pick(Kind::Int, fixed) {
                Some(k) => k.name,
                None => self.lit(),
            },
            3 => match self.pick(Kind::Ints, fixed) {
                Some(t) => format!("{}[{}]", t.name, self.rng.below(3)),
                None => self.lit(),
            },
            4 => match self.pick(Kind::Grid, fixed) {
                Some(g) => format!("{}[{}][{}]", g.name, self.rng.below(2), self.rng.below(3)),
                None => self.lit(),
            },
            5 => match self.pick(Kind::Rec, fixed) {
                Some(r) => format!("{}.{}", r.name, ["a", "b", "x"][self.rng.below(3) as usize]),
                None => self.lit(),
            },
            6 => match self.pick(Kind::Recs, fixed) {
                Some(l) => format!("{}[{}].x", l.name, self.rng.below(2)),
                None => self.lit(),
            },
            7 => match self.pick(Kind::Ints, fixed) {
                Some(t) => match self.rng.below(3) {
                    0 => format!("total({})", t.name),
                    1 => format!("len({})", t.name),
                    _ => format!("sum({}) % 97", t.name),
                },
                None => self.lit(),
            },
            _ => {
                let (a, b) = (self.int(fixed, depth - 1), self.int(fixed, depth - 1));
                format!("({a} + {b}) % 97")
            }
        }
    }

    fn ints(&mut self, fixed: bool) -> String {
        match self.rng.below(10) {
            0..=2 => {
                let n = 3 + self.rng.below(3);
                let items: Vec<String> = (0..n).map(|_| self.int(fixed, 1)).collect();
                format!("[{}]", items.join(", "))
            }
            3 => match self.pick(Kind::Ints, fixed) {
                Some(t) => format!("scale({}, {})", t.name, self.int(fixed, 0)),
                None => format!("scale([{}, {}, 3], 2)", self.lit(), self.lit()),
            },
            4 => match (self.pick(Kind::Ints, fixed), self.pick(Kind::Int, fixed)) {
                (Some(t), Some(k)) => format!("map({}, fn(x) (x + {}) % 97 end)", t.name, k.name),
                (Some(t), None) => format!("map({}, fn(x) (x * 3) % 97 end)", t.name),
                _ => format!("[{}, 1, 2]", self.lit()),
            },
            5 => match self.pick(Kind::Ints, fixed) {
                Some(t) => format!("for x in {} do (x + {}) % 97 end", t.name, self.int(fixed, 0)),
                None => format!("for i in range(0, 4) do (i * {}) % 97 end", self.lit()),
            },
            6 => match (self.pick(Kind::Ints, fixed), self.pick(Kind::Ints, fixed)) {
                (Some(a), Some(b)) => format!("concat(sort({}), reverse({}))", a.name, b.name),
                _ => format!("sort([{}, {}, {}])", self.lit(), self.lit(), self.lit()),
            },
            // An element of a table that may be kept: the same heap object
            // as the row inside it.
            7 => match self.pick(Kind::Grid, fixed) {
                Some(g) => format!("{}[{}]", g.name, self.rng.below(2)),
                None => format!("[{}, 0, 1]", self.lit()),
            },
            8 => match self.pick(Kind::Ints, fixed) {
                Some(t) => format!("if {} % 2 == 0 then {} else slice({}, 0, 3) end", self.int(fixed, 0), t.name, t.name),
                None => format!("[1, {}, 3]", self.lit()),
            },
            _ if self.has_enum => format!("En.all[{}].xs", self.rng.below(2)),
            _ => format!("[{}, {}, {}]", self.lit(), self.lit(), self.lit()),
        }
    }

    fn rec(&mut self, fixed: bool) -> String {
        match self.rng.below(4) {
            0 => format!("mk({}, {})", self.int(fixed, 1), self.int(fixed, 0)),
            1 => match self.pick(Kind::Rec, fixed) {
                Some(r) => format!("{{...{}, a: {}}}", r.name, self.int(fixed, 0)),
                None => format!("mk({}, 1)", self.lit()),
            },
            2 => match self.pick(Kind::Recs, fixed) {
                Some(l) => format!("{}[{}]", l.name, self.rng.below(2)),
                None => format!("{{a: {}, b: 2, x: 3}}", self.lit()),
            },
            _ => format!("{{a: {}, b: {}, x: {}}}", self.int(fixed, 1), self.int(fixed, 0), self.lit()),
        }
    }

    fn value(&mut self, kind: Kind, fixed: bool) -> String {
        match kind {
            Kind::Int => self.int(fixed, 2),
            Kind::Ints => self.ints(fixed),
            Kind::Grid => match self.rng.below(3) {
                0 => format!("rows(2, {})", self.int(fixed, 0)),
                1 => match self.pick(Kind::Ints, fixed) {
                    Some(t) => format!("[{}, slice({}, 0, 3)]", self.ints(fixed), t.name),
                    None => format!("[[{}, 1, 2], [3, 4, 5]]", self.lit()),
                },
                _ => {
                    let cells: Vec<String> = (0..6).map(|_| self.int(fixed, 0)).collect();
                    format!("[[{}], [{}]]", cells[..3].join(", "), cells[3..].join(", "))
                }
            },
            Kind::Rec => self.rec(fixed),
            Kind::Recs => {
                let n = 2 + self.rng.below(2);
                let items: Vec<String> = (0..n).map(|_| self.rec(fixed)).collect();
                format!("[{}]", items.join(", "))
            }
        }
    }

    fn any_kind(&mut self) -> Kind {
        [Kind::Int, Kind::Ints, Kind::Ints, Kind::Grid, Kind::Rec, Kind::Recs][self.rng.below(6) as usize]
    }

    fn declare(&mut self, name: &str, kind: Kind, fixed: bool) {
        self.names.push(Name { name: name.to_string(), kind, fixed });
    }

    /// `let m = <something that may be kept>`, then a write through `m`, and
    /// a print of both sides.
    fn write_through(&mut self) {
        let kind = [Kind::Ints, Kind::Ints, Kind::Grid, Kind::Rec, Kind::Recs][self.rng.below(5) as usize];
        // The value written through: a name, or a static expression written
        // where it is used, which is a kept value nothing else names. That
        // is the one a lowering would most like to edit in place.
        let s = if self.rng.chance(40) {
            format!("({})", self.value(kind, true))
        } else {
            match self.pick(kind, false) {
                Some(src) => src.name,
                None => return,
            }
        };
        let m = self.fresh("m");
        let v = self.int(false, 1);
        match (kind, self.rng.below(5)) {
            // An alias, written in place if anything is.
            (Kind::Ints, 0) => {
                emit!(self, "let {m} = {s}");
                emit!(self, "{m}[{}] = {v}", self.rng.below(3));
            }
            (Kind::Ints, 1) => emit!(self, "let {m} = bump({s}, {v})"),
            (Kind::Ints, 2) => emit!(self, "let {m} = append({s}, {v})"),
            (Kind::Ints, 3) => {
                emit!(self, "var {m} = {s}");
                emit!(self, "set {m}[{}] = {v}", self.rng.below(3));
                if self.rng.chance(50) {
                    emit!(self, "set {m} = append(get {m}, {})", self.lit());
                }
                emit!(self, "print(\"{m}\", get {m}, {s})");
                return;
            }
            // An accumulator that grows from the kept list, a different
            // number of times each run.
            (Kind::Ints, 4) if self.rng.chance(50) => {
                emit!(self, "let {m} = {s}");
                emit!(self, "for i in range(0, n % 3) do\n  {m} = append({m}, i)\nend");
            }
            // A loop that rewrites a list starting from the kept one.
            (Kind::Ints, _) => {
                emit!(self, "let {m} = {s}");
                emit!(self, "for i in range(0, 3) do\n  {m}[i] = ({m}[i] + {v}) % 97\nend");
            }
            (Kind::Grid, 0) => {
                emit!(self, "let {m} = {s}");
                emit!(self, "{m}[{}][{}] = {v}", self.rng.below(2), self.rng.below(3));
            }
            // A row taken out of a grid, handed to a function that writes it.
            (Kind::Grid, 1) => {
                emit!(self, "let {m}r = {s}[{}]", self.rng.below(2));
                emit!(self, "let {m} = [bump({m}r, {v}), {m}r]");
            }
            (Kind::Grid, 2) => {
                emit!(self, "var {m} = {s}");
                emit!(self, "set {m}[{}][{}] = {v}", self.rng.below(2), self.rng.below(3));
                emit!(self, "print(\"{m}\", get {m}, {s})");
                return;
            }
            (Kind::Grid, 3) => emit!(self, "let {m} = append({s}, [{v}, 0, 1])"),
            (Kind::Grid, _) => {
                emit!(self, "let {m} = {s}");
                emit!(self, "{m}[{}] = [{v}, 0, 1]", self.rng.below(2));
            }
            (Kind::Rec, 0) => {
                emit!(self, "let {m} = {s}");
                emit!(self, "{m}.{} = {v}", ["a", "b", "x"][self.rng.below(3) as usize]);
            }
            (Kind::Rec, 1) => {
                emit!(self, "var {m} = {s}");
                emit!(self, "set {m}.a = {v}");
                emit!(self, "print(\"{m}\", get {m}, {s})");
                return;
            }
            (Kind::Rec, _) => emit!(self, "let {m} = poke({s}, {v})"),
            (Kind::Recs, 0) => emit!(self, "let {m} = [poke({s}[{}], {v}), {s}[0]]", self.rng.below(2)),
            (Kind::Recs, 1) => {
                emit!(self, "let {m} = {s}");
                emit!(self, "{m}[{}].x = {v}", self.rng.below(2));
            }
            (Kind::Recs, _) => {
                emit!(self, "let {m} = append({s}, mk({v}, 1))");
            }
            (Kind::Int, _) => return,
        }
        emit!(self, "print(\"{m}\", {m}, {s})");
        // The result is the same kind as its source, and dynamic.
        self.declare(&m, kind, false);
    }

    /// One top-level statement (which may be several lines).
    fn statement(&mut self) {
        match self.rng.below(20) {
            // Static declarations.
            0..=6 => {
                let kind = self.any_kind();
                let value = self.value(kind, true);
                let name = self.fresh(match kind {
                    Kind::Int => "K",
                    Kind::Ints => "T",
                    Kind::Grid => "G",
                    Kind::Rec => "R",
                    Kind::Recs => "L",
                });
                let how = match self.rng.below(6) {
                    0 | 1 => "const",
                    _ => "let",
                };
                emit!(self, "{how} {name} = {value}");
                self.declare(&name, kind, true);
            }
            7 => {
                let name = self.fresh("C");
                let v = self.lit();
                emit!(self, "config let {name} = {v}");
                self.declare(&name, Kind::Int, true);
            }
            // Dynamic declarations of each kind.
            8 | 9 => {
                let kind = self.any_kind();
                let value = self.value(kind, false);
                let name = self.fresh("d");
                // Make sure it really moves, whatever the picks were.
                let value = match kind {
                    Kind::Int => format!("({value} + n) % 97"),
                    _ => value,
                };
                emit!(self, "let {name} = {value}");
                self.declare(&name, kind, false);
            }
            10..=12 => self.write_through(),
            // A `state` cell seeded from a table that may be kept, and
            // written every run. It keeps its value across a reload.
            13 => {
                let Some(t) = self.pick(Kind::Ints, false) else { return };
                let s = self.fresh("s");
                emit!(self, "state {s} = {}", t.name);
                match self.rng.below(3) {
                    0 => emit!(self, "{s}[{}] = n", self.rng.below(3)),
                    1 => emit!(self, "{s} = bump({s}, ({s}[0] + 1) % 97)"),
                    _ => {}
                }
                emit!(self, "print(\"{s}\", {s}, {})", t.name);
                self.declare(&s, Kind::Ints, false);
            }
            // An accumulator filled from static data at top level: static
            // inputs, but bound again, so never one kept value.
            14 => {
                let Some(t) = self.pick(Kind::Ints, true) else { return };
                let acc = self.fresh("acc");
                emit!(self, "let {acc} = [0, {}, 0]", self.lit());
                emit!(self, 
                    "for x in {} do\n  {acc}[x % 3] = ({acc}[x % 3] + x) % 97\nend",
                    t.name
                );
                self.declare(&acc, Kind::Ints, false);
            }
            // A loop that builds a table in its body from the loop variable.
            15 => {
                let k = self.int(true, 0);
                let d = self.int(false, 0);
                let row = self.fresh("row");
                emit!(self, 
                    "for i in range(0, 3) do\n  let {row} = [i, {k}, i * 2]\n  let {row}b = bump({row}, {d})\n  print(\"{row}\", {row}, {row}b)\nend"
                );
            }
            // A static statement that must run on every run.
            16 => match self.pick(Kind::Ints, true) {
                Some(t) => emit!(self, "assert(len({}) >= 3)", t.name),
                None => self.line("assert(1 + 1 == 2)"),
            },
            // A dynamic branch over static tables.
            17 => {
                let Some(t) = self.pick(Kind::Ints, false) else { return };
                let z = self.fresh("z");
                emit!(self, 
                    "if (n + {}) % 2 == 0 then\n  let {z} = [{}, n, {}[1]]\n  print(\"{z}\", {z})\nelse\n  print(\"{z}\", total({}))\nend",
                    self.lit(),
                    self.lit(),
                    t.name,
                    t.name
                );
            }
            18 => {
                let v = self.int(false, 2);
                emit!(self, "print(\"v\", {v}, tick())");
            }
            // Reads of enum data, by name and through a variant picked at
            // run time.
            _ if self.has_enum => {
                let k = self.fresh("e");
                emit!(self, "let {k} = En.all[n % 2]");
                emit!(self, "print(\"{k}\", {k}, {k}.w, {k}.xs, En.a.w + En.b.xs[1], Dyn.on.w)");
            }
            _ => {
                let v = self.int(true, 2);
                emit!(self, "print(\"k\", {v})");
            }
        }
    }

    fn program(mut self) -> (String, usize, usize) {
        self.src.push_str(PRELUDE);
        // Enum data is evaluated ahead of the file's statements: literals
        // and functions only.
        if self.rng.chance(60) {
            self.has_enum = true;
            let (a, b, c, d) = (self.lit(), self.lit(), self.lit(), self.lit());
            emit!(self, 
                "enum En\n  a(w: {a}, xs: [{b}, 2, 3]),\n  b(w: {c}, xs: scale([1, {d}, 3], 2)),\nend"
            );
            let e = self.lit();
            emit!(self, "enum Dyn\n  off(w: {e}),\n  on(w: tick()),\nend");
        }
        self.line("state n = 0");
        self.line("n = n + 1");
        let count = 10 + self.rng.below(14) as usize;
        for i in 0..count {
            self.statement();
            self.statements += 1;
            if self.edit.extra_after == Some(i) {
                self.extra();
            }
        }
        let mut all: Vec<String> = self.names.iter().map(|n| n.name.clone()).collect();
        all.extend(self.extras.iter().cloned());
        for chunk in all.chunks(8) {
            emit!(self, "print(\"end\", {})", chunk.join(", "));
        }
        (self.src, self.literals, self.statements)
    }

    /// The statement a structural edit adds: a static table and a write
    /// through it, built with its own random stream and names so the rest
    /// of the program is the text it was.
    fn extra(&mut self) {
        let saved = (std::mem::replace(&mut self.rng, Rng::new(0xE17)), self.next_id, self.literals);
        self.next_id = 9000;
        let known = self.names.len();
        let value = self.ints(true);
        emit!(self, "let X1 = {value}");
        self.line("let X2 = for x in X1 do [x, (x * 7) % 97] end");
        self.line("let X3 = bump(X1, n)");
        self.extras.extend(["X1", "X2", "X3"].map(String::from));
        self.names.truncate(known);
        (self.rng, self.next_id, self.literals) = saved;
    }
}

/// The three texts of a seed: the plain program, the same with a third of
/// its literals changed, and that with one statement added.
pub(super) fn versions(seed: u64) -> [String; 3] {
    let (plain, literals, statements) = Gen::new(seed, Edit::default()).program();
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
    let (patched, _, _) = Gen::new(seed, edit.clone()).program();
    edit.extra_after = Some(rng.below(statements.max(1) as u64) as usize);
    let (grown, _, _) = Gen::new(seed, edit).program();
    [plain, patched, grown]
}

/// What driving a seed's three versions did, line by line, and what the
/// cache did meanwhile.
struct Driven {
    transcript: Vec<String>,
    skips: u64,
    clears: u64,
    slots: usize,
    /// How each reload was applied.
    reloads: Vec<String>,
    errors: usize,
}

fn drive(texts: &[String; 3], policy: RunPolicy) -> Driven {
    let mut env = Env::new();
    env.set_policy(policy);
    env.set_echo(false);
    let mut out = Driven {
        transcript: Vec::new(),
        skips: 0,
        clears: 0,
        slots: 0,
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
    // Patched, recompiled, and back to the first text (a recompile that
    // takes the added statement away and restores the literals).
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
        out.clears = values.stats.clears;
        out.slots = values.slots.len();
    }
    out
}

/// The policies the cache runs under, each as a pair that differs in the
/// cache alone.
const POLICIES: [&str; 4] = ["replay", "replay-memo", "fast", "baseline"];

#[track_caller]
fn assert_parity(seed: u64) -> Driven {
    let texts = versions(seed);
    let mut first = None;
    for name in POLICIES {
        let base = RunPolicy::parse(name).unwrap();
        let on = drive(&texts, base.with_static_cache(true));
        let off = drive(&texts, base.with_static_cache(false));
        assert_eq!(off.slots, 0, "the cache is off");
        if on.transcript != off.transcript {
            let at = on
                .transcript
                .iter()
                .zip(&off.transcript)
                .position(|(a, b)| a != b)
                .unwrap_or(on.transcript.len().min(off.transcript.len()));
            panic!(
                "static-cache divergence at seed {seed} under {name}, line {at} of the transcript:\n  \
                 on:  {:?}\n  off: {:?}\nreproduce with static_fuzz::versions({seed})\n\
                 --- program ---\n{}\n--- with literals changed ---\n{}\n--- with a statement added ---\n{}",
                on.transcript.get(at),
                off.transcript.get(at),
                texts[0],
                texts[1],
                texts[2],
            );
        }
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
fn static_differential_fuzz() {
    for seed in 0..iters() {
        assert_parity(seed);
    }
}

/// The soak proves nothing unless the programs compile, run, keep values,
/// and are reloaded both ways. Guards the generator against drifting.
#[test]
fn the_generator_reaches_the_static_cache() {
    let (mut clean, mut kept, mut patched, mut recompiled, mut consts, mut enums) = (0, 0, 0, 0, 0, 0);
    for seed in 0..60 {
        let texts = versions(seed);
        let d = drive(&texts, RunPolicy::REPLAY);
        assert!(
            !d.transcript[0].starts_with("compile error"),
            "seed {seed} does not compile: {}\n{}",
            d.transcript[0],
            texts[0]
        );
        clean += (d.errors == 0) as u32;
        kept += (d.skips >= 6 && d.clears >= 3) as u32;
        patched += (d.reloads.first().map(String::as_str) == Some("Patched")) as u32;
        recompiled += (d.reloads.get(1).map(String::as_str) == Some("Recompiled")) as u32;
        consts += texts[0].contains("\nconst ") as u32;
        enums += texts[0].contains("enum En") as u32;
    }
    assert!(clean >= 54, "only {clean}/60 generated programs run without an error");
    assert!(kept >= 50, "only {kept}/60 kept static values before and after every reload");
    assert!(patched >= 50, "only {patched}/60 literal edits were patched in place");
    assert!(recompiled >= 54, "only {recompiled}/60 added statements were a recompile");
    assert!(consts >= 30, "only {consts}/60 programs declare a const");
    assert!(enums >= 20, "only {enums}/60 programs declare enums with data");
}
