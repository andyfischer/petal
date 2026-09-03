//! Key routing and text injection. Translated key presses from every frontend
//! (and the debug server's `/key`) enter through [`App::apply_key`], which
//! dispatches the `:` command line, the `Ctrl+W` window prefix, panel-pane
//! forwarding, the global Cmd/Ctrl editing shortcuts, and finally the vim
//! layer. Focus movement between panes lives here too.

use garden_script::{LayoutNode, NavIntent};

use crate::clipboard::Clipboard;
use crate::command_line::{CommandLine, CommandLineKind};
use crate::editor_view::EditorView;
use crate::vim::{self, Key};
use crate::window_nav::{self, Direction};

use super::{App, KeyOutcome, KeyPhase, Mods, Pane};

/// Upper bound on the files the fuzzy finder gathers, so a walk of a huge tree
/// stays bounded in time and memory. Comfortably covers a typical project.
const FILE_FINDER_LIMIT: usize = 20_000;

impl App {
    /// Apply one logical key press with the given modifiers. Shared by every
    /// frontend's input translation and debug-server `/key` injection.
    pub fn apply_key(&mut self, key: Key, mods: Mods) {
        self.apply_key_phase(key, mods, KeyPhase::Tap);
    }

    /// Apply one key in a given press [phase](KeyPhase). [`KeyPhase::Tap`] — a
    /// press and its release in one frame — is what every frontend delivers and
    /// what [`apply_key`](Self::apply_key) means.
    ///
    /// `Down`/`Up` exist for a **focused panel**: they let a driver hold a key
    /// across frames, so `key_down("w")` is observable from a later `GET /state`
    /// and a hold-to-move interaction is testable headless. Only a panel can
    /// observe held keys — over an editor or a process pane a `Down` acts like a
    /// tap and an `Up` is dropped, since neither has any notion of a held key.
    pub fn apply_key_phase(&mut self, key: Key, mods: Mods, phase: KeyPhase) {
        // Every key press is logged for replay (this is the single funnel for
        // all frontend input and debug injection); the event log buffers it.
        self.log_event("key", describe_key(key, mods));
        // Status messages are transient: whatever the last action reported is
        // shown until this key starts the next one. (A standing script error —
        // `script_error` — is not a message about an action and stays until
        // the script reloads cleanly.)
        if self.status_error.is_some() || self.status_note.is_some() {
            self.status_error = None;
            self.status_note = None;
            self.needs_redraw = true;
        }
        self.wake_panels();
        // A held key is only meaningful to a panel script; route it straight
        // there (still through the reserved-chord classification, so `Cmd+Q`
        // cannot be held instead of quitting). Anything else falls back to the
        // normal single-press dispatch.
        let panel_focused = self.panes.get(self.focus).is_some_and(Pane::is_panel);
        let modal = self.command_line.is_some() || self.file_finder.is_some();
        let outcome = match phase {
            KeyPhase::Tap => self.key_outcome(key, mods),
            _ if panel_focused && !modal => self.panel_key(key, mods, phase),
            KeyPhase::Down => self.key_outcome(key, mods),
            KeyPhase::Up => KeyOutcome::Ignored,
        };
        match outcome {
            KeyOutcome::Quit => {
                self.log_event("quit", "editor exiting");
                self.quit = true;
            }
            KeyOutcome::CloseWindow => {
                self.log_event("close-window", "window closing");
                self.close_window = true;
            }
            KeyOutcome::Handled | KeyOutcome::Ignored => {}
        }
    }

