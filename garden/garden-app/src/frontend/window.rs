//! The windowed frontend: a winit event loop presenting through the wgpu
//! [`Renderer`]. This is the default way to run Garden.
//!
//! Debug requests cross from the server threads into the event loop as winit
//! user events (`EventLoopProxy<DebugRequest>` implements
//! [`debug::RequestSink`]) and are answered against the live [`App`].

use std::sync::Arc;
use std::time::Instant;

use garden_render::{FrameOutcome, Renderer};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::window::{Window, WindowId};

use crate::app::{App, ClickCounter, Mods, Viewport};
use crate::clipboard::{SharedClipboard, SystemClipboard};
use crate::debug::{self, DebugCmd, DebugRequest, Reply, RequestSink};
use crate::frontend::menu::MenuBar;
use crate::frontend::registry::WindowRegistry;
use crate::frontend::{AppConfig, Frontend, RELOAD_POLL};
use crate::panel_view::PANEL_FRAME;
use crate::vim;

pub struct WindowFrontend;

impl Frontend for WindowFrontend {
    fn run(self: Box<Self>, config: AppConfig) -> Result<(), String> {
        let event_loop = EventLoop::<DebugRequest>::with_user_event()
            .build()
            .map_err(|err| format!("failed to create event loop: {err}"))?;
        event_loop.set_control_flow(ControlFlow::Wait);

        if let Some(port) = config.debug_port {
            let port = debug::spawn(port, event_loop.create_proxy())
                .map_err(|err| format!("failed to start debug server on port {port}: {err}"))?;
            eprintln!("garden: debug server on http://127.0.0.1:{port}");
        }

        let mut handler = Handler {
            config: Some(config),
            windows: WindowRegistry::new(),
            next_ordinal: 1,
            // One clipboard for the whole process: every window's `App` gets
            // a clone, so yanks cross windows even via the in-process
            // fallback when no OS pasteboard is reachable.
            clipboard: SharedClipboard::new(Box::new(SystemClipboard::new())),
            menu: None,
        };
        event_loop
            .run_app(&mut handler)
            .map_err(|err| format!("event loop error: {err}"))
    }
}

impl RequestSink for EventLoopProxy<DebugRequest> {
    fn send(&self, request: DebugRequest) -> bool {
        self.send_event(request).is_ok()
    }
}

struct Handler {
    /// Moved into the first `WindowState` when the startup window is created.
    config: Option<AppConfig>,
    windows: WindowRegistry<WindowState>,
    /// Next window ordinal to hand out. 1-based, monotonically increasing, and
    /// never reused — the stable human/test-facing handle the debug server's
    /// `?window=<n>` selector addresses (winit's `WindowId` is opaque and not
    /// re-creatable from outside).
    next_ordinal: u64,
    /// The process-wide clipboard; each window's `App` receives a clone.
    clipboard: SharedClipboard,
    /// Native macOS File/Edit menu bar (a no-op stub elsewhere). The menu bar
    /// is per-process on macOS, so exactly one `MenuBar` exists — installed
    /// when the first window is created and drained once per `about_to_wait`
    /// tick, dispatching to the focused window.
    menu: Option<MenuBar>,
}

struct WindowState {
    // Declared before `window` so dropping a WindowState tears down the
    // renderer (and its surface) before the window Arc. Renderer orders its
    // own fields for the same hazard; keep this struct safe regardless.
    renderer: Renderer,
    window: Arc<Window>,
    app: App,
    /// This window's 1-based session ordinal (see `Handler::next_ordinal`);
    /// the debug server addresses it as `?window=<ordinal>`.
    ordinal: u64,
    modifiers: Modifiers,
    /// Double/triple-click detection for left presses.
    clicks: ClickCounter,
    /// When the last `render` skipped its frame because the surface was
    /// unavailable (occluded, asleep), the time at which to retry — throttled to
    /// the poll cadence so an occluded window never spins the redraw loop (the
    /// overnight 20 GB leak). `None` once a frame presents successfully.
    retry_surface_at: Option<Instant>,
}

impl WindowState {
    fn logical_size(window: &Window) -> (f32, f32) {
        let size = window.inner_size().to_logical::<f32>(window.scale_factor());
        (size.width, size.height)
    }

