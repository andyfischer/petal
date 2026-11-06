//! Declarative input scenarios: a list of input events keyed by frame.
//!
//! A scenario is the *replayable* half of a headless run. It is plain JSON, so
//! a human can read one, hand-edit it, and check it in next to the app it
//! drives; nothing in it is a script, and applying one is a pure function of
//! (scenario, frame number).
//!
//! ```json
//! { "size": [1280, 850], "frames": 120,
//!   "events": [
//!     {"at": 5,  "mouse_move": [640, 400]},
//!     {"at": 6,  "mouse_down": 0}, {"at": 7, "mouse_up": 0},
//!     {"at": 9,  "click": [100, 200]},
//!     {"at": 20, "key": "left"},
//!     {"at": 25, "key_down": "a"}, {"at": 30, "key_up": "a"},
//!     {"at": 40, "text": "hello"},
//!     {"at": 50, "scroll": [0, -3]} ] }
//! ```
//!
//! Frames are numbered from 0, and `at: N` delivers the event to frame `N` —
//! the events are fed to the [`Headless`] *before* that frame runs, so the
//! frame sees their edges.
//!
//! Two compound spellings expand at parse time into the primitive events, so
//! [`apply`](Scenario::apply) stays stateless and a normalized scenario says
//! exactly what happened:
//!
//! - `click: [x, y]` → `mouse_move` + `mouse_down` at `N`, `mouse_up` at `N+1`
//!   (the release edge reaches the following frame, as in [`Headless::click`]).
//! - `key: "name"` → `key_down` + `key_up` both at `N` (one frame sees both
//!   edges, as in [`Headless::key`]).
//!
//! Two more are resolved when applied, because where they land depends on what
//! the frame before registered: `click_target: "id"` (a click on a named
//! target) and `drag: {target, to, frames}`, a held drag from a `ui/drag`
//! draggable (or any named target) to a point or another target — the
//! press at `N`, `frames` moves (default [`DEFAULT_DRAG_FRAMES`]) on
//! `N+1..=N+frames`, the release on `N+frames+1`. See [`TargetDrag`].
//!
//! [`Scenario::monkey`] generates one from a seed: deterministic pseudo-random
//! clicks, canonical keys, and short text. A failing monkey run is replayable
//! from `(app, seed, monkey seed)` alone.

use std::cell::RefCell;
use std::collections::BTreeMap;

use serde_json::Value as Json;

use crate::harness::Headless;
use crate::input::{self, InputEvent};

/// A parsed scenario: optional window size and frame count (the CLI's flags
/// override them), plus the normalized event list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scenario {
    pub size: Option<(i32, i32)>,
    pub frames: Option<usize>,
    pub events: Vec<TimedEvent>,
    /// Clicks on named targets (`{"at": N, "click_target": "Save"}`). They
    /// cannot be turned into events when the file is read — where the target
    /// is depends on the frame before — so they are kept apart and resolved
    /// when applied ([`events_at`](Self::events_at)).
    pub target_clicks: Vec<TargetClick>,
    /// Drags between named things (`{"at": N, "drag": {...}}`), resolved
    /// when their press frame is about to run, like `target_clicks`.
    pub target_drags: Vec<TargetDrag>,
    /// Each drag's endpoints, fixed on its press frame so the rest of the
    /// gesture does not chase a target the drag itself moves (the preview
    /// reflows the layout). Indexed like `target_drags`; `None` when an
    /// endpoint did not resolve.
    resolved_drags: RefCell<BTreeMap<usize, Option<((i32, i32), (i32, i32))>>>,
}

/// Frames of pointer movement a scenario `drag` takes when it does not say.
pub const DEFAULT_DRAG_FRAMES: usize = 10;

