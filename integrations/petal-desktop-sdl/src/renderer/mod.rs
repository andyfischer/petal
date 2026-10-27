use std::collections::HashMap;

use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Canvas, RenderTarget};
use sdl2::surface::Surface;
use sdl2::ttf::Font;

use petal_ui::draw::DrawCommand;

use crate::font::FontBook;

mod raster;
mod surface_fx;
mod text;

use raster::{
    PI, TAU, annular_sector, ellipse_ring, fill_circle_gradient, fill_disc_aa, fill_polygon,
    fill_rect_gradient, fill_rounded_rect, fill_shadow, set_draw_color, stroke_line_aa,
    stroke_rect_outline, stroke_rounded_rect_outline,
};
use surface_fx::{blit_surface_impl, blur_surface, snapshot_surface};
use text::render_text_impl;

/// Render targets that can also render TTF text. `texture_creator()` is not
/// part of the generic `Canvas<T>` API — it is defined separately for
/// `Canvas<Window>` and `Canvas<Surface>` with distinct context types — so we
/// abstract text rendering behind this trait. Each impl keeps its concrete
/// texture-creator/context type internal.
pub trait TextTarget: RenderTarget + Sized {
    fn render_text(
        canvas: &mut Canvas<Self>,
        font: &Font,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
    );
    /// Blit `src` onto this canvas at (`x`, `y`), scaled to `w`×`h` (0 = its
    /// own size) at opacity `a`. Used to composite an offscreen canvas onto a
    /// render target. Defined per-target because the `TextureCreator` context
    /// type is target-specific.
    fn blit_surface(
        canvas: &mut Canvas<Self>,
        src: &Surface,
        x: i32,
        y: i32,
        a: u8,
        w: u32,
        h: u32,
    );
}

impl TextTarget for Surface<'_> {
    fn render_text(
        canvas: &mut Canvas<Self>,
        font: &Font,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
    ) {
        // On a software `Canvas<Surface>`, `create_texture_from_surface` /
        // `copy` go through the software renderer. If any step fails it is
        // ignored (text is best-effort); primitives are unaffected.
        let creator = canvas.texture_creator();
        render_text_impl(canvas, &creator, font, text, x, y, color);
    }

    fn blit_surface(
        canvas: &mut Canvas<Self>,
        src: &Surface,
        x: i32,
        y: i32,
        a: u8,
        w: u32,
        h: u32,
    ) {
        let creator = canvas.texture_creator();
        blit_surface_impl(canvas, &creator, src, x, y, a, w, h);
    }
}

/// Offscreen canvases (PGraphics-style render targets), keyed by id. They are
/// rebuilt fresh from the command stream every frame, so the per-frame re-run
/// model needs no extra bookkeeping. Each is a software `Canvas<Surface>` with
/// an alpha channel so unpainted regions stay transparent and only the drawn
/// pixels composite onto the destination.
type Offscreen = HashMap<u32, Canvas<Surface<'static>>>;

/// Evaluate `$body` with `$c` bound to the active render target — `$canvas`
/// for target `0`, else that offscreen canvas. A macro rather than a function
/// because the two arms are different `Canvas` types. Evaluates to `None` (and
/// the command is silently dropped) when the target id is unknown.
macro_rules! on_target {
    ($canvas:expr, $offscreen:expr, $target:expr, |$c:ident| $body:expr) => {
        if $target == 0 {
            let $c = &mut *$canvas;
            Some($body)
        } else if let Some($c) = $offscreen.get_mut(&$target) {
            Some($body)
        } else {
            None
        }
    };
}

