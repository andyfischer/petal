//! Builtins - Built-in function implementations registered via native FFI.
//!
//! The built-in functions are split across topic submodules (math, collections,
//! creative_coding, noise, color, vec2, autodiff, io). `register_builtins`
//! below is the single entry point that wires them all into the
//! `NativeFnTable`. The registration *order* is load-bearing: phantom term
//! indices in the IR are assigned in this order, so the test snapshots and
//! serialized programs would drift if this list were reordered. Don't
//! reorder, only append.

use crate::native_fn::{InputClasses, NativeClass, NativeEffects, NativeFnTable, PetalCxt};

mod autodiff;
mod classes;
mod collections;
mod color;
mod creative_coding;
mod effects;
mod format;
mod handle;
mod io;
mod math;
mod noise;
mod output;
mod pending;
mod vec2;

pub(crate) use collections::SortKey;
pub use effects::{
    is_mutating_builtin, is_pure_builtin, looks_mutating, retains_no_reference,
    returns_fresh_container,
};

// xorshift64* PRNG. State lives per run in `ExecutionContext::rng_state` (seeded by
// `initial_seed()`), so each run and fork has isolated randomness.

/// Fallback state for an xorshift PRNG, which cannot run from 0.
pub const FALLBACK_SEED: u64 = 0x9E3779B97F4A7C15;

/// Map a caller-supplied seed onto a usable xorshift state: 0 is not a legal
/// state, so it becomes [`FALLBACK_SEED`]. Every other seed passes through, so
/// `--seed N` and `PETAL_SEED=N` name the same stream.
pub fn normalize_seed(seed: u64) -> u64 {
    if seed == 0 { FALLBACK_SEED } else { seed }
}

/// The seed requested by the `PETAL_SEED` environment variable, if any.
/// Decimal, or hex with a `0x` prefix. An unparseable value is ignored (the
/// clock seed wins) rather than being an error — this knob is set by wrappers
/// and CI, and a typo should not take a run down.
pub fn seed_from_env() -> Option<u64> {
    let raw = std::env::var("PETAL_SEED").ok()?;
    let text = raw.trim();
    let value = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok()?,
        None => text.parse::<u64>().ok()?,
    };
    Some(normalize_seed(value))
}

