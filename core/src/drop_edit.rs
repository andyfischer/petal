//! Structural direct manipulation: a drop on a literal model, written back
//! to the source as goal edits.
//!
//! `ui/drag` (core-libs/petal-ui/prelude/drag.ptl) works on a *model value*:
//! a drag ends in a new model, `drop`. When that model is a top-level literal
//! binding —
//!
//! ```petal ignore
//! // The pipeline, in order.
//! let STEPS = ["fetch", "parse", "check", "emit"]
//! let d = drag_session(STEPS, view)
//! ```
//!
//! — the drop can be made true in the *source* instead of in `state`: the
//! difference between the old and the new model is a list of
//! [`Goal`](crate::goal_based_editing::Goal)s against that binding
//! (`should_remove("STEPS[0]")` + `should_insert("STEPS[2]", "fetch")`,
//! `should_set_path("BOARD[1].title", "Done")`), and goal-based editing
//! applies them without disturbing the rest of the file. This is the edit DM
//! v2 cannot express, since its levers are numbers: *move this element of the
//! list*.
//!
//! - [`diff_goals`] is the diff: lists are matched element by element on an
//!   identity (a record's `id`, else its `key`, else the element itself) with
//!   a longest common subsequence, so a move is one removal and one insertion
//!   and the untouched elements — and their comments — stay where they are;
//!   records are diffed field by field; anything else is set.
//! - [`literal_bindings`] finds the top-level `let` whose literal value is
//!   the model (by value: the running model *is* that literal).
//! - [`propose_drop`] puts the two together and applies the goals.
//!
//! Only plain `let` literals are targeted. `state` initializers are not (a
//! state slot keeps its value across a reload, so editing the initializer
//! would change nothing on screen), and `config let` is not required (it is
//! planned for removal; see docs/proposals/remove-config-keyword.md).
//!
//! Values arrive from the running program as JSON
//! ([`crate::value::value_to_json`]); [`static_from_json`] reads them. Where
//! the new value is the same as something already in the source — an element
//! that moved, a colour the runtime holds as `{r, g, b}` — the source's own
//! [`StaticValue`] is what is written, so a `#ff8800` stays a colour literal
//! and an enum variant stays a variant.

use crate::goal_based_editing::{modify_source_with_goals, Goal, GoalError, StaticValue};
use crate::static_value::static_bindings;

/// The goals that turn `old` (the value at `path` in the source) into `new`.
///
/// `path` is a goal path: a top-level name, then `.field` / `[index]` steps.
/// Goals are ordered to be applied in sequence: for each list, the removals
/// back to front, then the insertions front to back (each at its final
/// index), then the edits inside elements that stayed, at their final index.
/// An empty result means the values are already the same.
pub fn diff_goals(path: &str, old: &StaticValue, new: &StaticValue) -> Vec<Goal> {
    let mut goals = Vec::new();
    diff_into(path, old, new, true, &mut goals);
    goals
}

/// [`diff_goals`] against the top-level binding `name` in `source`, then
/// applied: the rewritten source and the goals that made it. `Ok` with no
/// goals returns `source` unchanged.
pub fn apply_drop(source: &str, name: &str, new: &StaticValue) -> Result<(String, Vec<Goal>), GoalError> {
    let old = binding_value(source, name)?;
    let goals = diff_goals(name, &old, new);
    if goals.is_empty() {
        return Ok((source.to_string(), goals));
    }
    let out = modify_source_with_goals(source, &goals)?;
    Ok((out, goals))
}

/// The top-level `let` bindings in `source` whose literal value is `model`
/// (compared as the runtime sees it: a colour literal equals its `{r, g, b}`
/// record, an int equals the same float), in source order. `state` and `fn`
/// declarations are never candidates, nor are bindings without a static
/// value.
pub fn literal_bindings(source: &str, model: &StaticValue) -> Result<Vec<String>, GoalError> {
    let bindings = static_bindings(source).map_err(|e| GoalError::from(e.to_string()))?;
    Ok(bindings
        .into_iter()
        .filter(|b| b.text.is_some())
        .filter(|b| b.value.as_ref().is_ok_and(|v| same(v, model)))
        .map(|b| b.name)
        .collect())
}