    fn key_outcome(&mut self, key: Key, mods: Mods) -> KeyOutcome {
        // An open `:` command line captures all input until it closes.
        if self.command_line.is_some() {
            return self.command_line_key(key);
        }

        // An open fuzzy file finder is modal too — it captures all input until
        // Enter opens the selection or Escape cancels.
        if self.file_finder.is_some() {
            return self.file_finder_key(key, mods);
        }

        // `Cmd`/`Ctrl`+`P` opens the fuzzy file finder. It is global (like the
        // command bar): it works in every vim mode and over a process pane, so
        // it is handled before pane-specific routing. Shift is excluded so it
        // does not shadow a future `Cmd+Shift+P`.
        if (mods.cmd || mods.ctrl) && !mods.shift && matches!(key, Key::Char('p' | 'P')) {
            self.open_file_finder();
            return KeyOutcome::Handled;
        }

        // A pending `Ctrl+W` consumes the next key as a window command (move or
        // cycle focus); any other key cancels the prefix without side effects.
        if std::mem::take(&mut self.window_cmd_pending) {
            return self.window_command_key(key);
        }
        // `Ctrl+W` starts a window command (vim's window prefix). It takes
        // precedence over the clipboard Ctrl aliases and the vim layer.
        if mods.ctrl && !mods.cmd && matches!(key, Key::Char('w' | 'W')) {
            self.window_cmd_pending = true;
            return KeyOutcome::Handled;
        }

        // A focused panel pane consumes plain keys (forwarded to its script);
        // the command bar and quit chords stay reserved, like a process pane.
        if self.panes.get(self.focus).is_some_and(Pane::is_panel) {
            return self.panel_key(key, mods, KeyPhase::Tap);
        }

        // Whether the focused pane rejects edits (the read-only "before" side of
        // a split review). Motions, selection, scrolling, search, and the command
        // bar all work; only buffer mutations are undone (below).
        let read_only = self.panes.get(self.focus).is_some_and(|p| p.read_only);

        let visible = self.focused_visible_lines();
        let cell_w = self.viewport.cell.0;
        let Some(pane) = self.panes.get_mut(self.focus) else {
            return KeyOutcome::Ignored;
        };
        let visible_cols = pane.view.visible_cols(pane.rect, cell_w);
        let view = &mut pane.view;
        // Capture the caret as it stands before this key, so any edit the key
        // triggers restores it on undo (e.g. `J` then `u`). Coalesced typing
        // runs keep their first character's position.
        view.buffer.set_pending_cursor(view.cursor);

        // Cmd shortcuts are global — they work in every vim mode.
        if mods.cmd {
            let Key::Char(c) = key else {
                return KeyOutcome::Ignored;
            };
            // A read-only pane rejects the mutating chords (save/cut/paste);
            // copy and select-all still work.
            if read_only && matches!(c.to_ascii_lowercase(), 's' | 'x' | 'v') {
                self.status_note = Some("read-only (before) pane — edit on the right".to_string());
                self.needs_redraw = true;
                return KeyOutcome::Handled;
            }
            match c.to_ascii_lowercase() {
                'q' => return KeyOutcome::Quit,
                // Cmd+W is the macOS close-window chord: it closes this OS
                // window (the whole window, panes and all — not a pane close),
                // while the process quits only via Cmd+Q / `:wqa`.
                'w' => return KeyOutcome::CloseWindow,
                's' if mods.shift => {
                    self.save_all();
                }
                's' => match view.save() {
                    Ok(()) => {
                        let name = view.display_name();
                        self.status_note = Some(format!("wrote {name}"));
                    }
                    Err(err) => self.status_error = Some(format!("save failed: {err}")),
                },
                'a' => view.select_all(),
                'z' if mods.shift => view.redo(),
                'z' => view.undo(),
                'c' => clipboard_copy(self.clipboard.as_mut(), view),
                'x' => clipboard_cut(self.clipboard.as_mut(), view),
                'v' => clipboard_paste(self.clipboard.as_mut(), view, visible, visible_cols),
                _ => return KeyOutcome::Ignored,
            }
            self.needs_redraw = true;
            return KeyOutcome::Handled;
        }

        // Ctrl+C/X/V/A/Q are global Mac-style editing shortcuts in every vim
        // mode — they override vim's Ctrl meanings for those keys (so no
        // Ctrl+V block-select or Ctrl+A increment). Every other Ctrl chord
        // (Ctrl+R redo, …) falls through to the vim layer below.
        if mods.ctrl {
            if let Key::Char(c) = key {
                if read_only && matches!(c.to_ascii_lowercase(), 'x' | 'v') {
                    self.status_note =
                        Some("read-only (before) pane — edit on the right".to_string());
                    self.needs_redraw = true;
                    return KeyOutcome::Handled;
                }
                let handled = match c.to_ascii_lowercase() {
                    'q' => return KeyOutcome::Quit,
                    'a' => {
                        view.select_all();
                        true
                    }
                    'c' => {
                        clipboard_copy(self.clipboard.as_mut(), view);
                        true
                    }
                    'x' => {
                        clipboard_cut(self.clipboard.as_mut(), view);
                        true
                    }
                    'v' => {
                        clipboard_paste(self.clipboard.as_mut(), view, visible, visible_cols);
                        true
                    }
                    _ => false,
                };
                if handled {
                    self.needs_redraw = true;
                    return KeyOutcome::Handled;
                }
            }
        }

        // Remaining Ctrl chords reach the vim layer as `Key::Ctrl` (e.g.
        // Ctrl+R redo): they are mode- and count-sensitive, so they belong in
        // vim's state machine. Ctrl + a non-character key means nothing yet.
        let key = match (mods.ctrl, key) {
            (true, Key::Char(c)) => Key::Ctrl(c),
            (true, _) => return KeyOutcome::Ignored,
            (false, key) => key,
        };

        // For a read-only pane, snapshot the text so any edit the vim layer makes
        // (a delete, a paste, an insert) can be rolled straight back — motions and
        // visual selection leave the text untouched and are kept.
        let ro_before = read_only.then(|| view.buffer.text());
        let action = vim::handle(view, key, visible, visible_cols, self.clipboard.as_mut());
        view.ensure_cursor_visible(visible, visible_cols);
        if let Some(why) = view.edit_refusal.take() {
            self.status_error = Some(why);
        }
        if let Some(before) = ro_before {
            // A read-only pane never stays in Insert mode…
            if view.vim.mode == crate::vim::Mode::Insert {
                vim::handle(
                    view,
                    Key::Escape,
                    visible,
                    visible_cols,
                    self.clipboard.as_mut(),
                );
            }
            // …nor keeps any text change.
            if view.buffer.text() != before {
                view.buffer = garden_core::Buffer::from_str(&before);
                view.cursor = view.buffer.clamp(view.cursor);
                view.anchor = None;
                self.status_note = Some("read-only (before) pane — edit on the right".to_string());
            }
        }
        match action {
            vim::Action::None => {}
            vim::Action::OpenCommandLine => self.command_line = Some(CommandLine::new()),
            vim::Action::OpenSearch { forward } => {
                self.command_line = Some(CommandLine::new_search(forward));
            }
            vim::Action::OpenDirectoryBrowser => {
                let dir = self.focused_browse_dir();
                self.open_directory_browser(&dir);
            }
        }
        self.needs_redraw = true;
        KeyOutcome::Handled
    }

    /// Whether the focused panel's script claimed this exact key + chord with
    /// `claim_key(name, mods)` on its last frame. A claim beats every host
    /// shortcut except quit — see [`classify_panel_key`].
    fn panel_claims_key(&self, key: Key, mods: Mods) -> bool {
        let Some(name) = panel_key_name(key) else {
            return false;
        };
        self.panes
            .get(self.focus)
            .and_then(|p| p.panel.as_ref())
            .is_some_and(|panel| panel.claims_key(&name, mods.bits()))
    }