    fn mods(&self) -> Mods {
        let m = self.modifiers.state();
        Mods {
            cmd: m.super_key(),
            ctrl: m.control_key(),
            shift: m.shift_key(),
            alt: m.alt_key(),
        }
    }

    fn handle_key(&mut self, event: KeyEvent) {
        let Some(key) = to_vim_key(&event.logical_key) else {
            return;
        };
        let mods = self.mods();
        self.app.apply_key(key, mods);
    }

    /// Push the core's redraw flag out to the window. Called after every
    /// event that reached the core; the quit/close flags are the Handler's
    /// job (see [`Handler::reap`]).
    fn sync(&mut self) {
        if self.app.take_redraw() {
            self.window.request_redraw();
        }
    }
}

impl Handler {
    /// Build a window, its GPU renderer, and its `App` core, and register it
    /// (the new window takes focus). `resumed` calls this once at startup;
    /// the multi-window spawner will call it for each additional window.
    fn create_window(&mut self, event_loop: &ActiveEventLoop, config: AppConfig) {
        let attrs = Window::default_attributes()
            .with_title("Garden")
            .with_inner_size(LogicalSize::new(1280.0, 850.0));
        // On macOS, let the content extend under a transparent, slim title bar
        // so the custom titlebar (drawn in the scene) reads as one unified
        // surface with the panes, with the traffic-light controls floating over
        // it. Other platforms keep their native decorations.
        #[cfg(target_os = "macos")]
        let attrs = {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true)
        };
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        #[cfg(target_os = "macos")]
        crate::frontend::macos_icon::install();
        // One GPU device for the whole process: the first window creates the
        // wgpu context, every later window shares it (Renderer::with_context)
        // instead of opening another device. A context that can't configure
        // the new surface only cancels this spawn, never the process.
        let shared = self
            .windows
            .iter_mut()
            .next()
            .map(|(_, state)| state.renderer.gpu_context());
        let renderer = match shared {
            None => {
                let renderer = Renderer::new(window.clone());
                // Report the adapter once, when the process-wide device is
                // created. A `Cpu` device type here means wgpu found no
                // hardware adapter and fell back to software rasterization —
                // the app still runs and looks identical, so without this line
                // the only symptom is that everything (smooth scrolling most
                // of all, since it repaints on every wheel event) is slow.
                eprintln!("garden: gpu {}", renderer.adapter_description());
                renderer
            }
            Some(context) => match Renderer::with_context(&context, window.clone()) {
                Ok(renderer) => renderer,
                Err(err) => {
                    eprintln!("garden: could not open a new window: {err}");
                    return;
                }
            },
        };
        let viewport = Viewport {
            size: WindowState::logical_size(&window),
            cell: renderer.cell_size(),
            scale: renderer.scale_factor(),
        };
        let mut app = App::new(
            config.script,
            config.fallback_layout,
            config.script_owns_layout,
            viewport,
            Box::new(self.clipboard.clone()),
        );
        app.set_event_log(config.event_log);
        app.set_recents(config.recents);
        app.set_save_as_paths(config.save_as_paths);
        // Only here: a native picker needs a desktop session and an event loop
        // to hand control back to, which `--term` and `--headless` lack.
        app.enable_native_dialogs();
        // The windowed frontend draws its own slim titlebar across the top.
        #[cfg(target_os = "macos")]
        app.enable_titlebar();
        // Petal-IDE mode reserves the toolbar band below it (all platforms).
        if let Some(target) = config.ide_target {
            app.enable_ide(target, crate::petal_ide_ir_view_path());
        }
        // The menu bar is installed once, for the first window only: on macOS
        // it is process-global (NSApp's menu), so a second `MenuBar::new`
        // would tear down and replace the installed menu rather than add one.
        if self.menu.is_none() {
            self.menu = Some(MenuBar::new());
        }
        let ordinal = self.next_ordinal;
        self.next_ordinal += 1;
        self.windows.insert(
            window.id(),
            WindowState {
                renderer,
                window,
                app,
                ordinal,
                modifiers: Modifiers::default(),
                clicks: ClickCounter::new(),
                retry_surface_at: None,
            },
        );
    }

    /// Act on a window core's quit/close flags after it processed an event or
    /// poll: `should_quit` ends the whole process (all windows), while
    /// `should_close` tears down just this window — the process exits when
    /// the last one goes.
    fn reap(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        let Some(state) = self.windows.get_mut(id) else {
            return;
        };
        if state.app.should_quit() {
            event_loop.exit();
        } else if state.app.should_close() {
            self.close_window(event_loop, id);
        }
    }

    /// Tear down one window (dropping its `WindowState` — the field order
    /// drops the renderer before the window Arc); the process exits when the
    /// last window goes.
    fn close_window(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        self.windows.remove(id);
        if self.windows.is_empty() {
            event_loop.exit();
        }
    }

    /// The `GET /windows` reply: every open window by ordinal, which is
    /// focused, and its pane count — ordered by ordinal so the listing is
    /// stable across calls.
    fn windows_json(&mut self) -> serde_json::Value {
        let focused = self.windows.focused_id();
        let mut rows: Vec<(u64, serde_json::Value)> = self
            .windows
            .iter_mut()
            .map(|(id, state)| {
                (
                    state.ordinal,
                    serde_json::json!({
                        "window": state.ordinal,
                        "focused": Some(id) == focused,
                        "panes": state.app.pane_count(),
                    }),
                )
            })
            .collect();
        rows.sort_by_key(|(ordinal, _)| *ordinal);
        let windows: Vec<serde_json::Value> = rows.into_iter().map(|(_, row)| row).collect();
        serde_json::json!({ "ok": true, "windows": windows })
    }

    /// Fulfill a core's pending new-window intent (`:windownew`, File ▸ New
    /// Window): the frontend-independent App can only raise a flag, so this
    /// drains it and spawns one fresh window — its own config, script `Env`,
    /// window id, and event log ([`crate::new_window_config`]) — on the
    /// event-loop thread, sharing the existing GPU context. Called after every
    /// dispatch that can run a command (window event, debug user event, poll
    /// tick), mirroring [`reap`](Handler::reap).
    fn fulfill_new_window(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        let requested = self
            .windows
            .get_mut(id)
            .is_some_and(|state| state.app.take_new_window_request());
        if requested {
            self.create_window(event_loop, crate::new_window_config());
        }
    }
}

