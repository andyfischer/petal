//! Process-backed (GPP) panes: applying the messages a subprocess pushes
//! (`render`, `setKeymap`, `setStatus`, `openPath`) and opening the directory
//! browser in the focused pane.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use garden_script::NavIntent;

use crate::editor_view::EditorView;
use crate::panel_view::ClientEvent;
use crate::process_pane::{self, ProcessPane};

use super::panes::process_dims;
use super::App;

/// How long the host blocks for a subprocess client to answer the `navigate`
/// mutation with the target screen's source. Navigation is user-initiated and
/// rare, so a short synchronous wait (like [`prime_script_client`]'s query
/// priming) keeps the swap feeling immediate without hanging on a wedged client.
const NAV_MUTATION_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the host blocks for a subprocess client to answer a script
/// `mutate(name, arg)` request (e.g. a save that writes files). User-initiated
/// and effectful, so a longer wait than navigation is warranted while still
/// bounding a wedged client.
const MUTATE_TIMEOUT: Duration = Duration::from_millis(2000);

impl App {
    /// Apply a drained batch of GPP messages to pane `pane_idx`: `render`
    /// replaces the passive view's content, `setKeymap` swaps the forwarded-key
    /// set, `setStatus` updates the status note, and `openPath` turns the GPP
    /// pane into a normal editor on the path (shutting the subprocess down).
    pub(in crate::app) fn apply_process_messages(
        &mut self,
        pane_idx: usize,
        msgs: Vec<gpp::Envelope>,
    ) {
        if msgs.is_empty() {
            return;
        }
        let cell = self.viewport.cell;
        // Set when an `openPath` turned this pane from a browser into an editor,
        // so the persisted layout is re-synced once after the batch.
        let mut content_changed = false;
        for env in msgs {
            if env.is_method(gpp::method::RENDER) {
                let Ok(params) = env.params_as::<gpp::RenderParams>() else {
                    continue;
                };
                let Some(pane) = self.panes.get_mut(pane_idx) else {
                    continue;
                };
                let text = params.lines.join("\n");
                pane.view.set_external_content(&text, params.cursor_line);
                if let Some(styles) = params.styles {
                    pane.view.set_external_styles(styles);
                }
                if let Some(backgrounds) = params.backgrounds {
                    pane.view.set_external_backgrounds(backgrounds);
                }
                if let Some(title) = params.title {
                    pane.view.set_external_title(Some(title));
                }
                let visible = EditorView::visible_lines(pane.rect, cell.1);
                let visible_cols = pane.view.visible_cols(pane.rect, cell.0);
                pane.view.ensure_cursor_visible(visible, visible_cols);
                if let Some(status) = params.status {
                    self.status_note = Some(status);
                }
            } else if env.is_method(gpp::method::SET_KEYMAP) {
                let Ok(params) = env.params_as::<gpp::SetKeymapParams>() else {
                    continue;
                };
                if let Some(process) = self
                    .panes
                    .get_mut(pane_idx)
                    .and_then(|p| p.process.as_mut())
                {
                    process.set_keymap(params.keys);
                    if let Some(takeover) = params.takeover {
                        process.set_takeover(takeover);
                    }
                    if let Some(mouse) = params.mouse {
                        process.set_mouse(mouse);
                    }
                }
            } else if env.is_method(gpp::method::SET_STATUS) {
                let Ok(params) = env.params_as::<gpp::SetStatusParams>() else {
                    continue;
                };
                self.status_note = Some(params.text);
            } else if env.is_method(gpp::method::OPEN_PATH) {
                let Ok(params) = env.params_as::<gpp::OpenPathParams>() else {
                    continue;
                };
                if let Some(pane) = self.panes.get_mut(pane_idx) {
                    // The pane becomes a normal editor on the path; `set_editor`
                    // drops the ProcessPane (shutting the child down).
                    pane.set_editor(Some(params.path));
                    content_changed = true;
                }
            }
        }
        if content_changed {
            // A browser became a plain editor; persist so a later split/reload
            // shows the opened file, not a fresh browser.
            self.sync_layout();
        }
        self.needs_redraw = true;
    }

