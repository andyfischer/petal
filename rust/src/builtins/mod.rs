//! Builtins - Built-in function implementations registered via native FFI.
//!
//! The built-in functions are split across topic submodules (math, collections,
//! creative_coding, noise, color, vec2, autodiff, io). `register_builtins`
//! below is the single entry point that wires them all into the
//! `NativeFnTable`. The registration *order* is load-bearing: phantom term
//! indices in the IR are assigned in this order, so the test snapshots and
//! serialized programs would drift if this list were reordered. Don't
//! reorder, only append.

use crate::heap::Heap;
use crate::native_fn::{NativeFnTable, PetalCxt};
use crate::value::Value;

mod autodiff;
mod collections;
mod color;
mod creative_coding;
mod io;
mod math;
mod noise;
mod vec2;

// xorshift64* PRNG, seeded from system time on first use. Replaces an earlier
// implementation that used `subsec_nanos()` per call — that aliased multiple
// random() calls within the same frame to nearly identical values.
use std::sync::atomic::{AtomicU64, Ordering};
static RNG_STATE: AtomicU64 = AtomicU64::new(0);

#[cfg(not(target_arch = "wasm32"))]
fn initial_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
}

#[cfg(target_arch = "wasm32")]
fn initial_seed() -> u64 {
    // `SystemTime::now()` traps on `wasm32-unknown-unknown` (no system clock).
    // Use a monotonically-bumped counter mixed with a constant so that repeated
    // process lifetimes still get distinct seeds.
    static SEED_BUMP: AtomicU64 = AtomicU64::new(0);
    let n = SEED_BUMP.fetch_add(1, Ordering::Relaxed);
    0x9E3779B97F4A7C15u64.wrapping_add(n.wrapping_mul(0x100000001B3))
}