    /// Route a key press to the focused panel pane. Reserved chords match a
    /// process pane ([`classify_process_key`]): `Cmd`/`Ctrl`+`Q` quits, `:` opens
    /// the command bar, other `Cmd`/`Ctrl` chords stay host-global **unless the
    /// script claimed them** with `claim_key(...)`; every other key is forwarded
    /// to the script under its canonical name. The panel is ticked immediately
    /// so the effect is visible at once.
    fn panel_key(&mut self, key: Key, mods: Mods, phase: KeyPhase) -> KeyOutcome {
        // A key the script claimed goes to the script and nowhere else — that
        // claim is the whole point (a panel whose bare letters are content has
        // no command keyspace otherwise), so it is tested before the region
        // editing chords and before the host globals.
        let claimed = self.panel_claims_key(key, mods);
        // A focused `text_view` region intercepts the editor selection chords —
        // Cmd/Ctrl+C copies its selection to the system clipboard, Cmd/Ctrl+A
        // selects all, Escape releases focus back to the script — before any
        // key reaches the panel script. Everything else falls through.
        // A claimed chord skips the focused region's own editing keys, and a
        // held key (`down`/`up`) is a script-level gesture that a region's vim
        // has no way to represent — both go straight to the classification below.
        if let Some(id) = self
            .focused_panel_region()
            .filter(|_| !claimed && phase == KeyPhase::Tap)
        {
            let editable = self
                .panes
                .get(self.focus)
                .and_then(|p| p.panel.as_ref())
                .and_then(|p| p.region_editable(id))
                .unwrap_or(false);
            if (mods.cmd || mods.ctrl) && matches!(key, Key::Char('c' | 'C')) {
                if let Some(view) = self
                    .panes
                    .get(self.focus)
                    .and_then(|p| p.panel.as_ref())
                    .and_then(|p| p.region_view(id))
                {
                    clipboard_copy(self.clipboard.as_mut(), view);
                }
                self.needs_redraw = true;
                return KeyOutcome::Handled;
            }
            if (mods.cmd || mods.ctrl) && matches!(key, Key::Char('a' | 'A')) {
                if let Some(panel) = self.focused_panel_mut() {
                    panel.region_select_all(id);
                }
                self.needs_redraw = true;
                return KeyOutcome::Handled;
            }
            // An **editable** region runs the real vim state machine on its
            // buffer: every non-global key (including printable text, motions,
            // and Escape-to-Normal) is fed to `vim::handle`. Escape only *leaves*
            // the region once it is already back in Normal mode, so the first
            // Escape exits Insert/Visual and a second hands focus to the script —
            // the familiar two-step. Ctrl chords are handled separately just
            // below, so the reserved ones still reach the host.
            if editable && !mods.cmd && !mods.ctrl {
                if matches!(key, Key::Escape)
                    && self
                        .panes
                        .get(self.focus)
                        .and_then(|p| p.panel.as_ref())
                        .and_then(|p| p.region_mode(id))
                        == Some(crate::vim::Mode::Normal)
                {
                    if let Some(panel) = self.focused_panel_mut() {
                        panel.clear_focused_region();
                    }
                    self.needs_redraw = true;
                    return KeyOutcome::Handled;
                }
                if self.region_vim_key(id, key) {
                    return KeyOutcome::Handled;
                }
            }
            // Vim's own Ctrl chords reach the region too, as `Key::Ctrl` — the
            // page scrolls (`Ctrl+D`/`U`/`F`/`B`), the line scrolls
            // (`Ctrl+E`/`Y`) and `Ctrl+R` redo. They are whitelisted rather
            // than forwarded wholesale because the chords Garden reserves are
            // exactly the ones a reviewer still needs mid-edit: `Ctrl+S` saves,
            // `Ctrl+W` is the window prefix, `Ctrl+[`/`]` walk the pane's
            // history, `Ctrl+P` opens the finder and `Ctrl+Q` quits. A region
            // that swallowed those would strand them inside the buffer.
            if editable && mods.ctrl && !mods.cmd {
                if let Key::Char(c) = key {
                    let c = c.to_ascii_lowercase();
                    if matches!(c, 'd' | 'u' | 'f' | 'b' | 'e' | 'y' | 'r')
                        && self.region_vim_key(id, Key::Ctrl(c))
                    {
                        return KeyOutcome::Handled;
                    }
                }
            }
            if !editable && matches!(key, Key::Escape) {
                if let Some(panel) = self.focused_panel_mut() {
                    panel.clear_focused_region();
                }
                self.needs_redraw = true;
                return KeyOutcome::Handled;
            }
            // Navigation keys scroll the focused region (like a real text view)
            // — but only while its content overflows its rect (see the
            // consume-or-forward rule in `panel_view.rs`). A non-scroll key, or
            // any key when the region has nothing to scroll, falls through to
            // the script below so keyboard navigation keeps working after a
            // click on selectable text.
            if !mods.cmd && !mods.ctrl {
                if let Some(name) = panel_key_name(key) {
                    let pane_rect = self.panes.get(self.focus).map(|p| p.rect);
                    let cell = self.viewport.cell;
                    if let (Some(rect), Some(panel)) = (pane_rect, self.focused_panel_mut()) {
                        if panel.region_scroll_key(id, rect, cell, &name) {
                            self.needs_redraw = true;
                            return KeyOutcome::Handled;
                        }
                    }
                }
            }
        }
        match classify_panel_key(key, mods, claimed) {
            PanelKey::Quit => KeyOutcome::Quit,
            PanelKey::CommandBar => {
                self.command_line = Some(CommandLine::new());
                self.needs_redraw = true;
                KeyOutcome::Handled
            }
            PanelKey::Forward(name) => {
                if let Some(panel) = self
                    .panes
                    .get_mut(self.focus)
                    .and_then(|p| p.panel.as_mut())
                {
                    panel.set_modifiers(mods);
                    // A chord carries no typed text: `text_input()` must be
                    // empty on the frame a panel handles `Ctrl+S`, or the first
                    // save types an "s" into the document.
                    let text = panel_key_text(key, mods);
                    match phase {
                        KeyPhase::Tap => panel.key(name, text),
                        KeyPhase::Down => panel.key_down(name, text),
                        KeyPhase::Up => panel.key_up(name),
                    }
                }
                self.tick_focused_panel();
                self.needs_redraw = true;
                KeyOutcome::Handled
            }
            PanelKey::NavBack => self.nav_focused_panel(NavIntent::Back),
            PanelKey::NavForward => self.nav_focused_panel(NavIntent::Forward),
            PanelKey::Ignore => KeyOutcome::Ignored,
        }
    }

