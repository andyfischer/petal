//! The recents reader behind Garden's start screen.
//!
//! **The schema read here is owned by `garden-app/src/state.rs` (the `MIGRATIONS`
//! list); this crate only ever reads it, on a READ-ONLY connection.** Nothing
//! links the two — a GPP app depends on no Garden crate — so a column renamed on
//! the writing side would silently empty a list here rather than fail to
//! compile. The end-to-end integration test (Garden records an open, this app
//! shows it) is what catches that drift; these unit tests only pin the shapes.
//!
//! Every failure mode degrades to an **empty list**, never an error: a
//! first-ever launch has no `~/.garden/state/db.sqlite` at all and must still
//! paint the menu with its Open buttons. A missing file, a database written by
//! an older Garden without these tables, and a corrupt one are all the same
//! answer — "nothing recent yet".

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

/// How many rows each section shows. The start screen is a glance, not a
/// history browser, and a fixed cap keeps the three sections on one screen.
pub const RECENT_LIMIT: usize = 8;

/// The shared state database Garden writes: `$HOME/.garden/state/db.sqlite`.
/// The same location `garden-app` resolves; with no `$HOME` the path is
/// relative and simply won't exist, which reads as "no recents".
pub fn state_db_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".garden/state/db.sqlite")
}

/// The whole `query("recents", "")` answer: `{ projects, files, prs }`, each
/// most-recent-first and capped at [`RECENT_LIMIT`].
///
/// The drawer does no path surgery, so the display fields (`name`, `dir`,
/// `project_name`) are derived here.
pub fn recents(db: &Path) -> Value {
    let conn = open_read_only(db);
    let (projects, files, prs) = match conn {
        Some(conn) => (projects(&conn), files(&conn), prs(&conn)),
        None => (Vec::new(), Vec::new(), Vec::new()),
    };
    json!({ "projects": projects, "files": files, "prs": prs })
}

