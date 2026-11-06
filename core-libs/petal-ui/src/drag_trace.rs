//! Reading a `ui/drag` drag from the outside: what `petal-ui-run
//! --drag-trace`, the scenario `drag` sugar and the `DragExplain` MCP tool
//! see of the engine.
//!
//! The engine (`prelude/drag.ptl`) keeps everything in ordinary `state`
//! holding plain values — the session's resolved spec with its target table,
//! the grab offset, the chosen leaf — so a host reads it out of the state
//! dump ([`crate::frame_core::FrameCore::state`]) with no hook in the script.
//! What is computed here (each target's distance from the dragged element,
//! the branch down to the chosen leaf) is the same arithmetic the engine's
//! own `drag_explain()` does, from the same data.
//!
//! A drop is announced on the [`DRAG_DROPS_CHANNEL`] output channel as
//! `{key, model, drop}`, which [`take_drops`] drains.

use std::collections::HashMap;

use petal::env::Env;
use serde_json::{json, Map, Value as Json};

/// The output channel `ui/drag` announces a drop on (`DRAG_DROPS_CHANNEL` in
/// the module).
pub const DRAG_DROPS_CHANNEL: &str = "drag_drops";

/// The state slot of a `drag_session`: its call path ends in this.
const SESSION_SUFFIX: &str = "ui/drag::s";
/// Every placed key's displayed value.
const ANIM_SLOT: &str = "ui/drag::_anim";

/// The sessions with a drag in flight, as their raw state records (phase
/// `"drag"`), in state-key order.
pub fn sessions(state: &Map<String, Json>) -> Vec<&Json> {
    state
        .iter()
        .filter(|(k, v)| k.ends_with(SESSION_SUFFIX) && v["phase"] == "drag")
        .map(|(_, v)| v)
        .collect()
}

/// Explain one in-flight session with the pointer at `pointer`: the target
/// table with each target's distance, the chosen leaf and the branch down to
/// it. `full` adds the spec as built (`raw`) and each target's drop state,
/// which can be large; a per-frame trace leaves them out.
pub fn explain(session: &Json, pointer: (i32, i32), full: bool) -> Json {
    let home = &session["home"];
    let grab = &session["grab"];
    let num = |v: &Json| v.as_f64().unwrap_or(0.0);
    let (px, py) = (pointer.0 as f64, pointer.1 as f64);
    // The element's centre if it were at the pointer, as the engine measures.
    let ex = px - num(&grab["x"]) + num(&home["w"]) / 2.0;
    let ey = py - num(&grab["y"]) + num(&home["h"]) / 2.0;
    let choice = session["choice"].as_i64();
    let mut leaves = Vec::new();
    collect_leaves(&session["spec"], &mut leaves);
    let targets: Vec<Json> = leaves
        .iter()
        .map(|l| {
            let at = &l["at"];
            let gap = center(at).map(|(cx, cy)| (cx - ex).hypot(cy - ey));
            let mut t = json!({
                "id": l["id"],
                "kind": l["d2"],
                "at": at,
                "zone": l.get("zone").cloned().unwrap_or(Json::Null),
                "gap": gap.map(round2),
                "active": choice.is_some() && l["id"].as_i64() == choice,
            });
            if full {
                t["drop"] = l.get("drop").cloned().unwrap_or(Json::Null);
            }
            t
        })
        .collect();
    let gap = targets
        .iter()
        .find(|t| t["active"] == true)
        .map(|t| t["gap"].clone())
        .unwrap_or(Json::Null);
    let unresolved = unresolved_kinds(&session["spec"]);
    let mut out = json!({
        "key": session["key"],
        "pointer": [pointer.0, pointer.1],
        "element": {"x": round2(ex), "y": round2(ey)},
        "home": home,
        "grab": grab,
        "choice": session["choice"],
        "gap": gap,
        "branch": choice.and_then(|c| branch(&session["spec"], c)),
        "targets": targets,
    });
    if !unresolved.is_empty() {
        // `between` / `vary` targets are computed per frame, not tabled.
        out["per_frame"] = json!(unresolved);
    }
    if full {
        out["spec"] = session["raw"].clone();
        out["base"] = session["base"].clone();
    }
    out
}

/// Every in-flight drag in `state`, explained (see [`explain`]).
pub fn explain_all(state: &Map<String, Json>, pointer: (i32, i32), full: bool) -> Vec<Json> {
    sessions(state)
        .into_iter()
        .map(|s| explain(s, pointer, full))
        .collect()
}

/// One line per in-flight drag, for a human reading stderr:
/// `drag "A": #3 of 5 (floating > closest > fixed #3), gap 90`.
pub fn summary(explained: &Json) -> String {
    let n = explained["targets"].as_array().map_or(0, |t| t.len());
    let key = &explained["key"];
    match explained["choice"].as_i64() {
        Some(c) => {
            let branch = explained["branch"]
                .as_array()
                .map(|b| {
                    b.iter()
                        .filter_map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(" > ")
                })
                .unwrap_or_default();
            format!(
                "drag {key}: #{c} of {n} ({branch}), gap {}",
                explained["gap"]
            )
        }
        None => format!("drag {key}: no target of {n}"),
    }
}

/// Drain the drops announced since the last call: `{key, model, drop}` each.
pub fn take_drops(env: &mut Env) -> Vec<Json> {
    let sym = env.intern_symbol(DRAG_DROPS_CHANNEL);
    let values = env.take_output_buffer(sym);
    let heap = env.heap();
    values
        .iter()
        .map(|v| petal::value::value_to_json(v, heap))
        .collect()
}