    /// Feed one key into editable region `id` of the focused panel, running its
    /// vim state machine, and act on whatever host chrome the region asks for in
    /// return. `false` when the region could not be reached (no focused panel),
    /// which leaves the key to the caller's fallbacks.
    fn region_vim_key(&mut self, id: i64, key: Key) -> bool {
        let Some(rect) = self.panes.get(self.focus).map(|p| p.rect) else {
            return false;
        };
        let cell = self.viewport.cell;
        // Disjoint field borrows: the panel (in `self.panes`) and the
        // clipboard are separate fields, so both can be borrowed at once.
        let clip = self.clipboard.as_mut();
        let Some(panel) = self
            .panes
            .get_mut(self.focus)
            .and_then(|p| p.panel.as_mut())
        else {
            return false;
        };
        let action = panel.region_key(id, rect, cell, key, clip);
        let refused = panel.take_edit_refusal(id);
        self.status_error = refused;
        // A region's vim can ask for host chrome it cannot draw itself. `/` and
        // `?` are the ones that matter: they open the same search prompt a
        // normal pane gets, and `accept_search` sends the pattern back to the
        // region. Everything else stays the pane's business.
        if let Some(crate::vim::Action::OpenSearch { forward }) = action {
            self.command_line = Some(CommandLine::new_search(forward));
        }
        self.needs_redraw = true;
        true
    }

    /// Route a key to the open `:` / `/` / `?` line: edit the text, accept on
    /// Enter (run the ex command, or jump to the search pattern), or cancel
    /// on Escape. The cursor never moves while a search prompt is open (no
    /// incremental search), so cancelling leaves it where the search began.
    fn command_line_key(&mut self, key: Key) -> KeyOutcome {
        let Some(cl) = self.command_line.as_mut() else {
            return KeyOutcome::Ignored;
        };
        match key {
            Key::Escape => {
                self.command_line = None;
            }
            Key::Enter => {
                let cl = self.command_line.take().unwrap();
                match cl.kind {
                    CommandLineKind::Command => return self.run_command(cl.command()),
                    CommandLineKind::SearchForward => self.accept_search(cl.input, true),
                    CommandLineKind::SearchBackward => self.accept_search(cl.input, false),
                }
            }
            Key::Backspace => {
                if !cl.backspace() {
                    self.command_line = None; // backspacing past the prompt closes it
                }
            }
            Key::Char(c) => cl.push(c),
            _ => return KeyOutcome::Ignored,
        }
        self.needs_redraw = true;
        KeyOutcome::Handled
    }

    /// Open the fuzzy file finder over the focused pane's project. The project
    /// root is the nearest ancestor of the focused file's directory that holds
    /// a `.git` (else that directory itself); the file list is gathered once,
    /// here (via `git ls-files` in a repo, so `.gitignore` is honored), and
    /// filtered purely as the user types.
    pub(in crate::app) fn open_file_finder(&mut self) {
        let start = self.focused_browse_dir();
        let root = project_root(std::path::Path::new(&start));
        let files = crate::file_finder::gather_project_files(&root, FILE_FINDER_LIMIT);
        self.file_finder_root = root;
        self.file_finder = Some(crate::file_finder::FileFinder::new(files));
        self.needs_redraw = true;
    }

    /// Route a key to the open fuzzy file finder: type into the query, move the
    /// selection (arrows, or `Ctrl+N`/`Ctrl+P`), open the selected file on
    /// Enter, or cancel on Escape. Opening resolves the selected project-
    /// relative path against the gathered root and reuses [`App::open_path`], so
    /// it drops a focused browser back to an editor just like `:e`.
    fn file_finder_key(&mut self, key: Key, mods: Mods) -> KeyOutcome {
        let Some(ff) = self.file_finder.as_mut() else {
            return KeyOutcome::Ignored;
        };
        // Ctrl+N / Ctrl+P move the selection without touching the query, matching
        // the readline-style navigation common to fuzzy finders.
        if mods.ctrl {
            match key {
                Key::Char('n' | 'N') => ff.move_down(),
                Key::Char('p' | 'P') => ff.move_up(),
                _ => {}
            }
            self.needs_redraw = true;
            return KeyOutcome::Handled;
        }
        match key {
            Key::Escape => self.file_finder = None,
            Key::Up => ff.move_up(),
            Key::Down => ff.move_down(),
            Key::Backspace => {
                ff.backspace();
            }
            Key::Enter => {
                let selected = ff.selected_path().map(str::to_string);
                self.file_finder = None;
                if let Some(rel) = selected {
                    let path = self.file_finder_root.join(rel);
                    self.open_path(&path.to_string_lossy());
                }
            }
            Key::Char(c) => ff.push(c),
            _ => return KeyOutcome::Ignored,
        }
        self.needs_redraw = true;
        KeyOutcome::Handled
    }

    /// The second key of a `Ctrl+W` window command. `h`/`j`/`k`/`l` move focus
    /// to the neighboring pane in that direction; `w` cycles to the next pane;
    /// `o` expands the focused pane to fill the window (vim's `Ctrl+W o`,
    /// "only"); `s` splits it into a stacked pair and `v` into a side-by-side
    /// pair (vim's `Ctrl+W s` / `Ctrl+W v`); `c` closes the focused pane
    /// (refusing the last one, vim's `Ctrl+W c`) and `q` closes it too but
    /// closes the window when it is the last (vim's `Ctrl+W q`). The direction
    /// key may be
    /// pressed with or without Ctrl held, matching vim's `Ctrl+W h` and
    /// `Ctrl+W Ctrl+H`. Any other key is a no-op.
    fn window_command_key(&mut self, key: Key) -> KeyOutcome {
        let Key::Char(c) = key else {
            return KeyOutcome::Ignored;
        };
        if let Some(dir) = Direction::from_key(c.to_ascii_lowercase()) {
            self.focus_neighbor(dir);
        } else if c.eq_ignore_ascii_case(&'w') {
            self.focus_next_pane();
        } else if c.eq_ignore_ascii_case(&'o') {
            self.window_only();
        } else if c.eq_ignore_ascii_case(&'s') {
            self.window_split(true);
        } else if c.eq_ignore_ascii_case(&'v') {
            self.window_split(false);
        } else if c.eq_ignore_ascii_case(&'c') {
            return self.window_close(false);
        } else if c.eq_ignore_ascii_case(&'q') {
            return self.window_close(true);
        }
        KeyOutcome::Handled
    }

