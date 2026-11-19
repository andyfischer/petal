//! An open document's source text: the edits an application makes to it,
//! undo and redo, saving, and noticing when another program changed the file.
//!
//! The source text is the single source of truth. Undo and redo keep text
//! snapshots. An edit is committed with an optional *key*: edits with the
//! same key within [`COALESCE_WINDOW`] are one undo step (typing into one
//! field, nudging with the arrow keys). A continuous gesture (a drag) can
//! instead bracket its edits with [`DocumentSession::begin_gesture`] and
//! [`DocumentSession::end_gesture`]: the source follows every step, and the
//! whole gesture is one undo step however long it takes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How close together two edits with the same key must be to coalesce.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(1200);

/// How many undo steps a session keeps.
pub const UNDO_LIMIT: usize = 500;

/// One open document. See the [module docs](self).
#[derive(Debug, Clone)]
pub struct DocumentSession {
    path: PathBuf,
    source: String,
    /// The text as last read from or written to disk.
    saved: String,
    saved_mtime: Option<SystemTime>,
    undo: Vec<String>,
    redo: Vec<String>,
    /// The key and time of the last edit, so a run of edits to one thing is
    /// one undo step.
    last_edit: Option<(String, Instant)>,
    /// The source when the open gesture began.
    gesture: Option<String>,
    /// The redo steps when the open gesture began: a cancelled gesture
    /// changed nothing, so they come back.
    gesture_redo: Vec<String>,
    /// A message for the user about the last thing that happened to the file
    /// (saved, reloaded, changed on disk under unsaved edits).
    pub notice: Option<String>,
}

/// A file's modification time, when it has one.
pub fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl DocumentSession {
    /// Read the file at `path` (made absolute when it exists).
    pub fn read(path: &Path) -> Result<DocumentSession, String> {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let source = std::fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        Ok(DocumentSession::new(path, source))
    }

    /// `source` as if read from `path`, which need not exist until the
    /// session is saved.
    pub fn new(path: PathBuf, source: String) -> DocumentSession {
        DocumentSession {
            saved: source.clone(),
            saved_mtime: mtime(&path),
            path,
            source,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            gesture: None,
            gesture_redo: Vec::new(),
            notice: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The current source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The text has edits that are not saved.
    pub fn dirty(&self) -> bool {
        self.source != self.saved
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.gesture.as_ref().is_some_and(|g| *g != self.source)
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Make `updated` the source as one edit. `key` coalesces a run of edits
    /// to the same thing into one undo step (see [`COALESCE_WINDOW`]). During
    /// a gesture every commit belongs to the gesture's one step. True when
    /// the source changed.
    pub fn commit(&mut self, updated: String, key: Option<&str>) -> bool {
        if updated == self.source {
            return false;
        }
        if self.gesture.is_none() {
            let now = Instant::now();
            let coalesce = match (&self.last_edit, key) {
                (Some((k, t)), Some(key)) => k == key && now.duration_since(*t) < COALESCE_WINDOW,
                _ => false,
            };
            if !coalesce {
                self.push_undo(self.source.clone());
            }
            self.last_edit = key.map(|k| (k.to_string(), now));
        }
        self.redo.clear();
        self.source = updated;
        true
    }

    /// Start a continuous gesture: every [`commit`](Self::commit) until
    /// [`end_gesture`](Self::end_gesture) is part of one undo step. A gesture
    /// already open is kept.
    pub fn begin_gesture(&mut self) {
        if self.gesture.is_none() {
            self.gesture = Some(self.source.clone());
            self.gesture_redo = self.redo.clone();
        }
    }

    /// Whether a gesture is open.
    pub fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }

    /// End the open gesture. True when it changed the source (and so left one
    /// undo step).
    pub fn end_gesture(&mut self) -> bool {
        let Some(base) = self.gesture.take() else { return false };
        self.gesture_redo.clear();
        self.last_edit = None;
        if base == self.source {
            return false;
        }
        self.push_undo(base);
        true
    }

    /// Abandon the open gesture, putting the source back as it was when the
    /// gesture began. True when that changed the source.
    pub fn cancel_gesture(&mut self) -> bool {
        let Some(base) = self.gesture.take() else { return false };
        let changed = base != self.source;
        self.source = base;
        self.redo = std::mem::take(&mut self.gesture_redo);
        changed
    }

    fn push_undo(&mut self, text: String) {
        self.undo.push(text);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Step back one edit (ending any open gesture first). True when there
    /// was one.
    pub fn undo(&mut self) -> bool {
        self.end_gesture();
        let Some(prev) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.source, prev));
        self.last_edit = None;
        true
    }

    /// Step forward one undone edit. True when there was one.
    pub fn redo(&mut self) -> bool {
        self.end_gesture();
        let Some(next) = self.redo.pop() else { return false };
        let prev = std::mem::replace(&mut self.source, next);
        self.push_undo(prev);
        self.last_edit = None;
        true
    }

    /// Write the source to the session's path.
    pub fn save(&mut self) -> Result<(), String> {
        std::fs::write(&self.path, &self.source).map_err(|e| format!("could not save {}: {e}", self.path.display()))?;
        self.saved = self.source.clone();
        self.saved_mtime = mtime(&self.path);
        self.notice = Some(format!("Saved {}", self.path.display()));
        Ok(())
    }

    /// Pick up an edit made to the file by another program. With no unsaved
    /// edits here, the new text replaces the source as an undoable step and
    /// this returns true (the host should run the document again). With
    /// unsaved edits, they are kept, [`notice`](Self::notice) says the file
    /// changed, and this returns false.
    pub fn poll_disk(&mut self) -> bool {
        let now = mtime(&self.path);
        if now.is_none() || now == self.saved_mtime {
            return false;
        }
        self.saved_mtime = now;
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return false;
        };
        if text == self.saved {
            return false;
        }
        if self.dirty() || self.gesture.is_some() {
            self.notice = Some("The file changed on disk; you have unsaved changes here".into());
            self.saved = text;
            return false;
        }
        let prev = std::mem::replace(&mut self.source, text.clone());
        self.push_undo(prev);
        self.redo.clear();
        self.saved = text;
        self.last_edit = None;
        self.notice = Some("Reloaded the script from disk".into());
        true
    }
}
