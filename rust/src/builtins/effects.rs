//! Builtin effect metadata — what each builtin does to the values it is handed.
//!
//! [`BUILTIN_EFFECTS`] is the single source of truth: one row per builtin,
//! carrying every effect property anything in the compiler wants to know about
//! it. Adding a builtin to an analysis means editing exactly one row. The
//! predicate functions below are the public API — nothing outside this module
//! reads the table or the flags directly.
//!
//! The soundness obligation each property carries is documented on its
//! predicate. Read it before adding a flag to a row: the backend in-place
//! analyses (`backend::bytecode::escape`, `::lastuse`) treat these answers as
//! facts, and a wrong one corrupts data rather than pessimizing it.

/// A builtin's effect properties, as a bitmask of the flags below.
type Effects = u8;

/// The container is the first argument and the result is the new container.
/// Documented in full on [`is_mutating_builtin`].
const MUTATES: Effects = 1 << 0;

/// The result is a freshly allocated, unaliased container.
/// Documented in full on [`returns_fresh_container`].
const FRESH: Effects = 1 << 1;

/// Nothing that outlives the call shares a backing store with an argument.
/// Documented in full on [`retains_no_reference`].
const NO_REF: Effects = 1 << 2;

/// The call's only effect is the value it returns.
/// Documented in full on [`is_pure_builtin`].
const PURE: Effects = 1 << 3;

/// The statement form reads like an in-place mutation but is not.
/// Documented in full on [`looks_mutating`]. Unlike the flags above this one is
/// a lint-wording concern, not a soundness property.
const LOOKS_MUT: Effects = 1 << 4;

/// Expand one list of rows into both the [`BUILTIN_EFFECTS`] table (which the
/// tests enumerate) and [`effects_of`]'s lookup. Writing the names twice would
/// defeat the point of the table, and a linear scan over it would be a
/// regression: the predicates are called per-term by the backend in-place
/// analyses, where the `matches!` trees they replaced compiled to a decision
/// tree. A `match` over string literals gets that back — the compiler is free
/// to switch on length and prefix instead of comparing every name in turn.
macro_rules! builtin_effects {
    ($(($name:literal, $flags:expr)),* $(,)?) => {
        /// Every builtin with an effect property worth recording, listed once.
        /// A builtin absent from this table has no property set: every
        /// predicate answers `false`, the conservative answer for all of them.
        ///
        /// The rows reach the predicates through [`effects_of`]'s `match`, not
        /// through this slice — it exists so the tests can enumerate the table
        /// and pin each predicate's exact membership.
        #[cfg_attr(not(test), allow(dead_code))]
        const BUILTIN_EFFECTS: &[(&str, Effects)] = &[$(($name, $flags)),*];

        /// The effect flags recorded for `name`, or none if it has no row.
        fn effects_of(name: &str) -> Effects {
            match name {
                $($name => $flags,)*
                _ => 0,
            }
        }
    };
}

