//! Edit mode: drag a drawn item, and the code that drew it is edited
//! (docs/tasks/differentiation-rewrite.md, Design §3; user docs in
//! docs/features/direct-manipulation.md, "Hosts").
//!
//! F9 turns it on. Entering freezes the timeline at the frame on screen, so
//! the program stops, game input never reaches the script, and the
//! speculative re-runs have a stable baseline (the frame's recorded input,
//! [`Timeline::checkpoint_for_current`]). Then:
//!
//! - **Click** a drawn item: the overlay names the call that drew it (the
//!   emit trace the timeline already records), so the author knows which
//!   line a drag will change.
//! - **Drag** it: petal-sdl has no source view to select lines in, so the
//!   request runs under the best-effort policy (`tunable` literals first,
//!   never `const(…)` or `rigid`, an equal split across ties). The single
//!   answer is written to the file and the hot reload replays the frozen
//!   frame through the edited code. When no edit reaches the goal, the
//!   overlay says why and nothing is written.
//! - **`z`** undoes the last applied edit (when the file still holds it).
//! - **F9** again leaves edit mode, resuming the game if entering froze it.
//!
//! The same operations are agent-protocol commands (`edit_mode`,
//! `edit_pick`, `edit_drag`, `edit_undo`), which is how the headless
//! integration test drives it. The session itself, the goal and the
//! conversion of answers to rewrites, is the shared
//! [`petal_ui::edit_mode`], which Garden's drag mode uses too.

use std::path::{Path, PathBuf};

use petal::dm::NoHost;
use petal::env::Env;
use petal::execution_context::EmitSite;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal_ui::draw::DrawCommand;
use petal_ui::edit_mode::{
    DragAlternative, DragFilter, DragOutcomeV2, DragProperty, EditSession, GoalTarget, SessionOptions,
    apply_rewrites_to,
};
use petal_ui::input::InputState;

use crate::game_loop::Host;
use crate::timeline::{self, Timeline};

/// Pointer travel (pixels, both axes summed) below which a press and release
/// is a click, not a drag.
const DRAG_SLOP: i32 = 3;

/// Edit mode's state. It lives on the [`Timeline`] (`timeline.edit`), since
/// it works on the frozen frame.
#[derive(Default)]
pub struct EditMode {
    on: bool,
    /// Entering froze the timeline, so leaving resumes it.
    froze: bool,
    picked: Option<Picked>,
    /// Where the left button went down, while it is held.
    press: Option<(i32, i32)>,
    pointer: (i32, i32),
    was_down: bool,
    note: Option<Note>,
    undo: Option<Undo>,
}

/// The item the author clicked (or last dragged).
struct Picked {
    /// Its index in the frame's commands.
    index: usize,
    /// The timeline generation of the frame it was picked on: after a reload
    /// the index no longer means the same command.
    generation: u32,
    /// "`draw_circle` at line 7: draw_circle(x, 100, 20, …)".
    label: String,
}

/// What the overlay's message line says.
enum Note {
    Applied(String),
    Failed(String),
}

/// The last applied edit, per file written: what to put back.
struct Undo {
    files: Vec<(PathBuf, String, String)>,
    description: String,
}

/// An edit that was written.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Applied {
    /// "100 → 140 (line 3)", one per rewrite, joined.
    pub description: String,
    /// The files written.
    pub files: Vec<String>,
    /// The call that drew the dragged item.
    pub call: String,
}

/// What [`toggle`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Toggled {
    Entered,
    /// Left edit mode; `resume` when entering had frozen the game, so the
    /// caller unfreezes the timeline (and moves its clock).
    Left { resume: bool },
}

impl EditMode {
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// The label of the picked item, if any.
    pub fn picked_label(&self) -> Option<&str> {
        self.picked.as_ref().map(|p| p.label.as_str())
    }