    /// Close the focused pane, removing its leaf from the layout tree and
    /// persisting the result (vim's `Ctrl+W c`/`q`; `:q` routes here too). On
    /// the last remaining pane: close this window when `close_if_last`
    /// (`Ctrl+W q`, `:q` — the process lives on if other windows do), else
    /// report vim's "cannot close last window" error (`Ctrl+W c`). Focus
    /// keeps its index, so it lands on the next pane in solver order (clamped
    /// by the rebuild when the closed pane was last). The closed pane's buffer
    /// is discarded without a dirty check, matching `:q`'s current semantics; a
    /// process pane's client is shut down by the drop.
    pub(in crate::app) fn window_close(&mut self, close_if_last: bool) -> KeyOutcome {
        if self.panes.len() <= 1 {
            if close_if_last {
                return KeyOutcome::CloseWindow;
            }
            self.status_error = Some("E444: cannot close last pane".to_string());
            self.needs_redraw = true;
            return KeyOutcome::Handled;
        }
        let mut tree = self.layout_from_panes();
        if window_nav::remove_leaf(&mut tree, self.focus) {
            self.apply_runtime_layout(tree);
        }
        KeyOutcome::Handled
    }

    /// `Ctrl+W s` / `Ctrl+W v` — split the focused pane in two, vim-style.
    /// `stacked` (`s`) stacks the new pane below the current one (a horizontal
    /// divider → a [`Column`](LayoutNode::Column)); otherwise (`v`) it sits to
    /// the right (a vertical divider → a [`Row`](LayoutNode::Row)). The new pane
    /// is an editor on the same file as the focused one (or empty when the
    /// focused pane is a process, so a split never spawns a duplicate process).
    ///
    /// Only the focused pane's slot in the layout tree is replaced — the rest of
    /// the layout is left intact — and the whole tree is persisted through
    /// [`App::apply_runtime_layout`] (the transient overlay when a script is
    /// loaded). Focus stays on the original pane.
    pub(in crate::app) fn window_split(&mut self, stacked: bool) {
        let Some(original) = self.focused_pane_node() else {
            return;
        };
        let new_pane = match &original {
            LayoutNode::Editor {
                file,
                line_numbers,
                wrap,
            } => LayoutNode::Editor {
                file: file.clone(),
                line_numbers: *line_numbers,
                wrap: *wrap,
            },
            _ => LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            },
        };
        let children = vec![original, new_pane];
        let split = if stacked {
            LayoutNode::Column {
                children,
                ratios: None,
            }
        } else {
            LayoutNode::Row {
                children,
                ratios: None,
            }
        };

        // Build the split onto the *live* layout (reconstructed from the panes),
        // not the script's possibly-stale snapshot, so a split never resurrects
        // outdated content for the panes it leaves alone (e.g. a sibling pane
        // that has since opened a different file or a browser).
        let mut tree = self.layout_from_panes();
        if window_nav::replace_leaf(&mut tree, self.focus, split) {
            self.apply_runtime_layout(tree);
        }
    }

    /// `Ctrl+W o` — make the focused pane the whole layout, closing the others.
    ///
    /// This is a *runtime layout change*: the focused pane is reconstructed as a
    /// single [`LayoutNode`] leaf (an editor on its file, or the process pane's
    /// command), which becomes the entire layout and is persisted through
    /// [`App::apply_runtime_layout`] (saved to the transient script when one is
    /// loaded). A no-op when there is nothing to collapse (zero or one pane).
    pub(in crate::app) fn window_only(&mut self) {
        if self.panes.len() <= 1 {
            return;
        }
        let Some(node) = self.focused_pane_node() else {
            return;
        };
        self.apply_runtime_layout(node);
    }

    /// Reconstruct the focused pane as a single [`LayoutNode`] leaf (see
    /// [`Pane::to_layout_node`]).
    pub(in crate::app) fn focused_pane_node(&self) -> Option<LayoutNode> {
        Some(self.panes.get(self.focus)?.to_layout_node())
    }

    /// Reconstruct the authoritative layout tree from the live panes: the active
    /// tree's structure and ratios, with each leaf replaced by the live pane's
    /// current content (in solver order). This is the single source of truth —
    /// any runtime change to what a pane shows (`:e`, `:E`, `:Git`, a browser
    /// opening a file, File ▸ New) is captured here, so the persisted layout
    /// never lags behind the panes on screen.
    pub(in crate::app) fn layout_from_panes(&self) -> LayoutNode {
        let mut tree = self.layout().clone();
        let mut leaves = self.panes.iter().map(Pane::to_layout_node);
        window_nav::rebuild_leaves(&mut tree, &mut leaves);
        tree
    }

    /// Persist the live panes as the authoritative layout *without* rebuilding
    /// them (they already reflect it). Call this after a runtime change that
    /// swapped a pane's content in place — `:e`, `:E`, `:Git`, a browser opening
    /// a file, File ▸ New — so the on-disk overlay (or, scriptless, the in-memory
    /// fallback) stays the source of truth and a later split/reload can't
    /// resurrect stale content. Quiet by design: unlike an explicit `Ctrl+W`
    /// layout command it shows no "layout saved" note. A save error surfaces in
    /// the status bar but is otherwise non-fatal (the panes are still correct).
    pub(in crate::app) fn sync_layout(&mut self) {
        let node = self.layout_from_panes();
        // A config-only script does not own the layout, so runtime content
        // changes stay in the in-memory fallback rather than rewriting init.ptl.
        if self.script_owns_layout {
            if let Some(script) = self.script.as_mut() {
                if let Err(err) = script.save_layout(&node) {
                    self.status_error = Some(format!("could not save layout: {err}"));
                }
            }
        } else {
            self.fallback_layout = node;
        }
    }

    /// Adopt `node` as the live layout and persist it. With a script loaded the
    /// change is written to the transient overlay (see
    /// [`ScriptHost::save_layout`](garden_script::ScriptHost::save_layout)) and
    /// the host re-points to watch it; without one (the plain-file/`$EDITOR`
    /// shape) it updates the in-memory fallback only. Either way the panes are
    /// rebuilt, reusing the surviving pane's buffer/cursor/process.
    pub(in crate::app) fn apply_runtime_layout(&mut self, node: LayoutNode) {
        self.log_event("layout", "runtime layout change");
        // Only a layout-owning script persists to its overlay; a config-only
        // script (file argument) keeps layout in the in-memory fallback, like
        // the scriptless `$EDITOR` shape.
        let owns = self.script_owns_layout;
        let script = self.script.as_mut().filter(|_| owns);
        if let Some(script) = script {
            match script.save_layout(&node) {
                Ok(path) => {
                    self.status_error = None;
                    self.status_note = Some(format!("layout saved to {}", path.display()));
                }
                Err(err) => {
                    self.status_error = Some(format!("could not save layout: {err}"));
                    self.needs_redraw = true;
                    return;
                }
            }
        } else {
            self.fallback_layout = node;
        }
        self.rebuild_panes();
        self.needs_redraw = true;
    }

    /// Move focus to the pane neighboring the focused one in `dir`, if any.
    fn focus_neighbor(&mut self, dir: Direction) {
        let rects: Vec<garden_render::Rect> = self.panes.iter().map(|p| p.rect).collect();
        if let Some(idx) = window_nav::neighbor(&rects, self.focus, dir) {
            self.set_focus(idx);
        }
    }

    /// Cycle focus to the next pane in index order (`Ctrl+W w`).
    pub(in crate::app) fn focus_next_pane(&mut self) {
        if !self.panes.is_empty() {
            self.set_focus((self.focus + 1) % self.panes.len());
        }
    }

    /// Move focus to pane `idx`, repainting if it actually changed.
    fn set_focus(&mut self, idx: usize) {
        if idx != self.focus && idx < self.panes.len() {
            self.focus = idx;
            self.needs_redraw = true;
        }
    }

    /// Insert text into the focused pane (debug `/text` injection).
    pub fn insert_text(&mut self, text: &str) -> Result<(), String> {
        self.log_event("text", format!("insert {text:?}"));
        // A focused **editable** panel region gets the text one character at a
        // time through the very path `panel_key` uses, so injected typing runs
        // the region's vim state machine and records real projection splices.
        // (Consequence: text injected while the region is in Normal mode is
        // interpreted as normal-mode commands, exactly as `/key` would be.)
        if let Some(id) = self.focused_panel_region() {
            let editable = self
                .panes
                .get(self.focus)
                .and_then(|p| p.panel.as_ref())
                .and_then(|p| p.region_editable(id))
                .unwrap_or(false);
            if editable {
                let pane_rect = self.panes.get(self.focus).map(|p| p.rect);
                let cell = self.viewport.cell;
                if let Some(rect) = pane_rect {
                    // Disjoint field borrows: the panel (in `self.panes`) and the
                    // clipboard are separate fields, so both can be borrowed at once.
                    let clip = self.clipboard.as_mut();
                    if let Some(panel) = self
                        .panes
                        .get_mut(self.focus)
                        .and_then(|p| p.panel.as_mut())
                    {
                        for c in text.chars() {
                            let key = if c == '\n' || c == '\r' {
                                Key::Enter
                            } else if c == '\t' {
                                Key::Tab
                            } else {
                                Key::Char(c)
                            };
                            panel.region_key(id, rect, cell, key, &mut *clip);
                        }
                        let refused = panel.take_edit_refusal(id);
                        self.status_error = refused;
                        self.needs_redraw = true;
                        return Ok(());
                    }
                }
            }
        }
        // A focused panel gets typed text as `text_input()`, not editor insertion.
        if self.panes.get(self.focus).is_some_and(Pane::is_panel) {
            if let Some(panel) = self
                .panes
                .get_mut(self.focus)
                .and_then(|p| p.panel.as_mut())
            {
                panel.text(text.to_string());
            }
            self.tick_focused_panel();
            self.needs_redraw = true;
            return Ok(());
        }
        // A read-only pane (the split-review "before" side) rejects inserted text.
        if self.panes.get(self.focus).is_some_and(|p| p.read_only) {
            return Ok(());
        }
        let visible = self.focused_visible_lines();
        let cell_w = self.viewport.cell.0;
        let Some(pane) = self.panes.get_mut(self.focus) else {
            return Err("no panes".to_string());
        };
        let visible_cols = pane.view.visible_cols(pane.rect, cell_w);
        pane.view.insert(text);
        pane.view.ensure_cursor_visible(visible, visible_cols);
        self.needs_redraw = true;
        Ok(())
    }
}