    /// Drain and apply pending messages from every process-backed pane. Called
    /// by each frontend on the reload tick, beside [`poll_files`](App::poll_files).
    pub fn poll_processes(&mut self) {
        for i in 0..self.panes.len() {
            let msgs = match self.panes.get(i).and_then(|p| p.process.as_ref()) {
                Some(process) => process.try_drain(),
                None => continue,
            };
            self.apply_process_messages(i, msgs);
        }
    }

    /// Drive the query round-trip for every **panel-mode GPP pane** (the
    /// script-push protocol): apply the client's `queryResult` answers to the
    /// shared cache and flush the queries the script asked for. Non-blocking —
    /// answers land here on a later tick and the panel re-renders. Called on the
    /// reload tick, *before* [`tick_panels`](App::tick_panels) so freshly landed
    /// data is on screen the same cycle. See
    /// `docs/gpp.md`.
    pub fn poll_script_clients(&mut self) {
        for i in 0..self.panes.len() {
            let is_client = self
                .panes
                .get(i)
                .and_then(|p| p.panel.as_ref())
                .is_some_and(|pv| pv.has_client());
            if !is_client {
                continue;
            }
            let (events, changed) = self
                .panes
                .get_mut(i)
                .and_then(|p| p.panel.as_mut())
                .map(|pv| pv.pump_client(None))
                .unwrap_or_default();
            if changed {
                // Keep the panel awake so it re-ticks and renders the new data.
                if let Some(pv) = self.panes.get_mut(i).and_then(|p| p.panel.as_mut()) {
                    pv.note_activity(Instant::now());
                }
                self.needs_redraw = true;
            }
            self.handle_client_events(i, events);
        }
    }

