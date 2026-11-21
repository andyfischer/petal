//! Damage: which parts of the screen a frame changed since the last one, so a
//! host that keeps its framebuffer between frames can paint only those parts.
//!
//! A canvas host runs its script every frame and gets the whole frame back as
//! draw commands, though most frames change a few pixels (a hover, a caret).
//! [`DamageTracker::update`] compares this frame's commands with the last
//! frame's and answers with a [`Damage`]: nothing, a few rectangles, or the
//! whole screen. Painting the frame again under a clip to each rectangle,
//! over the last frame's pixels, gives the same pixels as painting all of it.
//!
//! ```text
//! let damage = tracker.update(&commands, &[], w, h);
//! match damage {
//!     Damage::None => {}                          // keep last frame's pixels
//!     Damage::Full => paint(&commands, None),
//!     Damage::Rects(rs) => for r in rs {
//!         paint(&tracker.reaching(&commands, r), Some(r))   // clipped to r
//!     },
//! }
//! ```
//!
//! **The diff.** It is positional: the common prefix and suffix of the two
//! frames are unchanged; in between, items are compared pair by pair when both
//! frames have as many, else all of them changed. A changed item damages
//! where it was and where it is, cut to the clip in force. A clip itself
//! paints nothing; a moved clip shows as the bounds of what it clips moving.
//! An image is painted whole (a scaled blit samples by where its clip cuts
//! it), so damage that touches one grows to cover it. Damage past
//! [`FULL_SHARE`] of the screen, or in more than [`MAX_RECTS`] pieces after
//! merging, is painted as one.
//!
//! **When the whole frame is painted.** Painting part of a frame over the
//! last frame's pixels is exact only when the commands alone decide every
//! pixel. A frame is painted whole, and the frame after it too, when:
//!
//! - it is the first, or the screen changed size, or the host called
//!   [`DamageTracker::invalidate`] (it lost or replaced its framebuffer, or
//!   what an image or host command shows changed without its command
//!   changing and no identity was passed for it);
//! - it, or the frame before it, does not start with a `Clear`: its pixels
//!   build on the last frame's (a sketch that leaves trails), and drawing a
//!   translucent shape again changes them even when nothing else changed;
//! - it draws offscreen or reads pixels back (`CreateCanvas`, `SetTarget`,
//!   `DrawCanvas`, `Snapshot`, `BlurCanvas`), or holds a command the drain
//!   composes or a host paints itself (`Rects`, `Layer`, `Cache*`, `Mute`,
//!   `Host`), and it changed: its pixels depend on more than the screen
//!   bounds of its commands. Unchanged, it is still [`Damage::None`].
//!
//! Bounds are generous rather than tight (a pixel of slack for antialiasing;
//! text by its size and length, not its glyphs). A bound that is too small
//! would leave stale pixels, so a host can check itself by painting each frame
//! both ways and comparing (petal-sdl's `PETAL_SDL_DAMAGE=check`).

use crate::draw::DrawCommand;
use crate::subpixel::{FrameCommand, SubCommand};

/// A rectangle, `x0..x1` by `y0..y1` (exclusive), in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x0: x, y0: y, x1: x.saturating_add(w.max(0)), y1: y.saturating_add(h.max(0)) }
    }
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn w(&self) -> i32 {
        (self.x1 - self.x0).max(0)
    }
    pub fn h(&self) -> i32 {
        (self.y1 - self.y0).max(0)
    }
    pub fn area(&self) -> i64 {
        self.w() as i64 * self.h() as i64
    }
    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) }
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        !self.intersect(o).is_empty()
    }
    /// Grown by `d` on every side.
    pub fn grow(&self, d: i32) -> Rect {
        Rect {
            x0: self.x0.saturating_sub(d),
            y0: self.y0.saturating_sub(d),
            x1: self.x1.saturating_add(d),
            y1: self.y1.saturating_add(d),
        }
    }
}

/// What changed between two frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Damage {
    /// Nothing: the last frame's pixels are this frame's.
    None,
    /// These rectangles, inside the screen and apart from each other.
    Rects(Vec<Rect>),
    /// Everything.
    Full,
}