/// Open `db` read-only, or `None` when it is missing or unreadable. Read-only
/// is load-bearing: the writing Garden process owns this file, and opening it
/// without `SQLITE_OPEN_CREATE` also means a missing database stays missing
/// rather than being conjured empty under the user's `~/.garden`.
fn open_read_only(db: &Path) -> Option<Connection> {
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// Final component of `path` (its whole string when it has none, e.g. `/`).
fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Parent directory of `path`, or `""` when it has none — the drawer prints
/// this dim beside a file's basename.
fn parent_dir(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn projects(conn: &Connection) -> Vec<Value> {
    rows(
        conn,
        "SELECT path, name, last_opened_ms, open_count FROM recent_projects
         ORDER BY last_opened_ms DESC LIMIT ?1",
        |row| {
            let path: String = row.get(0)?;
            // The stored `name` is the writer's copy of the basename; fall back
            // to deriving it so a null/empty column still shows something.
            let name: String = row.get(1).unwrap_or_default();
            let name = if name.is_empty() {
                base_name(&path)
            } else {
                name
            };
            Ok(json!({
                "path": path,
                "name": name,
                "last_opened_ms": row.get::<_, i64>(2)?,
                "open_count": row.get::<_, i64>(3)?,
            }))
        },
    )
}

fn files(conn: &Connection) -> Vec<Value> {
    rows(
        conn,
        "SELECT path, project_path, last_opened_ms, open_count FROM recent_files
         ORDER BY last_opened_ms DESC LIMIT ?1",
        |row| {
            let path: String = row.get(0)?;
            // `project_path` is NULL for a file outside any repo — an absent
            // project reads as `""`, so the drawer never handles a nil.
            let project: Option<String> = row.get(1)?;
            Ok(json!({
                "path": path,
                "name": base_name(&path),
                "dir": parent_dir(&path),
                "project_name": project.as_deref().map(base_name).unwrap_or_default(),
                "last_opened_ms": row.get::<_, i64>(2)?,
                "open_count": row.get::<_, i64>(3)?,
            }))
        },
    )
}

fn prs(conn: &Connection) -> Vec<Value> {
    rows(
        conn,
        "SELECT repo, number, title, last_opened_ms FROM recent_prs
         ORDER BY last_opened_ms DESC LIMIT ?1",
        |row| {
            Ok(json!({
                "repo": row.get::<_, String>(0)?,
                "number": row.get::<_, i64>(1)?,
                "title": row.get::<_, String>(2).unwrap_or_default(),
                "last_opened_ms": row.get::<_, i64>(3)?,
            }))
        },
    )
}

/// Run one `LIMIT ?1` query, mapping every row with `shape`. Any SQL failure —
/// most plausibly a table this database is too old to have — yields an empty
/// list, so one missing section can never take the screen down with it.
fn rows(
    conn: &Connection,
    sql: &str,
    shape: impl Fn(&rusqlite::Row) -> rusqlite::Result<Value>,
) -> Vec<Value> {
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => {
            eprintln!("main-menu: {err}");
            return Vec::new();
        }
    };
    let rows = stmt
        .query_map([RECENT_LIMIT as i64], |row| shape(row))
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>());
    match rows {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!("main-menu: {err}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture database with the same shape `garden-app`'s migrations create.
    /// Written here rather than imported because a GPP app links no Garden
    /// crate — see the module docs on schema drift.
    fn fixture(dir: &Path) -> PathBuf {
        let path = dir.join("db.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE recent_projects (
                 path TEXT PRIMARY KEY, name TEXT NOT NULL,
                 last_opened_ms INTEGER NOT NULL, open_count INTEGER NOT NULL);
             CREATE TABLE recent_files (
                 path TEXT PRIMARY KEY, project_path TEXT,
                 last_opened_ms INTEGER NOT NULL, open_count INTEGER NOT NULL);
             CREATE TABLE recent_prs (
                 repo TEXT NOT NULL, number INTEGER NOT NULL, title TEXT NOT NULL,
                 project_path TEXT, last_opened_ms INTEGER NOT NULL,
                 PRIMARY KEY (repo, number));",
        )
        .unwrap();
        path
    }

    fn add_project(path: &Path, dir: &str, at_ms: i64) {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "INSERT INTO recent_projects VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![dir, base_name(dir), at_ms],
        )
        .unwrap();
    }

    fn add_file(path: &Path, file: &str, project: Option<&str>, at_ms: i64) {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "INSERT INTO recent_files VALUES (?1, ?2, ?3, 2)",
            rusqlite::params![file, project, at_ms],
        )
        .unwrap();
    }

    fn add_pr(path: &Path, repo: &str, number: i64, title: &str, at_ms: i64) {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "INSERT INTO recent_prs VALUES (?1, ?2, ?3, NULL, ?4)",
            rusqlite::params![repo, number, title, at_ms],
        )
        .unwrap();
    }

    #[test]
    fn lists_are_most_recent_first() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());
        add_project(&db, "/work/older", 1000);
        add_project(&db, "/work/newer", 2000);
        add_file(&db, "/work/older/a.rs", Some("/work/older"), 1000);
        add_file(&db, "/work/newer/b.rs", Some("/work/newer"), 2000);
        add_pr(&db, "owner/name", 1, "First", 1000);
        add_pr(&db, "owner/name", 2, "Second", 2000);

        let v = recents(&db);
        assert_eq!(v["projects"][0]["name"], "newer");
        assert_eq!(v["projects"][1]["name"], "older");
        assert_eq!(v["files"][0]["name"], "b.rs");
        assert_eq!(v["prs"][0]["number"], 2);
    }

    #[test]
    fn each_section_stops_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());
        for i in 0..(RECENT_LIMIT as i64 + 5) {
            add_file(&db, &format!("/work/f{i}.rs"), None, 1000 + i);
            add_pr(&db, "owner/name", i, "t", 1000 + i);
            add_project(&db, &format!("/work/p{i}"), 1000 + i);
        }
        let v = recents(&db);
        for section in ["projects", "files", "prs"] {
            assert_eq!(v[section].as_array().unwrap().len(), RECENT_LIMIT);
        }
    }

    /// The drawer prints these straight, so the split happens here.
    #[test]
    fn display_fields_are_derived_from_the_paths() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());
        add_file(&db, "/work/garden/src/main.rs", Some("/work/garden"), 1000);
        add_file(&db, "/tmp/loose.txt", None, 900);

        let v = recents(&db);
        assert_eq!(v["files"][0]["name"], "main.rs");
        assert_eq!(v["files"][0]["dir"], "/work/garden/src");
        assert_eq!(v["files"][0]["project_name"], "garden");
        // A file outside any repo has no project — an empty string, not a nil.
        assert_eq!(v["files"][1]["project_name"], "");
    }

    /// The first-ever launch: nothing has written the database yet.
    #[test]
    fn a_missing_database_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let v = recents(&dir.path().join("nope.sqlite"));
        for section in ["projects", "files", "prs"] {
            assert!(v[section].as_array().unwrap().is_empty());
        }
    }

    /// A database from a Garden older than the recents migration: it opens, but
    /// no query finds its table. Still a menu, not an error pane.
    #[test]
    fn a_database_without_the_tables_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (x INTEGER)")
            .unwrap();
        let v = recents(&db);
        for section in ["projects", "files", "prs"] {
            assert!(v[section].as_array().unwrap().is_empty());
        }
    }
}
