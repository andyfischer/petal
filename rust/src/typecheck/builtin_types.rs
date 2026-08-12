//! Declared result types for the builtin functions.
//!
//! The runtime registers builtins as plain natives with no static signature, so
//! this table is the only place the checker learns that `len(xs)` is an `int`
//! and `sqrt(x)` is a `float`. It is consulted *last* — after a local binding,
//! after a `class` constructor of that name, and after the module's own `fn`
//! signatures — so a user function or class named `len` shadows the entry here,
//! the same precedence the runtime uses. (The `int`/`float`/`str` casts that
//! [`super::Checker::check_call`] answers ahead of all of that are safe from a
//! class: those are built-in *type* names, which a class may not take — see
//! [`crate::classes::ClassTable::declare`].)
//!
//! Two rules govern what may be listed:
//!
//! 1. **Only certainties.** A wrong entry becomes a false type warning, or —
//!    via [`crate::lint`]'s redundant-cast rule — a wrong rewrite. Builtins
//!    whose result type depends on the argument's *runtime* type (`reverse`
//!    and `slice` return a list or a string; `choose`, `get`, `last` return an
//!    element) are deliberately absent, so they infer `Any`.
//! 2. **Argument-dependent results are computed, not assumed.** `abs`, `floor`,
//!    `ceil`, `round` and `sign` preserve int-ness (`rust/src/builtins/math.rs`,
//!    `unary_num_preserving`); `min`/`max` return one of their operands
//!    unchanged, and `clamp` is int for an all-int call. Those are handled by
//!    the match arms below rather than a fixed type.
//!
//! Host builtins registered by an embedding (the `petal-ui` drawing/input set)
//! are listed too. They are not part of the core runtime, so an embedder
//! *could* register a different `screen_width` — but every consumer of this
//! table is advisory (warnings) or gated (lint re-compiles its output), and the
//! UI set is the one every sample app and editor script is written against.

use crate::types::Type;

/// The result type of a call to builtin `name` with arguments of type
/// `args`, or `None` when the builtin is unknown or its result type isn't
/// statically decidable (which callers read as [`Type::Any`]).
pub fn builtin_return_type(name: &str, args: &[Type]) -> Option<Type> {
    // Argument-dependent results first.
    match name {
        // `unary_num_preserving`: Int -> Int, Float -> Float, Dual -> Dual.
        "abs" | "floor" | "ceil" | "sign" => {
            return match args {
                [Type::Int] => Some(Type::Int),
                [Type::Float] => Some(Type::Float),
                _ => None,
            };
        }
        // `round` is int-preserving in both its arities: `round(x, places)`
        // rounds to `places` decimals and keeps the argument's kind.
        "round" => {
            return match args {
                [Type::Int] | [Type::Int, _] => Some(Type::Int),
                [Type::Float] | [Type::Float, _] => Some(Type::Float),
                _ => None,
            };
        }
        // `clamp` returns one of its three arguments' kinds: all-int stays int
        // (so a clamped index or `range` bound is still usable as one), and any
        // float argument makes the result float, as `+` does.
        "clamp" => {
            return match args {
                [a, b, c] if [a, b, c].iter().all(|t| **t == Type::Int) => Some(Type::Int),
                [a, b, c]
                    if [a, b, c]
                        .iter()
                        .all(|t| matches!(t, Type::Int | Type::Float)) =>
                {
                    Some(Type::Float)
                }
                _ => None,
            };
        }
        // Return whichever operand compares smaller/larger, unchanged — so the
        // result type is known only when both operands agree.
        "min" | "max" => {
            return match args {
                [a, b] if a == b && *a != Type::Any => Some(*a),
                _ => None,
            };
        }
        // `unary_float_dual`: a Dual argument propagates as a Dual, so these
        // are only known-Float for a statically numeric argument.
        "sqrt" | "sin" | "cos" | "tan" => {
            return match args {
                [Type::Int | Type::Float] => Some(Type::Float),
                _ => None,
            };
        }
        _ => {}
    }

    let ty = match name {
        // ── core: int results ───────────────────────────────────────────────
        "int" | "len" | "random_int" | "char_len" | "index_of" => Type::Int,
        // ── core: float results ─────────────────────────────────────────────
        // `float` is `unary_float_dual` like `sqrt` above, but it is also the
        // sanctioned cast: `float(x)` is written precisely to leave the float
        // domain, and treating it as anything but `float` would defeat every
        // annotation that uses it.
        "float" | "atan2" | "pi" | "random" | "lerp" | "map_range" | "distance" | "mag" | "pow"
        | "fract" | "smoothstep" | "radians" | "degrees" | "exp" | "log" | "dot" => Type::Float,
        // ── core: string results ────────────────────────────────────────────
        // `format`/`fixed`/`commas`/`pad_*` render *into* a string, so unlike
        // `concat` (list or string, decided by its arguments) their result is
        // known statically.
        "str" | "type" | "join" | "upper" | "lower" | "char_at" | "char_slice" | "fixed"
        | "commas" | "pad_start" | "pad_end" | "format" => Type::String,
        // ── core: bool results ──────────────────────────────────────────────
        "contains" | "includes" | "is_loading" | "is_error" | "is_pending" | "is_ready" => {
            Type::Bool
        }
        // ── core: list results ──────────────────────────────────────────────
        "range" | "keys" | "values" | "split" | "enumerate" | "zip" | "flat" | "sort"
        | "sort_by" | "prepend" | "chars" => Type::List,
        // ── core: record / vec2 results ─────────────────────────────────────
        "hsv" | "hsl" | "hsv_deg" | "hsl_deg" | "color_lerp" => Type::Record,
        "vec2" | "normalize" | "limit" => Type::Vec2,
        "f64_array" => Type::F64Array,
        "symbol" => Type::Symbol,

        // ── host (petal-ui): int results ────────────────────────────────────
        "screen_width" | "screen_height" | "mouse_x" | "mouse_y" | "mouse_dx" | "mouse_dy"
        | "scroll_x" | "scroll_y" | "drag_start_x" | "drag_start_y" | "click_count"
        | "frame_count" | "text_width" | "ui_version" => Type::Int,
        // ── host (petal-ui): float results ──────────────────────────────────
        "dt" | "time" => Type::Float,
        // ── host (petal-ui): bool results ───────────────────────────────────
        "mouse_down" | "mouse_pressed" | "mouse_released" | "key_down" | "key_pressed"
        | "key_released" | "mod_shift" | "mod_ctrl" | "mod_alt" | "mod_cmd" | "drag_active" => {
            Type::Bool
        }
        // ── host (petal-ui): string results ─────────────────────────────────
        "text_input" => Type::String,

        _ => return None,
    };
    Some(ty)
}