/// A held drag (`{"at": N, "drag": {"target": "A", "to": [x, y] | "id",
/// "frames": k}}`): press on the middle of `target` on frame `at`, move in
/// `frames` even steps to `to` on frames `at+1..=at+frames`, release on
/// `at+frames+1`.
///
/// A name is looked up as a registered target id (`interact`, `ui_target`, a
/// button's label), then as `"drag:<name>"` — the id `ui/drag`'s
/// `draggable(key, …)` registers, so a draggable is named by its place key —
/// and, by [`Scenario::apply`], as a key `ui/drag` has placed (the rect it
/// was last drawn at). Both endpoints are looked up once, on frame `at`.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetDrag {
    pub at: usize,
    pub target: String,
    pub to: DragTo,
    pub frames: usize,
}

/// Where a scenario `drag` ends.
#[derive(Clone, Debug, PartialEq)]
pub enum DragTo {
    Point(i32, i32),
    Target(String),
}

impl TargetDrag {
    /// The frame the release lands on.
    pub fn release_frame(&self) -> usize {
        self.at + self.frames + 1
    }
}

/// A click on the rect a script registered under `id` (`interact(r, id)`, a
/// button's label, `ui_target(id, r)` — see [`crate::hit`]): a move to its
/// centre and a press on frame `at`, the release on `at + 1`.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetClick {
    pub at: usize,
    pub id: String,
}

/// One input event and the frame it is delivered to.
#[derive(Clone, Debug, PartialEq)]
pub struct TimedEvent {
    pub at: usize,
    pub event: InputEvent,
}

impl Scenario {
    /// Parse a scenario from JSON text.
    pub fn from_json_str(text: &str) -> Result<Self, String> {
        let json: Json = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
        Self::from_json(&json)
    }

    /// Parse a scenario from an already-decoded JSON value.
    pub fn from_json(json: &Json) -> Result<Self, String> {
        let obj = json
            .as_object()
            .ok_or_else(|| "scenario must be a JSON object".to_string())?;

        let size = match obj.get("size") {
            None | Some(Json::Null) => None,
            Some(v) => Some(parse_pair(v, "size").map(|(w, h)| (w as i32, h as i32))?),
        };
        let frames = match obj.get("frames") {
            None | Some(Json::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .ok_or_else(|| "`frames` must be a non-negative integer".to_string())?
                    as usize,
            ),
        };

        let mut events = Vec::new();
        let mut target_clicks = Vec::new();
        let mut target_drags = Vec::new();
        if let Some(list) = obj.get("events") {
            let list = list
                .as_array()
                .ok_or_else(|| "`events` must be an array".to_string())?;
            for (i, entry) in list.iter().enumerate() {
                parse_event(entry, &mut events, &mut target_clicks, &mut target_drags)
                    .map_err(|e| format!("events[{i}]: {e}"))?;
            }
        }
        events.sort_by_key(|e| e.at);
        target_clicks.sort_by_key(|c| c.at);
        target_drags.sort_by_key(|d| d.at);
        Ok(Self {
            size,
            frames,
            events,
            target_clicks,
            target_drags,
            resolved_drags: RefCell::default(),
        })
    }

    /// Feed every event scheduled for `frame` to `ui`, in order, and click
    /// the named targets due on it. Call this before running that frame.
    /// Returns the ids of targets that were to be clicked on this frame and
    /// are not registered — a scenario that names a widget the app does not
    /// have should fail loudly, and the caller decides how.
    pub fn apply(&self, ui: &mut Headless, frame: usize) -> Vec<String> {
        let left_down = ui.input.button_down(input::buttons::LEFT);
        // A drag endpoint may also be a key `ui/drag` placed: read where it
        // was last drawn out of the state, only when a drag is due.
        let placed = if self.target_drags.iter().any(|d| d.at == frame) {
            crate::drag_trace::placed_keys(&ui.state())
        } else {
            Default::default()
        };
        let (events, missing) = self.events_at(frame, left_down, |id| {
            crate::hit::target(&mut ui.env, id)
                .map(|t| t.center())
                .or_else(|| placed.get(id).copied())
        });
        for ev in events {
            ui.event(ev);
        }
        missing
    }

