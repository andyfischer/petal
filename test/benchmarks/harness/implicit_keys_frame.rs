//! What an implicit key costs against an id the script builds (layer L8 of
//! the retained optimization plan, `docs/dev/retained-scene-output.md`).
//!
//! The same frame declares `n` props from a helper called in a loop, three
//! ways: with a string id built per call (`decl("prop{i}", …)`), with an int
//! id (`decl(i, …)`), and with no id, the native reading its call's implicit
//! key (`PetalCxt::call_key`). Each declaration moves every frame, so every
//! call runs, which is the case where the key is computed rather than
//! replayed. The host reconciles each declaration by its key in a hash map.
//! A fourth run is the implicit-key script with memoization on and nothing
//! moving, where the keys come back with the replayed declarations.
//!
//! ```sh
//! cargo run --release --example implicit_keys_frame -- [frames] [props]
//! ```

use std::collections::HashMap;
use std::time::Instant;

use petal::env::Env;
use petal::native_fn::{NativeEffects, PetalCxt};
use petal::policy::RunPolicy;
use petal::value::Value;

#[derive(Clone, Copy, PartialEq)]
enum Ids {
    Strings,
    Ints,
    Implicit,
}

fn src(ids: Ids, props: usize, moving: bool) -> String {
    let call = match ids {
        Ids::Strings => "decl(\"prop{i}\", ",
        Ids::Ints => "decl(i, ",
        Ids::Implicit => "decl_k(",
    };
    let t = if moving { "t" } else { "0.0" };
    format!(
        r#"
native fn decl(id, opts)
native fn decl_k(opts)

fn prop(i, t)
  {call}{{shape: "box", size: 1.0, x: float(i % 50) * 2.0 + t, z: float(i / 50) * 2.0}})
  nil
end

fn props(n, t)
  for i in range(0, n) do prop(i, t) end
  nil
end

state frame = 0
frame += 1
let t = float(frame) / 60.0
props({props}, {t})
"#
    )
}

#[derive(Default)]
struct Host {
    by_string: HashMap<String, f64>,
    by_int: HashMap<i64, f64>,
}

fn x_of(env: &Env, opts: Value) -> f64 {
    let Value::Map(id) = opts else { return 0.0 };
    match env.heap().get_map(id).get("x") {
        Some(Value::Float(x)) => *x,
        _ => 0.0,
    }
}

fn run(label: &str, ids: Ids, props: usize, frames: usize, moving: bool, memo: bool) {
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.memo = memo;
    policy.gate = false;
    env.set_policy(policy);
    env.register_native(
        "decl",
        |c: &mut PetalCxt| {
            let id = c.get_value(1)?;
            let opts = c.get_value(2)?;
            let pair = c.heap_mut().alloc_list(vec![id, opts]);
            let sym = c.intern_symbol("scene");
            c.push_output(sym, Value::List(pair));
            c.push_nil();
            Ok(1)
        },
        NativeEffects::DECLARES,
    );
    env.register_native(
        "decl_k",
        |c: &mut PetalCxt| {
            let key = match c.call_key() {
                Some(k) => Value::Int(k as i64),
                None => Value::Nil,
            };
            let opts = c.get_value(1)?;
            let pair = c.heap_mut().alloc_list(vec![key, opts]);
            let sym = c.intern_symbol("scene");
            c.push_output(sym, Value::List(pair));
            c.push_nil();
            Ok(1)
        },
        NativeEffects::DECLARES,
    );
    let pid = env.load_program(&src(ids, props, moving)).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let scene = env.intern_symbol("scene");
    let mut host = Host::default();
    let mut script = Vec::with_capacity(frames);
    let mut reconcile = Vec::with_capacity(frames);
    for _ in 0..frames {
        let t0 = Instant::now();
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        let t1 = Instant::now();
        let values = env.take_output_buffer(scene);
        assert_eq!(values.len(), props);
        for v in &values {
            let Value::List(l) = v else { unreachable!() };
            let pair = env.heap().get_list(*l);
            let x = x_of(&env, pair[1]);
            match pair[0] {
                Value::String(s) => {
                    let s = env.heap().get_string(s);
                    match host.by_string.get_mut(s) {
                        Some(e) => *e = x,
                        None => {
                            host.by_string.insert(s.to_string(), x);
                        }
                    }
                }
                Value::Int(k) => {
                    host.by_int.insert(k, x);
                }
                other => panic!("no key: {other:?}"),
            }
        }
        let t2 = Instant::now();
        script.push((t1 - t0).as_secs_f64() * 1e6);
        reconcile.push((t2 - t1).as_secs_f64() * 1e6);
    }
    let keys = host.by_string.len() + host.by_int.len();
    assert_eq!(keys, props, "{label}: one entry per prop");
    let median = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let (s, h) = (median(&mut script), median(&mut reconcile));
    println!("{label:<34} {s:>9.0} {h:>9.0} {:>9.0}", s + h);
}