/// At most this many rectangles; past it they are merged into their union.
pub const MAX_RECTS: usize = 6;
/// Damage covering more than this share of the screen is painted whole.
pub const FULL_SHARE: f64 = 0.6;

/// What a command does to the screen, for the diff.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reach {
    /// Paints inside these bounds (before clipping).
    Paints(Rect),
    /// Paints, but where can't be bounded: any change to it is a full repaint.
    Unbounded,
    /// Fills the whole screen, whatever the clip.
    Clear,
    /// Clip stack operations, as petal-ui's hosts resolve them: a push
    /// intersects with the clip in force, `Clip` and `ClipNone` replace it.
    ClipPush(Rect),
    ClipPop,
    Clip(Rect),
    ClipNone,
    /// Makes the frame's pixels depend on more than its commands' bounds
    /// (offscreen drawing, reading pixels back, composed or host commands).
    Whole,
}

/// A draw command the tracker can follow.
pub trait Damageable: Clone + PartialEq {
    fn reach(&self) -> Reach;
    /// Painted whole or not at all: damage touching it grows to cover it.
    fn atomic(&self) -> bool {
        false
    }
}

impl Damageable for DrawCommand {
    fn reach(&self) -> Reach {
        use DrawCommand as D;
        let clip = |x: i32, y: i32, w: u32, h: u32| Rect::new(x, y, w.max(1) as i32, h.max(1) as i32);
        match self {
            D::Clear { .. } => Reach::Clear,
            D::ClipPush { x, y, w, h, .. } => Reach::ClipPush(clip(*x, *y, *w, *h)),
            D::ClipPop => Reach::ClipPop,
            D::Clip { x, y, w, h, .. } => Reach::Clip(clip(*x, *y, *w, *h)),
            D::ClipNone => Reach::ClipNone,
            D::CreateCanvas { .. } | D::SetTarget { .. } | D::DrawCanvas { .. } | D::Snapshot { .. } | D::BlurCanvas { .. }
            | D::Rects { .. } | D::Layer { .. } | D::CacheBegin { .. } | D::CacheEnd { .. } | D::CacheDraw { .. } | D::Mute { .. }
            | D::Host { .. } => Reach::Whole,
            other => match bounds(other) {
                Some(b) => Reach::Paints(b),
                None => Reach::Unbounded,
            },
        }
    }

    fn atomic(&self) -> bool {
        matches!(self, DrawCommand::Image { .. })
    }
}

impl Damageable for FrameCommand {
    fn reach(&self) -> Reach {
        match self {
            FrameCommand::Px(c) => c.reach(),
            FrameCommand::Sub(SubCommand::DrawCanvas { .. }) => Reach::Whole,
            // The whole-pixel twin's bounds, with a pixel more for the
            // fraction its coordinates were truncated by.
            FrameCommand::Sub(s) => match s.to_px().reach() {
                Reach::Paints(b) => Reach::Paints(b.grow(1)),
                other => other,
            },
        }
    }

    fn atomic(&self) -> bool {
        match self {
            FrameCommand::Px(c) => c.atomic(),
            FrameCommand::Sub(s) => matches!(s, SubCommand::Image { .. }),
        }
    }
}

