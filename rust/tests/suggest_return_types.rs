// `petal suggest`'s return-type kind: an un-annotated function that ends in a
// loop returns that loop's list implicitly, and whether it should say
// `-> list` or `-> nil` is read from how it is called
// (docs/dev/suggestions-plan.md, docs/implicit-return-values.md).
//
// The unit tests in `src/suggest/return_types.rs` cover the analysis. These
// cover the command: that the kind is selectable, that an applied file is the
// file the report promised, that a choice left to the author is never
// written — and that `--apply` writing `-> nil` really does change the
// compiled program while leaving what it prints alone.

use std::path::PathBuf;
use std::process::Command;

use petal::env::Env;
use petal::suggest::{Kinds, SuggestOptions, apply_all, suggest_source};
use petal::typecheck::globals::HostProfile;

const SRC: &str = "\
fn draw_all(items)
  for it in items do print(it) end
end

fn squares(xs)
  for x in xs do x * x end
end

fn orphan(xs)
  for x in xs do x end
end

draw_all([1, 2])
print(squares([1, 2, 3]))
";

fn only_returns() -> SuggestOptions {
    SuggestOptions {
        host: HostProfile::Core,
        kinds: Kinds::parse("return-types").unwrap(),
        ..Default::default()
    }
}

fn run(src: &str) -> String {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("load");
    let sid = env.create_stack(pid).expect("create_stack");
    env.run(sid).expect("run");
    env.take_output().join("\n")
}

/// A scratch file holding `src`, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, src: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "petal-suggest-returns-{}-{name}.ptl",
            std::process::id()
        ));
        std::fs::write(&path, src).expect("write scratch file");
        Scratch(path)
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.0).expect("read scratch file")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `petal suggest --host core <args> <file>`: exit code, stdout, stderr.
fn suggest(args: &[&str], file: &Scratch) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_petal"))
        .arg("suggest")
        .args(["--host", "core"])
        .args(args)
        .arg(&file.0)
        .output()
        .expect("spawn petal");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn each_function_gets_the_type_its_callers_imply() {
    let out = suggest_source(SRC, None, &only_returns()).expect("suggest");
    let got: Vec<(&str, Option<&str>)> = out
        .return_types
        .iter()
        .map(|r| (r.function.0.as_str(), r.ty))
        .collect();
    assert_eq!(
        got,
        [
            ("draw_all", Some("nil")),
            ("squares", Some("list")),
            ("orphan", None),
        ]
    );
    // `--only return-types` asked for nothing else.
    assert!(out.suggestions.is_empty() && out.named_args.is_empty() && out.advice.is_empty());
}

#[test]
fn the_kind_is_on_by_default_and_off_when_another_is_named() {
    let all = SuggestOptions {
        host: HostProfile::Core,
        ..Default::default()
    };
    assert_eq!(suggest_source(SRC, None, &all).unwrap().return_types.len(), 3);
    let types = SuggestOptions {
        host: HostProfile::Core,
        kinds: Kinds::parse("types,advice").unwrap(),
        ..Default::default()
    };
    assert!(suggest_source(SRC, None, &types).unwrap().return_types.is_empty());
}

#[test]
fn applying_writes_the_settled_ones_and_skips_the_choice() {
    let out = suggest_source(SRC, None, &only_returns()).expect("suggest");
    let rewritten = apply_all(SRC, &[], &out.return_types, &[]);
    assert!(rewritten.contains("fn draw_all(items) -> nil\n"), "{rewritten}");
    assert!(rewritten.contains("fn squares(xs) -> list\n"), "{rewritten}");
    assert!(rewritten.contains("fn orphan(xs)\n"), "{rewritten}");
    // The program prints what it printed, and has nothing further to settle.
    assert_eq!(run(&rewritten), run(SRC));
    let again = suggest_source(&rewritten, None, &only_returns()).expect("suggest");
    assert_eq!(again.return_types.len(), 1);
    assert_eq!(again.return_types[0].function.0, "orphan");
}

