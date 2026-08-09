//! Pane layout solver: walks a `LayoutNode` tree against a window rect and
//! produces a placed slot per leaf, in tree order.

use garden_render::Rect;
use garden_script::LayoutNode;

/// What a leaf slot holds, mirroring the leaf [`LayoutNode`] variants: a normal
/// editor (optionally pre-loaded with a file) or a GPP subprocess pane.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneContent {
    Editor {
        file: Option<String>,
        line_numbers: bool,
        wrap: bool,
    },
    Process {
        command: String,
        args: Vec<String>,
    },
    Panel {
        script: String,
        /// Optional explicit navigation allowlist (see
        /// [`LayoutNode::Panel`](garden_script::LayoutNode)). Empty = not
        /// declared, so the implicit script-directory default applies; non-empty
        /// narrows navigation to exactly these screen names.
        screens: Vec<String>,
    },
}

/// One leaf placed by the solver. `content` comes straight from the layout
/// script; leaves are emitted in depth-first tree order.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneSlot {
    pub rect: Rect,
    pub content: PaneContent,
}

/// Gap between sibling panes, logical pixels.
const GAP: f32 = 2.0;

/// Half-width of a divider's grab region, logical pixels — the GAP itself is too
/// thin to hit, so the draggable band extends this far to each side of it.
pub const DIVIDER_GRAB: f32 = 4.0;

/// A draggable boundary between two sibling panes of a `Row`/`Column`. Resizing
/// it shifts the split ratio between the two children it separates. Carries the
/// path to the owning split node and which boundary it is, so a drag maps back
/// to the exact ratios to adjust (see [`resize_divider`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    /// The grab region (the GAP widened by [`DIVIDER_GRAB`] each side).
    pub rect: Rect,
    /// `true` for a vertical divider (a `Row` split — drag left/right); `false`
    /// for a horizontal one (a `Column` split — drag up/down).
    pub vertical: bool,
    /// Child indices from the root to the owning `Row`/`Column` node.
    pub path: Vec<usize>,
    /// The boundary sits between child `before` and child `before + 1`.
    pub before: usize,
    /// The split axis's usable extent (total minus gaps), for converting a pixel
    /// drag into a ratio delta: `delta_ratio = delta_px / span`.
    pub span: f32,
}

pub fn solve(node: &LayoutNode, rect: Rect) -> Vec<PaneSlot> {
    let mut out = Vec::new();
    walk(node, rect, &mut out);
    out
}

/// Every draggable divider in `node` placed against `rect`, in tree order.
pub fn solve_dividers(node: &LayoutNode, rect: Rect) -> Vec<Divider> {
    let mut out = Vec::new();
    walk_dividers(node, rect, &mut Vec::new(), &mut out);
    out
}

fn walk_dividers(node: &LayoutNode, rect: Rect, path: &mut Vec<usize>, out: &mut Vec<Divider>) {
    let (children, ratios, horizontal) = match node {
        LayoutNode::Row { children, ratios } => (children, ratios, true),
        LayoutNode::Column { children, ratios } => (children, ratios, false),
        _ => return,
    };
    if children.is_empty() {
        return;
    }
    let ratios = normalized_ratios(children.len(), ratios);
    let total = if horizontal { rect.w } else { rect.h };
    let usable = (total - GAP * (children.len() as f32 - 1.0)).max(0.0);

    let mut offset = if horizontal { rect.x } else { rect.y };
    for (i, (child, ratio)) in children.iter().zip(ratios).enumerate() {
        let size = usable * ratio;
        let child_rect = if horizontal {
            Rect {
                x: offset,
                y: rect.y,
                w: size,
                h: rect.h,
            }
        } else {
            Rect {
                x: rect.x,
                y: offset,
                w: rect.w,
                h: size,
            }
        };
        path.push(i);
        walk_dividers(child, child_rect, path, out);
        path.pop();

        // Emit a divider in the GAP after every child but the last.
        if i + 1 < children.len() {
            let gap_center = offset + size + GAP / 2.0;
            let rect = if horizontal {
                Rect {
                    x: gap_center - DIVIDER_GRAB,
                    y: rect.y,
                    w: DIVIDER_GRAB * 2.0,
                    h: rect.h,
                }
            } else {
                Rect {
                    x: rect.x,
                    y: gap_center - DIVIDER_GRAB,
                    w: rect.w,
                    h: DIVIDER_GRAB * 2.0,
                }
            };
            out.push(Divider {
                rect,
                vertical: horizontal,
                path: path.clone(),
                before: i,
                span: usable,
            });
        }
        offset += size + GAP;
    }
}