/// Take offscreen canvas `id` out of `offscreen`, hand it to `f` as a surface
/// (the map is lent too, minus `id`, so `f` can reach another canvas as the
/// source or destination), then put it back as a canvas. An unknown id is a
/// no-op.
fn with_offscreen_surface(
    offscreen: &mut Offscreen,
    id: u32,
    f: impl FnOnce(&mut Surface<'static>, &mut Offscreen),
) {
    if let Some(c) = offscreen.remove(&id) {
        let mut surface = c.into_surface();
        f(&mut surface, offscreen);
        if let Ok(sc) = surface.into_canvas() {
            offscreen.insert(id, sc);
        }
    }
}

pub fn render<T: TextTarget>(
    canvas: &mut Canvas<T>,
    commands: Vec<DrawCommand>,
    fonts: &mut FontBook,
) {
    let mut offscreen: Offscreen = HashMap::new();
    // The active render target. `0` is the main framebuffer; any other value is
    // an offscreen canvas id set via `SetTarget` (i.e. `draw_to`).
    let mut target: u32 = 0;
    // The clip stack behind `ClipPush`/`ClipPop`. `cur` is the clip in force
    // (`None` = the whole drawable) and `saved` holds the enclosing clips a
    // push displaced. It is resolved here rather than in `render_one` because
    // a push has to *intersect* with the clip already set, which means knowing
    // it — and `render_one` is per-command and stateless.
    let mut cur_clip: Option<Rect> = None;
    let mut saved_clips: Vec<Option<Rect>> = Vec::new();

    for cmd in commands {
        // Resolve the stack ops into the plain `Clip`/`ClipNone` the renderer
        // already knows, so there is one clip implementation, not three.
        let cmd = match cmd {
            DrawCommand::ClipPush { x, y, w, h, .. } => {
                saved_clips.push(cur_clip);
                let want = clip_rect(x, y, w, h);
                cur_clip = Some(match cur_clip {
                    // An empty intersection must clip *everything* out, so it
                    // becomes a 1px rect far off the drawable — SDL has no
                    // zero-sized clip rect to say it with.
                    Some(c) => c.intersection(want).unwrap_or_else(empty_clip),
                    None => want,
                });
                clip_cmd(cur_clip)
            }
            DrawCommand::ClipPop => {
                // An unmatched pop is a no-op back to "no clip", which is what
                // the command means when the stack is empty.
                cur_clip = saved_clips.pop().flatten();
                clip_cmd(cur_clip)
            }
            DrawCommand::Clip { x, y, w, h, .. } => {
                cur_clip = Some(clip_rect(x, y, w, h));
                cmd
            }
            DrawCommand::ClipNone => {
                cur_clip = None;
                cmd
            }
            other => other,
        };
        match cmd {
            DrawCommand::CreateCanvas { id, w, h } => {
                if let Ok(mut surface) = Surface::new(w.max(1), h.max(1), PixelFormatEnum::ARGB8888)
                {
                    // Start fully transparent so only drawn pixels composite.
                    let _ = surface.fill_rect(None, Color::RGBA(0, 0, 0, 0));
                    if let Ok(mut sc) = surface.into_canvas() {
                        // Drawing into the canvas should overwrite (set) alpha so
                        // painted pixels become opaque; the blit later composites.
                        sc.set_blend_mode(BlendMode::None);
                        offscreen.insert(id, sc);
                    }
                }
            }
            DrawCommand::SetTarget { id } => {
                target = id;
            }
            // The source canvas is pulled out of the map so the destination
            // (which may itself be another offscreen canvas) can be borrowed
            // at the same time, and restored so it can be blitted again.
            DrawCommand::DrawCanvas { id, x, y, a, w, h } => {
                with_offscreen_surface(&mut offscreen, id, |src, offscreen| {
                    on_target!(canvas, offscreen, target, |dst| {
                        TextTarget::blit_surface(dst, src, x, y, a, w, h)
                    });
                });
            }
            DrawCommand::Snapshot { id, x, y } => {
                // Read the current target's pixels under the canvas rect into
                // the canvas — the backdrop a material blurs.
                with_offscreen_surface(&mut offscreen, id, |dst, offscreen| {
                    let (w, h) = dst.size();
                    let shot = on_target!(canvas, offscreen, target, |src| {
                        snapshot_surface(src, x, y, w, h)
                    });
                    if let Some(mut shot) = shot.flatten() {
                        let _ = shot.set_blend_mode(BlendMode::None);
                        let _ = shot.blit(None, dst, None);
                    }
                });
            }
            DrawCommand::BlurCanvas { id, radius } => {
                with_offscreen_surface(&mut offscreen, id, |surface, _| {
                    blur_surface(surface, radius as f32)
                });
            }
            other => {
                on_target!(canvas, offscreen, target, |dst| render_one(dst, other, fonts));
            }
        }
    }
}

/// Render a single primitive draw command onto a target canvas. The canvas ops
/// (`CreateCanvas`, `SetTarget`, `DrawCanvas`, `Snapshot`, `BlurCanvas`) and
/// the clip stack ops are handled by `render` and never reach here.
fn render_one<T: TextTarget>(canvas: &mut Canvas<T>, cmd: DrawCommand, fonts: &mut FontBook) {
    match cmd {
        DrawCommand::Clear { r, g, b } => {
            canvas.set_draw_color(Color::RGB(r, g, b));
            canvas.clear();
        }
        DrawCommand::Rect {
            x,
            y,
            w,
            h,
            r,
            g,
            b,
            a,
            radius,
        } => {
            if radius == 0 {
                set_draw_color(canvas, r, g, b, a);
                let _ = canvas.fill_rect(Rect::new(x, y, w, h));
            } else {
                fill_rounded_rect(canvas, x, y, w, h, radius, r, g, b, a);
            }
        }
        DrawCommand::RectOutline {
            x,
            y,
            w,
            h,
            r,
            g,
            b,
            a,
            width,
            radius,
        } => {
            if radius == 0 {
                stroke_rect_outline(canvas, x, y, w, h, width, r, g, b, a);
            } else {
                stroke_rounded_rect_outline(canvas, x, y, w, h, radius, width, r, g, b, a);
            }
        }
        DrawCommand::Line {
            x1,
            y1,
            x2,
            y2,
            r,
            g,
            b,
            a,
            width,
        } => {
            stroke_line_aa(canvas, x1, y1, x2, y2, width, r, g, b, a);
        }
        DrawCommand::Circle {
            cx,
            cy,
            radius,
            r,
            g,
            b,
            a,
        } => {
            fill_disc_aa(
                canvas,
                cx as f64 + 0.5,
                cy as f64 + 0.5,
                radius as f64,
                r,
                g,
                b,
                a,
            );
        }
        DrawCommand::Triangle {
            x1,
            y1,
            x2,
            y2,
            x3,
            y3,
            r,
            g,
            b,
            a,
        } => {
            set_draw_color(canvas, r, g, b, a);
            fill_polygon(canvas, &[(x1, y1), (x2, y2), (x3, y3)]);
        }
        // The scanline filler is already even-odd, so a concave outline needs
        // no separate path here — `Polygon` differs from `Poly` only in the
        // guarantee it makes to the script.
        DrawCommand::Poly { points, r, g, b, a } | DrawCommand::Polygon { points, r, g, b, a } => {
            set_draw_color(canvas, r, g, b, a);
            fill_polygon(canvas, &points);
        }
        // A fan from `(cx, cy)` over an open point list covers exactly the
        // polygon center→p0→…→pn, which the scanline filler draws directly.
        DrawCommand::Fan {
            cx,
            cy,
            points,
            r,
            g,
            b,
            a,
        } => {
            let mut ring = Vec::with_capacity(points.len() + 1);
            ring.push((cx, cy));
            ring.extend(points);
            set_draw_color(canvas, r, g, b, a);
            fill_polygon(canvas, &ring);
        }
        DrawCommand::Polyline {
            points,
            r,
            g,
            b,
            a,
            width,
        } => {
            match points.as_slice() {
                [] => {}
                // A single point is a dot — the click a paint app must mark.
                [(x, y)] => fill_disc_aa(
                    canvas,
                    *x as f64 + 0.5,
                    *y as f64 + 0.5,
                    width.max(1) as f64 / 2.0,
                    r,
                    g,
                    b,
                    a,
                ),
                pts => {
                    for w in pts.windows(2) {
                        stroke_line_aa(canvas, w[0].0, w[0].1, w[1].0, w[1].1, width, r, g, b, a);
                    }
                    // Round joins, so a thick polyline has no notch at a corner.
                    if width > 2 {
                        for (x, y) in &pts[1..pts.len() - 1] {
                            fill_disc_aa(
                                canvas,
                                *x as f64 + 0.5,
                                *y as f64 + 0.5,
                                width as f64 / 2.0,
                                r,
                                g,
                                b,
                                a,
                            );
                        }
                    }
                }
            }
        }
        DrawCommand::Ellipse {
            cx,
            cy,
            rx,
            ry,
            r,
            g,
            b,
            a,
        } => {
            set_draw_color(canvas, r, g, b, a);
            fill_polygon(
                canvas,
                &ellipse_ring(cx, cy, rx as f64, ry as f64, 0.0, TAU),
            );
        }
        DrawCommand::EllipseOutline {
            cx,
            cy,
            rx,
            ry,
            r,
            g,
            b,
            a,
            width,
        } => {
            // A whole annulus is not a simple polygon, so it is filled as two
            // half-rings, each of which is.
            set_draw_color(canvas, r, g, b, a);
            let t = width.max(1) as f64;
            for (a0, a1) in [(0.0, PI), (PI, TAU)] {
                fill_polygon(
                    canvas,
                    &annular_sector(
                        cx,
                        cy,
                        (rx as f64 - t).max(0.0),
                        rx as f64,
                        (ry as f64 - t).max(0.0),
                        ry as f64,
                        a0,
                        a1,
                    ),
                );
            }
        }
        DrawCommand::Arc {
            cx,
            cy,
            r_in,
            r_out,
            a0,
            a1,
            r,
            g,
            b,
            a,
        } => {
            set_draw_color(canvas, r, g, b, a);
            let sweep = (a1 - a0).clamp(-TAU as f32, TAU as f32) as f64;
            // Split a sweep past half a turn so each piece stays simple.
            let halves = if sweep.abs() > PI { 2 } else { 1 };
            for i in 0..halves {
                let s0 = a0 as f64 + sweep * (i as f64) / halves as f64;
                let s1 = a0 as f64 + sweep * ((i + 1) as f64) / halves as f64;
                fill_polygon(
                    canvas,
                    &annular_sector(
                        cx,
                        cy,
                        r_in as f64,
                        r_out as f64,
                        r_in as f64,
                        r_out as f64,
                        s0,
                        s1,
                    ),
                );
            }
        }
        DrawCommand::Text {
            text,
            x,
            y,
            size,
            r,
            g,
            b,
            a,
            font,
            weight,
            italic,
            spacing,
        } => {
            // Face first (a role or installed family; anything else degrades
            // to the default), then the rung baked at exactly the command's
            // size, then synthetic bold/italic on that rung.
            let face = fonts.resolve(font.as_deref());
            let font = face.sized_styled(size, weight, italic);
            let color = Color::RGBA(r, g, b, a);
            match spacing == 0.0 {
                // The common path: one texture for the whole run, laid out
                // unkerned (every rung is) so it matches `text_width`.
                true => T::render_text(canvas, font, &text, x, y, color),
                // Letter-spacing has to place each glyph itself.
                false => {
                    let mut pen = x as f32;
                    for ch in text.chars() {
                        let advance = font.find_glyph_metrics(ch).map_or(0, |m| m.advance) as f32;
                        T::render_text(canvas, font, &ch.to_string(), pen as i32, y, color);
                        pen += advance + spacing;
                    }
                }
            }
        }
        DrawCommand::RectGradient {
            x,
            y,
            w,
            h,
            radius,
            r0,
            g0,
            b0,
            a0,
            r1,
            g1,
            b1,
            a1,
            angle,
        } => {
            fill_rect_gradient(
                canvas,
                x,
                y,
                w,
                h,
                radius,
                [r0, g0, b0, a0],
                [r1, g1, b1, a1],
                angle,
            );
        }
        DrawCommand::CircleGradient {
            cx,
            cy,
            radius,
            r0,
            g0,
            b0,
            a0,
            r1,
            g1,
            b1,
            a1,
        } => {
            fill_circle_gradient(canvas, cx, cy, radius, [r0, g0, b0, a0], [r1, g1, b1, a1]);
        }
        DrawCommand::Shadow {
            x,
            y,
            w,
            h,
            radius,
            blur,
            spread,
            dx,
            dy,
            r,
            g,
            b,
            a,
        } => {
            fill_shadow(canvas, x, y, w, h, radius, blur, spread, dx, dy, r, g, b, a);
        }
        // The corner radius is dropped: SDL's clip is a scissor rectangle, so
        // a rounded clip degrades to the square one this command has always
        // meant.
        DrawCommand::Clip { x, y, w, h, .. } => {
            canvas.set_clip_rect(clip_rect(x, y, w, h));
        }
        DrawCommand::ClipNone => {
            canvas.set_clip_rect(None);
        }
        // Bitmap loading is host-specific and this integration has not opted
        // into it yet. Unsupported standard commands are intentionally skipped.
        DrawCommand::Image { .. } => {}
        // Host-extension tags this app doesn't define; nothing to draw.
        DrawCommand::Host { .. } => {}
        // Lowered by the drain for a host that has not asked for them
        // (`petal_ui::compose::HostCaps`): batches arrive as rects, retained
        // layers as their content, and layer markers never leave the drain.
        DrawCommand::Rects { .. }
        | DrawCommand::Layer { .. }
        | DrawCommand::Mute { .. }
        | DrawCommand::CacheBegin { .. }
        | DrawCommand::CacheEnd { .. }
        | DrawCommand::CacheDraw { .. } => {}
        // Handled in `render` (the clip stack ops are resolved to plain
        // `Clip`/`ClipNone` there, which owns the stack); unreachable here.
        DrawCommand::ClipPush { .. }
        | DrawCommand::ClipPop
        | DrawCommand::CreateCanvas { .. }
        | DrawCommand::SetTarget { .. }
        | DrawCommand::DrawCanvas { .. }
        | DrawCommand::Snapshot { .. }
        | DrawCommand::BlurCanvas { .. } => {}
    }
}

/// The clip rect for a `Clip`/`ClipPush` of `w`×`h` at (`x`, `y`). SDL rects
/// can't be empty, so a zero side is widened to 1px.
fn clip_rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
    Rect::new(x, y, w.max(1), h.max(1))
}

