//! sqlite-browser — a **panel-mode** GPP app for browsing and visualizing a
//! relational database, built on the [`petal_query::App`] provider API.
//!
//! Despite the name it speaks to more than one engine: the launch argument
//! (`initialize` args, else the pane `cwd`) is classified by [`db::Source`] into
//! a **SQLite** file path or a **Postgres** `postgres://…` URL, and everything
//! above the [`db`] layer is engine-agnostic. The host runs the colocated
//! `db_view.ptl` drawer in-process and drives it by answering the drawer's
//! `query(kind, arg)` requests over the pipe. The drawer paints a **two-column
//! master-detail** view:
//!
//! - **Left** — the database catalog: an *Overview* entry followed by every user
//!   table and view, each showing its row count.
//! - **Right** — for a table/view, its **column schema** (types, PK, NOT NULL)
//!   above a **data grid** of the first [`db::ROW_LIMIT`] rows in a selectable
//!   `text_view`; for the Overview, a **bar chart** visualizing the relative row
//!   counts across the whole database.
//!
//! The database is opened **read-only**. The connection is cached in the
//! per-run [`State`] and reused across queries (a fresh statement still sees
//! other processes' commits), and transparently re-established if it drops.
//! Answers carry a short `max_age` with a stale-while-revalidate window: the
//! catalog and row pages stay live without ever flashing a spinner after the
//! first load.

mod db;

use std::time::Duration;

use petal_query::gpp::{self, PanelUi};
use petal_query::{CachePolicy, Provider, Reply};
use serde_json::{json, Value};

/// The `db_view.ptl` drawer, embedded from this crate. The host compiles and
/// runs it in-process; this app only answers its `query(...)` requests.
const DB_VIEW: &str = include_str!("db_view.ptl");

/// Per-run state: which database to browse, and a lazily-opened, cached
/// connection to it.
struct State {
    source: db::Source,
    backend: Option<Box<dyn db::Backend>>,
}

impl State {
    /// The live backend, opening (or re-opening a dropped) connection on demand.
    /// The connection is cached so a Postgres session isn't re-established on
    /// every stale-while-revalidate refetch; if it has gone away, [`is_live`]
    /// reports false and we reconnect. Open failures surface to the drawer.
    ///
    /// [`is_live`]: db::Backend::is_live
    fn backend(&mut self) -> Result<&mut dyn db::Backend, String> {
        let stale = self.backend.as_ref().map_or(true, |b| !b.is_live());
        if stale {
            self.backend = Some(db::open(&self.source)?);
        }
        Ok(self.backend.as_mut().unwrap().as_mut())
    }
}

/// A short-lived, stale-while-revalidate policy: the answer stays fresh for a
/// couple seconds, then is served *while* a background refetch runs for another
/// half-minute — so a database changing under us updates without a spinner.
fn live_policy() -> CachePolicy {
    CachePolicy::max_age(Duration::from_secs(2)).stale_while_revalidate(Duration::from_secs(30))
}

fn run() -> std::io::Result<()> {
    let provider = Provider::new(|init| State {
        source: db::Source::from_arg(&init.repo_arg()),
        backend: None,
    })
    // The database catalog (Overview + table/view list with row counts).
    .query("catalog", |state: &mut State, _ctx| {
        let source = state.source.clone();
        let value = state.backend().and_then(|b| catalog_value(&source, b));
        Reply::from(value).cache(live_policy())
    })
    // One table/view: its column schema + a page of rows as a formatted grid.
    .query("table", |state: &mut State, ctx| {
        let name = ctx.arg.to_string();
        let value = state.backend().and_then(|b| table_value(b, &name));
        Reply::from(value).cache(live_policy())
    });
    // The pane is named from the opened database — a GPP-layer concern.
    let ui =
        PanelUi::new("sqlite-browser", DB_VIEW).title(|state: &State| state.source.display_name());
    gpp::serve(provider, ui)
}

// ---------------------------------------------------------------------------
// Query value shapers (JSON the drawer reads)
// ---------------------------------------------------------------------------