/// The seed a fresh [`ExecutionContext`](crate::execution_context::ExecutionContext)
/// initializes its `rng_state` to. `PETAL_SEED` overrides it, which is what
/// makes an embedder (petal-ui's `Headless`, garden) reproducible with no code
/// change of its own.
#[cfg(not(target_arch = "wasm32"))]
pub fn initial_seed() -> u64 {
    if let Some(seed) = seed_from_env() {
        return seed;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(FALLBACK_SEED)
}

#[cfg(target_arch = "wasm32")]
pub fn initial_seed() -> u64 {
    if let Some(seed) = seed_from_env() {
        return seed;
    }
    // `SystemTime::now()` traps on `wasm32-unknown-unknown` (no system clock).
    // Use a monotonically-bumped counter mixed with a constant so that repeated
    // process lifetimes still get distinct seeds.
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED_BUMP: AtomicU64 = AtomicU64::new(0);
    let n = SEED_BUMP.fetch_add(1, Ordering::Relaxed);
    0x9E3779B97F4A7C15u64.wrapping_add(n.wrapping_mul(0x100000001B3))
}

/// Advance the caller-owned xorshift64* state and return the next raw u64.
pub(super) fn rng_next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    if x == 0 {
        x = initial_seed() | 1; // xorshift requires non-zero state
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

pub(super) fn rng_next_f64(state: &mut u64) -> f64 {
    // 53-bit mantissa, uniform in [0, 1)
    (rng_next_u64(state) >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Validate that a native function received exactly `n` arguments.
///
/// Worded exactly as the user-function arity error (`Vm::push_closure_frame`) —
/// "expects N arguments, got M" — so a reader never has to wonder whether the
/// difference in tense means a difference in kind.
pub(super) fn require_args(state: &PetalCxt, n: usize, name: &str) -> Result<(), String> {
    if state.arg_count() != n {
        return Err(format!(
            "{}() expects {} argument{}, got {}",
            name,
            n,
            if n == 1 { "" } else { "s" },
            state.arg_count()
        ));
    }
    Ok(())
}

/// The row of the pending-inspection builtins (`is_loading`, `or_else`, …):
/// they read the resource table to answer, and they must see a `Pending`
/// argument rather than absorb it.
const INSPECTS_PENDING: NativeEffects =
    NativeEffects::reads(InputClasses::RESOURCES).with_pending(NativeClass::AllowPending);

/// Register all built-in functions into the native function table.
/// Must be called once at startup before any programs are loaded.
///
/// Every core native is registered with its [`NativeEffects`] row — what it
/// reads, whether it emits, whether it does something a replay could not
/// reproduce — and `every_core_native_is_declared` below keeps it that way.
/// The row is the union over every path through the native: the pending
/// inspectors read the resource table only when handed a `Pending`, and
/// declare `RESOURCES` regardless. `noise` reads the per-run noise seed,
/// which is program state set through `noise_seed`'s effect, not an external
/// input, so it is pure; the RNG builtins read (and advance) the random
/// stream, which a memoized scope compares at entry and exit itself. The
/// higher-order builtins (`map`, `sort_by`, …) are placeholders the VM
/// intercepts — what they do is whatever the closure they run does, and the
/// VM records that directly.
pub fn register_builtins(table: &mut NativeFnTable) {
    // Order matters — these must be registered in the same order as the old
    // BuiltinTable so that phantom term indices stay consistent.
    table.register_with(
        "print",
        io::native_print,
        NativeEffects::EFFECT.with_pending(NativeClass::Effectful),
    );
    table.register_with("range", collections::native_range, NativeEffects::PURE);
    table.register_with("len", collections::native_len, NativeEffects::PURE);
    table.register_with("push", collections::native_push, NativeEffects::PURE);
    table.register_with("str", io::native_str, NativeEffects::PURE);
    table.register_with("abs", math::native_abs, NativeEffects::PURE);
    table.register_with("sqrt", math::native_sqrt, NativeEffects::PURE);
    table.register_with("floor", math::native_floor, NativeEffects::PURE);
    table.register_with("ceil", math::native_ceil, NativeEffects::PURE);
    table.register_with("float", math::native_float, NativeEffects::PURE);
    table.register_with("int", math::native_int, NativeEffects::PURE);
    table.register_with(
        "random",
        math::native_random,
        NativeEffects::reads(InputClasses::RNG),
    );
    table.register_with("type", io::native_type, NativeEffects::PURE);
    table.register_with("append", collections::native_append, NativeEffects::PURE);
    table.register_with("pop", collections::native_pop, NativeEffects::PURE);
    table.register_with("keys", collections::native_keys, NativeEffects::PURE);
    table.register_with("values", collections::native_values, NativeEffects::PURE);
    table.register_with(
        "contains",
        collections::native_contains,
        NativeEffects::PURE,
    );
    table.register_with("min", math::native_min, NativeEffects::PURE);
    table.register_with("max", math::native_max, NativeEffects::PURE);
    table.register_with("round", math::native_round, NativeEffects::PURE);
    table.register_with("dual", autodiff::native_dual, NativeEffects::PURE);
    table.register_with("value_of", autodiff::native_value_of, NativeEffects::PURE);
    table.register_with("deriv_of", autodiff::native_deriv_of, NativeEffects::PURE);
    table.register_with("sort", collections::native_sort, NativeEffects::PURE);
    table.register_with("reverse", collections::native_reverse, NativeEffects::PURE);
    table.register_with("join", collections::native_join, NativeEffects::PURE);
    table.register_with("split", collections::native_split, NativeEffects::PURE);
    table.register_with("upper", collections::native_upper, NativeEffects::PURE);
    table.register_with("lower", collections::native_lower, NativeEffects::PURE);
    table.register_with(
        "enumerate",
        collections::native_enumerate,
        NativeEffects::PURE,
    );
    table.register_with("zip", collections::native_zip, NativeEffects::PURE);
    table.register_with("slice", collections::native_slice, NativeEffects::PURE);
    table.register_with("flat", collections::native_flat, NativeEffects::PURE);
    table.register_with(
        "includes",
        collections::native_contains,
        NativeEffects::PURE,
    ); // JS-style alias for contains
    table.register_with("sin", math::native_sin, NativeEffects::PURE);
    table.register_with("cos", math::native_cos, NativeEffects::PURE);
    table.register_with("tan", math::native_tan, NativeEffects::PURE);
    table.register_with("atan2", math::native_atan2, NativeEffects::PURE);
    table.register_with("pi", math::native_pi, NativeEffects::PURE);

    // --- Creative coding math builtins ---
    table.register_with("clamp", creative_coding::native_clamp, NativeEffects::PURE);
    table.register_with("lerp", creative_coding::native_lerp, NativeEffects::PURE);
    table.register_with(
        "map_range",
        creative_coding::native_map_range,
        NativeEffects::PURE,
    );
    table.register_with(
        "distance",
        creative_coding::native_distance,
        NativeEffects::PURE,
    );
    table.register_with("mag", creative_coding::native_mag, NativeEffects::PURE);
    table.register_with("pow", creative_coding::native_pow, NativeEffects::PURE);
    table.register_with("sign", creative_coding::native_sign, NativeEffects::PURE);
    table.register_with("fract", creative_coding::native_fract, NativeEffects::PURE);
    table.register_with(
        "smoothstep",
        creative_coding::native_smoothstep,
        NativeEffects::PURE,
    );
    table.register_with(
        "radians",
        creative_coding::native_radians,
        NativeEffects::PURE,
    );
    table.register_with(
        "degrees",
        creative_coding::native_degrees,
        NativeEffects::PURE,
    );
    table.register_with("exp", creative_coding::native_exp, NativeEffects::PURE);
    table.register_with("log", creative_coding::native_log, NativeEffects::PURE);

    // --- Noise ---
    table.register_with("noise", noise::native_noise, NativeEffects::PURE);
    table.register_with(
        "noise_seed",
        noise::native_noise_seed,
        NativeEffects::EFFECT,
    );

    // --- Randomness ---
    table.register_with(
        "random_int",
        creative_coding::native_random_int,
        NativeEffects::reads(InputClasses::RNG),
    );
    table.register_with(
        "choose",
        creative_coding::native_choose,
        NativeEffects::reads(InputClasses::RNG),
    );

    // --- Color ---
    table.register_with("hsv", color::native_hsv, NativeEffects::PURE);
    table.register_with("hsl", color::native_hsl, NativeEffects::PURE);
    table.register_with("color_lerp", color::native_color_lerp, NativeEffects::PURE);

    // --- Vec2 ---
    table.register_with("vec2", vec2::native_vec2, NativeEffects::PURE);
    table.register_with("normalize", vec2::native_normalize, NativeEffects::PURE);
    table.register_with("dot", vec2::native_dot, NativeEffects::PURE);
    table.register_with("limit", vec2::native_limit, NativeEffects::PURE);

    // Higher-order builtins: registered so the compiler sees them, but
    // dispatched as evaluator intrinsics at runtime.
    let map_id = table.register_with("map", native_intrinsic_placeholder, NativeEffects::PURE);
    let filter_id =
        table.register_with("filter", native_intrinsic_placeholder, NativeEffects::PURE);
    let reduce_id =
        table.register_with("reduce", native_intrinsic_placeholder, NativeEffects::PURE);
    let for_each_id =
        table.register_with("forEach", native_intrinsic_placeholder, NativeEffects::PURE);

    // --- Assertions (append-only to preserve phantom term indices) ---
    table.register_with("assert", io::native_assert, NativeEffects::PURE);
    table.register_with("assert_eq", io::native_assert_eq, NativeEffects::PURE);

    // --- Flat unboxed f64 arrays (append-only to preserve phantom term indices) ---
    table.register_with(
        "f64_array",
        collections::native_f64_array,
        NativeEffects::PURE,
    );
    table.register_with("set_at", collections::native_set_at, NativeEffects::PURE);
    table.register_with("swap", collections::native_swap, NativeEffects::PURE);
    table.register_with("hsv_deg", color::native_hsv_deg, NativeEffects::PURE);
    table.register_with("hsl_deg", color::native_hsl_deg, NativeEffects::PURE);

    // --- Symbols & buffered output (append-only to preserve phantom term indices) ---
    table.register_with("symbol", output::native_symbol, NativeEffects::PURE);
    table.register_with(
        "push_output",
        output::native_push_output,
        NativeEffects::EMITS,
    );
    table.register_with(
        "binding",
        output::native_binding,
        NativeEffects::probe(InputClasses::BINDINGS),
    );

    // --- Immutable collection ops (append-only to preserve phantom term indices) ---
    table.register_with("last", collections::native_last, NativeEffects::PURE);
    table.register_with(
        "drop_last",
        collections::native_drop_last,
        NativeEffects::PURE,
    );
    table.register_with("remove", collections::native_remove, NativeEffects::PURE);

    // --- Handles (append-only to preserve phantom term indices) ---
    table.register_with("is_valid", handle::native_is_valid, NativeEffects::PURE);

    // --- Test-only pending-resource builtins (append-only) ---
    table.register_with(
        "__pending",
        pending::native_pending,
        NativeEffects::EFFECT.with_pending(NativeClass::AllowPending),
    );
    table.register_with(
        "__resolve",
        pending::native_resolve,
        NativeEffects::EFFECT.with_pending(NativeClass::AllowPending),
    );
    table.register_with(
        "__reject",
        pending::native_reject,
        NativeEffects::EFFECT.with_pending(NativeClass::AllowPending),
    );

    // --- Pending meta builtins (Chunk D, append-only) ---
    // The sanctioned way to inspect pending-ness. Each is `AllowPending` so it
    // sees the Pending arg instead of absorbing it (Strict here would collapse
    // inspection into absorption), and reads the resource table to answer.
    table.register_with("is_loading", pending::native_is_loading, INSPECTS_PENDING);
    table.register_with("is_error", pending::native_is_error, INSPECTS_PENDING);
    table.register_with("is_pending", pending::native_is_pending, INSPECTS_PENDING);
    table.register_with("is_ready", pending::native_is_ready, INSPECTS_PENDING);
    table.register_with("error_of", pending::native_error_of, INSPECTS_PENDING);
    table.register_with("or_else", pending::native_or_else, INSPECTS_PENDING);
    table.register_with(
        "resource_key",
        pending::native_resource_key,
        INSPECTS_PENDING,
    );

    // --- Classes (append-only to preserve phantom term indices) ---
    // Built-in class constructors and methods; see `builtins::classes`.
    classes::register(table);

    // Method declaration (`fn Class.method`). Registered so the compiler can
    // emit an ordinary `BuiltinCall` for it, but never actually called: the VM
    // intercepts the id below, because publishing a method touches runtime
    // state that `PetalCxt` deliberately does not expose.
    let declare_method_id = table.register_with(
        crate::classes::DECLARE_METHOD_BUILTIN,
        native_intrinsic_placeholder,
        NativeEffects::EFFECT,
    );
    table.intrinsic_declare_method = Some(declare_method_id);

    // --- Failable numeric parsing + character-indexed strings (append-only) ---
    // `parse_*` return nil instead of aborting, so a program reading user input
    // can validate it. The `char_*` family indexes text by character where
    // `len`/`slice` index by byte.
    table.register_with("parse_float", math::native_parse_float, NativeEffects::PURE);
    table.register_with("parse_int", math::native_parse_int, NativeEffects::PURE);
    table.register_with("chars", collections::native_chars, NativeEffects::PURE);
    table.register_with(
        "char_len",
        collections::native_char_len,
        NativeEffects::PURE,
    );
    table.register_with("char_at", collections::native_char_at, NativeEffects::PURE);
    table.register_with(
        "char_slice",
        collections::native_char_slice,
        NativeEffects::PURE,
    );
    table.register_with(
        "index_of",
        collections::native_index_of,
        NativeEffects::PURE,
    );

    // --- Collections, formatting, and safe arithmetic (append-only) ---
    // `sort_by` and the two-argument `sort` call user code, so they are
    // dispatched as VM intrinsics (see `vm::native::call_native_or_intrinsic`);
    // the placeholder below only exists so the compiler resolves the name.
    let sort_by_id =
        table.register_with("sort_by", native_intrinsic_placeholder, NativeEffects::PURE);
    table.register_with("prepend", collections::native_prepend, NativeEffects::PURE);
    table.register_with("concat", collections::native_concat, NativeEffects::PURE);
    table.register_with("fixed", format::native_fixed, NativeEffects::PURE);
    table.register_with("commas", format::native_commas, NativeEffects::PURE);
    table.register_with("pad_start", format::native_pad_start, NativeEffects::PURE);
    table.register_with("pad_end", format::native_pad_end, NativeEffects::PURE);
    table.register_with("format", format::native_format, NativeEffects::PURE);
    table.register_with("safe_div", math::native_safe_div, NativeEffects::PURE);

    table.intrinsic_map = Some(map_id);
    table.intrinsic_sort = table.lookup_name("sort");
    table.intrinsic_sort_by = Some(sort_by_id);
    table.intrinsic_filter = Some(filter_id);
    table.intrinsic_reduce = Some(reduce_id);
    table.intrinsic_for_each = Some(for_each_id);
}

fn native_intrinsic_placeholder(_state: &mut PetalCxt) -> Result<u32, String> {
    Err("This function requires evaluator context and should be dispatched as an intrinsic".into())
}

#[cfg(test)]
mod effect_tests {
    use super::*;

    /// Every core native declares its effect row at registration, so the core
    /// can never regress to inference — the mechanism that let `panel_store_set`
    /// look pure to the memo (see docs/tasks/declarative-effect-refactoring.md).
    #[test]
    fn every_core_native_is_declared() {
        let mut table = NativeFnTable::new();
        register_builtins(&mut table);
        let undeclared: Vec<&str> = table
            .undeclared()
            .into_iter()
            .map(|id| table.get_name(id))
            .collect();
        assert!(
            undeclared.is_empty(),
            "core natives without an effect row: {undeclared:?}"
        );
        assert!(
            table.count() > 100,
            "expected the full core table, got {}",
            table.count()
        );
    }

    /// The `Pending` policy the row carries is the one the call boundary
    /// consults — `get_class` never disagrees with the declaration.
    #[test]
    fn pending_policy_comes_from_the_row() {
        let mut table = NativeFnTable::new();
        register_builtins(&mut table);
        for (name, class) in [
            ("print", NativeClass::Effectful),
            ("push_output", NativeClass::Effectful),
            ("is_loading", NativeClass::AllowPending),
            ("__resolve", NativeClass::AllowPending),
            ("sqrt", NativeClass::Strict),
        ] {
            let id = table.lookup_name(name).unwrap();
            assert_eq!(table.get_class(id), class, "{name}");
            assert_eq!(table.effects(id).unwrap().pending, class, "{name}");
        }
    }

    /// Spot-check the rows against what the natives' bodies actually call on
    /// `PetalCxt`, so a row cannot quietly say less than the code does.
    #[test]
    fn rows_match_what_the_bodies_do() {
        let mut table = NativeFnTable::new();
        register_builtins(&mut table);
        let row = |n: &str| table.effects(table.lookup_name(n).unwrap()).unwrap();
        assert!(row("print").effect && !row("print").emits);
        assert!(row("push_output").emits && !row("push_output").effect);
        assert!(row("binding").probe && row("binding").reads.contains(InputClasses::BINDINGS));
        assert!(row("random").reads.contains(InputClasses::RNG) && !row("random").effect);
        assert!(row("noise_seed").effect);
        assert!(row("or_else").reads.contains(InputClasses::RESOURCES));
        assert!(row("__pending").effect);
        assert_eq!(row("sqrt"), NativeEffects::PURE);
        assert_eq!(row("Rect"), NativeEffects::PURE);
    }
}
