//! `FpsHost` — petal-fps's delta on top of the `petal-sdl` game loop.
//!
//! The loop (SDL window, event pump, timing, agent/headless/screenshot/record
//! modes, hot reload, pointer grab) comes from `petal-sdl`. This host supplies
//! only the two axes petal-fps varies:
//!
//!   - the **renderer**: a software z-buffered framebuffer (`framebuffer.rs`)
//!     uploaded through a streaming texture (`renderer.rs`), and
//!   - the **native set**: the `triangle3d` family (`native_fns.rs`) instead of
//!     the `petal-ui` draw vocabulary. Input/timing natives still come from
//!     `petal_ui::input`, so mouselook (`mouse_dx`/`grab_mouse`) works for free.
//!
//! The camera, projection, and scene all live in Petal (see `examples/`).

use image::{Rgb, RgbImage};
use serde::Serialize;
use serde_json::Value as JsonValue;

use petal::env::Env;
use petal::stack::StackKey;
use petal_sdl::Host;
use sdl2::render::Canvas;
use sdl2::video::Window;

use crate::commands::{take_draw_commands, take_draw_commands_for, DrawCommand};
use crate::framebuffer::Framebuffer;
use crate::renderer::Presenter;

pub struct FpsHost {
    width: u32,
    height: u32,
    /// Persistent framebuffer for the live window: pixels accumulate across
    /// frames unless the sketch clears them (via `clear3d`/`sky_gradient`).
    fb: Framebuffer,
    /// Streaming-texture presenter, built lazily from the loop's canvas.
    presenter: Option<Presenter>,
}

impl FpsHost {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, fb: Framebuffer::new(width, height), presenter: None }
    }
}

impl Host for FpsHost {
    fn register(&mut self, env: &mut Env) {
        // Input, timing, dimensions, mouselook (relative mouse + grab) — all the
        // standard interactivity — come from petal-ui. Only the 3D draw
        // vocabulary is ours.
        petal_ui::input::register_input(env);
        crate::native_fns::register_draw(env);
    }

    fn present(&mut self, canvas: &mut Canvas<Window>, env: &mut Env) -> Result<(), String> {
        let commands = take_draw_commands(env);
        if !commands.is_empty() {
            self.fb.execute(&commands);
        }
        if self.presenter.is_none() {
            self.presenter = Some(Presenter::new(canvas, self.width, self.height)?);
        }
        self.presenter.as_mut().unwrap().present(canvas, &self.fb)
    }

    fn render_image(
        &mut self,
        env: &mut Env,
        stack: StackKey,
        width: u32,
        height: u32,
    ) -> Result<RgbImage, String> {
        // A fresh framebuffer per capture: a screenshot is one self-contained
        // frame (3D scenes clear each frame), not the accumulated window buffer.
        let commands = take_draw_commands_for(env, stack);
        let mut fb = Framebuffer::new(width, height);
        fb.execute(&commands);
        let mut img = RgbImage::from_pixel(width, height, Rgb([0, 0, 0]));
        for (i, px) in fb.color.chunks_exact(3).enumerate() {
            let x = (i as u32) % width;
            let y = (i as u32) / width;
            img.put_pixel(x, y, Rgb([px[0], px[1], px[2]]));
        }
        Ok(img)
    }

    fn draw_commands_json(&mut self, env: &mut Env, stack: StackKey) -> JsonValue {
        let commands = take_draw_commands_for(env, stack);
        serde_json::to_value(&commands).unwrap_or(JsonValue::Null)
    }

    fn draw_stats(&mut self, env: &mut Env, stack: StackKey) -> Option<JsonValue> {
        let commands = take_draw_commands_for(env, stack);
        serde_json::to_value(compute_stats(&commands)).ok()
    }
}

/// Per-frame draw statistics, including the approximate depth range across all
/// 3D vertices — a cheap way for an agent to sanity-check a scene without a PNG.
#[derive(Serialize, Default)]
pub struct DrawStats {
    pub total: usize,
    pub triangles: usize,
    pub lines: usize,
    pub rects: usize,
    pub circles: usize,
    pub texts: usize,
    pub z_min: Option<f32>,
    pub z_max: Option<f32>,
}

pub fn compute_stats(commands: &[DrawCommand]) -> DrawStats {
    let mut s = DrawStats { total: commands.len(), ..Default::default() };
    let mut min_z: Option<f32> = None;
    let mut max_z: Option<f32> = None;
    let mut track = |z: f32| {
        min_z = Some(min_z.map_or(z, |v| v.min(z)));
        max_z = Some(max_z.map_or(z, |v| v.max(z)));
    };
    for c in commands {
        match c {
            DrawCommand::Triangle3d { z1, z2, z3, .. } => {
                s.triangles += 1;
                track(*z1);
                track(*z2);
                track(*z3);
            }
            DrawCommand::Triangle3dShaded { z1, z2, z3, .. } => {
                s.triangles += 1;
                track(*z1);
                track(*z2);
                track(*z3);
            }
            DrawCommand::Line3d { z1, z2, .. } => {
                s.lines += 1;
                track(*z1);
                track(*z2);
            }
            DrawCommand::Line2d { .. } => s.lines += 1,
            DrawCommand::Rect2d { .. } => s.rects += 1,
            DrawCommand::Circle2d { .. } => s.circles += 1,
            DrawCommand::Text2d { .. } => s.texts += 1,
            _ => {}
        }
    }
    s.z_min = min_z;
    s.z_max = max_z;
    s
}
