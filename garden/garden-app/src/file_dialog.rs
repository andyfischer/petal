//! The native file/folder picker, in one place so every caller opens the same
//! dialog: the macOS menu bar's `Open…` / `Open Folder…` items and the
//! host-handled `open_file_dialog` panel mutation (a panel screen's "Open a
//! file…" button).
//!
//! These block the calling thread until the user answers, and they need a
//! windowing system — so the app core must only reach them when a windowed
//! frontend is driving it (see `App::native_dialogs`), never from `--term`,
//! `--headless`, or a test.

use std::path::PathBuf;

/// Ask the user for one existing file. `None` if they cancelled.
pub fn pick_file() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_file()
}

/// Ask the user for one directory. `None` if they cancelled.
pub fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}