    /// The input events for `frame`: the scheduled ones, then, for each
    /// target click due, a move to the target's centre and a left press — and
    /// the release of a target click pressed on the frame before. `resolve`
    /// looks a target up by id (a host passes [`crate::hit::target`]);
    /// `left_down` is whether the left button is held, so a click whose
    /// target was never found does not release a press that never happened.
    /// The second value lists the ids due on this frame that did not resolve.
    pub fn events_at(
        &self,
        frame: usize,
        left_down: bool,
        mut resolve: impl FnMut(&str) -> Option<(i32, i32)>,
    ) -> (Vec<InputEvent>, Vec<String>) {
        let mut events: Vec<InputEvent> = self
            .events
            .iter()
            .filter(|e| e.at == frame)
            .map(|e| e.event.clone())
            .collect();
        let mut missing = Vec::new();
        let button = input::buttons::LEFT;
        if left_down && self.target_clicks.iter().any(|c| c.at + 1 == frame) {
            events.push(InputEvent::MouseUp { button });
        }
        for (i, drag) in self.target_drags.iter().enumerate() {
            if frame < drag.at || frame > drag.release_frame() {
                continue;
            }
            if frame == drag.at {
                // A draggable is registered as `drag:<key>`; try that second.
                let mut find = |name: &str| {
                    resolve(name).or_else(|| resolve(&format!("drag:{name}")))
                };
                let from = find(&drag.target);
                if from.is_none() {
                    missing.push(drag.target.clone());
                }
                let to = match &drag.to {
                    DragTo::Point(x, y) => Some((*x, *y)),
                    DragTo::Target(id) => {
                        let to = find(id);
                        if to.is_none() {
                            missing.push(id.clone());
                        }
                        to
                    }
                };
                let ends = from.zip(to);
                self.resolved_drags.borrow_mut().insert(i, ends);
                if let Some(((x, y), _)) = ends {
                    events.push(InputEvent::MouseMove { x, y });
                    events.push(InputEvent::MouseDown { button });
                }
                continue;
            }
            let Some(Some(((x0, y0), (x1, y1)))) = self.resolved_drags.borrow().get(&i).copied()
            else {
                continue;
            };
            if frame == drag.release_frame() {
                events.push(InputEvent::MouseUp { button });
            } else {
                let t = (frame - drag.at) as f64 / drag.frames.max(1) as f64;
                let lerp = |a: i32, b: i32| (a as f64 + (b - a) as f64 * t).round() as i32;
                events.push(InputEvent::MouseMove {
                    x: lerp(x0, x1),
                    y: lerp(y0, y1),
                });
            }
        }
        for click in self.target_clicks.iter().filter(|c| c.at == frame) {
            match resolve(&click.id) {
                Some((x, y)) => {
                    events.push(InputEvent::MouseMove { x, y });
                    events.push(InputEvent::MouseDown { button });
                }
                None => missing.push(click.id.clone()),
            }
        }
        (events, missing)
    }