/// Shift the boundary between children `before` and `before + 1` of the split
/// node at `path` by `delta_frac` (a fraction of the split axis, positive =
/// grow `before`). Each side is kept at least `MIN_FRAC` so a pane can't be
/// dragged to nothing. No-op if the path doesn't resolve to a split with that
/// boundary. Returns whether anything changed.
pub fn resize_divider(
    node: &mut LayoutNode,
    path: &[usize],
    before: usize,
    delta_frac: f32,
) -> bool {
    /// Smallest a pane may shrink to, as a fraction of its split axis.
    const MIN_FRAC: f32 = 0.05;

    let Some(target) = node_at_mut(node, path) else {
        return false;
    };
    let (children, ratios) = match target {
        LayoutNode::Row { children, ratios } | LayoutNode::Column { children, ratios } => {
            (children, ratios)
        }
        _ => return false,
    };
    let n = children.len();
    if before + 1 >= n {
        return false;
    }
    let mut r = normalized_ratios(n, ratios);
    let pair = r[before] + r[before + 1];
    // Degenerate: the two adjacent panes already share less than two minimums
    // between them (e.g. ratios like [0.95, 0.03, 0.02]). `pair - MIN_FRAC`
    // would then be below `MIN_FRAC`, inverting the clamp range so `f32::clamp`
    // panics (min > max). Nothing sensible to resize here, so no-op.
    if pair < 2.0 * MIN_FRAC {
        return false;
    }
    let mut a = (r[before] + delta_frac).clamp(MIN_FRAC, pair - MIN_FRAC);
    if !a.is_finite() {
        a = r[before];
    }
    r[before] = a;
    r[before + 1] = pair - a;
    *ratios = Some(r);
    true
}

/// The subtree at `path` (child indices from `node`), or `None` if the path
/// runs off a leaf or out of bounds.
fn node_at_mut<'a>(node: &'a mut LayoutNode, path: &[usize]) -> Option<&'a mut LayoutNode> {
    let mut cur = node;
    for &i in path {
        let children = match cur {
            LayoutNode::Row { children, .. } | LayoutNode::Column { children, .. } => children,
            _ => return None,
        };
        cur = children.get_mut(i)?;
    }
    Some(cur)
}

fn walk(node: &LayoutNode, rect: Rect, out: &mut Vec<PaneSlot>) {
    match node {
        LayoutNode::Editor {
            file,
            line_numbers,
            wrap,
        } => out.push(PaneSlot {
            rect,
            content: PaneContent::Editor {
                file: file.clone(),
                line_numbers: *line_numbers,
                wrap: *wrap,
            },
        }),
        LayoutNode::Process { command, args } => out.push(PaneSlot {
            rect,
            content: PaneContent::Process {
                command: command.clone(),
                args: args.clone(),
            },
        }),
        LayoutNode::Panel { script, screens } => out.push(PaneSlot {
            rect,
            content: PaneContent::Panel {
                script: script.clone(),
                screens: screens.clone(),
            },
        }),
        LayoutNode::Row { children, ratios } => {
            split(children, ratios, rect, true, out);
        }
        LayoutNode::Column { children, ratios } => {
            split(children, ratios, rect, false, out);
        }
    }
}

fn split(
    children: &[LayoutNode],
    ratios: &Option<Vec<f32>>,
    rect: Rect,
    horizontal: bool,
    out: &mut Vec<PaneSlot>,
) {
    if children.is_empty() {
        return;
    }
    let ratios = normalized_ratios(children.len(), ratios);
    let total = if horizontal { rect.w } else { rect.h };
    let usable = (total - GAP * (children.len() as f32 - 1.0)).max(0.0);

    let mut offset = if horizontal { rect.x } else { rect.y };
    for (child, ratio) in children.iter().zip(ratios) {
        let size = usable * ratio;
        let child_rect = if horizontal {
            Rect {
                x: offset,
                y: rect.y,
                w: size,
                h: rect.h,
            }
        } else {
            Rect {
                x: rect.x,
                y: offset,
                w: rect.w,
                h: size,
            }
        };
        walk(child, child_rect, out);
        offset += size + GAP;
    }
}

