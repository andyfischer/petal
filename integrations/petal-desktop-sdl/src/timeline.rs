//! Time travel for the game loop.
//!
//! After every committed frame the loop forks the live execution
//! (`Env::fork_execution`) and files the fork, the frame's input, and what the
//! frame drew as a [`FrameRecord`]. That ring of executions is what makes the
//! rest possible:
//!
//! - **Freeze and scrub.** Stop the game and step through recorded frames.
//!   Scrubbing re-presents recorded draw commands; nothing re-runs.
//! - **Rewind.** Unfreezing at an earlier frame restores that fork into the
//!   live stack (`Env::restore_execution`) and the game continues from there,
//!   with the frames after it discarded — the player takes over again.
//! - **Replay through an edit.** A hot reload while frozen re-simulates every
//!   recorded frame after the cursor through the *new* program, feeding each
//!   frame the input that was recorded for it. The future on screen is now
//!   the future the edited code produces — Bret Victor's "change the code and
//!   watch the trajectory move" gesture, built on the language's own fork and
//!   state-transfer primitives rather than a game-specific replay system.
//! - **Trails.** Point at a shape and the emit trace attributes it to the
//!   `draw_*` call that made it. That call site is then found in every recorded
//!   frame and its positions are drawn as a path: where this thing has been,
//!   and (when frozen with replayed frames ahead) where it is going.
//!
//! Every recorded frame is a full heap copy, so the ring is capped
//! ([`Timeline::new`]); ten seconds at 60 fps is the default.

use std::collections::{HashMap, VecDeque};

use petal::env::Env;
use petal::execution_context::EmitSite;
use petal::program::{ProgramId, TermId};
use petal::provenance::{self, CallSite};
use petal::source_map::ENTRY_FILE;
use petal::stack::StackKey;

use petal_ui::draw::{DrawCommand, clear_draw_commands, take_draw_commands_traced};
use petal_ui::input::{InputState, bind_frame_info, bind_input, bind_time};

use crate::game_loop::Host;

/// One committed frame: the execution after it ran, and everything needed to
/// run it again.
pub struct FrameRecord {
    pub frame_count: i64,
    pub dt: f64,
    pub time: f64,
    /// The input as bound for this frame (edges already promoted).
    pub input: InputState,
    /// A fork of the live execution taken after this frame ran.
    pub snapshot: StackKey,
    pub commands: Vec<DrawCommand>,
    /// Index-aligned with `commands`; empty chains when untraced.
    pub origins: Vec<EmitSite>,
    /// Which program produced this frame (see [`Timeline::generation`]).
    pub generation: u32,
}

/// The `draw_*` call being followed through time. Term ids are per program,
/// so the site is re-resolved by source position after each reload and the
/// id for every generation is remembered — old frames resolve against the id
/// their program used.
struct Tracked {
    callee: Option<String>,
    line: u32,
    column: u32,
    terms: HashMap<u32, TermId>,
}

/// One point of a trail: where the tracked call drew on one recorded frame.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TrailPoint {
    pub index: usize,
    pub frame: i64,
    pub x: i32,
    pub y: i32,
}

pub struct Timeline {
    frames: VecDeque<FrameRecord>,
    cap: usize,
    /// `Some(i)` while frozen at `frames[i]`.
    pub cursor: Option<usize>,
    /// Bumped on every hot reload; frames remember which one drew them.
    pub generation: u32,
    tracked: Option<Tracked>,
    pub show_trail: bool,
    /// Frames re-simulated by the last replay (for the status line).
    pub last_replayed: usize,
}