    /// Run the first few query round-trips for a just-spawned script client
    /// *synchronously* (waiting briefly for each answer), so the pane paints with
    /// data instead of a spinner the moment it appears — the panel-mode analogue
    /// of draining a Lines client's initial `render`.
    pub(in crate::app) fn prime_script_client(&mut self, idx: usize) {
        let now = Instant::now();
        // Bounded rounds: a run issues queries, a pump waits for the answers; a
        // round with no new data means the view has settled.
        for _ in 0..6 {
            let Some(rect) = self.panes.get(idx).map(|p| p.rect) else {
                return;
            };
            // Inject the host theme so the pane's first painted frame already
            // reads `panel_theme()` (matches the steady-state tick loop).
            let panel_theme = self.theme.to_panel_theme();
            match self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) {
                Some(pv) => {
                    pv.note_activity(now);
                    pv.set_theme(panel_theme);
                    pv.tick(now, rect, self.viewport.cell);
                }
                None => return,
            }
            let (events, changed) = match self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) {
                Some(pv) => pv.pump_client(Some(Duration::from_millis(200))),
                None => return,
            };
            self.handle_client_events(idx, events);
            if !changed {
                break;
            }
        }
        self.needs_redraw = true;
    }

    /// Act on a script client's `openPath`/`setStatus`/`navigate` signals:
    /// `openPath` turns the pane into a normal editor (dropping the client) and
    /// re-syncs the layout, exactly like a Lines client's `openPath`; `setStatus`
    /// sets the status note; `navigate` resolves the target screen against the
    /// panel's script-directory whitelist and swaps the running screen in place
    /// ([`navigate_panel`](Self::navigate_panel)).
    pub(in crate::app) fn handle_client_events(&mut self, idx: usize, events: Vec<ClientEvent>) {
        let mut content_changed = false;
        for ev in events {
            match ev {
                ClientEvent::OpenPath(path) => {
                    if let Some(pane) = self.panes.get_mut(idx) {
                        pane.set_editor(Some(path));
                        content_changed = true;
                    }
                }
                ClientEvent::SetStatus(text) => self.status_note = Some(text),
                ClientEvent::Navigate(intent) => {
                    if self.navigate_panel(idx, intent) {
                        content_changed = true;
                    }
                }
                ClientEvent::Mutate { name, arg, handle } => {
                    self.mutate_panel(idx, &name, arg, handle)
                }
            }
        }
        if content_changed {
            self.sync_layout();
            self.needs_redraw = true;
        }
    }

    /// Drive browser-style history on the **focused** pane from a host affordance
    /// — the `Ctrl+[` / `Ctrl+]` keys and the `:back` / `:forward` commands, which
    /// pass `NavIntent::Back` / `NavIntent::Forward`. Unlike a script-issued
    /// intent (routed through [`handle_client_events`](Self::handle_client_events)),
    /// this originates in Garden, so it resolves the target pane here.
    ///
    /// A no-op with an explanatory status note when the focused pane is not a
    /// panel, or when history is already at the requested end. On a real move the
    /// layout is re-synced (mirroring the script-driven navigation path) and a
    /// redraw is requested. Always returns [`KeyOutcome::Handled`] — the key/command
    /// was consumed regardless of whether the cursor moved.
    pub(in crate::app) fn nav_focused_panel(&mut self, intent: NavIntent) -> super::KeyOutcome {
        let idx = self.focus;
        let dir = match intent {
            NavIntent::Back => "back",
            NavIntent::Forward => "forward",
            // Push/Replace never originate from a host affordance.
            _ => "",
        };
        if !self.panes.get(idx).is_some_and(|p| p.panel.is_some()) {
            self.status_note = Some("history: the focused pane has no history".to_string());
            self.needs_redraw = true;
            return super::KeyOutcome::Handled;
        }
        if self.navigate_panel(idx, intent) {
            // A navigated screen leaves the pane's origin (layout-declared) script
            // untouched, so this rewrite is a no-op for persistence — kept only to
            // match the script-driven path in `handle_client_events`.
            self.sync_layout();
        } else {
            self.status_note = Some(format!("history: nothing {dir}"));
        }
        self.needs_redraw = true;
        super::KeyOutcome::Handled
    }

    /// Act on a panel's browser-history navigation intent for pane `idx`.
    ///
    /// `Push`/`Replace` name a target screen — an untrusted string from the panel
    /// script — which is resolved against the pane's **whitelist**: the origin
    /// panel's own script directory. The target is rejected (a no-op, with the
    /// reason surfaced in the status note) unless it is a directory-relative
    /// `.ptl` file that resolves (symlinks included) to a still-existing path
    /// under that directory; absolute paths, `..` traversal, non-`.ptl` names,
    /// symlink escapes, and missing files are all refused. On accept the source
    /// is read and the screen swapped in place via the panel's history stack.
    ///
    /// `Back`/`Forward` need no resolution — they move the existing history cursor
    /// and are a no-op at the ends. Returns whether the running screen changed
    /// (so the caller re-syncs the layout and redraws). Navigation leaves the
    /// panel's *origin* script (its layout-declared path) untouched, so a later
    /// layout sync still records the pane by that origin screen, not the
    /// navigated-to one.
    fn navigate_panel(&mut self, idx: usize, intent: NavIntent) -> bool {
        let (screen, arg, replace) = match intent {
            NavIntent::Push(screen, arg) => (screen, arg, false),
            NavIntent::Replace(screen, arg) => (screen, arg, true),
            NavIntent::Back => return self.nav_history(idx, false),
            NavIntent::Forward => return self.nav_history(idx, true),
        };
        // How the target screen's source is resolved differs by pane kind:
        //  - A **subprocess** (pushed-script) pane has no on-disk screens; the
        //    client owns them. Fetch the source over the pipe via the built-in
        //    `navigate` mutation (the client's declared screens are its allowlist).
        //  - An **in-process** `panel(...)` pane resolves a sibling `.ptl` against
        //    its origin script directory, narrowed by the explicit `screens` list.
        let is_client = self
            .panes
            .get(idx)
            .and_then(|p| p.panel.as_ref())
            .is_some_and(|pv| pv.has_client());
        let source = if is_client {
            match self
                .panes
                .get_mut(idx)
                .and_then(|p| p.panel.as_mut())
                .map(|pv| pv.client_fetch_screen(&screen, &arg, NAV_MUTATION_TIMEOUT))
            {
                Some(Ok(source)) => source,
                Some(Err(reason)) => {
                    self.status_note = Some(format!("navigate rejected: {reason}"));
                    return false;
                }
                None => return false,
            }
        } else {
            let (root, allowlist) = match self.panes.get(idx).and_then(|p| p.panel.as_ref()) {
                // The whitelist root is the ORIGIN screen's own **resolved**
                // directory, never the live one: after the first navigation the
                // live screen is a bare `.ptl` name with no directory to resolve
                // siblings within. Using the origin's resolved path (not its
                // layout-declared string) is what makes `panel("clock.ptl")` — a
                // bare relative name whose parent is empty — resolve against the
                // layout script's directory rather than the process CWD. The
                // explicit `screens` allowlist (empty = not declared) narrows that
                // directory default when present.
                Some(pv) => {
                    let root = pv
                        .origin_path()
                        .and_then(std::path::Path::parent)
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    (root, pv.screens().to_vec())
                }
                None => return false,
            };
            match resolve_screen(&root, &screen, &allowlist) {
                Ok(source) => source,
                Err(reason) => {
                    self.status_note = Some(reason);
                    return false;
                }
            }
        };
        match self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) {
            Some(pv) => {
                if replace {
                    pv.nav_replace(screen, source, arg);
                } else {
                    pv.nav_push(screen, source, arg);
                }
                true
            }
            None => false,
        }
    }

    /// Move pane `idx`'s history cursor one step and, for a **subprocess**
    /// panel, re-issue the restored entry's `navigate` mutation.
    ///
    /// Restoring an entry replays the *host's* record of it — the source, the
    /// `state` snapshot, the navigation argument — but the client that served
    /// that screen keeps its own state, and an app whose `on_mutation("navigate")`
    /// handler primes the data a screen reads would otherwise never learn about
    /// the revisit: the screen would come back drawn from whatever the provider
    /// happens to hold now, which is the stale-identity bug that made apps poll
    /// a `selection` query every frame instead of latching it. Re-issuing makes
    /// *back* and *forward* as much a navigation to the client as the original
    /// push was, and the entry's own argument is what gets replayed, so each
    /// entry re-primes its own subject.
    ///
    /// Best effort by design: the cursor has already moved when the mutation is
    /// sent, so a client that is gone, slow, or rejects the screen leaves the
    /// restored entry showing its cached source — with the reason in the status
    /// note — rather than failing a navigation the user explicitly asked for.
    /// A fresh source swaps the running program in; an identical one costs
    /// nothing. In-process `panel(...)` panes have no provider to re-ask and are
    /// unaffected, as is the seed entry (see [`PanelView::restored_entry`]).
    fn nav_history(&mut self, idx: usize, forward: bool) -> bool {
        let moved = match self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) {
            Some(pv) => {
                if forward {
                    pv.nav_forward()
                } else {
                    pv.nav_back()
                }
            }
            None => false,
        };
        if !moved {
            return false;
        }
        let target = self
            .panes
            .get(idx)
            .and_then(|p| p.panel.as_ref())
            .filter(|pv| pv.has_client())
            .and_then(|pv| pv.restored_entry());
        let Some((screen, arg)) = target else {
            return true;
        };
        let fetched = self
            .panes
            .get_mut(idx)
            .and_then(|p| p.panel.as_mut())
            .map(|pv| pv.client_fetch_screen(&screen, &arg, NAV_MUTATION_TIMEOUT));
        match fetched {
            Some(Ok(source)) => {
                if let Some(pv) = self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) {
                    pv.refresh_current_source(source);
                }
            }
            Some(Err(reason)) => {
                self.status_note = Some(format!(
                    "navigate replay failed: {reason} (showing the cached screen)"
                ));
            }
            None => {}
        }
        true
    }

    /// Relay a script `mutate(name, arg)` on pane `idx` to its subprocess and
    /// surface the reply as the pane's status: an `on_mutation` handler's success
    /// value becomes a status note (e.g. a save's "wrote 2 files"), an error (or a
    /// timeout / no-subprocess panel) a status error. Blocks briefly
    /// ([`MUTATE_TIMEOUT`]) for the round-trip, like the navigate mutation. This is
    /// the write-back path for editable panels: a drawer reading `edit_view_text`
    /// and calling `mutate("save", …)` reaches its subprocess here.
    ///
    /// A short list of names is answered by the **host** first, without ever
    /// reaching a subprocess ([`host_mutation`](Self::host_mutation)) — an
    /// in-process `panel(...)` pane has no client, and `emit(...)` is dropped for
    /// it, so `mutate` is the only channel such a panel has to ask Garden to act.
    /// Every other name keeps forwarding, so a client's own mutations (garden-diff's
    /// `"apply"`, a drawer's `"save"`) are untouched.
    /// Report a mutation's outcome back to the panel that raised it, under the
    /// handle its `mutate(...)` returned. Missing pane / missing panel is simply
    /// nothing to report to.
    fn resolve_panel_mutation(
        &mut self,
        idx: usize,
        handle: i64,
        result: Result<Option<String>, String>,
    ) {
        if let Some(pv) = self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) {
            pv.resolve_mutation(handle, result);
        }
    }

    pub(in crate::app) fn mutate_panel(
        &mut self,
        idx: usize,
        name: &str,
        arg: serde_json::Value,
        handle: i64,
    ) {
        if self.host_mutation(name, &arg) {
            // Host-answered names resolve immediately and successfully; the
            // script still learns so, under the same handle.
            self.resolve_panel_mutation(idx, handle, Ok(None));
            self.needs_redraw = true;
            return;
        }
        let outcome = self
            .panes
            .get_mut(idx)
            .and_then(|p| p.panel.as_mut())
            .map(|pv| pv.client_mutate(name, arg, MUTATE_TIMEOUT));
        if let Some(outcome) = outcome.as_ref() {
            self.resolve_panel_mutation(idx, handle, outcome.clone());
        }
        match outcome {
            Some(Ok(Some(status))) => {
                self.status_note = Some(status);
                self.status_error = None;
            }
            Some(Ok(None)) => {
                self.status_note = Some(format!("{name}: done"));
                self.status_error = None;
            }
            Some(Err(reason)) => self.status_error = Some(format!("{name}: {reason}")),
            None => {}
        }
        self.needs_redraw = true;
    }

    /// Answer the mutations the **host** owns, returning whether `name` was one
    /// of them (an unrecognized name is left to the subprocess). These are the
    /// app actions a panel screen — the start screen's recent-files list, say —
    /// has no other way to reach:
    ///
    /// | name | arg | effect |
    /// |---|---|---|
    /// | `open_path` | `{ "path": "…" }` | [`open_path`](App::open_path): the file replaces the focused pane |
    /// | `open_project` | `{ "path": "…" }` | record the project, then browse it (as File ▸ Open Folder) |
    /// | `open_pr` | `{ "number": 12 }` | [`open_garden_diff`](App::open_garden_diff) on `--pr <n>` |
    /// | `open_file_dialog` | `{ "mode": "file" \| "folder" }` | native picker, then the matching open above |
    ///
    /// A malformed or missing argument is a status *error*, never a panic: the
    /// arg is JSON a script built, so every field is untrusted. A cancelled
    /// picker is an ordinary note — the user chose nothing, which is not a fault.
    fn host_mutation(&mut self, name: &str, arg: &serde_json::Value) -> bool {
        let path_arg = arg.get("path").and_then(serde_json::Value::as_str);
        match name {
            "open_path" => match path_arg {
                Some(path) => self.open_path_from_panel(&PathBuf::from(path)),
                None => self.status_error = Some("open_path: expected a `path` string".to_string()),
            },
            "open_project" => match path_arg {
                Some(path) => self.open_project_from_panel(&PathBuf::from(path)),
                None => {
                    self.status_error = Some("open_project: expected a `path` string".to_string())
                }
            },
            "open_pr" => match arg.get("number").and_then(serde_json::Value::as_i64) {
                Some(number) => {
                    self.open_garden_diff(vec!["--pr".to_string(), number.to_string()]);
                    self.status_note = Some(format!("PR #{number}"));
                    self.status_error = None;
                }
                None => {
                    self.status_error = Some("open_pr: expected a `number` integer".to_string())
                }
            },
            "open_file_dialog" => {
                let mode = arg
                    .get("mode")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("file");
                self.open_file_dialog(mode);
            }
            _ => return false,
        }
        true
    }

    /// Open a file a panel named. Split out of
    /// [`host_mutation`](Self::host_mutation) so the file picker's result takes
    /// the identical path, status note included.
    fn open_path_from_panel(&mut self, path: &Path) {
        self.open_path(&path.to_string_lossy());
        self.status_note = Some(format!("opened {}", path.display()));
        self.status_error = None;
    }

    /// Open a directory a panel named as a project — the same pair of steps as
    /// [`MenuAction::OpenFolder`](crate::app::MenuAction::OpenFolder): record it
    /// as a project (the user is naming one, so it belongs in the recents list)
    /// and browse it in the focused pane.
    fn open_project_from_panel(&mut self, path: &Path) {
        let dir = path.to_string_lossy();
        self.record_project_opened(&dir);
        self.open_directory_browser(&dir);
        self.status_note = Some(format!("opened {dir}"));
        self.status_error = None;
    }

    /// Pop the native picker for `open_file_dialog` and open what comes back.
    ///
    /// Refused outright unless a windowed frontend enabled it
    /// ([`enable_native_dialogs`](App::enable_native_dialogs)): the dialog blocks
    /// this thread until the user answers, so under `--headless`/`--term` (and in
    /// tests) it would hang the editor with no window to answer from. The panel
    /// hears about that as a status error rather than freezing.
    fn open_file_dialog(&mut self, mode: &str) {
        if !self.native_dialogs {
            self.status_error =
                Some("open_file_dialog: no native file picker without a window".to_string());
            return;
        }
        let picked = match mode {
            "file" => crate::file_dialog::pick_file().map(|p| (p, false)),
            "folder" => crate::file_dialog::pick_folder().map(|p| (p, true)),
            other => {
                self.status_error = Some(format!("open_file_dialog: unknown mode '{other}'"));
                return;
            }
        };
        match picked {
            Some((path, true)) => self.open_project_from_panel(&path),
            Some((path, false)) => self.open_path_from_panel(&path),
            None => {
                self.status_note = Some("open cancelled".to_string());
                self.status_error = None;
            }
        }
    }

    /// The directory the focused pane should browse when opening the directory
    /// browser (`:E` / `-`): the focused file's parent directory, or the
    /// current working directory (`.`) for a pathless buffer or a file with no
    /// usable parent.
    pub(in crate::app) fn focused_browse_dir(&self) -> String {
        let file = self.panes.get(self.focus).and_then(|p| p.file.as_deref());
        match file
            .map(std::path::Path::new)
            .and_then(std::path::Path::parent)
        {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_string_lossy().into_owned(),
            _ => ".".to_string(),
        }
    }

    /// Replace the focused pane with the GPP directory browser listing `dir`
    /// (the `:E` command and the `-` key, vim-netrw style). The pane's editor
    /// view becomes the passive render surface the browser drives; selecting a
    /// file there reopens the pane as an editor via the normal `openPath` flow.
    /// On spawn failure the current buffer is left untouched and the error
    /// surfaces in the status bar.
    pub(in crate::app) fn open_directory_browser(&mut self, dir: &str) {
        self.open_browser(
            process_pane::directory_browser_bin(),
            vec![dir.to_string()],
            "directory browser",
            " (build the workspace with `cargo build` so `directory-browser` \
             is placed next to `garden`, or set GARDEN_DIRECTORY_BROWSER_BIN)",
        );
    }

    /// Replace the focused pane with the `git-log` panel-mode GPP client — the
    /// `:Git` history browser (commit list + per-commit files + numbered diff),
    /// rooted at the focused file's repository. The client pushes the production
    /// history drawer the host runs in-process and answers its `query("log")` /
    /// `query("commit", …)` requests by shelling `git`. On spawn failure the pane
    /// is left untouched and the error surfaces in the status bar.
    pub(in crate::app) fn open_git_viewer(&mut self) {
        let dir = self.focused_browse_dir();
        self.open_browser(
            process_pane::git_log_bin(),
            vec![dir],
            "git viewer",
            " (build the workspace with `cargo build` so `git-log` is placed next \
             to `garden`, or set GARDEN_GIT_LOG_BIN)",
        );
    }

    /// Replace the focused pane with the `garden-diff` panel-mode GPP client — the
    /// editable before/after (and unified) review, rooted at the focused file's
    /// repository. `extra` is the client args after the repo dir: a base ref for
    /// `:Diff`/`:Review*`, or `--pr [number]` for `:PR`. The client answers its
    /// `query("doc")` by shelling `git`/`gh` and writes edited files back on `^S`.
    /// On spawn failure the pane is left untouched and the error surfaces in the
    /// status bar.
    pub(in crate::app) fn open_garden_diff(&mut self, extra: Vec<String>) {
        let dir = self.focused_browse_dir();
        // `--pr` with no number means "whatever PR this branch has", which only
        // `gh` can resolve — there is no identity to record yet, so don't.
        if extra.first().is_some_and(|a| a == "--pr") {
            if let Some(number) = extra.get(1).and_then(|n| n.parse::<i64>().ok()) {
                self.record_pr_opened(&dir, number);
            }
        }
        let mut args = vec![dir];
        args.extend(extra);
        self.open_browser(
            process_pane::garden_diff_bin(),
            args,
            "diff review",
            " (build the workspace with `cargo build` so `garden-diff` is placed \
             next to `garden`, or set GARDEN_DIFF_BIN)",
        );
    }

    /// Swap the focused pane to a GPP client backed by `command` rooted at `dir`
    /// (used by `:E`, `:PR`, `:Git`, `:Diff`): spawn the client and adopt whichever
    /// [`ClientMode`](gpp::ClientMode) it reports, then persist so a later
    /// split/reload keeps the pane. On spawn failure the pane is left untouched and
    /// a `could not start {what}` error (with a `NotFound` build hint) surfaces in
    /// the status bar.
    ///
    /// - A **Lines** client (`directory-browser`) becomes a process-backed pane
    ///   via [`Pane::set_process`](super::Pane::set_process), whose initial
    ///   `render` is drained so its listing shows immediately.
    /// - A **Panel** client (`git-log`, `garden-diff`) pushes a Petal UI script the
    ///   host runs in-process; the pane becomes a script client
    ///   ([`build_script_client_pane`]) primed with its first frame of data.
    fn open_browser(
        &mut self,
        command: String,
        args: Vec<String>,
        what: &str,
        not_found_hint: &str,
    ) {
        let cell = self.viewport.cell;
        let idx = self.focus;

        let Some(pane) = self.panes.get(idx) else {
            return;
        };
        let rect = pane.rect;
        let (rows, cols) = process_dims(&pane.view, rect, cell);
        let process = match ProcessPane::spawn(&command, &args, idx as u64, rows, cols) {
            Ok(process) => process,
            Err(err) => {
                let mut msg = format!("could not start {what}: {err}");
                if err.kind() == std::io::ErrorKind::NotFound {
                    msg.push_str(not_found_hint);
                }
                self.status_error = Some(msg);
                self.needs_redraw = true;
                return;
            }
        };

        match process.mode() {
            gpp::ClientMode::Panel => {
                // The client pushes a Petal UI script; build a script-client pane
                // and prime its first frame of query data so it paints immediately.
                let view = EditorView::open(None);
                let pane =
                    super::panes::build_script_client_pane(rect, view, process, command, args);
                if let Some(slot) = self.panes.get_mut(idx) {
                    *slot = pane;
                }
                self.prime_script_client(idx);
            }
            gpp::ClientMode::Lines => {
                if let Some(pane) = self.panes.get_mut(idx) {
                    pane.set_process(process, command, args);
                }
                // Drain the client's initial render so its listing shows immediately.
                if let Some(process) = self.panes.get(idx).and_then(|p| p.process.as_ref()) {
                    let msgs = process.drain_for(Duration::from_millis(250));
                    self.apply_process_messages(idx, msgs);
                }
            }
        }
        // The pane became a client; persist so a later split/reload keeps it one.
        self.sync_layout();
        self.needs_redraw = true;
    }
}