builtin_effects![
    // collections (value-semantic: return a new collection, mutate nothing)
    ("range", FRESH | PURE),
    ("len", NO_REF | PURE),
    ("push", MUTATES | PURE | LOOKS_MUT),
    ("append", MUTATES | PURE | LOOKS_MUT),
    ("pop", MUTATES | PURE | LOOKS_MUT),
    ("keys", FRESH | NO_REF | PURE),
    ("values", FRESH | NO_REF | PURE),
    ("contains", NO_REF | PURE),
    ("includes", NO_REF | PURE),
    ("sort", FRESH | NO_REF | PURE | LOOKS_MUT),
    ("reverse", FRESH | NO_REF | PURE | LOOKS_MUT),
    ("join", NO_REF | PURE),
    ("split", NO_REF | PURE),
    ("enumerate", FRESH | NO_REF | PURE),
    ("zip", FRESH | NO_REF | PURE),
    ("slice", FRESH | NO_REF | PURE),
    ("flat", FRESH | NO_REF | PURE),
    ("last", NO_REF | PURE),
    ("drop_last", MUTATES | PURE | LOOKS_MUT),
    ("remove", MUTATES | PURE | LOOKS_MUT),
    ("get", NO_REF | PURE),
    ("set_at", MUTATES | PURE | LOOKS_MUT),
    ("swap", MUTATES | PURE | LOOKS_MUT),
    ("f64_array", FRESH | PURE),
    ("first", PURE),
    ("is_empty", PURE),
    ("take", PURE),
    ("drop", PURE),
    // math / numeric
    ("abs", PURE),
    ("sqrt", PURE),
    ("floor", PURE),
    ("ceil", PURE),
    ("float", PURE),
    ("int", PURE),
    ("min", PURE),
    ("max", PURE),
    ("round", PURE),
    ("sin", PURE),
    ("cos", PURE),
    ("tan", PURE),
    ("atan2", PURE),
    ("pi", PURE),
    ("pow", PURE),
    ("sign", PURE),
    ("fract", PURE),
    ("exp", PURE),
    ("log", PURE),
    ("clamp", PURE),
    ("clamp01", PURE),
    ("lerp", PURE),
    ("map_range", PURE),
    ("distance", PURE),
    ("mag", PURE),
    ("smoothstep", PURE),
    ("radians", PURE),
    ("degrees", PURE),
    ("sum", PURE),
    ("product", PURE),
    ("mean", PURE),
    ("minimum", PURE),
    ("maximum", PURE),
    // conversion / reflection
    ("str", NO_REF | PURE),
    ("type", NO_REF | PURE),
    // color
    ("hsv", PURE),
    ("hsl", PURE),
    ("color_lerp", PURE),
    ("hsv_deg", PURE),
    ("hsl_deg", PURE),
    // vec2
    ("vec2", PURE),
    ("normalize", PURE),
    ("dot", PURE),
    ("limit", PURE),
    // autodiff (pure readers)
    ("value_of", PURE),
    ("deriv_of", PURE),
    // effectful, but retains nothing: `print` formats its argument immediately.
    ("print", NO_REF),
];

/// The mutating builtins: those whose container is their first argument and
/// whose result is the new (or in-place-updated) container. This is the single
/// source of truth shared by the two backend in-place analyses
/// (`backend::bytecode::escape` and `::lastuse`) and the `PetalCxt::in_place`
/// consumers in [`super::collections`].
pub fn is_mutating_builtin(name: &str) -> bool {
    effects_of(name) & MUTATES != 0
}

/// The builtins that return a **freshly allocated, unaliased container**: the
/// returned id is created by that call, is not reachable from any pre-existing
/// value, and no reference to it is retained anywhere else (not in the heap, not
/// in the arguments, not in the native's own state). Each call therefore hands
/// its caller sole ownership of the result.
///
/// This is the property `backend::bytecode::escape` needs to let a call result
/// *root* a unique value-web — `f64_array(n)` is the only way to construct an
/// f64 array, so without it no f64-array write can ever be in place. Every
/// [`FRESH`] row allocates its result with a fresh `alloc_list` /
/// `alloc_f64_array`
/// on every path that yields a container (the absorb-a-`Pending` paths in
/// `sort`/`join` return a `Pending` scalar, not a container, so they are not a
/// counterexample). Elements *inside* the result may alias existing values; that
/// is fine, since an in-place write replaces a slot in the fresh outer store and
/// never touches an element's own store.
///
/// Set [`FRESH`] on a row only after checking the native for a path that
/// returns an argument's id unchanged — such a builtin would make the caller's
/// "unique owner" assumption false and silently corrupt data.
pub fn returns_fresh_container(name: &str) -> bool {
    effects_of(name) & FRESH != 0
}

/// The builtins that **retain no reference to any argument**: after the call
/// returns, nothing reachable from the result — or from anywhere else in the
/// program — shares a backing store with an argument the caller passed in.
///
/// This is the property `backend::bytecode::escape` needs to let a term *observe*
/// a container that is being mutated in place without breaking uniqueness:
/// `len(xs)` yields an int, `get(a, i)` a float, `sort(xs)` a brand-new list.
/// Sharing element *ids* with the argument is fine and is why the transforms
/// qualify: an in-place write replaces a slot in the argument's own store and
/// never touches an element's separate store.
///
/// Two exclusions worth naming, because they look like they belong:
/// `min`/`max` return *one of their arguments* — handing back the container id
/// itself — and `push_output` parks the value in an output buffer the host reads
/// after the run. Both retain. Check for those two shapes before setting
/// [`NO_REF`] on a row.
pub fn retains_no_reference(name: &str) -> bool {
    effects_of(name) & NO_REF != 0
}

