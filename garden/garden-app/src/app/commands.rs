//! Ex commands, search/substitution, and the native-menu dispatch. The `:`
//! command line and the `/` `?` search prompts (both edited in [`super::input`])
//! hand their accepted input here; the macOS File/Edit menus route through
//! [`App::dispatch_menu`], reusing the keyboard-shortcut paths.

use std::path::PathBuf;

use std::time::Instant;

use garden_script::LayoutNode;

use crate::command_line::{Addr, Command, CommandLine, Substitution};
use crate::search;
use crate::vim::{self, Key};
use crate::window_nav;

use super::{App, KeyOutcome, MenuAction, Mods, ToolbarAction};

impl App {
    /// An accepted search prompt: remember the pattern (per-pane, beside the
    /// vim register), turn highlights on, and jump to the first match strictly
    /// after/before the cursor, wrapping around the buffer. An empty pattern
    /// is a no-op; no match reports in the status bar and leaves the cursor.
    pub(in crate::app) fn accept_search(&mut self, pattern: String, forward: bool) {
        if pattern.is_empty() {
            return;
        }
        // A focused panel region is the search target when there is one: the
        // prompt was opened from inside it (see the `OpenSearch` handling in
        // `input.rs`), so the pattern belongs to its buffer, not to the pane's.
        // This is what makes `/` work in the diff reviewer's unified view.
        if let Some(id) = self.focused_panel_region() {
            let rect = self.panes.get(self.focus).map(|p| p.rect);
            let cell = self.viewport.cell;
            if let (Some(rect), Some(panel)) = (
                rect,
                self.panes
                    .get_mut(self.focus)
                    .and_then(|p| p.panel.as_mut()),
            ) {
                if !panel.region_search(id, rect, cell, &pattern, forward) {
                    self.status_error = Some(format!("E: pattern not found: {pattern}"));
                }
                self.needs_redraw = true;
                return;
            }
        }
        let visible = self.focused_visible_lines();
        let cell_w = self.viewport.cell.0;
        let Some(pane) = self.panes.get_mut(self.focus) else {
            return;
        };
        let view = &mut pane.view;
        view.vim.last_search = Some(pattern.clone());
        view.vim.last_search_forward = forward;
        view.vim.last_search_word = false; // prompt patterns are plain substrings
        view.vim.search_hl = true;
        match search::find_next(&view.buffer, view.cursor, &pattern, forward, false) {
            Some(p) => {
                vim::collapse_selection_in_normal(view);
                view.cursor = p;
                view.desired_col = None;
                let visible_cols = view.visible_cols(pane.rect, cell_w);
                view.ensure_cursor_visible(visible, visible_cols);
            }
            None => self.status_error = Some(format!("E: pattern not found: {pattern}")),
        }
    }

