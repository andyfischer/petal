//! Tests for the sqlite-browser query shapers: grid formatting and the
//! `catalog`/`table` JSON answers, exercised end-to-end against a real
//! on-disk SQLite file (the `db` module has its own in-memory unit tests).

use super::*;
use rusqlite::Connection;
use std::path::PathBuf;

/// Open a SQLite file as a boxed backend for the shapers under test.
fn open(path: &PathBuf) -> (db::Source, Box<dyn db::Backend>) {
    let source = db::Source::Sqlite(path.clone());
    let backend = db::open(&source).expect("open db");
    (source, backend)
}

/// Create a throwaway `.db` seeded with a table + a view, and return its path.
/// Lives under the OS temp dir with a name unique per (pid, call) so tests
/// running in parallel never share a file.
fn temp_db() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("sqlite-browser-test-{}-{n}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, note TEXT);
         INSERT INTO users (name, note) VALUES ('Ada', 'first'), ('Alan', NULL);
         CREATE VIEW named AS SELECT id, name FROM users WHERE name IS NOT NULL;",
    )
    .unwrap();
    path
}

#[test]
fn catalog_value_lists_tables_and_views() {
    let path = temp_db();
    let (source, mut backend) = open(&path);
    let v = catalog_value(&source, backend.as_mut()).expect("catalog");
    let expected_name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(v["name"], expected_name);
    let tables = v["tables"].as_array().unwrap();
    let users = tables.iter().find(|t| t["name"] == "users").unwrap();
    assert_eq!(users["kind"], "table");
    assert_eq!(users["rows"], 2);
    assert_eq!(users["cols"], 3);
    let named = tables.iter().find(|t| t["name"] == "named").unwrap();
    assert_eq!(named["kind"], "view");
    assert_eq!(named["rows"], 2);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_reports_error_for_missing_file() {
    let source = db::Source::Sqlite(PathBuf::from("/nonexistent/definitely/not/here.db"));
    assert!(db::open(&source).is_err());
}

#[test]
fn source_from_arg_classifies_sqlite_vs_postgres() {
    assert!(matches!(
        db::Source::from_arg("/tmp/foo.db"),
        db::Source::Sqlite(_)
    ));
    assert!(matches!(
        db::Source::from_arg("postgres://localhost/shop"),
        db::Source::Postgres(_)
    ));
    assert!(matches!(
        db::Source::from_arg("postgresql://u:p@host:5432/db"),
        db::Source::Postgres(_)
    ));
}

#[test]
fn table_value_returns_schema_and_grid() {
    let path = temp_db();
    let (_source, mut backend) = open(&path);
    let v = table_value(backend.as_mut(), "users").expect("table");
    assert_eq!(v["heading"], "users");
    assert_eq!(v["row_count"], 2);
    assert_eq!(v["truncated"], false);

    let cols = v["columns"].as_array().unwrap();
    assert_eq!(cols.len(), 3);
    assert_eq!(cols[0]["name"], "id");
    assert_eq!(cols[0]["pk"], true);
    assert_eq!(cols[1]["name"], "name");
    assert_eq!(cols[1]["notnull"], true);

    let lines = v["lines"].as_array().unwrap();
    // header + rule + 2 data rows
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["style"], "title");
    assert_eq!(lines[1]["style"], "dim");
    assert_eq!(lines[2]["style"], "");
    let header = lines[0]["text"].as_str().unwrap();
    assert!(header.contains("id"));
    assert!(header.contains("name"));
    assert!(header.contains("note"));
    // A NULL cell renders as the literal NULL in the grid.
    let joined = lines
        .iter()
        .map(|l| l["text"].as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("NULL"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn table_value_error_for_unknown_table() {
    let path = temp_db();
    let (_source, mut backend) = open(&path);
    // `SELECT * FROM <missing>` fails, so the shaper returns Err — surfaced to
    // the drawer as a query error rather than a crash.
    let r = table_value(backend.as_mut(), "does_not_exist");
    assert!(r.is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn fit_pads_and_truncates() {
    assert_eq!(fit("ab", 4), "ab  ");
    assert_eq!(fit("abcd", 4), "abcd");
    assert_eq!(fit("abcdef", 4), "abc…");
    assert_eq!(fit("abc", 0), "");
}

#[test]
fn sanitize_collapses_control_whitespace() {
    assert_eq!(sanitize("a\nb\tc\rd"), "a b c d");
}

#[test]
fn grid_lines_handles_empty_table() {
    // An empty page (columns present, no rows) still yields header + rule + note.
    let page = db::Page {
        columns: vec!["a".to_string(), "b".to_string()],
        rows: vec![],
        truncated: false,
    };
    let lines = grid_lines(&page);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[2]["text"], "(no rows)");
}

/// Live Postgres round-trip through the same shapers the SQLite path uses.
/// Skipped unless `SQLITE_BROWSER_TEST_PG_URL` points at a reachable server —
/// there's no bundled Postgres to spin up the way SQLite is in-process. Set it
/// to a throwaway database and run `cargo test -p sqlite-browser` to exercise
/// the catalog/columns/page introspection against a real engine:
///
///   createdb sqlite_browser_test
///   psql sqlite_browser_test -c "CREATE TABLE t (id serial primary key, s text not null);"
///   psql sqlite_browser_test -c "INSERT INTO t (s) VALUES ('a'), (NULL_OK_ONLY_IF_NULLABLE);"
///   SQLITE_BROWSER_TEST_PG_URL=postgres://localhost/sqlite_browser_test cargo test -p sqlite-browser
#[test]
fn postgres_catalog_and_table_roundtrip() {
    let Ok(url) = std::env::var("SQLITE_BROWSER_TEST_PG_URL") else {
        eprintln!("skipping: set SQLITE_BROWSER_TEST_PG_URL to run the Postgres integration test");
        return;
    };
    let source = db::Source::from_arg(&url);
    assert!(
        matches!(source, db::Source::Postgres(_)),
        "URL should classify as Postgres"
    );
    let mut backend = db::open(&source).expect("connect to Postgres");

    // The catalog must at least round-trip (an empty database is still valid).
    let cat = catalog_value(&source, backend.as_mut()).expect("catalog");
    assert!(cat["tables"].is_array());
    // The redacted URL is what the header shows — never the raw password.
    let path = cat["path"].as_str().unwrap();
    assert!(
        !path.contains("secret"),
        "password must not leak into the header"
    );

    // If the fixture table `t` exists, its schema and grid must shape correctly.
    let tables = cat["tables"].as_array().unwrap();
    if tables.iter().any(|t| t["name"] == "t") {
        let v = table_value(backend.as_mut(), "t").expect("table t");
        let cols = v["columns"].as_array().unwrap();
        assert!(cols.iter().any(|c| c["name"] == "id" && c["pk"] == true));
        let lines = v["lines"].as_array().unwrap();
        assert!(lines.len() >= 2, "at least a header row and a rule");
    }
}
