//! The parameter names of the core builtins, declared so a call can pass
//! arguments by name: `clamp(value: v, lo: 0, hi: 1)`,
//! `xs.slice(start: 1, end: 3)`.
//!
//! A native reads its arguments by index, so the names live here, next to the
//! registrations, rather than in each body. [`declare`] applies the table at
//! the end of [`super::register_builtins`]; from then on the registry is what
//! everything asks (`NativeFnTable::signatures`) — the VM, the compiler, and
//! `petal check`. `docs/Builtins.md` spells each signature with these names.
//!
//! Each entry is the builtin's name and one spec per call form (see
//! [`NativeSignature::parse`](crate::native_fn::NativeSignature::parse)): the
//! names in argument order, `?` on a trailing optional one. A builtin that
//! reads its arguments differently depending on how many it got (`random`,
//! `distance`, `mag`) lists each form; a named call takes the first form it
//! fits.
//!
//! The names are the ones `docs/Builtins.md` writes each signature with, and
//! the same role gets the same word everywhere (`list`, `collection`,
//! `string`, `value`, `x`; a function argument is `f`, or `pred` as in the
//! prelude's `find`/`any`/`all`). A keyword works — an argument label is parsed
//! like a record key, so `slice(xs, 1, end: 3)` is an ordinary call even though
//! no Petal `fn` could declare such a parameter — but `end` is the only one
//! used: editor grammars model a label as a plain identifier, so a keyword
//! label is best kept to where no other word will do.
//!
//! Left out on purpose: the variadic builtins (`print`, `format`), whose
//! arguments have no fixed roles, and the `__`-prefixed internals. They keep
//! refusing names. The test below holds every declared form to the arity the
//! native actually enforces.

use crate::native_fn::NativeFnTable;