/// The `catalog` answer: the source's name/description and every table & view
/// with its row and column counts. Errors (a bad path, an unreachable server)
/// return `Err`, which the handler maps to a `Reply::error` the drawer shows via
/// its `is_error` path — success values carry no `error` field, so the script
/// never has to guard a nil. The `path` field carries a display string (a file
/// path, or a password-redacted URL) — engine-agnostic to the drawer.
fn catalog_value(source: &db::Source, backend: &mut dyn db::Backend) -> Result<Value, String> {
    let entries = backend.catalog()?;
    Ok(json!({
        "path": source.describe(),
        "name": source.display_name(),
        "tables": entries
            .iter()
            .map(|e| json!({
                "name": e.name,
                "kind": e.kind,
                "rows": e.rows,
                "cols": e.cols,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// The `table` answer: `{ heading, row_count, truncated, columns, lines }`.
/// `columns` drives the schema panel; `lines` is a list of `{ text, style }`
/// rows the drawer dumps into a `text_view` — a monospace, column-aligned grid
/// (a header row, a rule, then the data). Errors return `Err` (see
/// [`catalog_value`]).
fn table_value(backend: &mut dyn db::Backend, name: &str) -> Result<Value, String> {
    let columns = backend.columns(name)?;
    let page = backend.page(name)?;
    Ok(json!({
        "heading": name,
        "row_count": page.rows.len(),
        "truncated": page.truncated,
        "columns": columns
            .iter()
            .map(|c| json!({
                "name": c.name,
                "type": c.decl_type,
                "pk": c.pk,
                "notnull": c.notnull,
            }))
            .collect::<Vec<_>>(),
        "lines": grid_lines(&page),
    }))
}

/// One styled row for the drawer's `text_view`. `style` is a
/// `text_view_line_styles` name: "" (plain data), "title" (header), or "dim".
fn row(text: impl Into<String>, style: &str) -> Value {
    json!({ "text": text.into(), "style": style })
}

/// Widest cell (in characters) we render before truncating with an ellipsis —
/// keeps one long text column from blowing out the grid.
const CELL_MAX: usize = 32;

/// Render a page into an aligned monospace grid: a header row (styled `title`),
/// a rule (`dim`), then each data row. Column widths are the max over the
/// header and the shown cells, capped at [`CELL_MAX`].
fn grid_lines(page: &db::Page) -> Vec<Value> {
    if page.columns.is_empty() {
        return vec![row("(no columns)", "dim")];
    }
    let ncols = page.columns.len();

    // Column widths: header vs. every cell, capped.
    let mut widths: Vec<usize> = page
        .columns
        .iter()
        .map(|h| char_len(h).min(CELL_MAX))
        .collect();
    for r in &page.rows {
        for (i, cell) in r.iter().enumerate().take(ncols) {
            widths[i] = widths[i].max(char_len(&sanitize(cell)).min(CELL_MAX));
        }
    }

    let sep = " │ ";
    let mut lines = Vec::with_capacity(page.rows.len() + 3);

    // Header + rule.
    let header = (0..ncols)
        .map(|i| fit(&page.columns[i], widths[i]))
        .collect::<Vec<_>>()
        .join(sep);
    lines.push(row(header, "title"));
    let rule = (0..ncols)
        .map(|i| "─".repeat(widths[i]))
        .collect::<Vec<_>>()
        .join("─┼─");
    lines.push(row(rule, "dim"));

    if page.rows.is_empty() {
        lines.push(row("(no rows)", "dim"));
        return lines;
    }
    for r in &page.rows {
        let cells = (0..ncols)
            .map(|i| {
                fit(
                    &sanitize(r.get(i).map(String::as_str).unwrap_or("")),
                    widths[i],
                )
            })
            .collect::<Vec<_>>()
            .join(sep);
        lines.push(row(cells, ""));
    }
    lines
}

/// Character count (not byte length) — the grid is monospace so alignment is by
/// character.
fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Collapse control whitespace so a multi-line/tabbed cell stays on one grid row.
fn sanitize(s: &str) -> String {
    s.replace(['\n', '\r', '\t'], " ")
}

/// Pad `s` to `width` chars, or truncate to `width - 1` + an ellipsis if longer.
fn fit(s: &str, width: usize) -> String {
    let n = char_len(s);
    if n == width {
        s.to_string()
    } else if n < width {
        let mut out = String::from(s);
        out.extend(std::iter::repeat(' ').take(width - n));
        out
    } else if width == 0 {
        String::new()
    } else {
        let keep: String = s.chars().take(width - 1).collect();
        format!("{keep}…")
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("sqlite-browser: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
