import { describe, it, expect } from "vitest";
import { spawnSync } from "child_process";
import {
  PETAL,
  petalCapture,
  runPetal,
  runPetalError,
  checkJson,
  checkJsonAllowFail,
  checkStrict,
  showAstJson,
  showBytecodeText,
  showIrJsonRaw,
  runIr,
} from "./helpers";

// `match` as the control-flow statement, through the CLI. See
// docs/language-guide.md (Pattern Matching): statement-list arm bodies,
// or-patterns, the exhaustiveness error, the unreachable-arm warning, the
// no-arm-matched runtime error, and the jump table literal arms compile to.

const KIND = "enum Kind\n  park,\n  plaza,\n  road(lanes),\nend\n";
const CMD = "enum Cmd\n  quit,\n  move(dx, dy),\n  say(text),\nend\n";

/** stdout of `petal run`, with the optimizer on and with it off: they must agree. */
function runBoth(code: string): string {
  const fast = runPetal(code);
  const baseline = petalCapture(["run", "--no-opt", "-e", code]);
  expect(baseline.code, baseline.stderr).toBe(0);
  expect(baseline.stdout.trim()).toBe(fast);
  return fast;
}

const lines = (code: string) => runBoth(code).split("\n");
const warningsOf = (code: string): string[] =>
  checkJson(code).warnings.map((w: any) => w.message as string);

