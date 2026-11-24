//! Rule — **`prefer-color-arg`**: pass a colour whole instead of unpacking
//! its channels into a draw call that takes a colour.
//!
//! ```text
//! clear(BG.r, BG.g, BG.b)                        =>   clear(BG)
//! draw_line(x1, y1, x2, y2, c.r, c.g, c.b, 200)  =>   draw_line(x1, y1, x2, y2, c, 200)
//! ```
//!
//! Every `ui` draw call with a flat `r, g, b` form also has a form taking one
//! colour value (a hex literal, a `{r, g, b}` record) in the same place, and
//! that form reads `c.r, c.g, c.b` and hands them to the same native, so the
//! two calls draw the same thing. [`COLOR_ARG_CALLS`] lists the calls and
//! where their channels sit; `core-stdlib/ui/native/tests/prelude.rs` runs
//! both spellings of every entry and compares the draw commands, so the
//! table cannot drift from the library.
//!
//! It fires when three positional arguments in the channel slot are `E.r`,
//! `E.g`, `E.b` of one stable expression `E` ([`is_stable_place`]: names,
//! literals, field and index reads, so evaluating it once instead of three
//! times changes nothing). A file that declares or imports its own function
//! of that name is left alone: its calls are not the library's.

use std::collections::HashSet;

use crate::ast::{Expr, ExprKind, ExprVisitor, Stmt, walk_expr};

use super::Fix;
use super::compound::is_stable_place;
use super::to_match::Splice;
use super::util::{bound_names, span_text, squash};

/// A `ui` draw call with a flat `r, g, b` form: its name, the index of the
/// `r` argument, and the argument counts the flat form takes.
#[derive(Debug, Clone, Copy)]
pub struct ColorArgCall {
    pub name: &'static str,
    pub channel: usize,
    pub min_args: usize,
    pub max_args: usize,
}

const fn call(
    name: &'static str,
    channel: usize,
    min_args: usize,
    max_args: usize,
) -> ColorArgCall {
    ColorArgCall {
        name,
        channel,
        min_args,
        max_args,
    }
}

/// Every call the rule rewrites. Trailing arguments after `b` are alpha and,
/// where the call has one, stroke width.
pub const COLOR_ARG_CALLS: &[ColorArgCall] = &[
    call("clear", 0, 3, 3),
    call("draw_rect", 4, 7, 8),
    call("draw_rect_rounded", 5, 8, 9),
    call("draw_rect_outline", 4, 7, 9),
    call("draw_rect_rounded_outline", 5, 8, 10),
    call("draw_line", 4, 7, 9),
    call("draw_circle", 3, 6, 7),
    call("draw_circle_outline", 3, 6, 8),
    call("draw_ellipse", 4, 7, 8),
    call("draw_ellipse_outline", 4, 7, 9),
    call("fill_arc", 6, 9, 10),
    call("fill_triangle", 6, 9, 10),
    call("fill_poly", 1, 4, 5),
    call("fill_polygon", 1, 4, 5),
    call("fill_fan", 3, 6, 7),
    call("draw_polyline", 1, 4, 6),
    call("draw_polygon_outline", 1, 4, 6),
    call("draw_text", 4, 7, 8),
];

/// Plan every rewrite in `stmts`, in source order.
pub(super) fn plan_color_arg_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let mut finder = Finder {
        chars,
        shadowed: bound_names(stmts),
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

struct Finder<'a> {
    chars: &'a [char],
    /// Names the file declares or imports itself.
    shadowed: HashSet<String>,
    fixes: Vec<Fix>,
}

impl ExprVisitor for Finder<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        if let Some(fix) = self.plan(e) {
            self.fixes.push(fix);
        }
        // The colour expression is a stable place, so it holds no call for a
        // nested fix to overlap; other arguments may.
        walk_expr(self, e);
    }
}

impl Finder<'_> {
    fn plan(&self, e: &Expr) -> Option<Fix> {
        let ExprKind::Call {
            function,
            args,
            arg_names,
        } = &e.kind
        else {
            return None;
        };
        let ExprKind::Ident(name) = &function.kind else {
            return None;
        };
        if !arg_names.is_empty() || self.shadowed.contains(name) {
            return None;
        }
        let entry = COLOR_ARG_CALLS.iter().find(|c| c.name == name)?;
        if !(entry.min_args..=entry.max_args).contains(&args.len()) {
            return None;
        }
        let channels = &args[entry.channel..entry.channel + 3];
        let mut object_text = None;
        for (arg, want) in channels.iter().zip(["r", "g", "b"]) {
            let ExprKind::FieldAccess { object, field } = &arg.kind else {
                return None;
            };
            if field != want || !is_stable_place(object) {
                return None;
            }
            let text = span_text(self.chars, object.span)?;
            match &object_text {
                None => object_text = Some(text),
                Some(first) if squash(first) == squash(&text) => {}
                Some(_) => return None,
            }
        }
        let color = object_text?;
        Some(Fix {
            anchor: channels[0].span.start.offset as usize,
            message: format!(
                "`{name}` takes a colour: pass `{color}` instead of `{color}.r, {color}.g, {color}.b`"
            ),
            splices: vec![Splice {
                start: channels[0].span.start.offset as usize,
                end: channels[2].span.end.offset as usize,
                text: color,
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(src: &str) -> String {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        let chars: Vec<char> = src.chars().collect();
        let fixes = plan_color_arg_fixes(&stmts, &chars);
        super::super::to_match::apply_match_edits(&chars, &super::super::flatten(fixes))
    }

    #[test]
    fn unpacked_channels_become_the_colour() {
        assert_eq!(fix("clear(BG.r, BG.g, BG.b)\n"), "clear(BG)\n");
        assert_eq!(
            fix("draw_line(0, 0, 9, 9, t.fg.r, t.fg.g, t.fg.b, 200, 2)\n"),
            "draw_line(0, 0, 9, 9, t.fg, 200, 2)\n"
        );
        assert_eq!(
            fix("draw_text(\"a\", 1, 2, 12, P[i].r, P[i].g, P[i].b)\n"),
            "draw_text(\"a\", 1, 2, 12, P[i])\n"
        );
    }

    #[test]
    fn leaves_other_shapes_alone() {
        for src in [
            // Different colours, wrong order, a call in the colour, numbers.
            "clear(a.r, b.g, a.b)\n",
            "clear(c.g, c.r, c.b)\n",
            "clear(pick().r, pick().g, pick().b)\n",
            "clear(1, 2, 3)\n",
            // Not the channel slot, or not the flat form's argument count.
            "draw_rect(c.r, c.g, c.b, 4, 1, 2, 3)\n",
            "draw_rect(0, 0, 4, 4, c.r, c.g, c.b, 255, 9)\n",
            // Named arguments, and a file with its own `clear`.
            "clear(r: c.r, g: c.g, b: c.b)\n",
            "fn clear(r, g, b) r end\nclear(c.r, c.g, c.b)\n",
        ] {
            assert_eq!(fix(src), src, "{src}");
        }
    }
}