/// A short, stable label for a key press, used in the event log (e.g. `"j"`,
/// `"cmd+s"`, `"ctrl-r"`, `"Escape"`). Modifiers are prefixed in a fixed order.
fn describe_key(key: Key, mods: Mods) -> String {
    let mut s = String::new();
    if mods.cmd {
        s.push_str("cmd+");
    }
    if mods.ctrl {
        s.push_str("ctrl+");
    }
    if mods.shift {
        s.push_str("shift+");
    }
    let name = match key {
        Key::Char(c) => c.to_string(),
        Key::Ctrl(c) => format!("ctrl-{c}"),
        Key::Enter => "Enter".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::Backspace => "Backspace".to_string(),
        Key::Delete => "Delete".to_string(),
        Key::Escape => "Escape".to_string(),
        Key::Left => "Left".to_string(),
        Key::Right => "Right".to_string(),
        Key::Up => "Up".to_string(),
        Key::Down => "Down".to_string(),
        Key::Home => "Home".to_string(),
        Key::End => "End".to_string(),
        Key::PageUp => "PageUp".to_string(),
        Key::PageDown => "PageDown".to_string(),
    };
    s.push_str(&name);
    s
}

/// Resolve the project root for the fuzzy finder: the nearest ancestor of
/// `start` (inclusive) that contains a `.git`, falling back to `start` itself
/// when none is found. `start` is canonicalized first so the walk climbs real
/// parents (and `.` becomes the working directory).
fn project_root(start: &std::path::Path) -> std::path::PathBuf {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir = start.as_path();
    loop {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return start.clone(),
        }
    }
}

/// What the host should do with a key arriving at a focused panel pane.
#[derive(Clone, PartialEq, Eq, Debug)]
enum PanelKey {
    /// Quit the editor (`Cmd`/`Ctrl`+`Q`).
    Quit,
    /// Open the host command line (`:`). Reserved, like a process pane.
    CommandBar,
    /// Forward to the script as a named one-frame key press.
    Forward(String),
    /// Step back / forward in the pane's browser-style history (`Ctrl+[` /
    /// `Ctrl+]`). Reserved by the host — never forwarded to the script.
    NavBack,
    NavForward,
    /// Swallow the key — a host chord we don't forward, or nothing to do.
    Ignore,
}

