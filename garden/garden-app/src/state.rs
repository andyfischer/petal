//! Per-machine persistent state under `~/.garden/state`.
//!
//! A single SQLite database (`db.sqlite`) tracks machine-unique window ids;
//! each window persists its runtime layout overlay alongside it, under
//! `window-<id>/window.ptl`. The schema is versioned with an ordered,
//! append-only [`MIGRATIONS`] list applied on open via SQLite's `user_version`
//! pragma — so future tables slot in without touching data already migrated.
//!
//! [`State`] is the whole public surface: open it once at startup
//! ([`State::open`]), allocate a window id ([`State::new_window_id`]), and ask
//! for that window's overlay path ([`State::window_overlay_path`]).

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// Ordered, append-only schema migrations. Each entry is one migration step,
/// applied (inside a transaction) the first time the database's recorded
/// `user_version` is below its 1-based index, which then becomes the new
/// version.
///
/// **Never edit, reorder, or remove an existing entry** — that desyncs
/// databases already migrated past it. To evolve the schema, append a new
/// entry: a fresh database runs them all in order, an existing one runs only
/// the new tail.
const MIGRATIONS: &[&str] = &[
    // v1 — the window registry. The AUTOINCREMENT id is the per-machine window
    // id: monotonic and never reused (it survives row deletion). Later
    // migrations can add columns here or whole new tables.
    "CREATE TABLE windows (
         id         INTEGER PRIMARY KEY AUTOINCREMENT,
         created_at TEXT NOT NULL DEFAULT (datetime('now'))
     );",
    // v2 — the per-session event log and the bug/feature reports filed from it
    // (see `event_log.rs`). Every window appends the actions and events it
    // processes to `events` (buffered in memory, flushed on a timer); the
    // `:report` command snapshots the recent tail into `reports` as context.
    // `at_ms` is the event's wall-clock time in milliseconds since the Unix
    // epoch, indexed so a report's "last five minutes" lookup stays cheap.
    "CREATE TABLE events (
         id        INTEGER PRIMARY KEY AUTOINCREMENT,
         window_id INTEGER NOT NULL,
         at_ms     INTEGER NOT NULL,
         category  TEXT NOT NULL,
         detail    TEXT NOT NULL
     );
     CREATE INDEX events_at_ms ON events (at_ms);
     CREATE TABLE reports (
         id        INTEGER PRIMARY KEY AUTOINCREMENT,
         window_id INTEGER NOT NULL,
         at_ms     INTEGER NOT NULL,
         message   TEXT NOT NULL,
         context   TEXT NOT NULL
     );",
];

/// Handle to the `~/.garden/state` directory and its SQLite database.
pub struct State {
    conn: Connection,
    /// The state directory itself, so window paths resolve relative to it.
    dir: PathBuf,
}