/// `(builtin, one spec per call form)`.
pub const BUILTIN_PARAMS: &[(&str, &[&str])] = &[
    // --- I/O & types ---
    ("str", &["value"]),
    ("type", &["value"]),
    ("assert", &["condition, message?"]),
    ("assert_eq", &["actual, expected"]),
    // --- Math ---
    ("abs", &["x"]),
    ("sqrt", &["x"]),
    ("floor", &["x"]),
    ("ceil", &["x"]),
    ("round", &["x, places?"]),
    ("float", &["value"]),
    ("int", &["value"]),
    ("parse_float", &["string"]),
    ("parse_int", &["string"]),
    ("random", &["", "max", "min, max"]),
    ("min", &["a, b"]),
    ("max", &["a, b"]),
    ("sin", &["x"]),
    ("cos", &["x"]),
    ("tan", &["x"]),
    ("asin", &["x"]),
    ("acos", &["x"]),
    ("atan", &["x"]),
    ("atan2", &["y, x"]),
    ("hypot", &["x, y"]),
    ("pi", &[""]),
    ("safe_div", &["a, b"]),
    // --- Creative-coding math ---
    ("clamp", &["value, lo, hi"]),
    ("lerp", &["a, b, t"]),
    ("map_range", &["value, in_lo, in_hi, out_lo, out_hi"]),
    ("distance", &["x1, y1, x2, y2", "v1, v2"]),
    ("mag", &["x, y, z?", "v"]),
    ("pow", &["base, exp"]),
    ("sign", &["x"]),
    ("fract", &["x"]),
    ("smoothstep", &["edge0, edge1, x"]),
    ("radians", &["degrees"]),
    ("degrees", &["radians"]),
    ("exp", &["x"]),
    ("log", &["x"]),
    // --- Noise & randomness ---
    ("noise", &["x, y?, z?"]),
    ("noise_seed", &["seed"]),
    ("random_int", &["lo, hi"]),
    ("choose", &["list"]),
    // --- Color ---
    ("hsv", &["h, s, v"]),
    ("hsl", &["h, s, l"]),
    ("hsv_deg", &["h, s, v"]),
    ("hsl_deg", &["h, s, l"]),
    ("color_lerp", &["c1, c2, t"]),
    // --- Vectors ---
    ("vec2", &["x, y"]),
    ("vec3", &["x, y, z"]),
    ("normalize", &["v"]),
    ("dot", &["a, b"]),
    ("cross", &["a, b"]),
    ("limit", &["v, max_mag"]),
    ("rotate", &["v, angle"]),
    // --- Collections ---
    ("range", &["end", "start, end, step?"]),
    ("len", &["collection"]),
    ("append", &["list, value"]),
    ("push", &["list, value"]),
    ("prepend", &["list, value"]),
    ("concat", &["a, b"]),
    ("pop", &["list"]),
    ("last", &["list"]),
    ("drop_last", &["list"]),
    ("remove", &["record, key"]),
    ("keys", &["record"]),
    ("values", &["record"]),
    ("contains", &["collection, needle"]),
    ("includes", &["collection, needle"]),
    ("index_of", &["collection, needle"]),
    ("sort", &["list, compare?"]),
    ("sort_by", &["list, key, descending?"]),
    ("reverse", &["collection"]),
    ("join", &["list, separator"]),
    ("split", &["string, separator"]),
    ("upper", &["string"]),
    ("lower", &["string"]),
    ("enumerate", &["list"]),
    ("zip", &["list_a, list_b"]),
    ("slice", &["collection, start, end?"]),
    ("flat", &["list"]),
    // --- Text (character-indexed) ---
    ("chars", &["string"]),
    ("char_len", &["string"]),
    ("char_at", &["string, index"]),
    ("char_slice", &["string, start, end?"]),
    ("repeat", &["string, count"]),
    ("starts_with", &["string, prefix"]),
    ("ends_with", &["string, suffix"]),
    ("trim", &["string"]),
    // --- Formatting & JSON ---
    ("fixed", &["value, places?"]),
    ("commas", &["value, places?"]),
    ("pad_start", &["value, width, fill?"]),
    ("pad_end", &["value, width, fill?"]),
    ("json_stringify", &["value, indent?"]),
    ("json_parse", &["text"]),
    // --- Higher-order ---
    ("map", &["list, f"]),
    ("filter", &["list, pred"]),
    ("reduce", &["list, initial, f"]),
    ("forEach", &["list, f"]),
    // --- Automatic differentiation ---
    ("dual", &["value, derivative"]),
    ("value_of", &["x"]),
    ("deriv_of", &["x"]),
    // --- Typed numeric arrays ---
    ("f64_array", &["length"]),
    ("set_at", &["array, index, value"]),
    ("swap", &["array, i, j"]),
    // --- Symbols, output, handles ---
    ("symbol", &["name"]),
    ("push_output", &["buffer, value"]),
    ("binding", &["symbol"]),
    ("is_valid", &["handle"]),
    // --- Pending values ---
    ("is_loading", &["value"]),
    ("is_error", &["value"]),
    ("is_pending", &["value"]),
    ("is_ready", &["value"]),
    ("error_of", &["value"]),
    ("or_else", &["value, fallback"]),
    ("resource_key", &["value"]),
    // --- Built-in classes: the receiver is a method's first parameter ---
    ("Rect", &["x, y, w, h"]),
    ("Rect.center_x", &["r"]),
    ("Rect.center_y", &["r"]),
    ("Rect.right", &["r"]),
    ("Rect.bottom", &["r"]),
    ("Rect.inset", &["r, n"]),
    ("Rect.offset", &["r, dx, dy"]),
];