/// Decide what a focused panel pane does with a key. Mirrors
/// [`classify_process_key`]'s reserved set (a pure function so it is unit-tested
/// without an `App`): `Cmd`/`Ctrl`+`Q` quits; a bare `:` opens the command bar;
/// any other `Cmd`/`Ctrl` chord is a host-global shortcut and is not forwarded;
/// every remaining key forwards to the script under its canonical name.
///
/// `claimed` is the escape hatch: the focused script asked for this exact chord
/// with `claim_key(...)`, so it is forwarded instead of swallowed. A panel whose
/// bare letters are its content (a spreadsheet, a console, a text editor) has no
/// command keyspace at all otherwise. **`Cmd`/`Ctrl`+`Q` is the one chord a
/// claim cannot take** — quitting must never be something a script can capture.
fn classify_panel_key(key: Key, mods: Mods, claimed: bool) -> PanelKey {
    if (mods.cmd || mods.ctrl) && matches!(key, Key::Char('q' | 'Q')) {
        return PanelKey::Quit;
    }
    if claimed {
        return match panel_key_name(key) {
            Some(name) => PanelKey::Forward(name),
            None => PanelKey::Ignore,
        };
    }
    if !mods.cmd && !mods.ctrl && matches!(key, Key::Char(':')) {
        return PanelKey::CommandBar;
    }
    // `Ctrl+[` / `Ctrl+]` drive the pane's history (back / forward), the keyboard
    // counterpart of `:back` / `:forward`. Reserved before the generic Ctrl-chord
    // swallow below so a screen can never shadow them.
    if mods.ctrl && !mods.cmd && matches!(key, Key::Char('[')) {
        return PanelKey::NavBack;
    }
    if mods.ctrl && !mods.cmd && matches!(key, Key::Char(']')) {
        return PanelKey::NavForward;
    }
    // `Ctrl+S` is forwarded to the drawer as a save gesture (the panel-app
    // counterpart of `:w`): a drawer with an `edit_view` reads it as
    // `key_down("ctrl") && key_pressed("s")` and issues its `mutate("save", …)`.
    // Forwarded even while an editable region is focused (the region-key routing
    // above claims only vim's own Ctrl chords, and `s` is not one), so save works
    // mid-edit. Other Ctrl/Cmd chords stay host-global.
    if mods.ctrl && !mods.cmd && matches!(key, Key::Char('s' | 'S')) {
        if let Some(name) = panel_key_name(key) {
            return PanelKey::Forward(name);
        }
    }
    if mods.cmd || mods.ctrl {
        return PanelKey::Ignore;
    }
    match panel_key_name(key) {
        Some(name) => PanelKey::Forward(name),
        None => PanelKey::Ignore,
    }
}

/// The typed text a panel key produces for `text_input()`, or `None` for keys
/// that aren't text (arrows, Enter, Escape…) **and for every chord**: a key held
/// with `Cmd`, `Ctrl`, or `Alt` is a command, not typing, so `text_input()` is
/// empty on that frame. Without this, the one chord Garden forwarded (`Ctrl+S`)
/// arrived with its character attached and the first save typed an "s" into the
/// document — latent in every panel that handles a chord and reads
/// `text_input()`. Shift is not a command modifier: it is already baked into the
/// character (an uppercase letter), so shifted keys still produce text.
fn panel_key_text(key: Key, mods: Mods) -> Option<String> {
    if mods.any_command() {
        return None;
    }
    match key {
        Key::Char(' ') => Some(" ".to_string()),
        Key::Char(c) if !c.is_control() => Some(c.to_string()),
        _ => None,
    }
}

/// The canonical name a panel script reads a key as (e.g. `key_pressed("down")`).
/// Named keys use lowercase words; a character key uses the character itself,
/// with the space bar as `"space"`. `Ctrl`-chords never reach here.
fn panel_key_name(key: Key) -> Option<String> {
    Some(match key {
        Key::Char(' ') => "space".to_string(),
        Key::Char(c) => c.to_string(),
        // `return`, not `enter`: petal-ui's `KEY_NAMES` is the cross-host
        // vocabulary and every other embedder (SDL, the test prelude) spells
        // Return this way. A panel script — including petal-ui's own
        // `text_field`, which we vendor and can't patch — reads
        // `key_pressed("return")`, so Garden has to emit it too.
        Key::Enter => "return".to_string(),
        Key::Tab => "tab".to_string(),
        Key::Backspace => "backspace".to_string(),
        Key::Delete => "delete".to_string(),
        Key::Escape => "escape".to_string(),
        Key::Left => "left".to_string(),
        Key::Right => "right".to_string(),
        Key::Up => "up".to_string(),
        Key::Down => "down".to_string(),
        Key::Home => "home".to_string(),
        Key::End => "end".to_string(),
        Key::PageUp => "pageup".to_string(),
        Key::PageDown => "pagedown".to_string(),
        Key::Ctrl(_) => return None,
    })
}

/// Copy the selection to the clipboard (a no-op without one; the selection
/// stays). Shared by Cmd+C and Ctrl+C.
fn clipboard_copy(clipboard: &mut dyn Clipboard, view: &EditorView) {
    let text = view.selected_text();
    if !text.is_empty() {
        clipboard.set(&text);
    }
}

/// Cut the selection: copy, then delete it as one undo transaction. Shared by
/// Cmd+X and Ctrl+X.
fn clipboard_cut(clipboard: &mut dyn Clipboard, view: &mut EditorView) {
    let text = view.selected_text();
    if !text.is_empty() {
        clipboard.set(&text);
        view.delete_selection();
        after_clipboard_edit(view);
    }
}

/// Paste the clipboard, replacing any selection as one undo transaction, and
/// keep the cursor visible. Shared by Cmd+V and Ctrl+V.
fn clipboard_paste(
    clipboard: &mut dyn Clipboard,
    view: &mut EditorView,
    visible: usize,
    visible_cols: usize,
) {
    if let Some(text) = clipboard.get().filter(|t| !t.is_empty()) {
        view.insert(&text);
        after_clipboard_edit(view);
        view.ensure_cursor_visible(visible, visible_cols);
    }
}