    /// A deterministic pseudo-random scenario: clicks inside a `size` window,
    /// keys drawn from [`input::KEY_NAMES`], and short typed text, spread over
    /// `frames` frames. Same seed → same events, on any platform (a plain
    /// xorshift, no floating point, no external crate).
    pub fn monkey(seed: u64, frames: usize, size: (i32, i32)) -> Self {
        const TEXT: &[&str] = &["a", "hi", "xyz", "42", "petal", " ", "ok"];
        let mut rng = XorShift::new(seed);
        let (w, h) = (size.0.max(1) as u64, size.1.max(1) as u64);
        let mut events = Vec::new();
        // One action every few frames, leaving quiet frames in between so
        // edge-triggered logic (and any release edge) can settle.
        let mut at = 1;
        while at < frames {
            match rng.below(6) {
                0 | 1 => {
                    let x = rng.below(w) as i32;
                    let y = rng.below(h) as i32;
                    push_click(&mut events, at, x, y);
                }
                2 => {
                    let x = rng.below(w) as i32;
                    let y = rng.below(h) as i32;
                    events.push(TimedEvent {
                        at,
                        event: InputEvent::MouseMove { x, y },
                    });
                }
                3 | 4 => {
                    let key = input::KEY_NAMES[rng.below(input::KEY_NAMES.len() as u64) as usize];
                    push_key(&mut events, at, key);
                }
                _ => {
                    let text = TEXT[rng.below(TEXT.len() as u64) as usize];
                    events.push(TimedEvent {
                        at,
                        event: InputEvent::Text {
                            text: text.to_string(),
                        },
                    });
                }
            }
            at += 1 + rng.below(3) as usize;
        }
        events.retain(|e| e.at < frames);
        events.sort_by_key(|e| e.at);
        Self {
            size: Some(size),
            frames: Some(frames),
            events,
            ..Self::default()
        }
    }

    /// Render back to the JSON shape [`from_json_str`](Self::from_json_str)
    /// reads — for writing a generated scenario into a repro bundle.
    pub fn to_json(&self) -> Json {
        let mut events: Vec<Json> = self
            .events
            .iter()
            .map(|e| {
                let mut o = serde_json::Map::new();
                o.insert("at".into(), Json::from(e.at));
                let (key, val) = match &e.event {
                    InputEvent::MouseMove { x, y } => ("mouse_move", Json::from(vec![*x, *y])),
                    InputEvent::MouseRelative { dx, dy } => {
                        ("mouse_relative", Json::from(vec![*dx, *dy]))
                    }
                    InputEvent::MouseDown { button } => ("mouse_down", Json::from(*button)),
                    InputEvent::MouseUp { button } => ("mouse_up", Json::from(*button)),
                    InputEvent::Scroll { dx, dy } => ("scroll", Json::from(vec![*dx, *dy])),
                    InputEvent::KeyDown { key } => ("key_down", Json::from(key.clone())),
                    InputEvent::KeyUp { key } => ("key_up", Json::from(key.clone())),
                    InputEvent::Text { text } => ("text", Json::from(text.clone())),
                    InputEvent::Modifiers(m) => (
                        "modifiers",
                        serde_json::json!({
                            "shift": m.shift, "ctrl": m.ctrl, "alt": m.alt, "cmd": m.cmd
                        }),
                    ),
                };
                o.insert(key.into(), val);
                Json::Object(o)
            })
            .collect();
        events.extend(
            self.target_clicks
                .iter()
                .map(|c| serde_json::json!({"at": c.at, "click_target": c.id})),
        );
        events.extend(self.target_drags.iter().map(|d| {
            let to = match &d.to {
                DragTo::Point(x, y) => serde_json::json!([x, y]),
                DragTo::Target(id) => Json::from(id.clone()),
            };
            serde_json::json!({"at": d.at, "drag": {"target": d.target, "to": to, "frames": d.frames}})
        }));
        let mut root = serde_json::Map::new();
        if let Some((w, h)) = self.size {
            root.insert("size".into(), Json::from(vec![w, h]));
        }
        if let Some(f) = self.frames {
            root.insert("frames".into(), Json::from(f));
        }
        root.insert("events".into(), Json::Array(events));
        Json::Object(root)
    }
}

fn push_click(out: &mut Vec<TimedEvent>, at: usize, x: i32, y: i32) {
    out.push(TimedEvent {
        at,
        event: InputEvent::MouseMove { x, y },
    });
    out.push(TimedEvent {
        at,
        event: InputEvent::MouseDown {
            button: input::buttons::LEFT,
        },
    });
    out.push(TimedEvent {
        at: at + 1,
        event: InputEvent::MouseUp {
            button: input::buttons::LEFT,
        },
    });
}

