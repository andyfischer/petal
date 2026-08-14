//! Host side of the Garden Pane Protocol (GPP): one child process driving the
//! text content of a pane over newline-delimited JSON-RPC on stdio.
//!
//! [`ProcessPane`] owns the child, a writer over its stdin (host -> client),
//! and a background reader thread that forwards every [`gpp::Envelope`] it
//! reads from stdout to an mpsc channel (client -> host), mirroring the
//! thread+channel shape of [`crate::debug`]. The host pushes subscribed keys
//! and resizes; the client answers with `render` / `setKeymap` / `openPath` /
//! `setStatus` notifications, which [`crate::app::App`] drains and applies.
//!
//! All wire failures (a broken pipe means the child is gone) are swallowed and
//! logged to stderr rather than panicking, so a misbehaving client can never
//! take the editor down.

use std::io::{BufReader, BufWriter};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use gpp::{
    method, ClientMode, EmitParams, Envelope, InitializeParams, InitializeResult, Key, KeyParams,
    MouseKind, MouseParams, MutateParams, QueryParams, ResizeParams, Takeover,
};

/// A pane whose content is driven by a GPP child process.
pub struct ProcessPane {
    child: Child,
    /// Writer over the child's stdin (host -> client messages).
    stdin: BufWriter<ChildStdin>,
    /// Envelopes read from the child's stdout by the reader thread.
    rx: Receiver<Envelope>,
    /// Keys this client wants forwarded (canonical GPP key names). Consulted
    /// only at the [`Takeover::Keymap`] level; ignored under `Keyboard`, which
    /// forwards everything non-reserved.
    keymap: Vec<String>,
    /// Which rendering model this client drives. `Lines` clients push `render`;
    /// `Panel` clients push a Petal UI script and answer `query` requests. See
    /// [`gpp::ClientMode`] and `docs/gpp.md`.
    mode: ClientMode,
    /// How much host behavior this client takes over. See [`gpp::Takeover`].
    takeover: Takeover,
    /// Whether this client opted into `mouse` notifications (clicks on the
    /// pane's content are forwarded instead of moving the passive cursor).
    mouse: bool,
    /// Display name reported in the `initialize` response.
    name: String,
    /// This pane's id, handed to the client at initialize time.
    #[allow(dead_code)]
    pane_id: u64,
    /// Next request id (the initialize request used id 1).
    next_id: u64,
}

impl ProcessPane {
    /// Spawn `command` with `args`, perform the synchronous `initialize`
    /// handshake, and start the reader thread.
    ///
    /// The handshake writes an `initialize` request (id 1) and reads exactly one
    /// message directly from stdout, expecting the matching response carrying an
    /// [`InitializeResult`]; only then is the reader thread started. If the child
    /// dies or answers with the wrong message an [`std::io::Error`] is returned.
    pub fn spawn(
        command: &str,
        args: &[String],
        pane_id: u64,
        rows: u32,
        cols: u32,
    ) -> std::io::Result<ProcessPane> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let mut stdin = BufWriter::new(child.stdin.take().expect("child stdin was piped"));
        let stdout = child.stdout.take().expect("child stdout was piped");
        let mut reader = BufReader::new(stdout);

        // Synchronous handshake: send `initialize`, then block on the one
        // response the client must send before anything else.
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let init = Envelope::request(
            1,
            method::INITIALIZE,
            InitializeParams {
                pane_id,
                rows,
                cols,
                args: args.to_vec(),
                cwd,
            },
        );
        gpp::write_message(&mut stdin, &init)?;

