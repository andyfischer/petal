//! The PPU: the console's video state, and the rasterizer that turns it into
//! pixels.
//!
//! Shape and vocabulary follow the NES (see `docs/design.md`): a fixed master
//! palette, eight 4-color sub-palettes sharing a backdrop, 8x8 2bpp tiles in
//! one pattern table, a wrapping tile map with per-scanline horizontal scroll,
//! and a per-frame sprite list. It is *not* an emulator — there are no
//! registers, no timing, and no VRAM bus. It is a plain struct that a Petal
//! cart rewrites every frame.
//!
//! Two lifetimes matter, and they are why the API splits the way it does:
//!
//! - **Persistent** state (palettes, tiles, map, map size) is written
//!   idempotently and survives frames and hot reloads, so a cart that redefines
//!   its whole tileset at the top of every frame costs nothing and a reload
//!   mid-game repaints correctly.
//! - **Per-frame** state (sprites, per-scanline scroll overrides) is cleared by
//!   [`Ppu::begin_frame`] before the script runs and rebuilt by it. Nothing a
//!   cart forgets to push can linger.
//!
//! `Ppu` is `Clone` on purpose: a speculative frame (`--screenshot`'s final
//! capture, the agent's `screenshot` command) applies its video commands to a
//! clone so the live console state is not disturbed by a frame that is going
//! to be thrown away.

pub mod palette;

/// Visible resolution. Everything here is in these pixels; scaling to the
/// window is the host's problem, not the PPU's.
pub const SCREEN_W: usize = 256;
pub const SCREEN_H: usize = 240;

/// Bytes in a rendered frame (RGB8, no padding).
pub const FRAME_BYTES: usize = SCREEN_W * SCREEN_H * 3;

pub const TILE_W: usize = 8;
pub const TILE_H: usize = 8;
/// Pixels in one tile — the length of a decoded tile's index array.
pub const TILE_PIXELS: usize = TILE_W * TILE_H;

/// Pattern-table capacity.
pub const MAX_TILES: usize = 512;
/// Sub-palette count: 0-3 background, 4-7 sprite.
pub const PALETTE_COUNT: usize = 8;
/// Tile-map capacity, in cells.
pub const MAX_MAP_W: usize = 64;
pub const MAX_MAP_H: usize = 60;
/// Sprites accepted per frame; pushes past this are dropped silently.
pub const MAX_SPRITES: usize = 64;
/// Sprites drawn per scanline when the drop-out limit is enabled.
pub const SPRITES_PER_SCANLINE: usize = 8;

/// Sprite flag bits, as the `sprite(...)` native's 5th argument.
pub const FLIP_X: u8 = 1;
pub const FLIP_Y: u8 = 2;
pub const BEHIND_BG: u8 = 4;

/// One entry of the per-frame sprite list (the NES's OAM).
///
/// Coordinates are signed and unclamped so a sprite can walk off any edge;
/// clipping is the rasterizer's job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sprite {
    pub x: i32,
    pub y: i32,
    pub tile: u16,
    /// Sub-palette index. Sprites conventionally use 4-7, but any of 0-7 is
    /// accepted — the restriction buys nothing here.
    pub palette: u8,
    pub flags: u8,
}

impl Sprite {
    pub fn flip_x(&self) -> bool {
        self.flags & FLIP_X != 0
    }
    pub fn flip_y(&self) -> bool {
        self.flags & FLIP_Y != 0
    }
    pub fn behind_bg(&self) -> bool {
        self.flags & BEHIND_BG != 0
    }
}

/// One background map cell: which tile, and which sub-palette to color it with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapCell {
    pub tile: u16,
    pub palette: u8,
}

/// A decoded 8x8 tile: one 2-bit color index (0-3) per pixel, row-major.
/// Index 0 is transparent for sprites and the backdrop for background tiles.
pub type Tile = [u8; TILE_PIXELS];