/// Returns one ratio per child, summing to 1.0. Falls back to an equal split
/// when ratios are missing, the wrong length, or not all positive.
fn normalized_ratios(n: usize, ratios: &Option<Vec<f32>>) -> Vec<f32> {
    if let Some(r) = ratios {
        if r.len() == n && r.iter().all(|v| *v > 0.0) {
            let sum: f32 = r.iter().sum();
            return r.iter().map(|v| v / sum).collect();
        }
    }
    vec![1.0 / n as f32; n]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(file: &str) -> LayoutNode {
        LayoutNode::Editor {
            file: Some(file.to_string()),
            line_numbers: false,
            wrap: true,
        }
    }

    /// The file of an `Editor` slot (panics on a `Process` slot).
    fn slot_file(slot: &PaneSlot) -> Option<&str> {
        match &slot.content {
            PaneContent::Editor { file, .. } => file.as_deref(),
            PaneContent::Process { .. } | PaneContent::Panel { .. } => {
                panic!("expected an Editor slot")
            }
        }
    }

    const WIN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1000.0,
        h: 800.0,
    };

    #[test]
    fn single_editor_fills_rect() {
        let slots = solve(&editor("a"), WIN);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].rect, WIN);
        assert_eq!(slot_file(&slots[0]), Some("a"));
    }

    #[test]
    fn a_leaf_has_no_dividers() {
        assert!(solve_dividers(&editor("a"), WIN).is_empty());
    }

    #[test]
    fn a_row_has_one_vertical_divider_in_the_gap() {
        let node = LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: Some(vec![0.75, 0.25]),
        };
        let divs = solve_dividers(&node, WIN);
        assert_eq!(divs.len(), 1);
        let d = &divs[0];
        assert!(d.vertical, "a Row split gives a vertical (drag-x) divider");
        assert_eq!(d.before, 0);
        assert_eq!(d.path, Vec::<usize>::new()); // owned by the root split
                                                 // Centered on the gap after child 0 (~750px), spanning full height.
        let usable = 1000.0 - GAP;
        let center = usable * 0.75 + GAP / 2.0;
        assert!(
            (d.rect.x + DIVIDER_GRAB - center).abs() < 0.5,
            "centered on the gap"
        );
        assert_eq!(d.rect.h, WIN.h);
        assert!((d.span - usable).abs() < 0.01);
    }

    #[test]
    fn resize_divider_shifts_the_ratio_between_two_children() {
        let mut node = LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: Some(vec![0.5, 0.5]),
        };
        // Drag the boundary 20% toward child b (grow a).
        assert!(resize_divider(&mut node, &[], 0, 0.2));
        let LayoutNode::Row { ratios, .. } = &node else {
            panic!()
        };
        let r = ratios.as_ref().unwrap();
        assert!((r[0] - 0.7).abs() < 1e-4 && (r[1] - 0.3).abs() < 1e-4);

        // A huge drag is clamped so neither pane vanishes (>= 5%).
        assert!(resize_divider(&mut node, &[], 0, 100.0));
        let LayoutNode::Row { ratios, .. } = &node else {
            panic!()
        };
        let r = ratios.as_ref().unwrap();
        assert!(r[1] >= 0.05 - 1e-4 && r[0] <= 0.95 + 1e-4);
    }

    #[test]
    fn nested_split_dividers_carry_their_path() {
        // Row [ editor, Column[ editor, editor ] ]
        let node = LayoutNode::Row {
            children: vec![
                editor("a"),
                LayoutNode::Column {
                    children: vec![editor("b"), editor("c")],
                    ratios: None,
                },
            ],
            ratios: None,
        };
        let divs = solve_dividers(&node, WIN);
        assert_eq!(divs.len(), 2);
        // The inner column's divider is horizontal and its path descends into
        // the root's child 1.
        let inner = divs.iter().find(|d| !d.vertical).unwrap();
        assert_eq!(inner.path, vec![1]);
        assert_eq!(inner.before, 0);
        // Resizing it only touches the nested column's ratios.
        let mut n = node.clone();
        assert!(resize_divider(&mut n, &inner.path, inner.before, 0.1));
    }

    #[test]
    fn row_splits_horizontally_with_ratios() {
        let node = LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: Some(vec![0.75, 0.25]),
        };
        let slots = solve(&node, WIN);
        assert_eq!(slots.len(), 2);
        let usable = 1000.0 - GAP;
        assert!((slots[0].rect.w - usable * 0.75).abs() < 0.01);
        assert!((slots[1].rect.w - usable * 0.25).abs() < 0.01);
        assert!(slots[1].rect.x > slots[0].rect.x + slots[0].rect.w);
        assert_eq!(slots[0].rect.h, 800.0);
    }

    #[test]
    fn column_splits_vertically_equal_by_default() {
        let node = LayoutNode::Column {
            children: vec![editor("a"), editor("b"), editor("c")],
            ratios: None,
        };
        let slots = solve(&node, WIN);
        assert_eq!(slots.len(), 3);
        let usable = 800.0 - 2.0 * GAP;
        for slot in &slots {
            assert!((slot.rect.h - usable / 3.0).abs() < 0.01);
            assert_eq!(slot.rect.w, 1000.0);
        }
    }

    #[test]
    fn nested_layout_emits_leaves_in_tree_order() {
        let node = LayoutNode::Row {
            children: vec![
                LayoutNode::Column {
                    children: vec![editor("a"), editor("b")],
                    ratios: None,
                },
                editor("c"),
            ],
            ratios: Some(vec![0.5, 0.5]),
        };
        let slots = solve(&node, WIN);
        let files: Vec<_> = slots.iter().map(|s| slot_file(s).unwrap()).collect();
        assert_eq!(files, ["a", "b", "c"]);
        assert!(slots[0].rect.y < slots[1].rect.y);
        assert!(slots[2].rect.x > slots[0].rect.x);
    }

    #[test]
    fn bad_ratios_fall_back_to_equal() {
        let node = LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: Some(vec![0.3]), // wrong length
        };
        let slots = solve(&node, WIN);
        assert!((slots[0].rect.w - slots[1].rect.w).abs() < 0.01);
    }

    #[test]
    fn process_leaf_becomes_a_process_slot() {
        let node = LayoutNode::Process {
            command: "directory-browser".to_string(),
            args: vec![".".to_string()],
        };
        let slots = solve(&node, WIN);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].rect, WIN);
        assert_eq!(
            slots[0].content,
            PaneContent::Process {
                command: "directory-browser".to_string(),
                args: vec![".".to_string()],
            }
        );
    }

    #[test]
    fn panel_leaf_becomes_a_panel_slot() {
        let node = LayoutNode::Panel {
            script: "sketch.ptl".to_string(),
            screens: Vec::new(),
        };
        let slots = solve(&node, WIN);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].rect, WIN);
        assert_eq!(
            slots[0].content,
            PaneContent::Panel {
                script: "sketch.ptl".to_string(),
                screens: Vec::new(),
            }
        );
    }

    #[test]
    fn panel_leaf_threads_its_screens_allowlist() {
        let node = LayoutNode::Panel {
            script: "sketch.ptl".to_string(),
            screens: vec!["a.ptl".to_string(), "b.ptl".to_string()],
        };
        let slots = solve(&node, WIN);
        assert_eq!(
            slots[0].content,
            PaneContent::Panel {
                script: "sketch.ptl".to_string(),
                screens: vec!["a.ptl".to_string(), "b.ptl".to_string()],
            }
        );
    }

    #[test]
    fn ratios_are_normalized() {
        let node = LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: Some(vec![3.0, 1.0]),
        };
        let slots = solve(&node, WIN);
        let usable = 1000.0 - GAP;
        assert!((slots[0].rect.w - usable * 0.75).abs() < 0.01);
    }
}
