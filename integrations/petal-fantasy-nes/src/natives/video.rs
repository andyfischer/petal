//! Video natives: palettes, the pattern table, the background map, scroll, and
//! sprites. Everything a cart calls to put pixels on the screen.
//!
//! Each native emits a tagged command into the `nes_video` channel;
//! [`apply`] / [`apply_for`] drain that channel and walk the commands into a
//! [`Ppu`]. See [`super`] for why the indirection exists.
//!
//! ## Where errors are raised, and where they are not
//!
//! A cart author gets tile and palette indices wrong constantly, and the
//! command channel is the wrong place to complain from: by the time the host
//! drains it the script has finished and there is nothing to attach the blame
//! to. So the *natives* validate — they run inside the cart, so a bad index
//! surfaces as an ordinary Petal error naming the call — and the apply side
//! stays lenient, treating anything that slipped through as harmless.
//!
//! Two things are deliberately not errors:
//!
//! - **Master-palette colors** (0-63) wrap instead. A cart computing a color
//!   (`base + level`) should fade to a wrong shade, not halt.
//! - **Map coordinates outside the map**. Writing a rectangle that straddles
//!   the edge is normal cart code, and the PPU already drops those cells.
//!
//! ## Reads
//!
//! `get_tile` cannot go through the channel — the cart wants the answer during
//! its own run, before the host has drained anything. It is served from a
//! thread-local mirror of the map, written by the same natives that emit map
//! commands and resynced from the real console at the end of every frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use petal::env::Env;
use petal::heap::Heap;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::stack::StackKey;
use petal::value::Value;

use crate::natives::{Arg, Command, emit, opt_nums, take_commands, take_commands_for};
use crate::ppu::{
    MAX_MAP_H, MAX_MAP_W, MAX_TILES, PALETTE_COUNT, Ppu, SCREEN_H, Sprite, TILE_H, TILE_PIXELS,
    TILE_W, Tile, palette,
};

/// Output channel carrying video commands from the cart to the PPU.
pub const VIDEO_CHANNEL: &str = "nes_video";

pub fn register_video(env: &mut Env) {
    // Palettes
    env.register_native("set_palette", native_set_palette);
    env.register_native("set_backdrop", native_set_backdrop);
    env.register_native("master_rgb", native_master_rgb);

    // Pattern table
    env.register_native("define_tile", native_define_tile);
    env.register_native("define_tiles", native_define_tiles);
    env.register_native("load_tiles_png", native_load_tiles_png);

    // Background map
    env.register_native("set_map_size", native_set_map_size);
    env.register_native("set_tile", native_set_tile);
    env.register_native("get_tile", native_get_tile);
    env.register_native("fill_map", native_fill_map);
    env.register_native("set_scroll", native_set_scroll);
    env.register_native("set_scroll_at", native_set_scroll_at);

    // Sprites
    env.register_native("sprite", native_sprite);
    env.register_native("sprite_meta", native_sprite_meta);
    env.register_native("set_sprite_limit", native_set_sprite_limit);
}

// ── The map mirror ────────────────────────────────────────────────────────
//
// A whole `Ppu` rather than a bespoke struct so the mirror cannot drift from
// the console's own clamping and bounds rules — only its map is ever read.

thread_local! {
    static MAP_MIRROR: RefCell<Ppu> = RefCell::new(Ppu::new());
}

fn with_mirror<R>(f: impl FnOnce(&mut Ppu) -> R) -> R {
    MAP_MIRROR.with(|m| f(&mut m.borrow_mut()))
}

// ── Draining ──────────────────────────────────────────────────────────────

/// Apply this frame's video commands from the live stack.
pub fn apply(env: &mut Env, ppu: &mut Ppu) {
    let commands = take_commands(env, VIDEO_CHANNEL);
    apply_commands(&commands, ppu);
    // Heals any drift the mirror picked up from a speculative frame, and is
    // the only thing that keeps `get_tile` honest across a hot reload.
    with_mirror(|m| m.copy_map_from(ppu));
}

/// Apply a speculative frame's video commands (screenshot / agent capture).
/// The caller passes a `Ppu` clone so the live console is not disturbed.
pub fn apply_for(env: &mut Env, stack: StackKey, ppu: &mut Ppu) {
    let commands = take_commands_for(env, stack, VIDEO_CHANNEL);
    apply_commands(&commands, ppu);
}