/// Resolve a panel navigation target against its whitelist.
///
/// `origin_script` is the panel's layout-declared origin `.ptl`; its parent
/// directory is the *only* allowed root. `screen` is untrusted input from the
/// panel script. `allowlist` is the panel's optional explicit `screens` list:
/// when non-empty it *narrows* the directory default — `screen` must be a
/// member — and when empty the implicit "any `.ptl` in the directory" default
/// applies. On success the target's source is returned; otherwise a
/// human-readable rejection reason (surfaced in the status note by the caller).
///
/// The explicit list only ever *narrows*: a listed entry still must pass every
/// safety check below, so declaring `screens` can never widen access past the
/// script directory. The membership test runs first (cheapest, and it is what an
/// author most expects to gate), then the layered path defense — the target must
/// be a relative path (not absolute), contain no `..` component, end in `.ptl`,
/// and — after canonicalizing both it and the root, which follows symlinks —
/// resolve to an existing file still under the root. Canonicalizing the root too
/// (rather than string-prefixing the join) is what makes the containment check
/// hold on platforms where the temp/root dir is itself a symlink, and what
/// catches a `screen` that is a symlink pointing outside the directory.
/// Resolve a navigation target `screen` (an untrusted string from a panel script)
/// against `root` (the panel's origin script directory) and read its source.
///
/// Layered defense, each a no-op-on-reject: an explicit `screens` allowlist (empty
/// = not declared) must contain the target; the target must be a relative `.ptl`
/// name with no `..`; and — after canonicalizing both `root` and the joined target
/// (resolving symlinks) — the target must still live under `root`. Absolute paths,
/// `..` traversal, symlink escapes, non-`.ptl` names, and missing files are refused.
fn resolve_screen(
    root: &std::path::Path,
    screen: &str,
    allowlist: &[String],
) -> Result<String, String> {
    use std::path::{Component, Path};

    // An explicit `screens: [...]` list narrows the directory default: a target
    // that is not a declared member is refused before any path resolution. An
    // empty list means no explicit declaration, so this gate is skipped and the
    // implicit script-directory default (enforced below) stands alone.
    if !allowlist.is_empty() && !allowlist.iter().any(|s| s == screen) {
        return Err(format!(
            "navigate rejected: '{screen}' is not in the panel's declared screens"
        ));
    }

    let candidate = Path::new(screen);
    if candidate.is_absolute() {
        return Err(format!("navigate rejected: '{screen}' is an absolute path"));
    }
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(format!(
            "navigate rejected: '{screen}' escapes the screen directory"
        ));
    }
    if candidate.extension().and_then(|e| e.to_str()) != Some("ptl") {
        return Err(format!(
            "navigate rejected: '{screen}' is not a .ptl screen"
        ));
    }

    let canon_root = root
        .canonicalize()
        .map_err(|_| "navigate rejected: screen directory is unavailable".to_string())?;
    let canon_target = root
        .join(candidate)
        .canonicalize()
        .map_err(|_| format!("navigate rejected: '{screen}' does not exist"))?;
    if !canon_target.starts_with(&canon_root) {
        return Err(format!(
            "navigate rejected: '{screen}' escapes the screen directory"
        ));
    }
    std::fs::read_to_string(&canon_target)
        .map_err(|e| format!("navigate rejected: cannot read '{screen}': {e}"))
}

