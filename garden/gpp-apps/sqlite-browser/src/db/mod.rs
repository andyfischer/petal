//! The database layer, abstracted over the concrete client so the browser can
//! speak to more than one engine. Everything above this module (the JSON
//! shapers in `main.rs`, the `db_view.ptl` drawer) is engine-agnostic: it asks a
//! [`Backend`] for the three shapes it draws — the table/view catalog (with row
//! counts), a table's column schema, and a page of its rows — and never learns
//! whether those came from SQLite or Postgres.
//!
//! - [`Source`] parses the launch argument into a target: a file path is SQLite,
//!   a `postgres://` / `postgresql://` URL is Postgres.
//! - [`open`] connects and returns a boxed [`Backend`] for that target.
//! - The concrete clients live in the [`sqlite`] and [`postgres`] submodules;
//!   each renders its native values into the shared [`CatalogEntry`] / [`Column`]
//!   / [`Page`] types defined here, so the display shape is identical regardless
//!   of engine. Adding a third engine is: a new submodule implementing
//!   [`Backend`], plus a branch in [`Source::from_arg`] and [`open`].

mod postgres;
mod sqlite;

use std::path::PathBuf;

/// The most rows we read for a single table's data grid. A browser, not an
/// exporter — a hard cap keeps a huge table from stalling a frame, and the
/// drawer notes when the view is truncated. Backends fetch one extra row to
/// detect truncation without a second count.
pub const ROW_LIMIT: usize = 1000;

/// One entry in the database catalog — a table or a view.
pub struct CatalogEntry {
    /// The display/lookup name. Unqualified for the default schema; qualified
    /// (`schema.name`) when a backend has more than one and the object lives
    /// outside the default one.
    pub name: String,
    /// `"table"` or `"view"`.
    pub kind: String,
    /// Row count for the object. Exact for SQLite; a planner estimate for
    /// Postgres (an exact `COUNT(*)` per table would scan the whole database on
    /// open) — good enough for the catalog list and the overview bar chart.
    pub rows: i64,
    /// Number of columns.
    pub cols: usize,
}

/// One column of a table/view: its name, declared type, and key/nullability flags.
pub struct Column {
    pub name: String,
    /// The declared type (may be empty in a dynamically-typed SQLite table).
    pub decl_type: String,
    pub notnull: bool,
    /// Part of the primary key.
    pub pk: bool,
}

/// A page of a table's data: the column headers and the string-rendered cells,
/// plus whether more rows exist beyond [`ROW_LIMIT`].
pub struct Page {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}

/// A read-only view of one database, whatever the engine. The three methods map
/// one-to-one onto what the drawer paints. `&mut self` because a Postgres client
/// query needs it; the SQLite implementation is logically immutable.
pub trait Backend {
    /// The catalog of user tables and views (engine internals excluded),
    /// tables before views, name-sorted, each with a row and column count.
    fn catalog(&mut self) -> Result<Vec<CatalogEntry>, String>;

    /// A table/view's columns, in definition order.
    fn columns(&mut self, name: &str) -> Result<Vec<Column>, String>;

    /// A page of rows (up to [`ROW_LIMIT`]) with every cell rendered to a
    /// display string.
    fn page(&mut self, name: &str) -> Result<Page, String>;

    /// Whether the underlying connection is still usable. A network engine can
    /// go away between queries; the caller drops and re-[`open`]s when this is
    /// false. Defaults to always-live for a local file.
    fn is_live(&self) -> bool {
        true
    }
}

/// What to browse, parsed from the launch argument. The variants deliberately
/// hold only the *target* (a path, a URL) — a live connection is a separate,
/// re-establishable thing produced by [`open`].
#[derive(Clone)]
pub enum Source {
    /// A SQLite database file.
    Sqlite(PathBuf),
    /// A Postgres server, addressed by connection URL.
    Postgres(String),
}

impl Source {
    /// Classify the launch argument: a `postgres://` / `postgresql://` URL is a
    /// Postgres server, anything else is a SQLite file path. (The host passes
    /// the first CLI arg, or the pane `cwd` when none — see `repo_arg`.)
    pub fn from_arg(arg: &str) -> Source {
        let lower = arg.trim().to_ascii_lowercase();
        if lower.starts_with("postgres://") || lower.starts_with("postgresql://") {
            Source::Postgres(arg.trim().to_string())
        } else {
            Source::Sqlite(PathBuf::from(arg))
        }
    }

    /// The short name for the pane title and catalog header: a SQLite file's
    /// name, or a Postgres URL's database name (falling back to its host).
    pub fn display_name(&self) -> String {
        match self {
            Source::Sqlite(path) => path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            Source::Postgres(url) => postgres::url_display_name(url),
        }
    }

    /// A one-line description of the source for the catalog subheader: the full
    /// file path, or the Postgres URL with any password redacted.
    pub fn describe(&self) -> String {
        match self {
            Source::Sqlite(path) => path.to_string_lossy().into_owned(),
            Source::Postgres(url) => postgres::redact_url(url),
        }
    }
}

/// Connect to `source` and return a boxed backend. Errors (a bad path, an
/// unreachable server, a failed handshake) come back as a message the drawer
/// shows on its open-error path.
pub fn open(source: &Source) -> Result<Box<dyn Backend>, String> {
    match source {
        Source::Sqlite(path) => Ok(Box::new(sqlite::SqliteBackend::open(path)?)),
        Source::Postgres(url) => Ok(Box::new(postgres::PostgresBackend::connect(url)?)),
    }
}
