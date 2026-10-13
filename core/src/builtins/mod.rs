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
mod json;
mod math;
mod noise;
mod output;
#[cfg(test)]
mod decls;
mod pending;
mod vec2;

pub(crate) use collections::SortKey;
pub(crate) use io::type_name_id;
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
/// Every native is registered with its [`NativeEffects`] row — what it
/// reads, whether it emits, whether it does something a replay could not
/// reproduce; registration takes the row, so none can be left out.
/// The row is the union over every path through the native: the pending
/// inspectors read the resource table only when handed a `Pending`, and
/// declare `RESOURCES` regardless. `noise` reads the per-run noise seed,
/// which is program state set through `noise_seed`'s effect, not an external
/// input, so it is pure; the RNG builtins read (and advance) the random
/// stream, which a memoized scope compares at entry and exit itself. The
/// higher-order builtins (`map`, `sort_by`, …) are placeholders the VM
/// intercepts — what they do is whatever the closure they run does, and the
/// VM records that directly.
///
/// **`CONST` vs `PURE`.** A row is [`NativeEffects::CONST`] when the native
/// is a constant function — same arguments, same result, for the life of the
/// program, reading nothing dynamic at all — which is what lets a static
/// value (docs/proposals/static-values.md) keep the result of a call. Every
/// `CONST` row below was audited by reading the native's body: it touches
/// its arguments and the heap, and nothing else on `PetalCxt`. The rows left
/// at `PURE` are pure for the memo's purposes and *not* constant, each for a
/// reason noted where it is registered: `noise` (the noise seed), `is_valid`
/// (a handle's liveness), `fn_info`/`fn_ast` (the program's source),
/// `__enum_from` (the program's enum table), and the higher-order builtins
/// (whatever their callback does — see
/// [`NativeConstness::ConstIfCallbacksAre`](crate::native_fn::NativeConstness)).
/// A new builtin is dynamic until someone reads it and says otherwise; the
/// `const_builtins_are_exactly` test pins the list so a row cannot change
/// class silently.
pub fn register_builtins(table: &mut NativeFnTable) {
    // Order matters — these must be registered in the same order as the old
    // BuiltinTable so that phantom term indices stay consistent.
    table.register(
        "print",
        io::native_print,
        NativeEffects::EFFECT.with_pending(NativeClass::Effectful),
    );
    table.register("range", collections::native_range, NativeEffects::CONST);
    table.register("len", collections::native_len, NativeEffects::CONST);
    table.register("push", collections::native_push, NativeEffects::CONST);
    table.register("str", io::native_str, NativeEffects::CONST);
    table.register("abs", math::native_abs, NativeEffects::CONST);
    table.register("sqrt", math::native_sqrt, NativeEffects::CONST);
    table.register("floor", math::native_floor, NativeEffects::CONST);
    table.register("ceil", math::native_ceil, NativeEffects::CONST);
    table.register("float", math::native_float, NativeEffects::CONST);
    table.register("int", math::native_int, NativeEffects::CONST);
    table.register(
        "random",
        math::native_random,
        NativeEffects::reads(InputClasses::RNG),
    );
    table.register("type", io::native_type, NativeEffects::CONST);
    table.register("append", collections::native_append, NativeEffects::CONST);
    table.register("pop", collections::native_pop, NativeEffects::CONST);
    table.register("keys", collections::native_keys, NativeEffects::CONST);
    table.register("values", collections::native_values, NativeEffects::CONST);
    table.register(
        "contains",
        collections::native_contains,
        NativeEffects::CONST,
    );
    table.register("min", math::native_min, NativeEffects::CONST);
    table.register("max", math::native_max, NativeEffects::CONST);
    table.register("round", math::native_round, NativeEffects::CONST);
    table.register("dual", autodiff::native_dual, NativeEffects::CONST);
    table.register("value_of", autodiff::native_value_of, NativeEffects::CONST);
    table.register("deriv_of", autodiff::native_deriv_of, NativeEffects::CONST);
    // Not CONST as a row: `sort(xs, cmp)` runs a user comparator. Its
    // constness is `ConstIfCallbacksAre` (see `NativeFnTable::constness`),
    // which `sort(xs)` satisfies vacuously.
    table.register("sort", collections::native_sort, NativeEffects::PURE);
    table.register("reverse", collections::native_reverse, NativeEffects::CONST);
    table.register("join", collections::native_join, NativeEffects::CONST);
    table.register("split", collections::native_split, NativeEffects::CONST);
    table.register("upper", collections::native_upper, NativeEffects::CONST);
    table.register("lower", collections::native_lower, NativeEffects::CONST);
    table.register(
        "enumerate",
        collections::native_enumerate,
        NativeEffects::CONST,
    );
    table.register("zip", collections::native_zip, NativeEffects::CONST);
    table.register("slice", collections::native_slice, NativeEffects::CONST);
    table.register("flat", collections::native_flat, NativeEffects::CONST);
    table.register(
        "includes",
        collections::native_contains,
        NativeEffects::CONST,
    ); // JS-style alias for contains
    table.register("sin", math::native_sin, NativeEffects::CONST);
    table.register("cos", math::native_cos, NativeEffects::CONST);
    table.register("tan", math::native_tan, NativeEffects::CONST);
    table.register("atan2", math::native_atan2, NativeEffects::CONST);
    table.register("pi", math::native_pi, NativeEffects::CONST);

    // --- Creative coding math builtins ---
    table.register("clamp", creative_coding::native_clamp, NativeEffects::CONST);
    table.register("lerp", creative_coding::native_lerp, NativeEffects::CONST);
    table.register(
        "map_range",
        creative_coding::native_map_range,
        NativeEffects::CONST,
    );
    table.register(
        "distance",
        creative_coding::native_distance,
        NativeEffects::CONST,
    );
    table.register("mag", creative_coding::native_mag, NativeEffects::CONST);
    table.register("pow", creative_coding::native_pow, NativeEffects::CONST);
    table.register("sign", creative_coding::native_sign, NativeEffects::CONST);
    table.register("fract", creative_coding::native_fract, NativeEffects::CONST);
    table.register(
        "smoothstep",
        creative_coding::native_smoothstep,
        NativeEffects::CONST,
    );
    table.register(
        "radians",
        creative_coding::native_radians,
        NativeEffects::CONST,
    );
    table.register(
        "degrees",
        creative_coding::native_degrees,
        NativeEffects::CONST,
    );
    table.register("exp", creative_coding::native_exp, NativeEffects::CONST);
    table.register("log", creative_coding::native_log, NativeEffects::CONST);

    // --- Noise ---
    // Pure but not CONST: it reads the per-run noise seed, which
    // `noise_seed` (or the run's seeding) changes under the same arguments.
    table.register("noise", noise::native_noise, NativeEffects::PURE);
    table.register(
        "noise_seed",
        noise::native_noise_seed,
        NativeEffects::EFFECT,
    );

    // --- Randomness ---
    table.register(
        "random_int",
        creative_coding::native_random_int,
        NativeEffects::reads(InputClasses::RNG),
    );
    table.register(
        "choose",
        creative_coding::native_choose,
        NativeEffects::reads(InputClasses::RNG),
    );

    // --- Color ---
    table.register("hsv", color::native_hsv, NativeEffects::CONST);
    table.register("hsl", color::native_hsl, NativeEffects::CONST);
    table.register("color_lerp", color::native_color_lerp, NativeEffects::CONST);

    // --- Vec2 ---
    table.register("vec2", vec2::native_vec2, NativeEffects::CONST);
    table.register("normalize", vec2::native_normalize, NativeEffects::CONST);
    table.register("dot", vec2::native_dot, NativeEffects::CONST);
    table.register("limit", vec2::native_limit, NativeEffects::CONST);

    // Higher-order builtins: registered so the compiler sees them, but
    // dispatched as evaluator intrinsics at runtime. Not CONST as rows —
    // what a call does is what its callback does; `constness` answers
    // `ConstIfCallbacksAre` for `map`/`filter`/`reduce` (and `sort`,
    // `sort_by`), and `Dynamic` for `forEach`, which exists for effects.
    let map_id = table.register("map", native_intrinsic_placeholder, NativeEffects::PURE);
    let filter_id =
        table.register("filter", native_intrinsic_placeholder, NativeEffects::PURE);
    let reduce_id =
        table.register("reduce", native_intrinsic_placeholder, NativeEffects::PURE);
    let for_each_id =
        table.register("forEach", native_intrinsic_placeholder, NativeEffects::PURE);

    // --- Assertions (append-only to preserve phantom term indices) ---
    // CONST: the same arguments pass or raise the same way every time, and
    // raising is the call's outcome, not an effect a kept value could lose
    // (a static build that raises is not stored and raises again next run).
    table.register("assert", io::native_assert, NativeEffects::CONST);
    table.register("assert_eq", io::native_assert_eq, NativeEffects::CONST);

    // --- Flat unboxed f64 arrays (append-only to preserve phantom term indices) ---
    table.register(
        "f64_array",
        collections::native_f64_array,
        NativeEffects::CONST,
    );
    table.register("set_at", collections::native_set_at, NativeEffects::CONST);
    table.register("swap", collections::native_swap, NativeEffects::CONST);
    table.register("hsv_deg", color::native_hsv_deg, NativeEffects::CONST);
    table.register("hsl_deg", color::native_hsl_deg, NativeEffects::CONST);

    // --- Symbols & buffered output (append-only to preserve phantom term indices) ---
    // CONST: interning is idempotent and the id is stable for the life of
    // the `Env` that owns the symbol table.
    table.register("symbol", output::native_symbol, NativeEffects::CONST);
    table.register(
        "push_output",
        output::native_push_output,
        NativeEffects::EMITS,
    );
    table.register(
        "binding",
        output::native_binding,
        NativeEffects::probe(InputClasses::BINDINGS),
    );

    // --- Immutable collection ops (append-only to preserve phantom term indices) ---
    table.register("last", collections::native_last, NativeEffects::CONST);
    table.register(
        "drop_last",
        collections::native_drop_last,
        NativeEffects::CONST,
    );
    table.register("remove", collections::native_remove, NativeEffects::CONST);

    // --- Handles (append-only to preserve phantom term indices) ---
    // Pure but not CONST: the same handle answers differently once the host
    // drops what it names.
    table.register("is_valid", handle::native_is_valid, NativeEffects::PURE);

    // --- Test-only pending-resource builtins (append-only) ---
    table.register(
        "__pending",
        pending::native_pending,
        NativeEffects::EFFECT.with_pending(NativeClass::AllowPending),
    );
    table.register(
        "__resolve",
        pending::native_resolve,
        NativeEffects::EFFECT.with_pending(NativeClass::AllowPending),
    );
    table.register(
        "__reject",
        pending::native_reject,
        NativeEffects::EFFECT.with_pending(NativeClass::AllowPending),
    );

    // --- Pending meta builtins (Chunk D, append-only) ---
    // The sanctioned way to inspect pending-ness. Each is `AllowPending` so it
    // sees the Pending arg instead of absorbing it (Strict here would collapse
    // inspection into absorption), and reads the resource table to answer.
    table.register("is_loading", pending::native_is_loading, INSPECTS_PENDING);
    table.register("is_error", pending::native_is_error, INSPECTS_PENDING);
    table.register("is_pending", pending::native_is_pending, INSPECTS_PENDING);
    table.register("is_ready", pending::native_is_ready, INSPECTS_PENDING);
    table.register("error_of", pending::native_error_of, INSPECTS_PENDING);
    table.register("or_else", pending::native_or_else, INSPECTS_PENDING);
    table.register(
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
    let declare_method_id = table.register(
        crate::classes::DECLARE_METHOD_BUILTIN,
        native_intrinsic_placeholder,
        NativeEffects::EFFECT,
    );
    table.intrinsic_declare_method = Some(declare_method_id);

    // --- Failable numeric parsing + character-indexed strings (append-only) ---
    // `parse_*` return nil instead of aborting, so a program reading user input
    // can validate it. The `char_*` family indexes text by character where
    // `len`/`slice` index by byte.
    table.register("parse_float", math::native_parse_float, NativeEffects::CONST);
    table.register("parse_int", math::native_parse_int, NativeEffects::CONST);
    table.register("chars", collections::native_chars, NativeEffects::CONST);
    table.register(
        "char_len",
        collections::native_char_len,
        NativeEffects::CONST,
    );
    table.register("char_at", collections::native_char_at, NativeEffects::CONST);
    table.register(
        "char_slice",
        collections::native_char_slice,
        NativeEffects::CONST,
    );
    table.register(
        "index_of",
        collections::native_index_of,
        NativeEffects::CONST,
    );

    // --- Collections, formatting, and safe arithmetic (append-only) ---
    // `sort_by` and the two-argument `sort` call user code, so they are
    // dispatched as VM intrinsics (see `vm::native::call_native_or_intrinsic`);
    // the placeholder below only exists so the compiler resolves the name.
    let sort_by_id =
        table.register("sort_by", native_intrinsic_placeholder, NativeEffects::PURE);
    table.register("prepend", collections::native_prepend, NativeEffects::CONST);
    table.register("concat", collections::native_concat, NativeEffects::CONST);
    table.register("fixed", format::native_fixed, NativeEffects::CONST);
    table.register("commas", format::native_commas, NativeEffects::CONST);
    table.register("pad_start", format::native_pad_start, NativeEffects::CONST);
    table.register("pad_end", format::native_pad_end, NativeEffects::CONST);
    table.register("format", format::native_format, NativeEffects::CONST);
    table.register("safe_div", math::native_safe_div, NativeEffects::CONST);
    // The string codec the panel store (and any string-only channel) needs;
    // `json_parse` is failable like `parse_*`.
    table.register("json_stringify", json::native_json_stringify, NativeEffects::CONST);
    table.register("json_parse", json::native_json_parse, NativeEffects::CONST);
    // `rotate(v, angle)` — a vec2 rotated counter-clockwise (in y-up terms;
    // clockwise on a y-down screen) by `angle` radians. Append-only, like the
    // rest of this block.
    table.register("rotate", vec2::native_rotate, NativeEffects::CONST);
    // `vec3(x, y, z)` and `cross(a, b)` — the 3D vector (a heap value; see
    // docs/dev/vec3.md). The shared vector natives above take vec3s too.
    // Append-only, like the rest of this block.
    table.register("vec3", vec2::native_vec3, NativeEffects::CONST);
    table.register("cross", vec2::native_cross, NativeEffects::CONST);
    // The inverse trig functions and `hypot`, next to `sin`/`cos`/`atan2` in
    // spirit. Append-only, like the rest of this block.
    table.register("asin", math::native_asin, NativeEffects::CONST);
    table.register("acos", math::native_acos, NativeEffects::CONST);
    table.register("atan", math::native_atan, NativeEffects::CONST);
    table.register("hypot", math::native_hypot, NativeEffects::CONST);
    // The string helpers every text-handling script was writing by hand.
    // Append-only, like the rest of this block.
    table.register("repeat", collections::native_repeat, NativeEffects::CONST);
    table.register(
        "starts_with",
        collections::native_starts_with,
        NativeEffects::CONST,
    );
    table.register(
        "ends_with",
        collections::native_ends_with,
        NativeEffects::CONST,
    );
    table.register("trim", collections::native_trim, NativeEffects::CONST);
    // The explicit byte-unit pair, for when `len`/`slice` (code points) are
    // the wrong unit: wire formats, size budgets.
    table.register("byte_len", collections::native_byte_len, NativeEffects::CONST);
    table.register(
        "byte_slice",
        collections::native_byte_slice,
        NativeEffects::CONST,
    );

    // Function introspection (experimental; docs/features/function-introspection.md).
    // VM intrinsics: they read the closure table and the program's source.
    // Append-only, like the rest of this block. Pure but not CONST: the
    // result carries source positions, which an edit moves without touching
    // the function's own text.
    let fn_info_id = table.register("fn_info", native_intrinsic_placeholder, NativeEffects::PURE);
    let fn_ast_id = table.register("fn_ast", native_intrinsic_placeholder, NativeEffects::PURE);
    table.intrinsic_fn_info = Some(fn_info_id);
    table.intrinsic_fn_ast = Some(fn_ast_id);

    // Enums (append-only). `__enum_data` publishes a variant's constant data
    // into the running stack and `__enum_from` is `Enum.from(name)`, which
    // reads the program's enum table; both are VM intrinsics the compiler
    // emits and nothing else calls.
    let enum_data_id = table.register(
        crate::program::ENUM_DATA_BUILTIN,
        native_intrinsic_placeholder,
        NativeEffects::EFFECT,
    );
    let enum_from_id = table.register(
        crate::program::ENUM_FROM_BUILTIN,
        native_intrinsic_placeholder,
        // Pure but not CONST: it answers from the program's enum table.
        NativeEffects::PURE,
    );
    table.intrinsic_enum_data = Some(enum_data_id);
    table.intrinsic_enum_from = Some(enum_from_id);

    // `freeze(v)` / `frozen(v)`: make data that lives as long as the program
    // permanent so the collector stops re-tracing it (see `Heap::freeze`).
    // Both are effects, not pure: freezing changes how the heap treats the
    // value, and `frozen` answers from that heap state rather than from the
    // value's contents, so neither may be folded away, replayed from a
    // memoized scope, or (docs/proposals/static-values.md) treated as a
    // constant. Append-only, like the rest of this block.
    table.register("freeze", collections::native_freeze, NativeEffects::EFFECT);
    table.register("frozen", collections::native_frozen, NativeEffects::EFFECT);

    // `__enum_all` is `Enum.all`: the one list of an enum's payload-free
    // variants the running stack keeps (`EnumTable::all`). A VM intrinsic the
    // compiler emits, like `__enum_from`, and pure but not CONST for the same
    // reason: it answers from the program's enum table. Deliberately absent
    // from `effects::BUILTIN_EFFECTS`, so nothing takes its result for a
    // fresh container and writes through the shared list. Append-only.
    let enum_all_id = table.register(
        crate::program::ENUM_ALL_BUILTIN,
        native_intrinsic_placeholder,
        NativeEffects::PURE,
    );
    table.intrinsic_enum_all = Some(enum_all_id);

    // The trivial builtins the VM retires inline (`Inst::Intrinsic`), by the
    // ids they were just given.
    for op in crate::backend::bytecode::isa::IntrinsicOp::ALL {
        table.core_intrinsics[op as usize] = table.lookup_name(op.name());
    }

    table.intrinsic_map = Some(map_id);
    table.intrinsic_sort = table.lookup_name("sort");
    table.intrinsic_sort_by = Some(sort_by_id);
    table.intrinsic_filter = Some(filter_id);
    table.intrinsic_reduce = Some(reduce_id);
    table.intrinsic_for_each = Some(for_each_id);

    // Parameter names, for named arguments: read off the builtins' `native fn`
    // declarations in the bundled `core` and domain builtin packages, the
    // one place they are written (`core/packages/`, `crate::native_decl`). Applied last and
    // by name, so the registration order above — which is load-bearing — is
    // not disturbed.
    let undeclared = crate::native_decl::builtin_decls().apply_to(table);
    debug_assert!(
        undeclared.is_empty(),
        "the builtin packages declare natives that are not registered: {undeclared:?}"
    );
}

fn native_intrinsic_placeholder(_state: &mut PetalCxt) -> Result<u32, String> {
    Err("This function requires evaluator context and should be dispatched as an intrinsic".into())
}

#[cfg(test)]
mod effect_tests {
    use super::*;

    /// The whole core table registers, each native with a row (registration
    /// takes one, so this is a count rather than a scan for gaps).
    #[test]
    fn the_core_table_is_full() {
        let mut table = NativeFnTable::new();
        register_builtins(&mut table);
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
            assert_eq!(table.effects(id).pending, class, "{name}");
        }
    }

    /// Spot-check the rows against what the natives' bodies actually call on
    /// `PetalCxt`, so a row cannot quietly say less than the code does.
    #[test]
    fn rows_match_what_the_bodies_do() {
        let mut table = NativeFnTable::new();
        register_builtins(&mut table);
        let row = |n: &str| table.effects(table.lookup_name(n).unwrap());
        assert!(row("print").effect && !row("print").emits);
        assert!(row("push_output").emits && !row("push_output").effect);
        assert!(row("binding").probe && row("binding").reads.contains(InputClasses::BINDINGS));
        assert!(row("random").reads.contains(InputClasses::RNG) && !row("random").effect);
        assert!(row("noise_seed").effect);
        assert!(row("or_else").reads.contains(InputClasses::RESOURCES));
        assert!(row("__pending").effect);
        assert_eq!(row("sqrt"), NativeEffects::CONST);
        assert_eq!(row("Rect"), NativeEffects::CONST);
        assert_eq!(row("noise"), NativeEffects::PURE);
    }

    /// Every builtin whose row is `CONST`, pinned by name. A static value
    /// built from one of these is kept until the source changes, so a row
    /// joining this list is a correctness decision someone has to make on
    /// purpose: read the native's body, check that it reads nothing but its
    /// arguments, and add the name here.
    #[test]
    fn const_builtins_are_exactly() {
        use crate::native_fn::NativeConstness;
        let mut table = NativeFnTable::new();
        register_builtins(&mut table);
        let mut got: Vec<String> = (0..table.count() as u32)
            .map(crate::native_fn::NativeFnId)
            .filter(|id| table.constness(*id) == NativeConstness::Const)
            .map(|id| table.get_name(id).to_string())
            .collect();
        got.sort_unstable();
        let expected = [
            "Rect",
            "Rect.bottom",
            "Rect.center_x",
            "Rect.center_y",
            "Rect.inset",
            "Rect.offset",
            "Rect.right",
            "abs",
            "acos",
            "append",
            "asin",
            "assert",
            "assert_eq",
            "atan",
            "atan2",
            "byte_len",
            "byte_slice",
            "ceil",
            "char_at",
            "char_len",
            "char_slice",
            "chars",
            "clamp",
            "color_lerp",
            "commas",
            "concat",
            "contains",
            "cos",
            "cross",
            "degrees",
            "deriv_of",
            "distance",
            "dot",
            "drop_last",
            "dual",
            "ends_with",
            "enumerate",
            "exp",
            "f64_array",
            "fixed",
            "flat",
            "float",
            "floor",
            "format",
            "fract",
            "hsl",
            "hsl_deg",
            "hsv",
            "hsv_deg",
            "hypot",
            "includes",
            "index_of",
            "int",
            "join",
            "json_parse",
            "json_stringify",
            "keys",
            "last",
            "len",
            "lerp",
            "limit",
            "log",
            "lower",
            "mag",
            "map_range",
            "max",
            "min",
            "normalize",
            "pad_end",
            "pad_start",
            "parse_float",
            "parse_int",
            "pi",
            "pop",
            "pow",
            "prepend",
            "push",
            "radians",
            "range",
            "remove",
            "repeat",
            "reverse",
            "rotate",
            "round",
            "safe_div",
            "set_at",
            "sign",
            "sin",
            "slice",
            "smoothstep",
            "split",
            "sqrt",
            "starts_with",
            "str",
            "swap",
            "symbol",
            "tan",
            "trim",
            "type",
            "upper",
            "value_of",
            "values",
            "vec2",
            "vec3",
            "zip",
        ];
        assert_eq!(got, expected);
    }

    /// The pure rows that are not constant — the list the static-values
    /// proposal names, each of which would be a stale value if it were.
    #[test]
    fn pure_is_not_const() {
        use crate::native_fn::NativeConstness;
        let mut table = NativeFnTable::new();
        register_builtins(&mut table);
        for name in [
            "noise",
            "is_valid",
            "fn_info",
            "fn_ast",
            crate::program::ENUM_FROM_BUILTIN,
            crate::program::ENUM_ALL_BUILTIN,
            "forEach",
        ] {
            let id = table.lookup_name(name).unwrap();
            assert_eq!(table.effects(id), NativeEffects::PURE, "{name}");
            assert!(!table.effects(id).is_const(), "{name}");
            assert_eq!(table.constness(id), NativeConstness::Dynamic, "{name}");
        }
        // Nothing that reads, emits or has an effect is constant either.
        for name in [
            "random",
            "choose",
            "binding",
            "print",
            "push_output",
            "noise_seed",
            "is_loading",
        ] {
            assert_eq!(
                table.constness_of_name(name),
                NativeConstness::Dynamic,
                "{name}"
            );
        }
        assert_eq!(
            table.constness_of_name("no_such_native"),
            NativeConstness::Dynamic
        );
    }

    /// The higher-order builtins are constant only as far as their callbacks
    /// are, and never plain `Const` — a caller that forgets to look at the
    /// callback must not get a yes.
    #[test]
    fn higher_order_builtins_defer_to_their_callbacks() {
        use crate::native_fn::NativeConstness;
        let mut table = NativeFnTable::new();
        register_builtins(&mut table);
        for name in ["map", "filter", "reduce", "sort", "sort_by"] {
            let id = table.lookup_name(name).unwrap();
            assert!(!table.effects(id).is_const(), "{name}");
            assert_eq!(
                table.constness(id),
                NativeConstness::ConstIfCallbacksAre,
                "{name}"
            );
        }
    }

    /// The name-keyed effect table (`builtins::effects`) and the rows agree
    /// where they overlap: a native that table calls pure has a `CONST` row,
    /// `sort` (whose comparator form runs user code) aside. A tripwire for a
    /// new builtin listed pure by name whose row nobody audited — if it is
    /// genuinely pure-but-not-constant, name it here with the reason.
    #[test]
    fn pure_by_name_natives_have_const_rows() {
        let mut table = NativeFnTable::new();
        register_builtins(&mut table);
        for id in (0..table.count() as u32).map(crate::native_fn::NativeFnId) {
            let name = table.get_name(id);
            if is_pure_builtin(name) && !table.effects(id).is_const() {
                assert_eq!(
                    name, "sort",
                    "`{name}` is listed pure by name but its row is not CONST"
                );
            }
        }
    }
}