#[cfg(test)]
mod resolve_screen_tests {
    use super::resolve_screen;
    use std::fs;

    /// A directory with the given `.ptl` files, each holding its own name as body.
    fn screen_dir(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in names {
            fs::write(dir.path().join(name), format!("// {name}\n")).unwrap();
        }
        dir
    }

    #[test]
    fn accepts_an_existing_in_directory_ptl() {
        let dir = screen_dir(&["a.ptl", "b.ptl"]);
        let src = resolve_screen(dir.path(), "b.ptl", &[]).unwrap();
        assert_eq!(src, "// b.ptl\n");
    }

    #[test]
    fn rejects_absolute_traversal_missing_and_non_ptl() {
        let dir = screen_dir(&["a.ptl"]);
        for bad in [
            "/etc/passwd",
            "../a.ptl",
            "sub/../../a.ptl",
            "missing.ptl",
            "a.txt",
        ] {
            assert!(
                resolve_screen(dir.path(), bad, &[]).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn declared_screens_list_narrows_but_still_enforces_safety() {
        let dir = screen_dir(&["a.ptl", "b.ptl"]);
        let allow = vec!["a.ptl".to_string()];
        // In the directory but not on the declared list -> rejected.
        assert!(resolve_screen(dir.path(), "b.ptl", &allow).is_err());
        // On the list -> accepted.
        assert!(resolve_screen(dir.path(), "a.ptl", &allow).is_ok());
        // On the list but still failing a safety check (missing) -> rejected.
        let allow_missing = vec!["gone.ptl".to_string()];
        assert!(resolve_screen(dir.path(), "gone.ptl", &allow_missing).is_err());
    }

    /// A symlink inside the screen directory that points outside it must not be a
    /// navigation escape hatch: canonicalizing both sides catches it.
    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_escaping_the_screen_directory() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.ptl"), "// secret\n").unwrap();
        let dir = screen_dir(&["a.ptl"]);
        std::os::unix::fs::symlink(
            outside.path().join("secret.ptl"),
            dir.path().join("escape.ptl"),
        )
        .unwrap();
        assert!(
            resolve_screen(dir.path(), "escape.ptl", &[]).is_err(),
            "a symlink pointing outside the screen dir must be rejected"
        );
    }
}