    /// The edit mode part of the timeline's status.
    pub fn status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "on": self.on,
            "picked": self.picked.as_ref().map(|p| p.label.clone()),
            "applied": match &self.note { Some(Note::Applied(s)) => Some(s.clone()), _ => None },
            "error": match &self.note { Some(Note::Failed(s)) => Some(s.clone()), _ => None },
            "undo": self.undo.as_ref().map(|u| u.description.clone()),
        })
    }

    /// The overlay edit mode adds to the timeline's: a banner with the keys,
    /// the picked call, the last result, the picked item's outline and, while
    /// dragging, where it is being dragged to.
    pub fn overlay(&self, tl: &Timeline, width: i32, _height: i32) -> Vec<DrawCommand> {
        let mut cmds = Vec::new();
        if !self.on {
            return cmds;
        }
        let rec = tl.current();
        if let (Some(p), Some(rec)) = (self.picked.as_ref(), rec) {
            if p.generation == tl.generation && rec.generation == tl.generation {
                if let Some((x, y, w, h)) = rec.commands.get(p.index).and_then(timeline::bounds) {
                    cmds.push(outline(x - 3, y - 3, w + 6, h + 6, (255, 220, 60)));
                    if let Some((px, py)) = self.press {
                        let (dx, dy) = (self.pointer.0 - px, self.pointer.1 - py);
                        if dx.abs() + dy.abs() >= DRAG_SLOP {
                            cmds.push(outline(x - 3 + dx, y - 3 + dy, w + 6, h + 6, (120, 220, 255)));
                            cmds.push(DrawCommand::Line {
                                x1: px,
                                y1: py,
                                x2: self.pointer.0,
                                y2: self.pointer.1,
                                r: 120,
                                g: 220,
                                b: 255,
                                a: 200,
                                width: 1,
                            });
                        }
                    }
                }
            }
        }
        let lines = 1 + usize::from(self.picked.is_some()) + usize::from(self.note.is_some());
        cmds.push(DrawCommand::Rect {
            x: 0,
            y: 0,
            w: width.max(1) as u32,
            h: 8 + 18 * lines as u32,
            r: 10,
            g: 12,
            b: 20,
            a: 215,
            radius: 0,
        });
        let undo = if self.undo.is_some() { "   z undo" } else { "" };
        let banner = format!("EDIT MODE   click a shape to name its call, drag it to edit the code{undo}   F9 exit");
        cmds.push(DrawCommand::plain_text(&banner, 10, 4, 14, 255, 220, 60, 255));
        let mut y = 22;
        if let Some(p) = &self.picked {
            cmds.push(DrawCommand::plain_text(&p.label, 10, y, 14, 240, 240, 255, 255));
            y += 18;
        }
        match &self.note {
            Some(Note::Applied(s)) => {
                let text = format!("applied: {s}{undo}");
                cmds.push(DrawCommand::plain_text(&text, 10, y, 14, 120, 230, 140, 255));
            }
            Some(Note::Failed(s)) => {
                let text = format!("no edit: {s}");
                cmds.push(DrawCommand::plain_text(&text, 10, y, 14, 255, 120, 110, 255));
            }
            None => {}
        }
        cmds
    }
}

fn outline(x: i32, y: i32, w: u32, h: u32, (r, g, b): (u8, u8, u8)) -> DrawCommand {
    DrawCommand::RectOutline {
        x,
        y,
        w,
        h,
        r,
        g,
        b,
        a: 230,
        width: 2,
        radius: 3,
    }
}

/// Turn edit mode on or off. On freezes the timeline (when it is not
/// already frozen).
pub fn toggle(tl: &mut Timeline) -> Toggled {
    if tl.edit.on {
        let resume = tl.edit.froze;
        tl.edit.on = false;
        tl.edit.froze = false;
        tl.edit.press = None;
        tl.edit.picked = None;
        tl.edit.note = None;
        Toggled::Left { resume }
    } else {
        enter(tl);
        Toggled::Entered
    }
}

/// Turn edit mode on (freezing the timeline) if it is off.
pub fn enter(tl: &mut Timeline) {
    if tl.edit.on {
        return;
    }
    let froze = !tl.is_frozen();
    tl.freeze();
    tl.edit.on = true;
    tl.edit.froze = froze && tl.is_frozen();
    tl.edit.was_down = false;
    tl.edit.press = None;
}