#[test]
fn a_return_type_sits_beside_a_parameter_annotation_on_the_same_line() {
    // Both kinds write into `fn draw_all(items)`: one inside the parentheses,
    // one just past them.
    let all = SuggestOptions {
        host: HostProfile::Core,
        ..Default::default()
    };
    let out = suggest_source(SRC, None, &all).expect("suggest");
    let rewritten = apply_all(SRC, &out.suggestions, &out.return_types, &out.named_args);
    assert!(
        rewritten.contains("fn draw_all(items: list) -> nil\n"),
        "{rewritten}"
    );
    assert!(rewritten.contains("fn squares(xs: list) -> list\n"), "{rewritten}");
}

#[test]
fn a_from_entry_point_supplies_the_callers_a_pub_function_lacks() {
    let lib = "pub fn rows(n)\n  for i in range(0, n) do i end\nend\n";
    let alone = suggest_source(lib, None, &only_returns()).expect("suggest");
    assert_eq!(alone.return_types[0].ty, None);

    let app = Scratch::new("from-app", "import lib\nlet r = lib.rows(3)\nprint(r)\n");
    let opts = SuggestOptions {
        from: vec![app.0.clone()],
        ..only_returns()
    };
    let seen = suggest_source(lib, None, &opts).expect("suggest");
    assert_eq!(seen.return_types[0].ty, Some("list"));
}

#[test]
fn apply_writes_nil_and_the_compiled_program_changes() {
    let file = Scratch::new("apply", SRC);
    let (code, stdout, stderr) = suggest(&["--only", "return-types", "--apply"], &file);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("Applied 2 of 2"), "{stdout}");
    // Each strength reports the proof it was actually held to.
    assert!(stderr.contains("1 `-> list` return type proven IR-equal"), "{stderr}");
    assert!(stderr.contains("1 `-> nil` return type compiles with no new type warning"), "{stderr}");

    let rewritten = file.text();
    assert!(rewritten.contains("fn draw_all(items) -> nil\n"), "{rewritten}");
    assert!(rewritten.contains("fn squares(xs) -> list\n"), "{rewritten}");
    assert!(rewritten.contains("fn orphan(xs)\n"), "{rewritten}");
    assert_eq!(run(&rewritten), run(SRC));

    // `suggest` is no longer IR-preserving: this rewrite is a different
    // program, on purpose.
    let before = Scratch::new("apply-before", SRC);
    let equal = Command::new(env!("CARGO_BIN_EXE_petal"))
        .arg("ir-equal")
        .arg(&before.0)
        .arg(&file.0)
        .output()
        .expect("spawn petal");
    assert_eq!(equal.status.code(), Some(1));
}

#[test]
fn verify_counts_only_what_could_be_written() {
    let file = Scratch::new("verify", SRC);
    let (code, _stdout, stderr) = suggest(&["--only", "return-types", "--verify"], &file);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(file.text(), SRC, "--verify must not write");

    // A file with only a choice to make has nothing to apply or to prove.
    let open = Scratch::new("open", "fn orphan(xs)\n  for x in xs do x end\nend\n");
    let (code, stdout, _) = suggest(&["--only", "return-types", "--apply"], &open);
    assert_eq!(code, 0);
    assert!(stdout.contains("choose: -> list or -> nil"), "{stdout}");
    assert!(stdout.contains("1 left to choose, never applied"), "{stdout}");
    assert!(!stdout.contains("Applied"), "{stdout}");
    assert_eq!(open.text(), "fn orphan(xs)\n  for x in xs do x end\nend\n");
}

#[test]
fn json_carries_the_type_the_options_and_whether_the_ir_survives() {
    let file = Scratch::new("json", SRC);
    let (code, stdout, _) = suggest(&["--only", "return-types", "--json"], &file);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let items = v["suggestions"].as_array().expect("suggestions");
    assert_eq!(items.len(), 3);
    for item in items {
        assert_eq!(item["kind"], "return-type");
        assert_eq!(item["options"], serde_json::json!(["list", "nil"]));
    }
    assert_eq!(items[0]["type"], "nil");
    assert_eq!(items[0]["usage"], "unused");
    assert_eq!(items[0]["preserves_ir"], false);
    assert_eq!(items[0]["edits"][0]["insert_text"], " -> nil");
    assert_eq!(items[1]["type"], "list");
    assert_eq!(items[1]["preserves_ir"], true);
    assert!(items[2]["type"].is_null());
    assert_eq!(items[2]["usage"], "unknown");
    assert_eq!(items[2]["edits"], serde_json::json!([]));
}
