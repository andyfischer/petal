//! What the checker knows about natives that a declaration cannot say.
//!
//! A native's parameter names, arity, parameter types and result type are
//! written in its `native fn` declaration ([`crate::native_decl`]): the
//! bundled `core` package for the builtins, a host's interface module for its
//! own. The checker reads those. What is left here is the part a signature
//! has no way to spell:
//!
//! 1. **Argument-dependent results** ([`intrinsic_return_type`]). `abs`,
//!    `floor`, `ceil`, `round` and `sign` preserve int-ness
//!    (`core/src/builtins/math.rs`, `unary_num_preserving`); `min`/`max`
//!    return one of their operands unchanged; `clamp` is int for an all-int
//!    call; the float functions answer a dual for a dual; the vector natives
//!    answer in the kind they were given.
//! 2. **Argument slots with no type name** ([`intrinsic_param_slots`]):
//!    `len` measures a list, a string or an `f64_array`, and there is no type
//!    that says "one of those".
//! 3. [`ArgSlot`] itself: how a declared parameter type is held against an
//!    argument, and worded when it does not fit.
//!
//! The rule for all of it is **only certainties**. A wrong entry becomes a
//! false type warning, or — via [`crate::lint`]'s redundant-cast rule — a
//! wrong rewrite. The same rule governs what a declaration should annotate: a
//! parameter type is a claim that the native **fails at runtime** on anything
//! else, and a result type that it never answers anything else (`reverse` and
//! `slice` return a list or a string, `choose` and `last` an element, the
//! `parse_*` pair may answer nil — all un-annotated, so they infer `any`).

use std::borrow::Cow;

use crate::types::Type;

/// The result type of a call to native `name` (its *host* name) with
/// arguments of type `args`, when that result depends on the arguments.
///
/// `None` — this native has no such rule; its declaration's result type
/// stands. `Some(None)` — it has one, and these arguments do not decide it
/// (which callers read as [`Type::Any`]). `Some(Some(ty))` — decided.
pub fn intrinsic_return_type(name: &str, args: &[Type]) -> Option<Option<Type>> {
    let decided = match name {
        // `unary_num_preserving`: Int -> Int, Float -> Float, Dual -> Dual.
        "abs" | "floor" | "ceil" | "sign" => match args {
            [Type::Int] => Some(Type::Int),
            [Type::Float] => Some(Type::Float),
            _ => None,
        },
        // `round` is int-preserving in both its arities: `round(x, places)`
        // rounds to `places` decimals and keeps the argument's kind.
        "round" => match args {
            [Type::Int] | [Type::Int, _] => Some(Type::Int),
            [Type::Float] | [Type::Float, _] => Some(Type::Float),
            _ => None,
        },
        // `clamp` returns one of its three arguments' kinds: all-int stays int
        // (so a clamped index or `range` bound is still usable as one), and any
        // float argument makes the result float, as `+` does.
        "clamp" => match args {
            [a, b, c] if [a, b, c].iter().all(|t| **t == Type::Int) => Some(Type::Int),
            [a, b, c]
                if [a, b, c]
                    .iter()
                    .all(|t| matches!(t, Type::Int | Type::Float)) =>
            {
                Some(Type::Float)
            }
            _ => None,
        },
        // Return whichever operand compares smaller/larger, unchanged — so the
        // result type is known only when both operands agree.
        "min" | "max" => match args {
            [a, b] if a == b && *a != Type::Any => Some(*a),
            _ => None,
        },
        // `unary_float_dual`: a Dual argument propagates as a Dual, so these
        // are only known-Float for a statically numeric argument.
        "sqrt" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" => match args {
            [Type::Int | Type::Float] => Some(Type::Float),
            _ => None,
        },
        // The vector natives take a vec2 or a vec3 and hand back the same
        // kind (`core/src/builtins/vec2.rs`).
        "normalize" | "limit" => match args.first() {
            Some(Type::Vec2) => Some(Type::Vec2),
            Some(Type::Vec3) => Some(Type::Vec3),
            _ => None,
        },
        // `lerp` interpolates numbers, or two vectors, colors or number lists
        // of the same kind.
        "lerp" => match args {
            [Type::Vec2, Type::Vec2, _] => Some(Type::Vec2),
            [Type::Vec3, Type::Vec3, _] => Some(Type::Vec3),
            [Type::Record, Type::Record, _] => Some(Type::Record),
            [Type::List, Type::List, _] => Some(Type::List),
            [Type::Int | Type::Float, Type::Int | Type::Float, _] => Some(Type::Float),
            _ => None,
        },
        _ => return None,
    };
    Some(decided)
}

