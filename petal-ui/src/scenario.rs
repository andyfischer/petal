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
//! [`Scenario::monkey`] generates one from a seed: deterministic pseudo-random
//! clicks, canonical keys, and short text. A failing monkey run is replayable
//! from `(app, seed, monkey seed)` alone.

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
        if let Some(list) = obj.get("events") {
            let list = list
                .as_array()
                .ok_or_else(|| "`events` must be an array".to_string())?;
            for (i, entry) in list.iter().enumerate() {
                parse_event(entry, &mut events).map_err(|e| format!("events[{i}]: {e}"))?;
            }
        }
        events.sort_by_key(|e| e.at);
        Ok(Self {
            size,
            frames,
            events,
        })
    }

    /// Feed every event scheduled for `frame` to `ui`, in order. Call this
    /// before running that frame.
    pub fn apply(&self, ui: &mut Headless, frame: usize) {
        for ev in self.events.iter().filter(|e| e.at == frame) {
            ui.event(ev.event.clone());
        }
    }

    /// The last frame any event targets, or 0 for an empty scenario.
    pub fn last_frame(&self) -> usize {
        self.events.iter().map(|e| e.at).max().unwrap_or(0)
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
        }
    }

    /// Render back to the JSON shape [`from_json_str`](Self::from_json_str)
    /// reads — for writing a generated scenario into a repro bundle.
    pub fn to_json(&self) -> Json {
        let events: Vec<Json> = self
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

fn parse_event(entry: &Json, out: &mut Vec<TimedEvent>) -> Result<(), String> {
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
        return Err(format!(
            "`{name}` is not a canonical key name (see petal_ui::input::KEY_NAMES)"
        ));
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

    #[test]
    fn round_trips_through_json() {
        let a = Scenario::monkey(3, 40, (800, 600));
        let b = Scenario::from_json(&a.to_json()).unwrap();
        assert_eq!(a, b);
    }
}