fn apply_commands(commands: &[Command], ppu: &mut Ppu) {
    for c in commands {
        match c.tag.as_str() {
            "set_palette" => ppu.set_palette(c.usize(0), c.u8(1), c.u8(2), c.u8(3), c.u8(4)),
            "set_backdrop" => ppu.set_backdrop(c.u8(0)),

            "define_tile" => {
                let rows = c.arg(1);
                ppu.define_tile_hashed(c.usize(0), hash_of(rows), || decode_tile(rows.as_list()));
            }
            "define_tiles" => {
                let base = c.usize(0);
                for (i, rows) in c.arg(1).as_list().iter().enumerate() {
                    ppu.define_tile_hashed(base + i, hash_of(rows), || decode_tile(rows.as_list()));
                }
            }
            "load_tiles_png" => apply_load_png(c.str(0), c.usize(1), ppu),

            "set_map_size" => ppu.set_map_size(c.usize(0), c.usize(1)),
            "set_tile" => ppu.set_tile(c.i32(0), c.i32(1), c.i64(2) as u16, c.u8(3)),
            "fill_map" => ppu.fill_map(c.i64(0) as u16, c.u8(1)),
            "set_scroll" => ppu.set_scroll(c.i32(0), c.i32(1)),
            "set_scroll_at" => ppu.set_scroll_at(c.usize(0), c.i32(1)),

            "sprite" => ppu.push_sprite(Sprite {
                x: c.i32(0),
                y: c.i32(1),
                tile: c.i64(2) as u16,
                palette: c.u8(3),
                flags: c.u8(4),
            }),
            "sprite_meta" => push_meta_sprite(c, ppu),
            "set_sprite_limit" => ppu.set_sprite_limit(c.bool(0)),

            _ => {}
        }
    }
}

/// Expand a metasprite into its component 8x8 sprites.
///
/// Tiles are numbered left to right, top to bottom from `tile_base`, and a flip
/// flag mirrors the *arrangement* as well as each tile — otherwise flipping a
/// two-tile-wide character would swap its halves back to front, which is never
/// what the cart meant.
fn push_meta_sprite(c: &Command, ppu: &mut Ppu) {
    let (x, y) = (c.i32(0), c.i32(1));
    let base = c.i64(2);
    let pal = c.u8(3);
    let (w, h) = (c.usize(4).max(1), c.usize(5).max(1));
    let flags = c.u8(6);
    let flip_x = flags & crate::ppu::FLIP_X != 0;
    let flip_y = flags & crate::ppu::FLIP_Y != 0;

    for row in 0..h {
        for col in 0..w {
            let sx = if flip_x { w - 1 - col } else { col };
            let sy = if flip_y { h - 1 - row } else { row };
            ppu.push_sprite(Sprite {
                x: x + (col * TILE_W) as i32,
                y: y + (row * TILE_H) as i32,
                tile: (base + (sy * w + sx) as i64) as u16,
                palette: pal,
                flags,
            });
        }
    }
}

// ── Tile decoding ─────────────────────────────────────────────────────────

/// Decode the `rows` argument of `define_tile` into a tile.
///
/// Two accepted forms, because they serve different authors: eight 8-character
/// strings (`"..2222.."`) read as pixel art in the source, while eight packed
/// ints are what a converter or a generator emits. In the packed form the
/// leftmost pixel is the most significant pair, so `0b11_10_01_00...` reads
/// left to right like the string does.
///
/// Lenient on purpose — the native has already rejected malformed input with a
/// real error, so anything unexpected here becomes color 0 rather than a panic
/// at 60Hz.
fn decode_tile(rows: &[Arg]) -> Tile {
    let mut tile = [0u8; TILE_PIXELS];
    for (y, row) in rows.iter().take(TILE_H).enumerate() {
        match row {
            Arg::Str(s) => {
                for (x, c) in s.chars().take(TILE_W).enumerate() {
                    tile[y * TILE_W + x] = pixel_char(c);
                }
            }
            other => {
                let packed = other.as_i64();
                for x in 0..TILE_W {
                    tile[y * TILE_W + x] = ((packed >> (14 - 2 * x)) & 3) as u8;
                }
            }
        }
    }
    tile
}