/// Where a painting command can paint, ignoring clips, with a pixel of slack
/// for antialiasing and rounding; `None` for a command that doesn't paint on
/// its own (clips, canvases, composed and host commands).
///
/// A size past `i32::MAX` is a negative one wrapped by the decode, which
/// renderers don't agree on (petal-sdl fills it rightward but outlines it
/// leftward), so such a command can't be bounded either.
pub fn bounds(c: &DrawCommand) -> Option<Rect> {
    use DrawCommand as D;
    match c {
        D::Image { w, h, .. }
        | D::Rect { w, h, .. }
        | D::RectGradient { w, h, .. }
        | D::RectOutline { w, h, .. }
        | D::Shadow { w, h, .. }
            if (*w as i32) < 0 || (*h as i32) < 0 =>
        {
            return None;
        }
        _ => {}
    }
    let r = |x: i32, y: i32, w: u32, h: u32| Rect::new(x, y, w as i32, h as i32);
    let pts = |ps: &[(i32, i32)]| {
        let (mut a, mut b) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
        for &(x, y) in ps {
            a = (a.0.min(x), a.1.min(y));
            b = (b.0.max(x), b.1.max(y));
        }
        if ps.is_empty() { Rect::new(0, 0, 0, 0) } else { Rect { x0: a.0, y0: a.1, x1: b.0 + 1, y1: b.1 + 1 } }
    };
    let b = match c {
        D::Image { x, y, w, h, .. } => r(*x, *y, *w, *h),
        D::Rect { x, y, w, h, .. } | D::RectGradient { x, y, w, h, .. } => r(*x, *y, *w, *h),
        D::RectOutline { x, y, w, h, width, .. } => r(*x, *y, *w, *h).grow(*width as i32),
        D::Line { x1, y1, x2, y2, width, .. } => pts(&[(*x1, *y1), (*x2, *y2)]).grow(*width as i32 + 1),
        D::Circle { cx, cy, radius, .. } | D::CircleGradient { cx, cy, radius, .. } => {
            Rect::new(cx - radius, cy - radius, 2 * radius + 1, 2 * radius + 1)
        }
        // Text by its size and length: every glyph within an em of advance
        // (plus the letter spacing), and the line within two ems of height
        // from half an em above `y`.
        D::Text { text, x, y, size, spacing, .. } => {
            let s = *size as i32;
            let n = text.chars().count() as i32;
            let adv = s + spacing.abs().ceil() as i32 + 1;
            Rect::new(x - s / 2, y - s / 2, n.saturating_mul(adv).saturating_add(s), 2 * s + 4)
        }
        D::Triangle { x1, y1, x2, y2, x3, y3, .. } => pts(&[(*x1, *y1), (*x2, *y2), (*x3, *y3)]),
        D::Poly { points, .. } | D::Polygon { points, .. } => pts(points),
        D::Fan { cx, cy, points, .. } => pts(points).union(&Rect::new(*cx, *cy, 1, 1)),
        D::Polyline { points, width, .. } => pts(points).grow(*width as i32 + 1),
        D::Ellipse { cx, cy, rx, ry, .. } => Rect::new(cx - rx, cy - ry, 2 * rx + 1, 2 * ry + 1),
        D::EllipseOutline { cx, cy, rx, ry, width, .. } => {
            Rect::new(cx - rx, cy - ry, 2 * rx + 1, 2 * ry + 1).grow(*width as i32)
        }
        D::Arc { cx, cy, r_out, .. } => {
            let ro = r_out.ceil() as i32;
            Rect::new(cx - ro, cy - ro, 2 * ro + 1, 2 * ro + 1)
        }
        D::Shadow { x, y, w, h, blur, spread, dx, dy, .. } => {
            r(*x + dx, *y + dy, *w, *h).grow(*blur as i32 * 2 + spread.abs()).union(&r(*x, *y, *w, *h))
        }
        _ => return None,
    };
    Some(b.grow(1))
}

/// One command of a frame, as the diff sees it.
#[derive(Clone, Debug, PartialEq)]
struct Drawn<C> {
    cmd: C,
    /// Where it can paint, cut to the clip in force and to the screen; `None`
    /// when that can't be bounded; an empty rect when it paints nothing.
    bounds: Option<Rect>,
    /// What it shows that it doesn't hold (an image's pixels).
    ident: u64,
}

/// A frame's commands as [`Drawn`] items on `screen`.
struct Frame<C> {
    items: Vec<Drawn<C>>,
    /// It clears the screen before it paints anything else, so its pixels
    /// don't build on the last frame's.
    starts_clear: bool,
    /// It has a command whose pixels its bounds don't describe
    /// ([`Reach::Whole`]).
    has_whole: bool,
}