/// Name the call that drew the item under (`x`, `y`) on the frame on screen,
/// and remember it as picked.
pub fn pick(tl: &mut Timeline, env: &Env, program_id: ProgramId, x: i32, y: i32) -> Result<String, String> {
    enter(tl);
    let (index, origin) = item_at(tl, x, y)?;
    let label = name_call(env, program_id, &origin).unwrap_or_else(|| "a call with no source position".to_string());
    tl.edit.picked = Some(Picked {
        index,
        generation: tl.generation,
        label: label.clone(),
    });
    Ok(label)
}

/// The command under (`x`, `y`) on the frame on screen and its emit site.
fn item_at(tl: &Timeline, x: i32, y: i32) -> Result<(usize, EmitSite), String> {
    let rec = tl.current().ok_or("nothing recorded yet")?;
    if rec.generation != tl.generation {
        return Err("the frame on screen was drawn by older code".to_string());
    }
    let index = timeline::hit_test(&rec.commands, x, y).ok_or("nothing is drawn there")?;
    let origin = rec.origins.get(index).cloned().unwrap_or_default();
    Ok((index, origin))
}

/// "`draw_circle` at line 7: draw_circle(x, 100, 20, …)": the call in the
/// sketch the emit came through, and its source line.
fn name_call(env: &Env, program_id: ProgramId, origin: &EmitSite) -> Option<String> {
    petal_ui::edit_mode::describe_call(env, program_id, origin).map(|c| c.label())
}

/// Drag the item under (`x`, `y`) by (`dx`, `dy`): a best-effort request
/// against a re-run of the frame on screen. The answer is written to the
/// program's files (`path` is the sketch), and the hot reload follows. On
/// any failure nothing is written and the overlay shows the reason.
#[allow(clippy::too_many_arguments)]
pub fn drag<H: Host>(
    tl: &mut Timeline,
    env: &mut Env,
    program_id: ProgramId,
    live: StackKey,
    path: Option<&str>,
    host: &mut H,
    (x, y): (i32, i32),
    (dx, dy): (f64, f64),
) -> Result<Applied, String> {
    let result = try_drag(tl, env, program_id, live, path, host, (x, y), (dx, dy));
    tl.edit.note = Some(match &result {
        Ok(a) => Note::Applied(a.description.clone()),
        Err(e) => Note::Failed(e.clone()),
    });
    result
}

#[allow(clippy::too_many_arguments)]
fn try_drag<H: Host>(
    tl: &mut Timeline,
    env: &mut Env,
    program_id: ProgramId,
    live: StackKey,
    path: Option<&str>,
    host: &mut H,
    (x, y): (i32, i32),
    (dx, dy): (f64, f64),
) -> Result<Applied, String> {
    let call = pick(tl, env, program_id, x, y)?;
    let (index, _) = item_at(tl, x, y)?;
    let path = path.ok_or("this program was not loaded from a file, so there is nowhere to write an edit")?;
    let disk = std::fs::read_to_string(path).map_err(|e| format!("could not read {path}: {e}"))?;
    let target = {
        let rec = tl.current().ok_or("nothing recorded yet")?;
        GoalTarget::at(&rec.origins, index, DragProperty::Position)
    };
    let checkpoint = tl.checkpoint_for_current(env, live, host)?;
    let opened = EditSession::open(
        env,
        &mut NoHost,
        checkpoint,
        &target,
        &DragFilter::default(),
        &SessionOptions::best_effort(),
    );
    env.drop_fork(checkpoint);
    let session = opened.map_err(outcome_error)?;
    if session.source() != disk {
        session.cancel(env);
        return Err(format!("{path} changed on disk since it was loaded; wait for the reload"));
    }
    let alt = match session.finish(env, &mut NoHost, program_id, &[dx, dy]) {
        DragOutcomeV2::Alternatives { mut alternatives, .. } if !alternatives.is_empty() => alternatives.remove(0),
        other => return Err(outcome_error(other)),
    };
    let files = write_alternative(Path::new(path), &alt)?;
    let description = alt.rewrites.iter().map(|r| r.description.clone()).collect::<Vec<_>>().join("; ");
    let applied = Applied {
        description: description.clone(),
        files: files.iter().map(|(p, _, _)| p.display().to_string()).collect(),
        call,
    };
    tl.edit.undo = Some(Undo { files, description });
    Ok(applied)
}