#[cfg(test)]
mod tests {
    use super::builtin_return_type;
    use crate::types::Type;

    #[test]
    fn fixed_results() {
        assert_eq!(builtin_return_type("len", &[Type::List]), Some(Type::Int));
        assert_eq!(builtin_return_type("str", &[Type::Int]), Some(Type::String));
        assert_eq!(builtin_return_type("range", &[Type::Int]), Some(Type::List));
        assert_eq!(builtin_return_type("banana", &[]), None);
    }

    /// `clamp` preserves int-ness the way `min`/`max` do: an all-int clamp is
    /// still an int, so `xs[clamp(i, 0, len(xs) - 1)]` type-checks as an index.
    #[test]
    fn clamp_preserves_int_ness() {
        assert_eq!(
            builtin_return_type("clamp", &[Type::Int, Type::Int, Type::Int]),
            Some(Type::Int)
        );
        assert_eq!(
            builtin_return_type("clamp", &[Type::Float, Type::Int, Type::Int]),
            Some(Type::Float)
        );
        assert_eq!(
            builtin_return_type("clamp", &[Type::Any, Type::Int, Type::Int]),
            None
        );
    }

    /// The two-argument `round(x, places)` keeps its argument's kind too.
    #[test]
    fn round_with_places_follows_its_argument() {
        assert_eq!(
            builtin_return_type("round", &[Type::Float, Type::Int]),
            Some(Type::Float)
        );
        assert_eq!(
            builtin_return_type("round", &[Type::Int, Type::Int]),
            Some(Type::Int)
        );
    }

    /// `parse_float`/`parse_int` answer `nil` on bad input, so their result is
    /// not statically a number and they must stay unlisted.
    #[test]
    fn failable_parsers_are_absent() {
        assert_eq!(builtin_return_type("parse_float", &[Type::String]), None);
        assert_eq!(builtin_return_type("parse_int", &[Type::String]), None);
    }

    #[test]
    fn character_indexed_string_ops() {
        assert_eq!(
            builtin_return_type("char_len", &[Type::String]),
            Some(Type::Int)
        );
        assert_eq!(
            builtin_return_type("chars", &[Type::String]),
            Some(Type::List)
        );
        assert_eq!(
            builtin_return_type("char_at", &[Type::String, Type::Int]),
            Some(Type::String)
        );
        assert_eq!(
            builtin_return_type("index_of", &[Type::String, Type::String]),
            Some(Type::Int)
        );
    }

    #[test]
    fn int_preserving_unaries_follow_their_argument() {
        assert_eq!(builtin_return_type("round", &[Type::Int]), Some(Type::Int));
        assert_eq!(
            builtin_return_type("round", &[Type::Float]),
            Some(Type::Float)
        );
        // An unknown argument could be a dual number: infer nothing.
        assert_eq!(builtin_return_type("round", &[Type::Any]), None);
    }

    #[test]
    fn min_max_need_agreeing_operands() {
        assert_eq!(
            builtin_return_type("max", &[Type::Int, Type::Int]),
            Some(Type::Int)
        );
        assert_eq!(builtin_return_type("max", &[Type::Int, Type::Float]), None);
        assert_eq!(builtin_return_type("max", &[Type::Any, Type::Any]), None);
    }

    #[test]
    fn dual_capable_floats_need_a_numeric_argument() {
        assert_eq!(
            builtin_return_type("sqrt", &[Type::Float]),
            Some(Type::Float)
        );
        assert_eq!(builtin_return_type("sqrt", &[Type::Any]), None);
    }

    /// The formatting builtins all answer a string, and `sort_by` a list.
    #[test]
    fn formatting_and_sorting_results() {
        assert_eq!(
            builtin_return_type("fixed", &[Type::Float, Type::Int]),
            Some(Type::String)
        );
        assert_eq!(
            builtin_return_type("commas", &[Type::Int]),
            Some(Type::String)
        );
        assert_eq!(
            builtin_return_type("format", &[Type::String, Type::Float]),
            Some(Type::String)
        );
        assert_eq!(
            builtin_return_type("sort_by", &[Type::List, Type::Any]),
            Some(Type::List)
        );
    }

    /// Builtins whose result type is decided at runtime must stay unlisted.
    /// `concat` joins two lists *or* two strings, and `safe_div` answers nil on
    /// a zero divisor, so neither has a static result type.
    #[test]
    fn runtime_dependent_builtins_are_absent() {
        for name in [
            "reverse", "slice", "choose", "last", "first", "sum", "concat", "safe_div",
        ] {
            assert_eq!(builtin_return_type(name, &[Type::List]), None, "{name}");
        }
    }
}