/// `.`/`0` are color 0 (transparent, or the backdrop in a background tile);
/// `1`-`3` are the sub-palette's other three entries.
fn pixel_char(c: char) -> u8 {
    match c {
        '1' => 1,
        '2' => 2,
        '3' => 3,
        _ => 0,
    }
}

fn valid_pixel_char(c: char) -> bool {
    matches!(c, '.' | '0' | '1' | '2' | '3')
}

/// Hash of a tile's *source* form, so an unchanged redefinition can be skipped.
/// Arg has no `Hash` (it holds a float), so this walks it, tagging each variant
/// to keep `1` and `"1"` from colliding.
fn hash_of(arg: &Arg) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_arg(arg, &mut hasher);
    hasher.finish()
}

fn hash_arg(arg: &Arg, hasher: &mut DefaultHasher) {
    match arg {
        Arg::Nil => 0u8.hash(hasher),
        Arg::Int(n) => {
            1u8.hash(hasher);
            n.hash(hasher);
        }
        Arg::Float(f) => {
            2u8.hash(hasher);
            f.to_bits().hash(hasher);
        }
        Arg::Str(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        Arg::List(items) => {
            4u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_arg(item, hasher);
            }
        }
    }
}

// ── Argument checking ─────────────────────────────────────────────────────

/// Read an argument that may be absent, as an int. Missing reads as 0 so one
/// native registered at the widest arity serves every overload.
fn opt_int(cxt: &PetalCxt, index: usize) -> i64 {
    match cxt.get_value(index) {
        Ok(Value::Int(n)) => n,
        Ok(Value::Float(f)) => f as i64,
        Ok(Value::Bool(b)) => b as i64,
        _ => 0,
    }
}

fn check_range(call: &str, what: &str, value: i64, lo: i64, hi: i64) -> Result<(), String> {
    if value < lo || value > hi {
        return Err(format!(
            "{}: {} {} is out of range ({}-{})",
            call, what, value, lo, hi
        ));
    }
    Ok(())
}

fn check_tile(call: &str, index: i64) -> Result<(), String> {
    check_range(call, "tile index", index, 0, MAX_TILES as i64 - 1)
}

fn check_palette(call: &str, index: i64) -> Result<(), String> {
    check_range(call, "palette index", index, 0, PALETTE_COUNT as i64 - 1)
}

/// Validate one tile's row data, straight off the heap.
///
/// This duplicates the shape [`decode_tile`] later assumes rather than sharing
/// it, because only here is there a cart to blame: the message names the tile
/// slot and the offending row, which is the difference between a five-second
/// fix and a hunt through 200 lines of artwork.
fn check_tile_rows(heap: &Heap, call: &str, tile: i64, rows: Value) -> Result<(), String> {
    let Value::List(id) = rows else {
        return Err(format!(
            "{}: tile {} needs a list of 8 rows, got {}",
            call,
            tile,
            rows.type_name()
        ));
    };
    let rows = heap.get_list(id);
    if rows.len() != TILE_H {
        return Err(format!(
            "{}: tile {} needs 8 rows, got {}",
            call,
            tile,
            rows.len()
        ));
    }
    for (y, row) in rows.iter().enumerate() {
        match row {
            Value::Int(_) | Value::Float(_) => {}
            Value::String(id) => {
                let text = heap.get_string(*id);
                if text.chars().count() != TILE_W || !text.chars().all(valid_pixel_char) {
                    return Err(format!(
                        "{}: tile {} row {} must be 8 characters of \".0123\", got {:?}",
                        call, tile, y, text
                    ));
                }
            }
            other => {
                return Err(format!(
                    "{}: tile {} row {} must be a string or a packed int, got {}",
                    call,
                    tile,
                    y,
                    other.type_name()
                ));
            }
        }
    }
    Ok(())
}

// ── Palettes ──────────────────────────────────────────────────────────────

fn native_set_palette(cxt: &mut PetalCxt) -> NativeResult {
    check_palette("set_palette", opt_int(cxt, 1))?;
    let args = opt_nums(cxt, 5);
    emit(cxt, VIDEO_CHANNEL, "set_palette", args);
    cxt.push_nil();
    Ok(1)
}