#[derive(Clone)]
pub struct Ppu {
    /// Eight sub-palettes of four master-palette indices. Entry 0 of each is
    /// ignored at render time — [`Ppu::backdrop`] is the shared color 0.
    pub palettes: [[u8; 4]; PALETTE_COUNT],
    /// The universal color 0, shared by every sub-palette.
    pub backdrop: u8,
    /// The pattern table, always [`MAX_TILES`] long. Undefined tiles are all
    /// zeros, i.e. fully transparent / backdrop.
    pub tiles: Vec<Tile>,
    /// Hash of the *source* form each tile was last defined from, when it was
    /// defined through [`Ppu::define_tile_hashed`]. A cart redefines its whole
    /// tileset at the top of every frame, so the common case is 500 writes a
    /// frame of data that has not changed; comparing one `u64` lets the caller
    /// skip both the decode and the store. `None` means "unknown source", which
    /// never matches and so always re-decodes.
    tile_hash: Vec<Option<u64>>,
    /// Row-major map cells, `map_w * map_h` of them.
    pub map: Vec<MapCell>,
    pub map_w: usize,
    pub map_h: usize,
    /// Frame-wide scroll, in pixels; wraps around the map.
    pub scroll_x: i32,
    pub scroll_y: i32,
    /// Per-scanline horizontal scroll overrides (status-bar splits, parallax).
    /// `None` means "use `scroll_x`". Cleared every frame.
    pub scroll_at: Vec<Option<i32>>,
    /// This frame's sprite list, in push order (earlier = higher priority).
    pub sprites: Vec<Sprite>,
    /// Whether to emulate the 8-sprites-per-scanline drop-out. Off by default:
    /// it is an authenticity opt-in, not a constraint worth inflicting by
    /// surprise.
    pub sprite_limit: bool,
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    pub fn new() -> Self {
        Self {
            palettes: [[0; 4]; PALETTE_COUNT],
            backdrop: 0x0F,
            tiles: vec![[0u8; TILE_PIXELS]; MAX_TILES],
            tile_hash: vec![None; MAX_TILES],
            map: vec![MapCell::default(); 32 * 30],
            map_w: 32,
            map_h: 30,
            scroll_x: 0,
            scroll_y: 0,
            scroll_at: vec![None; SCREEN_H],
            sprites: Vec::with_capacity(MAX_SPRITES),
            sprite_limit: false,
        }
    }

    // ── Persistent state ──────────────────────────────────────────────────
    // All of these are idempotent and cheap to re-issue every frame.

    pub fn set_palette(&mut self, index: usize, c0: u8, c1: u8, c2: u8, c3: u8) {
        if index < PALETTE_COUNT {
            self.palettes[index] = [c0, c1, c2, c3];
        }
    }

    pub fn set_backdrop(&mut self, c: u8) {
        self.backdrop = c;
    }

    /// Install a decoded tile. Decoding the cart's row strings / packed ints
    /// into color indices happens in the natives layer, so the PPU never sees
    /// script-shaped data.
    pub fn define_tile(&mut self, index: usize, pixels: &Tile) {
        if index < MAX_TILES {
            self.tiles[index] = *pixels;
            self.tile_hash[index] = None;
        }
    }

    /// Install a tile only if `hash` differs from the hash the same slot was
    /// last written with, decoding it lazily via `decode` when it does.
    ///
    /// This is the path the `define_tile` native takes. Petal re-runs the cart
    /// top to bottom every frame, so the artwork is re-declared 60 times a
    /// second; hashing the cart's own row data (cheap, and already in hand)
    /// turns all but the first of those into a `u64` compare. Returns whether
    /// the tile was actually rewritten, which the tests use to prove the skip.
    pub fn define_tile_hashed(
        &mut self,
        index: usize,
        hash: u64,
        decode: impl FnOnce() -> Tile,
    ) -> bool {
        if index >= MAX_TILES {
            return false;
        }
        if self.tile_hash[index] == Some(hash) {
            return false;
        }
        self.tiles[index] = decode();
        self.tile_hash[index] = Some(hash);
        true
    }

    /// Resize the map, clamped to [`MAX_MAP_W`]x[`MAX_MAP_H`]. A no-op when the
    /// size is unchanged, so a cart may call it every frame; a real change
    /// clears the map (there is no meaningful way to reflow cells).
    pub fn set_map_size(&mut self, w: usize, h: usize) {
        let w = w.clamp(1, MAX_MAP_W);
        let h = h.clamp(1, MAX_MAP_H);
        if w == self.map_w && h == self.map_h {
            return;
        }
        self.map_w = w;
        self.map_h = h;
        self.map = vec![MapCell::default(); w * h];
    }