        let response = gpp::read_message(&mut reader)?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "GPP client exited before responding to initialize",
            )
        })?;
        let result: InitializeResult = response.result_as().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("GPP initialize response was not an InitializeResult: {e}"),
            )
        })?;

        // Hand the live reader to a thread that forwards every envelope until
        // the child closes stdout.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(Some(env)) = gpp::read_message(&mut reader) {
                if tx.send(env).is_err() {
                    break; // the ProcessPane was dropped
                }
            }
        });

        Ok(ProcessPane {
            child,
            stdin,
            rx,
            keymap: result.keymap,
            mode: result.mode,
            takeover: result.takeover,
            mouse: result.mouse,
            name: result.name,
            pane_id,
            next_id: 2,
        })
    }

    /// Forward a subscribed key press to the client as a `key` notification.
    pub fn send_key(&mut self, key: Key, ctrl: bool, shift: bool, cmd: bool) {
        self.notify(
            method::KEY,
            KeyParams {
                key: key.to_name(),
                ctrl,
                shift,
                cmd,
            },
        );
    }

    /// Tell the client its pane was resized.
    pub fn send_resize(&mut self, rows: u32, cols: u32) {
        self.notify(method::RESIZE, ResizeParams { rows, cols });
    }

    /// Forward a mouse press on content row `line` (0-based, scroll-adjusted)
    /// at char column `col` as a `mouse` notification. Only called when the
    /// client opted in (see [`mouse`](Self::mouse)).
    pub fn send_mouse(&mut self, line: usize, col: usize, kind: MouseKind) {
        self.notify(method::MOUSE, MouseParams { line, col, kind });
    }

    /// Which rendering model this client drives (`Lines` vs `Panel`).
    pub fn mode(&self) -> ClientMode {
        self.mode
    }

    /// Send a `query` **request** (host -> client) for `(kind, arg)` — the pipe
    /// side of the panel data channel. The client answers with a `queryResult`
    /// response (correlated by the echoed `kind`/`arg`, not the id). Only used
    /// for [`ClientMode::Panel`] clients.
    pub fn send_query(&mut self, kind: &str, arg: &str) {
        let id = self.next_id;
        self.next_id += 1;
        let env = Envelope::request(
            id,
            method::QUERY,
            QueryParams {
                kind: kind.to_string(),
                arg: arg.to_string(),
            },
        );
        if let Err(err) = gpp::write_message(&mut self.stdin, &env) {
            eprintln!("garden: GPP query to {} failed: {err}", self.name);
        }
    }

    /// Forward a script-emitted user intent to the client as an `emit`
    /// notification (host -> client). Fire-and-forget — a notification, so no
    /// reply is expected (or possible). `arg` is the JSON tree the script
    /// passed to `emit(event, arg)`. Only used for [`ClientMode::Panel`]
    /// clients: [`PanelView::tick`](crate::panel_view::PanelView::tick) drains
    /// each frame's emitted events here.
    pub fn send_emit(&mut self, event: &str, arg: serde_json::Value) {
        self.notify(
            method::EMIT,
            EmitParams {
                event: event.to_string(),
                arg,
            },
        );
    }

    /// Send a `mutate` **request** (host -> client) — an effectful call the host
    /// makes on the script's behalf, awaiting a `mutateResult` response. Returns
    /// the request `id` so the caller can correlate the answer (mutations, unlike
    /// queries, are not idempotent by content, so they correlate by id). Only used
    /// for [`ClientMode::Panel`] clients; the host drives navigation with the
    /// built-in `navigate` mutation.
    pub fn send_mutate(&mut self, name: &str, arg: serde_json::Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let env = Envelope::request(
            id,
            method::MUTATE,
            MutateParams {
                name: name.to_string(),
                arg,
            },
        );
        if let Err(err) = gpp::write_message(&mut self.stdin, &env) {
            eprintln!("garden: GPP mutate to {} failed: {err}", self.name);
        }
        id
    }

    /// Non-blocking: collect every envelope the reader thread has queued so far.
    pub fn try_drain(&self) -> Vec<Envelope> {
        self.rx.try_iter().collect()
    }

    /// Block up to `dur` for the first envelope, then drain the rest without
    /// waiting. Used right after sending a key so the client's reply feels
    /// synchronous.
    pub fn drain_for(&self, dur: Duration) -> Vec<Envelope> {
        let mut out = Vec::new();
        if let Ok(env) = self.rx.recv_timeout(dur) {
            out.push(env);
            out.extend(self.rx.try_iter());
        }
        out
    }

    /// Keys this client wants forwarded (canonical GPP key names).
    pub fn keymap(&self) -> &[String] {
        &self.keymap
    }

    /// Replace the forwarded-key set (a `setKeymap` notification arrived).
    pub fn set_keymap(&mut self, keys: Vec<String>) {
        self.keymap = keys;
    }

    /// How much host behavior this client takes over.
    pub fn takeover(&self) -> Takeover {
        self.takeover
    }

    /// Change the takeover level at runtime (a `setKeymap` notification carried
    /// a new `takeover`).
    pub fn set_takeover(&mut self, takeover: Takeover) {
        self.takeover = takeover;
    }

    /// Whether this client opted into mouse-click forwarding.
    pub fn mouse(&self) -> bool {
        self.mouse
    }

    /// Change the mouse opt-in at runtime (a `setKeymap` notification carried
    /// a new `mouse`).
    pub fn set_mouse(&mut self, mouse: bool) {
        self.mouse = mouse;
    }

    /// Whether this client subscribed to the canonical key `name` (relevant
    /// only at the [`Takeover::Keymap`] level).
    pub fn subscribes(&self, name: &str) -> bool {
        self.keymap.iter().any(|k| k == name)
    }

    /// The client's display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Send one notification, swallowing a broken pipe (the child has gone).
    fn notify(&mut self, method: &str, params: impl serde::Serialize) {
        // Bump the id even though notifications carry none, keeping the counter
        // monotonic for any future request use.
        self.next_id += 1;
        let env = Envelope::notification(method, params);
        if let Err(err) = gpp::write_message(&mut self.stdin, &env) {
            eprintln!("garden: GPP write to {} failed: {err}", self.name);
        }
    }
}

