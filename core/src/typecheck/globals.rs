//! Unknown globals: names a program calls or reads that nothing defines.
//!
//! The compiler does not reject an unresolved name: a call to one and a read
//! of one both become a deferred `Error` term ("Unknown function: x",
//! "Undefined variable: x") that fires only if the line runs. That is
//! deliberate — a line that never runs should not stop the rest of a script —
//! but it means a program that is certain to fail the moment the line runs
//! would otherwise pass `petal check`.
//!
//! [`unresolved_globals`] closes that hole after the compile, by scanning the
//! program's IR for those terms. No host is consulted: a name is in scope
//! because the file declares it, an import binds it, or it is a `core`/`std`
//! builtin, and for no other reason. A host's natives are answered for by its
//! interface package (`import ui`), whose `native fn` declarations carry the
//! parameter lists a named call is checked against, whether or not this
//! process registers the natives.

use std::collections::HashSet;

use crate::diagnostic::Diagnostic;
use crate::native_fn::NativeSignature;
use crate::program::{Program, TermOp};

/// How the deferred error for a call to a name nothing binds starts; the
/// name follows.
pub const UNKNOWN_FUNCTION_PREFIX: &str = "Unknown function: ";

/// How the deferred error for a read of a name nothing binds starts.
pub const UNDEFINED_VARIABLE_PREFIX: &str = "Undefined variable: ";

/// Check the names a call to native `name` writes against the parameter lists
/// it declares, and say what is wrong — `None` when some form takes the call.
///
/// Worded as the checker words the same slip on a Petal `fn`
/// (`Checker::check_named_args`): the VM's sentence, plus the parameters that
/// do exist or the argument that already filled the slot. `names` is parallel
/// to the call's arguments, `None` for a positional one.
pub fn named_native_call_error(
    name: &str,
    sigs: &[NativeSignature],
    names: &[Option<&str>],
) -> Option<String> {
    // Only the shape of the call matters here, so the "arguments" bound are
    // their own indices.
    let indices: Vec<usize> = (0..names.len()).collect();
    if sigs.iter().any(|s| s.bind(name, &indices, names).is_ok()) {
        return None;
    }
    // The form the call was aimed at, chosen as the run chooses it
    // (`native_fn::bind_native_args`): the only one, or the one with exactly
    // this many parameters. With several forms and none of that length there
    // is no single parameter list to hold the names against, and the run's
    // own summary of the forms is the report.
    let sig = match sigs {
        [only] => only,
        _ => match sigs.iter().find(|s| s.params().len() == names.len()) {
            Some(sig) => sig,
            None => {
                return crate::native_fn::bind_native_args(name, sigs, &indices, names).err();
            }
        },
    };
    let params = sig.params();
    let mut filled: Vec<Option<usize>> = vec![None; params.len()];
    for (i, written) in names.iter().enumerate() {
        let Some(written) = written else {
            if let Some(cell) = filled.get_mut(i) {
                *cell = Some(i);
            }
            continue;
        };
        let Some(slot) = params.iter().position(|p| p == written) else {
            let known: Vec<String> = params.iter().map(|p| format!("'{p}'")).collect();
            let known = if known.is_empty() {
                "none".to_string()
            } else {
                known.join(", ")
            };
            return Some(format!(
                "{name}() has no parameter named '{written}' (parameters: {known})"
            ));
        };
        if let Some(first) = filled[slot] {
            return Some(format!(
                "{name}() got multiple values for parameter '{written}' (argument {} already fills it)",
                first + 1
            ));
        }
        filled[slot] = Some(i);
    }
    // Neither of those: a required parameter left out, a gap below an
    // optional one, or too many arguments. The binder's own sentence says
    // which.
    sig.bind(name, &indices, names).err()
}

