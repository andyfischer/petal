//! `pb_vm_propose_edit`: direct manipulation through the C ABI, on a script
//! that calls host-registered natives.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use petal::value::Value;
use serde_json::{Value as Json, json};

use crate::ffi::Status;
use crate::natives::{self, FX_CONTAINED, FX_EFFECT, HostCallback, PbCall, pb_call_arg, pb_call_result};
use crate::vm::{Vm, VmHandle, pb_vm_destroy, pb_vm_last_error, pb_vm_propose_edit};

const SCRIPT: &str = "\
import io
native fn to_screen(x)
native fn play_sound(n)
native fn tally(n)
config let px = 10
play_sound(1)
tally(1)
push_output(symbol(\"scene\"), to_screen(px))
";

static SOUNDS: AtomicUsize = AtomicUsize::new(0);
static TALLIES: AtomicUsize = AtomicUsize::new(0);

/// `to_screen(x)`: `x * 2 + 100`, a pure host native.
unsafe extern "C" fn to_screen(call: *mut PbCall, _: *mut c_void) -> i32 {
    let arg = unsafe { &*pb_call_arg(call, 0) };
    let x = if arg.kind == crate::view::Kind::Int as u32 { arg.integer as f64 } else { arg.number };
    crate::builder::pb_builder_float(pb_call_result(call), x * 2.0 + 100.0);
    0
}

/// An escaping effect (PB_FX_EFFECT): a speculative run must not call it.
unsafe extern "C" fn play_sound(_: *mut PbCall, _: *mut c_void) -> i32 {
    SOUNDS.fetch_add(1, Ordering::SeqCst);
    0
}

/// A contained one (PB_FX_EFFECT | PB_FX_CONTAINED): speculative runs may.
unsafe extern "C" fn tally(_: *mut PbCall, _: *mut c_void) -> i32 {
    TALLIES.fetch_add(1, Ordering::SeqCst);
    0
}

fn vm_with(path: &std::path::Path) -> *mut VmHandle {
    let mut vm = Vm::new();
    let env = vm.env_mut();
    natives::register_callback(env, HostCallback::new("to_screen", 0, to_screen, std::ptr::null_mut(), None));
    natives::register_callback(env, HostCallback::new("play_sound", FX_EFFECT, play_sound, std::ptr::null_mut(), None));
    natives::register_callback(
        env,
        HostCallback::new("tally", FX_EFFECT | FX_CONTAINED, tally, std::ptr::null_mut(), None),
    );
    vm.load_file(path).unwrap();
    VmHandle::into_raw(vm)
}

fn propose(vm: *mut VmHandle, request: &Json) -> (Status, Json) {
    let req = CString::new(request.to_string()).unwrap();
    let mut out: *const c_char = std::ptr::null();
    let st = unsafe { pb_vm_propose_edit(vm, req.as_ptr(), &mut out) };
    assert!(!out.is_null(), "no response for {request}");
    let text = unsafe { CStr::from_ptr(out) }.to_str().unwrap();
    (st, serde_json::from_str(text).unwrap())
}

fn scene(vm: *mut VmHandle) -> Vec<f64> {
    let vm = unsafe { VmHandle::vm_mut(vm) }.unwrap();
    vm.run().unwrap();
    let env = vm.env_mut();
    let sym = env.intern_symbol("scene");
    env.take_output_buffer(sym)
        .into_iter()
        .map(|v| match v {
            Value::Float(f) => f,
            Value::Int(n) => n as f64,
            other => panic!("not a number: {other:?}"),
        })
        .collect()
}

#[test]
fn drags_through_a_host_native_without_firing_escaping_effects() {
    let dir = std::env::temp_dir().join(format!("petal-bridge-propose-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scene.ptl");
    std::fs::write(&path, SCRIPT).unwrap();
    let vm = vm_with(&path);

    // The host's frame: the sound plays once.
    assert_eq!(scene(vm), vec![120.0]);
    assert_eq!(SOUNDS.load(Ordering::SeqCst), 1);
    let tallies = TALLIES.load(Ordering::SeqCst);

    let (st, report) = propose(vm, &json!({"channel": "scene", "emit": 0, "value": true, "to": 150}));
    assert_eq!(st, Status::Ok, "{report:#}");
    let best = &report["alternatives"][0];
    assert_eq!(best["verified"], json!(true), "{report:#}");
    assert_eq!(best["changes"][0]["binding"], json!("px"), "{report:#}");
    assert_eq!(best["changes"][0]["new"].as_f64(), Some(25.0), "{report:#}");
    // Dozens of speculative runs: none played the sound; the contained
    // native ran in them.
    assert_eq!(SOUNDS.load(Ordering::SeqCst), 1);
    assert!(TALLIES.load(Ordering::SeqCst) > tallies);

    // Apply it; the host reloads and the next frame draws where it was dragged.
    let (st, applied) = propose(vm, &json!({"channel": "scene", "emit": 0, "value": true, "to": 150, "apply": true, "pick": 1}));
    assert_eq!(st, Status::Ok, "{applied:#}");
    assert_eq!(applied["applied"]["pick"], json!(1), "{applied:#}");
    assert!(std::fs::read_to_string(&path).unwrap().contains("config let px = 25"));
    {
        let v = unsafe { VmHandle::vm_mut(vm) }.unwrap();
        assert!(v.sources_changed());
        v.reload().unwrap();
    }
    assert_eq!(scene(vm), vec![150.0]);

    // Failures answer with the CLI's error object and set the last error.
    let (st, err) = propose(vm, &json!({"channel": "nope", "emit": 0, "value": true, "to": 1}));
    assert_eq!(st, Status::Runtime);
    assert_eq!((err["error"].clone(), err["phase"].clone()), (json!(true), json!("goal")), "{err:#}");
    let last = unsafe { &*pb_vm_last_error(vm) };
    assert_eq!(unsafe { CStr::from_ptr(last.phase) }.to_str().unwrap(), "goal");
    let (st, err) = propose(vm, &json!({"channel": "scene", "emit": 0, "value": true, "to": 1, "speling": 1}));
    assert_eq!(st, Status::InvalidArg);
    assert_eq!(err["phase"], json!("request"), "{err:#}");
    let (st, err) = propose(vm, &json!("not an object"));
    assert_eq!((st, err["error"].clone()), (Status::InvalidArg, json!(true)));

    pb_vm_destroy(vm);
}

#[test]
fn best_effort_policy_gives_one_answer_or_an_error() {
    let dir = std::env::temp_dir().join(format!("petal-bridge-best-effort-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scene.ptl");
    // Without the effect calls: the other test counts them, in parallel.
    std::fs::write(&path, SCRIPT.replace("play_sound(1)\ntally(1)\n", "")
        .replace("config let px = 10", "let px = 10.0\nlet off = 0.0\nconst FIXED = 0.0")
        .replace("to_screen(px)", "to_screen(px + off + FIXED)"))
    .unwrap();
    let vm = vm_with(&path);
    assert_eq!(scene(vm), vec![120.0]);

    // `px` and `off` tie; `FIXED` is a const and never moves. +30 on the
    // screen is +15 through `to_screen`, split equally.
    let req = json!({"channel": "scene", "emit": 0, "value": true, "to": 150, "policy": "best_effort"});
    let (st, report) = propose(vm, &req);
    assert_eq!(st, Status::Ok, "{report:#}");
    assert_eq!(report["policy"], json!("best_effort"));
    let alts = report["alternatives"].as_array().unwrap();
    assert_eq!(alts.len(), 1, "{report:#}");
    assert_eq!(alts[0]["verified"], json!(true), "{report:#}");
    let mut moved: Vec<(String, f64)> = alts[0]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["binding"].as_str().unwrap().to_string(), c["new"].as_f64().unwrap()))
        .collect();
    moved.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(moved, vec![("off".to_string(), 7.5), ("px".to_string(), 17.5)], "{report:#}");

    let fixed = report["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["binding"] == json!("FIXED"))
        .unwrap_or_else(|| panic!("{report:#}"));
    assert_eq!(fixed["eligible"], json!(false), "{fixed:#}");
    assert_eq!(fixed["flags"]["through_const"], json!(true), "{fixed:#}");

    // A goal no edit reaches is an error: `px` bounded to [0, 20] tops out
    // at 160 on the screen.
    std::fs::write(&path, std::fs::read_to_string(&path).unwrap()
        .replace("let px = 10.0\nlet off = 0.0", "let px = tunable(10.0, 0.0, 20.0)\nlet off = 0.0"))
    .unwrap();
    {
        let v = unsafe { VmHandle::vm_mut(vm) }.unwrap();
        v.reload().unwrap();
    }
    assert_eq!(scene(vm), vec![120.0]);
    let req = json!({"channel": "scene", "emit": 0, "value": true, "to": 200, "policy": "best_effort", "apply": true});
    let (st, err) = propose(vm, &req);
    assert_eq!((st, err["phase"].clone()), (Status::Runtime, json!("goal")), "{err:#}");
    assert!(err["message"].as_str().unwrap().contains("no edit reaches the goals"), "{err:#}");
    assert!(std::fs::read_to_string(&path).unwrap().contains("tunable(10.0, 0.0, 20.0)"), "nothing written");

    // An unknown policy is a bad request.
    let (st, err) = propose(vm, &json!({"channel": "scene", "emit": 0, "value": true, "to": 150, "policy": "best"}));
    assert_eq!((st, err["phase"].clone()), (Status::InvalidArg, json!("request")), "{err:#}");
    pb_vm_destroy(vm);
}