/// Builtins whose only effect is the value they return — discarding that value
/// makes the call dead. Deliberately excludes every effectful or
/// closure-invoking native (`print`, `draw_*`, `random`, `assert`, host input
/// readers, `map`/`filter`/`reduce`/`forEach`), which is what keeps
/// `typecheck::unused` from warning about calls that do work.
pub fn is_pure_builtin(name: &str) -> bool {
    effects_of(name) & PURE != 0
}

/// The value-semantic collection ops whose statement form reads like an
/// in-place mutation but is not — worth a targeted "capture the result" hint.
///
/// This is a message-selection concern in `typecheck::unused` (which of two
/// wordings to use), not a soundness property: getting it wrong changes the
/// prose of a warning and nothing else. It is a superset of
/// [`is_mutating_builtin`] because `sort`/`reverse` read as mutations to a
/// newcomer without being in-place candidates in the backend.
pub fn looks_mutating(name: &str) -> bool {
    effects_of(name) & LOOKS_MUT != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name any effect predicate has ever answered `true` for. Filtering
    /// it through a predicate reconstructs that predicate's exact membership,
    /// which is what the assertions below pin: this is the regression guard for
    /// edits to [`BUILTIN_EFFECTS`], so a row that gains or loses a flag has to
    /// change an expected list here too.
    fn universe() -> Vec<&'static str> {
        BUILTIN_EFFECTS.iter().map(|(n, _)| *n).collect()
    }

    fn accepted(pred: fn(&str) -> bool) -> Vec<&'static str> {
        let mut names: Vec<&str> = universe().into_iter().filter(|n| pred(n)).collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn every_builtin_is_listed_once() {
        let mut names = universe();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate row in BUILTIN_EFFECTS");
    }

    #[test]
    fn mutating_builtins_are_exactly() {
        assert_eq!(
            accepted(is_mutating_builtin),
            [
                "append",
                "drop_last",
                "pop",
                "push",
                "remove",
                "set_at",
                "swap"
            ]
        );
    }

    #[test]
    fn fresh_container_builtins_are_exactly() {
        assert_eq!(
            accepted(returns_fresh_container),
            [
                "enumerate",
                "f64_array",
                "flat",
                "keys",
                "range",
                "reverse",
                "slice",
                "sort",
                "values",
                "zip"
            ]
        );
    }

    #[test]
    fn reference_free_builtins_are_exactly() {
        assert_eq!(
            accepted(retains_no_reference),
            [
                "contains",
                "enumerate",
                "flat",
                "get",
                "includes",
                "join",
                "keys",
                "last",
                "len",
                "print",
                "reverse",
                "slice",
                "sort",
                "split",
                "str",
                "type",
                "values",
                "zip"
            ]
        );
    }

    #[test]
    fn pure_builtins_are_exactly() {
        assert_eq!(
            accepted(is_pure_builtin),
            [
                "abs",
                "append",
                "atan2",
                "ceil",
                "clamp",
                "clamp01",
                "color_lerp",
                "contains",
                "cos",
                "degrees",
                "deriv_of",
                "distance",
                "dot",
                "drop",
                "drop_last",
                "enumerate",
                "exp",
                "f64_array",
                "first",
                "flat",
                "float",
                "floor",
                "fract",
                "get",
                "hsl",
                "hsl_deg",
                "hsv",
                "hsv_deg",
                "includes",
                "int",
                "is_empty",
                "join",
                "keys",
                "last",
                "len",
                "lerp",
                "limit",
                "log",
                "mag",
                "map_range",
                "max",
                "maximum",
                "mean",
                "min",
                "minimum",
                "normalize",
                "pi",
                "pop",
                "pow",
                "product",
                "push",
                "radians",
                "range",
                "remove",
                "reverse",
                "round",
                "set_at",
                "sign",
                "sin",
                "slice",
                "smoothstep",
                "sort",
                "split",
                "sqrt",
                "str",
                "sum",
                "swap",
                "take",
                "tan",
                "type",
                "value_of",
                "values",
                "vec2",
                "zip"
            ]
        );
    }

    #[test]
    fn looks_mutating_builtins_are_exactly() {
        assert_eq!(
            accepted(looks_mutating),
            [
                "append",
                "drop_last",
                "pop",
                "push",
                "remove",
                "reverse",
                "set_at",
                "sort",
                "swap"
            ]
        );
    }
}
