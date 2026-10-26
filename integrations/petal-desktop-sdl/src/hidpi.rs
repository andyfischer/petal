//! HiDPI drawing: scripts lay out in logical points, the framebuffer is
//! painted in physical pixels.
//!
//! A window built with `allow_highdpi()` has a drawable larger than its size
//! on a Retina screen (2× on most Macs). Scripts keep seeing the window size
//! (`bind_dimensions`) and SDL keeps delivering mouse, touch and wheel input
//! in the same points, so nothing a script does changes. Only the painting
//! changes: a host paints a framebuffer at [`Scale::pixels`] and runs the
//! frame's commands through [`scale_commands`] first, which multiplies every
//! coordinate, length and text size by the scale. Text is then baked at its
//! shown resolution rather than drawn at 1× and magnified by the compositor.
//!
//! `PETAL_UI_SCALE` forces the scale (`PETAL_UI_SCALE=2`), for seeing a
//! Retina frame on a 1× screen: the framebuffer is painted at that scale and
//! the window shows it fitted, which is also what a snapshot saves.

use petal_ui::draw::DrawCommand;
use sdl2::render::Canvas;
use sdl2::video::Window;

/// The environment variable that overrides the measured scale.
pub const SCALE_ENV: &str = "PETAL_UI_SCALE";

/// A window's logical size and the scale its framebuffer is painted at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    /// The window's size in points: what scripts lay out in.
    pub logical: (u32, u32),
    /// Physical pixels per point.
    pub factor: f32,
}

impl Scale {
    /// The framebuffer size: the logical size times the factor.
    pub fn pixels(&self) -> (u32, u32) {
        let px = |v: u32| ((v as f32 * self.factor).round() as u32).max(1);
        (px(self.logical.0), px(self.logical.1))
    }

    /// Measure `canvas`'s window: drawable pixels over window points, unless
    /// [`SCALE_ENV`] overrides it. Measure again after a resize or a move to
    /// another display, since either can change it.
    pub fn of(canvas: &Canvas<Window>) -> Scale {
        let logical = canvas.window().size();
        let drawable = canvas.output_size().unwrap_or(logical);
        let factor = scale_override().unwrap_or_else(|| measured(logical, drawable));
        Scale { logical, factor }
    }
}

/// Pixels per point from a window's size and its drawable size. The width
/// decides; 1 when the window has no size yet.
pub fn measured(logical: (u32, u32), drawable: (u32, u32)) -> f32 {
    if logical.0 == 0 {
        return 1.0;
    }
    drawable.0 as f32 / logical.0 as f32
}

/// [`SCALE_ENV`] as a scale, when it is set to a positive number.
pub fn scale_override() -> Option<f32> {
    parse_override(std::env::var(SCALE_ENV).ok()?.as_str())
}

fn parse_override(v: &str) -> Option<f32> {
    v.trim().parse::<f32>().ok().filter(|s| s.is_finite() && *s > 0.0 && *s <= 8.0)
}

/// A frame's commands, drawn in points, as commands in pixels at `factor`
/// (see [`petal_ui::compose::scale`]). At 1 the list is returned untouched.
pub fn scale_commands(commands: Vec<DrawCommand>, factor: f32) -> Vec<DrawCommand> {
    if factor == 1.0 {
        return commands;
    }
    commands.into_iter().map(|c| petal_ui::compose::scale(c, factor)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_scale_is_drawable_over_window() {
        assert_eq!(measured((1440, 900), (2880, 1800)), 2.0);
        assert_eq!(measured((1440, 900), (1440, 900)), 1.0);
        assert_eq!(measured((0, 0), (0, 0)), 1.0);
    }

    #[test]
    fn override_accepts_positive_numbers_only() {
        assert_eq!(parse_override("2"), Some(2.0));
        assert_eq!(parse_override(" 1.5 "), Some(1.5));
        assert_eq!(parse_override("0"), None);
        assert_eq!(parse_override("-1"), None);
        assert_eq!(parse_override("retina"), None);
        assert_eq!(parse_override("NaN"), None);
    }

    #[test]
    fn pixels_scale_the_logical_size() {
        let s = Scale { logical: (1440, 900), factor: 2.0 };
        assert_eq!(s.pixels(), (2880, 1800));
        let s = Scale { logical: (101, 51), factor: 1.5 };
        assert_eq!(s.pixels(), (152, 77));
    }

    #[test]
    fn scale_one_leaves_commands_alone() {
        let cmds = vec![DrawCommand::plain_text("a", 1, 2, 13, 255, 255, 255, 255)];
        assert_eq!(scale_commands(cmds.clone(), 1.0), cmds);
        match &scale_commands(cmds, 2.0)[0] {
            DrawCommand::Text { size, .. } => assert_eq!(*size, 26),
            other => panic!("{other:?}"),
        }
    }
}