fn native_set_backdrop(cxt: &mut PetalCxt) -> NativeResult {
    let args = opt_nums(cxt, 1);
    emit(cxt, VIDEO_CHANNEL, "set_backdrop", args);
    cxt.push_nil();
    Ok(1)
}

/// Pure lookup into the fixed master palette — no console state involved, so
/// it answers directly instead of going through the command channel.
fn native_master_rgb(cxt: &mut PetalCxt) -> NativeResult {
    let index = cxt.get_int(1).unwrap_or(0);
    cxt.push_int(palette::packed(index.rem_euclid(64) as u8));
    Ok(1)
}

// ── Pattern table ─────────────────────────────────────────────────────────

fn native_define_tile(cxt: &mut PetalCxt) -> NativeResult {
    let index = cxt.get_int(1).unwrap_or(0);
    check_tile("define_tile", index)?;
    let rows = cxt.get_value(2).unwrap_or(Value::Nil);
    check_tile_rows(cxt.heap(), "define_tile", index, rows)?;
    emit(
        cxt,
        VIDEO_CHANNEL,
        "define_tile",
        vec![Value::Int(index), rows],
    );
    cxt.push_nil();
    Ok(1)
}

fn native_define_tiles(cxt: &mut PetalCxt) -> NativeResult {
    let base = cxt.get_int(1).unwrap_or(0);
    check_tile("define_tiles", base)?;
    let tiles = cxt.get_value(2).unwrap_or(Value::Nil);
    let Value::List(id) = tiles else {
        return Err(format!(
            "define_tiles: expected a list of tiles, got {}",
            tiles.type_name()
        ));
    };
    let entries: Vec<Value> = cxt.heap().get_list(id).to_vec();
    if !entries.is_empty() {
        check_tile("define_tiles", base + entries.len() as i64 - 1)?;
    }
    for (i, rows) in entries.iter().enumerate() {
        check_tile_rows(cxt.heap(), "define_tiles", base + i as i64, *rows)?;
    }
    emit(
        cxt,
        VIDEO_CHANNEL,
        "define_tiles",
        vec![Value::Int(base), tiles],
    );
    cxt.push_nil();
    Ok(1)
}

fn native_load_tiles_png(cxt: &mut PetalCxt) -> NativeResult {
    let path = cxt.get_string(1)?;
    let base = cxt.get_int(2).unwrap_or(0);
    check_tile("load_tiles_png", base)?;
    // Existence is checked here (cheap, and the cart can be told) while the
    // decode happens at apply time behind a mtime cache, so a cart re-issuing
    // the call every frame does not re-read the file 60 times a second.
    if !Path::new(&path).is_file() {
        return Err(format!("load_tiles_png: no such file: {}", path));
    }
    let pathv = Value::String(cxt.heap_mut().alloc_string(path));
    emit(
        cxt,
        VIDEO_CHANNEL,
        "load_tiles_png",
        vec![pathv, Value::Int(base)],
    );
    cxt.push_nil();
    Ok(1)
}

// ── PNG import ────────────────────────────────────────────────────────────

// Decoded PNG tilesets, keyed by path and invalidated by modification time so
// editing the art file hot-reloads it like the cart itself. Failures are
// cached too: without that, a broken path would print once per frame.
thread_local! {
    static PNG_CACHE: RefCell<HashMap<String, (u128, Result<Vec<Tile>, String>)>> =
        RefCell::new(HashMap::new());
}

fn apply_load_png(path: &str, base: usize, ppu: &mut Ppu) {
    let stamp = file_stamp(path);
    PNG_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let fresh = cache.get(path).is_some_and(|(s, _)| *s == stamp);
        if !fresh {
            let decoded = png_to_tiles(path);
            if let Err(e) = &decoded {
                eprintln!("[fantasy-nes] {}", e);
            }
            cache.insert(path.to_string(), (stamp, decoded));
        }
        if let Some((_, Ok(tiles))) = cache.get(path) {
            for (i, tile) in tiles.iter().enumerate() {
                ppu.define_tile(base + i, tile);
            }
        }
    });
}