    /// Run a `:s` / `:%s` / `:N,Ms` substitution against the focused pane.
    /// Replaces plain-text matches over one line (the cursor's), the whole
    /// buffer, or an explicit line range, as **one undo transaction**, leaves
    /// the cursor on the last changed line, and reports the count (or "pattern
    /// not found"). An empty pattern reuses the pane's last search pattern
    /// (vim's `:s//rep/`); running a substitution also updates that
    /// last-search pattern, like vim.
    fn substitute(&mut self, sub: Substitution) {
        if self.focused_is_process() {
            self.status_error = Some("E: cannot edit a process pane".to_string());
            return;
        }
        let Some(pane) = self.panes.get_mut(self.focus) else {
            return;
        };
        let view = &mut pane.view;

        let pattern = if sub.pattern.is_empty() {
            view.vim.last_search.clone().unwrap_or_default()
        } else {
            sub.pattern.clone()
        };
        if pattern.is_empty() {
            self.status_error = Some("E: no previous search pattern".to_string());
            return;
        }
        view.vim.last_search = Some(pattern.clone());
        view.vim.last_search_word = false; // :s patterns are plain substrings

        let last_line = view.buffer.line_count() - 1;
        let lines = if sub.whole_buffer {
            0..=last_line
        } else if let Some((from, to)) = sub.range {
            // Resolve the symbolic addresses (1-based lines, `.`, `$`) against
            // the buffer, clamping and reordering so the range is always valid.
            let resolve = |addr: Addr| match addr {
                Addr::Line(n) => n.saturating_sub(1).min(last_line),
                Addr::Current => view.cursor.line,
                Addr::Last => last_line,
            };
            let (start, end) = (resolve(from), resolve(to));
            start.min(end)..=start.max(end)
        } else {
            view.cursor.line..=view.cursor.line
        };

        // Rewrite each line in range; remember the first and last that changed
        // so the edit (and its undo) spans only the affected block.
        let start_line = *lines.start();
        let mut new_lines = Vec::new();
        let mut first_changed = None;
        let mut last_changed = start_line;
        let mut total = 0;
        let mut changed_lines = 0;
        for line in lines {
            let (new, n) = search::substitute_line(
                &view.buffer.line(line),
                &pattern,
                &sub.replacement,
                sub.global,
                sub.ignore_case,
            );
            if n > 0 {
                total += n;
                changed_lines += 1;
                first_changed.get_or_insert(line);
                last_changed = line;
            }
            new_lines.push(new);
        }

        let Some(first) = first_changed else {
            self.status_error = Some(format!("E: pattern not found: {pattern}"));
            return;
        };
        let last = last_changed;

        let text = new_lines[first - start_line..=last - start_line].join("\n");
        let from = garden_core::Point::new(first, 0);
        let to = garden_core::Point::new(last, view.buffer.line_len(last));
        view.buffer.replace(from, to, &text);
        view.cursor = view.buffer.clamp(garden_core::Point::new(last, 0));
        view.anchor = None;
        view.desired_col = None;
        self.status_note = Some(format!(
            "{total} substitution{} on {changed_lines} line{}",
            if total == 1 { "" } else { "s" },
            if changed_lines == 1 { "" } else { "s" },
        ));
    }

    /// Turn soft line-wrapping on or off for the focused editor pane (`:set
    /// wrap` / `:set nowrap`). A process pane is a passive render surface whose
    /// rows must stay 1:1 with the client's content, so wrapping stays off there.
    fn set_wrap(&mut self, on: bool) {
        if self.focused_is_process() {
            self.status_error = Some("E: cannot wrap a process pane".to_string());
            return;
        }
        let visible = self.focused_visible_lines();
        let cell_w = self.viewport.cell.0;
        let Some(pane) = self.panes.get_mut(self.focus) else {
            return;
        };
        let visible_cols = pane.view.visible_cols(pane.rect, cell_w);
        pane.view.set_wrap(on, visible, visible_cols);
        self.status_note = Some(if on { "wrap" } else { "nowrap" }.to_string());
        // Persist the flag through the layout script's `editor(...)` config so
        // `:set nowrap` survives a restart (mirrors `toggle_line_numbers`).
        self.sync_layout();
    }