fn frame<C: Damageable>(cmds: &[C], idents: &[u64], screen: Rect) -> Frame<C> {
    let mut clip: Option<Rect> = None;
    let mut saved: Vec<Option<Rect>> = Vec::new();
    let mut painted = false;
    let mut starts_clear = false;
    let mut has_whole = false;
    let nothing = Some(Rect::new(0, 0, 0, 0));
    let out = cmds
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let bounds = match c.reach() {
                Reach::ClipPush(want) => {
                    saved.push(clip);
                    clip = Some(clip.map_or(want, |c| c.intersect(&want)));
                    nothing
                }
                Reach::ClipPop => {
                    clip = saved.pop().flatten();
                    nothing
                }
                Reach::Clip(want) => {
                    clip = Some(want);
                    nothing
                }
                Reach::ClipNone => {
                    clip = None;
                    nothing
                }
                Reach::Clear => {
                    starts_clear |= !painted;
                    painted = true;
                    Some(screen)
                }
                Reach::Paints(b) => {
                    painted = true;
                    Some(b.intersect(&clip.unwrap_or(screen)).intersect(&screen))
                }
                Reach::Unbounded => {
                    painted = true;
                    None
                }
                Reach::Whole => {
                    // A canvas op paints nothing on screen by itself, but a
                    // later `Clear` no longer starts the frame's pixels.
                    painted = true;
                    has_whole = true;
                    None
                }
            };
            Drawn { cmd: c.clone(), bounds, ident: idents.get(i).copied().unwrap_or(0) }
        })
        .collect();
    Frame { items: out, starts_clear, has_whole }
}

/// The damage from `prev` to `cur`.
fn diff<C: PartialEq>(prev: &[Drawn<C>], cur: &[Drawn<C>], screen: Rect) -> Damage {
    let n = prev.len().min(cur.len());
    let prefix = (0..n).find(|&i| prev[i] != cur[i]).unwrap_or(n);
    if prefix == prev.len() && prefix == cur.len() {
        return Damage::None;
    }
    let rest = n - prefix;
    let suffix = (0..rest).find(|&i| prev[prev.len() - 1 - i] != cur[cur.len() - 1 - i]).unwrap_or(rest);
    let (p, c) = (&prev[prefix..prev.len() - suffix], &cur[prefix..cur.len() - suffix]);
    let mut rects: Vec<Rect> = Vec::new();
    let mut add = |d: &Drawn<C>| -> bool {
        match d.bounds {
            None => false,
            Some(r) => {
                if !r.is_empty() {
                    rects.push(r);
                }
                true
            }
        }
    };
    if p.len() == c.len() {
        for (a, b) in p.iter().zip(c) {
            if a != b && !(add(a) && add(b)) {
                return Damage::Full;
            }
        }
    } else {
        for d in p.iter().chain(c) {
            if !add(d) {
                return Damage::Full;
            }
        }
    }
    settle(merge(rects), screen)
}

/// Grow `damage` so that every atomic item it touches is inside it.
fn close_over_atomic<C: Damageable>(damage: Damage, items: &[Drawn<C>], screen: Rect) -> Damage {
    let Damage::Rects(mut rects) = damage else { return damage };
    let whole: Vec<Rect> = items
        .iter()
        .filter(|d| d.cmd.atomic())
        .filter_map(|d| d.bounds)
        .filter(|b| !b.is_empty())
        .collect();
    loop {
        let mut grew = false;
        for r in rects.iter_mut() {
            for b in &whole {
                if r.overlaps(b) && r.union(b) != *r {
                    *r = r.union(b);
                    grew = true;
                }
            }
        }
        rects = merge(rects);
        if !grew {
            break;
        }
    }
    settle(rects, screen)
}

/// Rectangles as a [`Damage`]: none, these, or the whole screen when they
/// cover more than [`FULL_SHARE`] of it.
fn settle(rects: Vec<Rect>, screen: Rect) -> Damage {
    if rects.is_empty() {
        return Damage::None;
    }
    let area: i64 = rects.iter().map(|r| r.area()).sum();
    if area as f64 > FULL_SHARE * screen.area() as f64 {
        return Damage::Full;
    }
    Damage::Rects(rects)
}

