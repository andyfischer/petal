//! Geometric pane navigation — vim's `Ctrl+W` window commands.
//!
//! Given the laid-out pane rectangles and the currently focused pane, pick the
//! neighbor to move to in a spatial direction (`h`/`j`/`k`/`l`). Pure and
//! unit-tested; [`crate::app::App`] owns the focus index and the `Ctrl+W`
//! prefix state, and only calls in here to resolve a direction to a pane.

use garden_render::Rect;
use garden_script::LayoutNode;

/// A spatial direction for window navigation (vim's `Ctrl+W` h/j/k/l).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    /// Map the second key of a `Ctrl+W` chord to a direction (vim's
    /// h/j/k/l), or `None` if the key does not name one.
    pub fn from_key(c: char) -> Option<Direction> {
        match c {
            'h' => Some(Direction::Left),
            'j' => Some(Direction::Down),
            'k' => Some(Direction::Up),
            'l' => Some(Direction::Right),
            _ => None,
        }
    }

    /// True for the horizontal pair (Left/Right); false for the vertical one.
    fn is_horizontal(self) -> bool {
        matches!(self, Direction::Left | Direction::Right)
    }
}

/// The pane to focus when moving `dir` from pane `from`, or `None` when there
/// is no pane on that side (so the caller leaves focus where it is).
///
/// Among the panes whose center lies on the correct side of the focused pane's
/// center, this prefers one whose perpendicular span overlaps the focused pane
/// (a true row/column neighbor), then the nearest along the travel axis, then
/// the nearest by center on the cross axis. The overlap preference keeps
/// navigation intuitive in ragged layouts where a closer-but-offset pane would
/// otherwise win.
pub fn neighbor(rects: &[Rect], from: usize, dir: Direction) -> Option<usize> {
    let cur = *rects.get(from)?;
    let (cx, cy) = center(cur);

    let mut best: Option<(usize, (bool, f32, f32))> = None;
    for (i, &r) in rects.iter().enumerate() {
        if i == from {
            continue;
        }
        let (rcx, rcy) = center(r);
        let on_side = match dir {
            Direction::Left => rcx < cx,
            Direction::Right => rcx > cx,
            Direction::Up => rcy < cy,
            Direction::Down => rcy > cy,
        };
        if !on_side {
            continue;
        }

        let disjoint = perpendicular_overlap(cur, r, dir) <= 0.0;
        let (primary, secondary) = if dir.is_horizontal() {
            ((rcx - cx).abs(), (rcy - cy).abs())
        } else {
            ((rcy - cy).abs(), (rcx - cx).abs())
        };
        // Sort key: overlapping panes first (`disjoint` false < true), then the
        // smallest gap along the travel axis, then the closest cross-axis center.
        let key = (disjoint, primary, secondary);
        if best.is_none_or(|(_, b)| key < b) {
            best = Some((i, key));
        }
    }
    best.map(|(i, _)| i)
}

/// Replace the `target`-th leaf of `node` with `replacement`, counting leaves
/// (editor/process panes) in the same depth-first order the layout solver emits
/// panes — so `target` is a pane's focus index. Returns `true` if a leaf was
/// replaced, `false` if `target` is out of range (the tree is left untouched).
///
/// This is how a split splices a two-child `row`/`column` in at the focused
/// pane while leaving the rest of the layout tree intact.
pub fn replace_leaf(node: &mut LayoutNode, target: usize, replacement: LayoutNode) -> bool {
    let mut next = 0;
    let mut replacement = Some(replacement);
    replace_leaf_at(node, target, &mut next, &mut replacement)
}