/// Apply [`BUILTIN_PARAMS`] to the registered builtins. A name the table does
/// not have, or a spec that does not parse, is a mistake in this file, so both
/// panic at startup rather than leave a builtin quietly positional-only.
pub(super) fn declare(table: &mut NativeFnTable) {
    for (name, specs) in BUILTIN_PARAMS {
        let id = table
            .lookup_name(name)
            .unwrap_or_else(|| panic!("BUILTIN_PARAMS names an unregistered builtin: {name}"));
        for spec in *specs {
            table
                .declare_params(id, spec)
                .unwrap_or_else(|e| panic!("BUILTIN_PARAMS[{name}]: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;
    use crate::native_fn::NativeSignature;

    /// Whether `err` is a complaint about how many arguments arrived, as
    /// opposed to what they were. Every arity message in `builtins/` says
    /// `expects <count…>`; a type message says `expects a list` and the like.
    fn is_arity_error(err: &str) -> bool {
        err.split("expects ").skip(1).any(|rest| {
            let rest = rest.trim_start_matches("at least ");
            rest.starts_with(|c: char| c.is_ascii_digit()) || rest.starts_with("no arguments")
        })
    }

    /// Call `name` with `count` arguments and return the error, if any. A
    /// method (`Rect.inset`) is called on a receiver, which is its first
    /// argument.
    fn call_with(name: &str, count: usize) -> Option<String> {
        let args = |n: usize| vec!["1"; n].join(", ");
        let src = match name.split_once('.') {
            Some((class, method)) => {
                format!("{class}(0, 0, 4, 4).{method}({})", args(count - 1))
            }
            None => format!("{name}({})", args(count)),
        };
        Env::new().run_source(&src).err()
    }

    /// Every declared form is one the native really takes: called with any
    /// argument count the form allows, it does not complain about the count —
    /// and one argument past the longest form, or one short of the shortest,
    /// it does. (The arguments are all `1`, so a type error is expected and
    /// fine; only an arity error is evidence against the declaration.)
    #[test]
    fn declared_signatures_agree_with_the_natives_arity() {
        // Natives that read the arguments they want and never count them, so
        // a wrong count is not refused and cannot be checked from outside.
        const UNCOUNTED: &[&str] = &["is_valid"];
        for (name, specs) in BUILTIN_PARAMS {
            let sigs: Vec<NativeSignature> = specs
                .iter()
                .map(|s| NativeSignature::parse(s).unwrap())
                .collect();
            let accepts = |n: usize| sigs.iter().any(|s| s.accepts_count(n));
            let longest = sigs.iter().map(|s| s.params().len()).max().unwrap();
            // A method call always supplies its receiver.
            let fewest = usize::from(name.contains('.'));
            for count in fewest..=longest + 1 {
                let err = call_with(name, count);
                let arity = err.as_deref().is_some_and(is_arity_error);
                if accepts(count) {
                    assert!(
                        !arity,
                        "{name}: declared to take {count} argument(s), but the native refuses \
                         that count: {err:?}"
                    );
                } else if !UNCOUNTED.contains(name) {
                    assert!(
                        arity,
                        "{name}: not declared to take {count} argument(s), but the native did \
                         not refuse that count: {err:?}"
                    );
                }
            }
        }
    }

    /// The table and the registry agree, and every name is one a call site
    /// can write as an argument label (a keyword included: `end:`).
    #[test]
    fn every_entry_is_registered_with_writable_names() {
        let env = Env::new();
        let mut seen = std::collections::HashSet::new();
        for (name, specs) in BUILTIN_PARAMS {
            assert!(seen.insert(*name), "{name} is listed twice");
            let sigs = env.native_signatures(name).expect("registered");
            assert_eq!(sigs.len(), specs.len(), "{name}");
            for sig in sigs {
                for param in sig.params() {
                    let src = format!("f({param}: 1)\n");
                    let mut lexer = crate::lexer::Lexer::new(&src);
                    lexer.tokenize().expect("lexes");
                    let parsed = crate::parse::Parser::new(lexer.tokens, lexer.token_spans)
                        .parse_program();
                    assert!(
                        parsed.is_ok(),
                        "{name}: `{param}: …` is not a writable argument label: {parsed:?}"
                    );
                }
            }
        }
    }

    /// What is left undeclared is left so deliberately: a builtin added
    /// without a row here shows up in this list and has to be decided.
    #[test]
    fn the_undeclared_builtins_are_the_variadic_and_internal_ones() {
        let env = Env::new();
        let mut undeclared: Vec<String> = env
            .native_fn_names()
            .into_iter()
            .filter(|n| env.native_signatures(n).is_some_and(|s| s.is_empty()))
            .collect();
        undeclared.sort();
        assert_eq!(
            undeclared,
            [
                "__declare_method",
                "__pending",
                "__reject",
                "__resolve",
                "format",
                "print"
            ]
        );
    }
}