fn push_key(out: &mut Vec<TimedEvent>, at: usize, key: &str) {
    out.push(TimedEvent {
        at,
        event: InputEvent::KeyDown {
            key: key.to_string(),
        },
    });
    out.push(TimedEvent {
        at,
        event: InputEvent::KeyUp {
            key: key.to_string(),
        },
    });
}

fn parse_event(
    entry: &Json,
    out: &mut Vec<TimedEvent>,
    target_clicks: &mut Vec<TargetClick>,
    target_drags: &mut Vec<TargetDrag>,
) -> Result<(), String> {
    let obj = entry
        .as_object()
        .ok_or_else(|| "event must be an object".to_string())?;
    let at = obj
        .get("at")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "event needs an `at` frame number".to_string())? as usize;

    let mut matched = 0;
    for (key, v) in obj {
        if key == "at" {
            continue;
        }
        matched += 1;
        match key.as_str() {
            "mouse_move" => {
                let (x, y) = parse_pair(v, "mouse_move")?;
                out.push(TimedEvent {
                    at,
                    event: InputEvent::MouseMove {
                        x: x as i32,
                        y: y as i32,
                    },
                });
            }
            "mouse_relative" => {
                let (dx, dy) = parse_pair(v, "mouse_relative")?;
                out.push(TimedEvent {
                    at,
                    event: InputEvent::MouseRelative {
                        dx: dx as i32,
                        dy: dy as i32,
                    },
                });
            }
            "mouse_down" => out.push(TimedEvent {
                at,
                event: InputEvent::MouseDown {
                    button: parse_button(v)?,
                },
            }),
            "mouse_up" => out.push(TimedEvent {
                at,
                event: InputEvent::MouseUp {
                    button: parse_button(v)?,
                },
            }),
            "click" => {
                let (x, y) = parse_pair(v, "click")?;
                push_click(out, at, x as i32, y as i32);
            }
            "click_target" => target_clicks.push(TargetClick {
                at,
                id: v
                    .as_str()
                    .ok_or_else(|| "`click_target` must be a target id string".to_string())?
                    .to_string(),
            }),
            "drag" => target_drags.push(parse_drag(at, v)?),
            "scroll" => {
                let (dx, dy) = parse_pair(v, "scroll")?;
                out.push(TimedEvent {
                    at,
                    event: InputEvent::Scroll { dx, dy },
                });
            }
            "key" => push_key(out, at, parse_key(v)?.as_str()),
            "key_down" => out.push(TimedEvent {
                at,
                event: InputEvent::KeyDown { key: parse_key(v)? },
            }),
            "key_up" => out.push(TimedEvent {
                at,
                event: InputEvent::KeyUp { key: parse_key(v)? },
            }),
            "text" => out.push(TimedEvent {
                at,
                event: InputEvent::Text {
                    text: v
                        .as_str()
                        .ok_or_else(|| "`text` must be a string".to_string())?
                        .to_string(),
                },
            }),
            "modifiers" => {
                let o = v
                    .as_object()
                    .ok_or_else(|| "`modifiers` must be an object".to_string())?;
                let flag = |n: &str| o.get(n).and_then(|b| b.as_bool()).unwrap_or(false);
                out.push(TimedEvent {
                    at,
                    event: InputEvent::Modifiers(input::Modifiers {
                        shift: flag("shift"),
                        ctrl: flag("ctrl"),
                        alt: flag("alt"),
                        cmd: flag("cmd"),
                    }),
                });
            }
            other => return Err(format!("unknown event field `{other}`")),
        }
    }
    if matched == 0 {
        return Err("event has no action field".to_string());
    }
    Ok(())
}