/// A drop, written back: which binding, the goals, and the new source.
#[derive(Debug, Clone, PartialEq)]
pub struct DropEdit {
    pub binding: String,
    pub goals: Vec<Goal>,
    pub source: String,
}

/// Turn a drop on `model` into a source edit: find the one top-level literal
/// binding `model` is ([`literal_bindings`]), diff it against `drop`
/// ([`diff_goals`]) and apply the goals. `model` and `drop` are the runtime
/// values as JSON ([`static_from_json`] reads them).
///
/// Errors say why there is no edit: the model is no literal in this file,
/// it is ambiguous (two bindings hold the same value), a value cannot be
/// written as a literal, or a goal could not be applied.
pub fn propose_drop(source: &str, model: &serde_json::Value, drop: &serde_json::Value) -> Result<DropEdit, GoalError> {
    let model = static_from_json(model)
        .ok_or_else(|| GoalError::from("the dragged model is not a value a literal can spell".to_string()))?;
    let new = static_from_json(drop)
        .ok_or_else(|| GoalError::from("the drop is not a value a literal can spell".to_string()))?;
    let names = literal_bindings(source, &model)?;
    let binding = match names.as_slice() {
        [one] => one.clone(),
        [] => {
            return Err(GoalError::from(
                "the dragged model is not a top-level `let` literal in this file".to_string(),
            ))
        }
        many => {
            return Err(GoalError::from(format!(
                "the dragged model is the value of {} bindings ({}); give it one",
                many.len(),
                many.join(", ")
            )))
        }
    };
    let (source, goals) = apply_drop(source, &binding, &new)?;
    Ok(DropEdit {
        binding,
        goals,
        source,
    })
}

/// A runtime value's JSON ([`crate::value::value_to_json`]) as a literal:
/// nil, bools, numbers (an integer stays an int), strings, lists and records
/// whose field names are identifiers. `None` for anything a literal cannot
/// spell (a `Map()`, a vector, a dual, a handle's tagged form).
pub fn static_from_json(v: &serde_json::Value) -> Option<StaticValue> {
    use serde_json::Value as J;
    Some(match v {
        J::Null => StaticValue::Nil,
        J::Bool(b) => StaticValue::Bool(*b),
        J::Number(n) => match n.as_i64() {
            Some(i) => StaticValue::Int(i),
            None => StaticValue::Float(n.as_f64()?),
        },
        J::String(s) => StaticValue::Str(s.clone()),
        J::Array(xs) => StaticValue::List(xs.iter().map(static_from_json).collect::<Option<_>>()?),
        J::Object(o) => {
            if o.get("type").and_then(|t| t.as_str()).is_some()
                && matches!(
                    o.get("type").and_then(|t| t.as_str()),
                    Some("map" | "dual" | "vec2" | "vec3" | "pending" | "handle" | "function")
                )
            {
                return None;
            }
            let mut fields = Vec::with_capacity(o.len());
            for (k, v) in o {
                if !is_ident(k) {
                    return None;
                }
                fields.push((k.clone(), static_from_json(v)?));
            }
            StaticValue::Record(fields)
        }
    })
}

// ── The diff ─────────────────────────────────────────────────────────────

