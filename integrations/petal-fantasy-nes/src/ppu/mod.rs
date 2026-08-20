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
        }
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
    /// STUB: fills with the backdrop color. Background, sprites, scroll, and
    /// priority are the PPU task's work.
    pub fn render(&self, out: &mut [u8]) {
        assert_eq!(
            out.len(),
            FRAME_BYTES,
            "PPU frame buffer must be {} bytes",
            FRAME_BYTES
        );
        let bg = palette::rgb(self.backdrop);
        for px in out.chunks_exact_mut(3) {
            px.copy_from_slice(&bg);
        }
    }
}