/// Modification time as a change stamp; 0 when the file has gone away, which
/// differs from any real stamp and so forces one re-read (and one error).
fn file_stamp(path: &str) -> u128 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Cut a PNG into 8x8 tiles, left to right then top to bottom.
///
/// The console has no notion of RGB art, so the importer has to invent a
/// mapping. The rule is the one that matches how 4-color art is actually
/// exported: transparent pixels are color 0 (the backdrop / sprite
/// transparency), and opaque colors take indices 1-3 in order of first
/// appearance across the whole image, so every tile shares one mapping.
fn png_to_tiles(path: &str) -> Result<Vec<Tile>, String> {
    let img = image::open(path)
        .map_err(|e| format!("load_tiles_png: cannot read {}: {}", path, e))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    if w % TILE_W as u32 != 0 || h % TILE_H as u32 != 0 {
        return Err(format!(
            "load_tiles_png: {} is {}x{}, which is not a whole number of 8x8 tiles",
            path, w, h
        ));
    }

    let mut colors: Vec<[u8; 3]> = Vec::with_capacity(3);
    let mut index_of = |px: &image::Rgba<u8>| -> u8 {
        if px[3] < 128 {
            return 0;
        }
        let rgb = [px[0], px[1], px[2]];
        match colors.iter().position(|c| *c == rgb) {
            Some(i) => i as u8 + 1,
            None if colors.len() < 3 => {
                colors.push(rgb);
                colors.len() as u8
            }
            // Past three opaque colors the art is out of budget; folding the
            // rest into the last entry keeps the import usable and visibly
            // wrong rather than silently blank.
            None => 3,
        }
    };

    let (cols, rows) = (w / TILE_W as u32, h / TILE_H as u32);
    let mut tiles = Vec::with_capacity((cols * rows) as usize);
    for ty in 0..rows {
        for tx in 0..cols {
            let mut tile = [0u8; TILE_PIXELS];
            for y in 0..TILE_H as u32 {
                for x in 0..TILE_W as u32 {
                    let px = img.get_pixel(tx * TILE_W as u32 + x, ty * TILE_H as u32 + y);
                    tile[(y as usize) * TILE_W + x as usize] = index_of(px);
                }
            }
            tiles.push(tile);
            if tiles.len() >= MAX_TILES {
                return Ok(tiles);
            }
        }
    }
    Ok(tiles)
}

// ── Background map ────────────────────────────────────────────────────────

fn native_set_map_size(cxt: &mut PetalCxt) -> NativeResult {
    let (w, h) = (opt_int(cxt, 1), opt_int(cxt, 2));
    check_range("set_map_size", "width", w, 1, MAX_MAP_W as i64)?;
    check_range("set_map_size", "height", h, 1, MAX_MAP_H as i64)?;
    with_mirror(|m| m.set_map_size(w as usize, h as usize));
    let args = opt_nums(cxt, 2);
    emit(cxt, VIDEO_CHANNEL, "set_map_size", args);
    cxt.push_nil();
    Ok(1)
}

fn native_set_tile(cxt: &mut PetalCxt) -> NativeResult {
    let (x, y) = (opt_int(cxt, 1), opt_int(cxt, 2));
    let (tile, pal) = (opt_int(cxt, 3), opt_int(cxt, 4));
    check_tile("set_tile", tile)?;
    check_palette("set_tile", pal)?;
    with_mirror(|m| m.set_tile(x as i32, y as i32, tile as u16, pal as u8));
    let args = opt_nums(cxt, 4);
    emit(cxt, VIDEO_CHANNEL, "set_tile", args);
    cxt.push_nil();
    Ok(1)
}

/// Answered from the map mirror, not the channel: the cart needs the value
/// mid-run. Cells outside the map read as tile 0, matching the PPU.
fn native_get_tile(cxt: &mut PetalCxt) -> NativeResult {
    let (x, y) = (opt_int(cxt, 1), opt_int(cxt, 2));
    let tile = with_mirror(|m| m.get_tile(x as i32, y as i32));
    cxt.push_int(tile as i64);
    Ok(1)
}

