//! The terminal frontend: a crossterm TUI in the controlling terminal, so
//! Garden can serve as `EDITOR="garden --term"`.
//!
//! The terminal reports its size in cells; the frontend presents that to the
//! core as a virtual viewport of [`grid::CELL`] logical pixels per cell, then
//! rasterizes each frame's `Scene` back onto the character grid (see
//! [`grid`]). Keys map straight onto [`vim::Key`]; mouse clicks, drags, and
//! wheel scrolling map to cell centers in logical pixels.
//!
//! Ctrl+Q force-quits regardless of editor state (raw mode swallows Ctrl+C,
//! and Cmd shortcuts don't reach most terminals). The debug server works here
//! too; its `/screenshot` answers with the rendered grid as plain text.

use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::{cursor, execute, queue, style, terminal};
use garden_render::Color;
use serde_json::json;

use crate::app::{App, ClickCounter, Mods, Viewport};
use crate::clipboard::SystemClipboard;
use crate::debug::{self, DebugCmd, Reply};
use crate::frontend::grid::{self, Grid, CELL};
use crate::frontend::{AppConfig, Frontend, RELOAD_POLL};
use crate::vim;

/// How long one `event::poll` waits before the loop services the debug
/// channel and the script reload poll.
const INPUT_POLL: Duration = Duration::from_millis(50);

pub struct TerminalFrontend;

impl Frontend for TerminalFrontend {
    fn run(self: Box<Self>, config: AppConfig) -> Result<(), String> {
        let _guard = TermGuard::enter().map_err(|err| format!("terminal setup failed: {err}"))?;
        run_loop(config).map_err(|err| format!("terminal frontend failed: {err}"))
    }
}

fn run_loop(config: AppConfig) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let viewport = Viewport {
        size: logical_size(cols, rows),
        cell: CELL,
        scale: 1.0,
    };
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

    let (tx, rx) = mpsc::channel();
    if let Some(port) = config.debug_port {
        // Messages to stderr would scribble on the alternate screen, so the
        // bound port is only observable via the state endpoints themselves.
        debug::spawn(port, tx.clone())?;
    }
    drop(tx); // the server keeps its own clones; an unused channel just runs dry

    let mut out = io::BufWriter::new(io::stdout());
    let mut last_reload = Instant::now();
    let mut clicks = ClickCounter::new();

    loop {
        if app.take_redraw() {
            draw(&mut out, &current_grid(&app)?)?;
        }
        // The terminal frontend is single-window: closing the window ends the
        // process.
        if app.should_quit() || app.should_close() {
            return Ok(());
        }

        // One blocking poll, then drain whatever else is already queued so a
        // key flurry becomes a single repaint.
        let mut pending = event::poll(INPUT_POLL)?;
        while pending {
            if !handle_event(&mut app, &mut clicks, event::read()?) {
                return Ok(());
            }
            pending = event::poll(Duration::ZERO)?;
        }

        while let Ok(request) = rx.try_recv() {
            // The TUI is a single window with the fixed ordinal 1; a
            // `?window=<n>` selector for anything else has no target.
            let result = match request.window {
                Some(n) if n != 1 => Err(format!("no window with ordinal {n}")),
                _ => match request.cmd {
                    DebugCmd::Screenshot => {
                        // The same settle-then-capture contract as the windowed
                        // and headless frontends, so the text grid reflects all
                        // previously injected input (see App::settle_panels).
                        app.settle_panels();
                        Ok(Reply::Text(current_grid(&app)?.to_text()))
                    }
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

        // The TUI can't create OS windows; drain the intent into an error.
        if app.take_new_window_request() {
            app.set_status_error("E: new window requires the windowed frontend");
        }

        if last_reload.elapsed() >= RELOAD_POLL {
            app.poll_script();
            app.poll_files();
            app.poll_lsp();
            app.poll_script_clients();
            app.tick_panels(); // panels animate at the poll cadence here
            app.poll_event_log();
            last_reload = Instant::now();
        }
    }
}

fn logical_size(cols: u16, rows: u16) -> (f32, f32) {
    (cols as f32 * CELL.0, rows as f32 * CELL.1)
}

/// The current frame rasterized at the terminal's present size.
fn current_grid(app: &App) -> io::Result<Grid> {
    let (cols, rows) = terminal::size()?;
    Ok(grid::rasterize(
        &app.build_scene(),
        cols as usize,
        rows as usize,
    ))
}

/// Feed one crossterm event into the core. Returns false to force-quit.
fn handle_event(app: &mut App, clicks: &mut ClickCounter, ev: Event) -> bool {
    match ev {
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            // The terminal escape hatch: raw mode disables Ctrl+C and Cmd+Q
            // never reaches us, so Ctrl+Q always quits.
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
                return false;
            }
            if let Some((vkey, mods)) = translate_key(key) {
                app.apply_key(vkey, mods);
            }
        }
        Event::Mouse(mouse) => handle_mouse(app, clicks, mouse),
        Event::Resize(cols, rows) => {
            let (w, h) = logical_size(cols, rows);
            app.set_viewport_size(w, h);
        }
        _ => {}
    }
    true
}