/// The clip rect that means "nothing is visible": 1px, far outside any
/// drawable. SDL's clip is a `Rect`, which cannot be empty, so an empty
/// intersection of two clips has to be spelled this way.
fn empty_clip() -> Rect {
    Rect::new(i32::MIN / 2, i32::MIN / 2, 1, 1)
}

/// Turn the clip this renderer is tracking back into the draw command that
/// applies it, so the stack ops share `render_one`'s one clip implementation.
fn clip_cmd(clip: Option<Rect>) -> DrawCommand {
    match clip {
        Some(c) => DrawCommand::Clip {
            x: c.x(),
            y: c.y(),
            w: c.width(),
            h: c.height(),
            radius: 0,
        },
        None => DrawCommand::ClipNone,
    }
}

/// SDL_ttf is process-global: `sdl2::ttf::init()` fails with
/// `AlreadyInitializedError` while another context is alive, and dropping a
/// context calls `TTF_Quit` under everyone else. Tests that open a TTF context
/// hold this lock for their whole body so the parallel test runner cannot
/// interleave them.
#[cfg(test)]
pub(crate) fn ttf_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::DEFAULT_LADDER;
    use sdl2::pixels::PixelFormatEnum;
    use sdl2::surface::Surface;

    const W: u32 = 64;
    const H: u32 = 64;

    /// Build a black, persistent software surface for accumulation tests.
    fn new_black_surface() -> Surface<'static> {
        let mut surface = Surface::new(W, H, PixelFormatEnum::RGB888).unwrap();
        surface.fill_rect(None, Color::RGB(0, 0, 0)).unwrap();
        surface
    }

    /// Build a small font ladder for tests; returns None if no system font is
    /// available so tests stay robust in headless CI. Primitive-only frames
    /// don't need it.
    fn load_test_fonts(ttf: &sdl2::ttf::Sdl2TtfContext) -> Option<FontBook<'_>> {
        FontBook::load_system(ttf, DEFAULT_LADDER).ok()
    }

    /// Render a frame's commands into the persistent surface via the REAL
    /// `render` entry point, exercising the generic `Canvas<Surface>` target.
    /// Uses the same into_canvas / into_surface dance the game loop uses.
    fn render_frame(
        surface: Surface<'static>,
        commands: Vec<DrawCommand>,
        fonts: &mut FontBook,
    ) -> Surface<'static> {
        let mut sc = surface.into_canvas().unwrap();
        render(&mut sc, commands, fonts);
        sc.into_surface()
    }

    /// Read back the RGB of a pixel from a locked surface.
    fn pixel_rgb(surface: &Surface, px: u32, py: u32) -> (u8, u8, u8) {
        let pitch = surface.pitch() as usize;
        let bpp = surface.pixel_format_enum().byte_size_per_pixel();
        let mut out = (0u8, 0u8, 0u8);
        surface.with_lock(|pixels: &[u8]| {
            let offset = (py as usize) * pitch + (px as usize) * bpp;
            // RGB888 packs as a 32-bit little-endian value: byte0=B, byte1=G, byte2=R
            let b = pixels[offset];
            let g = pixels[offset + 1];
            let r = pixels[offset + 2];
            out = (r, g, b);
        });
        out
    }

    fn is_white(px: (u8, u8, u8)) -> bool {
        px.0 > 200 && px.1 > 200 && px.2 > 200
    }

    fn is_black(px: (u8, u8, u8)) -> bool {
        px.0 < 50 && px.1 < 50 && px.2 < 50
    }

    fn white_rect(x: i32, y: i32, w: u32, h: u32) -> DrawCommand {
        DrawCommand::Rect {
            x,
            y,
            w,
            h,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            radius: 0,
        }
    }

    /// `snapshot` copies the target as painted so far; `blur_canvas` turns a
    /// hard edge in the copy into a ramp; `draw_canvas` puts it back.
    #[test]
    fn a_snapshot_blurred_and_redrawn_is_a_ramp() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let surface = render_frame(
            new_black_surface(),
            vec![
                // Left half white.
                white_rect(0, 0, 32, 64),
                DrawCommand::CreateCanvas {
                    id: 1,
                    w: 64,
                    h: 16,
                },
                DrawCommand::Snapshot { id: 1, x: 0, y: 0 },
                DrawCommand::BlurCanvas { id: 1, radius: 4 },
                // Cover everything, then draw the blurred strip at the bottom.
                DrawCommand::Rect {
                    x: 0,
                    y: 0,
                    w: 64,
                    h: 64,
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                    radius: 0,
                },
                DrawCommand::DrawCanvas {
                    id: 1,
                    x: 0,
                    y: 48,
                    a: 255,
                    w: 0,
                    h: 0,
                },
            ],
            &mut fonts,
        );
        let at = |x: u32| pixel_rgb(&surface, x, 56).0 as i32;
        assert!(at(2) > 247, "far left stays white: {}", at(2));
        assert!(at(61) < 8, "far right stays black: {}", at(61));
        assert!(at(30) > 20 && at(30) < 235, "ramp at 30: {}", at(30));
        assert!(at(34) > 20 && at(34) < 235, "ramp at 34: {}", at(34));
        assert!(at(28) > at(32) && at(32) > at(36), "monotone ramp");
        // Above the strip the covering rect stands.
        assert!(is_black(pixel_rgb(&surface, 10, 10)));
    }

    /// A layer composited at half opacity scales the whole layer once.
    #[test]
    fn draw_canvas_alpha_applies_to_the_layer() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let surface = render_frame(
            new_black_surface(),
            vec![
                DrawCommand::CreateCanvas {
                    id: 1,
                    w: 16,
                    h: 16,
                },
                DrawCommand::SetTarget { id: 1 },
                white_rect(0, 0, 16, 16),
                white_rect(0, 0, 16, 16),
                DrawCommand::SetTarget { id: 0 },
                DrawCommand::DrawCanvas {
                    id: 1,
                    x: 0,
                    y: 0,
                    a: 128,
                    w: 32,
                    h: 32,
                },
            ],
            &mut fonts,
        );
        // Scaled to 32×32, so (24, 24) is inside; grey, not white.
        let p = pixel_rgb(&surface, 24, 24);
        assert!(
            p.0 > 100 && p.0 < 160,
            "half-opacity white over black: {p:?}"
        );
        assert!(is_black(pixel_rgb(&surface, 40, 40)));
    }

    #[test]
    fn no_clear_accumulates() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let mut surface = new_black_surface();

        // Frame 1: white rect at (2,2), NO clear.
        surface = render_frame(
            surface,
            vec![DrawCommand::Rect {
                x: 2,
                y: 2,
                w: 4,
                h: 4,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                radius: 0,
            }],
            &mut fonts,
        );
        // Frame 2: white rect at (40,40), NO clear — should accumulate.
        surface = render_frame(
            surface,
            vec![DrawCommand::Rect {
                x: 40,
                y: 40,
                w: 4,
                h: 4,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                radius: 0,
            }],
            &mut fonts,
        );

        assert!(
            is_white(pixel_rgb(&surface, 3, 3)),
            "frame-1 pixel should persist"
        );
        assert!(
            is_white(pixel_rgb(&surface, 41, 41)),
            "frame-2 pixel should be drawn"
        );
    }

    #[test]
    fn clear_wipes() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let mut surface = new_black_surface();

        // Frame 1: white rect at (2,2).
        surface = render_frame(
            surface,
            vec![DrawCommand::Rect {
                x: 2,
                y: 2,
                w: 4,
                h: 4,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                radius: 0,
            }],
            &mut fonts,
        );
        // Frame 2: Clear(black) then white rect at (40,40).
        surface = render_frame(
            surface,
            vec![
                DrawCommand::Clear { r: 0, g: 0, b: 0 },
                DrawCommand::Rect {
                    x: 40,
                    y: 40,
                    w: 4,
                    h: 4,
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                    radius: 0,
                },
            ],
            &mut fonts,
        );

        assert!(
            is_black(pixel_rgb(&surface, 3, 3)),
            "frame-1 pixel should be wiped"
        );
        assert!(
            is_white(pixel_rgb(&surface, 41, 41)),
            "frame-2 pixel should be drawn"
        );
    }

    #[test]
    fn text_renders_on_software_surface() {
        // Confirms TTF text rendering works on a `Canvas<Surface>` (software
        // renderer path) and is not silently dropped. We scan the text's
        // bounding box for any non-black pixel.
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let surface = render_frame(
            new_black_surface(),
            vec![DrawCommand::plain_text(
                "Hi".to_string(),
                2,
                2,
                24,
                255,
                255,
                255,
                255,
            )],
            &mut fonts,
        );

        let mut any_lit = false;
        for py in 0..32 {
            for px in 0..40 {
                if !is_black(pixel_rgb(&surface, px, py)) {
                    any_lit = true;
                }
            }
        }
        assert!(
            any_lit,
            "text should draw at least one non-black pixel on the software surface"
        );
    }

    /// Vertical extent (in rows) of any non-black pixel in a surface — a proxy
    /// for rendered glyph height.
    fn lit_height(surface: &Surface) -> u32 {
        let (w, h) = surface.size();
        let mut top: Option<u32> = None;
        let mut bottom = 0u32;
        for py in 0..h {
            for px in 0..w {
                if !is_black(pixel_rgb(surface, px, py)) {
                    top.get_or_insert(py);
                    bottom = py;
                }
            }
        }
        top.map_or(0, |t| bottom - t + 1)
    }

    /// Count vertically-lit pixels in column `px` (a proxy for stroke thickness).
    fn lit_rows_in_column(surface: &Surface, px: u32) -> u32 {
        let (_, h) = surface.size();
        (0..h)
            .filter(|&py| !is_black(pixel_rgb(surface, px, py)))
            .count() as u32
    }

    #[test]
    fn line_width_thickens_the_stroke() {
        // A width-6 horizontal line must light several rows; a hairline lights
        // ~1. (AA may add a faint fringe row, so compare thick vs thin.)
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let thin = render_frame(
            new_black_surface(),
            vec![DrawCommand::Line {
                x1: 5,
                y1: 30,
                x2: 58,
                y2: 30,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                width: 1,
            }],
            &mut fonts,
        );
        let thick = render_frame(
            new_black_surface(),
            vec![DrawCommand::Line {
                x1: 5,
                y1: 30,
                x2: 58,
                y2: 30,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                width: 8,
            }],
            &mut fonts,
        );
        let thin_rows = lit_rows_in_column(&thin, 30);
        let thick_rows = lit_rows_in_column(&thick, 30);
        assert!(
            thick_rows >= 6,
            "width-8 line should light ~8 rows, got {thick_rows}"
        );
        assert!(
            thick_rows > thin_rows + 3,
            "thick ({thick_rows}) should clearly exceed thin ({thin_rows})"
        );
    }

    #[test]
    fn circle_edge_is_antialiased() {
        // A white disc on black must have partially-lit edge pixels (grays),
        // not a hard black/white boundary — proof of coverage antialiasing.
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let surface = render_frame(
            new_black_surface(),
            vec![DrawCommand::Circle {
                cx: 30,
                cy: 30,
                radius: 18,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }],
            &mut fonts,
        );
        assert!(
            is_white(pixel_rgb(&surface, 30, 30)),
            "the disc center should be filled"
        );
        // Somewhere along the edge ring there is a gray (partially covered) pixel.
        let mut found_gray = false;
        for py in 0..64 {
            for px in 0..64 {
                let (r, _, _) = pixel_rgb(&surface, px, py);
                if r > 40 && r < 215 {
                    found_gray = true;
                }
            }
        }
        assert!(
            found_gray,
            "an antialiased edge should leave gray coverage pixels"
        );
    }

    #[test]
    fn rounded_rect_clips_its_corners() {
        // A rounded rect must leave its corners unpainted (background shows
        // through) while its center is filled. A square rect would paint the
        // corner too.
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let surface = render_frame(
            new_black_surface(),
            vec![DrawCommand::Rect {
                x: 0,
                y: 0,
                w: 40,
                h: 40,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                radius: 12,
            }],
            &mut fonts,
        );
        assert!(
            is_black(pixel_rgb(&surface, 1, 1)),
            "the rounded corner should be unpainted, got {:?}",
            pixel_rgb(&surface, 1, 1)
        );
        assert!(
            is_white(pixel_rgb(&surface, 20, 20)),
            "the center should be filled"
        );
        // A point along the flat top edge (past the corner radius) is filled.
        assert!(
            is_white(pixel_rgb(&surface, 20, 1)),
            "the flat edge should be filled"
        );
    }

    #[test]
    fn alpha_blends_over_background() {
        // A 50%-opacity white rect over black should composite to mid-gray, not
        // overwrite to full white — proof that per-primitive alpha blends.
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let surface = render_frame(
            new_black_surface(),
            vec![DrawCommand::Rect {
                x: 0,
                y: 0,
                w: 20,
                h: 20,
                r: 255,
                g: 255,
                b: 255,
                a: 128,
                radius: 0,
            }],
            &mut fonts,
        );
        let px = pixel_rgb(&surface, 5, 5);
        assert!(
            px.0 > 100 && px.0 < 160,
            "alpha-blended white over black should be ~gray, got {px:?}"
        );
        assert!(
            px.0 == px.1 && px.1 == px.2,
            "blend should stay neutral gray, got {px:?}"
        );
    }

    #[test]
    fn text_size_is_honored() {
        // A larger `size` must render taller glyphs than a smaller one — the
        // ladder picks a bigger font rung. With size ignored (one baked font)
        // both would be identical and this fails.
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");

        let big = 48u16;
        let small = 12u16;
        let mut make = |size| {
            let mut surface = Surface::new(96, 96, PixelFormatEnum::RGB888).unwrap();
            surface.fill_rect(None, Color::RGB(0, 0, 0)).unwrap();
            render_frame(
                surface,
                vec![DrawCommand::plain_text(
                    "Hg", 4, 4, size, 255, 255, 255, 255,
                )],
                &mut fonts,
            )
        };

        let big_h = lit_height(&make(big));
        let small_h = lit_height(&make(small));
        assert!(big_h > 0 && small_h > 0, "both sizes should draw glyphs");
        assert!(
            big_h > small_h + 8,
            "size {big} glyphs ({big_h}px tall) should be clearly taller than size {small} ({small_h}px)"
        );
    }

    #[test]
    fn offscreen_canvas_composites_onto_main() {
        // Draw a white rect into an offscreen canvas, then blit that canvas
        // onto the main framebuffer at an offset. Only the painted region
        // should appear; the rest of the framebuffer stays black (the canvas
        // is transparent where nothing was drawn).
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");

        let surface = render_frame(
            new_black_surface(),
            vec![
                DrawCommand::CreateCanvas {
                    id: 1,
                    w: 16,
                    h: 16,
                },
                DrawCommand::SetTarget { id: 1 },
                // Fill a 6x6 white block in the canvas's top-left.
                DrawCommand::Rect {
                    x: 0,
                    y: 0,
                    w: 6,
                    h: 6,
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                    radius: 0,
                },
                DrawCommand::SetTarget { id: 0 },
                // Blit the canvas onto the main framebuffer at (20, 20).
                DrawCommand::DrawCanvas {
                    id: 1,
                    x: 20,
                    y: 20,
                    a: 255,
                    w: 0,
                    h: 0,
                },
            ],
            &mut fonts,
        );

        // The blitted block should be white at (22, 22).
        assert!(
            is_white(pixel_rgb(&surface, 22, 22)),
            "offscreen canvas content should composite onto the main framebuffer"
        );
        // A pixel just outside the 6x6 block (canvas was transparent there) must
        // remain black — the offscreen canvas must not paint a full opaque rect.
        assert!(
            is_black(pixel_rgb(&surface, 30, 30)),
            "transparent regions of the offscreen canvas should not overwrite the framebuffer"
        );
        // And the area the canvas did NOT cover at all stays black.
        assert!(
            is_black(pixel_rgb(&surface, 2, 2)),
            "untouched framebuffer stays black"
        );
    }

    /// Horizontal extent (in columns) of any non-black pixel — a proxy for the
    /// rendered width of a text run.
    fn lit_width(surface: &Surface) -> u32 {
        let (w, h) = surface.size();
        let mut left: Option<u32> = None;
        let mut right = 0u32;
        for px in 0..w {
            for py in 0..h {
                if !is_black(pixel_rgb(surface, px, py)) {
                    left.get_or_insert(px);
                    right = px;
                }
            }
        }
        left.map_or(0, |l| right - l + 1)
    }

    /// Render one text command on a fresh black surface and return its width.
    fn text_run_width(fonts: &mut FontBook, cmd: DrawCommand) -> u32 {
        lit_width(&render_frame(new_black_surface(), vec![cmd], fonts))
    }

    #[test]
    fn weight_italic_and_spacing_change_the_pixels() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let text = |weight, italic, spacing| DrawCommand::Text {
            text: "nnn".to_string(),
            x: 4,
            y: 4,
            size: 24,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            font: None,
            weight,
            italic,
            spacing,
        };

        let regular = text_run_width(&mut fonts, text(400, false, 0.0));
        let bold = text_run_width(&mut fonts, text(700, false, 0.0));
        let italic = text_run_width(&mut fonts, text(400, true, 0.0));
        let spaced = text_run_width(&mut fonts, text(400, false, 6.0));

        assert!(regular > 0, "the regular run must draw something");
        // Synthetic emboldening and shearing both widen the run; letter-spacing
        // adds 6px between each of the three glyphs.
        assert!(
            bold > regular,
            "bold ({bold}px) should be wider than regular ({regular}px)"
        );
        assert!(
            italic > regular,
            "italic ({italic}px) should be wider than regular ({regular}px)"
        );
        assert!(
            spaced >= regular + 10,
            "6px letter-spacing over 3 glyphs should widen the run well past {regular}px, got {spaced}px"
        );
    }

    #[test]
    fn a_named_role_selects_a_different_face() {
        // `mono` is a real second face on a machine that has one; the run has to
        // come out a different width than the proportional default. Where no
        // fixed-pitch font exists the role degrades to the default face, which
        // is also correct — so that machine is skipped rather than failing.
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        if !fonts.has_role("mono") {
            return;
        }
        let run = |face: Option<&str>| DrawCommand::Text {
            text: "illll".to_string(),
            x: 2,
            y: 2,
            size: 24,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            font: face.map(str::to_string),
            weight: 400,
            italic: false,
            spacing: 0.0,
        };

        let default = text_run_width(&mut fonts, run(None));
        let mono = text_run_width(&mut fonts, run(Some("mono")));
        let unknown = text_run_width(&mut fonts, run(Some("No Such Face (petal test)")));
        assert!(
            mono > default,
            "narrow glyphs are wider in a fixed-pitch face: mono {mono}px vs default {default}px"
        );
        assert_eq!(
            unknown, default,
            "a face this host lacks falls back to the default"
        );
    }

    /// Off-ladder sizes draw at exactly that size, so the run's ink is as wide
    /// as `text_width` (advance ratios × the asked-for size) says. Snapping 11,
    /// 13 and 15 to the 10/12/14 rungs drew them 7–10% narrower than measured,
    /// and whole-pixel glyph advances drifted ~3% even on a rung.
    #[test]
    fn off_ladder_sizes_draw_as_wide_as_they_measure() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let ratios = fonts.default_face().ascii_advance_ratios();
        let text = "The quick brown fox jumps over the lazy dog again";
        for size in [11u16, 13, 15, 17, 22, 37] {
            let measured: f64 = text.chars().map(|c| ratios[c as usize] * size as f64).sum();
            let mut surface = Surface::new(1400, 80, PixelFormatEnum::RGB888).unwrap();
            surface.fill_rect(None, Color::RGB(0, 0, 0)).unwrap();
            let cmd = DrawCommand::plain_text(text, 4, 4, size, 255, 255, 255, 255);
            let drawn = lit_width(&render_frame(surface, vec![cmd], &mut fonts)) as f64;
            // Within a couple of pixels over the whole line: the ink sits a
            // little inside the advance box at either end. Drawing at the
            // nearest rung instead was 20+px short at 13.
            assert!(
                (drawn - measured).abs() <= 2.0,
                "size {size}: drew {drawn}px but measures {measured:.1}px"
            );
            assert!(
                fonts.default_face().sizes().contains(&size),
                "size {size} should have been baked on demand"
            );
        }
    }

    /// Pair-heavy runs draw as wide as they measure. `text_width` sums
    /// per-glyph advances, so while rungs kerned, "AVAVAVAVAVAVAVAV" in
    /// Helvetica drew 124px at 13 against a 138.5px measurement (and every
    /// ellipsize/wrap/flow decision built on that width was off with it).
    #[test]
    fn kerned_pairs_draw_as_wide_as_they_measure() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let ratios = fonts.default_face().ascii_advance_ratios();
        for text in ["AVAVAVAVAVAVAVAV", "WAVE To Yo LT AV", "VAVAVAVA"] {
            for size in [13u16, 24, 48, 64] {
                let measured: f64 = text.chars().map(|c| ratios[c as usize] * size as f64).sum();
                let mut surface = Surface::new(1400, 100, PixelFormatEnum::RGB888).unwrap();
                surface.fill_rect(None, Color::RGB(0, 0, 0)).unwrap();
                let cmd = DrawCommand::plain_text(text, 4, 4, size, 255, 255, 255, 255);
                let drawn = lit_width(&render_frame(surface, vec![cmd], &mut fonts)) as f64;
                // Ink sits a little inside the advance box at either end (the
                // "T"/"L"/"W" side bearings); kerning was off by 10%.
                let slack = 2.0 + size as f64 * 0.03;
                assert!(
                    (drawn - measured).abs() <= slack,
                    "{text:?} at {size}: drew {drawn}px but measures {measured:.1}px"
                );
            }
        }
    }

    /// A size past the on-demand range draws with the nearest rung rather than
    /// baking an enormous font, and size 0 draws nothing rather than panicking.
    #[test]
    fn out_of_range_sizes_fall_back_to_the_nearest_rung() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = load_test_fonts(&ttf).expect("a system font for tests");
        let before = fonts.default_face().sizes();
        let huge = crate::font::MAX_RUNG_SIZE + 1;
        for size in [0u16, huge] {
            let cmd = DrawCommand::plain_text("x", 0, 0, size, 255, 255, 255, 255);
            let _ = render_frame(new_black_surface(), vec![cmd], &mut fonts);
        }
        assert_eq!(fonts.default_face().sizes(), before);
    }
}