describe("match arm bodies", () => {
  it("runs a statement list after `->`, and its last expression is the arm's value", () => {
    const code = `${CMD}
var x = 0
var y = 0
fn step(cmd)
  match cmd
    when Cmd.quit -> "bye"
    when Cmd.move(dx, dy) ->
      set x += dx
      set y += dy
      "at {get x},{get y}"
    when Cmd.say(text) ->
      let loud = text ++ "!"
      loud
  end
end
print(step(Cmd.move(1, 2)))
print(step(Cmd.move(10, 20)))
print(step(Cmd.say("hi")))
print(step(Cmd.quit))`;
    expect(lines(code)).toEqual(["at 1,2", "at 11,22", "hi!", "bye"]);
  });

  it("parses an arm body as a statement list", () => {
    const ast = showAstJson('match x\n  when 1 -> "one"\n  when 2 ->\n    let a = 1\n    a\nend');
    const arms = ast[0].kind.Expr.kind.Match.arms;
    expect(arms[0].body).toHaveLength(1);
    expect(arms[1].body).toHaveLength(2);
  });

  it("accepts arms on one line, bodies on the next line, and `do … end` arms", () => {
    const code = `let a = match 2 when 1 -> "one" when 2 -> "two" when _ -> "many" end
let b = match 2
  when 1 ->
    "one"
  when 2 do
    let t = "tw"
    t ++ "o"
  end
  when _ -> "many"
end
print(a, b)`;
    expect(runBoth(code)).toBe("two two");
  });

  it("refuses an arm with nothing after the arrow", () => {
    const err = runPetalError("match 1\n  when 1 ->\n  when _ -> 2\nend");
    expect(err).toMatch(/Expected the arm's body after '->'/);
    expect(err).toMatch(/line 3, column 3/);
  });

  it("works in statement position and in value position", () => {
    const code = `fn describe(n)
  match n
    when 0 ->
      print("zero")
      print("nothing")
    when _ -> print("some")
  end
  let size = match n
    when 0 | 1 -> "small"
    when _ ->
      let big = n > 100
      if big then "huge" else "medium" end
  end
  size
end
print(describe(0))
print(describe(500))`;
    expect(lines(code)).toEqual(["zero", "nothing", "small", "some", "huge"]);
  });

  it("nests a match inside an arm body", () => {
    const code = `${KIND}
fn f(k, n)
  match k
    when Kind.park ->
      let size = match n
        when 1 -> "small"
        when _ ->
          let m = n * 2
          "size {m}"
      end
      "park, " ++ size
    when Kind.plaza -> "plaza"
    when Kind.road(lanes) ->
      match lanes
        when 1 | 2 -> "lane"
        when _ -> "highway"
      end
  end
end
print(f(Kind.park, 1))
print(f(Kind.park, 4))
print(f(Kind.road(6), 0))`;
    expect(lines(code)).toEqual(["park, small", "park, size 8", "highway"]);
  });

  it("lets break, continue and return leave from an arm, as from an if branch", () => {
    const body = (form: "match" | "if") =>
      form === "match"
        ? `    match item
      when "skip" -> continue
      when "stop" ->
        set seen = push(get seen, "stopped")
        break
      when "bail" -> return "bailed with {len(get seen)}"
      when _ -> set seen = push(get seen, item)
    end`
        : `    if item == "skip" then
      continue
    elsif item == "stop" then
      set seen = push(get seen, "stopped")
      break
    elsif item == "bail" then
      return "bailed with {len(get seen)}"
    else
      set seen = push(get seen, item)
    end`;
    const program = (form: "match" | "if") => `fn scan(items)
  var seen = []
  for item in items do
${body(form)}
    set seen = push(get seen, "|")
  end
  join(get seen, "")
end
print(scan(["a", "skip", "b", "stop", "c"]))
print(scan(["a", "bail", "b"]))`;
    const expected = ["a|b|stopped", "bailed with 2"];
    expect(lines(program("match"))).toEqual(expected);
    expect(lines(program("if"))).toEqual(expected);
  });

  it("is laid out by `petal fmt`: a body under its arm, the next `when` back out", () => {
    const messy = `fn f(k, n)
match k
when 1 ->
print(n)
if n > 1 then
print("big")
end
when 2
| 3 ->
match n
when 0 -> "zero"
when _ ->
let m = n
m
end
when _ -> nil
end
end
`;
    const r = petalCapture(["fmt", "-"], messy);
    expect(r.code, r.stderr).toBe(0);
    expect(r.stdout).toBe(`fn f(k, n)
  match k
    when 1 ->
      print(n)
      if n > 1 then
        print("big")
      end
    when 2
    | 3 ->
      match n
        when 0 -> "zero"
        when _ ->
          let m = n
          m
      end
    when _ -> nil
  end
end
`);
    // Formatting is stable, and the formatted program still parses.
    expect(petalCapture(["fmt", "-"], r.stdout).stdout).toBe(r.stdout);
    expect(petalCapture(["check", "-e", r.stdout]).code).toBe(0);
  });
});

describe("or-patterns", () => {
  it("matches any alternative: literals, variants, and across lines", () => {
    const code = `${KIND}
fn shade(k)
  match k
    when Kind.park | Kind.plaza -> "open"
    when Kind.road(1) | Kind.road(2) -> "street"
    when Kind.road(_) -> "highway"
  end
end
fn vowel(s)
  match s
    when "a" | "e"
       | "i" | "o" | "u" -> true
    when _ -> false
  end
end
print(shade(Kind.park), shade(Kind.plaza), shade(Kind.road(2)), shade(Kind.road(8)))
print(vowel("i"), vowel("x"))`;
    expect(lines(code)).toEqual(["open open street highway", "true false"]);
  });

  it("parses to an `Or` of its alternatives", () => {
    const ast = showAstJson('match x\n  when "a" | "b" | "c" -> 1\n  when _ -> 2\nend');
    const pattern = ast[0].kind.Expr.kind.Match.arms[0].pattern;
    expect(pattern.Or).toHaveLength(3);
    expect(pattern.Or[1]).toEqual({ Literal: { String: "b" } });
  });

  it("binds the same names in every alternative, nested ones included", () => {
    const code = `enum Op
  fight(place, won),
  truce(place),
  gift(amount),
end
fn place_of(op)
  match op
    when Op.fight(place, _) | Op.truce(place) -> place
    when Op.gift(_) -> "nowhere"
  end
end
fn classify(v)
  match v
    when [0 | 1, rest] -> "bit then {rest}"
    when Op.fight("north" | "south", won) -> "pole, won={won}"
    when Op.gift(n) | Op.truce(n) if n == 5 -> "five"
    when _ -> "other"
  end
end
print(place_of(Op.fight("east", true)), place_of(Op.truce("west")))
print(classify([1, "x"]), "/", classify([2, "x"]))
print(classify(Op.fight("south", false)), "/", classify(Op.fight("east", false)))
print(classify(Op.gift(5)), classify(Op.truce(5)), classify(Op.gift(6)))`;
    expect(lines(code)).toEqual([
      "east west",
      "bit then x / other",
      "pole, won=false / other",
      "five five other",
    ]);
  });

  it("is a compile error when the alternatives bind different names", () => {
    const code =
      "enum Op\n  fight(place, won),\n  truce(place),\nend\n" +
      "fn f(op)\n  match op\n    when Op.fight(place, won) | Op.truce(place) -> place\n  end\nend";
    const err = runPetalError(code);
    expect(err).toMatch(/every alternative of an or-pattern must bind the same names/);
    expect(err).toMatch(/`Op.fight\(place, won\)` binds `won` and `Op.truce\(place\)` does not/);
    expect(err).toMatch(/line 7, column 10/);
    expect(checkJsonAllowFail(code)).toMatchObject({ error: true, phase: "compile" });
  });

  it("survives the IR round trip", () => {
    const code = 'fn f(x)\n  match x\n    when 1 | 2 -> "a"\n    when "x" | "y" -> "b"\n    when _ -> "c"\n  end\nend\nprint(f(2), f("y"), f(9))';
    expect(runIr(showIrJsonRaw(code))).toBe("a b c");
  });
});

describe("match exhaustiveness", () => {
  it("is a compile error to leave out a variant, naming the missing ones", () => {
    const code = `${KIND}fn f(k)\n  match k\n    when Kind.park -> 1\n  end\nend`;
    const err = runPetalError(code);
    expect(err).toMatch(/this match does not cover every variant of `Kind`/);
    expect(err).toMatch(/no arm for `Kind.plaza` and `Kind.road`/);
    expect(err).toMatch(/line 7, column 3/);
    // `check` reports it as an error, not a warning.
    const checked = checkJsonAllowFail(code);
    expect(checked).toMatchObject({ error: true, phase: "compile", line: 7, column: 3 });
  });

  it("accepts a match that covers every variant, in any grouping", () => {
    const code = `${KIND}
fn f(k)
  match k
    when Kind.road(n) -> n
    when Kind.plaza | Kind.park -> 0
  end
end
print(f(Kind.road(3)), f(Kind.park))`;
    expect(runBoth(code)).toBe("3 0");
    expect(warningsOf(code)).toEqual([]);
  });

  it("does not count a guarded arm, or a payload pattern that can fail", () => {
    const guarded = `${KIND}fn f(k, on)\n  match k\n    when Kind.park if on -> 1\n    when Kind.plaza -> 2\n    when Kind.road(_) -> 3\n  end\nend`;
    expect(runPetalError(guarded)).toMatch(/no arm for `Kind.park` — an arm with an `if` guard/);
    const partial = `${KIND}fn f(k)\n  match k\n    when Kind.park | Kind.plaza -> 0\n    when Kind.road(1) -> 1\n  end\nend`;
    expect(runPetalError(partial)).toMatch(/no arm for `Kind.road`/);
    // A payload of bindings and wildcards cannot fail, so it covers.
    const whole = `${KIND}fn f(k)\n  match k\n    when Kind.park | Kind.plaza -> 0\n    when Kind.road(1) -> 1\n    when Kind.road(n) -> n\n  end\nend\nprint(f(Kind.road(5)))`;
    expect(runBoth(whole)).toBe("5");
  });

  it("is satisfied by an arm for everything else", () => {
    for (const rest of ["when _ -> 9", "when other -> 9"]) {
      const code = `${KIND}fn f(k)\n  match k\n    when Kind.park -> 1\n    ${rest}\n  end\nend\nprint(f(Kind.park), f(Kind.road(2)))`;
      expect(runBoth(code)).toBe("1 9");
    }
  });

  it("checks only a match whose arms are all variants of one enum", () => {
    const code = `${KIND}enum Use
  green,
  paved,
end
fn f(v)
  match v
    when Kind.park -> "park"
    when Use.green -> "green"
  end
end
fn g(v)
  match v
    when Kind.park -> "park"
    when "park" -> "a string"
  end
end
print(f(Use.green), g("park"))`;
    expect(runBoth(code)).toBe("green a string");
  });
});

describe("unreachable match arms", () => {
  it("warns on an arm after one that matches every value", () => {
    const code = 'fn f(x)\n  match x\n    when 1 -> "a"\n    when _ -> "b"\n    when 2 -> "c"\n  end\nend';
    const checked = checkJson(code);
    expect(checked.ok).toBe(true);
    const w = checked.warnings.filter((d: any) => /can never run/.test(d.message));
    expect(w).toHaveLength(1);
    expect(w[0].message).toMatch(/an earlier arm of the match already matches every value/);
    expect(w[0].severity).toBe("warning");
    expect([w[0].line, w[0].column]).toEqual([5, 10]);
    // A warning: the program runs, and `--strict` turns it into a failure.
    expect(runBoth(`${code}\nprint(f(2))`)).toBe("b");
    expect(checkStrict(code).code).not.toBe(0);
  });

  it("warns on a repeated literal and a repeated variant", () => {
    const literal = 'fn f(x)\n  match x\n    when "a" -> 1\n    when "b" -> 2\n    when "a" -> 3\n    when _ -> 4\n  end\nend';
    expect(warningsOf(literal)).toEqual([
      'this arm can never run: an earlier arm already matches `"a"`',
    ]);
    const variant = `${KIND}fn f(k)\n  match k\n    when Kind.park | Kind.plaza -> 1\n    when Kind.road(n) -> n\n    when Kind.plaza -> 2\n  end\nend`;
    expect(warningsOf(variant)).toEqual([
      "this arm can never run: an earlier arm already matches `Kind.plaza`",
    ]);
  });

  it("warns on one repeated alternative of an or-pattern", () => {
    const code = 'fn f(x)\n  match x\n    when "a" | "b" -> 1\n    when "b" | "c" -> 2\n    when _ -> 3\n  end\nend';
    expect(warningsOf(code)).toEqual([
      '`"b"` can never match here: an earlier pattern already matches it',
    ]);
  });

  it("stays quiet when a guard or a refutable payload leaves the later arm reachable", () => {
    const code = `${KIND}fn f(k, on)
  match k
    when Kind.park if on -> 1
    when Kind.park -> 2
    when Kind.road(1) -> 3
    when Kind.road(n) -> n
    when Kind.plaza -> 4
  end
end`;
    expect(warningsOf(code)).toEqual([]);
  });
});

describe("a match no arm of which matched", () => {
  it("is a runtime error naming the value and the place, in value position", () => {
    const err = runPetalError('let x = 3\nlet r = match x\n  when 1 -> "one"\n  when 2 -> "two"\nend\nprint(r)');
    expect(err).toMatch(/No matching pattern for value: 3/);
    expect(err).toMatch(/line 2, column 9/);
  });

  it("is the same error in statement position, and with the optimizer off", () => {
    const code = 'fn f(s)\n  match s\n    when "a" -> print("a")\n    when "b" -> print("b")\n  end\n  print("after")\nend\nf("zebra")';
    for (const flags of [[], ["--no-opt"]]) {
      const r = petalCapture(["run", ...flags, "-e", code]);
      expect(r.code).not.toBe(0);
      expect(r.stdout).toBe("");
      expect(r.stderr).toMatch(/No matching pattern for value: "zebra"/);
      expect(r.stderr).toMatch(/line 2, column 3/);
    }
  });

  it("yields nil only when the fallback arm says so", () => {
    expect(runBoth('print(match 3\n  when 1 -> "one"\n  when _ -> nil\nend)')).toBe("nil");
  });
});

describe("literal dispatch", () => {
  const pick = (prelude: string, arms: string[], subjects: string[]) =>
    `${prelude}fn pick(x)\n  match x\n${arms.map((a) => `    when ${a}\n`).join("")}  end\nend\n` +
    `print([${subjects.map((s) => `pick(${s})`).join(", ")}])`;

  it("enters string, int and variant arms through a jump table", () => {
    const strings = pick("", ['"a" -> 1', '"b" -> 2', '"c" -> 3', "_ -> 0"], ['"b"', '"z"']);
    expect(showBytecodeText(strings)).toMatch(/match_switch r\d+ \["a" => \d+, "b" => \d+, "c" => \d+\]/);
    expect(runBoth(strings)).toBe("[2, 0]");

    const ints = pick("", ["10 -> 1", "20 -> 2", "30 | 40 -> 3", "_ -> 0"], ["40", "5"]);
    expect(showBytecodeText(ints)).toMatch(/match_switch r\d+ \[10 => \d+, 20 => \d+, 30 => \d+, 40 => \d+\]/);
    expect(runBoth(ints)).toBe("[3, 0]");

    const variants = pick(KIND, ["Kind.park -> 1", "Kind.plaza -> 2", "Kind.road(n) -> n"], [
      "Kind.plaza",
      "Kind.road(7)",
    ]);
    // A payload variant's case lands on its arm's own test (`->`), which binds.
    expect(showBytecodeText(variants)).toMatch(
      /match_switch r\d+ \[Kind\.park => \d+, Kind\.plaza => \d+, Kind\.road -> \d+\]/,
    );
    expect(runBoth(variants)).toBe("[2, 7]");
  });

  it("is off with the optimizer off, where the arms are tested in turn", () => {
    const code = pick("", ['"a" -> 1', '"b" -> 2', "_ -> 0"], ['"b"']);
    const r = spawnSync(PETAL, ["show-bytecode", "-e", code], {
      encoding: "utf-8",
      env: { ...process.env, PETAL_OPT: "off" },
    });
    expect(r.status, r.stderr).toBe(0);
    expect(r.stdout).not.toMatch(/match_switch/);
    expect(r.stdout).toMatch(/match_arm/);
    expect(runBoth(code)).toBe("[2]");
  });

  it("stays first-match-wins across duplicates, guards, floats and bindings", () => {
    const arms = [
      '"a" | "b" -> 1',
      "2 -> 2",
      '"a" -> 3',
      '"c" if false -> 4',
      '"c" -> 5',
      "3 if false -> 6",
      "2.5 -> 7",
      "3 -> 8",
      '"d" -> 9',
      "[_] -> 10",
      "n if n == 4 -> 11",
      "4 -> 12",
      '"e" -> 13',
      "_ -> 0",
    ];
    const subjects = ['"a"', '"b"', '"c"', '"d"', '"e"', '"zz"', "2", "2.0", "3", "3.0", "4", "2.5", "5", "nil", "[1]"];
    expect(runBoth(pick("", arms, subjects))).toBe("[1, 1, 5, 9, 13, 0, 2, 2, 8, 8, 11, 7, 0, 0, 10]");
  });

  it("stays first-match-wins across variants, payloads and look-alike strings", () => {
    const prelude = "enum K\n  a,\n  b,\n  c(n),\nend\nenum Other\n  a,\nend\n";
    const arms = [
      "K.a -> 1",
      "K.c(n) if n > 10 -> 2",
      "K.c(0) -> 3",
      '"K.a" -> 4',
      "K.a -> 5",
      "K.c(n) -> n",
      "Other.a -> 6",
      "K.b | 8 -> 8",
      "_ -> 0",
    ];
    const subjects = ["K.a", "K.b", "K.c(50)", "K.c(0)", "K.c(4)", "Other.a", '"K.a"', "8", '"K.b"', "nil"];
    expect(runBoth(pick(prelude, arms, subjects))).toBe("[1, 8, 2, 3, 4, 6, 4, 8, 0, 0]");
  });
});