/// The argument slots of native `name` (its host name) that no parameter
/// type can spell, for a call of `arity` arguments. Consulted ahead of the
/// declaration's own parameter types.
pub fn intrinsic_param_slots(name: &str, arity: usize) -> Option<&'static [ArgSlot]> {
    match (name, arity) {
        ("len", 1) => Some(&[ArgSlot::Sized]),
        _ => None,
    }
}

/// The result type of a call to the native declared as `decl`, with the
/// arguments of type `args` in parameter order: the argument-dependent rule
/// when the native has one ([`intrinsic_return_type`]), else what its
/// declaration says. `None` reads as [`Type::Any`].
pub fn native_return_type(decl: &crate::native_decl::NativeDecl, args: &[Type]) -> Option<Type> {
    match intrinsic_return_type(&decl.host, args) {
        Some(decided) => decided,
        None => decl.return_type(args.len()),
    }
}

/// The argument slots of a call of `arity` arguments to the native declared
/// as `decl`: [`intrinsic_param_slots`] when the native has such a rule, else
/// its declared parameter types ([`ArgSlot::of`]). Positions past the fixed
/// parameters (a `...rest`) are unchecked.
pub fn native_param_slots(decl: &crate::native_decl::NativeDecl, arity: usize) -> Vec<ArgSlot> {
    match intrinsic_param_slots(&decl.host, arity) {
        Some(slots) => slots.to_vec(),
        None => decl.param_slots(arity),
    }
}

/// What one argument position of a builtin accepts, for the call-site check
/// in [`super::Checker::check_call`]. Every slot also accepts [`Type::Any`]
/// (nothing is known) and [`Type::Pending`] (the native-call boundary absorbs
/// a pending argument before the native sees it), so only a statically known,
/// definitely-wrong type ever warns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgSlot {
    /// Unchecked.
    Any,
    /// `PetalCxt::get_float`/`get_int`: an int, a float, or a dual number.
    /// (`get_int` refuses a dual, but a dual is never inferred, and accepting
    /// one keeps the slot sound.)
    Num,
    /// `PetalCxt::get_string`.
    Str,
    /// A list.
    List,
    /// A record, which a class instance is at runtime.
    Record,
    /// Anything `len` measures: a list, a string, an `f64_array`.
    Sized,
    /// A callable — the closure a higher-order intrinsic drives.
    Function,
    /// Exactly this declared type (`x: bool`, `v: vec2`), by the rule an
    /// annotated `fn` parameter is held to ([`Type::is_assignable_to`]).
    Exact(Type),
}

impl ArgSlot {
    /// The slot a parameter declared `: ty` on a `native fn` is. The types
    /// with a looser runtime reading keep it: a `num` slot takes a dual (the
    /// native reads its primal), a `record` slot a class instance.
    pub fn of(ty: Type) -> ArgSlot {
        match ty {
            Type::Any => ArgSlot::Any,
            Type::Num => ArgSlot::Num,
            Type::String => ArgSlot::Str,
            Type::List => ArgSlot::List,
            Type::Record => ArgSlot::Record,
            Type::Function => ArgSlot::Function,
            other => ArgSlot::Exact(other),
        }
    }

    /// Whether an argument of static type `ty` can fill this slot.
    pub fn accepts(self, ty: Type) -> bool {
        if matches!(ty, Type::Any | Type::Pending) {
            return true;
        }
        match self {
            ArgSlot::Any => true,
            ArgSlot::Num => matches!(ty, Type::Int | Type::Float | Type::Num | Type::Dual),
            ArgSlot::Str => ty == Type::String,
            ArgSlot::List => ty == Type::List,
            ArgSlot::Record => matches!(ty, Type::Record | Type::Class(_)),
            ArgSlot::Sized => matches!(ty, Type::List | Type::String | Type::F64Array),
            ArgSlot::Function => ty == Type::Function,
            ArgSlot::Exact(want) => ty.is_assignable_to(&want),
        }
    }

    /// How the slot reads in a diagnostic.
    pub fn describe(self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            ArgSlot::Exact(ty) => return Cow::Owned(format!("`{}`", ty.name())),
            ArgSlot::Any => "any",
            ArgSlot::Num => "a number",
            ArgSlot::Str => "a string",
            ArgSlot::List => "a list",
            ArgSlot::Record => "a record",
            ArgSlot::Sized => "a list or string",
            ArgSlot::Function => "a function",
        })
    }
}

/// Whether `sort_by` can order keys of static type `ty`: it compares numbers
/// and strings and fails at runtime on anything else
/// (`Vm::builtin_sort_by`). `any` is unknown rather than wrong, and a pending
/// key makes the result pending instead of failing.
pub fn sort_key_type_ok(ty: Type) -> bool {
    matches!(
        ty,
        Type::Any
            | Type::Pending
            | Type::Int
            | Type::Float
            | Type::Num
            | Type::Dual
            | Type::String
    )
}