/// Recursive worker for [`replace_leaf`]: walks leaves left→right tracking the
/// running index in `next`, and moves `replacement` out of the slot the first
/// time the index matches `target`.
fn replace_leaf_at(
    node: &mut LayoutNode,
    target: usize,
    next: &mut usize,
    replacement: &mut Option<LayoutNode>,
) -> bool {
    match node {
        LayoutNode::Editor { .. } | LayoutNode::Process { .. } | LayoutNode::Panel { .. } => {
            if *next == target {
                *node = replacement.take().expect("replacement taken at most once");
                true
            } else {
                *next += 1;
                false
            }
        }
        LayoutNode::Row { children, .. } | LayoutNode::Column { children, .. } => children
            .iter_mut()
            .any(|child| replace_leaf_at(child, target, next, replacement)),
    }
}

/// Remove the `target`-th leaf of `node`, counting leaves in the same
/// depth-first order the layout solver emits panes — so `target` is a pane's
/// focus index. A `row`/`column` left with a single child collapses into that
/// child (and a removed child's ratio entry is dropped with it, the remaining
/// ratios staying proportional — the solver normalizes them). Returns `true` if
/// a leaf was removed; `false` — tree untouched — when `target` is out of range
/// or the tree has only one leaf (the last pane can't be closed away).
pub fn remove_leaf(node: &mut LayoutNode, target: usize) -> bool {
    if count_leaves(node) <= 1 {
        return false;
    }
    let mut next = 0;
    remove_leaf_at(node, target, &mut next) == Removal::Handled
}

/// Leaves (editor/process/panel panes) under `node`, in solver order.
fn count_leaves(node: &LayoutNode) -> usize {
    match node {
        LayoutNode::Editor { .. } | LayoutNode::Process { .. } | LayoutNode::Panel { .. } => 1,
        LayoutNode::Row { children, .. } | LayoutNode::Column { children, .. } => {
            children.iter().map(count_leaves).sum()
        }
    }
}

/// Outcome of a [`remove_leaf`] walk over one subtree.
#[derive(PartialEq, Eq)]
enum Removal {
    /// The target leaf is this node itself — the *parent* must remove it
    /// (a node can't delete itself in place).
    RemoveMe,
    /// The removal happened somewhere inside; the tree is already fixed up.
    Handled,
    /// The target is not in this subtree (`next` has advanced past it).
    NotHere,
}

/// Recursive worker for [`remove_leaf`]: walks leaves left→right tracking the
/// running index in `next`; the container holding the target drops that child
/// (and its ratio entry) and collapses itself if one child remains.
fn remove_leaf_at(node: &mut LayoutNode, target: usize, next: &mut usize) -> Removal {
    match node {
        LayoutNode::Editor { .. } | LayoutNode::Process { .. } | LayoutNode::Panel { .. } => {
            if *next == target {
                Removal::RemoveMe
            } else {
                *next += 1;
                Removal::NotHere
            }
        }
        LayoutNode::Row { children, ratios } | LayoutNode::Column { children, ratios } => {
            for i in 0..children.len() {
                match remove_leaf_at(&mut children[i], target, next) {
                    Removal::RemoveMe => {
                        children.remove(i);
                        if let Some(r) = ratios {
                            if i < r.len() {
                                r.remove(i);
                            }
                        }
                        if children.len() == 1 {
                            *node = children.remove(0);
                        }
                        return Removal::Handled;
                    }
                    Removal::Handled => return Removal::Handled,
                    Removal::NotHere => {}
                }
            }
            Removal::NotHere
        }
    }
}

/// Replace every leaf of `node` (editor/process panes), in the same depth-first
/// order the layout solver emits panes, with the next entry pulled from
/// `leaves`. The row/column structure and ratios are left intact — only the leaf
/// *content* is swapped. Stops early if `leaves` runs out (extra leaves keep
/// their old content); leftover entries are ignored.
///
/// This is how [`App::layout_from_panes`](crate::app::App) rebuilds the
/// authoritative layout tree from the live panes after a runtime change that
/// swapped a pane's content out-of-band (`:e`, `:E`, a browser opening a file),
/// so the persisted layout always mirrors what is on screen.
pub fn rebuild_leaves(node: &mut LayoutNode, leaves: &mut impl Iterator<Item = LayoutNode>) {
    match node {
        LayoutNode::Editor { .. } | LayoutNode::Process { .. } | LayoutNode::Panel { .. } => {
            if let Some(replacement) = leaves.next() {
                *node = replacement;
            }
        }
        LayoutNode::Row { children, .. } | LayoutNode::Column { children, .. } => {
            for child in children.iter_mut() {
                rebuild_leaves(child, leaves);
            }
        }
    }
}

