//! `pb_scenario`: petal-ui's declarative input scenarios over the C ABI.
//!
//! A scenario is JSON — input events keyed by frame number, plus an optional
//! window size and frame count (see `petal_ui::scenario` for the format). A
//! host replays one by calling `pb_vm_apply_scenario` (in [`crate::vm`]) with
//! the frame number before `pb_vm_begin_frame`, so headless C/C++ tests and
//! recorded repros use exactly the input model `petal-ui-run` uses. Applying is a pure
//! function of (scenario, frame), so a scenario is independent of any VM and
//! can drive several.

use std::ffi::{CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use petal_ui::scenario::Scenario;

use crate::ffi::{Status, arg_str, cstring_lossy, guard, panic_message};

/// The state behind a `pb_scenario*`.
#[derive(Default)]
pub struct PbScenario {
    pub scenario: Scenario,
    /// Message of the last failed load, if the most recent load failed.
    error: Option<CString>,
    /// Buffer behind `pb_scenario_to_json`.
    json: CString,
}

impl PbScenario {
    /// Replace the contents with a parse of `text`, or record why it failed
    /// (keeping the previous contents).
    fn load(&mut self, text: &str) -> Status {
        match Scenario::from_json_str(text) {
            Ok(s) => {
                self.scenario = s;
                self.error = None;
                Status::Ok
            }
            Err(e) => self.fail(Status::InvalidArg, format!("scenario: {e}")),
        }
    }

    fn fail(&mut self, code: Status, msg: String) -> Status {
        self.error = Some(cstring_lossy(&msg));
        code
    }

    /// One past the frame of the last event (0 when there are none).
    pub fn end_frame(&self) -> usize {
        self.scenario
            .events
            .iter()
            .map(|e| e.at + 1)
            .max()
            .unwrap_or(0)
    }
}

/// Run `f` on a scenario with NULL and panic protection.
fn with_scenario<T>(s: *mut PbScenario, fallback: T, f: impl FnOnce(&mut PbScenario) -> T) -> T {
    if s.is_null() {
        return fallback;
    }
    // SAFETY: a non-null pb_scenario* came from pb_scenario_new.
    let s = unsafe { &mut *s };
    match catch_unwind(AssertUnwindSafe(|| f(&mut *s))) {
        Ok(v) => v,
        Err(payload) => {
            s.error = Some(cstring_lossy(&format!(
                "petal-bridge internal panic: {}",
                panic_message(&*payload)
            )));
            fallback
        }
    }
}

/// Read a scenario (NULL yields `fallback`; a panic too).
fn read_scenario<T>(s: *const PbScenario, fallback: T, f: impl FnOnce(&PbScenario) -> T) -> T {
    if s.is_null() {
        return fallback;
    }
    // SAFETY: a non-null pb_scenario* came from pb_scenario_new.
    let s = unsafe { &*s };
    catch_unwind(AssertUnwindSafe(|| f(s))).unwrap_or(fallback)
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_scenario_new() -> *mut PbScenario {
    guard(std::ptr::null_mut(), || {
        Box::into_raw(Box::new(PbScenario::default()))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_scenario_free(s: *mut PbScenario) {
    if !s.is_null() {
        guard((), || drop(unsafe { Box::from_raw(s) }));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_scenario_load_json(s: *mut PbScenario, json: *const c_char) -> Status {
    let st = with_scenario(s, Status::Panic, |s| {
        match unsafe { arg_str(json, "json") } {
            Ok(text) => s.load(text),
            Err(e) => s.fail(e.code, e.message),
        }
    });
    if s.is_null() { Status::InvalidArg } else { st }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_scenario_load_file(s: *mut PbScenario, path: *const c_char) -> Status {
    let st = with_scenario(s, Status::Panic, |s| {
        let path = match unsafe { arg_str(path, "path") } {
            Ok(p) => p,
            Err(e) => return s.fail(e.code, e.message),
        };
        match std::fs::read_to_string(path) {
            Ok(text) => s.load(&text),
            Err(e) => s.fail(Status::Io, format!("cannot read {path}: {e}")),
        }
    });
    if s.is_null() { Status::InvalidArg } else { st }
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_scenario_monkey(
    s: *mut PbScenario,
    seed: u64,
    frames: usize,
    width: i32,
    height: i32,
) -> Status {
    let st = with_scenario(s, Status::Panic, |s| {
        s.scenario = Scenario::monkey(seed, frames, (width, height));
        s.error = None;
        Status::Ok
    });
    if s.is_null() { Status::InvalidArg } else { st }
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_scenario_error(s: *const PbScenario) -> *const c_char {
    if s.is_null() {
        return std::ptr::null();
    }
    unsafe { &*s }
        .error
        .as_ref()
        .map_or(std::ptr::null(), |e| e.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_scenario_event_count(s: *const PbScenario) -> usize {
    read_scenario(s, 0, |s| s.scenario.events.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_scenario_end_frame(s: *const PbScenario) -> usize {
    read_scenario(s, 0, |s| s.end_frame())
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_scenario_frames(s: *const PbScenario, out_frames: *mut usize) -> bool {
    read_scenario(s, false, |s| match s.scenario.frames {
        Some(n) => {
            if !out_frames.is_null() {
                unsafe { out_frames.write(n) };
            }
            true
        }
        None => false,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_scenario_size(
    s: *const PbScenario,
    out_width: *mut i32,
    out_height: *mut i32,
) -> bool {
    read_scenario(s, false, |s| match s.scenario.size {
        Some((w, h)) => {
            if !out_width.is_null() {
                unsafe { out_width.write(w) };
            }
            if !out_height.is_null() {
                unsafe { out_height.write(h) };
            }
            true
        }
        None => false,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_scenario_to_json(s: *mut PbScenario) -> *const c_char {
    with_scenario(s, std::ptr::null(), |s| {
        s.json = cstring_lossy(&s.scenario.to_json().to_string());
        s.json.as_ptr()
    })
}