impl Timeline {
    pub fn new(cap: usize) -> Self {
        Self {
            frames: VecDeque::new(),
            cap: cap.max(2),
            cursor: None,
            generation: 0,
            tracked: None,
            show_trail: false,
            last_replayed: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn is_frozen(&self) -> bool {
        self.cursor.is_some()
    }

    /// The frame on screen: the cursor while frozen, else the latest.
    pub fn current_index(&self) -> Option<usize> {
        self.cursor.or_else(|| self.frames.len().checked_sub(1))
    }

    pub fn current(&self) -> Option<&FrameRecord> {
        self.current_index().and_then(|i| self.frames.get(i))
    }

    pub fn get(&self, i: usize) -> Option<&FrameRecord> {
        self.frames.get(i)
    }

    /// Forget every recorded frame (a script switch).
    pub fn clear(&mut self, env: &mut Env) {
        for rec in self.frames.drain(..) {
            env.drop_fork(rec.snapshot);
        }
        self.cursor = None;
        self.tracked = None;
    }

    // ── Recording ────────────────────────────────────────────────────────

    /// File the frame that just ran: drain its traced draw commands and fork
    /// the live execution. Call after `env.run`, before presenting.
    pub fn record(
        &mut self,
        env: &mut Env,
        live: StackKey,
        frame_count: i64,
        dt: f64,
        time: f64,
        input: &InputState,
    ) -> Result<(), String> {
        let (commands, origins) = drain_traced(env);
        let snapshot = env.fork_execution(live)?;
        self.frames.push_back(FrameRecord {
            frame_count,
            dt,
            time,
            input: input.clone(),
            snapshot,
            commands,
            origins,
            generation: self.generation,
        });
        while self.frames.len() > self.cap {
            if let Some(old) = self.frames.pop_front() {
                env.drop_fork(old.snapshot);
                if let Some(c) = self.cursor {
                    self.cursor = Some(c.saturating_sub(1));
                }
            }
        }
        Ok(())
    }

    // ── Freeze / scrub / unfreeze ────────────────────────────────────────

    pub fn freeze(&mut self) {
        if self.cursor.is_none() && !self.frames.is_empty() {
            self.cursor = Some(self.frames.len() - 1);
        }
    }

    pub fn scrub_by(&mut self, delta: i64) {
        if let Some(c) = self.cursor {
            let max = self.frames.len() as i64 - 1;
            self.cursor = Some((c as i64 + delta).clamp(0, max.max(0)) as usize);
        }
    }

    /// Move the cursor to an absolute index; negative counts from the end
    /// (`-1` = the latest frame).
    pub fn scrub_to(&mut self, to: i64) {
        if self.frames.is_empty() {
            return;
        }
        let n = self.frames.len() as i64;
        let idx = if to < 0 { n + to } else { to }.clamp(0, n - 1);
        self.cursor = Some(idx as usize);
    }

    /// Resume from the cursor: the live execution becomes the cursor frame's
    /// snapshot and the frames after it are discarded. Returns the resumed
    /// frame's `(frame_count, time)` so the loop's clock can follow.
    pub fn unfreeze(&mut self, env: &mut Env, live: StackKey) -> Result<Option<(i64, f64)>, String> {
        let Some(c) = self.cursor.take() else {
            return Ok(None);
        };
        let Some(rec) = self.frames.get(c) else {
            return Ok(None);
        };
        let at = (rec.frame_count, rec.time);
        env.restore_execution(live, rec.snapshot)?;
        self.truncate_after(env, c);
        Ok(Some(at))
    }

    /// Rewind the live game by `n` frames without freezing: restore that
    /// frame's snapshot and forget everything after it.
    pub fn rewind(&mut self, env: &mut Env, live: StackKey, n: usize) -> Result<Option<(i64, f64)>, String> {
        if self.frames.is_empty() {
            return Ok(None);
        }
        let target = (self.frames.len() - 1).saturating_sub(n);
        self.cursor = Some(target);
        self.unfreeze(env, live)
    }

    fn truncate_after(&mut self, env: &mut Env, keep: usize) {
        while self.frames.len() > keep + 1 {
            if let Some(rec) = self.frames.pop_back() {
                env.drop_fork(rec.snapshot);
            }
        }
    }

    // ── Replay ───────────────────────────────────────────────────────────

    /// Re-simulate frames `from..` through the program loaded *now*, feeding
    /// each the input recorded for it. Their snapshots and draw output are
    /// replaced. The live execution is left at the last replayed frame; a
    /// frozen loop restores the cursor on unfreeze anyway.
    pub fn replay<H: Host>(
        &mut self,
        env: &mut Env,
        live: StackKey,
        from: usize,
        host: &mut H,
    ) -> Result<usize, String> {
        let n = self.frames.len();
        // Frame 0 has no predecessor to restart from; replay begins at 1.
        let from = from.max(1);
        if n == 0 || from >= n {
            self.last_replayed = 0;
            return Ok(0);
        }
        let start = self.frames[from - 1].snapshot;
        env.restore_execution(live, start)?;
        let generation = self.generation;
        for j in from..n {
            let rec = &mut self.frames[j];
            clear_draw_commands(env);
            host.prepare_frame(env);
            env.advance_frame(live);
            bind_frame_info(env, rec.dt, rec.frame_count);
            bind_time(env, rec.time);
            bind_input(env, &rec.input);
            env.reset_stack(live)?;
            if let Err(e) = env.run(live) {
                eprintln!("[timeline] replay frame {}: {}", rec.frame_count, e);
            }
            // What the frame printed the first time already reached stderr.
            let _ = env.take_output();
            let (commands, origins) = drain_traced(env);
            rec.commands = commands;
            rec.origins = origins;
            let snapshot = env.fork_execution(live)?;
            env.drop_fork(rec.snapshot);
            rec.snapshot = snapshot;
            rec.generation = generation;
        }
        self.last_replayed = n - from;
        Ok(n - from)
    }

    /// The program changed under the loop. Old frames keep their old ids;
    /// the tracked site is re-found in the new program by source position.
    pub fn on_reload(&mut self, env: &Env, program_id: ProgramId) {
        self.generation += 1;
        let generation = self.generation;
        if let (Some(t), Some(program)) = (self.tracked.as_mut(), env.get_program(program_id)) {
            if let Some(term) = find_call_site(program, t.callee.as_deref(), t.line, t.column) {
                t.terms.insert(generation, term);
            }
        }
    }

    // ── Tracking ─────────────────────────────────────────────────────────

    /// Pick the shape under `(x, y)` on the frame on screen and follow the
    /// call that drew it. Returns the resolved call, or `None` when nothing
    /// attributable is there.
    pub fn track_at(&mut self, env: &Env, program_id: ProgramId, x: i32, y: i32) -> Option<CallSite> {
        let rec = self.current()?;
        // A frame drawn by an older program carries ids that no longer mean
        // anything; refuse rather than point at unrelated code.
        if rec.generation != self.generation {
            return None;
        }
        let program = env.get_program(program_id)?;
        let i = hit_test(&rec.commands, x, y)?;
        let chain = &rec.origins.get(i)?.chain;
        let term = provenance::pick_frame(program, chain, ENTRY_FILE)?;
        let site = CallSite::resolve(program, term)?;
        let (line, column) = site
            .span
            .map(|s| (s.start.line as u32, s.start.column as u32))
            .unwrap_or((0, 0));
        let mut terms = HashMap::new();
        terms.insert(self.generation, term);
        self.tracked = Some(Tracked {
            callee: site.callee.clone(),
            line,
            column,
            terms,
        });
        self.show_trail = true;
        Some(site)
    }

    pub fn untrack(&mut self) {
        self.tracked = None;
    }

    /// A description of the tracked call for a status line.
    pub fn tracked_label(&self) -> Option<String> {
        let t = self.tracked.as_ref()?;
        Some(format!(
            "{} (line {})",
            t.callee.as_deref().unwrap_or("call"),
            t.line
        ))
    }

    /// Where the tracked call drew on every recorded frame.
    pub fn trail(&self, env: &Env, program_id: ProgramId) -> Vec<TrailPoint> {
        let mut out = Vec::new();
        let (Some(t), Some(_program)) = (self.tracked.as_ref(), env.get_program(program_id)) else {
            return out;
        };
        for (index, rec) in self.frames.iter().enumerate() {
            let Some(term) = t.terms.get(&rec.generation) else {
                continue;
            };
            for (cmd, site) in rec.commands.iter().zip(rec.origins.iter()) {
                if site.chain.is_empty() {
                    continue;
                }
                // Frames drawn by an older program are attributed against
                // that program's ids, which `pick_frame` only reads through
                // the source map; a cheap membership test is the honest
                // equivalent and avoids resolving stale ids.
                if !site.chain.contains(term) {
                    continue;
                }
                if let Some((x, y)) = centroid(cmd) {
                    out.push(TrailPoint {
                        index,
                        frame: rec.frame_count,
                        x,
                        y,
                    });
                }
            }
        }
        out
    }

    // ── Overlay ──────────────────────────────────────────────────────────

    /// Draw commands the host paints over the frame: the trail, the tracked
    /// shape's outline, and a timeline bar while frozen.
    pub fn overlay(&self, env: &Env, program_id: ProgramId, width: i32, height: i32) -> Vec<DrawCommand> {
        let mut cmds = Vec::new();
        let cur = self.current_index();

        if self.show_trail {
            let points = self.trail(env, program_id);
            let mut prev: Option<(usize, i32, i32)> = None;
            for p in &points {
                let future = cur.is_some_and(|c| p.index > c);
                let (r, g, b) = if future { (255, 150, 40) } else { (60, 200, 255) };
                // Fade the deep past so the recent path reads strongest.
                let dist = cur.map_or(0, |c| c.abs_diff(p.index));
                let a = (255 - (dist as i64 * 255 / self.cap as i64).min(200)) as u8;
                if let Some((pi, px, py)) = prev {
                    if p.index == pi + 1 || (p.index == pi) {
                        cmds.push(DrawCommand::Line {
                            x1: px,
                            y1: py,
                            x2: p.x,
                            y2: p.y,
                            r,
                            g,
                            b,
                            a,
                            width: 2,
                        });
                    }
                }
                if p.index % 6 == 0 || Some(p.index) == cur {
                    cmds.push(DrawCommand::Circle {
                        cx: p.x,
                        cy: p.y,
                        radius: if Some(p.index) == cur { 5 } else { 3 },
                        r,
                        g,
                        b,
                        a,
                    });
                }
                prev = Some((p.index, p.x, p.y));
            }
            // Outline the tracked shape on the frame on screen.
            if let (Some(t), Some(rec)) = (self.tracked.as_ref(), self.current()) {
                if let Some(term) = t.terms.get(&rec.generation) {
                    for (cmd, site) in rec.commands.iter().zip(rec.origins.iter()) {
                        if site.chain.contains(term) {
                            if let Some((x, y, w, h)) = bounds(cmd) {
                                cmds.push(DrawCommand::RectOutline {
                                    x: x - 3,
                                    y: y - 3,
                                    w: w + 6,
                                    h: h + 6,
                                    r: 255,
                                    g: 255,
                                    b: 255,
                                    a: 230,
                                    width: 2,
                                    radius: 3,
                                });
                            }
                        }
                    }
                }
            }
        }

        if let Some(c) = self.cursor {
            let n = self.frames.len().max(1);
            let bar_y = height - 26;
            cmds.push(DrawCommand::Rect {
                x: 0,
                y: bar_y,
                w: width as u32,
                h: 26,
                r: 10,
                g: 12,
                b: 20,
                a: 210,
                radius: 0,
            });
            let track_x = 12;
            let track_w = (width - 24).max(10);
            cmds.push(DrawCommand::Rect {
                x: track_x,
                y: bar_y + 11,
                w: track_w as u32,
                h: 4,
                r: 70,
                g: 80,
                b: 110,
                a: 255,
                radius: 2,
            });
            // Frames the last replay rewrote glow orange: that is the future
            // the edit produced.
            if self.last_replayed > 0 && c + 1 < n {
                let x0 = track_x + (track_w as i64 * (c as i64 + 1) / n as i64) as i32;
                let x1 = track_x + track_w;
                cmds.push(DrawCommand::Rect {
                    x: x0,
                    y: bar_y + 11,
                    w: (x1 - x0).max(1) as u32,
                    h: 4,
                    r: 255,
                    g: 150,
                    b: 40,
                    a: 255,
                    radius: 2,
                });
            }
            let cx = track_x + (track_w as i64 * c as i64 / (n as i64 - 1).max(1)) as i32;
            cmds.push(DrawCommand::Rect {
                x: cx - 3,
                y: bar_y + 5,
                w: 6,
                h: 16,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                radius: 2,
            });
            let frame = self.frames.get(c).map_or(0, |r| r.frame_count);
            let mut label = format!("FROZEN  frame {}   ({}/{})   , . scrub   F5 resume", frame, c + 1, n);
            if let Some(t) = self.tracked_label() {
                label.push_str(&format!("   tracking {}", t));
            }
            cmds.push(DrawCommand::plain_text(&label, 12, bar_y - 20, 14, 240, 240, 255, 255));
        }
        cmds
    }

    /// A JSON summary for the agent protocol.
    pub fn status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "frames": self.frames.len(),
            "capacity": self.cap,
            "cursor": self.cursor,
            "frozen": self.cursor.is_some(),
            "first_frame": self.frames.front().map(|r| r.frame_count),
            "last_frame": self.frames.back().map(|r| r.frame_count),
            "generation": self.generation,
            "tracking": self.tracked_label(),
            "last_replayed": self.last_replayed,
        })
    }
}