    pub fn set_tile(&mut self, x_cell: i32, y_cell: i32, tile: u16, palette: u8) {
        if let Some(i) = self.cell_index(x_cell, y_cell) {
            self.map[i] = MapCell { tile, palette };
        }
    }

    pub fn get_tile(&self, x_cell: i32, y_cell: i32) -> u16 {
        self.cell_index(x_cell, y_cell)
            .map(|i| self.map[i].tile)
            .unwrap_or(0)
    }

    pub fn get_cell(&self, x_cell: i32, y_cell: i32) -> MapCell {
        self.cell_index(x_cell, y_cell)
            .map(|i| self.map[i])
            .unwrap_or_default()
    }

    pub fn fill_map(&mut self, tile: u16, palette: u8) {
        let cell = MapCell { tile, palette };
        self.map.fill(cell);
    }

    /// Index of a map cell, or `None` when out of range. Out-of-range writes
    /// are dropped rather than wrapped: a cart that walks off the map is
    /// buggy, and silently aliasing to the far edge hides it.
    /// Copy just the map (size and cells) out of another PPU.
    ///
    /// The `get_tile` native needs to answer *during* the cart's run, before
    /// the host has drained anything, so the natives layer keeps a map mirror
    /// and resyncs it from the real console here once a frame. Nothing else
    /// about the PPU is copied — this is a read cache, not a checkpoint.
    pub fn copy_map_from(&mut self, other: &Ppu) {
        self.map_w = other.map_w;
        self.map_h = other.map_h;
        self.map.clear();
        self.map.extend_from_slice(&other.map);
    }

    fn cell_index(&self, x_cell: i32, y_cell: i32) -> Option<usize> {
        if x_cell < 0 || y_cell < 0 {
            return None;
        }
        let (x, y) = (x_cell as usize, y_cell as usize);
        if x >= self.map_w || y >= self.map_h {
            return None;
        }
        Some(y * self.map_w + x)
    }

    // ── Per-frame state ───────────────────────────────────────────────────

    pub fn set_scroll(&mut self, x: i32, y: i32) {
        self.scroll_x = x;
        self.scroll_y = y;
    }

    pub fn set_scroll_at(&mut self, scanline: usize, x: i32) {
        if scanline < SCREEN_H {
            self.scroll_at[scanline] = Some(x);
        }
    }

    /// Effective horizontal scroll for one scanline.
    pub fn scroll_for(&self, scanline: usize) -> i32 {
        self.scroll_at
            .get(scanline)
            .copied()
            .flatten()
            .unwrap_or(self.scroll_x)
    }

    pub fn push_sprite(&mut self, sprite: Sprite) {
        if self.sprites.len() < MAX_SPRITES {
            self.sprites.push(sprite);
        }
    }

    pub fn set_sprite_limit(&mut self, on: bool) {
        self.sprite_limit = on;
    }

    /// Clear everything the cart is expected to re-push this frame. Called by
    /// the host immediately before the script runs.
    pub fn begin_frame(&mut self) {
        self.sprites.clear();
        for s in self.scroll_at.iter_mut() {
            *s = None;
        }
    }

    // ── Rasterization ─────────────────────────────────────────────────────

    /// Resolve a (sub-palette, 2-bit index) pair to a master-palette index.
    /// Index 0 is always the shared backdrop, never the sub-palette's slot 0.
    #[inline]
    pub fn color_of(&self, palette: u8, index: u8) -> u8 {
        if index == 0 {
            return self.backdrop;
        }
        let p = (palette as usize) % PALETTE_COUNT;
        self.palettes[p][(index as usize) & 3]
    }

