//! `PETAL_STATIC_AUDIT=watch`: every `Env` starts with the static audit on
//! (`petal::static_audit`), which is how a whole test suite is run under it.
//!
//! One test, alone in its binary: it sets a process environment variable.

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::static_audit::StaticAuditMode;

fn prints(cxt: &mut PetalCxt) -> NativeResult {
    cxt.print("built".to_string());
    cxt.push_int(1);
    Ok(1)
}

#[test]
fn the_environment_variable_turns_the_audit_on_for_every_env() {
    assert_eq!(Env::new().static_audit_mode(), StaticAuditMode::Off);
    // SAFETY: the only test in this binary; nothing else reads the
    // environment concurrently.
    unsafe { std::env::set_var("PETAL_STATIC_AUDIT", "watch") };
    let mut env = Env::new();
    assert_eq!(env.static_audit_mode(), StaticAuditMode::Watch);
    env.set_echo(false);
    // Claims to be constant, and prints.
    env.register_native("prints", prints, NativeEffects::CONST);
    env.run_source("native fn prints()\nlet TABLE = [prints(), 2]\nlen(TABLE)\n").unwrap();
    let report = env.static_audit_report();
    assert_eq!(report.findings.len(), 1, "{report}");
    assert_eq!(report.findings[0].declaration.as_deref(), Some("TABLE"));
}