fn center(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// Overlap length of two rects along the axis perpendicular to `dir`: their
/// shared vertical span for Left/Right, shared horizontal span for Up/Down.
/// Non-positive when they do not overlap on that axis.
fn perpendicular_overlap(a: Rect, b: Rect, dir: Direction) -> f32 {
    if dir.is_horizontal() {
        (a.y + a.h).min(b.y + b.h) - a.y.max(b.y)
    } else {
        (a.x + a.w).min(b.x + b.w) - a.x.max(b.x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn from_key_maps_hjkl_and_rejects_others() {
        assert_eq!(Direction::from_key('h'), Some(Direction::Left));
        assert_eq!(Direction::from_key('j'), Some(Direction::Down));
        assert_eq!(Direction::from_key('k'), Some(Direction::Up));
        assert_eq!(Direction::from_key('l'), Some(Direction::Right));
        assert_eq!(Direction::from_key('x'), None);
    }

    /// A simple two-column row: pane 0 left, pane 1 right.
    fn two_columns() -> Vec<Rect> {
        vec![rect(0.0, 0.0, 100.0, 100.0), rect(100.0, 0.0, 100.0, 100.0)]
    }

    #[test]
    fn moves_right_and_left_across_columns() {
        let r = two_columns();
        assert_eq!(neighbor(&r, 0, Direction::Right), Some(1));
        assert_eq!(neighbor(&r, 1, Direction::Left), Some(0));
    }

    #[test]
    fn no_neighbor_off_the_edge() {
        let r = two_columns();
        assert_eq!(neighbor(&r, 0, Direction::Left), None);
        assert_eq!(neighbor(&r, 1, Direction::Right), None);
        // No pane above or below in a single row.
        assert_eq!(neighbor(&r, 0, Direction::Up), None);
        assert_eq!(neighbor(&r, 0, Direction::Down), None);
    }

    #[test]
    fn moves_up_and_down_across_rows() {
        let r = vec![rect(0.0, 0.0, 100.0, 100.0), rect(0.0, 100.0, 100.0, 100.0)];
        assert_eq!(neighbor(&r, 0, Direction::Down), Some(1));
        assert_eq!(neighbor(&r, 1, Direction::Up), Some(0));
    }

    #[test]
    fn single_pane_has_no_neighbors() {
        let r = vec![rect(0.0, 0.0, 100.0, 100.0)];
        for dir in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            assert_eq!(neighbor(&r, 0, dir), None);
        }
    }

    #[test]
    fn out_of_range_focus_returns_none() {
        let r = two_columns();
        assert_eq!(neighbor(&r, 9, Direction::Left), None);
    }

    #[test]
    fn prefers_the_overlapping_neighbor_over_a_closer_offset_one() {
        // Focused pane is the tall left column. To its right sit two stacked
        // panes; only the top one overlaps the focused pane's vertical span at
        // the row that matters. Moving Right from a focus whose center aligns
        // with the top-right pane should land there, not the (equally close but
        // non-overlapping) bottom-right pane.
        //
        //   +--------+--------+
        //   |        |   1    |
        //   |   0    +--------+
        //   |        |   2    |
        //   +--------+--------+
        let r = vec![
            rect(0.0, 0.0, 100.0, 100.0),   // 0: focused, full-height left
            rect(100.0, 0.0, 100.0, 50.0),  // 1: top-right
            rect(100.0, 50.0, 100.0, 50.0), // 2: bottom-right
        ];
        // Both right panes overlap pane 0's span; the one whose center is
        // closest on the cross (vertical) axis to pane 0's center wins ties.
        // Pane 0's center y is 50; pane 1 center y = 25, pane 2 = 75 — equal
        // distance, so the first-found (pane 1) wins deterministically.
        assert_eq!(neighbor(&r, 0, Direction::Right), Some(1));
    }

    #[test]
    fn three_column_navigation_steps_one_pane_at_a_time() {
        let r = vec![
            rect(0.0, 0.0, 100.0, 100.0),
            rect(100.0, 0.0, 100.0, 100.0),
            rect(200.0, 0.0, 100.0, 100.0),
        ];
        assert_eq!(neighbor(&r, 0, Direction::Right), Some(1));
        assert_eq!(neighbor(&r, 1, Direction::Right), Some(2));
        assert_eq!(neighbor(&r, 2, Direction::Left), Some(1));
    }

    fn editor(name: &str) -> LayoutNode {
        LayoutNode::Editor {
            file: Some(name.into()),
            line_numbers: false,
            wrap: true,
        }
    }

    #[test]
    fn replace_leaf_splits_the_only_pane() {
        let mut tree = editor("a");
        let split = LayoutNode::Row {
            children: vec![
                editor("a"),
                LayoutNode::Editor {
                    file: None,
                    line_numbers: false,
                    wrap: true,
                },
            ],
            ratios: None,
        };
        assert!(replace_leaf(&mut tree, 0, split.clone()));
        assert_eq!(tree, split);
    }

    #[test]
    fn replace_leaf_targets_the_nth_leaf_in_dfs_order() {
        // row([ column([a, b]), c ]) — leaves in solver order are a, b, c.
        let mut tree = LayoutNode::Row {
            children: vec![
                LayoutNode::Column {
                    children: vec![editor("a"), editor("b")],
                    ratios: None,
                },
                editor("c"),
            ],
            ratios: None,
        };
        // Replace the second leaf (b) with a fresh editor leaf.
        assert!(replace_leaf(&mut tree, 1, editor("B")));
        let expected = LayoutNode::Row {
            children: vec![
                LayoutNode::Column {
                    children: vec![editor("a"), editor("B")],
                    ratios: None,
                },
                editor("c"),
            ],
            ratios: None,
        };
        assert_eq!(tree, expected);
    }

    fn process(arg: &str) -> LayoutNode {
        LayoutNode::Process {
            command: "browser".into(),
            args: vec![arg.into()],
        }
    }

    #[test]
    fn rebuild_leaves_swaps_content_preserving_structure_and_ratios() {
        // row([ column([a, b], [0.3, 0.7]), c ], [0.5, 0.5]) — leaves a, b, c.
        let mut tree = LayoutNode::Row {
            children: vec![
                LayoutNode::Column {
                    children: vec![editor("a"), editor("b")],
                    ratios: Some(vec![0.3, 0.7]),
                },
                editor("c"),
            ],
            ratios: Some(vec![0.5, 0.5]),
        };
        // The live panes now show: a browser, an editor on "B", and "c" still.
        let live = vec![process("/src"), editor("B"), editor("c")];
        rebuild_leaves(&mut tree, &mut live.into_iter());

        let expected = LayoutNode::Row {
            children: vec![
                LayoutNode::Column {
                    children: vec![process("/src"), editor("B")],
                    ratios: Some(vec![0.3, 0.7]),
                },
                editor("c"),
            ],
            ratios: Some(vec![0.5, 0.5]),
        };
        assert_eq!(tree, expected);
    }

    #[test]
    fn rebuild_leaves_stops_when_leaves_run_out() {
        let mut tree = LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: None,
        };
        let before = tree.clone();
        // Only one replacement available; the second leaf keeps its content.
        rebuild_leaves(&mut tree, &mut std::iter::once(editor("A")));
        let expected = LayoutNode::Row {
            children: vec![editor("A"), editor("b")],
            ratios: None,
        };
        assert_eq!(tree, expected);
        assert_ne!(tree, before);
    }

    #[test]
    fn replace_leaf_out_of_range_is_a_noop() {
        let mut tree = LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: None,
        };
        let before = tree.clone();
        assert!(!replace_leaf(&mut tree, 5, editor("x")));
        assert_eq!(tree, before);
    }

    #[test]
    fn remove_leaf_collapses_a_two_child_container() {
        // row([a, b]) minus b → just a.
        let mut tree = LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: None,
        };
        assert!(remove_leaf(&mut tree, 1));
        assert_eq!(tree, editor("a"));
    }

    #[test]
    fn remove_leaf_drops_the_matching_ratio_entry() {
        // row([a, b, c], [0.5, 0.3, 0.2]) minus b → row([a, c], [0.5, 0.2]).
        let mut tree = LayoutNode::Row {
            children: vec![editor("a"), editor("b"), editor("c")],
            ratios: Some(vec![0.5, 0.3, 0.2]),
        };
        assert!(remove_leaf(&mut tree, 1));
        let expected = LayoutNode::Row {
            children: vec![editor("a"), editor("c")],
            ratios: Some(vec![0.5, 0.2]),
        };
        assert_eq!(tree, expected);
    }

    #[test]
    fn remove_leaf_collapses_nested_containers() {
        // row([ column([a, b]), c ]) — removing b collapses the column into a.
        let mut tree = LayoutNode::Row {
            children: vec![
                LayoutNode::Column {
                    children: vec![editor("a"), editor("b")],
                    ratios: None,
                },
                editor("c"),
            ],
            ratios: None,
        };
        assert!(remove_leaf(&mut tree, 1));
        let expected = LayoutNode::Row {
            children: vec![editor("a"), editor("c")],
            ratios: None,
        };
        assert_eq!(tree, expected);
    }

    #[test]
    fn remove_leaf_counts_across_containers() {
        // row([ column([a, b]), c ]) — leaf 2 is c; the column is untouched.
        let mut tree = LayoutNode::Row {
            children: vec![
                LayoutNode::Column {
                    children: vec![editor("a"), editor("b")],
                    ratios: None,
                },
                editor("c"),
            ],
            ratios: None,
        };
        assert!(remove_leaf(&mut tree, 2));
        let expected = LayoutNode::Column {
            children: vec![editor("a"), editor("b")],
            ratios: None,
        };
        assert_eq!(tree, expected);
    }

    #[test]
    fn remove_leaf_refuses_the_last_leaf_and_out_of_range() {
        let mut only = editor("a");
        assert!(!remove_leaf(&mut only, 0));
        assert_eq!(only, editor("a"));

        let mut tree = LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: None,
        };
        let before = tree.clone();
        assert!(!remove_leaf(&mut tree, 5));
        assert_eq!(tree, before);
    }

    #[test]
    fn picks_the_vertically_overlapping_pane_when_crossing_a_grid() {
        // A 2x2 grid. Moving Down from the top-left lands on the bottom-left
        // (the column-aligned pane), never the bottom-right.
        //   0 | 1
        //   --+--
        //   2 | 3
        let r = vec![
            rect(0.0, 0.0, 100.0, 100.0),
            rect(100.0, 0.0, 100.0, 100.0),
            rect(0.0, 100.0, 100.0, 100.0),
            rect(100.0, 100.0, 100.0, 100.0),
        ];
        assert_eq!(neighbor(&r, 0, Direction::Down), Some(2));
        assert_eq!(neighbor(&r, 0, Direction::Right), Some(1));
        assert_eq!(neighbor(&r, 3, Direction::Up), Some(1));
        assert_eq!(neighbor(&r, 3, Direction::Left), Some(2));
    }
}
