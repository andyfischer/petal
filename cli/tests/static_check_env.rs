//! `PETAL_STATIC_CHECK=1`: every `Env` checks the values the static cache
//! keeps for function bodies instead of trusting them
//! (`Env::set_static_check`), and prints each one that was built different
//! on a later call on stderr. That line is all a whole test suite run under
//! the variable has to show for a wrong value, since none of its tests reads
//! `StaticValues::mismatches`. A value that differs only in having been
//! frozen is a `static check note:`, not a failure: a host that runs its
//! games under the variable and greps for failures must not find one in a
//! game that freezes what its functions return.
//!
//! Shells out to the built `petal` binary, so the variable is set for a
//! child process and not for this one.

use std::process::Command;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// The one difference a correct cache has: `freeze` is about the heap
/// object, so the kept list is frozen where a list built again is not.
const FROZEN_KEPT_VALUE: &str = "fn mk()\n  [[1, 2], [3, 4]]\nend\nprint(frozen(mk()))\nfreeze(mk())\nprint(frozen(mk()), mk())\n";

/// Nothing to find: the list is the same on every call.
const HONEST: &str = "fn mk()\n  [[1, 2], [3, 4]]\nend\nprint(mk(), mk())\n";

fn stderr_of(source: &str, check: bool) -> String {
    let mut cmd = Command::new(PETAL);
    cmd.args(["run", "--runs", "3", "-e", source])
        .env_remove("PETAL_STATIC_CHECK")
        // The audit turns the cache off, and with it the check.
        .env_remove("PETAL_STATIC_AUDIT")
        .env_remove("PETAL_POLICY")
        .env_remove("PETAL_OPT");
    if check {
        cmd.env("PETAL_STATIC_CHECK", "1");
    }
    let out = cmd.output().expect("spawn petal");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn the_environment_variable_prints_each_value_that_differs() {
    let found = stderr_of(FROZEN_KEPT_VALUE, true);
    let lines: Vec<&str> = found.lines().filter(|l| l.starts_with("static check")).collect();
    assert_eq!(
        lines,
        ["static check note: the value kept for mk (line 2) has been frozen and the same value built again is not: \
          `freeze` reached the one value every call returns"],
        "once per value, however many calls and runs found it; a note, since the contents are the same\n{found}"
    );
    assert!(!found.contains("static check failure"), "freezing a kept value is not a failure\n{found}");
    assert!(!stderr_of(HONEST, true).contains("static check"), "a value that is the same every call");
    assert!(!stderr_of(FROZEN_KEPT_VALUE, false).contains("static check"), "off unless asked for");
}
