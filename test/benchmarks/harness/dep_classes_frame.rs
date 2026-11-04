//! What the dependency classes' memo hints save on a game-shaped frame
//! (layer L5 of the retained optimization plan,
//! `docs/dev/retained-scene-output.md`).
//!
//! The script declares a static city through a helper per block, ships
//! that read a ticking clock directly, and a HUD that reads the clock for a
//! spinner and draws rows that do not change. It runs with memoization on, once honouring the
//! hints and once ignoring them (`Env::set_memo_hints`), and every frame's
//! output is compared between the two.
//!
//! ```sh
//! cargo run --release --example dep_classes_frame -- [frames] [blocks] [ships] [rows]
//! NOSTATIC=1 cargo run --release --example dep_classes_frame
//! ```

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use petal::env::Env;
use petal::native_fn::{InputClasses, NativeEffects, PetalCxt};
use petal::policy::RunPolicy;

fn src(blocks: usize, ships: usize, rows: usize) -> String {
    format!(
        r#"
native fn now()
native fn put(v)

const CITY = map(range(0, {blocks}), fn(i) {{mesh: "block{{i % 8}}", x: float(i % 50) * 12.0, z: float(i / 50) * 12.0}} end)

fn block(b)
  put({{mesh: b.mesh, x: b.x, z: b.z}})
  nil
end

fn district(bs)
  for b in bs do block(b) end
  nil
end

fn city(c)
  let n = len(c)
  let step = 100
  for i in range(0, (n + step - 1) / step) do
    district(slice(c, i * step, min(n, (i + 1) * step)))
  end
  nil
end

fn row(i)
  put({{row: i, label: "item {{i}}", w: 200.0}})
  nil
end

fn hud(n)
  let t = now()
  put({{spinner: t * 6.0}})
  for i in range(0, n) do row(i) end
  nil
end

fn ship(i)
  let t = now()
  put({{ship: i, x: cos(t + float(i)) * 60.0, z: sin(t + float(i)) * 60.0}})
  nil
end

city(CITY)
for i in range(0, {ships}) do ship(i) end
hud({rows})
"#
    )
}

fn put(c: &mut PetalCxt) -> petal::native_fn::NativeResult {
    let v = c.get_value(1)?;
    let sym = c.intern_symbol("out");
    c.push_output(sym, v);
    c.push_nil();
    Ok(1)
}

struct Run {
    env: Env,
    sid: petal::stack::StackKey,
    clock: Rc<Cell<f64>>,
}

fn setup(src: &str, hints: bool) -> Run {
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.gate = false;
    // `NOSTATIC=1`: no static-value cache (as under observation), so
    // static arguments are rebuilt every run and compared by content.
    if std::env::var("NOSTATIC").is_ok_and(|v| v == "1") {
        policy = policy.with_static_cache(false);
    }
    env.set_policy(policy);
    let clock = Rc::new(Cell::new(0.0));
    let c = clock.clone();
    env.register_native_boxed(
        "now",
        move |cx| {
            cx.push_float(c.get());
            Ok(1)
        },
        NativeEffects::probe(InputClasses::CLOCK).ticks(),
    );
    env.register_native("put", put, NativeEffects::EMITS);
    let pid = env.load_program(src).expect("load");
    let sid = env.create_stack(pid).unwrap();
    env.set_memo_hints(sid, hints);
    Run { env, sid, clock }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let frames = args.first().copied().unwrap_or(300);
    let blocks = args.get(1).copied().unwrap_or(2000);
    let ships = args.get(2).copied().unwrap_or(300);
    let rows = args.get(3).copied().unwrap_or(300);
    let src = src(blocks, ships, rows);
    let mut runs = [setup(&src, false), setup(&src, true)];
    let mut times = [Vec::new(), Vec::new()];
    let out = runs[0].env.intern_symbol("out");
    for f in 0..frames {
        let mut outs = Vec::new();
        for (r, t) in runs.iter_mut().zip(times.iter_mut()) {
            r.clock.set(f as f64 / 60.0);
            r.env.reset_stack(r.sid).unwrap();
            let start = Instant::now();
            r.env.run(r.sid).expect("run");
            t.push(start.elapsed().as_secs_f64() * 1e6);
            outs.push(r.env.take_output_buffer(out).len());
        }
        assert_eq!(outs[0], outs[1], "frame {f}: the runs disagree");
    }
    println!(
        "{frames} frames, {blocks} city blocks, {ships} ships reading a ticking clock, a HUD reading it with {rows} rows{}",
        if std::env::var("NOSTATIC").is_ok_and(|v| v == "1") { ", static cache off" } else { "" }
    );
    for (name, (r, t)) in ["hints ignored", "hints"].iter().zip(runs.iter().zip(times)) {
        let warm: Vec<f64> = t.into_iter().skip(10).collect();
        let s = r.env.memo_stats(r.sid).unwrap();
        println!(
            "  {name:<14} median {:>8.1} µs   hits {}  misses {}  records {}  cold {}  static skips {}  static args {}",
            median(warm),
            s.hits,
            s.misses,
            s.records,
            s.cold,
            s.static_skips,
            s.static_args
        );
    }
}