fn drain_traced(env: &mut Env) -> (Vec<DrawCommand>, Vec<EmitSite>) {
    take_draw_commands_traced(env).into_iter().unzip()
}

/// Find the call in `program` written at (or nearest below) `line`, with the
/// given callee, matching a tracked site across a reload. Column breaks ties
/// between several calls on one line.
fn find_call_site(program: &petal::program::Program, callee: Option<&str>, line: u32, column: u32) -> Option<TermId> {
    let mut best: Option<(u32, u32, TermId)> = None;
    for i in 0..program.terms.len() {
        let term = TermId(i as u32);
        let Some(span) = program.source_map.get(term) else {
            continue;
        };
        if span.file != ENTRY_FILE || span.start.line == 0 {
            continue;
        }
        let Some(site) = CallSite::resolve(program, term) else {
            continue;
        };
        if site.callee.as_deref() != callee || site.callee.is_none() {
            continue;
        }
        let dl = (span.start.line as u32).abs_diff(line);
        let dc = (span.start.column as u32).abs_diff(column);
        if best.is_none_or(|(bl, bc, _)| (dl, dc) < (bl, bc)) {
            best = Some((dl, dc, term));
        }
    }
    // Only accept a match that is plausibly the same call: a few lines off
    // (an edit above it) is fine, a different part of the file is not.
    best.filter(|(dl, _, _)| *dl <= 40).map(|(_, _, t)| t)
}

