//! Rasterizes a logical-pixel [`Scene`] onto a terminal character grid.
//!
//! The terminal frontend reports a virtual cell size of [`CELL`] logical
//! pixels, so every coordinate the app core computes is a whole multiple of a
//! cell away from its base offsets and rounds consistently: text, selection
//! highlights, and the caret all land in the same columns. Sub-cell chrome
//! (1px borders, the 2px insert caret, 6px scrollbars) snaps to the single
//! cell it overlaps.
//!
//! Paint order matches the GPU renderer: all quads first (background colors),
//! then all text runs (glyphs + foreground colors) on top.

use garden_render::{Color, Primitive, Rect, Scene};

/// Virtual cell size in logical pixels: chosen with the ~1:2 aspect ratio of
/// a terminal cell so the app's pixel-space chrome (margins, status bar)
/// rounds onto sensible rows and columns.
pub const CELL: (f32, f32) = (8.0, 16.0);

/// One terminal cell: a glyph plus foreground/background colors (sRGB).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
}

/// A `cols` × `rows` grid of [`Cell`]s, row-major.
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
}

impl Grid {
    pub fn get(&self, col: usize, row: usize) -> &Cell {
        &self.cells[row * self.cols + col]
    }

    fn get_mut(&mut self, col: usize, row: usize) -> &mut Cell {
        &mut self.cells[row * self.cols + col]
    }

    /// One row's characters, e.g. for tests and the text `/screenshot`.
    pub fn row_text(&self, row: usize) -> String {
        (0..self.cols).map(|c| self.get(c, row).ch).collect()
    }