fn diff_into(path: &str, old: &StaticValue, new: &StaticValue, root: bool, out: &mut Vec<Goal>) {
    if same(old, new) {
        return;
    }
    match (old, new) {
        (StaticValue::List(o), StaticValue::List(n)) => diff_list(path, o, n, out),
        (StaticValue::Record(o), StaticValue::Record(n)) if n.iter().all(|(k, _)| is_ident(k)) => {
            for (k, nv) in n {
                let p = format!("{path}.{k}");
                match field(o, k) {
                    Some(ov) => diff_into(&p, ov, nv, false, out),
                    None => out.push(Goal::should_insert(p, nv.clone())),
                }
            }
            for (k, _) in o {
                if field(n, k).is_none() {
                    out.push(Goal::should_remove(format!("{path}.{k}")));
                }
            }
        }
        // A colour the runtime holds as a record, a changed scalar, a change
        // of shape: write the new value (in the source's spelling where it
        // has one).
        _ => {
            let v = spelled(new, std::slice::from_ref(old));
            out.push(if root {
                Goal::should_set_value(path, v)
            } else {
                Goal::should_set_path(path, v)
            });
        }
    }
}

fn diff_list(path: &str, old: &[StaticValue], new: &[StaticValue], out: &mut Vec<Goal>) {
    let pairs = lcs(old, new);
    let kept_old: Vec<bool> = {
        let mut k = vec![false; old.len()];
        for &(i, _) in &pairs {
            k[i] = true;
        }
        k
    };
    let mut kept_new = vec![false; new.len()];
    for &(_, j) in &pairs {
        kept_new[j] = true;
    }
    // Removals back to front, so each index still names the element it did.
    let removed: Vec<StaticValue> = (0..old.len())
        .filter(|&i| !kept_old[i])
        .map(|i| old[i].clone())
        .collect();
    for i in (0..old.len()).rev().filter(|&i| !kept_old[i]) {
        out.push(Goal::should_remove(format!("{path}[{i}]")));
    }
    // What is left is the kept elements in their new order; inserting front
    // to back puts each new one at its final index.
    for j in (0..new.len()).filter(|&j| !kept_new[j]) {
        out.push(Goal::should_insert(format!("{path}[{j}]"), spelled(&new[j], &removed)));
    }
    for &(i, j) in &pairs {
        diff_into(&format!("{path}[{j}]"), &old[i], &new[j], false, out);
    }
}

