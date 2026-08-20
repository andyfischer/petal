//! Video natives: palettes, the pattern table, the background map, scroll, and
//! sprites. Everything a cart calls to put pixels on the screen.
//!
//! Each native emits a tagged command into the `nes_video` channel;
//! [`apply`] / [`apply_for`] drain that channel and walk the commands into a
//! [`Ppu`]. See [`super`] for why the indirection exists.
//!
//! STUB: every native is registered (so a cart never hits an unknown-function
//! error) and every call is emitted, but only the palette commands are applied.
//! Tile decoding, the map, scroll, and sprites are the video task's work.

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::stack::StackKey;
use petal::value::Value;

use crate::natives::{Command, emit, opt_nums, take_commands, take_commands_for};
use crate::ppu::{Ppu, palette};

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

// ── Draining ──────────────────────────────────────────────────────────────

/// Apply this frame's video commands from the live stack.
pub fn apply(env: &mut Env, ppu: &mut Ppu) {
    let commands = take_commands(env, VIDEO_CHANNEL);
    apply_commands(&commands, ppu);
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
            // STUB: the remaining tags are emitted but not yet applied.
            _ => {}
        }
    }
}

// ── Palettes ──────────────────────────────────────────────────────────────

fn native_set_palette(cxt: &mut PetalCxt) -> NativeResult {
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
    let index = Value::Int(cxt.get_int(1).unwrap_or(0));
    let rows = cxt.get_value(2).unwrap_or(Value::Nil);
    emit(cxt, VIDEO_CHANNEL, "define_tile", vec![index, rows]);
    cxt.push_nil();
    Ok(1)
}

fn native_define_tiles(cxt: &mut PetalCxt) -> NativeResult {
    let base = Value::Int(cxt.get_int(1).unwrap_or(0));
    let tiles = cxt.get_value(2).unwrap_or(Value::Nil);
    emit(cxt, VIDEO_CHANNEL, "define_tiles", vec![base, tiles]);
    cxt.push_nil();
    Ok(1)
}

fn native_load_tiles_png(cxt: &mut PetalCxt) -> NativeResult {
    let path = cxt.get_string(1).unwrap_or_default();
    let base = Value::Int(cxt.get_int(2).unwrap_or(0));
    let pathv = Value::String(cxt.heap_mut().alloc_string(path));
    emit(cxt, VIDEO_CHANNEL, "load_tiles_png", vec![pathv, base]);
    cxt.push_nil();
    Ok(1)
}

// ── Background map ────────────────────────────────────────────────────────

fn native_set_map_size(cxt: &mut PetalCxt) -> NativeResult {
    let args = opt_nums(cxt, 2);
    emit(cxt, VIDEO_CHANNEL, "set_map_size", args);
    cxt.push_nil();
    Ok(1)
}

fn native_set_tile(cxt: &mut PetalCxt) -> NativeResult {
    let args = opt_nums(cxt, 4);
    emit(cxt, VIDEO_CHANNEL, "set_tile", args);
    cxt.push_nil();
    Ok(1)
}

/// STUB: reads cannot go through the command channel — the cart needs an
/// answer during its own run, before the host drains anything. The video task
/// serves this from a per-frame map snapshot bound into the Env.
fn native_get_tile(cxt: &mut PetalCxt) -> NativeResult {
    cxt.push_int(0);
    Ok(1)
}

fn native_fill_map(cxt: &mut PetalCxt) -> NativeResult {
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
    let args = opt_nums(cxt, 2);
    emit(cxt, VIDEO_CHANNEL, "set_scroll_at", args);
    cxt.push_nil();
    Ok(1)
}

// ── Sprites ───────────────────────────────────────────────────────────────

/// Both the 4- and 5-argument forms; the missing `flags` reads as 0.
fn native_sprite(cxt: &mut PetalCxt) -> NativeResult {
    let args = opt_nums(cxt, 5);
    emit(cxt, VIDEO_CHANNEL, "sprite", args);
    cxt.push_nil();
    Ok(1)
}

fn native_sprite_meta(cxt: &mut PetalCxt) -> NativeResult {
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