/// Resolve the directory-browser GPP client binary. Resolution order:
///
/// 1. `$GARDEN_DIRECTORY_BROWSER_BIN`, if set — an explicit override for
///    installs, packaging, or tests that place the client elsewhere.
/// 2. A `directory-browser` next to the running executable — the normal case
///    after a workspace `cargo build` or a side-by-side install.
/// 3. The bare name `directory-browser`, resolved through `$PATH`.
///
/// Note: `directory-browser` is a *separate* workspace binary and is
/// deliberately not a dependency of garden-app, so `cargo run -p garden-app`
/// (and release builds) do **not** build it. Build the whole workspace
/// (`cargo build`) so the client lands beside `garden`. Used by both the
/// `garden <dir>` startup path and the on-demand `:E` / `-` browser open.
pub fn directory_browser_bin() -> String {
    sibling_bin("directory-browser", "GARDEN_DIRECTORY_BROWSER_BIN")
}

/// Resolve the `git-log` GPP client binary — the panel-mode app that backs
/// `:Git` / `garden git log` — by the same rules as [`directory_browser_bin`]:
/// `$GARDEN_GIT_LOG_BIN`, else a sibling of the running executable, else the bare
/// name on `$PATH`. A separate workspace binary (in `gpp-apps/git-viewers`);
/// build the whole workspace (`cargo build`) so it lands beside `garden`.
pub fn git_log_bin() -> String {
    sibling_bin("git-log", "GARDEN_GIT_LOG_BIN")
}

/// Resolve the `garden-diff` GPP client binary — the one diff/review tool, behind
/// `:Diff`, `:Review*`, `:PR`, `garden diff`, and `garden pr` — by the same rules as
/// [`directory_browser_bin`]: `$GARDEN_DIFF_BIN`, else a sibling of the running
/// executable, else the bare name on `$PATH`. A separate workspace binary (in
/// `gpp-apps/garden-diff`); build the whole workspace so it lands beside `garden`.
pub fn garden_diff_bin() -> String {
    sibling_bin("garden-diff", "GARDEN_DIFF_BIN")
}

/// Resolve the `main-menu` GPP client binary — the panel-mode app a bare
/// `garden` opens (recent projects / files / PRs) — by the same rules as
/// [`directory_browser_bin`]: `$GARDEN_MAIN_MENU_BIN`, else a sibling of the
/// running executable, else the bare name on `$PATH`. A separate workspace
/// binary (in `gpp-apps/main-menu`); build the whole workspace so it lands
/// beside `garden`. Because this one backs the *default* launch, callers pair it
/// with [`client_bin_exists`] and fall back to the init-script layout when the
/// binary is missing — Garden must launch either way.
pub fn main_menu_bin() -> String {
    sibling_bin("main-menu", "GARDEN_MAIN_MENU_BIN")
}

/// Whether a resolved client binary (as returned by [`main_menu_bin`] & co.)
/// names something that exists: a path is checked directly, a bare name is
/// looked up across `$PATH` the way the spawn would. Only worth calling when a
/// missing client has a graceful fallback — spawning and failing is otherwise
/// the cheaper check.
pub fn client_bin_exists(bin: &str) -> bool {
    if bin.contains(std::path::MAIN_SEPARATOR) {
        return std::path::Path::new(bin).exists();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(bin).exists())
}

/// Resolve a user-supplied `--subprocess <cmd>` binary. A `cmd` that carries a
/// path separator (absolute or relative) is used verbatim; a bare name is
/// resolved beside the running `garden` — so `garden --subprocess sqlite-browser
/// …` finds the sibling that `cargo build` drops next to `garden` — falling back
/// to the bare name (i.e. `$PATH` resolution at spawn) when no sibling exists.
/// This is the generic counterpart of [`git_log_bin`] & co. for arbitrary GPP
/// clients launched from the command line.
pub fn resolve_client_bin(name: &str) -> String {
    if name.contains(std::path::MAIN_SEPARATOR) {
        return name.to_string();
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)));
    match beside {
        Some(path) if path.exists() => path.to_string_lossy().into_owned(),
        _ => name.to_string(),
    }
}

/// Resolve a GPP client binary named `name`: an explicit `$<env>` override, else
/// a sibling of the running executable, else the bare name resolved on `$PATH`.
fn sibling_bin(name: &str, env: &str) -> String {
    if let Some(path) = std::env::var_os(env) {
        return path.to_string_lossy().into_owned();
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)));
    match beside {
        Some(path) if path.exists() => path.to_string_lossy().into_owned(),
        _ => name.to_string(),
    }
}

impl Drop for ProcessPane {
    fn drop(&mut self) {
        // Best-effort graceful shutdown: ask the client to exit, then drop
        // stdin (EOF) so it stops even if it ignored the notification.
        let env = Envelope::notification(method::SHUTDOWN, serde_json::json!({}));
        let _ = gpp::write_message(&mut self.stdin, &env);
        // Backstop in case the client neither honored shutdown nor EOF.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
