//! Rules about the direct-manipulation hints `tunable` and `rigid`
//! (docs/proposals/dm-v2-hints.md, docs/tasks/lifecycle-annotation-functions-refactor.md).
//! All of them only report: what to write instead is the author's call.
//!
//! **`dm-hint-not-literal`**: `tunable(e)` where `e` is computed
//! (`tunable(a + b)`). The hint marks the literal its argument is, the
//! literal a name is bound to, or the number literals in data (a record, a
//! list, a constructor call), so a computed argument marks nothing. A
//! setting's computed value (`let X = tunable(level.SPAWN)`) is reported by
//! `config-not-data` instead. (A name bound to a computed value is caught
//! where the hints are read, `crate::dm::hints`, which sees bindings.)
//!
//! **`dm-hint-bounds`**: a `tunable` bound (`min`, `max`, `step`) that is not
//! a number literal, so no tool can read it without running the program; a
//! `min` above the `max`; or bounds on a value that is not a number
//! (`tunable("title", 0, 1)`), where they mean nothing.
//!
//! **`dm-hint-state-init`**: `state x = tunable(…)`. A running program keeps
//! its state across a hot reload, so a drag cannot move a state's initial
//! value; the start value belongs in a setting the state reads.
//!
//! **`tunable-value-not-in-options`**: `tunable("x", options: ["a", "b"])`,
//! whose value is not one of its own choices.
//!
//! **`tunable-on-var`**: a top-level `var X = tunable(…)`. A `var` is a
//! mutable cell, not a setting, so hosts do not list it.
//!
//! **`tunable-not-whole-initializer`**: a top-level `let X = tunable(4) * 2`.
//! Only a `let` whose whole initializer is the `tunable` call is a setting;
//! here the hint marks the `4` for direct manipulation, and hosts do not
//! list `X`.

use crate::ast::{Expr, ExprKind, ExprVisitor, Literal, Stmt, StmtKind, walk_expr, walk_stmt};

use super::Fix;

/// The number a literal expression is (`4`, `-2.5`), if it is one.
fn number(e: &Expr) -> Option<f64> {
    match &e.kind {
        ExprKind::Literal(Literal::Int(n)) => Some(*n as f64),
        ExprKind::Literal(Literal::Float(f)) => Some(*f),
        _ => None,
    }
}

/// `tunable(…)`'s arguments, with each one's name (`None` = positional).
fn tunable_call(e: &Expr) -> Option<Vec<(Option<&str>, &Expr)>> {
    let ExprKind::Call { function, args, arg_names } = &e.kind else { return None };
    let ExprKind::Ident(f) = &function.kind else { return None };
    if f != "tunable" {
        return None;
    }
    Some(
        args.iter()
            .enumerate()
            .map(|(i, a)| (arg_names.get(i).and_then(|n| n.as_deref()), a))
            .collect(),
    )
}

/// The rules' findings, by rule name.
pub(super) struct HintFindings {
    pub not_literal: Vec<Fix>,
    pub bounds: Vec<Fix>,
    pub state_init: Vec<Fix>,
    pub not_in_options: Vec<Fix>,
    pub on_var: Vec<Fix>,
    pub not_whole: Vec<Fix>,
}

pub(super) fn plan_hint_reports(stmts: &[Stmt], chars: &[char]) -> HintFindings {
    let mut v = Finder {
        chars,
        settings: Vec::new(),
        out: HintFindings {
            not_literal: Vec::new(),
            bounds: Vec::new(),
            state_init: Vec::new(),
            not_in_options: Vec::new(),
            on_var: Vec::new(),
            not_whole: Vec::new(),
        },
    };
    // Top-level `let`s: settings, and the two ways of almost writing one.
    for s in stmts {
        let StmtKind::Let { name, value, is_var, is_const, .. } = &s.kind else { continue };
        let is_const = &crate::static_value::declares_const_let(value, *is_var, *is_const);
        if tunable_call(value).is_some() {
            if *is_var {
                v.out.on_var.push(report(
                    value,
                    format!(
                        "a `var` is a mutable cell, not a setting, so hosts do not list `{name}`; \
                         write `let {name} = tunable(…)`"
                    ),
                ));
            } else if !*is_const {
                v.settings.push(value as *const Expr);
            }
        } else if !*is_var && !*is_const && wraps_tunable_operand(value) {
            v.out.not_whole.push(report(
                value,
                format!(
                    "`{name}` is not a setting: hosts list a `let` only when its whole \
                     initializer is `tunable(…)`. Here the hint only marks the number for \
                     direct manipulation"
                ),
            ));
        }
    }
    for s in stmts {
        v.visit_stmt(s);
    }
    for list in [
        &mut v.out.not_literal,
        &mut v.out.bounds,
        &mut v.out.state_init,
        &mut v.out.not_in_options,
        &mut v.out.on_var,
        &mut v.out.not_whole,
    ] {
        list.sort_by_key(|f| f.anchor);
    }
    v.out
}

/// `e` is an operator expression with a `tunable(…)` call as a direct
/// operand (`tunable(4) * 2`, `-tunable(4)`).
fn wraps_tunable_operand(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::BinaryOp { left, right, .. } => {
            tunable_call(left).is_some() || tunable_call(right).is_some()
        }
        ExprKind::UnaryOp { operand, .. } => tunable_call(operand).is_some(),
        _ => false,
    }
}