    /// Execute a parsed ex command. `:q` / `:wq` close the focused pane and
    /// return `Quit` only from the last pane; `:wqa` always quits.
    pub(in crate::app) fn run_command(&mut self, command: Command) -> KeyOutcome {
        // Record every ex command for the event log (and thus bug reports), so a
        // `:report` carries the commands that led up to it. Logged before
        // dispatch so the `:report` invocation itself appears in its own context.
        self.log_event("command", format!("{command:?}"));
        match command {
            Command::Empty => {}
            Command::Write => {
                self.save_focused();
            }
            Command::WriteAs(path) => {
                self.save_focused_as(&path);
            }
            Command::WriteAll => {
                self.save_all();
            }
            // `:q` closes the focused pane, vim-style; only the last pane
            // standing closes this window (window_close handles both; the
            // process quits via `:wqa` / Cmd+Q).
            Command::Quit => return self.window_close(true),
            Command::WriteQuit => {
                // Don't close if the write failed (e.g. a process pane).
                if self.save_focused() {
                    return self.window_close(true);
                }
            }
            Command::WriteAllQuit => {
                // Don't quit while a save-protected scratch buffer is still
                // unsaved: `save_all` leaves it dirty and returns false, guiding
                // the user to `:w <path>` rather than losing their scratch.
                if self.save_all() {
                    return KeyOutcome::Quit;
                }
            }
            // `:windownew` raises the new-window intent; the windowed frontend
            // drains it and spawns the window (the core can't create OS
            // windows). No pane, close, or quit effects on this window.
            Command::WindowNew => {
                self.new_window_requested = true;
            }
            Command::Edit(path) => {
                if path.is_empty() {
                    self.status_error = Some("E: :e needs a file name".to_string());
                } else {
                    self.open_path(&path);
                }
            }
            Command::Explore => {
                let dir = self.focused_browse_dir();
                self.open_directory_browser(&dir);
            }
            Command::Git => self.open_git_viewer(),
            // `:Diff [rev]` / `:Diff --stat [rev]` — the same `garden-diff`
            // review; `--stat` only picks the view it opens in.
            Command::Diff { rev, stat } => {
                let rev = rev.trim();
                let mut extra: Vec<String> = Vec::new();
                if !rev.is_empty() {
                    extra.push(rev.to_string());
                }
                if stat {
                    extra.push("--stat".to_string());
                }
                self.open_garden_diff(extra);
            }
            // `:Review` / `:Review2` / `:ReviewSplit` are aliases of `:Diff` — the
            // one `garden-diff` editable review (base ref → working tree).
            Command::Review(base) | Command::ReviewSplit(base) => {
                let base = base.trim();
                let extra = (!base.is_empty()).then(|| vec![base.to_string()]);
                self.open_garden_diff(extra.unwrap_or_default());
            }
            // `:PR [number]` opens the same client in PR mode (an absent number =
            // the current branch's PR), fetching the PR with `gh`.
            Command::Pr(number) => {
                let number = number.trim();
                let mut extra = vec!["--pr".to_string()];
                if !number.is_empty() {
                    extra.push(number.to_string());
                }
                self.open_garden_diff(extra);
            }
            Command::NoHighlight => {
                if let Some(pane) = self.panes.get_mut(self.focus) {
                    pane.view.vim.search_hl = false;
                }
            }
            Command::Report(message) => self.file_report(&message),
            Command::SetWrap(on) => self.set_wrap(on),
            Command::ToggleState => {
                self.show_panel_state = !self.show_panel_state;
                self.status_note = Some(
                    if self.show_panel_state {
                        "state inspector on"
                    } else {
                        "state inspector off"
                    }
                    .to_string(),
                );
            }
            Command::Back => return self.nav_focused_panel(garden_script::NavIntent::Back),
            Command::Forward => return self.nav_focused_panel(garden_script::NavIntent::Forward),
            Command::Substitute(sub) => self.substitute(sub),
            Command::Goto(addr) => self.goto_addr(addr),
            Command::Unknown(text) => {
                self.status_error = Some(format!("E: not a command: {text}"));
            }
        }
        self.needs_redraw = true;
        KeyOutcome::Handled
    }

    /// Replace the focused pane's buffer with the file at `path` (shared by
    /// `:e` and the File ▸ Open menu). If the pane was process-backed (a GPP
    /// browser), dropping its [`ProcessPane`](crate::process_pane::ProcessPane)
    /// shuts the child down and the pane becomes a normal editor — so `:e file`
    /// is a way out of a browser pane.
    pub(in crate::app) fn open_path(&mut self, path: &str) {
        self.log_event("file", format!("open {path}"));
        self.record_file_opened(path);
        if let Some(pane) = self.panes.get_mut(self.focus) {
            pane.set_editor(Some(path.to_string()));
            self.needs_redraw = true;
            // The pane's content changed out-of-band; keep the persisted layout
            // in sync so a later split/reload reflects the file actually open.
            self.sync_layout();
        }
    }