fn native_fill_map(cxt: &mut PetalCxt) -> NativeResult {
    let (tile, pal) = (opt_int(cxt, 1), opt_int(cxt, 2));
    check_tile("fill_map", tile)?;
    check_palette("fill_map", pal)?;
    with_mirror(|m| m.fill_map(tile as u16, pal as u8));
    let args = opt_nums(cxt, 2);
    emit(cxt, VIDEO_CHANNEL, "fill_map", args);
    cxt.push_nil();
    Ok(1)
}

fn native_set_scroll(cxt: &mut PetalCxt) -> NativeResult {
    let args = opt_nums(cxt, 2);
    emit(cxt, VIDEO_CHANNEL, "set_scroll", args);
    cxt.push_nil();
    Ok(1)
}

fn native_set_scroll_at(cxt: &mut PetalCxt) -> NativeResult {
    check_range(
        "set_scroll_at",
        "scanline",
        opt_int(cxt, 1),
        0,
        SCREEN_H as i64 - 1,
    )?;
    let args = opt_nums(cxt, 2);
    emit(cxt, VIDEO_CHANNEL, "set_scroll_at", args);
    cxt.push_nil();
    Ok(1)
}

// ── Sprites ───────────────────────────────────────────────────────────────

/// Both the 4- and 5-argument forms; the missing `flags` reads as 0.
fn native_sprite(cxt: &mut PetalCxt) -> NativeResult {
    check_tile("sprite", opt_int(cxt, 3))?;
    check_palette("sprite", opt_int(cxt, 4))?;
    let args = opt_nums(cxt, 5);
    emit(cxt, VIDEO_CHANNEL, "sprite", args);
    cxt.push_nil();
    Ok(1)
}

fn native_sprite_meta(cxt: &mut PetalCxt) -> NativeResult {
    let base = opt_int(cxt, 3);
    let (w, h) = (opt_int(cxt, 5), opt_int(cxt, 6));
    check_tile("sprite_meta", base)?;
    check_palette("sprite_meta", opt_int(cxt, 4))?;
    check_range("sprite_meta", "width in tiles", w, 1, MAX_TILES as i64)?;
    check_range("sprite_meta", "height in tiles", h, 1, MAX_TILES as i64)?;
    // The whole block has to exist, or the last row would silently wrap to
    // tile 0 and draw a hole where the character's feet are.
    check_tile("sprite_meta", base + w * h - 1)?;
    let args = opt_nums(cxt, 7);
    emit(cxt, VIDEO_CHANNEL, "sprite_meta", args);
    cxt.push_nil();
    Ok(1)
}

fn native_set_sprite_limit(cxt: &mut PetalCxt) -> NativeResult {
    let args = opt_nums(cxt, 1);
    emit(cxt, VIDEO_CHANNEL, "set_sprite_limit", args);
    cxt.push_nil();
    Ok(1)
}