    /// Render the current state into `out` as RGB8, `SCREEN_W * SCREEN_H * 3`
    /// bytes. A short or oversized buffer is refused rather than partially
    /// filled, so a caller's sizing bug surfaces here instead of as garbage.
    ///
    /// One scanline at a time, background then sprites, exactly the order the
    /// hardware composites in — which is what makes per-scanline scroll and
    /// behind-background priority fall out naturally rather than needing a
    /// second pass. Everything it touches is either `self` or a fixed-size
    /// stack buffer: this runs 60 times a second and must not allocate.
    pub fn render(&self, out: &mut [u8]) {
        assert_eq!(
            out.len(),
            FRAME_BYTES,
            "PPU frame buffer must be {} bytes",
            FRAME_BYTES
        );

        // Per-scanline scratch. `bg_index` keeps the raw 2-bit index (not the
        // resolved color) because priority asks "is the background transparent
        // here", which only the index can answer.
        let mut bg_index = [0u8; SCREEN_W];
        let mut bg_color = [0u8; SCREEN_W];
        let mut spr_color = [0u8; SCREEN_W];
        let mut spr_state = [0u8; SCREEN_W];

        let map_px_w = (self.map_w * TILE_W) as i32;
        let map_px_h = (self.map_h * TILE_H) as i32;

        for y in 0..SCREEN_H {
            self.render_background(y, map_px_w, map_px_h, &mut bg_index, &mut bg_color);
            spr_state.fill(0);
            self.render_sprites(y, &mut spr_color, &mut spr_state);

            let row = y * SCREEN_W * 3;
            for x in 0..SCREEN_W {
                let s = spr_state[x];
                // A sprite pixel loses to the background only when it asked to
                // (BEHIND_BG) *and* the background is actually opaque there.
                let color = if s & SPR_DRAWN != 0 && (s & SPR_BEHIND == 0 || bg_index[x] == 0) {
                    spr_color[x]
                } else {
                    bg_color[x]
                };
                let i = row + x * 3;
                out[i..i + 3].copy_from_slice(&palette::rgb(color));
            }
        }
    }

    /// One scanline of the tile map, with the scroll for that line applied and
    /// wrapped. Walking the tile column by hand keeps the inner loop free of
    /// division and of the modulo the wrap would otherwise need per pixel.
    fn render_background(
        &self,
        y: usize,
        map_px_w: i32,
        map_px_h: i32,
        bg_index: &mut [u8; SCREEN_W],
        bg_color: &mut [u8; SCREEN_W],
    ) {
        // `rem_euclid` rather than `%`: a negative scroll must wrap to the far
        // edge of the map, not mirror around zero.
        let wy = (y as i32 + self.scroll_y).rem_euclid(map_px_h) as usize;
        let row_base = (wy / TILE_H) * self.map_w;
        let row_in_tile = wy % TILE_H;

        let wx = self.scroll_for(y).rem_euclid(map_px_w) as usize;
        let mut cell_x = wx / TILE_W;
        let mut px = wx % TILE_W;

        for x in 0..SCREEN_W {
            let cell = self.map[row_base + cell_x];
            let tile = &self.tiles[cell.tile as usize & (MAX_TILES - 1)];
            let index = tile[row_in_tile * TILE_W + px];
            bg_index[x] = index;
            bg_color[x] = self.color_of(cell.palette, index);

            px += 1;
            if px == TILE_W {
                px = 0;
                cell_x += 1;
                if cell_x == self.map_w {
                    cell_x = 0;
                }
            }
        }
    }

    /// One scanline of sprites. Push order is priority order: the first sprite
    /// to claim a pixel keeps it, so a later sprite never paints over an
    /// earlier one even when both are in front of the background.
    fn render_sprites(
        &self,
        y: usize,
        spr_color: &mut [u8; SCREEN_W],
        spr_state: &mut [u8; SCREEN_W],
    ) {
        let mut on_line = 0usize;
        for s in &self.sprites {
            let dy = y as i32 - s.y;
            if dy < 0 || dy >= TILE_H as i32 {
                continue;
            }
            // The hardware's limit counts sprites *evaluated* on the line, not
            // pixels drawn, so a fully transparent or off-screen sprite still
            // consumes a slot. Later sprites on the line are simply dropped.
            if self.sprite_limit && on_line >= SPRITES_PER_SCANLINE {
                break;
            }
            on_line += 1;

            let ty = if s.flip_y() {
                TILE_H - 1 - dy as usize
            } else {
                dy as usize
            };
            let tile = &self.tiles[s.tile as usize & (MAX_TILES - 1)];
            let flags = SPR_DRAWN | if s.behind_bg() { SPR_BEHIND } else { 0 };

            for tx in 0..TILE_W {
                let sx = s.x
                    + if s.flip_x() {
                        (TILE_W - 1 - tx) as i32
                    } else {
                        tx as i32
                    };
                if sx < 0 || sx >= SCREEN_W as i32 {
                    continue;
                }
                let xi = sx as usize;
                if spr_state[xi] & SPR_DRAWN != 0 {
                    continue;
                }
                let index = tile[ty * TILE_W + tx];
                if index == 0 {
                    continue;
                }
                spr_color[xi] = self.color_of(s.palette, index);
                spr_state[xi] = flags;
            }
        }
    }
}