// ── Geometry over draw commands ──────────────────────────────────────────

/// Which command paints the point: the last one covering it, in paint order,
/// honoring clips and skipping offscreen targets (see the hit-test notes in
/// docs/direct-manipulation.md).
pub fn hit_test(cmds: &[DrawCommand], x: i32, y: i32) -> Option<usize> {
    let mut clip: Option<(i32, i32, i32, i32)> = None;
    let mut saved: Vec<Option<(i32, i32, i32, i32)>> = Vec::new();
    let mut target = 0u32;
    let mut hit = None;
    for (i, cmd) in cmds.iter().enumerate() {
        match cmd {
            DrawCommand::Clip { x: cx, y: cy, w, h, .. } => {
                clip = Some((*cx, *cy, *cx + *w as i32, *cy + *h as i32));
                continue;
            }
            DrawCommand::ClipNone => {
                clip = None;
                continue;
            }
            DrawCommand::ClipPush { x: cx, y: cy, w, h, .. } => {
                saved.push(clip);
                let want = (*cx, *cy, *cx + *w as i32, *cy + *h as i32);
                clip = Some(match clip {
                    Some(c) => (c.0.max(want.0), c.1.max(want.1), c.2.min(want.2), c.3.min(want.3)),
                    None => want,
                });
                continue;
            }
            DrawCommand::ClipPop => {
                clip = saved.pop().flatten();
                continue;
            }
            DrawCommand::SetTarget { id } => {
                target = *id;
                continue;
            }
            _ => {}
        }
        if target != 0 {
            continue;
        }
        if let Some((x0, y0, x1, y1)) = clip {
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                continue;
            }
        }
        if contains(cmd, x, y) {
            hit = Some(i);
        }
    }
    hit
}

