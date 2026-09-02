//! Standalone demo of the garden-render crate.
//!
//! Opens a window and draws two editor-like "panes": background quads, a
//! focused-pane border built from thin quads, a cursor quad, and ~30 lines of
//! monospace text laid out with `Renderer::cell_size()` metrics.
//!
//! Run with `cargo run -p garden-render --example demo`. Close the window or
//! press Cmd+Q to exit. Set `GARDEN_DEMO_EXIT_AFTER_FRAMES=N` to render N
//! frames and exit automatically (used for smoke testing).

use std::sync::Arc;
use std::time::{Duration, Instant};

use garden_render::{Color, FrameOutcome, Primitive, Rect, Renderer, Scene, TextStyle, FONT_SIZE};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState};
use winit::window::{Window, WindowId};

const BG: Color = Color::rgb(0.10, 0.11, 0.13);
const PANE_BG: Color = Color::rgb(0.13, 0.14, 0.17);
const BORDER: Color = Color::rgb(0.36, 0.55, 0.93);
const GUTTER_FG: Color = Color::rgba(0.6, 0.65, 0.75, 0.5);
const TEXT_FG: Color = Color::rgb(0.85, 0.87, 0.91);
const CURSOR: Color = Color::rgba(0.95, 0.75, 0.30, 0.9);

/// How long to wait before retrying after the surface was unavailable. The
/// renderer never re-requests its own redraw, so retrying on a timer keeps an
/// occluded window from spinning an unthrottled redraw loop.
const SURFACE_RETRY: Duration = Duration::from_millis(100);

struct Demo {
    state: Option<DemoState>,
    modifiers: ModifiersState,
    frames_rendered: u64,
    exit_after_frames: Option<u64>,
    /// When the surface was unavailable, when to ask for the next redraw.
    retry_surface_at: Option<Instant>,
}

struct DemoState {
    window: Arc<Window>,
    renderer: Renderer,
}

/// Build the demo scene: two panes with backgrounds, a focused border,
/// gutter + text lines, and a cursor quad.
fn build_scene(state: &DemoState) -> Scene {
    let scale = state.renderer.scale_factor();
    let size = state.window.inner_size().to_logical::<f32>(scale);
    let (cell_w, cell_h) = state.renderer.cell_size();

    let mut primitives = Vec::new();
    let margin = 12.0;
    let gap = 8.0;
    let pane_w = (size.width - 2.0 * margin - gap) / 2.0;
    let pane_h = size.height - 2.0 * margin;
    let panes = [
        Rect::new(margin, margin, pane_w, pane_h),
        Rect::new(margin + pane_w + gap, margin, pane_w, pane_h),
    ];

    for (pane_idx, pane) in panes.iter().enumerate() {
        primitives.push(Primitive::Quad {
            rect: *pane,
            color: PANE_BG,
        });

        // Focused-pane border: 4 thin quads around the first pane.
        if pane_idx == 0 {
            let t = 1.0;
            let Rect { x, y, w, h } = *pane;
            for rect in [
                Rect::new(x, y, w, t),
                Rect::new(x, y + h - t, w, t),
                Rect::new(x, y, t, h),
                Rect::new(x + w - t, y, t, h),
            ] {
                primitives.push(Primitive::Quad {
                    rect,
                    color: BORDER,
                });
            }
        }

        // ~15 lines of text per pane, with a line-number gutter.
        let pad = 8.0;
        let gutter_w = 3.0 * cell_w + pad;
        let clip = Rect::new(pane.x + 1.0, pane.y + 1.0, pane.w - 2.0, pane.h - 2.0);
        for line in 0..15 {
            let y = pane.y + pad + line as f32 * cell_h;
            primitives.push(Primitive::Text {
                pos: (pane.x + pad, y),
                text: format!("{:>3}", line + 1),
                color: GUTTER_FG,
                clip,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
            primitives.push(Primitive::Text {
                pos: (pane.x + pad + gutter_w, y),
                text: format!(
                    "pane {} line {:02}: fn render(scene: &Scene) {{ /* quads + glyphs */ }}",
                    pane_idx + 1,
                    line + 1
                ),
                color: TEXT_FG,
                clip,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
        }

        // A cursor quad in the focused pane.
        if pane_idx == 0 {
            primitives.push(Primitive::Quad {
                rect: Rect::new(
                    pane.x + pad + gutter_w + 14.0 * cell_w,
                    pane.y + pad + 4.0 * cell_h,
                    cell_w,
                    cell_h,
                ),
                color: CURSOR,
            });
        }
    }

    Scene { bg: BG, primitives }
}

impl ApplicationHandler for Demo {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("garden-render demo")
                        .with_inner_size(LogicalSize::new(960.0, 600.0)),
                )
                .expect("failed to create window"),
        );
        let renderer = Renderer::new(window.clone());
        let (cell_w, cell_h) = renderer.cell_size();
        println!(
            "garden-render demo: cell size {:.2} x {:.2} logical px, scale factor {}",
            cell_w,
            cell_h,
            renderer.scale_factor()
        );
        window.request_redraw();
        self.state = Some(DemoState { window, renderer });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Cmd+Q quits (winit windows have no default macOS menu).
                if event.state == ElementState::Pressed
                    && self.modifiers.super_key()
                    && matches!(event.logical_key.as_ref(), Key::Character("q"))
                {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                state.renderer.resize(size.width, size.height);
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let scene = build_scene(state);
                if state.renderer.render(&scene) == FrameOutcome::Skipped {
                    // Surface unavailable (occluded / display asleep): nothing
                    // was presented, so retry on a timer rather than counting
                    // the frame or re-requesting a redraw now, which would
                    // busy-loop. See `about_to_wait`.
                    self.retry_surface_at = Some(Instant::now() + SURFACE_RETRY);
                    return;
                }
                self.retry_surface_at = None;
                self.frames_rendered += 1;
                if let Some(limit) = self.exit_after_frames {
                    if self.frames_rendered >= limit {
                        println!("garden-render demo: rendered {limit} frames, exiting");
                        event_loop.exit();
                    } else {
                        state.window.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(at) = self.retry_surface_at else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        if Instant::now() >= at {
            self.retry_surface_at = None;
            event_loop.set_control_flow(ControlFlow::Wait);
            if let Some(state) = &self.state {
                state.window.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(at));
        }
    }
}

fn main() {
    let exit_after_frames = std::env::var("GARDEN_DEMO_EXIT_AFTER_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok());
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut demo = Demo {
        state: None,
        modifiers: ModifiersState::default(),
        frames_rendered: 0,
        exit_after_frames,
        retry_surface_at: None,
    };
    event_loop.run_app(&mut demo).expect("event loop error");
}