impl State {
    /// Open the state database under `dir` (creating the directory and the
    /// database file if absent) and bring its schema up to date by running any
    /// pending [`MIGRATIONS`].
    pub fn open(dir: &Path) -> Result<State, String> {
        fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let mut conn =
            Connection::open(dir.join("db.sqlite")).map_err(|e| format!("open state db: {e}"))?;
        // Two windows = two connections writing to one db.sqlite; WAL + a
        // busy_timeout keep concurrent EventLog flushes from failing with
        // SQLITE_BUSY (roadmap §5.3).
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("enable WAL on state db: {e}"))?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .map_err(|e| format!("set state db busy_timeout: {e}"))?;
        migrate(&mut conn, MIGRATIONS)?;
        Ok(State {
            conn,
            dir: dir.to_path_buf(),
        })
    }

    /// Allocate and return a fresh window id — unique across this machine and
    /// never reused, so two windows (even across restarts) never collide.
    pub fn new_window_id(&self) -> Result<i64, String> {
        self.conn
            .execute("INSERT INTO windows DEFAULT VALUES", [])
            .map_err(|e| format!("allocate window id: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The directory holding window `id`'s state: `<state>/window-<id>`.
    pub fn window_dir(&self, id: i64) -> PathBuf {
        self.dir.join(format!("window-{id}"))
    }

    /// The layout-overlay file for window `id`:
    /// `<state>/window-<id>/window.ptl`. The directory is created lazily by the
    /// first save (see [`garden_script::ScriptHost::save_layout`]).
    pub fn window_overlay_path(&self, id: i64) -> PathBuf {
        self.window_dir(id).join("window.ptl")
    }

    /// Consume this handle, keeping its open database connection alive as the
    /// per-session [`EventLog`](crate::event_log::EventLog) for `window_id`.
    /// The state DB is opened (and migrated) once at startup to allocate the
    /// window id; rather than reopen it, the same connection then backs the
    /// window's event log for the rest of its life.
    pub fn into_event_log(self, window_id: i64) -> crate::event_log::EventLog {
        crate::event_log::EventLog::new(self.conn, window_id)
    }
}

/// Apply every migration in `migrations` whose 1-based index exceeds the
/// database's recorded `user_version`, each in its own transaction, bumping the
/// version after each. Idempotent: a database already at or past the last
/// migration is left untouched.
fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<(), String> {
    let current: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| format!("read schema version: {e}"))?;
    for (i, sql) in migrations.iter().enumerate() {
        let version = (i + 1) as i64;
        if version > current {
            let tx = conn
                .transaction()
                .map_err(|e| format!("begin migration {version}: {e}"))?;
            tx.execute_batch(sql)
                .map_err(|e| format!("apply migration {version}: {e}"))?;
            // The pragma write is part of the transaction, so a failed
            // migration rolls back the version bump too.
            tx.pragma_update(None, "user_version", version)
                .map_err(|e| format!("record schema version {version}: {e}"))?;
            tx.commit()
                .map_err(|e| format!("commit migration {version}: {e}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_version(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn open_creates_db_and_migrates_to_latest() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::open(dir.path()).unwrap();
        assert!(dir.path().join("db.sqlite").exists());
        assert_eq!(schema_version(&state.conn), MIGRATIONS.len() as i64);
    }

    /// Multiple windows in one process each open their own connection to the
    /// shared `db.sqlite`, so `State::open` must switch the database to WAL
    /// (persistent, recorded in the file) and give each connection a nonzero
    /// `busy_timeout` — the default rollback journal + 0ms timeout makes
    /// concurrent writers fail immediately with SQLITE_BUSY.
    #[test]
    fn open_enables_wal_and_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::open(dir.path()).unwrap();

        // journal_mode is a property of the database file; assert it through
        // an independent probe connection so the test doesn't depend on how
        // `State` configured its own handle.
        let probe = Connection::open(dir.path().join("db.sqlite")).unwrap();
        let mode: String = probe
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");

        // busy_timeout is per-connection, so it must be read off the State's
        // own connection (visible here because tests live in-module).
        let timeout_ms: i64 = state
            .conn
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert!(
            timeout_ms > 0,
            "State's connection should have a nonzero busy_timeout, got {timeout_ms}"
        );
    }

    /// Two `State`s (two connections, as two windows in one process would
    /// hold) on the same directory can both allocate window ids. Regression
    /// guard for the multi-window world: interleaved writes on separate
    /// connections must succeed and never hand out the same id.
    #[test]
    fn two_connections_write_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let a = State::open(dir.path()).unwrap();
        let b = State::open(dir.path()).unwrap();

        let ia = a.new_window_id().unwrap();
        let ib = b.new_window_id().unwrap();
        assert_ne!(ia, ib);

        // Interleave once more in each direction while both stay open.
        let ia2 = a.new_window_id().unwrap();
        let ib2 = b.new_window_id().unwrap();
        let mut ids = vec![ia, ib, ia2, ib2];
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4, "window ids must never collide");
    }

    #[test]
    fn window_ids_are_unique_and_monotonic_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = {
            let s = State::open(dir.path()).unwrap();
            (s.new_window_id().unwrap(), s.new_window_id().unwrap())
        };
        assert!(b > a);
        // Reopening the existing DB keeps counting up — ids never restart.
        let s = State::open(dir.path()).unwrap();
        let c = s.new_window_id().unwrap();
        assert!(c > b);
    }

    #[test]
    fn overlay_path_is_per_window() {
        let dir = tempfile::tempdir().unwrap();
        let s = State::open(dir.path()).unwrap();
        assert_eq!(
            s.window_overlay_path(7),
            dir.path().join("window-7").join("window.ptl")
        );
    }

    /// Migrations apply incrementally: a DB created at version 1 picks up only
    /// the newly-appended step on the next open, and re-running is a no-op.
    #[test]
    fn migrations_apply_incrementally_and_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");

        let v1: &[&str] = &["CREATE TABLE windows (id INTEGER PRIMARY KEY AUTOINCREMENT);"];
        let v2: &[&str] = &[
            v1[0],
            "CREATE TABLE sessions (id INTEGER PRIMARY KEY, window_id INTEGER);",
        ];

        {
            let mut conn = Connection::open(&path).unwrap();
            migrate(&mut conn, v1).unwrap();
            assert_eq!(schema_version(&conn), 1);
        }
        {
            let mut conn = Connection::open(&path).unwrap();
            migrate(&mut conn, v2).unwrap();
            assert_eq!(schema_version(&conn), 2);
            // The new table exists and the old one was not recreated/cleared.
            let n: i64 = conn
                .query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('windows','sessions')", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 2);
        }
        {
            // Re-running the same set changes nothing.
            let mut conn = Connection::open(&path).unwrap();
            migrate(&mut conn, v2).unwrap();
            assert_eq!(schema_version(&conn), 2);
        }
    }
}