/// The reason to show for an outcome that is not an edit.
fn outcome_error(outcome: DragOutcomeV2) -> String {
    match outcome {
        DragOutcomeV2::Refused { why, .. } => why,
        DragOutcomeV2::Stale => "the program changed under the drag; try again".to_string(),
        DragOutcomeV2::Alternatives { .. } => "no edit reaches the goal".to_string(),
    }
}

/// Write `alt` into the sketch at `sketch` and any imported file it edits.
/// Every file's new text is computed before any is written, so a rewrite
/// that does not apply writes nothing. Returns `(path, before, after)` per
/// file written.
fn write_alternative(sketch: &Path, alt: &DragAlternative) -> Result<Vec<(PathBuf, String, String)>, String> {
    let mut groups: Vec<(PathBuf, Vec<petal_ui::edit_mode::SourceRewrite>)> = Vec::new();
    for r in &alt.rewrites {
        let file = r.file.clone().unwrap_or_else(|| sketch.to_path_buf());
        let mut r = r.clone();
        r.file = None;
        match groups.iter_mut().find(|(p, _)| *p == file) {
            Some((_, rs)) => rs.push(r),
            None => groups.push((file, vec![r])),
        }
    }
    let mut out = Vec::new();
    for (file, rewrites) in groups {
        let before = std::fs::read_to_string(&file).map_err(|e| format!("could not read {}: {e}", file.display()))?;
        let after = apply_rewrites_to(&before, &rewrites)?;
        out.push((file, before, after));
    }
    for (file, _, after) in &out {
        std::fs::write(file, after).map_err(|e| format!("could not write {}: {e}", file.display()))?;
    }
    Ok(out)
}

/// Put back the files the last applied edit wrote, when they still hold it.
pub fn undo(tl: &mut Timeline) -> Result<String, String> {
    let u = tl.edit.undo.take().ok_or("nothing to undo")?;
    for (file, _, after) in &u.files {
        let now = std::fs::read_to_string(file).unwrap_or_default();
        if now != *after {
            let msg = format!("{} changed since the edit; not undoing", file.display());
            tl.edit.note = Some(Note::Failed(msg.clone()));
            return Err(msg);
        }
    }
    for (file, before, _) in &u.files {
        std::fs::write(file, before).map_err(|e| format!("could not write {}: {e}", file.display()))?;
    }
    let msg = format!("undid {}", u.description);
    tl.edit.note = Some(Note::Applied(msg.clone()));
    Ok(msg)
}

/// The windowed loop's edit-mode input, read each frozen frame: a press
/// picks, a release past the slop drags, `z` undoes. Returns a line for the
/// log when something happened.
#[allow(clippy::too_many_arguments)]
pub fn handle_input<H: Host>(
    tl: &mut Timeline,
    env: &mut Env,
    program_id: ProgramId,
    live: StackKey,
    path: Option<&str>,
    host: &mut H,
    input: &InputState,
) -> Option<String> {
    if !tl.edit.on {
        return None;
    }
    let mut log = None;
    if input.was_key_pressed("z") {
        log = Some(match undo(tl) {
            Ok(m) => m,
            Err(e) => e,
        });
    }
    let (x, y) = (input.mouse_x, input.mouse_y);
    let down = input.is_button_down(0);
    tl.edit.pointer = (x, y);
    if down && !tl.edit.was_down {
        tl.edit.press = Some((x, y));
        log = Some(match pick(tl, env, program_id, x, y) {
            Ok(label) => label,
            Err(e) => {
                tl.edit.picked = None;
                e
            }
        });
    }
    if !down && tl.edit.was_down {
        if let Some((px, py)) = tl.edit.press.take() {
            let (dx, dy) = (x - px, y - py);
            if dx.abs() + dy.abs() >= DRAG_SLOP {
                log = Some(match drag(tl, env, program_id, live, path, host, (px, py), (dx as f64, dy as f64)) {
                    Ok(a) => format!("applied {} ({})", a.description, a.files.join(", ")),
                    Err(e) => format!("no edit: {e}"),
                });
            }
        }
    }
    tl.edit.was_down = down;
    log
}
