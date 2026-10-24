//! Direct manipulation v2, M1 acceptance items that need petal-ui
//! (docs/dev/direct-manipulation-v2-plan.md §5, M1, items 8 and 16): raw
//! arguments of a draw native before it truncates them, and host-data replay
//! through the frame core's run wrapper (`FrameCore::spec_parts`).

use std::cell::Cell;
use std::rc::Rc;

use petal::dm_v2::observe::hash_str;
use petal::dm_v2::spec::{NoHost, ObsValue, OverrideValue, Overrides, SpecConfig, SpecError, SpecSession};
use petal::dm_v2::{EmitKey, LiveObservation, Observable, PathStep};
use petal::env::Env;
use petal::program::{ProgramId, TermId, TermOp};
use petal_ui::draw::{DRAW_COMMANDS_SYMBOL, DrawCommand};
use petal_ui::harness::Headless;
use petal_ui::host_data::HostData;

/// The one numeric literal of the entry file written as `text`.
fn lit(env: &Env, pid: ProgramId, text: &str) -> TermId {
    let program = env.get_program(pid).unwrap();
    let source = &program.source_map.files[0].source;
    let found: Vec<TermId> = program
        .terms
        .iter()
        .filter(|t| matches!(t.op, TermOp::Constant(_)))
        .filter(|t| {
            program.source_map.get(t.id).is_some_and(|span| {
                span.file.0 == 0
                    && source.get(span.start.offset as usize..span.end.offset as usize) == Some(text)
            })
        })
        .map(|t| t.id)
        .collect();
    assert_eq!(found.len(), 1, "literal `{text}`: {found:?}");
    found[0]
}

/// The key of the first `circle` draw command a session's baseline emitted.
fn circle_key(session: &SpecSession) -> EmitKey {
    let circle = hash_str("circle");
    session
        .baseline()
        .emits
        .iter()
        .find(|e| matches!(&e.value, ObsValue::Variant { tag, .. } if *tag == circle))
        .expect("a circle")
        .key
}

// ── 8. raw_native_args_not_truncated ─────────────────────────────────────

#[test]
fn raw_native_args_not_truncated() {
    let mut ui = Headless::with_ui("draw_circle(100.3, 50, 10, 255, 0, 0)").unwrap();
    let cmds = ui.frame().unwrap().to_vec();
    assert!(
        cmds.iter().any(|c| matches!(c, DrawCommand::Circle { cx: 100, .. })),
        "the live command is truncated to 100: {cmds:?}"
    );
    let (pid, sid) = (ui.program_id(), ui.stack_id());
    let x = lit(&ui.env, pid, "100.3");
    ui.env.reset_stack(sid).unwrap();
    let (env, mut host) = ui.spec_parts();
    let mut session = SpecSession::begin(env, &mut host, sid, &[x], SpecConfig::default(), None).unwrap();

    let key = circle_key(&session);
    let raw = Observable::EmitArg {
        key,
        arg: 0,
        path: vec![],
    };
    let emitted = Observable::EmitValue {
        key,
        path: vec![PathStep::Index(0)],
    };
    assert_eq!(session.baseline().read(&raw), Some(100.3));
    assert_eq!(session.baseline().read(&emitted), Some(100.0));

    let r = session.run(env, &mut host, &Overrides::one(x, OverrideValue::Num(100.7)));
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(r.read(&raw), Some(100.7), "a small change shows in the raw argument");
    assert_eq!(r.read(&emitted), Some(100.0), "while the emitted command does not move");
    // The draw channel is where it went.
    let draw = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    assert!(r.channel(draw).count() >= 1);
}

// ── 16. host_data_replayed ───────────────────────────────────────────────

#[test]
fn host_data_replayed() {
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    let mut ui = Headless::with_ui(
        "let id = 3\n\
         let v = host_data(\"item\", str(id))\n\
         draw_circle(v ?? 0, 10, 5, 0, 0, 0)",
    )
    .unwrap();
    ui.set_data_provider(Box::new(move |kind, arg| {
        seen.set(seen.get() + 1);
        match (kind, arg) {
            ("item", "3") => HostData::Int(40),
            _ => HostData::Int(99),
        }
    }));
    let cmds = ui.frame().unwrap().to_vec();
    assert!(cmds.iter().any(|c| matches!(c, DrawCommand::Circle { cx: 40, .. })), "{cmds:?}");
    assert_eq!(calls.get(), 1);
    let (pid, sid) = (ui.program_id(), ui.stack_id());
    let id = lit(&ui.env, pid, "3");
    ui.env.reset_stack(sid).unwrap();

    let (env, mut host) = ui.spec_parts();
    let mut session = SpecSession::begin(env, &mut host, sid, &[id], SpecConfig::default(), None).unwrap();
    assert_eq!(calls.get(), 3, "each baseline asks the provider");
    let key = circle_key(&session);
    let x = Observable::EmitArg {
        key,
        arg: 0,
        path: vec![],
    };
    assert_eq!(session.baseline().read(&x), Some(40.0));

    // The same question again is answered from the record.
    let same = session.run(env, &mut host, &Overrides::one(id, OverrideValue::Int(3)));
    assert_eq!(same.read(&x), Some(40.0));
    assert_eq!(same.new_host_keys, 0);
    // A question the baseline never asked answers nil, without the provider.
    let other = session.run(env, &mut host, &Overrides::one(id, OverrideValue::Int(4)));
    assert!(other.ok(), "{:?}", other.error);
    assert_eq!(other.new_host_keys, 1);
    assert_eq!(other.read(&x), Some(0.0), "`v ?? 0` saw nil");
    assert_eq!(calls.get(), 3, "the provider was not asked again");
    session.end(env);

    // Without the frame core's run wrapper the provider is not installed,
    // and the baseline disagrees with the live frame (the test bites).
    let live = LiveObservation {
        goals: vec![(x.clone(), 40.0)],
    };
    let err = SpecSession::begin(&mut ui.env, &mut NoHost, sid, &[id], SpecConfig::default(), Some(&live))
        .err()
        .expect("mismatch");
    assert!(matches!(err, SpecError::BaselineMismatch { .. }), "{err:?}");
    // With it, the same comparison passes.
    let (env, mut host) = ui.spec_parts();
    SpecSession::begin(env, &mut host, sid, &[id], SpecConfig::default(), Some(&live)).unwrap();
}
