//! directory-browser — the first GPP client.
//!
//! A navigable directory listing rendered into a Garden pane, in the spirit of
//! vim's netrw. The host spawns this process, hands it a directory in the
//! `initialize` request, and forwards a subscribed set of navigation keys. The
//! client pushes the full listing back via `render` notifications and, when the
//! user selects a file, asks the host to open it via `openPath`.
//!
//! The file is split into a pure [`Browser`] core (all listing/movement logic,
//! unit-tested with no stdio) and a thin [`run`] stdio loop that wires the core
//! to the GPP transport.

use std::path::{Path, PathBuf};

use gpp::{
    method, Envelope, InitializeParams, InitializeResult, Key, KeyParams, MouseKind, MouseParams,
    OpenPathParams, RenderParams, Takeover,
};

/// One entry in a directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    is_dir: bool,
}

/// The result of activating the selected entry.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Activation {
    /// The browser stayed in the directory view (e.g. entered a subdirectory).
    Stay,
    /// The user selected a file; the host should open it at this path.
    Open(PathBuf),
}

/// The pure directory-browser core: current directory, its listing, and the
/// selected row. Contains no stdio so it can be unit-tested directly.
struct Browser {
    cwd: PathBuf,
    entries: Vec<Entry>,
    selected: usize,
}

impl Browser {
    /// Create a browser rooted at `dir` and list it immediately.
    fn new(dir: PathBuf) -> Browser {
        let mut browser = Browser {
            cwd: dir,
            entries: Vec::new(),
            selected: 0,
        };
        browser.relist();
        browser
    }

    /// Re-read [`Self::cwd`] and rebuild [`Self::entries`].
    ///
    /// Directories sort before files; within each group entries sort
    /// case-insensitively. A `..` entry (a directory) is prepended unless the
    /// cwd has no parent. Hidden files (leading `.`) are shown. On a read error
    /// the listing is just `..` (so the user can still navigate upward).
    fn relist(&mut self) {
        let mut dirs: Vec<Entry> = Vec::new();
        let mut files: Vec<Entry> = Vec::new();

        if let Ok(read_dir) = std::fs::read_dir(&self.cwd) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    dirs.push(Entry { name, is_dir: true });
                } else {
                    files.push(Entry {
                        name,
                        is_dir: false,
                    });
                }
            }
        }

        let by_name = |a: &Entry, b: &Entry| a.name.to_lowercase().cmp(&b.name.to_lowercase());
        dirs.sort_by(by_name);
        files.sort_by(by_name);

        let mut entries = Vec::with_capacity(dirs.len() + files.len() + 1);
        if self.cwd.parent().is_some() {
            entries.push(Entry {
                name: "..".to_string(),
                is_dir: true,
            });
        }
        entries.extend(dirs);
        entries.extend(files);

        self.entries = entries;
    }

    /// One text line per entry. Directories render with a trailing `/`. The
    /// selected entry is prefixed with `"> "` and the rest with `"  "`, so the
    /// selection is legible even in plain text.
    fn lines(&self) -> Vec<String> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let marker = if i == self.selected { "> " } else { "  " };
                let suffix = if entry.is_dir { "/" } else { "" };
                format!("{marker}{}{suffix}", entry.name)
            })
            .collect()
    }

    /// The 0-based selected row, or `None` when the listing is empty.
    fn cursor_line(&self) -> Option<usize> {
        if self.entries.is_empty() {
            None
        } else {
            Some(self.selected)
        }
    }

    /// The cwd display path used as the pane title.
    fn title(&self) -> String {
        self.cwd.display().to_string()
    }

    /// Move the selection down one row, clamped to the last entry.
    fn move_down(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    /// Move the selection up one row, clamped to the first entry.
    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move the selection to the first entry.
    fn move_top(&mut self) {
        self.selected = 0;
    }

    /// Move the selection to the last entry.
    fn move_bottom(&mut self) {
        if !self.entries.is_empty() {
            self.selected = self.entries.len() - 1;
        }
    }

    /// Select `row` directly (a mouse click). Returns false — leaving the
    /// selection unchanged — when the row is past the listing.
    fn select(&mut self, row: usize) -> bool {
        if row < self.entries.len() {
            self.selected = row;
            true
        } else {
            false
        }
    }

    /// Act on the selected entry.
    ///
    /// `..` or a directory: change cwd (to the parent, or `cwd/name`), re-list,
    /// reset the selection to the top, and return [`Activation::Stay`]. A file:
    /// return [`Activation::Open`] with `cwd/name` without changing cwd.
    fn activate(&mut self) -> Activation {
        let Some(entry) = self.entries.get(self.selected).cloned() else {
            return Activation::Stay;
        };

        if entry.name == ".." {
            self.parent();
            return Activation::Stay;
        }

        if entry.is_dir {
            self.cwd = self.cwd.join(&entry.name);
            self.selected = 0;
            self.relist();
            Activation::Stay
        } else {
            Activation::Open(self.cwd.join(&entry.name))
        }
    }

    /// Move to the parent directory if one exists, re-list, and reset the
    /// selection to the top.
    fn parent(&mut self) {
        if let Some(parent) = self.cwd.parent() {
            self.cwd = parent.to_path_buf();
            self.selected = 0;
            self.relist();
        }
    }
}

