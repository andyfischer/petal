//! The SQLite [`Backend`]: open a database **read-only** and pull out the shapes
//! the browser draws. Read-only is the right posture for a browser (never
//! mutates the user's data) and also opens files another process holds open.
//! The connection is re-usable across queries; SQLite readers see other
//! processes' commits on each new statement, so external writes still show up.

use std::path::Path;

use rusqlite::{types::ValueRef, Connection, OpenFlags};

use super::{Backend, CatalogEntry, Column, Page, ROW_LIMIT};

/// A live, read-only SQLite connection.
pub struct SqliteBackend {
    conn: Connection,
}

impl SqliteBackend {
    /// Open the database at `path` read-only.
    pub fn open(path: &Path) -> Result<SqliteBackend, String> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
        Ok(SqliteBackend { conn })
    }
}

impl Backend for SqliteBackend {
    fn catalog(&mut self) -> Result<Vec<CatalogEntry>, String> {
        catalog(&self.conn)
    }

    fn columns(&mut self, name: &str) -> Result<Vec<Column>, String> {
        columns(&self.conn, name)
    }

    fn page(&mut self, name: &str) -> Result<Page, String> {
        page(&self.conn, name)
    }
}

/// The catalog of user tables and views, tables before views then name-sorted,
/// each with a row count and column count. `sqlite_*` internal objects are
/// excluded.
fn catalog(conn: &Connection) -> Result<Vec<CatalogEntry>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT name, type FROM sqlite_master \
             WHERE type IN ('table','view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             ORDER BY type = 'view', name",
        )
        .map_err(|e| format!("reading schema: {e}"))?;
    let entries = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("reading schema: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("reading schema: {e}"))?;

    let mut out = Vec::with_capacity(entries.len());
    for (name, kind) in entries {
        out.push(CatalogEntry {
            rows: count_rows(conn, &name),
            cols: columns(conn, &name).map(|c| c.len()).unwrap_or(0),
            name,
            kind,
        });
    }
    Ok(out)
}

/// `COUNT(*)` for one object; `0` if the count can't be taken (e.g. a view over
/// a missing table) so the catalog still lists it.
fn count_rows(conn: &Connection, name: &str) -> i64 {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {}", quote_ident(name)),
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// A table/view's columns via `PRAGMA table_info`.
fn columns(conn: &Connection, name: &str) -> Result<Vec<Column>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quote_ident(name)))
        .map_err(|e| format!("reading columns: {e}"))?;
    let cols = stmt
        .query_map([], |r| {
            Ok(Column {
                name: r.get::<_, String>(1)?,
                decl_type: r.get::<_, String>(2)?,
                notnull: r.get::<_, i64>(3)? != 0,
                pk: r.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|e| format!("reading columns: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("reading columns: {e}"))?;
    Ok(cols)
}

/// A page of rows (up to [`ROW_LIMIT`] + 1 probed, [`ROW_LIMIT`] returned) with
/// every cell rendered to a display string.
fn page(conn: &Connection, name: &str) -> Result<Page, String> {
    // Fetch one extra row to detect truncation without a second COUNT.
    let sql = format!(
        "SELECT * FROM {} LIMIT {}",
        quote_ident(name),
        ROW_LIMIT + 1
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("preparing query: {e}"))?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let ncols = columns.len();

    let mut rows_iter = stmt.query([]).map_err(|e| format!("querying rows: {e}"))?;
    let mut rows: Vec<Vec<String>> = Vec::new();
    while let Some(row) = rows_iter.next().map_err(|e| format!("reading row: {e}"))? {
        let cells = (0..ncols)
            .map(|i| render_cell(row.get_ref_unwrap(i)))
            .collect();
        rows.push(cells);
    }
    let truncated = rows.len() > ROW_LIMIT;
    rows.truncate(ROW_LIMIT);
    Ok(Page {
        columns,
        rows,
        truncated,
    })
}

/// Render a single cell value to a display string. Blobs show a byte count
/// rather than raw bytes; reals drop a trailing `.0` so integers stored as REAL
/// read cleanly.
fn render_cell(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{f:.0}")
            } else {
                f.to_string()
            }
        }
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(b) => format!("<{} bytes>", b.len()),
    }
}

/// Quote an SQL identifier by wrapping it in double quotes and doubling any
/// embedded quote — safe against a name with spaces/keywords/quotes. (Catalog
/// names come from the DB itself, but this keeps every interpolation sound.)
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An in-memory DB seeded with two tables and a view, for deterministic
    /// tests without touching disk. Wrapped in a [`SqliteBackend`] so tests
    /// exercise the trait surface.
    fn fixture() -> SqliteBackend {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER);
             INSERT INTO users (name, age) VALUES ('Ada', 36), ('Alan', 41), ('Grace', NULL);
             CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT, blob_col BLOB, weight REAL);
             INSERT INTO notes (body, blob_col, weight) VALUES ('hi', x'0102', 1.5), ('yo', NULL, 3.0);
             CREATE VIEW adults AS SELECT * FROM users WHERE age >= 18;",
        )
        .unwrap();
        SqliteBackend { conn }
    }

    #[test]
    fn catalog_lists_tables_and_views_with_counts() {
        let mut db = fixture();
        let cat = db.catalog().unwrap();
        let names: Vec<&str> = cat.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"users"));
        assert!(names.contains(&"notes"));
        assert!(names.contains(&"adults"));
        let users = cat.iter().find(|e| e.name == "users").unwrap();
        assert_eq!(users.kind, "table");
        assert_eq!(users.rows, 3);
        assert_eq!(users.cols, 3);
        let adults = cat.iter().find(|e| e.name == "adults").unwrap();
        assert_eq!(adults.kind, "view");
        assert_eq!(adults.rows, 2); // Ada + Alan; Grace has NULL age
                                    // Tables sort before views.
        let last = cat.last().unwrap();
        assert_eq!(last.kind, "view");
    }

    #[test]
    fn columns_reports_pk_and_notnull() {
        let mut db = fixture();
        let cols = db.columns("users").unwrap();
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0].name, "id");
        assert!(cols[0].pk);
        assert!(cols[1].notnull); // name TEXT NOT NULL
        assert!(!cols[2].notnull); // age INTEGER (nullable)
    }

    #[test]
    fn page_renders_cells_including_null_blob_real() {
        let mut db = fixture();
        let p = db.page("notes").unwrap();
        assert_eq!(p.columns, vec!["id", "body", "blob_col", "weight"]);
        assert_eq!(p.rows.len(), 2);
        assert!(!p.truncated);
        assert_eq!(p.rows[0][1], "hi");
        assert_eq!(p.rows[0][2], "<2 bytes>"); // blob
        assert_eq!(p.rows[0][3], "1.5");
        assert_eq!(p.rows[1][2], "NULL"); // null blob
        assert_eq!(p.rows[1][3], "3"); // 3.0 REAL renders as integer
    }

    #[test]
    fn quote_ident_escapes_quotes() {
        assert_eq!(quote_ident("plain"), "\"plain\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }
}
