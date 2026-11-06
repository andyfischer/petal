//! **`config-not-data`** — a setting whose value is not static data
//! (`let SPAWN = tunable(level.SPAWN)`, `config let W = 2 * TILE`).
//!
//! A setting (`let X = tunable(…)`, or the deprecated `config let`) is a
//! binding that hosts list, set and override without running the program:
//! they read it with [`crate::static_value::static_bindings`]. A value that
//! needs evaluating has no value there, so a host's settings panel shows the
//! binding with nothing to edit, and `--set`-style overrides replace text
//! the program computes. For a setting this rule replaces
//! `dm-hint-not-literal`, which would say the same thing about the hint.
//!
//! Report only: whether the value should be written out or the binding
//! should be a plain `let` is the author's call.

use crate::ast::{Stmt, StmtKind};
use crate::static_value::{bindings_of, is_setting, setting_value_expr};

use super::Fix;

pub(super) fn plan_config_data_reports(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let mut out = Vec::new();
    for (i, stmt) in stmts.iter().enumerate() {
        let StmtKind::Let {
            name,
            value,
            is_config,
            is_var,
            is_const,
            ..
        } = &stmt.kind
        else {
            continue;
        };
        if !is_setting(value, *is_config, *is_var, *is_const) {
            continue;
        }
        // What the host sees: the binding as the file stands up to here.
        let bindings = bindings_of(&stmts[..=i], chars);
        let Some(Err(reason)) = bindings
            .iter()
            .find(|b| &b.name == name)
            .map(|b| b.value.as_ref())
        else {
            continue;
        };
        let expr = setting_value_expr(value);
        let start = (expr.span.start.offset as usize).min(chars.len());
        let end = (expr.span.end.offset as usize).clamp(start, chars.len());
        let text: String = chars[start..end].iter().collect();
        let text = text.lines().next().unwrap_or("").trim_end();
        let shown: String = text.chars().take(40).collect();
        let shown = if shown.chars().count() < text.chars().count() {
            format!("{shown}…")
        } else {
            shown
        };
        out.push(Fix {
            anchor: start,
            message: format!(
                "a setting's value is read by hosts without running the program; \
                 `{shown}` is {reason}, so hosts see no value"
            ),
            splices: Vec::new(),
        });
    }
    out
}
