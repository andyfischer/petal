//! `fn_info(f)` / `fn_ast(f)` — function introspection (experimental; see
//! docs/function-introspection.md), and the `glsl` library built on it
//! (core-runtime/glsl, docs/petal-to-glsl.md).
//!
//! The library half checks the two things the feature promises: the GLSL it
//! writes for the eleven post effects of the original experiment is the text
//! the Python stand-in wrote (the files under core-runtime/glsl/tests), and a
//! script that calls it every frame translates once, then again only after an
//! edit that changes the function.

use std::path::PathBuf;

use petal::env::{Env, ReloadOutcome};
use petal::policy::RunPolicy;
use petal::program::ProgramId;
use petal::stack::StackKey;

fn run(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.set_echo(false);
    env.run_source(src)
        .unwrap_or_else(|e| panic!("run failed: {e}\n{src}"));
    env.take_output()
}

// ── fn_info / fn_ast ────────────────────────────────────────────────────────

#[test]
fn fn_info_reads_params_types_defaults_and_captures() {
    let out = run(r#"
let K = 3
fn helper(i: int, n: int) -> float
  1.0 - i / (n + 1.0)
end
let f = fn(fx: Fx, amount = 0.35, tint = #1a2a6c, dir: vec2 = vec2(1, 0.5), late = amount * 2)
  helper(K, 2) * amount
end
let i = fn_info(f)
print(i.name, i.line, i.column, i.returns)
for p in i.params do
  print(p.name, p.type, p.has_default, p.default, p.default_source)
end
for c in i.captures do
  print(c.name, type(c.value), if type(c.value) == "function" then fn_info(c.value).name else c.value end)
end
let h = fn_info(helper)
print(h.name, h.returns, h.params[1].name, h.params[1].type, len(h.captures))
"#);
    assert_eq!(
        out,
        [
            "nil 6 9 nil",
            "fx Fx false nil nil",
            "amount nil true 0.35 0.35",
            "tint nil true { b: 108, g: 42, r: 26 } #1a2a6c",
            "dir vec2 true vec2(1.0, 0.5) vec2(1, 0.5)",
            "late nil true nil amount * 2",
            // in order of first use in the body
            "helper function helper",
            "K int 3",
            "helper float n int 0",
        ]
    );
}

#[test]
fn fn_ast_is_plain_records_with_every_field_present() {
    let out = run(r#"
let f = fn(x: float, k = 2)
  var acc = 0.0
  for i in range(0, k) do
    set acc += if x > 0.5 then x elsif x > 0.1 then 0.5 else 0.0 end
  end
  if acc <= 0.0 then
    return x
  end
  acc
end
let a = fn_ast(f)
print(a.tag, a.name, a.returns, len(a.params), a.params[0].type, a.params[1].default.value, a.line)
let body = a.body
print(for s in body do s.tag end)
print(body[0].name, body[0].is_var, body[0].type, body[0].value.type)
let loop = body[1]
print(loop.var, loop.iter.function.name, len(loop.iter.args), loop.body[0].target.kind, loop.body[0].target.name)
let pick = loop.body[0].value.right
print(pick.tag, pick.condition.op, pick.else_body[0].expr.tag, pick.else_body[0].expr.else_body[0].expr.value)
print(body[2].expr.else_body, body[2].expr.then_body[0].tag, body[2].expr.then_body[0].value.name)
print(body[3].expr.tag, body[3].expr.name, body[3].line, body[3].column)
"#);
    assert_eq!(
        out,
        [
            "Lambda nil nil 2 float 2 2",
            "[\"Let\", \"For\", \"Expr\", \"Expr\"]",
            "acc true nil float",
            "i range 2 Name acc",
            "If Gt If 0.0",
            "nil Return x",
            "Ident acc 10 3",
        ]
    );
}

#[test]
fn what_cannot_be_inspected_is_nil_and_a_non_function_is_an_error() {
    let out = run(r#"
class P
  x: num,
end
fn two(a) a end
fn two(a, b) a + b end
print(fn_info(floor), fn_ast(floor), fn_info(two), fn_ast(P), fn_info(P).name, fn_info(P).code_hash)
"#);
    assert_eq!(out, ["nil nil nil nil P nil"]);
    let err = Env::new().run_source("fn_info(3)").expect_err("an int is not a function");
    assert!(err.contains("fn_info() expects a function, got int"), "{err}");
}

fn hash_of(source: &str) -> String {
    run(&format!("{source}\nprint(fn_info(f).code_hash)")).remove(0)
}

#[test]
fn code_hash_follows_the_code_and_nothing_else() {
    let base = "let K = 2.0\nfn h(x: float) -> float\n  x * 2.0\nend\nlet f = fn(a: float) -> h(a) * K";
    let same = [
        // comments and layout
        "// note\nlet K = 2.0\n\nfn h(x: float) -> float\n    x * 2.0 // twice\nend\n\nlet f = fn(a: float) ->   h(a) * K",
        // a captured value is data, not code
        "let K = 9.5\nfn h(x: float) -> float\n  x * 2.0\nend\nlet f = fn(a: float) -> h(a) * K",
        // a function added above (the function's index in the program moves)
        "fn other() 1 end\nlet K = 2.0\nfn h(x: float) -> float\n  x * 2.0\nend\nlet f = fn(a: float) -> h(a) * K",
    ];
    let different = [
        // the body
        "let K = 2.0\nfn h(x: float) -> float\n  x * 2.0\nend\nlet f = fn(a: float) -> h(a) + K",
        // an annotation
        "let K = 2.0\nfn h(x: float) -> float\n  x * 2.0\nend\nlet f = fn(a: int) -> h(a) * K",
        // a function it captures
        "let K = 2.0\nfn h(x: float) -> float\n  x * 3.0\nend\nlet f = fn(a: float) -> h(a) * K",
    ];
    let want = hash_of(base);
    assert_eq!(want.len(), 16);
    for s in same {
        assert_eq!(hash_of(s), want, "should hash the same:\n{s}");
    }
    for s in different {
        assert_ne!(hash_of(s), want, "should hash differently:\n{s}");
    }
}

// ── A host driving frames, with hot reload ──────────────────────────────────

struct World {
    env: Env,
    pid: ProgramId,
    sid: StackKey,
}

fn glsl_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../core-runtime/glsl")
}

fn world(entry: &str, policy: RunPolicy) -> World {
    let mut env = Env::new();
    env.set_policy(policy);
    env.set_echo(false);
    env.add_package(glsl_root()).expect("the glsl package loads");
    env.profile_mut().set_enabled(true);
    let pid = env.load_program(entry).unwrap_or_else(|e| panic!("{e}\n{entry}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, pid, sid }
}

fn frame(w: &mut World) -> Vec<String> {
    w.env.reset_stack(w.sid).unwrap();
    w.env.run(w.sid).unwrap_or_else(|e| panic!("frame failed: {e}"));
    w.env.take_output()
}

/// How many times a native has run on this env since profiling was enabled.
fn native_calls(w: &World, name: &str) -> u64 {
    w.env
        .profile()
        .natives_by_count()
        .into_iter()
        .find(|(id, _)| w.env.native_fn_name(*id) == name)
        .map_or(0, |(_, n)| n)
}

fn reload(w: &mut World, source: &str) -> ReloadOutcome {
    let _ = w.pid;
    w.env
        .reload_program(w.sid, source, None)
        .unwrap_or_else(|e| panic!("reload failed: {e}\n{source}"))
        .outcome
}

#[test]
fn positions_follow_a_layout_edit_and_a_patched_literal_changes_the_hash() {
    let old = "let f = fn(a: float) -> a * 2.0\nlet i = fn_info(f)\nprint(i.line, i.code_hash, fn_ast(f).body[0].expr.right.value)";
    let mut w = world(old, RunPolicy::FAST);
    let before = frame(&mut w).remove(0);
    assert!(before.starts_with("1 "), "{before}");

    // A comment line above: nothing recompiles, positions move, the hash stays.
    let moved = format!("// moved down\n{old}");
    assert_eq!(reload(&mut w, &moved), ReloadOutcome::Relocated);
    let after = frame(&mut w).remove(0);
    assert_eq!(after, before.replacen("1 ", "2 ", 1));

    // A literal inside the function changes in place: new hash, new AST.
    let patched = moved.replace("a * 2.0", "a * 2.5");
    assert_eq!(reload(&mut w, &patched), ReloadOutcome::Patched);
    let last = frame(&mut w).remove(0);
    assert!(last.starts_with("2 ") && last.ends_with(" 2.5"), "{last}");
    assert_ne!(last.split(' ').nth(1), after.split(' ').nth(1));
}

// ── The glsl library ────────────────────────────────────────────────────────

/// The post-effect environment the experiment's stand-in had built in.
const POST_ENV: &str = r#"
let ENV = {
  structs: {Fx: {color: "vec3", uv: "vec2", time: "float", resolution: "vec2", aspect: "float"}},
  functions: {
    src: {args: ["vec2"], returns: "vec3"},
    fx_luma: {args: ["vec3"], returns: "float"},
    fx_bloom: {args: ["vec2"], returns: "vec3"},
    scene_depth: {args: ["vec2"], returns: "float"},
    scene_color: {args: ["vec2"], returns: "vec3"},
    linear_to_srgb: {args: ["vec3"], returns: "vec3"},
    hash11: {args: ["float"], returns: "float"},
    hash12: {args: ["vec2"], returns: "float"},
    hash33: {args: ["vec3"], returns: "vec3"},
    fbm: {args: ["vec2", "int"], returns: "float"},
    voronoi: {args: ["vec2"], returns: "vec3"},
    noise: {args: ["vec2"], returns: "float"},
  },
  self: "fx", result: "color", uniforms: "params", count: "src", entry: "effect",
}
"#;

const EFFECTS: [&str; 11] = [
    "film_grain",
    "scanlines",
    "chromatic_aberration",
    "glitch",
    "sharpen",
    "color_grade",
    "duotone",
    "paper",
    "pixelate",
    "outline",
    "streak",
];

fn read(rel: &str) -> String {
    let path = glsl_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_eleven_effects_translate_to_the_stand_ins_glsl() {
    // One program: the experiment's effects.ptl, then a print per effect in
    // the layout of the stand-in's .glsl files. `inline_captures` is what the
    // stand-in did with a captured float (TAU in `scanlines`).
    let mut src = format!("import glsl\n{}{POST_ENV}", read("tests/effects.ptl"));
    for name in EFFECTS {
        src.push_str(&format!(
            r#"
let g_{name} = glsl.glsl_function({name}, ENV, {{name: "gen_{name}", inline_captures: true}})
print("=== {name} " ++ str(g_{name}.error) ++ " taps " ++ str(g_{name}.taps) ++ " " ++ str(g_{name}.taps_in_loop))
print(if g_{name}.common != "" then "// common\n" ++ g_{name}.common ++ "\n\n" else "" end ++
  "// void effect(inout Fx fx)\n" ++ g_{name}.code)
"#
        ));
    }
    let mut w = world(&src, RunPolicy::FAST);
    let out = frame(&mut w).join("\n") + "\n";
    let mut seen = 0;
    for chunk in out.split("=== ").skip(1) {
        let (head, glsl) = chunk.split_once('\n').unwrap();
        let name = head.split(' ').next().unwrap();
        assert!(head.contains(" nil taps "), "{name} was refused: {head}");
        assert_eq!(glsl, read(&format!("tests/expected/{name}.glsl")), "{name}");
        if name == "streak" {
            // src() sits in a loop of 7 iterations.
            assert!(head.ends_with("taps 7 true"), "{head}");
        }
        seen += 1;
    }
    assert_eq!(seen, EFFECTS.len());
}

#[test]
fn code_outside_the_subset_is_refused_with_a_position() {
    let source = read("tests/unsupported.ptl");
    let mut src = format!("import glsl\n{source}{POST_ENV}");
    let names = ["uses_list", "truthy", "stringy", "untyped", "wrong_result", "mismatch", "dynamic_capture"];
    for name in names {
        src.push_str(&format!(
            "let g_{name} = glsl.glsl_function({name}, ENV)\nprint(if g_{name}.error == nil then \"ok \" ++ g_{name}.code ++ \" \" ++ str(g_{name}.uniforms) else glsl.glsl_error_text(g_{name}.error) end)\n"
        ));
    }
    src.push_str("print(glsl.to_glsl(truthy, ENV))\n");
    let mut w = world(&src, RunPolicy::FAST);
    // Lines are one more than in the file: the `import` goes first.
    assert_eq!(
        frame(&mut w),
        [
            "IndexAccess has no GLSL form [line 22, column 60]",
            "a condition must be a bool, found float (GLSL has no truthiness) [line 25, column 45]",
            "a string literal has no GLSL form [line 28, column 57]",
            "param `amount` needs a default (it is the param's default value) [line 31, column 15]",
            "the function returns the new `fx.color` (vec3), found float [line 34, column 48]",
            "no GLSL operation between vec3 and vec2 [line 37, column 44]",
            // The stand-in refused this one only because it ran offline: in the
            // VM the captured `state` has a value, and it becomes a uniform.
            "ok fx.color = fx.color * params.tint_k * params.amount; { tint_k: 0.5 }",
            "#error to_glsl: a condition must be a bool, found float (GLSL has no truthiness) [line 25, column 45]",
        ]
    );
}

#[test]
fn control_flow_helpers_uniforms_and_plain_functions() {
    let src = format!(
        r#"import glsl
{POST_ENV}
let TAU = 6.283185307179586
let INK = #05030a
fn wobble(x: float, k: float) -> float
  sin(x * TAU) * k
end
fn pick3(a: vec3, t: float)
  let m = if t > 0.5 then
    let q = a * 2.0
    q + 1.0
  else
    a
  end
  m * wobble(t, 2)
end
let e = fn(fx: Fx, amount = 0.5, n: int = 3)
  let v = lerp(fx.color, INK, amount) + pick3(fx.color, amount)
  if amount > 0.5 && !(n == 3) then
    return v % 2.0
  elsif amount < 0.1 then
    return v * (n % 2)
  end
  v * -amount
end
print(glsl.to_glsl(e, ENV, {{name: "e"}}))
let g = glsl.glsl_function(e, ENV, {{name: "e"}})
print(g.uniforms, g.uniform_types, g.param_types, g.params)
print(glsl.to_glsl(wobble, ENV, {{inline_captures: true}}))
print(glsl.to_glsl(fn(a: float, b: vec2) -> b * a + mag(a, 2), {{}}, {{name: "scale2"}}))
"#
    );
    let mut w = world(&src, RunPolicy::FAST);
    let want = r#"float e_wobble(float x, float k) {
    return sin(x * params.wobble_TAU) * k;
}
vec3 e_pick3(vec3 a, float t) {
    vec3 pick;
    if (t > 0.5) {
        vec3 q = a * 2.0;
        pick = q + 1.0;
    } else {
        pick = a;
    }
    vec3 m = pick;
    return m * e_wobble(t, 2.0);
}

void effect(inout Fx fx) {
    vec3 v = mix(fx.color, params.INK, params.amount) + e_pick3(fx.color, params.amount);
    if (params.amount > 0.5 && !(params.n == 3)) {
        fx.color = mod(v, 2.0);
        return;
    } else {
        if (params.amount < 0.1) {
            fx.color = v * float(params.n % 2);
            return;
        }
    }
    fx.color = v * -params.amount;
}
{ INK: { r: 5, g: 3, b: 10 }, wobble_TAU: 6.283185307179586 } { INK: "vec3", wobble_TAU: "float" } { amount: "float", n: "int" } { amount: 0.5, n: 3 }
float wobble(float x, float k) {
    return sin(x * 6.283185307179586) * k;
}
vec2 scale2(float a, vec2 b) {
    return b * a + length(vec2(a, 2.0));
}"#;
    assert_eq!(frame(&mut w).join("\n"), want);
}

/// A script that asks for the GLSL every frame. `PHASE` is a captured float
/// that changes every frame (a uniform), `TAPS` a captured int (written into
/// the loop bound), and `weight` a helper function.
fn cache_script(taps: i64, gain: &str, body: &str, helper: &str, extra: &str) -> String {
    format!(
        r#"import glsl
{POST_ENV}
state t = 0.0
t += 0.25
let PHASE = t
let TAPS = {taps}
let GAIN = {gain}
{extra}
fn weight(i: int) -> float
  {helper}
end
let streak = fn(fx: Fx, amount = 0.5)
  var acc = fx.color * GAIN * PHASE
  for i in range(0, TAPS) do
    set acc = acc + src(fx.uv) * weight(i)
  end
  {body}
end
let g = glsl.glsl_function(streak, ENV)
print(str(g.error) ++ " " ++ str(g.taps) ++ " " ++ str(g.uniforms.GAIN) ++ " " ++ str(g.uniforms.PHASE) ++ " " ++ g.code)
"#
    )
}

#[test]
fn a_script_that_translates_every_frame_translates_once_per_change() {
    // With the memo on and with it off: the cache is keyed state, so it does
    // not depend on the call memo.
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE] {
        let v1 = cache_script(2, "1.5", "acc * amount", "1.0 / (i + 1.0)", "");
        let mut w = world(&v1, policy);
        // `fn_ast` runs once per function translated and never on a cache hit:
        // the lambda and its helper are two.
        let translated = |w: &World| native_calls(w, "fn_ast") / 2;

        let first = frame(&mut w).remove(0);
        assert!(first.starts_with("nil 2 1.5 0.25 "), "{first}");
        assert!(first.contains("i < 2;"), "{first}");
        assert_eq!(translated(&w), 1);

        // Ten more frames: PHASE moves every frame, nothing is re-translated.
        for _ in 0..10 {
            frame(&mut w);
        }
        let later = frame(&mut w).remove(0);
        assert!(later.starts_with("nil 2 1.5 3.0 "), "{later}");
        assert_eq!(translated(&w), 1, "frames with no edit");

        // A comment: no recompile, no translation.
        let v2 = format!("// a note\n{v1}");
        assert_eq!(reload(&mut w, &v2), ReloadOutcome::Relocated);
        frame(&mut w);
        assert_eq!(translated(&w), 1, "a comment-only edit");

        // An unrelated binding: the program is recompiled, every closure is
        // new, the function's index moves; its code hash does not.
        let v3 = cache_script(2, "1.5", "acc * amount", "1.0 / (i + 1.0)", "fn unrelated() 7 end\nlet other = unrelated()");
        assert_eq!(reload(&mut w, &v3), ReloadOutcome::Recompiled);
        frame(&mut w);
        assert_eq!(translated(&w), 1, "an unrelated edit");

        // A captured float: it is a uniform, so only its value changes.
        let v4 = cache_script(2, "2.5", "acc * amount", "1.0 / (i + 1.0)", "fn unrelated() 7 end\nlet other = unrelated()");
        reload(&mut w, &v4);
        let out = frame(&mut w).remove(0);
        assert!(out.contains(" 2.5 "), "{out}");
        assert_eq!(translated(&w), 1, "a captured float changed");

        // The lambda's body: one translation.
        let v5 = cache_script(2, "2.5", "acc * amount * 0.5", "1.0 / (i + 1.0)", "fn unrelated() 7 end\nlet other = unrelated()");
        reload(&mut w, &v5);
        let out = frame(&mut w).remove(0);
        assert!(out.ends_with("fx.color = acc * params.amount * 0.5;"), "{out}");
        frame(&mut w);
        assert_eq!(translated(&w), 2, "the lambda was edited");

        // A helper it calls: one more.
        let v6 = cache_script(2, "2.5", "acc * amount * 0.5", "1.0 / (i + 2.0)", "fn unrelated() 7 end\nlet other = unrelated()");
        reload(&mut w, &v6);
        frame(&mut w);
        frame(&mut w);
        assert_eq!(translated(&w), 3, "a helper was edited");

        // A captured int is written into the text (a loop bound): one more.
        let v7 = cache_script(3, "2.5", "acc * amount * 0.5", "1.0 / (i + 2.0)", "fn unrelated() 7 end\nlet other = unrelated()");
        reload(&mut w, &v7);
        let out = frame(&mut w).remove(0);
        assert!(out.starts_with("nil 3 ") && out.contains("i < 3;"), "{out}");
        frame(&mut w);
        assert_eq!(translated(&w), 4, "a captured int changed");
    }
}

#[test]
fn a_cache_hit_fetches_no_syntax_tree_and_is_small() {
    // What a frame pays once the translation is cached: one `fn_info` per
    // function involved and a state read. (The call memo does not replay
    // `glsl_function` itself: comparing its captures means comparing the
    // library's own functions, which is past the memo's comparison budget.)
    for policy in [RunPolicy::REPLAY, RunPolicy::BASELINE] {
        let src = cache_script(2, "1.5", "acc * amount", "1.0 / (i + 1.0)", "");
        let mut w = world(&src, policy);
        frame(&mut w);
        frame(&mut w);
        let (ast, info, insts) = (
            native_calls(&w, "fn_ast"),
            native_calls(&w, "fn_info"),
            w.env.profile().total_insts(),
        );
        frame(&mut w);
        assert_eq!(native_calls(&w, "fn_ast"), ast);
        assert!(native_calls(&w, "fn_info") - info <= 2);
        let per_frame = w.env.profile().total_insts() - insts;
        // The whole frame, the script's own top level included: about 640
        // instructions with the optimizer on and 810 with it off, where the
        // first frame (which translates) runs 4,700 and 7,700.
        assert!(per_frame < 1500, "a cached frame ran {per_frame} instructions");
    }
}