/// Where every key `ui/drag` has placed was last drawn: the centre of its
/// displayed rect (or its point), by the key's text (a string key as it is,
/// any other key as its JSON). What a scenario `drag` names an element by.
pub fn placed_keys(state: &Map<String, Json>) -> HashMap<String, (i32, i32)> {
    let mut out = HashMap::new();
    let Some(entries) = state.get(ANIM_SLOT).and_then(|m| m["entries"].as_array()) else {
        return out;
    };
    for e in entries {
        let (Some(k), Some(v)) = (e.get(0), e.get(1)) else {
            continue;
        };
        let name = match k {
            Json::String(s) => s.clone(),
            other => other.to_string(),
        };
        if let Some((x, y)) = center(v) {
            out.insert(name, (x.round() as i32, y.round() as i32));
        }
    }
    out
}

/// The point a placed value stands for (a rect's centre, a point), as the
/// engine's `_center` reads it.
fn center(v: &Json) -> Option<(f64, f64)> {
    let x = v.get("x")?.as_f64()?;
    let y = v.get("y")?.as_f64()?;
    let w = v.get("w").and_then(|w| w.as_f64()).unwrap_or(0.0);
    let h = v.get("h").and_then(|h| h.as_f64()).unwrap_or(0.0);
    Some((x + w / 2.0, y + h / 2.0))
}

fn is_leaf(kind: &str) -> bool {
    matches!(kind, "fixed" | "drop_target" | "switch_to")
}

/// A spec's sub-specs, in spec order (the engine's `_kids`).
fn kids(spec: &Json) -> Vec<&Json> {
    match spec["d2"].as_str().unwrap_or("") {
        "closest" | "between" => spec["options"]
            .as_array()
            .map(|o| o.iter().collect())
            .unwrap_or_default(),
        "when_far" => vec![&spec["inner"], &spec["far"]],
        "floating" | "snap_radius" | "on_drop" => vec![&spec["inner"]],
        _ => Vec::new(),
    }
}

fn collect_leaves<'a>(spec: &'a Json, out: &mut Vec<&'a Json>) {
    if is_leaf(spec["d2"].as_str().unwrap_or("")) {
        out.push(spec);
        return;
    }
    for k in kids(spec) {
        collect_leaves(k, out);
    }
}

/// Kinds in the spec whose targets are not in the table (`between`, `vary`).
fn unresolved_kinds(spec: &Json) -> Vec<String> {
    let mut out = Vec::new();
    let kind = spec["d2"].as_str().unwrap_or("");
    if matches!(kind, "between" | "vary") {
        out.push(kind.to_string());
    }
    for k in kids(spec) {
        for u in unresolved_kinds(k) {
            if !out.contains(&u) {
                out.push(u);
            }
        }
    }
    out
}

/// The kinds from `spec` down to leaf `id` (the engine's `_branch`).
fn branch(spec: &Json, id: i64) -> Option<Vec<String>> {
    let kind = spec["d2"].as_str().unwrap_or("");
    if is_leaf(kind) {
        return (spec["id"].as_i64() == Some(id)).then(|| vec![format!("{kind} #{id}")]);
    }
    for k in kids(spec) {
        if let Some(mut b) = branch(k, id) {
            b.insert(0, kind.to_string());
            return Some(b);
        }
    }
    None
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Json {
        json!({
            "phase": "drag", "key": "A",
            "home": {"x": 0, "y": 0, "w": 20, "h": 20}, "grab": {"x": 10, "y": 10},
            "choice": 2,
            "raw": {"d2": "floating"}, "base": [1],
            "spec": {"d2": "floating", "inner": {"d2": "closest", "stickiness": 0, "options": [
                {"d2": "fixed", "id": 1, "at": {"x": 0, "y": 0, "w": 20, "h": 20}, "drop": [1]},
                {"d2": "fixed", "id": 2, "at": {"x": 0, "y": 30, "w": 20, "h": 20}, "drop": [2]},
            ]}}
        })
    }

    #[test]
    fn explains_targets_branch_and_gap() {
        let e = explain(&session(), (10, 30), false);
        assert_eq!(e["element"], json!({"x": 10.0, "y": 30.0}));
        assert_eq!(e["targets"][0]["gap"], json!(20.0));
        assert_eq!(e["targets"][1]["gap"], json!(10.0));
        assert_eq!(e["targets"][1]["active"], json!(true));
        assert_eq!(e["gap"], json!(10.0));
        assert_eq!(e["branch"], json!(["floating", "closest", "fixed #2"]));
        assert!(e.get("spec").is_none() && e["targets"][0].get("drop").is_none());
        let full = explain(&session(), (10, 30), true);
        assert_eq!(full["targets"][1]["drop"], json!([2]));
        assert_eq!(full["spec"], json!({"d2": "floating"}));
        assert_eq!(summary(&e), "drag \"A\": #2 of 2 (floating > closest > fixed #2), gap 10.0");
    }

    #[test]
    fn finds_sessions_and_placed_keys() {
        let mut state = Map::new();
        state.insert("ui/drag::drag_session/ui/drag::s".into(), session());
        state.insert("other/ui/drag::s".into(), json!({"phase": "idle"}));
        state.insert(
            ANIM_SLOT.into(),
            json!({"type": "map", "entries": [["A", {"x": 0, "y": 0, "w": 20, "h": 10}], [7, {"x": 4, "y": 6}]]}),
        );
        assert_eq!(sessions(&state).len(), 1);
        let placed = placed_keys(&state);
        assert_eq!(placed["A"], (10, 5));
        assert_eq!(placed["7"], (4, 6));
    }
}
