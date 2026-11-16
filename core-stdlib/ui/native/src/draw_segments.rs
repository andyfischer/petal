//! Draining a **segmented** draw buffer: decoded commands kept per segment.
//!
//! A panel redraws its whole tree every frame, and memoization
//! ([`petal::memo`]) replays most of it: a helper whose inputs did not change
//! pushes what it drew last time instead of running. With the draw buffer in
//! segmented mode ([`petal::output_segments`]) a replayed helper reaches the
//! host as one segment reference instead of its commands, and a host that
//! keeps what it made of each segment can splice it back without decoding
//! it again.
//!
//! [`DrawSegments`] is that host-side cache for petal-ui's own drain. It
//! keeps each segment's commands decoded (a [`DrawCommand`] and the raw value
//! it came from, with the `pixel_snap` markers in their places), and expands
//! a drain into exactly the list [`take_draw_commands`](crate::draw::take_draw_commands)
//! returns for the same frame: the fresh values are decoded, the segments
//! are copied from the cache, and the whole list is composed (layers sorted,
//! retained layers resolved, batches lowered) as usual. So a frame that
//! replays its panel decodes only what it drew fresh.
//!
//! ```ignore
//! let mut segs = DrawSegments::new();
//! segs.enable(&mut env);                       // once, before the runs
//! // per frame, after the run:
//! let cmds = segs.take_draw_commands(&mut env);
//! ```
//!
//! The host contract it follows (docs/usage/embedding-c.md, "Segmented
//! buffers"):
//!
//! - a segment is defined once, children first, by a serial whose contents
//!   never change, and forgotten when the drain lists it as dropped;
//! - a drain it cannot follow (a reference to a serial it was never given)
//!   resets the cache and asks the runtime to define everything again on
//!   the next drain ([`Env::reset_output_segments`]); that frame loses the
//!   segment's commands, which is logged, as a broken drain is a bug;
//! - a buffer is flat while emit tracing is on, or when memo is off: every
//!   entry is then fresh and this is the flat drain;
//! - the values kept with a segment's commands stay rooted by the runtime
//!   until the serial is dropped, so they are safe to compose with.
//!
//! Only the default context's buffer is cached; a forked stack drains flat
//! ([`take_draw_commands_for`](crate::draw::take_draw_commands_for)).
//! A segment's commands are its decode, not its rendering: what the host
//! does with them (a texture, damage rectangles) is the next layer up.

use std::collections::HashMap;

use petal::env::Env;
use petal::heap::Heap;
use petal::output_segments::{SegItem, SegmentedFrame};
use petal::value::Value;

use crate::compose::{Item, compose_items, frame_command};
use crate::draw::{DRAW_COMMANDS_SYMBOL, DrawCommand};
use crate::subpixel::{FrameCommand, PixelSnap, snap_marker};

/// One entry of a cached segment.
#[derive(Debug, Clone)]
enum Part {
    /// A decoded command and the raw value it came from.
    Cmd(DrawCommand, Value),
    /// A `pixel_snap` marker: the mode the commands after it are drawn in.
    Snap(PixelSnap),
    /// A child segment, in its place.
    Segment(u64),
}

/// What the last drain did, for a host's frame statistics and for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SegmentStats {
    /// Raw values decoded this drain: fresh ones and those of new segments.
    pub decoded: usize,
    /// Commands copied from segments an earlier drain defined, instead of
    /// decoded (a segment defined by this drain is decoded once and copied
    /// into place, and counts in `decoded`).
    pub reused: usize,
    /// Segment references in the frame's own list that were spliced.
    pub spliced: usize,
    /// Segments defined by this drain.
    pub defined: usize,
    /// Segments held after the drain.
    pub held: usize,
    /// Whether the drain referred to a segment the cache did not have (and
    /// so reset itself).
    pub broken: bool,
}

/// A host's cache of decoded draw segments. See the module docs.
#[derive(Debug, Default)]
pub struct DrawSegments {
    /// Each segment's parts, and the drain that defined it.
    defs: HashMap<u64, (Vec<Part>, u64)>,
    stats: SegmentStats,
    /// Drains so far.
    drains: u64,
}

impl DrawSegments {
    pub fn new() -> Self {
        Self::default()
    }

    /// Put `env`'s draw buffer in segmented mode and start the cache over.
    pub fn enable(&mut self, env: &mut Env) {
        let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
        env.set_output_segmented(sym, true);
        env.reset_output_segments(sym);
        self.defs.clear();
    }

    /// Put `env`'s draw buffer back to flat and drop the cache.
    pub fn disable(&mut self, env: &mut Env) {
        let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
        env.set_output_segmented(sym, false);
        self.defs.clear();
    }

    /// Whether `env`'s draw buffer is segmented.
    pub fn enabled(env: &mut Env) -> bool {
        let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
        env.output_segmented(sym)
    }

    /// Forget every segment and have the next drain define them again: for
    /// a host that lost what it built from them.
    pub fn reset(&mut self, env: &mut Env) {
        let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
        env.reset_output_segments(sym);
        self.defs.clear();
    }

    /// What the last drain did.
    pub fn stats(&self) -> SegmentStats {
        self.stats
    }