struct Finder<'a> {
    chars: &'a [char],
    /// The `tunable` calls that are a top-level setting's whole initializer.
    /// By identity, not span: the call an `@tunable` annotation builds shares
    /// its span with the value it wraps, which may be a written
    /// `tunable(…)` (`@tunable let X = tunable(a + b)`).
    settings: Vec<*const Expr>,
    out: HintFindings,
}

/// What kind of data `e` is, when it is a literal of something other than a
/// number: valid as a setting, but bounds mean nothing for it.
fn non_number_kind(e: &Expr) -> Option<&'static str> {
    if crate::static_value::color_literal(e).is_some() {
        return Some("a color");
    }
    Some(match &e.kind {
        ExprKind::Literal(Literal::String(_)) => "a string",
        ExprKind::Literal(Literal::Bool(_)) => "a bool",
        ExprKind::Literal(Literal::Nil) => "`nil`",
        ExprKind::List(_) => "a list",
        ExprKind::Record(_) => "a record",
        ExprKind::Call { .. } if is_data(e) => "a constructor call",
        _ => return None,
    })
}

/// Whether `e` is data: a literal, or a list, record or call whose every
/// part is data (`rgb(255, 0, 80)`, `{x: 1, c: vec3(0, 1, 0)}`).
fn is_data(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Literal(_) => true,
        ExprKind::List(items) => items.iter().all(is_data),
        ExprKind::Record(fields) => fields.iter().all(|f| match f {
            crate::ast::RecordField::Named(_, v) => is_data(v),
            _ => false,
        }),
        ExprKind::Call { function, args, .. } => {
            matches!(function.kind, ExprKind::Ident(_)) && args.iter().all(is_data)
        }
        _ => false,
    }
}

/// `text`, shortened to one line of at most 40 characters.
fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim_end();
    let mut out: String = line.chars().take(40).collect();
    if out.chars().count() < text.trim_end().chars().count() {
        out.push('…');
    }
    out
}

fn report(anchor: &Expr, message: String) -> Fix {
    Fix { anchor: anchor.span.start.offset as usize, message, splices: Vec::new() }
}

impl Finder<'_> {
    fn text(&self, e: &Expr) -> String {
        let start = (e.span.start.offset as usize).min(self.chars.len());
        let end = (e.span.end.offset as usize).clamp(start, self.chars.len());
        self.chars[start..end].iter().collect()
    }
}

impl ExprVisitor for Finder<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        if let StmtKind::State { init, .. } = &s.kind
            && tunable_call(init).is_some()
        {
            self.out.state_init.push(report(
                init,
                "a drag cannot move a running `state`'s initial value (hot reload keeps state); \
                 declare the start value as a setting, `let START = tunable(…)`, and initialise \
                 the state from it"
                    .to_string(),
            ));
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        if let Some(args) = tunable_call(e) {
            let is_setting = self.settings.contains(&(e as *const Expr));
            let mut positional = 0usize;
            let mut bounds: [Option<f64>; 3] = [None; 3];
            let mut any_bound = false;
            let mut value: Option<&Expr> = None;
            let mut options: Option<&Expr> = None;
            for (name, a) in &args {
                let slot = match name {
                    Some(n) => {
                        ["value", "min", "max", "step", "options"].iter().position(|x| x == n)
                    }
                    None => {
                        positional += 1;
                        Some(positional - 1)
                    }
                };
                match slot {
                    Some(0) => value = Some(a),
                    Some(k @ 1..=3) => match number(a) {
                        Some(v) => {
                            bounds[k - 1] = Some(v);
                            any_bound = true;
                        }
                        None if matches!(a.kind, ExprKind::Literal(Literal::Nil)) => {}
                        None => {
                            any_bound = true;
                            self.out.bounds.push(report(
                                a,
                                "`tunable` bounds must be literals so tools can read them \
                                 without running the program"
                                    .to_string(),
                            ));
                        }
                    },
                    Some(4) => options = Some(a),
                    _ => {}
                }
            }
            if let Some(a) = value {
                let marks = number(a).is_some()
                    || matches!(a.kind, ExprKind::Ident(_) | ExprKind::CellGet(_));
                match non_number_kind(a) {
                    _ if marks => {}
                    Some(kind) => {
                        if any_bound {
                            self.out.bounds.push(report(
                                a,
                                crate::dm::hints::bounds_on_non_number_message(
                                    &short(&self.text(a)),
                                    kind,
                                ),
                            ));
                        }
                    }
                    // A setting's computed value is `config-not-data`'s.
                    None if is_setting => {}
                    None => self.out.not_literal.push(report(
                        a,
                        "`tunable` marks a literal or a name bound to one; this argument is \
                         computed, so nothing is marked. Wrap the literal itself, as in \
                         `tunable(a) + b`"
                            .to_string(),
                    )),
                }
                if let (Some(ExprKind::List(choices)), ExprKind::Literal(lit)) =
                    (options.map(|o| &o.kind), &a.kind)
                    && choices.iter().all(|c| matches!(c.kind, ExprKind::Literal(_)))
                    && !choices
                        .iter()
                        .any(|c| matches!(&c.kind, ExprKind::Literal(l) if l == lit))
                {
                    self.out.not_in_options.push(report(
                        a,
                        format!(
                            "`{}` is not one of this `tunable`'s `options:`",
                            short(&self.text(a))
                        ),
                    ));
                }
            }
            if let [Some(lo), Some(hi), _] = bounds
                && lo > hi
            {
                self.out.bounds.push(report(
                    e,
                    format!("`tunable` range is empty: min {lo} is above max {hi}"),
                ));
            }
        }
        walk_expr(self, e);
    }
}