/// Report every call to, or read of, a name that nothing defines, and every
/// named argument a native cannot take.
///
/// `native_signatures` answers for the `Env` the program was compiled in:
/// `None` when it registers no native of that name, else the parameter lists
/// that native declares
/// ([`Env::native_signatures`](crate::env::Env::native_signatures)).
///
/// A call to an unbound name and a read of one are the compiler's deferred
/// `Error` terms ([`UNKNOWN_FUNCTION_PREFIX`], [`UNDEFINED_VARIABLE_PREFIX`]).
///
/// A `BuiltinCall` — a call the compiler bound to a native, a `core` builtin
/// or one a `native fn` in scope declares — is reported when it still carries
/// argument names: the compiler strips them from every call whose names bind
/// (`Compiler::normalize_named_builtin_calls`), so what is left fails when it
/// runs:
///
/// - a native that declares no parameters refuses names outright
///   (`builtin 'print' does not accept named arguments`);
/// - one that declares them is checked against them — an unknown name, a slot
///   filled twice, a required parameter left out — in the words the checker
///   uses for a Petal `fn` ([`named_native_call_error`]).
///
/// A native this process does not register is checked against the parameter
/// lists its declaration carries ([`Program::native_links`]).
///
/// One diagnostic per source position, in term order. Each is an error
/// ([`Severity::Error`](crate::diagnostic::Severity::Error)): the line fails
/// whenever it runs, so `check` fails on it without `--strict`.
pub fn unresolved_globals<'a>(
    program: &Program,
    native_signatures: impl Fn(&str) -> Option<&'a [NativeSignature]>,
) -> Vec<Diagnostic> {
    // Natives the program declares, with the parameter lists declared.
    let declared: std::collections::HashMap<&str, &[NativeSignature]> = program
        .native_links
        .iter()
        .map(|link| (link.host.as_str(), link.signatures.as_slice()))
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for term in &program.terms {
        let message = match term.op {
            TermOp::BuiltinCall(cid) => {
                let Some(name) = program.get_string_constant(cid) else {
                    continue;
                };
                if term.arg_names.iter().all(Option::is_none) {
                    continue;
                }
                let names: Vec<Option<&str>> = term
                    .arg_names
                    .iter()
                    .map(|n| n.and_then(|c| program.get_string_constant(c)))
                    .collect();
                let sigs: &[NativeSignature] = match native_signatures(name) {
                    Some(sigs) => sigs,
                    None => match declared.get(name) {
                        Some(sigs) => sigs,
                        None => continue,
                    },
                };
                if sigs.is_empty() {
                    // Same wording as the VM's refusal, like the
                    // checker's other named-argument errors.
                    format!(
                        "builtin '{name}' does not accept named arguments (pass them by position)"
                    )
                } else {
                    match named_native_call_error(name, sigs, &names) {
                        Some(message) => message,
                        None => continue,
                    }
                }
            }
            TermOp::Error(cid) => {
                let Some(msg) = program.get_string_constant(cid) else {
                    continue;
                };
                if let Some(name) = msg.strip_prefix(UNKNOWN_FUNCTION_PREFIX) {
                    format!(
                        "unknown function `{name}`: nothing by that name is in scope, and it is not \
                         a builtin (running this line fails with \"{UNKNOWN_FUNCTION_PREFIX}{name}\")"
                    )
                } else if let Some(rest) = msg.strip_prefix(UNDEFINED_VARIABLE_PREFIX) {
                    // The compiler appends a hint after an em dash for common
                    // slips from other languages (`null`, `elif`, …).
                    let name = rest.split(' ').next().unwrap_or(rest);
                    let hint = rest[name.len()..].trim_start();
                    if hint.is_empty() {
                        format!("undefined variable `{name}`")
                    } else {
                        format!("undefined variable `{name}` {hint}")
                    }
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        let Some(span) = program.source_map.get(term.id) else {
            continue;
        };
        if seen.insert((
            span.file,
            span.start.line,
            span.start.column,
            message.clone(),
        )) {
            out.push(Diagnostic::error(*span, message));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;

    fn unresolved(src: &str) -> Vec<String> {
        let mut env = Env::new();
        // A check, not a run: the natives a test declares are not registered.
        env.set_check_only(true);
        let pid = env.load_program(src).expect("compiles");
        let program = env.get_program(pid).unwrap();
        unresolved_globals(program, |n| env.native_signatures(n))
            .into_iter()
            .map(|d| {
                format!(
                    "{}:{} {}",
                    d.span.start.line, d.span.start.column, d.message
                )
            })
            .collect()
    }

    #[test]
    fn unknown_call_and_read_are_reported() {
        let got = unresolved("totally_bogus_fn(1)\nlet y = nope + 1");
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(
            got[0].starts_with("1:1 unknown function `totally_bogus_fn`"),
            "{got:?}"
        );
        assert_eq!(got[1], "2:9 undefined variable `nope`");
    }

    #[test]
    fn builtins_bindings_and_declared_fns_are_known() {
        let src = "fn f(x) sqrt(x) end\nlet g = f\nprint(g(4), len([1]), f(9))";
        assert!(unresolved(src).is_empty());
    }

    #[test]
    fn a_host_native_is_known_only_through_an_import_of_its_interface() {
        let src = "draw_rect(0, 0, 1, 1, 0, 0, 0)\nlet k = key_down";
        let got = unresolved(src);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].starts_with("1:1 unknown function `draw_rect`"), "{got:?}");
        assert_eq!(got[1], "2:9 undefined variable `key_down`");
        assert!(unresolved(&format!("import ui\n{src}")).is_empty());
    }

    #[test]
    fn a_registered_native_nothing_declares_is_still_unknown() {
        let mut env = Env::new();
        env.register_native(
            "host_only",
            |_| Ok(0),
            crate::native_fn::NativeEffects::PURE,
        );
        env.set_check_only(true);
        let pid = env.load_program("host_only()\nlet f = host_only").unwrap();
        let program = env.get_program(pid).unwrap();
        assert_eq!(
            unresolved_globals(program, |n| env.native_signatures(n)).len(),
            2
        );
        let pid = env
            .load_program("native fn host_only()\nhost_only()\nlet f = host_only")
            .unwrap();
        let program = env.get_program(pid).unwrap();
        assert!(unresolved_globals(program, |n| env.native_signatures(n)).is_empty());
    }

    #[test]
    fn a_named_argument_to_an_undeclared_builtin_is_reported() {
        let got = unresolved("if false then print(1, sep: 2) end\nprint(1, 2)");
        assert_eq!(
            got,
            ["1:15 builtin 'print' does not accept named arguments (pass them by position)"]
        );
        // A Petal `fn` of the same call shape is not a native at all.
        let src = "fn f(a, b) a - b end\nprint(f(b: 1, a: 2), sum(xs: [1]))";
        assert!(unresolved(src).is_empty());
    }

    #[test]
    fn names_a_builtin_declares_are_accepted_and_checked() {
        let ok = "print(clamp(5, lo: 0, hi: 3), clamp(hi: 3, value: 5, lo: 0), [1, 2, 3].slice(start: 1))";
        assert!(unresolved(ok).is_empty());
        // Inside an untaken branch, so only this pass ever reports them.
        let got = unresolved(
            "if false then\n  clamp(5, low: 0, hi: 3)\n  clamp(5, value: 0, hi: 3)\n  clamp(5, hi: 3)\n  slice([1], end: 1)\nend",
        );
        assert_eq!(
            got,
            [
                "2:3 clamp() has no parameter named 'low' (parameters: 'value', 'lo', 'hi')",
                "3:3 clamp() got multiple values for parameter 'value' (argument 1 already fills it)",
                "4:3 clamp() is missing a value for parameter 'lo'",
                "5:3 slice() is missing a value for parameter 'start'",
            ]
        );
    }

    #[test]
    fn a_builtin_with_several_forms_takes_any_of_them() {
        let ok = "print(random(max: 2), random(min: 1, max: 2), distance(v1: vec2(0, 0), v2: vec2(1, 1)))";
        assert!(unresolved(ok).is_empty());
        let got = unresolved("if false then random(lo: 1, hi: 2) end");
        assert_eq!(
            got,
            ["1:15 random() has no parameter named 'lo' (parameters: 'min', 'max')"]
        );
    }

    #[test]
    fn declared_host_natives_are_checked_against_their_declaration() {
        // A host's parameters are declared by its interface module, so they
        // are checked without the host's `Env`.
        let iface = "native fn mouse_down(button: num) -> bool\n\
                     native fn mouse_x() -> int\n\
                     native fn clip(x, y, w, h, radius?)\n";
        let check = |call: &str| unresolved(&format!("{iface}{call}"));
        assert!(check("mouse_down(button: 0)").is_empty());
        assert!(check("clip(x: 0, y: 0, w: 4, h: 4, radius: 2)").is_empty());
        assert_eq!(
            check("mouse_down(btn: 0)"),
            ["4:1 mouse_down() has no parameter named 'btn' (parameters: 'button')"]
        );
        assert_eq!(
            check("mouse_x(button: 0)"),
            ["4:1 mouse_x() has no parameter named 'button' (parameters: none)"]
        );
        // A declared native is known by call and by read.
        assert!(check("let f = mouse_x\nprint(f, mouse_x())").is_empty());
    }

    #[test]
    fn a_hint_after_the_name_is_kept() {
        let got = unresolved("print(null)");
        assert_eq!(
            got,
            ["1:7 undefined variable `null` — use 'nil' for null/empty values in Petal"]
        );
    }

    #[test]
    fn repeated_calls_on_one_line_each_report() {
        let got = unresolved("zz(1)\nzz(2)");
        assert_eq!(got.len(), 2, "{got:?}");
    }
}