    /// Whether the focused pane is process-backed (a GPP client drives it). The
    /// editing ex commands (`:w`, `:s`) report a friendly error instead of
    /// touching the passive render surface a client owns.
    fn focused_is_process(&self) -> bool {
        self.panes
            .get(self.focus)
            .is_some_and(super::Pane::is_process)
    }

    /// Replace the focused pane's buffer with a fresh, untitled one (File ▸ New).
    /// Like `:e`, this also drops a focused browser back to an editor.
    fn new_file(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focus) {
            pane.set_editor(None);
            self.needs_redraw = true;
            self.sync_layout();
        }
    }

    /// Run a native-menu command. Editing actions reuse the keyboard-shortcut
    /// path (so undo grouping, clipboard, and redraw flags behave identically);
    /// File ▸ New/Open use the same buffer-swap as `:e`, and the Git/Go items
    /// route through [`App::run_command`] so they match their ex commands
    /// exactly (event log included).
    pub fn dispatch_menu(&mut self, action: MenuAction) {
        let cmd = Mods {
            cmd: true,
            ..Mods::default()
        };
        let cmd_shift = Mods {
            cmd: true,
            shift: true,
            ..Mods::default()
        };
        match action {
            MenuAction::NewFile => self.new_file(),
            // File ▸ New Window raises the same new-window intent as
            // `:windownew` (routed through run_command for the event log).
            MenuAction::NewWindow => self.run_menu_command(Command::WindowNew),
            MenuAction::OpenFile(path) => self.open_path(&path.to_string_lossy()),
            // "Open Folder" is the user naming a project, so it records one —
            // unlike `:E` / `-`, which browse in and out of directories all
            // session and would otherwise flood the list.
            MenuAction::OpenFolder(path) => {
                let dir = path.to_string_lossy();
                self.record_project_opened(&dir);
                self.open_directory_browser(&dir);
            }
            MenuAction::Save => self.apply_key(Key::Char('s'), cmd),
            MenuAction::SaveAll => self.apply_key(Key::Char('s'), cmd_shift),
            MenuAction::CloseWindow => self.apply_key(Key::Char('w'), cmd),
            MenuAction::Quit => self.apply_key(Key::Char('q'), cmd),
            MenuAction::Undo => self.apply_key(Key::Char('z'), cmd),
            MenuAction::Redo => self.apply_key(Key::Char('z'), cmd_shift),
            MenuAction::Cut => self.apply_key(Key::Char('x'), cmd),
            MenuAction::Copy => self.apply_key(Key::Char('c'), cmd),
            MenuAction::Paste => self.apply_key(Key::Char('v'), cmd),
            MenuAction::SelectAll => self.apply_key(Key::Char('a'), cmd),
            MenuAction::Find => {
                self.command_line = Some(CommandLine::new_search(true));
                self.needs_redraw = true;
            }
            MenuAction::FindNext => self.repeat_search(false),
            MenuAction::FindPrev => self.repeat_search(true),
            MenuAction::SetTheme(scheme) => self.set_theme_scheme(scheme),
            MenuAction::ToggleWrap => {
                let on = self.panes.get(self.focus).is_none_or(|p| p.view.wrap);
                self.set_wrap(!on);
            }
            MenuAction::ToggleLineNumbers => self.toggle_line_numbers(),
            MenuAction::ToggleStateInspector => self.run_menu_command(Command::ToggleState),
            MenuAction::TogglePlay => self.toggle_play(),
            MenuAction::ToggleIr => self.toggle_ir_panel(),
            MenuAction::GoToFile => self.open_file_finder(),
            MenuAction::Back => self.run_menu_command(Command::Back),
            MenuAction::Forward => self.run_menu_command(Command::Forward),
            MenuAction::ExploreDirectory => self.run_menu_command(Command::Explore),
            MenuAction::GitLog => self.run_menu_command(Command::Git),
            MenuAction::GitDiff => self.run_menu_command(Command::Diff {
                rev: String::new(),
                stat: false,
            }),
            MenuAction::GitDiffStat => self.run_menu_command(Command::Diff {
                rev: String::new(),
                stat: true,
            }),
            MenuAction::ReviewChanges => self.run_menu_command(Command::Review(String::new())),
            MenuAction::SplitDown => self.window_split(true),
            MenuAction::SplitRight => self.window_split(false),
            MenuAction::CloseOtherPanes => self.window_only(),
            MenuAction::ClosePane => {
                // With `close_if_last: false` the last pane is refused (E444),
                // so today this can only be Handled; map the exit variants
                // anyway so they can never be silently dropped.
                match self.window_close(false) {
                    KeyOutcome::Quit => self.quit = true,
                    KeyOutcome::CloseWindow => self.close_window = true,
                    KeyOutcome::Handled | KeyOutcome::Ignored => {}
                }
                self.needs_redraw = true;
            }
            MenuAction::NextPane => self.focus_next_pane(),
        }
    }

    /// Dispatch a Petal-IDE toolbar button press. The toolbar only exists in IDE
    /// mode, so these are all IDE actions; they share their implementation with
    /// the equivalent [`MenuAction`]s so behavior stays identical (see
    /// [`App::dispatch_menu`]).
    pub(in crate::app) fn dispatch_toolbar(&mut self, action: ToolbarAction) {
        match action {
            ToolbarAction::TogglePlay => self.toggle_play(),
            ToolbarAction::ToggleIr => self.toggle_ir_panel(),
            ToolbarAction::ToggleState => self.run_menu_command(Command::ToggleState),
            ToolbarAction::ResetSketch => self.reset_sketch(),
        }
    }

    /// Freeze / resume canvas re-rendering (the toolbar's ▶ / ⏸, `TogglePlay`).
    /// While paused, panels don't tick and the IR source isn't refreshed; the
    /// editor stays fully live. Resuming wakes the panels so animation restarts.
    pub(in crate::app) fn toggle_play(&mut self) {
        self.paused = !self.paused;
        if self.paused {
            self.status_note = Some("paused — canvas frozen (editor still live)".to_string());
        } else {
            self.wake_panels();
            self.status_note = Some("playing".to_string());
        }
        self.needs_redraw = true;
    }

    /// Open or close the IR inspector pane (the toolbar's IR button, `ToggleIr`).
    /// Opening splits a `panel(ir_view.ptl)` node to the right of the focused
    /// pane; the pane rebuild attaches its data provider ([`App::attach_ir_providers`]).
    /// Closing focuses and closes it. A no-op outside IDE mode.
    pub(in crate::app) fn toggle_ir_panel(&mut self) {
        if self.ide.is_none() {
            return;
        }
        if let Some(idx) = self.ir_panel_index() {
            self.focus = idx;
            let _ = self.window_close(false);
        } else {
            let ir_view = self
                .ide
                .as_ref()
                .unwrap()
                .ir_view_path
                .to_string_lossy()
                .into_owned();
            let Some(original) = self.focused_pane_node() else {
                return;
            };
            let panel_node = LayoutNode::Panel {
                script: ir_view,
                screens: Vec::new(),
            };
            let split = LayoutNode::Row {
                children: vec![original, panel_node],
                ratios: None,
            };
            let mut tree = self.layout_from_panes();
            if window_nav::replace_leaf(&mut tree, self.focus, split) {
                self.apply_runtime_layout(tree);
            }
        }
        self.needs_redraw = true;
    }

    /// Restart the canvas sketch(es) from scratch, discarding Petal `state` (the
    /// toolbar's Reset, `ResetSketch`). The IR inspector pane is left alone (its
    /// tab selection shouldn't reset). Unsaved editor edits are kept — only the
    /// animation state restarts. Also un-pauses so the reset is visible.
    pub(in crate::app) fn reset_sketch(&mut self) {
        let ir_idx = self.ir_panel_index();
        let now = Instant::now();
        let mut any = false;
        for (i, pane) in self.panes.iter_mut().enumerate() {
            if Some(i) == ir_idx {
                continue;
            }
            if let Some(pv) = pane.panel.as_mut() {
                any |= pv.restart(now);
            }
        }
        if any {
            self.paused = false;
            self.status_note = Some("sketch reset".to_string());
            self.needs_redraw = true;
        }
    }

    /// Run an ex command on behalf of a menu item, honoring a quit or
    /// close-window outcome the way [`App::apply_key`] does (menu clicks
    /// arrive outside the key funnel).
    fn run_menu_command(&mut self, command: Command) {
        match self.run_command(command) {
            KeyOutcome::Quit => self.quit = true,
            KeyOutcome::CloseWindow => self.close_window = true,
            KeyOutcome::Handled | KeyOutcome::Ignored => {}
        }
    }

    /// Jump to the next (or, `reverse`, the previous) match of the focused
    /// pane's last search — vim's `n` / `N`, callable from the menu in any
    /// mode. Keeps the pattern's whole-word flag (`*` / `#` searches repeat as
    /// word searches) and re-arms match highlighting.
    fn repeat_search(&mut self, reverse: bool) {
        let visible = self.focused_visible_lines();
        let cell_w = self.viewport.cell.0;
        let Some(pane) = self.panes.get_mut(self.focus) else {
            return;
        };
        let view = &mut pane.view;
        let Some(pattern) = view.vim.last_search.clone() else {
            self.status_error = Some("E: no previous search pattern".to_string());
            self.needs_redraw = true;
            return;
        };
        let forward = view.vim.last_search_forward != reverse;
        let word = view.vim.last_search_word;
        view.vim.search_hl = true;
        match search::find_next(&view.buffer, view.cursor, &pattern, forward, word) {
            Some(p) => {
                vim::collapse_selection_in_normal(view);
                view.cursor = p;
                view.desired_col = None;
                let visible_cols = view.visible_cols(pane.rect, cell_w);
                view.ensure_cursor_visible(visible, visible_cols);
            }
            None => self.status_error = Some(format!("E: pattern not found: {pattern}")),
        }
        self.needs_redraw = true;
    }

    /// Toggle the focused pane's line-number gutter (View ▸ Line Numbers). The
    /// flag is per-pane layout state, so the change is persisted like `:e`'s
    /// content swaps are.
    fn toggle_line_numbers(&mut self) {
        let Some(pane) = self.panes.get_mut(self.focus) else {
            return;
        };
        pane.view.show_line_numbers = !pane.view.show_line_numbers;
        self.status_note = Some(
            if pane.view.show_line_numbers {
                "line numbers"
            } else {
                "no line numbers"
            }
            .to_string(),
        );
        self.needs_redraw = true;
        self.sync_layout();
    }

    /// `:42` / `:$` (and a range's end line) — vim's jump-to-line. The target
    /// is clamped to the buffer; the cursor lands on the line's first
    /// non-blank column and is scrolled into view. A no-op in process/panel
    /// panes, which have no cursor to move.
    fn goto_addr(&mut self, addr: Addr) {
        let visible = self.focused_visible_lines();
        let cell_w = self.viewport.cell.0;
        let Some(pane) = self.panes.get_mut(self.focus) else {
            return;
        };
        if pane.is_process() || pane.is_panel() {
            return;
        }
        let view = &mut pane.view;
        let line = match addr {
            Addr::Line(n) => n.saturating_sub(1),
            Addr::Current => view.cursor.line,
            Addr::Last => usize::MAX, // clamped to the last line below
        };
        vim::goto_line(view, line);
        let visible_cols = view.visible_cols(pane.rect, cell_w);
        view.ensure_cursor_visible(visible, visible_cols);
    }

    /// Save the focused pane. A process-backed pane has no file to write, so it
    /// reports an error and returns `false`; `:wq` uses the result to avoid
    /// quitting on a failed write.
    fn save_focused(&mut self) -> bool {
        if self.focused_is_process() {
            self.status_error = Some("E: cannot write a process pane".to_string());
            return false;
        }
        // A save-protected file (e.g. the Petal-IDE scratch): don't overwrite
        // it. Open the command line pre-filled with `w ` so the user names a
        // file to save to (`:w <path>` → save-as). Nothing is written here.
        if let Some(file) = self.focused_pane_file() {
            if self.save_as_paths.contains(&file) {
                self.command_line = Some(CommandLine::with_input("w "));
                self.status_note =
                    Some("scratch buffer — type a filename to save it as".to_string());
                self.needs_redraw = true;
                return false;
            }
        }
        if let Some(pane) = self.panes.get_mut(self.focus) {
            if let Err(err) = pane.view.save() {
                self.status_error = Some(format!("save failed: {err}"));
                return false;
            }
            // Vim-style write confirmation, so `:w` visibly did something.
            self.status_note = Some(format!("wrote {}", pane.view.display_name()));
            self.needs_redraw = true;
        }
        true
    }

    /// The focused pane's file path, if it has one.
    fn focused_pane_file(&self) -> Option<PathBuf> {
        self.panes
            .get(self.focus)
            .and_then(|p| p.file.as_ref())
            .map(PathBuf::from)
    }

    /// `:w <path>` — write the focused buffer to `path`, adopting it as the
    /// pane's file (vim save-as). This is how a save-protected buffer (the
    /// Petal-IDE scratch) is written without overwriting the source: the pane —
    /// and any panel paired with it by path (the live Petal-IDE canvas) —
    /// re-point to the new file, so it is no longer protected and future saves
    /// write there directly.
    fn save_focused_as(&mut self, path: &str) -> bool {
        if self.focused_is_process() {
            self.status_error = Some("E: cannot write a process pane".to_string());
            return false;
        }
        let target = PathBuf::from(path.trim());
        if target.as_os_str().is_empty() {
            self.status_error = Some("E: :w needs a file name".to_string());
            return false;
        }
        let old_file = self.focused_pane_file();
        let Some(pane) = self.panes.get_mut(self.focus) else {
            return false;
        };
        if let Err(err) = pane.view.save_as(&target) {
            self.status_error = Some(format!("save failed: {err}"));
            return false;
        }
        let new_file = target.to_string_lossy().into_owned();
        pane.file = Some(new_file.clone());
        // Re-point any panel paired with the old path (the Petal-IDE canvas) so
        // the live editor→panel binding keeps tracking this buffer.
        if let Some(old) = &old_file {
            let old_str = old.to_string_lossy();
            for p in &mut self.panes {
                if let Some(panel) = p.panel.as_mut() {
                    // Pair by the panel's ORIGIN screen (the layout-declared file
                    // the editor drives), which is also what `set_script` rewrites.
                    if panel.origin_script() == old_str {
                        panel.set_script(new_file.clone());
                    }
                }
            }
        }
        self.status_error = None;
        self.status_note = Some(format!("saved as {new_file}"));
        self.needs_redraw = true;
        true
    }

    /// `:wa` / Cmd+Shift+S — save every dirty pane that has a file path.
    /// Save-protected scratch panes (see [`App::save_as_paths`]) are left
    /// untouched, mirroring `save_focused`. Returns whether it is safe to quit
    /// afterward — `false` when a protected buffer was left dirty, so `:wqa`
    /// must not quit while it is still unsaved.
    pub(in crate::app) fn save_all(&mut self) -> bool {
        let outcome = super::save_all_panes(&mut self.panes, &self.save_as_paths);
        if let Some(err) = outcome.first_error {
            self.status_error = Some(err);
        }
        if outcome.skipped_protected > 0 {
            self.status_note =
                Some("scratch buffer skipped — type `:w <path>` to save it as a file".to_string());
            self.needs_redraw = true;
            return false;
        }
        true
    }
}