    /// The whole grid as plain text, one line per row.
    pub fn to_text(&self) -> String {
        (0..self.rows)
            .map(|r| self.row_text(r).trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Map a pixel span `[start, start+len)` to a cell range, rounding edges to
/// the nearest cell boundary. A span too thin to round onto any cell (a 1px
/// border, the insert caret) snaps to the single cell containing its center.
fn span(start: f32, len: f32, cell: f32, max: usize) -> (usize, usize) {
    let mut a = (start / cell).round() as isize;
    let mut b = ((start + len) / cell).round() as isize;
    if b <= a && len > 0.0 {
        a = ((start + len * 0.5) / cell).floor() as isize;
        b = a + 1;
    }
    let max = max as isize;
    (a.clamp(0, max) as usize, b.clamp(0, max) as usize)
}

/// Is `center` inside the half-open span `[start, start + len)`?
fn inside(center: f32, start: f32, len: f32) -> bool {
    center >= start && center < start + len
}

/// Cell range covered by `rect`: `(col0, col1, row0, row1)`, half-open.
fn cell_rect(rect: &Rect, cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    let (c0, c1) = span(rect.x, rect.w, CELL.0, cols);
    let (r0, r1) = span(rect.y, rect.h, CELL.1, rows);
    (c0, c1, r0, r1)
}

/// Render `scene` onto a `cols` × `rows` character grid.
pub fn rasterize(scene: &Scene, cols: usize, rows: usize) -> Grid {
    let blank = Cell {
        ch: ' ',
        fg: scene.bg,
        bg: scene.bg,
    };
    let mut grid = Grid {
        cols,
        rows,
        cells: vec![blank; cols * rows],
    };

    // Pass 1: quads set background colors (later quads overwrite earlier).
    for prim in &scene.primitives {
        let Primitive::Quad { rect, color } = prim else {
            continue;
        };
        let (c0, c1, r0, r1) = cell_rect(rect, cols, rows);
        for row in r0..r1 {
            for col in c0..c1 {
                grid.get_mut(col, row).bg = *color;
            }
        }
    }

    // Pass 2: text runs set glyphs and foreground colors, clipped.
    for prim in &scene.primitives {
        let Primitive::Text {
            pos,
            text,
            color,
            clip,
            ..
        } = prim
        else {
            continue;
        };
        let row = (pos.1 / CELL.1).round() as isize;
        // Clip by cell *center*, not by the rounded cell span a quad uses: a
        // cell is atomic here, so a row the GPU renderer would cut in half is
        // kept only when more than half of it is inside the clip. Rounding the
        // clip outward instead (what `cell_rect` does) drew a straddling row
        // whole, which is how text escaped a scrolling viewport in the TUI.
        if row < 0 || !inside(row as f32 * CELL.1 + CELL.1 * 0.5, clip.y, clip.h) {
            continue;
        }
        let row = row as usize;
        if row >= rows {
            continue;
        }
        let start = (pos.0 / CELL.0).round() as isize;
        for (i, ch) in text.chars().enumerate() {
            let col = start + i as isize;
            if col < 0
                || col >= cols as isize
                || !inside(col as f32 * CELL.0 + CELL.0 * 0.5, clip.x, clip.w)
            {
                continue;
            }
            let cell = grid.get_mut(col as usize, row);
            cell.ch = ch;
            cell.fg = *color;
        }
    }

    grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use garden_render::TextStyle;

    const BG: Color = Color::rgb(0.1, 0.1, 0.1);
    const RED: Color = Color::rgb(1.0, 0.0, 0.0);
    const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);

    fn scene(primitives: Vec<Primitive>) -> Scene {
        Scene { bg: BG, primitives }
    }

    #[test]
    fn empty_scene_fills_with_background() {
        let grid = rasterize(&scene(vec![]), 4, 2);
        for r in 0..2 {
            for c in 0..4 {
                assert_eq!(
                    grid.get(c, r),
                    &Cell {
                        ch: ' ',
                        fg: BG,
                        bg: BG
                    }
                );
            }
        }
    }

    #[test]
    fn quad_fills_cell_aligned_rect() {
        let grid = rasterize(
            &scene(vec![Primitive::Quad {
                rect: Rect::new(8.0, 16.0, 16.0, 16.0),
                color: RED,
            }]),
            4,
            3,
        );
        assert_eq!(grid.get(0, 1).bg, BG);
        assert_eq!(grid.get(1, 1).bg, RED);
        assert_eq!(grid.get(2, 1).bg, RED);
        assert_eq!(grid.get(3, 1).bg, BG);
        assert_eq!(grid.get(1, 0).bg, BG);
        assert_eq!(grid.get(1, 2).bg, BG);
    }

    #[test]
    fn thin_quad_snaps_to_one_cell() {
        // A 1px border line at x=4 rounds to an empty span; it must still
        // paint the column it overlaps.
        let grid = rasterize(
            &scene(vec![Primitive::Quad {
                rect: Rect::new(4.0, 0.0, 1.0, 32.0),
                color: RED,
            }]),
            4,
            2,
        );
        assert_eq!(grid.get(0, 0).bg, RED);
        assert_eq!(grid.get(0, 1).bg, RED);
        assert_eq!(grid.get(1, 0).bg, BG);
    }

    #[test]
    fn insert_caret_quad_lands_on_its_cell() {
        // The 2px bar caret at the left edge of column 3.
        let grid = rasterize(
            &scene(vec![Primitive::Quad {
                rect: Rect::new(24.0, 16.0, 2.0, 16.0),
                color: RED,
            }]),
            6,
            3,
        );
        assert_eq!(grid.get(3, 1).bg, RED);
        assert_eq!(grid.get(2, 1).bg, BG);
        assert_eq!(grid.get(4, 1).bg, BG);
    }

    #[test]
    fn text_lands_on_rounded_cells_and_keeps_quad_bg() {
        let clip = Rect::new(0.0, 0.0, 64.0, 32.0);
        let grid = rasterize(
            &scene(vec![
                Primitive::Quad {
                    rect: Rect::new(0.0, 0.0, 64.0, 32.0),
                    color: RED,
                },
                // Slightly off-grid position (PAD offsets) rounds to col 1, row 1.
                Primitive::Text {
                    pos: (10.0, 14.0),
                    text: "hi".to_string(),
                    color: WHITE,
                    clip,
                    size: garden_render::FONT_SIZE,
                    style: TextStyle::default(),
                },
            ]),
            8,
            2,
        );
        assert_eq!(grid.get(1, 1).ch, 'h');
        assert_eq!(grid.get(2, 1).ch, 'i');
        assert_eq!(grid.get(1, 1).fg, WHITE);
        assert_eq!(grid.get(1, 1).bg, RED); // text keeps the quad's background
        assert_eq!(grid.get(3, 1).ch, ' ');
    }

    #[test]
    fn text_is_clipped_to_its_clip_rect() {
        let grid = rasterize(
            &scene(vec![Primitive::Text {
                pos: (0.0, 0.0),
                text: "abcdef".to_string(),
                color: WHITE,
                // Clip allows columns 0..3 only.
                clip: Rect::new(0.0, 0.0, 24.0, 16.0),
                size: garden_render::FONT_SIZE,
                style: TextStyle::default(),
            }]),
            8,
            1,
        );
        assert_eq!(grid.row_text(0), "abc     ");
    }

    /// A cell is atomic in the terminal, so a row the GPU renderer cuts in half
    /// is a judgement call — it is kept only when its center is inside the clip.
    /// The row above the boundary stays whole; the one past it goes.
    #[test]
    fn text_row_straddling_the_clip_bottom_is_cut_by_cell_center() {
        // Clip bottom at y = 40: row 1 (16..32) is fully inside, row 2 (32..48)
        // straddles it with its center exactly on the edge.
        let clip = Rect::new(0.0, 0.0, 64.0, 40.0);
        let text = |y: f32, s: &str| Primitive::Text {
            pos: (0.0, y),
            text: s.to_string(),
            color: WHITE,
            clip,
            size: garden_render::FONT_SIZE,
            style: TextStyle::default(),
        };
        let grid = rasterize(
            &scene(vec![
                text(16.0, "in"),
                text(32.0, "half"),
                text(48.0, "out"),
            ]),
            8,
            4,
        );
        assert_eq!(grid.row_text(1), "in      ");
        assert_eq!(grid.row_text(2), "        ");
        assert_eq!(grid.row_text(3), "        ");
    }

    /// The same rule horizontally: a column whose center is past the clip's
    /// right edge is dropped rather than rounded back in.
    #[test]
    fn text_column_straddling_the_clip_right_edge_is_cut() {
        let grid = rasterize(
            &scene(vec![Primitive::Text {
                pos: (0.0, 0.0),
                text: "abcdef".to_string(),
                color: WHITE,
                // Right edge mid-way through column 3 (24..32).
                clip: Rect::new(0.0, 0.0, 28.0, 16.0),
                size: garden_render::FONT_SIZE,
                style: TextStyle::default(),
            }]),
            8,
            1,
        );
        assert_eq!(grid.row_text(0), "abc     ");
    }

    #[test]
    fn text_outside_grid_is_dropped() {
        let big = Rect::new(-100.0, -100.0, 1000.0, 1000.0);
        let grid = rasterize(
            &scene(vec![
                Primitive::Text {
                    pos: (-16.0, 0.0),
                    text: "xy".to_string(),
                    color: WHITE,
                    clip: big,
                    size: garden_render::FONT_SIZE,
                    style: TextStyle::default(),
                },
                Primitive::Text {
                    pos: (0.0, 64.0),
                    text: "below".to_string(),
                    color: WHITE,
                    clip: big,
                    size: garden_render::FONT_SIZE,
                    style: TextStyle::default(),
                },
            ]),
            4,
            2,
        );
        // Only the in-bounds tail of the first run lands; clamping never panics.
        assert_eq!(grid.row_text(0), "    ");
        assert_eq!(grid.row_text(1), "    ");
    }

    #[test]
    fn later_quads_overwrite_earlier() {
        let grid = rasterize(
            &scene(vec![
                Primitive::Quad {
                    rect: Rect::new(0.0, 0.0, 32.0, 16.0),
                    color: RED,
                },
                Primitive::Quad {
                    rect: Rect::new(8.0, 0.0, 8.0, 16.0),
                    color: WHITE,
                },
            ]),
            4,
            1,
        );
        assert_eq!(grid.get(0, 0).bg, RED);
        assert_eq!(grid.get(1, 0).bg, WHITE);
        assert_eq!(grid.get(2, 0).bg, RED);
    }

    #[test]
    fn status_bar_maps_to_the_last_row() {
        // The core's status bar: height cell_h + 8 at y = h - (cell_h + 8),
        // with its text at y + 4. On a 30-row grid both must land on row 29.
        let rows = 30usize;
        let h = rows as f32 * CELL.1;
        let bar = Rect::new(0.0, h - (CELL.1 + 8.0), 80.0 * CELL.0, CELL.1 + 8.0);
        let grid = rasterize(
            &scene(vec![
                Primitive::Quad {
                    rect: bar,
                    color: RED,
                },
                Primitive::Text {
                    pos: (8.0, bar.y + 4.0),
                    text: "NORMAL".to_string(),
                    color: WHITE,
                    clip: bar,
                    size: garden_render::FONT_SIZE,
                    style: TextStyle::default(),
                },
            ]),
            80,
            rows,
        );
        assert_eq!(grid.get(0, 28).bg, BG); // row above the bar untouched
        assert_eq!(grid.get(0, 29).bg, RED);
        assert_eq!(&grid.row_text(29)[1..7], "NORMAL");
    }
}
