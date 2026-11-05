//! CLI-level tests for the pending observability surfaces (Chunk O):
//! the `pending-report` subcommand and the `run --trace-pending` flag.
//!
//! These shell out to the built `petal` binary (via `CARGO_BIN_EXE_petal`,
//! which Cargo sets for integration tests) so they exercise argument parsing,
//! the handlers, and the JSON report end to end — the same path the MCP
//! `PendingReport` tool and an agent driving the CLI take.

use std::process::Command;

/// Path to the freshly built `petal` binary for this test run.
const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// `pending-report --json` runs the snippet and emits a JSON array in which the
/// live `__pending("k")` resource appears as a `loading` entry whose origin
/// names the call site.
#[test]
fn pending_report_subcommand_emits_json_with_a_pending_entry() {
    let out = Command::new(PETAL)
        .args([
            "pending-report",
            "--json",
            "-e",
            "let x = __pending(\"k\")\nx\n",
        ])
        .output()
        .expect("failed to run petal");

    assert!(
        out.status.success(),
        "pending-report exited non-zero; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout was not valid JSON ({e}):\n{stdout}"));

    let arr = report.as_array().expect("report must be a JSON array");
    assert_eq!(
        arr.len(),
        1,
        "expected exactly one live resource, got {report}"
    );

    let entry = &arr[0];
    assert_eq!(
        entry.get("state").and_then(|s| s.as_str()),
        Some("loading"),
        "resource state missing/wrong in {entry}"
    );
    let text = entry
        .get("origin")
        .and_then(|o| o.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    assert!(
        text.contains("__pending"),
        "origin source text should name the call site, got {entry}"
    );
}

/// `run --trace-pending` executes the program normally and then prints the
/// pending report so a developer can see what stayed unresolved this frame.
#[test]
fn run_trace_pending_prints_the_report() {
    let out = Command::new(PETAL)
        .args([
            "run",
            "--trace-pending",
            "-e",
            "let x = __pending(\"k\")\nx\n",
        ])
        .output()
        .expect("failed to run petal");

    assert!(
        out.status.success(),
        "run --trace-pending exited non-zero; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("pending report") && stderr.contains("loading"),
        "expected a pending report on stderr, got:\n{stderr}"
    );
}