/// The longest common subsequence of `old` and `new` by [`identity`]: the
/// `(old index, new index)` pairs of the elements that stay.
fn lcs(old: &[StaticValue], new: &[StaticValue]) -> Vec<(usize, usize)> {
    let (n, m) = (old.len(), new.len());
    let ids_o: Vec<&StaticValue> = old.iter().map(identity).collect();
    let ids_n: Vec<&StaticValue> = new.iter().map(identity).collect();
    // len[i][j]: the LCS of old[i..] and new[j..].
    let mut len = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            len[i][j] = if same(ids_o[i], ids_n[j]) {
                len[i + 1][j + 1] + 1
            } else {
                len[i + 1][j].max(len[i][j + 1])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if same(ids_o[i], ids_n[j]) {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if len[i + 1][j] >= len[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// What makes an element "the same element" across a drop: a record's `id`,
/// else its `key` (the fields `ui/drag`'s helpers identify items by), else
/// the whole value.
fn identity(v: &StaticValue) -> &StaticValue {
    if let StaticValue::Record(fields) = v {
        if let Some(id) = field(fields, "id").or_else(|| field(fields, "key")) {
            return id;
        }
    }
    v
}

/// `new` as the source would spell it: an equal value from `pool` (the old
/// element that moved, the old value being replaced) when there is one,
/// keeping its colour, variant or call form; otherwise `new`, with records
/// that are colours where the pool's counterpart was a colour.
fn spelled(new: &StaticValue, pool: &[StaticValue]) -> StaticValue {
    if let Some(p) = pool.iter().find(|p| same(p, new)) {
        return (*p).clone();
    }
    let colour_nearby = pool.iter().any(|p| matches!(p, StaticValue::Color { .. }));
    match new {
        StaticValue::Record(_) if colour_nearby => as_color(new).unwrap_or_else(|| new.clone()),
        StaticValue::List(items) => StaticValue::List(items.iter().map(|it| spelled(it, &[])).collect()),
        _ => new.clone(),
    }
}

/// Equal as the running program would see them.
fn same(a: &StaticValue, b: &StaticValue) -> bool {
    use StaticValue as S;
    match (a, b) {
        (S::Int(x), S::Float(y)) | (S::Float(y), S::Int(x)) => (*x as f64) == *y,
        (S::Float(x), S::Float(y)) => x == y,
        (S::List(x), S::List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (S::Record(x), S::Record(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| field(y, k).is_some_and(|w| same(v, w)))
        }
        (S::Color { .. }, S::Record(_)) => as_color(b).is_some_and(|c| c == *a),
        (S::Record(_), S::Color { .. }) => as_color(a).is_some_and(|c| c == *b),
        _ => a == b,
    }
}

/// A `{r, g, b}` / `{r, g, b, a}` record of 0–255 ints as a colour literal.
fn as_color(v: &StaticValue) -> Option<StaticValue> {
    let StaticValue::Record(fields) = v else {
        return None;
    };
    let ch = |k: &str| match field(fields, k)? {
        StaticValue::Int(n) if (0..=255).contains(n) => Some(*n as u8),
        StaticValue::Float(f) if f.fract() == 0.0 && (0.0..=255.0).contains(f) => Some(*f as u8),
        _ => None,
    };
    let known = fields.iter().all(|(k, _)| matches!(k.as_str(), "r" | "g" | "b" | "a"));
    if !known || !(3..=4).contains(&fields.len()) {
        return None;
    }
    let a = if field(fields, "a").is_some() { Some(ch("a")?) } else { None };
    Some(StaticValue::Color {
        r: ch("r")?,
        g: ch("g")?,
        b: ch("b")?,
        a,
    })
}

fn field<'a>(fields: &'a [(String, StaticValue)], k: &str) -> Option<&'a StaticValue> {
    fields.iter().find(|(f, _)| f == k).map(|(_, v)| v)
}

fn binding_value(source: &str, name: &str) -> Result<StaticValue, GoalError> {
    crate::static_value::get_static_value(source, name).map_err(|e| GoalError::from(e.to_string()))
}

fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn strs(xs: &[&str]) -> StaticValue {
        StaticValue::List(xs.iter().map(|s| StaticValue::str(*s)).collect())
    }

    #[test]
    fn a_move_is_one_removal_and_one_insertion() {
        let goals = diff_goals("STEPS", &strs(&["a", "b", "c", "d"]), &strs(&["b", "c", "a", "d"]));
        assert_eq!(
            goals,
            vec![Goal::should_remove("STEPS[0]"), Goal::should_insert("STEPS[2]", "a")]
        );
        assert!(diff_goals("STEPS", &strs(&["a"]), &strs(&["a"])).is_empty());
    }

    #[test]
    fn records_match_by_id_and_diff_by_field() {
        let card = |id: i64, t: &str| StaticValue::record(vec![("id", StaticValue::int(id)), ("t", StaticValue::str(t))]);
        let old = StaticValue::List(vec![card(1, "x"), card(2, "y")]);
        let new = StaticValue::List(vec![card(2, "y"), card(1, "z")]);
        let goals = diff_goals("B", &old, &new);
        assert_eq!(
            goals,
            vec![
                Goal::should_remove("B[0]"),
                Goal::should_insert("B[1]", card(1, "z")),
            ]
        );
        let renamed = StaticValue::List(vec![card(1, "z"), card(2, "y")]);
        assert_eq!(diff_goals("B", &old, &renamed), vec![Goal::should_set_path("B[0].t", "z")]);
    }

    #[test]
    fn json_values_read_as_literals() {
        assert_eq!(static_from_json(&json!([1, 2.5, "a", null, true])), Some(StaticValue::List(vec![
            StaticValue::int(1), StaticValue::float(2.5), StaticValue::str("a"), StaticValue::Nil, StaticValue::Bool(true)
        ])));
        assert_eq!(static_from_json(&json!({"type": "map", "entries": []})), None);
        assert_eq!(static_from_json(&json!({"bad key": 1})), None);
    }
}