/// Merge rectangles that overlap or nearly touch until none do, then into
/// their union past [`MAX_RECTS`].
fn merge(mut rects: Vec<Rect>) -> Vec<Rect> {
    loop {
        let mut merged = false;
        let mut i = 0;
        while i < rects.len() {
            let mut j = i + 1;
            while j < rects.len() {
                if rects[i].grow(4).overlaps(&rects[j]) {
                    let r = rects.swap_remove(j);
                    rects[i] = rects[i].union(&r);
                    merged = true;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
        if !merged {
            break;
        }
    }
    if rects.len() > MAX_RECTS {
        let all = rects.iter().fold(rects[0], |a, r| a.union(r));
        return vec![all];
    }
    rects
}

/// The last frame a host painted, to tell it what the next one changed. See
/// the [module docs](self).
#[derive(Debug)]
pub struct DamageTracker<C> {
    prev: Vec<Drawn<C>>,
    prev_starts_clear: bool,
    prev_has_whole: bool,
    prev_screen: Option<Rect>,
}

impl<C> Default for DamageTracker<C> {
    fn default() -> Self {
        DamageTracker { prev: Vec::new(), prev_starts_clear: false, prev_has_whole: false, prev_screen: None }
    }
}

impl<C: Damageable> DamageTracker<C> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Paint the next frame whole: the framebuffer was lost or replaced, or
    /// something a command shows changed without the command changing.
    pub fn invalidate(&mut self) {
        self.prev_screen = None;
    }

    /// What `cmds`, on a `w`×`h` screen, changed since the last frame passed
    /// here. `idents[i]` is an identity for what command `i` shows that it
    /// doesn't hold, such as the pixels behind an image's source name: a new
    /// identity damages the command though it reads the same. Missing
    /// entries are 0.
    pub fn update(&mut self, cmds: &[C], idents: &[u64], w: u32, h: u32) -> Damage {
        let screen = Rect::new(0, 0, w as i32, h as i32);
        let cur = frame(cmds, idents, screen);
        let damage = if self.prev_screen != Some(screen) || !(cur.starts_clear && self.prev_starts_clear) {
            Damage::Full
        } else {
            match diff(&self.prev, &cur.items, screen) {
                Damage::None => Damage::None,
                _ if cur.has_whole || self.prev_has_whole => Damage::Full,
                d => {
                    // An image that moved away is painted whole where it was
                    // too, so close over both frames' images.
                    let d = close_over_atomic(d, &self.prev, screen);
                    close_over_atomic(d, &cur.items, screen)
                }
            }
        };
        self.prev = cur.items;
        self.prev_starts_clear = cur.starts_clear;
        self.prev_has_whole = cur.has_whole;
        self.prev_screen = Some(screen);
        damage
    }

    /// Whether command `i` of the frame last passed to [`update`](Self::update)
    /// can paint inside `area`. Clips and commands that can't be bounded
    /// always can.
    pub fn reaches(&self, i: usize, area: &Rect) -> bool {
        match self.prev.get(i) {
            Some(d) => match (d.cmd.reach(), d.bounds) {
                (Reach::Paints(_), Some(b)) => b.overlaps(area),
                _ => true,
            },
            None => true,
        }
    }

    /// The commands of `cmds` (the frame last passed to
    /// [`update`](Self::update)) that can paint inside `area`, with every
    /// clip operation kept: what to paint, clipped to `area`, to repaint it.
    pub fn reaching(&self, cmds: &[C], area: Rect) -> Vec<C> {
        cmds.iter().enumerate().filter(|(i, _)| self.reaches(*i, &area)).map(|(_, c)| c.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 1000;

    fn rect(x: i32, y: i32, g: u8) -> DrawCommand {
        DrawCommand::Rect { x, y, w: 10, h: 10, r: 0, g, b: 0, a: 255, radius: 0 }
    }
    fn clear() -> DrawCommand {
        DrawCommand::Clear { r: 0, g: 0, b: 0 }
    }
    /// The damage of `b` after `a`.
    fn after(a: &[DrawCommand], b: &[DrawCommand]) -> Damage {
        let mut t = DamageTracker::new();
        assert_eq!(t.update(a, &[], W, W), Damage::Full, "the first frame is painted whole");
        t.update(b, &[], W, W)
    }
    /// A rect's bounds as the tracker grows them.
    fn grown(x: i32, y: i32) -> Rect {
        Rect::new(x - 1, y - 1, 12, 12)
    }

    #[test]
    fn the_same_frame_damages_nothing() {
        let f = [clear(), rect(0, 0, 1), rect(100, 0, 1)];
        assert_eq!(after(&f, &f), Damage::None);
    }

    #[test]
    fn a_changed_command_damages_where_it_was_and_is() {
        let a = [clear(), rect(0, 0, 1), rect(100, 100, 1), rect(500, 500, 1)];
        let mut b = a.clone();
        b[2] = rect(200, 100, 1);
        assert_eq!(after(&a, &b), Damage::Rects(vec![grown(100, 100), grown(200, 100)]));
    }

    #[test]
    fn an_insertion_damages_what_is_between_prefix_and_suffix() {
        let a = [clear(), rect(0, 0, 1), rect(500, 500, 1)];
        let b = [clear(), rect(0, 0, 1), rect(100, 100, 1), rect(500, 500, 1)];
        assert_eq!(after(&a, &b), Damage::Rects(vec![grown(100, 100)]));
    }

    #[test]
    fn a_new_identity_damages_a_command_that_reads_the_same() {
        let f = [clear(), rect(100, 100, 1)];
        let mut t = DamageTracker::new();
        t.update(&f, &[0, 1], W, W);
        assert_eq!(t.update(&f, &[0, 2], W, W), Damage::Rects(vec![grown(100, 100)]));
    }

    #[test]
    fn a_change_under_a_clip_damages_only_inside_it() {
        let clip = DrawCommand::ClipPush { x: 0, y: 0, w: 105, h: 1000, radius: 0 };
        let a = [clear(), clip.clone(), rect(100, 0, 1), DrawCommand::ClipPop];
        let b = [clear(), clip, rect(100, 0, 2), DrawCommand::ClipPop];
        assert_eq!(after(&a, &b), Damage::Rects(vec![Rect { x0: 99, y0: 0, x1: 105, y1: 11 }]));
    }

    #[test]
    fn a_moved_clip_damages_what_it_clips_where_it_was_and_is() {
        let at = |x| DrawCommand::ClipPush { x, y: 0, w: 5, h: 5, radius: 0 };
        let a = [clear(), at(0), rect(0, 0, 1), DrawCommand::ClipPop];
        let b = [clear(), at(5), rect(0, 0, 1), DrawCommand::ClipPop];
        assert_eq!(after(&a, &b), Damage::Rects(vec![Rect { x0: 0, y0: 0, x1: 10, y1: 5 }]));
    }

    #[test]
    fn damage_touching_an_image_covers_all_of_it() {
        let image = DrawCommand::Image {
            source: "a.png".into(),
            x: 100,
            y: 100,
            w: 50,
            h: 50,
            a: 255,
            radius: 0,
            tint: None,
            src: None,
            slice: None,
        };
        let a = [clear(), image.clone(), rect(140, 140, 1)];
        let b = [clear(), image, rect(140, 140, 2)];
        assert_eq!(after(&a, &b), Damage::Rects(vec![Rect::new(99, 99, 52, 52)]));
    }

    #[test]
    fn a_negative_size_is_unbounded() {
        // A width computed as -90, wrapped to u32 by the decode.
        let outline = |g| DrawCommand::RectOutline {
            x: 966,
            y: 96,
            w: -90i32 as u32,
            h: 400,
            r: 0,
            g,
            b: 0,
            a: 255,
            width: 1,
            radius: 0,
        };
        assert_eq!(bounds(&outline(1)), None);
        assert_eq!(after(&[clear(), outline(1)], &[clear(), outline(2)]), Damage::Full);
        assert_eq!(after(&[clear(), outline(1)], &[clear(), outline(1)]), Damage::None);
    }

    #[test]
    fn a_big_change_is_full() {
        let big = DrawCommand::Rect { x: 0, y: 0, w: 900, h: 900, r: 0, g: 0, b: 0, a: 255, radius: 0 };
        assert_eq!(after(&[clear(), rect(0, 0, 1)], &[clear(), big]), Damage::Full);
    }

    #[test]
    fn a_frame_without_a_clear_first_is_painted_whole() {
        // A sketch that leaves trails: last frame's pixels show through.
        let a = [rect(0, 0, 1)];
        let b = [rect(100, 0, 1)];
        assert_eq!(after(&a, &b), Damage::Full);
        // Unchanged too: a translucent shape drawn again over itself darkens.
        assert_eq!(after(&a, &a), Damage::Full);
    }

    #[test]
    fn offscreen_drawing_is_painted_whole() {
        // A change inside a canvas shows wherever the canvas is drawn, not
        // where the changed command's own bounds say.
        let canvas = |g| {
            vec![
                clear(),
                DrawCommand::CreateCanvas { id: 1, w: 20, h: 20 },
                DrawCommand::SetTarget { id: 1 },
                rect(0, 0, g),
                DrawCommand::SetTarget { id: 0 },
                DrawCommand::DrawCanvas { id: 1, x: 300, y: 300, a: 255, w: 0, h: 0 },
            ]
        };
        assert_eq!(after(&canvas(1), &canvas(2)), Damage::Full);
        assert_eq!(after(&canvas(1), &canvas(1)), Damage::None);
    }

    #[test]
    fn the_frame_after_a_whole_one_is_painted_whole() {
        let mut t = DamageTracker::new();
        t.update(&[rect(0, 0, 1)], &[], W, W);
        // Its pixels came from the frame before it, so this frame's commands
        // and the last one's don't say what is on screen.
        assert_eq!(t.update(&[clear(), rect(0, 0, 1)], &[], W, W), Damage::Full);
        assert_eq!(t.update(&[clear(), rect(0, 0, 2)], &[], W, W), Damage::Rects(vec![Rect::new(0, 0, 11, 11)]));
    }

    #[test]
    fn a_resize_or_an_invalidate_is_full() {
        let f = [clear(), rect(0, 0, 1)];
        let mut t = DamageTracker::new();
        t.update(&f, &[], W, W);
        assert_eq!(t.update(&f, &[], W, W + 1), Damage::Full);
        t.invalidate();
        assert_eq!(t.update(&f, &[], W, W + 1), Damage::Full);
        assert_eq!(t.update(&f, &[], W, W + 1), Damage::None);
    }

    #[test]
    fn reaching_keeps_clips_and_what_touches_the_area() {
        let f = [
            clear(),
            DrawCommand::ClipPush { x: 0, y: 0, w: 500, h: 500, radius: 0 },
            rect(0, 0, 1),
            rect(300, 300, 1),
            DrawCommand::ClipPop,
        ];
        let mut t = DamageTracker::new();
        t.update(&f, &[], W, W);
        let got = t.reaching(&f, Rect::new(0, 0, 20, 20));
        assert_eq!(got, vec![f[0].clone(), f[1].clone(), f[2].clone(), f[4].clone()]);
    }

    #[test]
    fn a_sub_pixel_command_is_bounded_by_its_whole_pixel_twin() {
        let sub = |x: f32| {
            FrameCommand::Sub(SubCommand::Rect { x, y: 0.5, w: 10.0, h: 10.0, r: 0, g: 0, b: 0, a: 255, radius: 0.0 })
        };
        let mut t = DamageTracker::new();
        t.update(&[FrameCommand::Px(clear()), sub(100.5)], &[], W, W);
        assert_eq!(
            t.update(&[FrameCommand::Px(clear()), sub(200.5)], &[], W, W),
            Damage::Rects(vec![Rect::new(98, 0, 14, 12), Rect::new(198, 0, 14, 12)])
        );
    }
}