/// The keys this client subscribes to. The host forwards only these while the
/// pane is focused; everything else stays host-global.
const KEYMAP: &[&str] = &[
    "j",
    "k",
    "Up",
    "Down",
    "Enter",
    "l",
    "Right",
    "h",
    "Left",
    "Backspace",
    "-",
    "g",
    "G",
    " ",
];

/// Build a [`RenderParams`] snapshot of the browser's current view.
fn render_params(browser: &Browser) -> RenderParams {
    RenderParams {
        lines: browser.lines(),
        cursor_line: browser.cursor_line(),
        title: Some(browser.title()),
        status: None,
        styles: None,
        backgrounds: None,
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("directory-browser: {err}");
        std::process::exit(1);
    }
}

/// The stdio I/O loop: handshake, then dispatch host notifications until
/// shutdown or EOF.
fn run() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    // 1. Read the initialize request and build the browser from its first arg.
    let init = match gpp::read_message(&mut reader)? {
        Some(env) if env.is_method(method::INITIALIZE) => env,
        // EOF or an unexpected first message: nothing to do.
        _ => return Ok(()),
    };
    let id = init.id.unwrap_or(1);
    let params: InitializeParams = init
        .params_as()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let dir = params
        .args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&params.cwd));
    let mut browser = Browser::new(canonicalize_or_keep(&dir));

    // Reply with the initialize response (name + subscribed keymap).
    // A light takeover: the host keeps its default pane behavior (scrolling the
    // listing, the command bar) and forwards just our navigation keys — plus
    // mouse clicks (click selects a row, double-click activates it).
    let result = InitializeResult {
        mode: gpp::ClientMode::Lines,
        name: "directory-browser".to_string(),
        takeover: Takeover::Keymap,
        keymap: KEYMAP.iter().map(|k| k.to_string()).collect(),
        mouse: true,
    };
    gpp::write_message(&mut writer, &Envelope::response(id, result))?;

    // 2. Push the initial content immediately.
    render(&mut writer, &browser)?;

    // 3. Dispatch host notifications.
    while let Some(env) = gpp::read_message(&mut reader)? {
        if env.is_method(method::KEY) {
            let key: KeyParams = match env.params_as() {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("directory-browser: bad key params: {e}");
                    continue;
                }
            };
            let outcome = handle_key(&mut browser, &key);
            apply_outcome(&mut writer, &browser, outcome)?;
        } else if env.is_method(method::MOUSE) {
            let mouse: MouseParams = match env.params_as() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("directory-browser: bad mouse params: {e}");
                    continue;
                }
            };
            let outcome = handle_mouse(&mut browser, &mouse);
            apply_outcome(&mut writer, &browser, outcome)?;
        } else if env.is_method(method::RESIZE) {
            // Rendering is size-independent, but re-render so the host always has
            // fresh content after a resize.
            render(&mut writer, &browser)?;
        } else if env.is_method(method::SHUTDOWN) {
            return Ok(());
        }
        // Unknown notifications are ignored.
    }

    // stdin EOF: exit cleanly.
    Ok(())
}