impl ApplicationHandler<DebugRequest> for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.windows.is_empty() {
            return;
        }
        let config = self.config.take().expect("app config");
        self.create_window(event_loop, config);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // Events that act on the registry rather than on one window's core.
        match event {
            WindowEvent::Focused(true) => {
                self.windows.set_focused(id);
                return;
            }
            WindowEvent::CloseRequested => {
                // The native close button closes just this window.
                self.close_window(event_loop, id);
                return;
            }
            _ => {}
        }
        let Some(state) = self.windows.get_mut(id) else {
            return; // a window we no longer track
        };
        match event {
            WindowEvent::Resized(size) => {
                state.renderer.resize(size.width, size.height);
                let (w, h) = WindowState::logical_size(&state.window);
                state.app.set_viewport_size(w, h);
            }
            WindowEvent::ModifiersChanged(m) => state.modifiers = m,
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                state.handle_key(event);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = position.to_logical::<f32>(state.window.scale_factor());
                state.app.mouse_moved(p.x, p.y);
            }
            WindowEvent::MouseInput {
                state: pressed,
                button: MouseButton::Left,
                ..
            } => match pressed {
                ElementState::Pressed => {
                    let (x, y) = state.app.mouse();
                    let mods = state.mods();
                    let clicks = state.clicks.click(x, y, Instant::now());
                    state.app.mouse_down(x, y, mods, clicks);
                }
                ElementState::Released => state.app.mouse_up(),
            },
            // The right button is the context gesture: it reaches panel scripts
            // as `petal-ui` button 1 and does nothing anywhere else. It is
            // deliberately not fed through `ClickCounter` — a context menu has
            // no double-click meaning, and counting these presses would let a
            // right click poison the *left* button's multi-click chain.
            WindowEvent::MouseInput {
                state: pressed,
                button: MouseButton::Right,
                ..
            } => match pressed {
                ElementState::Pressed => {
                    let (x, y) = state.app.mouse();
                    state.app.mouse_down_right(x, y);
                }
                ElementState::Released => state.app.mouse_up_right(),
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let (cell_w, cell_h) = state.app.viewport().cell;
                // Wheel motion in cells, kept **fractional**. A trackpad (and a
                // high-resolution wheel) reports pixel deltas — many small ones
                // per gesture, with the OS's own inertia already applied —
                // and rounding each to a whole cell on arrival both quantized
                // the motion and discarded every delta below half a cell, which
                // is what made slow scrolling stutter and then stall. A notched
                // wheel reports lines instead: three rows per notch, as before.
                let (cols, lines) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-x * 3.0, -y * 3.0),
                    MouseScrollDelta::PixelDelta(p) => {
                        (-(p.x as f32) / cell_w, -(p.y as f32) / cell_h)
                    }
                };
                if lines != 0.0 {
                    state.app.handle_scroll(lines);
                }
                if cols != 0.0 {
                    state.app.handle_scroll_h(cols);
                }
            }
            WindowEvent::RedrawRequested => {
                let scene = state.app.build_scene();
                state.retry_surface_at = match state.renderer.render(&scene) {
                    // Surface unavailable (occluded / display asleep): retry on
                    // the poll cadence instead of re-requesting a redraw now,
                    // which would busy-loop. See `about_to_wait`.
                    FrameOutcome::Skipped => Some(Instant::now() + RELOAD_POLL),
                    FrameOutcome::Presented => None,
                };
            }
            _ => {}
        }
        state.sync();
        self.reap(event_loop, id);
        self.fulfill_new_window(event_loop, id);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, request: DebugRequest) {
        // The registry keeps focus on some window whenever it is non-empty, but
        // fall back explicitly in case no focus event has arrived yet.
        if self.windows.focused_id().is_none() {
            let any = self.windows.iter_mut().next().map(|(id, _)| id);
            if let Some(any) = any {
                self.windows.set_focused(any);
            }
        }

        // `/windows` lists the whole registry, so it is answered before a
        // single target window is resolved.
        if let DebugCmd::Windows = request.cmd {
            let _ = request.reply.send(Ok(Reply::Json(self.windows_json())));
            return;
        }

        // Every other command targets one window: the `?window=<ordinal>`
        // selector when given, else the focused window (single-window default).
        let id = match request.window {
            Some(ordinal) => {
                let found = self
                    .windows
                    .iter_mut()
                    .find(|(_, s)| s.ordinal == ordinal)
                    .map(|(id, _)| id);
                match found {
                    Some(id) => id,
                    None => {
                        let _ = request
                            .reply
                            .send(Err(format!("no window with ordinal {ordinal}")));
                        return;
                    }
                }
            }
            None => match self.windows.focused_id() {
                Some(id) => id,
                None => {
                    let _ = request.reply.send(Err("window not ready yet".to_string()));
                    return;
                }
            },
        };
        let state = self.windows.get_mut(id).expect("resolved window present");
        let result = match request.cmd {
            DebugCmd::Screenshot { pane } => {
                // The consistency contract (same as the headless frontend):
                // settle panel frames first so the capture reflects all
                // previously injected input — two user events (a /key then
                // this) can arrive back-to-back with no about_to_wait tick
                // between them. `capture` then renders the scene into its own
                // offscreen texture, so it never races the live surface.
                state.app.settle_panels();
                match state.app.pane_capture_rect(pane) {
                    Err(err) => Err(err),
                    Ok(crop) => {
                        let scene = state.app.build_scene();
                        let cap = state.renderer.capture(&scene);
                        let scale = state.app.viewport().scale;
                        // `?pane=<n>` crops to that pane's rect — no tab strip,
                        // no status bar, no gutter.
                        let (w, h, rgba) = match crop {
                            Some(rect) => {
                                debug::crop_rgba(cap.width, cap.height, &cap.rgba, rect, scale)
                            }
                            None => (cap.width, cap.height, cap.rgba),
                        };
                        Ok(Reply::Png {
                            png: debug::encode_png(w, h, &rgba),
                            frame: state.app.frame(),
                        })
                    }
                }
            }
            cmd => state.app.handle_debug(cmd),
        };
        let _ = request.reply.send(result);
        state.sync();
        self.reap(event_loop, id);
        // A `:windownew` injected via /key (or a /menu NewWindow) should spawn
        // now, not on the next poll tick.
        self.fulfill_new_window(event_loop, id);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Native menu clicks arrive on one process-global channel (muda's
        // `MenuEvent::receiver()`), so the drain happens once per tick at the
        // Handler level, and every action goes to the focused window's core.
        // With no windows left the actions are dropped. The normal poll loop
        // below then picks up the resulting redraw/quit/close flags.
        if let Some(menu) = &self.menu {
            let actions = menu.drain();
            if let Some(state) = self.windows.focused_mut() {
                for action in actions {
                    state.app.dispatch_menu(action);
                }
            }
        }
        // Poll every window; the loop's wake-up is the minimum any of them asks
        // for. Windows whose cores signal quit/close are reaped afterwards
        // (collecting ids first keeps the iteration borrow simple).
        let mut next = RELOAD_POLL;
        let mut done: Vec<WindowId> = Vec::new();
        // New-window intents (e.g. a native File ▸ New Window click drained
        // above) are collected during iteration and fulfilled after it, so
        // spawning never mutates the registry mid-borrow (same pattern as
        // `done`).
        let mut wants_window: Vec<WindowId> = Vec::new();
        for (id, state) in self.windows.iter_mut() {
            state.app.poll_script();
            state.app.poll_files();
            state.app.poll_lsp();
            state.app.poll_script_clients();
            state.app.poll_event_log();
            // Drive panel animation. While any panel is awake, keep a ~60fps
            // tick; otherwise fall back to the slow reload-poll cadence so an
            // idle editor stays event-driven. See `crate::panel_view`.
            if state.app.tick_panels() {
                next = next.min(PANEL_FRAME);
            }
            state.sync();
            // Retry a frame the surface refused (occluded / display asleep). A
            // requested redraw is dispatched immediately, so we only issue it
            // once its throttle deadline passes — bounding retries to the poll
            // cadence rather than spinning. The redraw re-arms the timer if the
            // surface is still unavailable, and clears it once a frame presents.
            if let Some(at) = state.retry_surface_at {
                let now = Instant::now();
                if now >= at {
                    state.retry_surface_at = None;
                    state.window.request_redraw();
                } else {
                    next = next.min(at - now);
                }
            }
            if state.app.take_new_window_request() {
                wants_window.push(id);
            }
            if state.app.should_quit() || state.app.should_close() {
                done.push(id);
            }
        }
        for id in done {
            self.reap(event_loop, id);
        }
        // Each drained request spawns exactly one window (the flag was already
        // taken above, so this is the poll-tick half of `fulfill_new_window`).
        for _ in wants_window {
            self.create_window(event_loop, crate::new_window_config());
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + next));
    }
}

/// Translate a winit logical key into the toolkit-independent key the app
/// core consumes. Returns `None` for keys the editor does not handle.
fn to_vim_key(key: &WinitKey) -> Option<vim::Key> {
    use vim::Key as V;
    Some(match key {
        WinitKey::Character(s) => V::Char(s.chars().next()?),
        WinitKey::Named(named) => match named {
            NamedKey::Space => V::Char(' '),
            NamedKey::Enter => V::Enter,
            NamedKey::Tab => V::Tab,
            NamedKey::Backspace => V::Backspace,
            NamedKey::Delete => V::Delete,
            NamedKey::Escape => V::Escape,
            NamedKey::ArrowLeft => V::Left,
            NamedKey::ArrowRight => V::Right,
            NamedKey::ArrowUp => V::Up,
            NamedKey::ArrowDown => V::Down,
            NamedKey::Home => V::Home,
            NamedKey::End => V::End,
            NamedKey::PageUp => V::PageUp,
            NamedKey::PageDown => V::PageDown,
            _ => return None,
        },
        _ => return None,
    })
}