// ── Tests ─────────────────────────────────────────────────────────────────
//
// These cover the half of the video path the PPU's own tests cannot reach: the
// translation from what a cart wrote to what the console holds.

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(rows: [&str; 8]) -> Arg {
        Arg::List(rows.iter().map(|r| Arg::Str(r.to_string())).collect())
    }

    fn ints(rows: [i64; 8]) -> Arg {
        Arg::List(rows.iter().map(|r| Arg::Int(*r)).collect())
    }

    fn cmd(tag: &str, args: Vec<Arg>) -> Command {
        Command {
            tag: tag.to_string(),
            args,
        }
    }

    #[test]
    fn decodes_the_string_form() {
        let tile = decode_tile(
            strs([
                "0123....", "........", "........", "........", "........", "........", "........",
                "33333333",
            ])
            .as_list(),
        );
        assert_eq!(&tile[0..8], &[0, 1, 2, 3, 0, 0, 0, 0]);
        assert_eq!(&tile[8..16], &[0; 8]);
        assert_eq!(&tile[56..64], &[3; 8]);
    }

    #[test]
    fn dot_and_zero_are_the_same_pixel() {
        let dots = decode_tile(strs([".1.1.1.1"; 8]).as_list());
        let zeros = decode_tile(strs(["01010101"; 8]).as_list());
        assert_eq!(dots, zeros);
    }

    #[test]
    fn decodes_the_packed_int_form() {
        // 0b00_01_10_11_00_00_00_00: the leftmost pixel is the high pair.
        let tile = decode_tile(ints([0b0001101100000000, 0, 0, 0, 0, 0, 0, 0xFFFF]).as_list());
        assert_eq!(&tile[0..8], &[0, 1, 2, 3, 0, 0, 0, 0]);
        assert_eq!(&tile[56..64], &[3; 8]);
    }

    #[test]
    fn both_forms_agree() {
        let from_strings = decode_tile(
            strs([
                "..2222..", ".233332.", "23311332", "23111132", "23111132", "23311332", ".233332.",
                "..2222..",
            ])
            .as_list(),
        );
        // The same eight rows, hand-packed two bits per pixel.
        let from_ints = decode_tile(
            ints([
                0b0000101010100000,
                0b0010111111111000,
                0b1011110101111110,
                0b1011010101011110,
                0b1011010101011110,
                0b1011110101111110,
                0b0010111111111000,
                0b0000101010100000,
            ])
            .as_list(),
        );
        assert_eq!(from_strings, from_ints);
    }

    #[test]
    fn short_or_odd_rows_decode_to_transparent_rather_than_panicking() {
        let tile = decode_tile(&[Arg::Str("12".to_string()), Arg::Nil]);
        assert_eq!(&tile[0..8], &[1, 2, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&tile[8..64], &[0u8; 56]);
    }

    #[test]
    fn identical_row_data_hashes_identically() {
        let a = strs(["11111111"; 8]);
        let b = strs(["11111111"; 8]);
        let c = strs(["11111112"; 8]);
        assert_eq!(hash_of(&a), hash_of(&b));
        assert_ne!(hash_of(&a), hash_of(&c));
        // A string row and an int row that happen to look alike must not
        // collide, or a form switch would be skipped as unchanged.
        assert_ne!(hash_of(&Arg::Int(1)), hash_of(&Arg::Str("1".to_string())));
    }

    #[test]
    fn define_tile_commands_reach_the_pattern_table() {
        let mut ppu = Ppu::new();
        apply_commands(
            &[cmd(
                "define_tile",
                vec![
                    Arg::Int(5),
                    strs([
                        "11111111", "........", "........", "........", "........", "........",
                        "........", "........",
                    ]),
                ],
            )],
            &mut ppu,
        );
        assert_eq!(&ppu.tiles[5][0..8], &[1; 8]);
        assert_eq!(&ppu.tiles[5][8..16], &[0; 8]);
    }

    #[test]
    fn define_tiles_lays_a_run_out_from_the_base_index() {
        let mut ppu = Ppu::new();
        apply_commands(
            &[cmd(
                "define_tiles",
                vec![
                    Arg::Int(10),
                    Arg::List(vec![strs(["11111111"; 8]), strs(["22222222"; 8])]),
                ],
            )],
            &mut ppu,
        );
        assert_eq!(ppu.tiles[10], [1u8; TILE_PIXELS]);
        assert_eq!(ppu.tiles[11], [2u8; TILE_PIXELS]);
    }

    #[test]
    fn map_and_scroll_commands_are_applied_in_order() {
        let mut ppu = Ppu::new();
        apply_commands(
            &[
                cmd("set_map_size", vec![Arg::Int(64), Arg::Int(60)]),
                cmd("fill_map", vec![Arg::Int(3), Arg::Int(1)]),
                cmd(
                    "set_tile",
                    vec![Arg::Int(63), Arg::Int(59), Arg::Int(7), Arg::Int(2)],
                ),
                cmd("set_scroll", vec![Arg::Int(-16), Arg::Int(24)]),
                cmd("set_scroll_at", vec![Arg::Int(100), Arg::Int(5)]),
            ],
            &mut ppu,
        );
        assert_eq!((ppu.map_w, ppu.map_h), (64, 60));
        assert_eq!(
            ppu.get_cell(0, 0),
            crate::ppu::MapCell {
                tile: 3,
                palette: 1
            }
        );
        assert_eq!(
            ppu.get_cell(63, 59),
            crate::ppu::MapCell {
                tile: 7,
                palette: 2
            }
        );
        assert_eq!((ppu.scroll_x, ppu.scroll_y), (-16, 24));
        assert_eq!(ppu.scroll_for(100), 5);
        assert_eq!(ppu.scroll_for(101), -16);
    }

    #[test]
    fn sprite_commands_carry_their_flags() {
        let mut ppu = Ppu::new();
        apply_commands(
            &[
                cmd(
                    "sprite",
                    vec![Arg::Int(8), Arg::Int(16), Arg::Int(2), Arg::Int(4)],
                ),
                cmd(
                    "sprite",
                    vec![
                        Arg::Int(0),
                        Arg::Int(0),
                        Arg::Int(3),
                        Arg::Int(5),
                        Arg::Int(5),
                    ],
                ),
                cmd("set_sprite_limit", vec![Arg::Int(1)]),
            ],
            &mut ppu,
        );
        assert_eq!(
            ppu.sprites[0],
            Sprite {
                x: 8,
                y: 16,
                tile: 2,
                palette: 4,
                flags: 0
            }
        );
        assert!(ppu.sprites[1].flip_x() && ppu.sprites[1].behind_bg());
        assert!(!ppu.sprites[1].flip_y());
        assert!(ppu.sprite_limit);
    }

    #[test]
    fn sprite_meta_expands_row_major() {
        let mut ppu = Ppu::new();
        apply_commands(
            &[cmd(
                "sprite_meta",
                vec![
                    Arg::Int(100),
                    Arg::Int(50),
                    Arg::Int(16),
                    Arg::Int(4),
                    Arg::Int(2),
                    Arg::Int(2),
                    Arg::Int(0),
                ],
            )],
            &mut ppu,
        );
        let placed: Vec<_> = ppu.sprites.iter().map(|s| (s.x, s.y, s.tile)).collect();
        assert_eq!(
            placed,
            vec![(100, 50, 16), (108, 50, 17), (100, 58, 18), (108, 58, 19),]
        );
    }

    #[test]
    fn sprite_meta_mirrors_the_arrangement_when_flipped() {
        let mut ppu = Ppu::new();
        apply_commands(
            &[cmd(
                "sprite_meta",
                vec![
                    Arg::Int(0),
                    Arg::Int(0),
                    Arg::Int(16),
                    Arg::Int(4),
                    Arg::Int(2),
                    Arg::Int(2),
                    Arg::Int(crate::ppu::FLIP_X as i64 | crate::ppu::FLIP_Y as i64),
                ],
            )],
            &mut ppu,
        );
        // Positions stay in place; the tiles behind them come from the
        // opposite corner, so the whole 16x16 image reads mirrored.
        let placed: Vec<_> = ppu.sprites.iter().map(|s| (s.x, s.y, s.tile)).collect();
        assert_eq!(placed, vec![(0, 0, 19), (8, 0, 18), (0, 8, 17), (8, 8, 16)]);
        assert!(ppu.sprites.iter().all(|s| s.flip_x() && s.flip_y()));
    }

    #[test]
    fn unchanged_tile_definitions_are_skipped() {
        let mut ppu = Ppu::new();
        let command = cmd("define_tile", vec![Arg::Int(1), strs(["11111111"; 8])]);
        apply_commands(std::slice::from_ref(&command), &mut ppu);
        // Poke the slot behind the PPU's back: a skipped redefinition leaves
        // the poke in place, a re-decode would wipe it.
        ppu.tiles[1][0] = 9;
        apply_commands(std::slice::from_ref(&command), &mut ppu);
        assert_eq!(ppu.tiles[1][0], 9);

        // Changed data does get through.
        apply_commands(
            &[cmd("define_tile", vec![Arg::Int(1), strs(["22222222"; 8])])],
            &mut ppu,
        );
        assert_eq!(ppu.tiles[1][0], 2);
    }

    #[test]
    fn range_errors_name_the_call_and_the_value() {
        assert_eq!(
            check_tile("sprite", 512).unwrap_err(),
            "sprite: tile index 512 is out of range (0-511)"
        );
        assert_eq!(
            check_palette("set_tile", -1).unwrap_err(),
            "set_tile: palette index -1 is out of range (0-7)"
        );
        assert!(check_tile("sprite", 511).is_ok());
        assert!(check_palette("set_tile", 7).is_ok());
    }
}