fn parse_drag(at: usize, v: &Json) -> Result<TargetDrag, String> {
    let o = v
        .as_object()
        .ok_or_else(|| "`drag` must be an object {target, to, frames?}".to_string())?;
    for k in o.keys() {
        if !matches!(k.as_str(), "target" | "to" | "frames") {
            return Err(format!("unknown `drag` field `{k}` (expected target, to, frames)"));
        }
    }
    let target = match o.get("target") {
        Some(Json::String(s)) => s.clone(),
        // A numeric place key, written as a number.
        Some(Json::Number(n)) => n.to_string(),
        _ => return Err("`drag.target` must be a target id or place key".to_string()),
    };
    let to = match o.get("to") {
        Some(Json::String(s)) => DragTo::Target(s.clone()),
        Some(Json::Number(n)) => DragTo::Target(n.to_string()),
        Some(v @ Json::Array(_)) => {
            let (x, y) = parse_pair(v, "drag.to")?;
            DragTo::Point(x as i32, y as i32)
        }
        _ => return Err("`drag.to` must be [x, y] or a target id".to_string()),
    };
    let frames = match o.get("frames") {
        None | Some(Json::Null) => DEFAULT_DRAG_FRAMES,
        Some(f) => match f.as_u64() {
            Some(n) if n >= 1 => n as usize,
            _ => return Err("`drag.frames` must be a positive integer".to_string()),
        },
    };
    Ok(TargetDrag {
        at,
        target,
        to,
        frames,
    })
}

fn parse_pair(v: &Json, what: &str) -> Result<(f64, f64), String> {
    let arr = v
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or_else(|| format!("`{what}` must be a two-element array"))?;
    let n = |i: usize| {
        arr[i]
            .as_f64()
            .ok_or_else(|| format!("`{what}` must hold numbers"))
    };
    Ok((n(0)?, n(1)?))
}

fn parse_button(v: &Json) -> Result<u8, String> {
    match v {
        Json::Number(_) => Ok(v.as_u64().unwrap_or(0) as u8),
        Json::String(s) => match s.as_str() {
            "left" => Ok(input::buttons::LEFT),
            "right" => Ok(input::buttons::RIGHT),
            "middle" => Ok(input::buttons::MIDDLE),
            other => Err(format!("unknown mouse button `{other}`")),
        },
        _ => Err("mouse button must be a number or a name".to_string()),
    }
}

/// Key names must be canonical ([`input::KEY_NAMES`]) — a scenario that says
/// `"ArrowLeft"` would silently drive nothing, so it is rejected instead.
fn parse_key(v: &Json) -> Result<String, String> {
    let name = v
        .as_str()
        .ok_or_else(|| "key name must be a string".to_string())?;
    if !input::is_canonical_key(name) {
        return Err(input::non_canonical_key_error(name));
    }
    Ok(name.to_string())
}

