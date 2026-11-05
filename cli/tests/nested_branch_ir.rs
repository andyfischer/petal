//! A name reassigned in a nested branch — an `elsif` arm, an `if` inside an
//! `else`/`then` arm, a `match` arm, a loop inside an arm — must reach the
//! enclosing join through the term graph, not just through registers.
//!
//! The arm's seeded entry copy shares a carry-slot register with the nested
//! join (`Phi`), so execution was always right. But the arm's phi-out used to
//! name the entry copy instead of the nested join, which cut the nested
//! branch out of the dataflow graph: provenance, `petal graph --slice`,
//! `petal explain` and direct manipulation (`petal propose-edit` said "nothing
//! can move the goals") could not see a literal assigned there.

use petal::env::Env;
use petal::program::{Program, TermId, TermOp};

const SHAPES: &[(&str, &str)] = &[
    (
        "elsif arm",
        "let K = 3.0\nlet i = 2\nlet y = 1.0\nif i == 0 then\n  print(\"zero\")\nelsif i > 1 then\n  y = K * 2.0\nend\nlet out = y\n",
    ),
    (
        "if inside else",
        "let K = 3.0\nlet i = 2\nlet y = 1.0\nif i == 0 then\n  print(\"zero\")\nelse\n  if i > 1 then\n    y = K * 2.0\n  end\nend\nlet out = y\n",
    ),
    (
        "if inside then",
        "let K = 3.0\nlet i = 2\nlet y = 1.0\nif i > 0 then\n  if i > 1 then\n    y = K * 2.0\n  end\nend\nlet out = y\n",
    ),
    (
        "three-deep elsif chain",
        "let K = 3.0\nlet i = 3\nlet y = 1.0\nif i == 0 then\n  y = 0.0\nelsif i == 1 then\n  y = 1.0\nelsif i == 2 then\n  y = 2.0\nelsif i == 3 then\n  y = K * 2.0\nend\nlet out = y\n",
    ),
    (
        "if-in-else three deep",
        "let K = 3.0\nlet i = 3\nlet y = 1.0\nif i == 0 then\n  y = 0.0\nelse\n  if i == 1 then\n    y = 1.0\n  else\n    if i == 3 then\n      y = K * 2.0\n    end\n  end\nend\nlet out = y\n",
    ),
    (
        "match arm",
        "let K = 3.0\nlet i = 2\nlet y = 1.0\nmatch i\n  when 0 -> y = 0.0\n  when _ ->\n    if i > 1 then\n      y = K * 2.0\n    end\nend\nlet out = y\n",
    ),
    (
        "loop inside arm",
        "let K = 3.0\nlet i = 2\nlet y = 0.0\nif i == 0 then\n  y = 5.0\nelse\n  for j in range(0, 2) do\n    y = y + K\n  end\nend\nlet out = y\n",
    ),
];

fn compile(src: &str) -> (Env, petal::program::ProgramId) {
    let mut env = Env::new();
    let pid = env
        .load_program(&format!("import io\n{src}"))
        .unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    (env, pid)
}

/// A term that some `Phi` in its own block took as its pre-branch value has
/// been superseded by that phi: carrying it out of the block drops whatever
/// the nested branch assigned.
fn superseded_phi_outs(p: &Program) -> Vec<(TermId, TermId)> {
    let mut bad = Vec::new();
    for block in &p.blocks {
        for out in &block.phi_outs {
            let src = out.src_term;
            let src_block = p.terms[src.0 as usize].block_id;
            let superseded = p.terms.iter().any(|t| {
                matches!(t.op, TermOp::Phi)
                    && t.block_id == src_block
                    && t.inputs.first() == Some(&src)
            });
            if superseded {
                bad.push((src, out.dest_term));
            }
        }
    }
    bad
}

#[test]
fn phi_outs_carry_the_nested_join_not_the_arm_entry_copy() {
    for (label, src) in SHAPES {
        let (env, pid) = compile(src);
        let p = env.get_program(pid).unwrap();
        let bad = superseded_phi_outs(p);
        assert!(
            bad.is_empty(),
            "{label}: phi-outs wire from superseded terms {bad:?}\n{src}"
        );
    }
}

#[test]
fn provenance_and_slice_reach_the_nested_assignment() {
    for (label, src) in SHAPES {
        let (env, pid) = compile(src);
        let p = env.get_program(pid).unwrap();
        let out = p
            .find_term("out")
            .unwrap_or_else(|| panic!("{label}: no `out` term"));
        let k = p
            .find_term("K")
            .unwrap_or_else(|| panic!("{label}: no `K` term"));
        let prov = p.trace_provenance(out);
        assert!(
            prov.ancestors.contains(&k),
            "{label}: provenance of `out` misses `K`\n{src}"
        );
        let (slice, _) = p.slice(&[out]).conservative();
        assert!(
            slice.contains(&k),
            "{label}: slice of `out` misses `K`\n{src}"
        );
    }
}

#[test]
fn explain_walks_into_the_nested_branch() {
    for (label, src) in SHAPES {
        let (mut env, pid) = compile(src);
        env.trace_mut().enable();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let p = env.get_program(pid).unwrap();
        let out = p.find_term("out").unwrap();
        let k = p.find_term("K").unwrap();
        let chain = env.trace().explain(p, env.heap(), out, 64);
        assert!(
            chain.entries.iter().any(|e| e.term_id == k),
            "{label}: explain of `out` never reaches `K`\n{src}"
        );
        assert_eq!(
            chain.entries[0].value.as_deref(),
            Some("6.0"),
            "{label}: value of `out`"
        );
    }
}

/// The Canopy debrief repro: `propose-edit` on a value assigned in an `elsif`
/// arm (or an `if` nested in an `else`) used to fail with "nothing can move
/// the goals"; the flat `if` always solved. Every nesting shape must now
/// propose verified edits to the nested assignment's literal or to `K`.
#[test]
fn propose_edit_solves_through_nested_branches() {
    let dir = std::env::temp_dir().join(format!("petal-nested-branch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (n, (label, src)) in SHAPES.iter().enumerate() {
        let path = dir.join(format!("shape{n}.ptl"));
        std::fs::write(
            &path,
            format!("import io\n{src}push_output(symbol(\"ys\"), out)\n"),
        )
        .unwrap();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_petal"))
            .args([
                "propose-edit",
                "--json",
                "--channel",
                "ys",
                "--emit",
                "0",
                "--value",
                "--to",
                "10",
            ])
            .arg(&path)
            .env_remove("PETAL_SEED")
            .output()
            .expect("runs petal");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{label}: propose-edit failed\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let r: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let alts = r["alternatives"].as_array().unwrap();
        assert!(!alts.is_empty(), "{label}: no alternatives\n{r:#}");
        assert!(
            alts.iter().all(|a| a["verified"] == true),
            "{label}: unverified alternative\n{r:#}"
        );
        assert!(
            alts.iter().any(|a| a["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["binding"] == "K")),
            "{label}: no alternative edits `K`\n{r:#}"
        );
    }
}