/// Entities whose order flips every frame, declared through a helper per
/// entity, in a keyed or a positional loop.
fn reorder_src(ents: usize, keyed: bool) -> String {
    let key = if keyed { " key e.id" } else { "" };
    format!(
        r#"
native fn decl_k(opts)

const A = map(range(0, {ents}), fn(i) {{id: i, x: float(i % 50) * 2.0, z: float(i / 50) * 2.0}} end)
const B = reverse(A)

fn ent(e)
  decl_k({{shape: "box", size: 1.0, x: e.x, z: e.z}})
  nil
end

state frame = 0
frame += 1
for e in (if frame % 2 == 0 then A else B end){key} do ent(e) end
"#
    )
}

fn run_reorder(label: &str, ents: usize, frames: usize, keyed: bool, memo: bool) {
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.memo = memo;
    policy.gate = false;
    env.set_policy(policy);
    env.register_native(
        "decl_k",
        |c: &mut PetalCxt| {
            let key = match c.call_key() {
                Some(k) => Value::Int(k as i64),
                None => Value::Nil,
            };
            let opts = c.get_value(1)?;
            let pair = c.heap_mut().alloc_list(vec![key, opts]);
            let sym = c.intern_symbol("scene");
            c.push_output(sym, Value::List(pair));
            c.push_nil();
            Ok(1)
        },
        NativeEffects::DECLARES,
    );
    let pid = env.load_program(&reorder_src(ents, keyed)).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let scene = env.intern_symbol("scene");
    let mut times = Vec::with_capacity(frames);
    for _ in 0..frames {
        let t0 = Instant::now();
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        times.push(t0.elapsed().as_secs_f64() * 1e6);
        assert_eq!(env.take_output_buffer(scene).len(), ents);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let hits = if memo { env.memo_stats(sid).unwrap().hits } else { 0 };
    println!(
        "{label:<34} {:>9.0} {:>9.1}",
        times[times.len() / 2],
        hits as f64 / frames as f64
    );
}

fn main() {
    let args: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let frames = args.first().copied().unwrap_or(200);
    let props = args.get(1).copied().unwrap_or(4000);
    println!("{props} props, {frames} frames, median µs per frame");
    println!("{:<34} {:>9} {:>9} {:>9}", "ids", "script", "host", "total");
    run("string ids, moving", Ids::Strings, props, frames, true, false);
    run("int ids, moving", Ids::Ints, props, frames, true, false);
    run("implicit keys, moving", Ids::Implicit, props, frames, true, false);
    run("implicit keys, static, memo", Ids::Implicit, props, frames, false, true);
    println!();
    println!("{props} entities in a flipping order, median µs per frame");
    println!("{:<34} {:>9} {:>9}", "loop", "script", "hits/fr");
    run_reorder("positional, memo off", props, frames, false, false);
    run_reorder("positional, memo", props, frames, false, true);
    run_reorder("keyed (key e.id), memo", props, frames, true, true);
}