fn in_rect(rx: i32, ry: i32, w: u32, h: u32, x: i32, y: i32) -> bool {
    x >= rx && x < rx + w as i32 && y >= ry && y < ry + h as i32
}

fn near_segment(x1: i32, y1: i32, x2: i32, y2: i32, x: i32, y: i32, tol: f64) -> bool {
    let (ax, ay, bx, by, px, py) = (x1 as f64, y1 as f64, x2 as f64, y2 as f64, x as f64, y as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) };
    let (qx, qy) = (ax + t * dx, ay + t * dy);
    ((px - qx).powi(2) + (py - qy).powi(2)).sqrt() <= tol
}

fn in_polygon(points: &[(i32, i32)], x: i32, y: i32) -> bool {
    let mut inside = false;
    let n = points.len();
    if n < 3 {
        return false;
    }
    let (px, py) = (x as f64, y as f64);
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (points[i].0 as f64, points[i].1 as f64);
        let (xj, yj) = (points[j].0 as f64, points[j].1 as f64);
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Whether one command's painted geometry covers the point. A full-canvas
/// clear never matches, an outline is hit on its stroke, and lines get a
/// few pixels of tolerance.
fn contains(cmd: &DrawCommand, x: i32, y: i32) -> bool {
    match cmd {
        DrawCommand::Rect { x: rx, y: ry, w, h, .. }
        | DrawCommand::RectGradient { x: rx, y: ry, w, h, .. }
        | DrawCommand::Image { x: rx, y: ry, w, h, .. }
        | DrawCommand::DrawCanvas { x: rx, y: ry, w, h, .. } => in_rect(*rx, *ry, *w, *h, x, y),
        DrawCommand::RectOutline { x: rx, y: ry, w, h, width, .. } => {
            let t = (*width).max(1) as i32 + 2;
            in_rect(*rx - 1, *ry - 1, *w + 2, *h + 2, x, y)
                && !in_rect(*rx + t, *ry + t, w.saturating_sub(2 * t as u32), h.saturating_sub(2 * t as u32), x, y)
        }
        DrawCommand::Circle { cx, cy, radius, .. } | DrawCommand::CircleGradient { cx, cy, radius, .. } => {
            let (dx, dy) = ((x - cx) as i64, (y - cy) as i64);
            dx * dx + dy * dy <= (*radius as i64) * (*radius as i64)
        }
        DrawCommand::Text { text, x: tx, y: ty, size, .. } => {
            let w = (text.chars().count() as f64 * *size as f64 * 0.55) as u32;
            in_rect(*tx, *ty, w.max(4), (*size as u32).max(4), x, y)
        }
        DrawCommand::Line { x1, y1, x2, y2, width, .. } => {
            near_segment(*x1, *y1, *x2, *y2, x, y, 3.0 + *width as f64 / 2.0)
        }
        DrawCommand::Triangle { x1, y1, x2, y2, x3, y3, .. } => in_polygon(&[(*x1, *y1), (*x2, *y2), (*x3, *y3)], x, y),
        DrawCommand::Poly { points, .. } | DrawCommand::Polygon { points, .. } => in_polygon(points, x, y),
        _ => false,
    }
}

/// The visual centre of a command, for a trail point.
pub fn centroid(cmd: &DrawCommand) -> Option<(i32, i32)> {
    bounds(cmd).map(|(x, y, w, h)| (x + w as i32 / 2, y + h as i32 / 2))
}

/// The axis-aligned box of a command.
pub fn bounds(cmd: &DrawCommand) -> Option<(i32, i32, u32, u32)> {
    match cmd {
        DrawCommand::Rect { x, y, w, h, .. }
        | DrawCommand::RectOutline { x, y, w, h, .. }
        | DrawCommand::RectGradient { x, y, w, h, .. }
        | DrawCommand::Image { x, y, w, h, .. }
        | DrawCommand::DrawCanvas { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
        DrawCommand::Circle { cx, cy, radius, .. } | DrawCommand::CircleGradient { cx, cy, radius, .. } => {
            Some((cx - radius, cy - radius, (*radius as u32) * 2, (*radius as u32) * 2))
        }
        DrawCommand::Text { text, x, y, size, .. } => {
            let w = (text.chars().count() as f64 * *size as f64 * 0.55) as u32;
            Some((*x, *y, w.max(1), *size as u32))
        }
        DrawCommand::Line { x1, y1, x2, y2, .. } => Some((
            *x1.min(x2),
            *y1.min(y2),
            (x1 - x2).unsigned_abs().max(1),
            (y1 - y2).unsigned_abs().max(1),
        )),
        DrawCommand::Triangle { x1, y1, x2, y2, x3, y3, .. } => {
            poly_bounds(&[(*x1, *y1), (*x2, *y2), (*x3, *y3)])
        }
        DrawCommand::Poly { points, .. } | DrawCommand::Polygon { points, .. } => poly_bounds(points),
        _ => None,
    }
}

fn poly_bounds(points: &[(i32, i32)]) -> Option<(i32, i32, u32, u32)> {
    let x0 = points.iter().map(|p| p.0).min()?;
    let y0 = points.iter().map(|p| p.1).min()?;
    let x1 = points.iter().map(|p| p.0).max()?;
    let y1 = points.iter().map(|p| p.1).max()?;
    Some((x0, y0, (x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> DrawCommand {
        DrawCommand::Rect { x, y, w, h, r: 0, g: 0, b: 0, a: 255, radius: 0 }
    }

    #[test]
    fn hit_test_picks_the_last_shape_painted_over_a_point() {
        let cmds = vec![
            DrawCommand::Clear { r: 0, g: 0, b: 0 },
            rect(0, 0, 100, 100),
            rect(40, 40, 20, 20),
        ];
        assert_eq!(hit_test(&cmds, 50, 50), Some(2));
        assert_eq!(hit_test(&cmds, 10, 10), Some(1));
        assert_eq!(hit_test(&cmds, 200, 200), None, "a clear never matches");
    }

    #[test]
    fn outline_is_hit_on_its_stroke_not_its_middle() {
        let cmds = vec![DrawCommand::RectOutline {
            x: 10, y: 10, w: 100, h: 100, r: 0, g: 0, b: 0, a: 255, width: 1, radius: 0,
        }];
        assert_eq!(hit_test(&cmds, 10, 50), Some(0));
        assert_eq!(hit_test(&cmds, 60, 60), None);
    }

    // ── End to end: record, rewind, track, replay through an edit ──────────

    struct TestHost;
    impl Host for TestHost {
        fn register(&mut self, env: &mut Env) {
            crate::native_fns::register_all(env);
        }
        fn present(
            &mut self,
            _canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
            _env: &mut Env,
        ) -> Result<(), String> {
            Err("no window".to_string())
        }
        fn render_image(
            &mut self,
            _env: &mut Env,
            _stack: StackKey,
            _w: u32,
            _h: u32,
        ) -> Result<image::RgbImage, String> {
            Err("no window".to_string())
        }
    }

    const FRAME: &str = "config let STEP = 1\nstate var n = 0\nset n = n + STEP\ndraw_rect(n, 0, 10, 10, 255, 0, 0)\n";
    const EDITED: &str = "config let STEP = 10\nstate var n = 0\nset n = n + STEP\ndraw_rect(n, 0, 10, 10, 255, 0, 0)\n";

    /// One committed frame the way the loop runs it, recorded into `tl`.
    fn run_frame(env: &mut Env, live: StackKey, tl: &mut Timeline, host: &mut TestHost, frame: &mut i64) {
        let mut input = InputState::default();
        input.begin_frame(1.0 / 60.0);
        clear_draw_commands(env);
        host.prepare_frame(env);
        *frame += 1;
        env.advance_frame(live);
        bind_frame_info(env, 1.0 / 60.0, *frame);
        bind_time(env, *frame as f64 / 60.0);
        bind_input(env, &input);
        env.reset_stack(live).unwrap();
        env.run(live).unwrap();
        tl.record(env, live, *frame, 1.0 / 60.0, *frame as f64 / 60.0, &input).unwrap();
    }

    fn n_of(env: &Env, pid: ProgramId, live: StackKey) -> i64 {
        env.get_state_json(pid, live)["n"].as_i64().unwrap()
    }

    #[test]
    fn rewind_replay_and_trail_follow_the_recorded_history() {
        let mut host = TestHost;
        let mut env = Env::new();
        host.register(&mut env);
        env.enable_emit_trace(true);
        let pid = env.load_program(FRAME).unwrap();
        let live = env.create_stack(pid).unwrap();
        petal_ui::input::bind_dimensions(&mut env, 100, 100);
        let mut tl = Timeline::new(600);
        let mut frame = 0i64;

        for _ in 0..5 {
            run_frame(&mut env, live, &mut tl, &mut host, &mut frame);
        }
        assert_eq!(n_of(&env, pid, live), 5);
        assert_eq!(tl.len(), 5);
        let rec = tl.get(2).unwrap();
        assert!(
            rec.commands.iter().any(|c| matches!(c, DrawCommand::Rect { x: 3, .. })),
            "frame 3 recorded what it drew"
        );
        assert!(rec.origins.iter().any(|o| !o.chain.is_empty()), "with attribution");

        // Rewind two frames: the live execution is back at n = 3 and runs on.
        let at = tl.rewind(&mut env, live, 2).unwrap().unwrap();
        assert_eq!(at.0, 3);
        assert_eq!(n_of(&env, pid, live), 3);
        assert_eq!(tl.len(), 3);
        frame = at.0;
        run_frame(&mut env, live, &mut tl, &mut host, &mut frame);
        run_frame(&mut env, live, &mut tl, &mut host, &mut frame);
        assert_eq!(n_of(&env, pid, live), 5);
        assert_eq!(tl.len(), 5);

        // Freeze at the second frame and follow the rect drawn there.
        tl.freeze();
        tl.scrub_to(1);
        let site = tl.track_at(&env, pid, 5, 5).expect("the rect is under (5, 5)");
        assert_eq!(site.callee.as_deref(), Some("draw_rect"));
        let xs: Vec<i32> = tl.trail(&env, pid).iter().map(|p| p.x).collect();
        assert_eq!(xs, vec![6, 7, 8, 9, 10], "centre x of the rect on every frame");

        // Edit the knob and replay the future through the new program.
        let edited = env
            .compile_program_at(pid, EDITED, std::path::Path::new("edited.ptl"))
            .unwrap();
        env.transfer_state(live, edited).unwrap();
        tl.on_reload(&env, pid);
        let replayed = tl.replay(&mut env, live, 1, &mut host).unwrap();
        assert_eq!(replayed, 4);
        let xs: Vec<i32> = tl.trail(&env, pid).iter().map(|p| p.x).collect();
        assert_eq!(
            xs,
            vec![6, 16, 26, 36, 46],
            "the past is untouched, the future is what STEP = 10 produces"
        );
        assert_eq!(tl.get(0).unwrap().generation, 0);
        assert_eq!(tl.get(4).unwrap().generation, 1);

        // Resume from the cursor: the live game is that frame, later ones gone.
        let at = tl.unfreeze(&mut env, live).unwrap().unwrap();
        assert_eq!(at.0, 2);
        assert_eq!(n_of(&env, pid, live), 11);
        assert_eq!(tl.len(), 2);
    }

    #[test]
    fn centroid_of_a_circle_is_its_centre() {
        let c = DrawCommand::Circle { cx: 30, cy: 40, radius: 10, r: 0, g: 0, b: 0, a: 255 };
        assert_eq!(centroid(&c), Some((30, 40)));
    }
}