/// What the loop should do after a key press or mouse click.
enum KeyOutcome {
    /// State changed (or a refresh is wanted); re-render.
    Render,
    /// Ask the host to open this path.
    Open(PathBuf),
    /// The input maps to nothing here; do nothing.
    Ignore,
}

/// Carry out a [`KeyOutcome`]: re-render, send `openPath` (the host then shuts
/// us down; the loop keeps running until it does), or nothing.
fn apply_outcome<W: std::io::Write>(
    writer: &mut W,
    browser: &Browser,
    outcome: KeyOutcome,
) -> std::io::Result<()> {
    match outcome {
        KeyOutcome::Render => render(writer, browser),
        KeyOutcome::Open(path) => {
            let params = OpenPathParams {
                path: path.display().to_string(),
            };
            gpp::write_message(writer, &Envelope::notification(method::OPEN_PATH, params))
        }
        KeyOutcome::Ignore => Ok(()),
    }
}

/// Apply a key press to the browser and report what the loop should do next.
fn handle_key(browser: &mut Browser, key: &KeyParams) -> KeyOutcome {
    let Some(parsed) = Key::from_name(&key.key) else {
        return KeyOutcome::Ignore;
    };

    match parsed {
        Key::Down | Key::Char('j') | Key::Char(' ') => {
            browser.move_down();
            KeyOutcome::Render
        }
        Key::Up | Key::Char('k') => {
            browser.move_up();
            KeyOutcome::Render
        }
        Key::Char('g') => {
            browser.move_top();
            KeyOutcome::Render
        }
        Key::Char('G') => {
            browser.move_bottom();
            KeyOutcome::Render
        }
        Key::Enter | Key::Char('l') | Key::Right => match browser.activate() {
            Activation::Stay => KeyOutcome::Render,
            Activation::Open(path) => KeyOutcome::Open(path),
        },
        Key::Char('h') | Key::Left | Key::Backspace | Key::Char('-') => {
            browser.parent();
            KeyOutcome::Render
        }
        _ => KeyOutcome::Ignore,
    }
}

/// Apply a mouse click to the browser: a click on a row selects it; a
/// double-click selects and activates it (descend into a directory, or ask the
/// host to open a file — the same as Enter). A click past the listing is
/// ignored.
fn handle_mouse(browser: &mut Browser, mouse: &MouseParams) -> KeyOutcome {
    if !browser.select(mouse.line) {
        return KeyOutcome::Ignore;
    }
    match mouse.kind {
        MouseKind::Click => KeyOutcome::Render,
        MouseKind::Double => match browser.activate() {
            Activation::Stay => KeyOutcome::Render,
            Activation::Open(path) => KeyOutcome::Open(path),
        },
    }
}

/// Send a `render` notification carrying the browser's current view.
fn render<W: std::io::Write>(writer: &mut W, browser: &Browser) -> std::io::Result<()> {
    gpp::write_message(
        writer,
        &Envelope::notification(method::RENDER, render_params(browser)),
    )
}