/// Per-pixel sprite bookkeeping inside one scanline of [`Ppu::render`].
const SPR_DRAWN: u8 = 1;
const SPR_BEHIND: u8 = 2;

// ── Tests ─────────────────────────────────────────────────────────────────
//
// The rasterizer is the one piece of this crate with no way to be checked by
// eye at speed: a one-pixel priority or wrap mistake looks plausible in motion
// and wrong in a screenshot. So the cases below are all "put a known thing on
// screen, read the pixel back" rather than property checks.

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tile from the same row syntax carts use.
    fn tile(rows: [&str; 8]) -> Tile {
        let mut t = [0u8; TILE_PIXELS];
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                t[y * TILE_W + x] = match c {
                    '1' => 1,
                    '2' => 2,
                    '3' => 3,
                    _ => 0,
                };
            }
        }
        t
    }

    fn solid(index: u8) -> Tile {
        [index; TILE_PIXELS]
    }

    fn frame() -> Vec<u8> {
        vec![0u8; FRAME_BYTES]
    }

    fn px(buf: &[u8], x: usize, y: usize) -> [u8; 3] {
        let i = (y * SCREEN_W + x) * 3;
        [buf[i], buf[i + 1], buf[i + 2]]
    }

    /// The master index a rendered pixel came from, recovered by comparing RGB.
    /// The master palette has duplicate entries (several slots are the same
    /// white or black), so tests pick colors that appear exactly once.
    /// Comparing indices makes a failure readable ("expected 42, got 33")
    /// where comparing RGB triples does not.
    fn index_at(buf: &[u8], x: usize, y: usize) -> u8 {
        let rgb = px(buf, x, y);
        (0..palette::MASTER_COUNT as u8)
            .find(|&i| palette::rgb(i) == rgb)
            .expect("rendered color is not in the master palette")
    }

    #[test]
    fn color_zero_is_the_shared_backdrop() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(0, 1, 22, 42, 39);
        ppu.set_palette(4, 2, 5, 6, 7);

        // Slot 0 of a sub-palette is never consulted; the backdrop wins for
        // both a background and a sprite palette.
        assert_eq!(ppu.color_of(0, 0), 33);
        assert_eq!(ppu.color_of(4, 0), 33);
        assert_eq!(ppu.color_of(0, 1), 22);
        assert_eq!(ppu.color_of(0, 3), 39);
        assert_eq!(ppu.color_of(4, 2), 6);
    }

    #[test]
    fn blank_map_renders_the_backdrop() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        let mut buf = frame();
        ppu.render(&mut buf);
        assert_eq!(px(&buf, 0, 0), palette::rgb(33));
        assert_eq!(px(&buf, 255, 239), palette::rgb(33));
    }

    #[test]
    fn background_tiles_use_their_cell_palette() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(0, 0, 22, 0, 0);
        ppu.set_palette(1, 0, 42, 0, 0);
        ppu.define_tile(1, &solid(1));
        ppu.set_tile(0, 0, 1, 0);
        ppu.set_tile(1, 0, 1, 1);

        let mut buf = frame();
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 4, 4), 22);
        assert_eq!(index_at(&buf, 12, 4), 42);
        assert_eq!(index_at(&buf, 20, 4), 33, "cell 2 is still empty");
    }

    #[test]
    fn scroll_wraps_at_both_map_edges() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(0, 0, 22, 0, 0);
        ppu.define_tile(1, &solid(1));
        // A marker in the top-left cell of a 32x30 (256x240) map: exactly one
        // screen, so any scroll is a pure wrap.
        ppu.set_tile(0, 0, 1, 0);

        let mut buf = frame();

        // Scrolled one cell right, the marker lands at x = -8 .. 0 and its
        // right edge is off screen; the map's last column wraps into view.
        ppu.set_scroll(8, 0);
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 0, 0), 33);

        // Negative scroll must wrap to the far edge, not mirror around zero.
        ppu.set_scroll(-8, 0);
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 8, 0), 22);
        assert_eq!(index_at(&buf, 15, 0), 22);
        assert_eq!(index_at(&buf, 16, 0), 33);

        // A scroll of a whole map is the identity.
        ppu.set_scroll(256, 240);
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 0, 0), 22);

        // Vertical wrap, negative.
        ppu.set_scroll(0, -8);
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 0, 8), 22);
        assert_eq!(index_at(&buf, 0, 16), 33);

        // Far past the map in both directions still lands on the marker.
        ppu.set_scroll(-2560, -2400);
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 0, 0), 22);
    }

    #[test]
    fn per_scanline_scroll_splits_the_screen() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(0, 0, 22, 0, 0);
        ppu.define_tile(1, &solid(1));
        ppu.set_tile(0, 0, 1, 0);
        // Column 0 marked for every row, so a horizontal shift is visible on
        // any scanline.
        for y in 0..30 {
            ppu.set_tile(0, y, 1, 0);
        }
        ppu.set_scroll(0, 0);
        // A status bar: rows 0-15 stay pinned while the world below scrolls.
        for line in 16..SCREEN_H {
            ppu.set_scroll_at(line, -32);
        }

        let mut buf = frame();
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 0, 0), 22, "status rows are unscrolled");
        assert_eq!(index_at(&buf, 0, 16), 33);
        assert_eq!(index_at(&buf, 32, 16), 22, "the world below is shifted");

        // Overrides are per-frame: begin_frame drops them.
        ppu.begin_frame();
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 0, 16), 22);
    }

    #[test]
    fn sprites_flip_on_both_axes() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(4, 0, 0, 0, 39);
        // A single lit pixel in the top-left corner pins down both axes.
        ppu.define_tile(
            2,
            &tile([
                "3.......", "........", "........", "........", "........", "........", "........",
                "........",
            ]),
        );

        let mut buf = frame();
        let corner = |buf: &[u8]| {
            (0..8)
                .flat_map(|y| (0..8).map(move |x| (x, y)))
                .find(|&(x, y)| index_at(buf, x, y) == 39)
        };

        ppu.push_sprite(Sprite {
            x: 0,
            y: 0,
            tile: 2,
            palette: 4,
            flags: 0,
        });
        ppu.render(&mut buf);
        assert_eq!(corner(&buf), Some((0, 0)));

        ppu.sprites.clear();
        ppu.push_sprite(Sprite {
            x: 0,
            y: 0,
            tile: 2,
            palette: 4,
            flags: FLIP_X,
        });
        ppu.render(&mut buf);
        assert_eq!(corner(&buf), Some((7, 0)));

        ppu.sprites.clear();
        ppu.push_sprite(Sprite {
            x: 0,
            y: 0,
            tile: 2,
            palette: 4,
            flags: FLIP_Y,
        });
        ppu.render(&mut buf);
        assert_eq!(corner(&buf), Some((0, 7)));

        ppu.sprites.clear();
        ppu.push_sprite(Sprite {
            x: 0,
            y: 0,
            tile: 2,
            palette: 4,
            flags: FLIP_X | FLIP_Y,
        });
        ppu.render(&mut buf);
        assert_eq!(corner(&buf), Some((7, 7)));
    }

    #[test]
    fn sprite_color_zero_is_transparent() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(0, 0, 22, 0, 0);
        ppu.set_palette(4, 0, 0, 0, 39);
        ppu.define_tile(1, &solid(1));
        ppu.define_tile(
            2,
            &tile([
                "3333....", "3333....", "........", "........", "........", "........", "........",
                "........",
            ]),
        );
        ppu.set_tile(0, 0, 1, 0);
        ppu.push_sprite(Sprite {
            x: 0,
            y: 0,
            tile: 2,
            palette: 4,
            flags: 0,
        });

        let mut buf = frame();
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 0, 0), 39, "opaque sprite pixel");
        assert_eq!(index_at(&buf, 4, 0), 22, "transparent: background shows");
    }

    #[test]
    fn behind_bg_sprites_lose_to_opaque_background() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(0, 0, 22, 0, 0);
        ppu.set_palette(4, 0, 0, 0, 39);
        ppu.define_tile(1, &solid(1));
        ppu.define_tile(2, &solid(3));
        // Cell 0 opaque, cell 1 left empty so the same sprite has both a
        // covered and an uncovered half.
        ppu.set_tile(0, 0, 1, 0);
        ppu.push_sprite(Sprite {
            x: 4,
            y: 0,
            tile: 2,
            palette: 4,
            flags: BEHIND_BG,
        });

        let mut buf = frame();
        ppu.render(&mut buf);
        assert_eq!(
            index_at(&buf, 5, 0),
            22,
            "background wins where it is opaque"
        );
        assert_eq!(
            index_at(&buf, 9, 0),
            39,
            "sprite shows through background color 0"
        );

        // The same sprite in front covers both.
        ppu.sprites.clear();
        ppu.push_sprite(Sprite {
            x: 4,
            y: 0,
            tile: 2,
            palette: 4,
            flags: 0,
        });
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 5, 0), 39);
    }

    #[test]
    fn earlier_sprites_win_over_later_ones() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(4, 0, 0, 0, 39);
        ppu.set_palette(5, 0, 0, 0, 42);
        ppu.define_tile(2, &solid(3));
        ppu.push_sprite(Sprite {
            x: 0,
            y: 0,
            tile: 2,
            palette: 4,
            flags: 0,
        });
        ppu.push_sprite(Sprite {
            x: 0,
            y: 0,
            tile: 2,
            palette: 5,
            flags: 0,
        });

        let mut buf = frame();
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 0, 0), 39);
    }

    #[test]
    fn sprite_limit_drops_the_ninth_on_a_line() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(4, 0, 0, 0, 39);
        ppu.define_tile(2, &solid(3));
        // Nine side-by-side sprites on the same scanline.
        for i in 0..9 {
            ppu.push_sprite(Sprite {
                x: i * 8,
                y: 0,
                tile: 2,
                palette: 4,
                flags: 0,
            });
        }

        let mut buf = frame();
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 64, 0), 39, "off by default: all nine draw");

        ppu.set_sprite_limit(true);
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 56, 0), 39, "the first eight survive");
        assert_eq!(index_at(&buf, 64, 0), 33, "the ninth is dropped");

        // The limit is per scanline, not per frame: a tenth sprite on a
        // different line draws normally.
        ppu.push_sprite(Sprite {
            x: 64,
            y: 100,
            tile: 2,
            palette: 4,
            flags: 0,
        });
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 64, 100), 39);
    }

    #[test]
    fn sprites_clip_at_the_screen_edges() {
        let mut ppu = Ppu::new();
        ppu.set_backdrop(33);
        ppu.set_palette(4, 0, 0, 0, 39);
        ppu.define_tile(2, &solid(3));
        ppu.push_sprite(Sprite {
            x: -4,
            y: -4,
            tile: 2,
            palette: 4,
            flags: 0,
        });
        ppu.push_sprite(Sprite {
            x: 252,
            y: 236,
            tile: 2,
            palette: 4,
            flags: 0,
        });

        let mut buf = frame();
        ppu.render(&mut buf);
        assert_eq!(index_at(&buf, 0, 0), 39);
        assert_eq!(index_at(&buf, 3, 3), 39);
        assert_eq!(index_at(&buf, 4, 4), 33);
        assert_eq!(index_at(&buf, 255, 239), 39);
    }

    #[test]
    fn begin_frame_clears_only_per_frame_state() {
        let mut ppu = Ppu::new();
        ppu.set_palette(0, 1, 2, 3, 4);
        ppu.define_tile(1, &solid(1));
        ppu.set_tile(3, 3, 1, 0);
        ppu.push_sprite(Sprite {
            x: 0,
            y: 0,
            tile: 1,
            palette: 4,
            flags: 0,
        });
        ppu.set_scroll_at(10, 40);
        ppu.set_scroll(5, 6);

        ppu.begin_frame();

        assert!(ppu.sprites.is_empty());
        assert_eq!(
            ppu.scroll_for(10),
            5,
            "the override is gone, scroll_x remains"
        );
        assert_eq!(ppu.get_tile(3, 3), 1, "artwork and map survive");
        assert_eq!(ppu.palettes[0], [1, 2, 3, 4]);
    }

    #[test]
    fn map_writes_outside_the_map_are_dropped() {
        let mut ppu = Ppu::new();
        ppu.set_tile(-1, 0, 5, 0);
        ppu.set_tile(0, -1, 5, 0);
        ppu.set_tile(32, 0, 5, 0);
        ppu.set_tile(0, 30, 5, 0);
        assert_eq!(ppu.get_tile(0, 0), 0);
        assert_eq!(ppu.get_tile(-1, 0), 0);
    }

    #[test]
    fn resizing_the_map_clears_it_but_a_no_op_resize_does_not() {
        let mut ppu = Ppu::new();
        ppu.set_tile(1, 1, 7, 0);
        ppu.set_map_size(32, 30);
        assert_eq!(ppu.get_tile(1, 1), 7, "same size: cells kept");

        ppu.set_map_size(64, 60);
        assert_eq!(ppu.map.len(), 64 * 60);
        assert_eq!(ppu.get_tile(1, 1), 0);

        // Clamped to the hardware maximum rather than refused.
        ppu.set_map_size(9999, 9999);
        assert_eq!((ppu.map_w, ppu.map_h), (MAX_MAP_W, MAX_MAP_H));
    }

    #[test]
    fn hashed_tile_writes_skip_unchanged_data() {
        let mut ppu = Ppu::new();
        let mut decodes = 0;
        let define = |ppu: &mut Ppu, hash: u64, decodes: &mut i32| {
            ppu.define_tile_hashed(3, hash, || {
                *decodes += 1;
                solid(2)
            })
        };

        assert!(define(&mut ppu, 111, &mut decodes));
        assert!(!define(&mut ppu, 111, &mut decodes), "same source, skipped");
        assert!(define(&mut ppu, 222, &mut decodes));
        assert_eq!(decodes, 2);
        assert_eq!(ppu.tiles[3], solid(2));
    }

    #[test]
    fn sprite_capacity_is_capped() {
        let mut ppu = Ppu::new();
        for i in 0..MAX_SPRITES + 10 {
            ppu.push_sprite(Sprite {
                x: i as i32,
                y: 0,
                tile: 1,
                palette: 4,
                flags: 0,
            });
        }
        assert_eq!(ppu.sprites.len(), MAX_SPRITES);
    }

    /// A hand-built golden frame, checked pixel-exactly.
    ///
    /// One framed background tile in cell (0,0) with a 4x4 sprite block sitting
    /// on top of it, everything else backdrop. Small enough to read as a
    /// picture in the source, which is the point: if the composite order,
    /// palette resolution or sprite offset regresses, the diff below says which.
    #[test]
    fn golden_frame() {
        const A: u8 = 22; // background palette 0, index 1 — the frame
        const B: u8 = 42; // background palette 0, index 2 — the fill
        const S: u8 = 39; // sprite palette 4, index 3
        const D: u8 = 33; // the backdrop

        let mut ppu = Ppu::new();
        ppu.set_backdrop(D);
        ppu.set_palette(0, 0, A, B, 0);
        ppu.set_palette(4, 0, 0, 0, S);
        ppu.define_tile(
            1,
            &tile([
                "11111111", "12222221", "12222221", "12222221", "12222221", "12222221", "12222221",
                "11111111",
            ]),
        );
        ppu.define_tile(
            2,
            &tile([
                "3333....", "3333....", "3333....", "3333....", "........", "........", "........",
                "........",
            ]),
        );
        ppu.set_tile(0, 0, 1, 0);
        ppu.push_sprite(Sprite {
            x: 2,
            y: 2,
            tile: 2,
            palette: 4,
            flags: 0,
        });

        let mut buf = frame();
        ppu.render(&mut buf);

        #[rustfmt::skip]
        let expected: [[u8; 8]; 8] = [
            [A, A, A, A, A, A, A, A],
            [A, B, B, B, B, B, B, A],
            [A, B, S, S, S, S, B, A],
            [A, B, S, S, S, S, B, A],
            [A, B, S, S, S, S, B, A],
            [A, B, S, S, S, S, B, A],
            [A, B, B, B, B, B, B, A],
            [A, A, A, A, A, A, A, A],
        ];
        for (y, row) in expected.iter().enumerate() {
            for (x, want) in row.iter().enumerate() {
                assert_eq!(index_at(&buf, x, y), *want, "pixel ({}, {})", x, y);
            }
        }

        // Everything outside that one tile is the untouched backdrop.
        for y in 0..SCREEN_H {
            for x in 0..SCREEN_W {
                if x < 8 && y < 8 {
                    continue;
                }
                assert_eq!(px(&buf, x, y), palette::rgb(D), "pixel ({}, {})", x, y);
            }
        }
    }
}