/// A 64-bit xorshift* — enough randomness for a monkey, and identical on every
/// platform, which a floating-point generator would not be.
struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        // 0 is a fixed point of xorshift; any nonzero substitute will do.
        Self(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_event_spelling() {
        let s = Scenario::from_json_str(
            r#"{ "size": [1280, 850], "frames": 120,
                 "events": [
                   {"at": 5,  "mouse_move": [640, 400]},
                   {"at": 6,  "mouse_down": 0},
                   {"at": 7,  "mouse_up": 0},
                   {"at": 9,  "click": [100, 200]},
                   {"at": 20, "key": "left"},
                   {"at": 25, "key_down": "a"},
                   {"at": 30, "key_up": "a"},
                   {"at": 40, "text": "hello"},
                   {"at": 50, "scroll": [0, -3]} ] }"#,
        )
        .unwrap();
        assert_eq!(s.size, Some((1280, 850)));
        assert_eq!(s.frames, Some(120));
        // click expands to 3 events, key to 2 → 9 spellings + 3 extras.
        assert_eq!(s.events.len(), 12);
        // The click's release lands on the following frame.
        assert!(s.events.iter().any(|e| e.at == 10
            && e.event
                == InputEvent::MouseUp {
                    button: input::buttons::LEFT
                }));
        // Both key edges land on the same frame.
        assert_eq!(s.events.iter().filter(|e| e.at == 20).count(), 2);
    }

    #[test]
    fn rejects_non_canonical_keys() {
        let err =
            Scenario::from_json_str(r#"{"events": [{"at": 1, "key": "ArrowLeft"}]}"#).unwrap_err();
        assert!(err.contains("canonical"), "{err}");
    }

    #[test]
    fn monkey_is_deterministic_and_in_bounds() {
        let a = Scenario::monkey(7, 60, (400, 300));
        let b = Scenario::monkey(7, 60, (400, 300));
        assert_eq!(a, b);
        assert_ne!(a, Scenario::monkey(8, 60, (400, 300)));
        assert!(!a.events.is_empty());
        for e in &a.events {
            assert!(e.at < 61, "events stay near the frame window");
            if let InputEvent::MouseMove { x, y } = e.event {
                assert!((0..400).contains(&x) && (0..300).contains(&y));
            }
        }
    }

    fn drag_scenario() -> Scenario {
        Scenario::from_json_str(
            r#"{"events": [{"at": 2, "drag": {"target": "A", "to": "slot", "frames": 4}}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn drag_presses_moves_and_releases() {
        let s = drag_scenario();
        assert_eq!(s.target_drags[0].release_frame(), 7);
        let resolve = |id: &str| match id {
            "drag:A" => Some((10, 10)),
            "slot" => Some((50, 90)),
            _ => None,
        };
        let (e2, m2) = s.events_at(2, false, resolve);
        assert!(m2.is_empty());
        assert_eq!(
            e2,
            vec![
                InputEvent::MouseMove { x: 10, y: 10 },
                InputEvent::MouseDown { button: 0 }
            ]
        );
        // The endpoints were fixed on the press: a target that moves later
        // (the drag's own preview reflowing) does not bend the path.
        let moved = |id: &str| if id == "slot" { Some((0, 0)) } else { None };
        let moves: Vec<_> = (3..=6).map(|f| s.events_at(f, true, moved).0).collect();
        assert_eq!(moves[0], vec![InputEvent::MouseMove { x: 20, y: 30 }]);
        assert_eq!(moves[3], vec![InputEvent::MouseMove { x: 50, y: 90 }]);
        assert_eq!(s.events_at(7, true, moved).0, vec![InputEvent::MouseUp { button: 0 }]);
        assert!(s.events_at(8, false, moved).0.is_empty());
    }

    #[test]
    fn drag_reports_unresolved_names() {
        let s = drag_scenario();
        let (events, missing) = s.events_at(2, false, |_| None);
        assert!(events.is_empty());
        assert_eq!(missing, vec!["A".to_string(), "slot".to_string()]);
        // Nothing was pressed, so nothing moves or is released after.
        assert!(s.events_at(3, false, |_| None).0.is_empty());
        assert!(s.events_at(7, false, |_| None).0.is_empty());
    }

    #[test]
    fn drag_parses_points_and_defaults_and_round_trips() {
        let s = Scenario::from_json_str(
            r#"{"events": [{"at": 1, "drag": {"target": 7, "to": [3, 4]}}]}"#,
        )
        .unwrap();
        assert_eq!(
            s.target_drags[0],
            TargetDrag {
                at: 1,
                target: "7".into(),
                to: DragTo::Point(3, 4),
                frames: DEFAULT_DRAG_FRAMES
            }
        );
        assert_eq!(Scenario::from_json(&s.to_json()).unwrap(), s);
        let bad = Scenario::from_json_str(r#"{"events": [{"at": 1, "drag": {"target": "a", "to": "b", "speed": 2}}]}"#);
        assert!(bad.unwrap_err().contains("speed"));
    }

    #[test]
    fn round_trips_through_json() {
        let a = Scenario::monkey(3, 40, (800, 600));
        let b = Scenario::from_json(&a.to_json()).unwrap();
        assert_eq!(a, b);
    }
}