#[cfg(test)]
mod tests {
    use super::{ArgSlot, intrinsic_return_type, native_param_slots, native_return_type};
    use crate::native_decl::{NativeDecls, builtin_decls, core_decls};
    use crate::types::Type;

    /// What the checker concludes for a call to builtin `name` (`core`'s, or
    /// a domain builtin package's where it is imported).
    fn builtin_return_type(name: &str, args: &[Type]) -> Option<Type> {
        native_return_type(builtin_decls().get(name)?, args)
    }

    #[test]
    fn fixed_results_come_from_the_declarations() {
        assert_eq!(builtin_return_type("len", &[Type::List]), Some(Type::Int));
        assert_eq!(builtin_return_type("str", &[Type::Int]), Some(Type::String));
        assert_eq!(builtin_return_type("range", &[Type::Int]), Some(Type::List));
        assert_eq!(builtin_return_type("banana", &[]), None);
        assert_eq!(
            builtin_return_type("rotate", &[Type::Vec2, Type::Float]),
            Some(Type::Vec2)
        );
        assert_eq!(
            builtin_return_type("vec3", &[Type::Float, Type::Float, Type::Float]),
            Some(Type::Vec3)
        );
        assert_eq!(
            builtin_return_type("cross", &[Type::Vec3, Type::Vec3]),
            Some(Type::Vec3)
        );
    }

    /// A host's interface module says the same things about its natives.
    #[test]
    fn a_declared_host_native_has_a_result_and_slots() {
        let decls = NativeDecls::parse(
            "native fn text_width(text: string, style) -> int\n\
             native fn text_width(text: string, size: num, font?: string) -> int\n\
             native fn text_advance(text: string, style) -> float\n\
             native fn flag(on: bool)",
        )
        .unwrap();
        let width = decls.get("text_width").unwrap();
        assert_eq!(native_return_type(width, &[Type::String, Type::Int]), Some(Type::Int));
        assert_eq!(native_param_slots(width, 2), [ArgSlot::Str, ArgSlot::Any]);
        assert_eq!(
            native_param_slots(width, 3),
            [ArgSlot::Str, ArgSlot::Num, ArgSlot::Str]
        );
        let advance = decls.get("text_advance").unwrap();
        assert_eq!(native_return_type(advance, &[Type::String, Type::Any]), Some(Type::Float));
        let flag = decls.get("flag").unwrap();
        assert_eq!(native_param_slots(flag, 1), [ArgSlot::Exact(Type::Bool)]);
        assert!(ArgSlot::Exact(Type::Bool).accepts(Type::Bool));
        assert!(!ArgSlot::Exact(Type::Bool).accepts(Type::Int));
        assert_eq!(ArgSlot::Exact(Type::Bool).describe(), "`bool`");
    }

    /// `len` takes anything it can measure, which no type name says.
    #[test]
    fn len_keeps_its_sized_slot() {
        let len = core_decls().get("len").unwrap();
        assert_eq!(native_param_slots(len, 1), [ArgSlot::Sized]);
    }

    /// The vector natives answer in the kind they were given.
    #[test]
    fn vector_natives_follow_their_argument() {
        for v in [Type::Vec2, Type::Vec3] {
            assert_eq!(builtin_return_type("normalize", &[v]), Some(v));
            assert_eq!(builtin_return_type("limit", &[v, Type::Float]), Some(v));
            assert_eq!(builtin_return_type("lerp", &[v, v, Type::Float]), Some(v));
        }
        assert_eq!(builtin_return_type("normalize", &[Type::Any]), None);
        assert_eq!(
            builtin_return_type("lerp", &[Type::Float, Type::Int, Type::Float]),
            Some(Type::Float)
        );
        assert_eq!(builtin_return_type("lerp", &[Type::Any, Type::Any, Type::Float]), None);
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
    /// not statically a number and they must stay un-annotated.
    #[test]
    fn failable_parsers_declare_no_result() {
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
        assert_eq!(intrinsic_return_type("round", &[Type::Any]), Some(None));
        assert_eq!(intrinsic_return_type("len", &[Type::List]), None);
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

    /// Builtins whose result type is decided at runtime must stay
    /// un-annotated. `concat` joins two lists *or* two strings, and `safe_div`
    /// answers nil on a zero divisor, so neither has a static result type.
    #[test]
    fn runtime_dependent_builtins_declare_no_result() {
        for name in [
            "reverse", "slice", "choose", "last", "first", "sum", "concat", "safe_div",
        ] {
            assert_eq!(builtin_return_type(name, &[Type::List]), None, "{name}");
        }
    }
}