/// Crossterm key → the toolkit-independent key + modifiers the core consumes.
/// Shifted characters already arrive uppercase, so SHIFT only matters for
/// named keys and chord shortcuts.
fn translate_key(key: KeyEvent) -> Option<(vim::Key, Mods)> {
    use vim::Key as V;
    let vkey = match key.code {
        KeyCode::Char(c) => V::Char(c),
        KeyCode::Enter => V::Enter,
        KeyCode::Tab => V::Tab,
        KeyCode::Backspace => V::Backspace,
        KeyCode::Delete => V::Delete,
        KeyCode::Esc => V::Escape,
        KeyCode::Left => V::Left,
        KeyCode::Right => V::Right,
        KeyCode::Up => V::Up,
        KeyCode::Down => V::Down,
        KeyCode::Home => V::Home,
        KeyCode::End => V::End,
        KeyCode::PageUp => V::PageUp,
        KeyCode::PageDown => V::PageDown,
        _ => return None,
    };
    let mods = Mods {
        // SUPER only arrives from terminals speaking the kitty keyboard
        // protocol; elsewhere Cmd shortcuts simply don't exist in a TUI.
        cmd: key.modifiers.contains(KeyModifiers::SUPER),
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
        alt: key.modifiers.contains(KeyModifiers::ALT),
    };
    Some((vkey, mods))
}

fn handle_mouse(app: &mut App, clicks: &mut ClickCounter, mouse: MouseEvent) {
    // The center of the hovered cell, in the core's logical pixels.
    let x = (mouse.column as f32 + 0.5) * CELL.0;
    let y = (mouse.row as f32 + 0.5) * CELL.1;
    // A terminal reports no Cmd/Super for mouse events, so the jump-to-code
    // gesture is Ctrl-click here — which is moot in practice, since the canvas
    // it acts on is a GPU panel that `--term` cannot render anyway.
    let mods = crate::app::Mods {
        shift: mouse.modifiers.contains(KeyModifiers::SHIFT),
        ctrl: mouse.modifiers.contains(KeyModifiers::CONTROL),
        alt: mouse.modifiers.contains(KeyModifiers::ALT),
        cmd: false,
    };
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let count = clicks.click(x, y, Instant::now());
            app.mouse_down(x, y, mods, count);
        }
        MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => app.mouse_moved(x, y),
        MouseEventKind::Up(MouseButton::Left) => app.mouse_up(),
        // The context gesture, for panel scripts (see `App::mouse_down_right`).
        // A right drag carries no meaning of its own, so it only tracks the
        // pointer, exactly as a plain move does.
        MouseEventKind::Down(MouseButton::Right) => app.mouse_down_right(x, y),
        MouseEventKind::Drag(MouseButton::Right) => app.mouse_moved(x, y),
        MouseEventKind::Up(MouseButton::Right) => app.mouse_up_right(),
        MouseEventKind::ScrollUp => {
            app.mouse_moved(x, y);
            app.handle_scroll(-3.0);
        }
        MouseEventKind::ScrollDown => {
            app.mouse_moved(x, y);
            app.handle_scroll(3.0);
        }
        MouseEventKind::ScrollLeft => app.handle_scroll_h(-3.0),
        MouseEventKind::ScrollRight => app.handle_scroll_h(3.0),
        _ => {}
    }
}

/// Repaint the whole grid. Color changes are only emitted on transitions, so
/// a typical frame is a handful of escape sequences per row.
fn draw(out: &mut impl Write, grid: &Grid) -> io::Result<()> {
    let mut fg: Option<Color> = None;
    let mut bg: Option<Color> = None;
    for row in 0..grid.rows {
        queue!(out, cursor::MoveTo(0, row as u16))?;
        for col in 0..grid.cols {
            let cell = grid.get(col, row);
            if bg != Some(cell.bg) {
                queue!(out, style::SetBackgroundColor(term_color(cell.bg)))?;
                bg = Some(cell.bg);
            }
            if fg != Some(cell.fg) {
                queue!(out, style::SetForegroundColor(term_color(cell.fg)))?;
                fg = Some(cell.fg);
            }
            queue!(out, style::Print(cell.ch))?;
        }
    }
    out.flush()
}

fn term_color(c: Color) -> style::Color {
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    style::Color::Rgb {
        r: to_u8(c.r),
        g: to_u8(c.g),
        b: to_u8(c.b),
    }
}

/// Puts the terminal into raw mode + the alternate screen on entry and
/// restores it on drop — including on panic, so a crash never leaves the
/// shell unusable.
struct TermGuard;

impl TermGuard {
    fn enter() -> io::Result<TermGuard> {
        terminal::enable_raw_mode()?;
        if let Err(err) = execute!(
            io::stdout(),
            terminal::EnterAlternateScreen,
            terminal::DisableLineWrap,
            event::EnableMouseCapture,
            cursor::Hide,
        ) {
            let _ = terminal::disable_raw_mode();
            return Err(err);
        }
        Ok(TermGuard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            cursor::Show,
            event::DisableMouseCapture,
            terminal::EnableLineWrap,
            terminal::LeaveAlternateScreen,
        );
        let _ = terminal::disable_raw_mode();
    }
}
