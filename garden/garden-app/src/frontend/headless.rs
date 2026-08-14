//! The headless frontend: no window, no terminal — the debug server is the
//! only way to drive and observe the editor. Used for integration testing
//! (`tools/integration-test.ts`) and agent-driven verification, where a
//! real window would be slow, flaky, or unavailable.
//!
//! The loop blocks on the debug-request channel with a short timeout so the
//! layout script is still polled for hot reloads. Cell metrics come from
//! [`garden_render::cell_metrics`] (CPU-only font shaping), so all layout
//! math matches the windowed frontend exactly. `/screenshot` works too: a
//! surface-less [`HeadlessRenderer`] is created lazily on the first request
//! and renders the scene offscreen.

use std::sync::mpsc;

use garden_render::HeadlessRenderer;

use crate::app::{App, Viewport};
use crate::clipboard::SystemClipboard;
use serde_json::json;

use crate::debug::{self, DebugCmd, Reply};
use crate::frontend::{AppConfig, Frontend, RELOAD_POLL};

/// Logical size of the virtual viewport, matching the default window size.
const SIZE: (f32, f32) = (1280.0, 850.0);

/// The viewport size for this headless run: `GARDEN_HEADLESS_SIZE=WxH`
/// (logical pixels, e.g. `700x850`) overrides the default — integration tests
/// use it to exercise narrow/wide layouts, since headless has no window to
/// resize and the debug server has no resize endpoint.
fn viewport_size() -> (f32, f32) {
    let Ok(spec) = std::env::var("GARDEN_HEADLESS_SIZE") else {
        return SIZE;
    };
    let parse = |s: &str| s.trim().parse::<f32>().ok().filter(|v| *v >= 100.0);
    match spec.split_once('x').map(|(w, h)| (parse(w), parse(h))) {
        Some((Some(w), Some(h))) => (w, h),
        _ => {
            eprintln!("garden: ignoring malformed GARDEN_HEADLESS_SIZE={spec:?} (want WxH)");
            SIZE
        }
    }
}

pub struct HeadlessFrontend;

impl Frontend for HeadlessFrontend {
    fn run(self: Box<Self>, config: AppConfig) -> Result<(), String> {
        let port = config
            .debug_port
            .ok_or("headless mode needs the debug server; pass --debug-port <n>")?;

        let size = viewport_size();
        let viewport = Viewport {
            size,
            cell: garden_render::cell_metrics(),
            scale: 1.0,
        };
        // SystemClipboard is lazy and fault-tolerant: with no pasteboard
        // available it degrades to an in-process clipboard, never crashing
        // a headless session.
        let mut app = App::new(
            config.script,
            config.fallback_layout,
            config.script_owns_layout,
            viewport,
            Box::new(SystemClipboard::new()),
        );
        app.set_event_log(config.event_log);
        app.set_recents(config.recents);
        app.set_save_as_paths(config.save_as_paths);
        // Match the windowed frontend's chrome so screenshots taken here
        // reflect what the real window shows.
        #[cfg(target_os = "macos")]
        app.enable_titlebar();
        // Petal-IDE mode reserves the toolbar band below it (all platforms).
        if let Some(target) = config.ide_target {
            app.enable_ide(target, crate::petal_ide_ir_view_path());
        }

        let (tx, rx) = mpsc::channel();
        let port = debug::spawn(port, tx)
            .map_err(|err| format!("failed to start debug server on port {port}: {err}"))?;
        eprintln!("garden: headless, debug server on http://127.0.0.1:{port}");

        // Created on the first /screenshot; a missing GPU only disables
        // screenshots, not the whole session.
        let mut renderer: Option<Result<HeadlessRenderer, String>> = None;

        loop {
            match rx.recv_timeout(RELOAD_POLL) {
                Ok(request) => {
                    // Headless is a single window with the fixed ordinal 1; a
                    // `?window=<n>` selector for anything else has no target.
                    let result = match request.window {
                        Some(n) if n != 1 => Err(format!("no window with ordinal {n}")),
                        _ => match request.cmd {
                            DebugCmd::Screenshot => screenshot(&mut app, &mut renderer),
                            DebugCmd::Windows => Ok(Reply::Json(json!({
                                "ok": true,
                                "windows": [{
                                    "window": 1,
                                    "focused": true,
                                    "panes": app.pane_count(),
                                }],
                            }))),
                            cmd => app.handle_debug(cmd),
                        },
                    };
                    let _ = request.reply.send(result);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                // The accept thread holds its sink for the process lifetime,
                // so this only happens if it died.
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            app.poll_script();
            app.poll_files();
            app.poll_processes();
            app.poll_lsp();
            app.poll_script_clients();
            app.tick_panels(); // panels animate at the poll cadence here
            app.poll_event_log();
            let _ = app.take_redraw(); // nothing to present
                                       // Headless can't create OS windows; drain the intent into an error.
            if app.take_new_window_request() {
                app.set_status_error("E: new window requires the windowed frontend");
            }
            // Headless is single-window: closing the window ends the process.
            if app.should_quit() || app.should_close() {
                break;
            }
        }
        Ok(())
    }
}

fn screenshot(
    app: &mut App,
    renderer: &mut Option<Result<HeadlessRenderer, String>>,
) -> Result<Reply, String> {
    let renderer = renderer
        .get_or_insert_with(|| HeadlessRenderer::new(app.viewport().size, app.viewport().scale))
        .as_mut()
        .map_err(|err| format!("screenshot unavailable: {err}"))?;
    // The consistency contract (same as the windowed frontend): settle panel
    // frames first so the capture reflects all previously injected input —
    // the loop below answers requests *before* its tick_panels call, so a
    // panel's cached commands may otherwise lag queued input by a frame.
    app.settle_panels();
    let scene = app.build_scene();
    let cap = renderer.capture(&scene);
    Ok(Reply::Png {
        png: debug::encode_png(cap.width, cap.height, &cap.rgba),
        frame: app.frame(),
    })
}