fn rng_next_u64() -> u64 {
    let mut x = RNG_STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = initial_seed() | 1; // xorshift requires non-zero state
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    RNG_STATE.store(x, Ordering::Relaxed);
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

pub(super) fn rng_next_f64() -> f64 {
    // 53-bit mantissa, uniform in [0, 1)
    (rng_next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Validate that a native function received exactly `n` arguments.
pub(super) fn require_args(state: &PetalCxt, n: usize, name: &str) -> Result<(), String> {
    if state.arg_count() != n {
        return Err(format!(
            "{}() expects {} argument{}",
            name,
            n,
            if n == 1 { "" } else { "s" }
        ));
    }
    Ok(())
}

/// Register all built-in functions into the native function table.
/// Must be called once at startup before any programs are loaded.
pub fn register_builtins(table: &mut NativeFnTable) {
    // Order matters — these must be registered in the same order as the old
    // BuiltinTable so that phantom term indices stay consistent.
    table.register("print", io::native_print);
    table.register("range", collections::native_range);
    table.register("len", collections::native_len);
    table.register("push", collections::native_push);
    table.register("str", io::native_str);
    table.register("abs", math::native_abs);
    table.register("sqrt", math::native_sqrt);
    table.register("floor", math::native_floor);
    table.register("ceil", math::native_ceil);
    table.register("float", math::native_float);
    table.register("int", math::native_int);
    table.register("random", math::native_random);
    table.register("type", io::native_type);
    table.register("append", collections::native_append);
    table.register("pop", collections::native_pop);
    table.register("keys", collections::native_keys);
    table.register("values", collections::native_values);
    table.register("contains", collections::native_contains);
    table.register("min", math::native_min);
    table.register("max", math::native_max);
    table.register("round", math::native_round);
    table.register("dual", autodiff::native_dual);
    table.register("value_of", autodiff::native_value_of);
    table.register("deriv_of", autodiff::native_deriv_of);
    table.register("sort", collections::native_sort);
    table.register("reverse", collections::native_reverse);
    table.register("join", collections::native_join);
    table.register("split", collections::native_split);
    table.register("enumerate", collections::native_enumerate);
    table.register("zip", collections::native_zip);
    table.register("slice", collections::native_slice);
    table.register("flat", collections::native_flat);
    table.register("includes", collections::native_contains); // JS-style alias for contains
    table.register("sin", math::native_sin);
    table.register("cos", math::native_cos);
    table.register("tan", math::native_tan);
    table.register("atan2", math::native_atan2);
    table.register("pi", math::native_pi);

    // --- Creative coding math builtins ---
    table.register("clamp", creative_coding::native_clamp);
    table.register("lerp", creative_coding::native_lerp);
    table.register("map_range", creative_coding::native_map_range);
    table.register("distance", creative_coding::native_distance);
    table.register("mag", creative_coding::native_mag);
    table.register("pow", creative_coding::native_pow);
    table.register("sign", creative_coding::native_sign);
    table.register("fract", creative_coding::native_fract);
    table.register("smoothstep", creative_coding::native_smoothstep);
    table.register("radians", creative_coding::native_radians);
    table.register("degrees", creative_coding::native_degrees);
    table.register("exp", creative_coding::native_exp);
    table.register("log", creative_coding::native_log);

    // --- Noise ---
    table.register("noise", noise::native_noise);
    table.register("noise_seed", noise::native_noise_seed);

    // --- Randomness ---
    table.register("random_int", creative_coding::native_random_int);
    table.register("choose", creative_coding::native_choose);

    // --- Color ---
    table.register("hsv", color::native_hsv);
    table.register("hsl", color::native_hsl);
    table.register("color_lerp", color::native_color_lerp);

    // --- Vec2 ---
    table.register("vec2", vec2::native_vec2);
    table.register("normalize", vec2::native_normalize);
    table.register("dot", vec2::native_dot);
    table.register("limit", vec2::native_limit);

    // Higher-order builtins: registered so the compiler sees them, but
    // dispatched as evaluator intrinsics at runtime.
    let map_id = table.register("map", native_intrinsic_placeholder);
    let filter_id = table.register("filter", native_intrinsic_placeholder);
    let reduce_id = table.register("reduce", native_intrinsic_placeholder);
    let for_each_id = table.register("forEach", native_intrinsic_placeholder);

    // --- Assertions (append-only to preserve phantom term indices) ---
    table.register("assert", io::native_assert);
    table.register("assert_eq", io::native_assert_eq);

    // --- Flat unboxed f64 arrays (append-only to preserve phantom term indices) ---
    table.register("f64_array", collections::native_f64_array);
    table.register("get", collections::native_get);
    table.register("set", collections::native_set);
    table.register("swap", collections::native_swap);
    table.register("hsv_deg", color::native_hsv_deg);
    table.register("hsl_deg", color::native_hsl_deg);

    table.intrinsic_map = Some(map_id);
    table.intrinsic_filter = Some(filter_id);
    table.intrinsic_reduce = Some(reduce_id);
    table.intrinsic_for_each = Some(for_each_id);
}

fn native_intrinsic_placeholder(_state: &mut PetalCxt) -> Result<u32, String> {
    Err("This function requires evaluator context and should be dispatched as an intrinsic".into())
}

/// Compare two Values (used by eval.rs for sorting and by min/max).
pub fn compare_values(
    a: &Value,
    b: &Value,
    heap: &Heap,
) -> Result<std::cmp::Ordering, String> {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => Ok(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => {
            Ok(a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Int(a), Value::Float(b)) => {
            Ok((*a as f64)
                .partial_cmp(b)
                .unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Float(a), Value::Int(b)) => {
            Ok(a.partial_cmp(&(*b as f64))
                .unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::String(a), Value::String(b)) => {
            Ok(heap.get_string(*a).cmp(heap.get_string(*b)))
        }
        // Dual comparisons use primal value only
        _ if a.as_f64().is_some() && b.as_f64().is_some() => {
            let af = a.as_f64().unwrap();
            let bf = b.as_f64().unwrap();
            Ok(af.partial_cmp(&bf).unwrap_or(std::cmp::Ordering::Equal))
        }
        _ => Err(format!(
            "Cannot compare {} and {}",
            a.type_name(),
            b.type_name()
        )),
    }
}