/// After a cut/paste consumed the selection: Visual mode has nothing selected
/// anymore, so drop back to Normal, and block-cursor modes need the caret
/// pulled back onto a real character.
fn after_clipboard_edit(view: &mut EditorView) {
    if view.vim.mode.is_visual() {
        view.vim.mode = vim::Mode::Normal;
    }
    if view.vim.mode.is_block_cursor() {
        view.clamp_cursor_normal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        cmd: false,
        ctrl: false,
        shift: false,
        alt: false,
    };
    const CMD: Mods = Mods {
        cmd: true,
        ctrl: false,
        shift: false,
        alt: false,
    };
    const CTRL: Mods = Mods {
        cmd: false,
        ctrl: true,
        shift: false,
        alt: false,
    };
    const ALT: Mods = Mods {
        cmd: false,
        ctrl: false,
        shift: false,
        alt: true,
    };
    const SHIFT: Mods = Mods {
        cmd: false,
        ctrl: false,
        shift: true,
        alt: false,
    };

    /// A focused panel forwards plain keys under their canonical names, but
    /// keeps the reserved chords (quit, command bar, host chords).
    #[test]
    fn panel_key_reserved_and_forwarded() {
        assert_eq!(
            classify_panel_key(Key::Char('q'), CMD, false),
            PanelKey::Quit
        );
        assert_eq!(
            classify_panel_key(Key::Char('q'), CTRL, false),
            PanelKey::Quit
        );
        assert_eq!(
            classify_panel_key(Key::Char(':'), NONE, false),
            PanelKey::CommandBar
        );
        // Other Cmd/Ctrl chords stay host-global, never forwarded.
        assert_eq!(
            classify_panel_key(Key::Char('s'), CMD, false),
            PanelKey::Ignore
        );
        // Plain keys forward under their canonical name.
        assert_eq!(
            classify_panel_key(Key::Char('j'), NONE, false),
            PanelKey::Forward("j".into())
        );
        assert_eq!(
            classify_panel_key(Key::Down, NONE, false),
            PanelKey::Forward("down".into())
        );
        assert_eq!(
            classify_panel_key(Key::Char(' '), NONE, false),
            PanelKey::Forward("space".into())
        );
    }

    /// A claimed chord is forwarded instead of being swallowed as a host
    /// shortcut — including the ones the host would otherwise keep for itself
    /// (`Cmd+Z`, the bare `:` command bar) — because a panel whose bare letters
    /// are content has no command keyspace otherwise.
    #[test]
    fn a_claimed_chord_reaches_the_script() {
        assert_eq!(
            classify_panel_key(Key::Char('z'), CMD, true),
            PanelKey::Forward("z".into())
        );
        assert_eq!(
            classify_panel_key(Key::Char(':'), NONE, true),
            PanelKey::Forward(":".into())
        );
        assert_eq!(
            classify_panel_key(Key::Char('['), CTRL, true),
            PanelKey::Forward("[".into())
        );
    }

    /// …except quit. A script must never be able to capture Cmd/Ctrl+Q.
    #[test]
    fn quit_cannot_be_claimed() {
        assert_eq!(
            classify_panel_key(Key::Char('q'), CMD, true),
            PanelKey::Quit
        );
        assert_eq!(
            classify_panel_key(Key::Char('q'), CTRL, true),
            PanelKey::Quit
        );
    }

    /// An unclaimed Alt chord is not a host shortcut, so it forwards like any
    /// plain key — Alt is the modifier a panel gets for free.
    #[test]
    fn alt_chords_forward_to_the_script() {
        assert_eq!(
            classify_panel_key(Key::Char('a'), ALT, false),
            PanelKey::Forward("a".into())
        );
    }

    /// A chord produces no typed text: `text_input()` is empty on the frame a
    /// panel handles `Ctrl+S`, so the save doesn't also type an "s".
    #[test]
    fn a_chord_carries_no_text_input() {
        assert_eq!(panel_key_text(Key::Char('s'), CTRL), None);
        assert_eq!(panel_key_text(Key::Char('s'), CMD), None);
        assert_eq!(panel_key_text(Key::Char('s'), ALT), None);
        // Shift is not a command modifier — it is already in the character.
        assert_eq!(panel_key_text(Key::Char('S'), SHIFT), Some("S".to_string()));
        assert_eq!(panel_key_text(Key::Char('s'), NONE), Some("s".to_string()));
    }

    /// The modifier bitmask matches petal-ui's (`1=shift 2=ctrl 4=alt 8=cmd`) —
    /// the encoding a claim is matched on and `/state` reports.
    #[test]
    fn mods_bits_match_the_script_encoding() {
        assert_eq!(NONE.bits(), 0);
        assert_eq!(SHIFT.bits(), 1);
        assert_eq!(CTRL.bits(), 2);
        assert_eq!(ALT.bits(), 4);
        assert_eq!(CMD.bits(), 8);
        assert_eq!(
            Mods {
                cmd: true,
                shift: true,
                ..NONE
            }
            .bits(),
            9
        );
    }

    #[test]
    fn panel_key_names_are_canonical() {
        assert_eq!(panel_key_name(Key::PageDown), Some("pagedown".into()));
        assert_eq!(panel_key_name(Key::Escape), Some("escape".into()));
        assert_eq!(panel_key_name(Key::Char('K')), Some("K".into()));
        assert_eq!(panel_key_name(Key::Ctrl('d')), None);
        // Return is `"return"`, never `"enter"` — the spelling petal-ui's own
        // widgets (and every other host) read. Getting this wrong is invisible
        // in the suite and deaf in the pane, so pin it.
        assert_eq!(panel_key_name(Key::Enter), Some("return".into()));
    }

    /// Every named key Garden forwards has to be in petal-ui's cross-host
    /// vocabulary; only character keys are Garden's to spell.
    #[test]
    fn named_panel_keys_are_in_the_petal_ui_vocabulary() {
        let named = [
            Key::Enter,
            Key::Tab,
            Key::Backspace,
            Key::Delete,
            Key::Escape,
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
            Key::Char(' '),
        ];
        for key in named {
            let name = panel_key_name(key).expect("named key has a panel name");
            assert!(
                garden_script::KEY_NAMES.contains(&name.as_str()),
                "{name:?} is not a petal-ui canonical key name"
            );
        }
    }
}
