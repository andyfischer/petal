//! The `CAMERA` class: a revisioned class whose revision the host *sets* to
//! a fingerprint of its state (`Env::set_class_revision`,
//! `PetalCxt::set_class_revision`) instead of bumping it, and the
//! `ClassChange` record a command that changes a class in its call leaves in
//! its scope.
//!
//! The host here is an engine's frame camera: reset at the start of every
//! run, set by `cam_set` (absolute) and `cam_add` (relative, so re-issuing it
//! from another camera gives another one), and read by `proj` (a projection).
//! Memo off is the reference; memo on must draw the same on every frame, and
//! replay the readers on frames where nothing they depend on moved.

use std::cell::Cell;
use std::rc::Rc;

use petal::env::Env;
use petal::native_fn::{InputClasses, NativeEffects};
use petal::policy::RunPolicy;
use petal::stack::StackKey;
use petal::value::Value;

/// The camera's revision: a fingerprint of the camera (here, the camera
/// itself, offset so it is never a bumped counter's small value).
fn fingerprint(camera: i64) -> u64 {
    (camera as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xc0ff_ee00
}

#[derive(Default)]
struct Host {
    camera: Cell<i64>,
    input: Cell<i64>,
    projections: Cell<u64>,
}

struct World {
    env: Env,
    sid: StackKey,
    host: Rc<Host>,
}

/// What the host does when a command moved the camera.
#[derive(Clone, Copy, PartialEq)]
enum Notes {
    /// Sets the revision in the call (the design).
    InCall,
    /// Does not (a mutation the oracle must catch).
    Never,
}

/// The row the camera setters are registered with.
#[derive(Clone, Copy, PartialEq)]
enum Row {
    Command,
    Effect,
    /// Neither: a replay would drop the call, if Petal let it replay.
    Pure,
}

fn world(src: &str, memo: bool, notes: Notes, row: Row) -> World {
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.memo = memo;
    policy.gate = false;
    env.set_policy(policy);
    let host = Rc::new(Host::default());
    let settings = match row {
        Row::Command => NativeEffects::COMMAND,
        Row::Effect => NativeEffects::PURE.with_effect(),
        Row::Pure => NativeEffects::PURE,
    };

    let h = host.clone();
    env.register_native_boxed(
        "cam_set",
        move |c| {
            h.camera.set(c.get_int(1)?);
            if notes == Notes::InCall {
                c.set_class_revision(InputClasses::CAMERA, fingerprint(h.camera.get()));
            }
            c.push_nil();
            Ok(1)
        },
        settings,
    );
    let h = host.clone();
    env.register_native_boxed(
        "cam_add",
        move |c| {
            h.camera.set(h.camera.get() + c.get_int(1)?);
            if notes == Notes::InCall {
                c.set_class_revision(InputClasses::CAMERA, fingerprint(h.camera.get()));
            }
            c.push_nil();
            Ok(1)
        },
        settings,
    );
    let h = host.clone();
    env.register_native_boxed(
        "proj",
        move |c| {
            c.note_class_read(InputClasses::CAMERA);
            h.projections.set(h.projections.get() + 1);
            let p = c.get_int(1)?;
            c.push_int(h.camera.get() * 100 + p);
            Ok(1)
        },
        NativeEffects::query(InputClasses::CAMERA),
    );
    let h = host.clone();
    env.register_native_boxed(
        "inp",
        move |c| {
            c.push_int(h.input.get());
            Ok(1)
        },
        NativeEffects::probe(InputClasses::POINTER),
    );
    env.register_native(
        "put",
        |c| {
            let v = c.get_value(1)?;
            let sym = c.intern_symbol("out");
            c.push_output(sym, v);
            c.push_nil();
            Ok(1)
        },
        NativeEffects::EMITS,
    );
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("load: {e}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, sid, host }
}

impl World {
    /// One frame: the camera is reset (and its revision set to the reset
    /// camera's) before the run, as an engine does.
    fn frame(&mut self, input: i64) -> Vec<i64> {
        self.host.input.set(input);
        self.host.camera.set(0);
        self.env.set_class_revision(self.sid, InputClasses::CAMERA, fingerprint(0));
        self.env.reset_stack(self.sid).unwrap();
        self.env.run(self.sid).unwrap_or_else(|e| panic!("run: {e}"));
        let _ = self.env.take_output();
        let sym = self.env.intern_symbol("out");
        let values = self.env.take_output_buffer(sym);
        values
            .iter()
            .map(|v| match v {
                Value::Int(i) => *i,
                other => panic!("not an int: {other:?}"),
            })
            .collect()
    }

    fn hits(&self) -> u64 {
        self.env.memo_stats(self.sid).unwrap().hits
    }
}

const GAME: &str = r#"
native fn cam_set(x)
native fn cam_add(d)
native fn proj(p)
native fn inp()
native fn put(v)

fn hud(p)
  put(proj(p))
  nil
end

fn shot(x, p)
  cam_set(x)
  put(proj(p))
  nil
end

fn nudge(d, p)
  cam_add(d)
  put(proj(p))
  nil
end

fn scene(x, p)
  shot(x, p)
  hud(p + 1)
  nudge(1, p + 2)
  nil
end

let x = inp()
hud(1)
shot(x, 2)
hud(3)
nudge(1, 4)
hud(5)
scene(x, 6)
hud(9)
"#;

/// The inputs, frame by frame: a camera that holds, moves, holds, moves to
/// where `nudge` left it the frame before (so a scope that checked only the
/// revision its read saw would replay stale), and back.
const INPUTS: [i64; 10] = [1, 1, 1, 2, 2, 3, 3, 3, 1, 1];

fn run_all(notes: Notes, row: Row) -> (Vec<Vec<i64>>, Vec<Vec<i64>>, Vec<u64>) {
    let mut off = world(GAME, false, Notes::InCall, Row::Command);
    let mut on = world(GAME, true, notes, row);
    let (mut want, mut got, mut asked) = (Vec::new(), Vec::new(), Vec::new());
    for &x in &INPUTS {
        want.push(off.frame(x));
        let before = on.host.projections.get();
        got.push(on.frame(x));
        asked.push(on.host.projections.get() - before);
    }
    assert!(on.hits() > 0);
    (want, got, asked)
}

#[test]
fn memo_on_and_off_agree_and_the_readers_replay_while_the_camera_holds() {
    let (want, got, asked) = run_all(Notes::InCall, Row::Command);
    for (f, (w, g)) in want.iter().zip(&got).enumerate() {
        assert_eq!(w, g, "frame {f} (input {})", INPUTS[f]);
    }
    // 9 projections a frame when everything runs; on a frame whose camera is
    // the one of the frame before, every reader replays, even `shot` and
    // `nudge`, which set the camera and then read it.
    assert_eq!(asked[0], 9);
    for f in [1, 2, 4, 6, 7, 9] {
        assert_eq!(asked[f], 0, "frame {f} replays everything: {asked:?}");
    }
    for f in [3, 5, 8] {
        assert!(asked[f] > 0, "frame {f} moved the camera: {asked:?}");
    }
    // `hud(1)` reads the reset camera, which never moves: it replays on
    // the frames the others run too.
    assert!(asked[3] < 9 && asked[5] < 9 && asked[8] < 9, "{asked:?}");
}

#[test]
fn without_the_in_call_revision_the_oracle_sees_a_stale_replay() {
    let (want, got, _) = run_all(Notes::Never, Row::Command);
    assert_ne!(want, got, "a camera change the class does not see must show");
}

#[test]
fn a_camera_setter_that_is_not_a_command_keeps_its_scopes_running() {
    // As an effect: correct, and nothing that sets the camera replays (the
    // four projections of `shot`, `nudge` and `scene`'s two).
    let (want, got, asked) = run_all(Notes::InCall, Row::Effect);
    assert_eq!(want, got);
    assert_eq!(asked[1], 4, "the setters' scopes run: {asked:?}");
    // Neither an effect nor a command, a call that moves a class in its call
    // still makes its scope run: a replay would drop it.
    let (want, got, asked) = run_all(Notes::InCall, Row::Pure);
    assert_eq!(want, got);
    assert_eq!(asked[1], 4, "the setters' scopes run: {asked:?}");
}

/// A scope that reads the camera, moves it relatively, and reads again: the
/// second read is checked from the first's revision through the change, so
/// it replays only from the same starting camera.
#[test]
fn a_relative_change_is_followed_from_where_it_started() {
    const SRC: &str = r#"
native fn cam_set(x)
native fn cam_add(d)
native fn proj(p)
native fn inp()
native fn put(v)

fn twice(p)
  put(proj(p))
  cam_add(1)
  put(proj(p + 1))
  nil
end

cam_set(inp())
twice(1)
twice(3)
"#;
    let mut off = world(SRC, false, Notes::InCall, Row::Command);
    let mut on = world(SRC, true, Notes::InCall, Row::Command);
    let mut replays = 0;
    for x in [5, 5, 6, 6, 7, 6, 6] {
        let before = on.host.projections.get();
        assert_eq!(off.frame(x), on.frame(x), "input {x}");
        if on.host.projections.get() == before {
            replays += 1;
        }
    }
    assert_eq!(replays, 3, "the repeated frames replay");
}

/// A pure child re-executed during its parent's validation takes a branch
/// that sets the camera and then reads it. The re-execution defers the
/// command (its parent's replay or run issues it), so the read after it sees
/// the camera from before: the record it leaves must not replay that answer
/// once the command is issued.
#[test]
fn a_command_deferred_by_a_re_execution_does_not_leave_a_stale_read() {
    const SRC: &str = r#"
native fn cam_set(x)
native fn cam_add(d)
native fn proj(p)
native fn inp()
native fn put(v)

fn aim(p)
  if inp() > 1 then
    cam_set(9)
  end
  put(proj(p))
  nil
end

fn view(p)
  aim(p)
  nil
end

view(1)
"#;
    let mut off = world(SRC, false, Notes::InCall, Row::Command);
    let mut on = world(SRC, true, Notes::InCall, Row::Command);
    for x in [1, 1, 2, 2, 2, 1, 2] {
        assert_eq!(off.frame(x), on.frame(x), "input {x}");
    }
}