    /// Segments held.
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Drain `env`'s draw buffer: the same list as
    /// [`take_draw_commands`](crate::draw::take_draw_commands), with replayed
    /// segments copied from the cache instead of decoded. On a buffer that
    /// is not segmented, it is that flat drain.
    pub fn take_draw_commands(&mut self, env: &mut Env) -> Vec<DrawCommand> {
        self.take_items(env).into_iter().map(|item| item.cmd).collect()
    }

    /// The same drain as [`take_frame_commands`](crate::subpixel::take_frame_commands),
    /// for a host registered with sub-pixel draws.
    pub fn take_frame_commands(&mut self, env: &mut Env) -> Vec<FrameCommand> {
        let items = self.take_items(env);
        let heap = env.heap();
        items.into_iter().map(|item| frame_command(item, heap)).collect()
    }

    /// The same drain as [`take_frame_commands_snapped`](crate::subpixel::take_frame_commands_snapped).
    pub fn take_frame_commands_snapped(&mut self, env: &mut Env) -> Vec<(FrameCommand, PixelSnap)> {
        let items = self.take_items(env);
        let heap = env.heap();
        items
            .into_iter()
            .map(|item| {
                let snap = item.src.as_ref().map(|s| s.snap).unwrap_or_default();
                (frame_command(item, heap), snap)
            })
            .collect()
    }

    fn take_items(&mut self, env: &mut Env) -> Vec<Item> {
        let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
        let frame = env.take_output_segments(sym);
        let mut stats = SegmentStats::default();
        self.drains += 1;
        let (cmds, values, snaps) = self.expand_frame(&frame, env.heap(), &mut stats);
        if stats.broken {
            eprintln!(
                "[petal-ui draw] the draw buffer named a segment this host was never given; \
                 the cache starts over"
            );
            self.defs.clear();
            env.reset_output_segments(sym);
        }
        stats.held = self.defs.len();
        self.stats = stats;
        compose_items(env, None, cmds, values, &snaps)
    }

    /// Take in `frame`'s definitions and drops and return its commands in
    /// order, flattened, with the raw value and snap mode of each.
    fn expand_frame(
        &mut self,
        frame: &SegmentedFrame,
        heap: &Heap,
        stats: &mut SegmentStats,
    ) -> (Vec<DrawCommand>, Vec<Value>, Vec<PixelSnap>) {
        for d in &frame.defined {
            let parts = decode_parts(&d.items, heap, stats);
            self.defs.insert(d.serial, (parts, self.drains));
            stats.defined += 1;
        }
        let mut out = Expanded::default();
        for item in &frame.items {
            match item {
                SegItem::Value(v) => {
                    stats.decoded += 1;
                    if let Some(part) = decode_part(v, heap) {
                        out.push(part);
                    }
                }
                SegItem::Segment(s) => {
                    stats.spliced += 1;
                    self.splice(*s, &mut out, stats);
                }
            }
        }
        for s in &frame.dropped {
            self.defs.remove(s);
        }
        (out.cmds, out.values, out.snaps)
    }

    fn splice(&self, serial: u64, out: &mut Expanded, stats: &mut SegmentStats) {
        let Some((parts, born)) = self.defs.get(&serial) else {
            stats.broken = true;
            return;
        };
        let old = *born != self.drains;
        for p in parts {
            match p {
                Part::Segment(child) => self.splice(*child, out, stats),
                Part::Cmd(..) => {
                    stats.reused += old as usize;
                    out.push(p.clone());
                }
                Part::Snap(_) => out.push(p.clone()),
            }
        }
    }
}

/// The flattened frame, built in order.
#[derive(Default)]
struct Expanded {
    cmds: Vec<DrawCommand>,
    values: Vec<Value>,
    snaps: Vec<PixelSnap>,
    snap: PixelSnap,
}

impl Expanded {
    fn push(&mut self, part: Part) {
        match part {
            Part::Cmd(cmd, value) => {
                self.cmds.push(cmd);
                self.values.push(value);
                self.snaps.push(self.snap);
            }
            Part::Snap(mode) => self.snap = mode,
            Part::Segment(_) => unreachable!("segments are spliced, not pushed"),
        }
    }
}

fn decode_parts(items: &[SegItem], heap: &Heap, stats: &mut SegmentStats) -> Vec<Part> {
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        match item {
            SegItem::Value(v) => {
                stats.decoded += 1;
                parts.extend(decode_part(v, heap));
            }
            SegItem::Segment(s) => parts.push(Part::Segment(*s)),
        }
    }
    parts
}

/// One raw value as the flat drain decodes it: a snap marker, a command, or
/// nothing (a malformed command, logged as the flat drain logs it).
fn decode_part(v: &Value, heap: &Heap) -> Option<Part> {
    if let Some(mode) = snap_marker(v, heap) {
        return Some(Part::Snap(mode));
    }
    match DrawCommand::from_value(v, heap) {
        Ok(cmd) => Some(Part::Cmd(cmd, *v)),
        Err(e) => {
            eprintln!("[petal-ui draw] {}", e);
            None
        }
    }
}