/// Canonicalize a path for a clean title; if that fails (e.g. it doesn't exist
/// yet), keep the path as given.
fn canonicalize_or_keep(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A self-cleaning temporary directory tree for tests.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        /// Create a fresh, uniquely-named directory under the system temp dir.
        fn new() -> TempTree {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique = format!(
                "directory-browser-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let root = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&root).unwrap();
            // Canonicalize so paths match what Browser stores (macOS /tmp is a
            // symlink to /private/tmp).
            let root = std::fs::canonicalize(&root).unwrap();
            TempTree { root }
        }

        fn dir(&self, name: &str) -> &Self {
            std::fs::create_dir_all(self.root.join(name)).unwrap();
            self
        }

        fn file(&self, name: &str) -> &Self {
            std::fs::write(self.root.join(name), b"x").unwrap();
            self
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Strip the `"> "`/`"  "` selection marker from a rendered line.
    fn names(browser: &Browser) -> Vec<String> {
        browser.lines().iter().map(|l| l[2..].to_string()).collect()
    }

    #[test]
    fn listing_sorts_dirs_before_files_then_alphabetical() {
        let tree = TempTree::new();
        tree.file("zebra.txt")
            .file("Apple.txt")
            .dir("src")
            .dir("Assets")
            .file("banana.txt");

        let browser = Browser::new(tree.root.clone());
        // ".." first (temp dir has a parent), then dirs case-insensitively,
        // then files case-insensitively.
        assert_eq!(
            names(&browser),
            vec![
                "../",
                "Assets/",
                "src/",
                "Apple.txt",
                "banana.txt",
                "zebra.txt",
            ]
        );
    }

    #[test]
    fn parent_entry_present_when_parent_exists() {
        let tree = TempTree::new();
        let browser = Browser::new(tree.root.clone());
        assert_eq!(browser.entries.first().map(|e| e.name.as_str()), Some(".."));
        assert!(browser.entries[0].is_dir);
    }

    #[test]
    fn parent_entry_absent_at_filesystem_root() {
        let browser = Browser::new(PathBuf::from("/"));
        // The root has no parent, so no ".." entry.
        assert!(browser.entries.iter().all(|e| e.name != ".."));
    }

    #[test]
    fn move_down_and_up_clamp() {
        let tree = TempTree::new();
        tree.file("a").file("b");
        let mut browser = Browser::new(tree.root.clone());
        // Entries: "..", "a", "b" => 3 rows, indices 0..=2.
        assert_eq!(browser.selected, 0);
        browser.move_up();
        assert_eq!(browser.selected, 0, "up clamps at top");

        browser.move_down();
        browser.move_down();
        browser.move_down();
        browser.move_down();
        assert_eq!(browser.selected, 2, "down clamps at bottom");
    }

    #[test]
    fn move_top_and_bottom() {
        let tree = TempTree::new();
        tree.file("a").file("b").file("c");
        let mut browser = Browser::new(tree.root.clone());
        browser.move_bottom();
        assert_eq!(browser.selected, browser.entries.len() - 1);
        browser.move_top();
        assert_eq!(browser.selected, 0);
    }

    #[test]
    fn activate_subdir_changes_cwd_and_stays() {
        let tree = TempTree::new();
        tree.dir("child");
        tree.file("child/inner.txt");
        let mut browser = Browser::new(tree.root.clone());

        // Select "child" (after the leading "..").
        browser.move_down();
        assert_eq!(browser.entries[browser.selected].name, "child");

        let activation = browser.activate();
        assert_eq!(activation, Activation::Stay);
        assert_eq!(browser.cwd, tree.root.join("child"));
        assert_eq!(browser.selected, 0);
        // The new listing shows the child's contents.
        assert!(names(&browser).contains(&"inner.txt".to_string()));
    }

    #[test]
    fn activate_file_returns_open_without_changing_cwd() {
        let tree = TempTree::new();
        tree.file("notes.txt");
        let mut browser = Browser::new(tree.root.clone());
        browser.move_bottom(); // select the file (last row)
        assert_eq!(browser.entries[browser.selected].name, "notes.txt");

        let activation = browser.activate();
        assert_eq!(activation, Activation::Open(tree.root.join("notes.txt")));
        assert_eq!(browser.cwd, tree.root, "cwd unchanged when opening a file");
    }

    #[test]
    fn activate_dotdot_goes_to_parent() {
        let tree = TempTree::new();
        tree.dir("child");
        let mut browser = Browser::new(tree.root.join("child"));
        // Selected row 0 is "..".
        assert_eq!(browser.entries[0].name, "..");
        let activation = browser.activate();
        assert_eq!(activation, Activation::Stay);
        assert_eq!(browser.cwd, tree.root);
    }

    #[test]
    fn lines_prefix_selected_row() {
        let tree = TempTree::new();
        tree.file("a").file("b");
        let mut browser = Browser::new(tree.root.clone());
        browser.move_down(); // select index 1

        let lines = browser.lines();
        assert!(lines[1].starts_with("> "), "selected row marked: {lines:?}");
        assert!(
            lines[0].starts_with("  "),
            "unselected row marked: {lines:?}"
        );
        assert_eq!(browser.cursor_line(), Some(1));
    }

    #[test]
    fn hidden_files_are_shown() {
        let tree = TempTree::new();
        tree.file(".hidden").file("visible");
        let browser = Browser::new(tree.root.clone());
        assert!(names(&browser).contains(&".hidden".to_string()));
    }

    // ---- mouse clicks -------------------------------------------------------

    fn mouse(line: usize, kind: MouseKind) -> MouseParams {
        MouseParams { line, col: 0, kind }
    }

    #[test]
    fn click_selects_the_clicked_row() {
        let tree = TempTree::new();
        tree.file("a").file("b");
        let mut browser = Browser::new(tree.root.clone());
        // Rows: 0 "..", 1 "a", 2 "b".
        let outcome = handle_mouse(&mut browser, &mouse(2, MouseKind::Click));
        assert!(matches!(outcome, KeyOutcome::Render));
        assert_eq!(browser.selected, 2);
        assert_eq!(browser.cursor_line(), Some(2));
    }

    #[test]
    fn click_past_the_listing_is_ignored() {
        let tree = TempTree::new();
        tree.file("a");
        let mut browser = Browser::new(tree.root.clone());
        browser.move_down(); // selection on row 1
        let outcome = handle_mouse(&mut browser, &mouse(99, MouseKind::Click));
        assert!(matches!(outcome, KeyOutcome::Ignore));
        assert_eq!(browser.selected, 1, "selection unchanged");
    }

    #[test]
    fn double_click_on_a_directory_descends() {
        let tree = TempTree::new();
        tree.dir("child");
        tree.file("child/inner.txt");
        let mut browser = Browser::new(tree.root.clone());
        // Row 1 is "child/" (after "..").
        let outcome = handle_mouse(&mut browser, &mouse(1, MouseKind::Double));
        assert!(matches!(outcome, KeyOutcome::Render));
        assert_eq!(browser.cwd, tree.root.join("child"));
        assert!(names(&browser).contains(&"inner.txt".to_string()));
    }

    #[test]
    fn double_click_on_a_file_opens_it() {
        let tree = TempTree::new();
        tree.file("notes.txt");
        let mut browser = Browser::new(tree.root.clone());
        // Row 1 is "notes.txt" (after "..").
        let outcome = handle_mouse(&mut browser, &mouse(1, MouseKind::Double));
        match outcome {
            KeyOutcome::Open(path) => assert_eq!(path, tree.root.join("notes.txt")),
            _ => panic!("expected Open"),
        }
        assert_eq!(browser.cwd, tree.root, "cwd unchanged when opening a file");
    }

    #[test]
    fn unreadable_directory_lists_only_dotdot() {
        let tree = TempTree::new();
        let missing = tree.root.join("does-not-exist");
        let browser = Browser::new(missing);
        // The path has a parent, so ".." is present and is the only entry
        // (rendered with the trailing "/" all directories get).
        assert_eq!(names(&browser), vec!["../"]);
    }
}
