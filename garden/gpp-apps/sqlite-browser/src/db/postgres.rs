//! The Postgres [`Backend`]: connect to a server by URL and introspect it with
//! the system catalogs (`pg_class`, `pg_attribute`, `pg_constraint`). Like the
//! SQLite backend it's a read-only browser — the session is pinned to
//! `default_transaction_read_only` and only ever runs `SELECT`s — and it renders
//! every value into the same [`CatalogEntry`] / [`Column`] / [`Page`] shapes.
//!
//! Two things differ from SQLite and are handled here:
//!
//! - **Row counts are estimates.** An exact `COUNT(*)` per table would scan the
//!   whole database on open; the catalog uses `pg_class.reltuples` (the planner's
//!   estimate), which is instant and plenty for the list and the bar chart. The
//!   detail view's own row count is still the exact number of rows fetched.
//! - **Schemas.** A server has many; objects outside the default `public` schema
//!   are listed qualified (`schema.name`). [`PostgresBackend::resolve`] maps a
//!   catalog name back to its `(schema, table)` pair for follow-up queries.

use std::collections::HashMap;

use postgres::Client;

use super::{Backend, CatalogEntry, Column, Page, ROW_LIMIT};

/// A live Postgres connection plus the name → `(schema, table)` index built by
/// the last [`catalog`](Backend::catalog) call, so a follow-up `columns`/`page`
/// for a qualified name resolves to the exact relation.
pub struct PostgresBackend {
    client: Client,
    index: HashMap<String, (String, String)>,
}

impl PostgresBackend {
    /// Connect to `url` (a `postgres://…` connection string). A native-TLS
    /// connector is supplied so servers that require or offer SSL (most managed
    /// Postgres) work; plaintext servers negotiate down to no TLS. The session
    /// is set read-only with a statement timeout as a browser-safety net.
    pub fn connect(url: &str) -> Result<PostgresBackend, String> {
        let connector = native_tls::TlsConnector::builder()
            .build()
            .map_err(|e| format!("TLS setup failed: {e}"))?;
        let tls = postgres_native_tls::MakeTlsConnector::new(connector);
        let mut client =
            Client::connect(url, tls).map_err(|e| format!("could not connect: {e}"))?;
        // Best-effort hardening; a server that rejects either GUC still browses.
        let _ = client.batch_execute(
            "SET default_transaction_read_only = on; SET statement_timeout = '15s';",
        );
        Ok(PostgresBackend {
            client,
            index: HashMap::new(),
        })
    }

    /// Map a catalog display name back to `(schema, table)`. Prefers the index
    /// built by the last `catalog()`; falls back to splitting a `schema.name`
    /// (or assuming `public` for a bare name) when a detail query somehow
    /// precedes a catalog load.
    fn resolve(&self, name: &str) -> (String, String) {
        if let Some((schema, table)) = self.index.get(name) {
            return (schema.clone(), table.clone());
        }
        match name.split_once('.') {
            Some((schema, table)) => (schema.to_string(), table.to_string()),
            None => ("public".to_string(), name.to_string()),
        }
    }
}

/// The catalog query: every ordinary table, partitioned table, view and
/// materialized view outside the system schemas, tables before views then
/// name-sorted, with an estimated row count and an exact column count.
const CATALOG_SQL: &str = "\
    SELECT n.nspname AS schema, \
           c.relname AS name, \
           c.relkind AS kind, \
           GREATEST(c.reltuples, 0)::bigint AS rows, \
           (SELECT count(*) FROM pg_attribute a \
              WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped) AS cols \
    FROM pg_class c \
    JOIN pg_namespace n ON n.oid = c.relnamespace \
    WHERE c.relkind IN ('r','p','v','m') \
      AND n.nspname NOT IN ('pg_catalog','information_schema') \
      AND n.nspname NOT LIKE 'pg\\_toast%' \
      AND n.nspname NOT LIKE 'pg\\_temp%' \
    ORDER BY c.relkind IN ('v','m'), n.nspname, c.relname";

/// A relation's columns with declared type, NOT NULL, and primary-key membership.
const COLUMNS_SQL: &str = "\
    SELECT a.attname AS name, \
           format_type(a.atttypid, a.atttypmod) AS decl_type, \
           a.attnotnull AS notnull, \
           COALESCE(( \
             SELECT true FROM pg_constraint pc \
             WHERE pc.conrelid = c.oid AND pc.contype = 'p' \
               AND a.attnum = ANY(pc.conkey) \
           ), false) AS pk \
    FROM pg_attribute a \
    JOIN pg_class c ON c.oid = a.attrelid \
    JOIN pg_namespace n ON n.oid = c.relnamespace \
    WHERE n.nspname = $1 AND c.relname = $2 \
      AND a.attnum > 0 AND NOT a.attisdropped \
    ORDER BY a.attnum";

impl Backend for PostgresBackend {
    fn catalog(&mut self) -> Result<Vec<CatalogEntry>, String> {
        let rows = self
            .client
            .query(CATALOG_SQL, &[])
            .map_err(|e| format!("reading schema: {e}"))?;
        let mut index = HashMap::with_capacity(rows.len());
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let schema: String = row.get("schema");
            let relname: String = row.get("name");
            // relkind is Postgres' internal `"char"` type → i8 here.
            let kind_byte: i8 = row.get("kind");
            let is_view = kind_byte == b'v' as i8 || kind_byte == b'm' as i8;
            let est: i64 = row.get("rows");
            let cols: i64 = row.get("cols");
            // Qualify only when outside the default schema, to keep the common
            // `public` case clean; `resolve` inverts this via the index.
            let display = if schema == "public" {
                relname.clone()
            } else {
                format!("{schema}.{relname}")
            };
            index.insert(display.clone(), (schema, relname));
            out.push(CatalogEntry {
                name: display,
                kind: if is_view { "view" } else { "table" }.to_string(),
                rows: est.max(0),
                cols: cols.max(0) as usize,
            });
        }
        self.index = index;
        Ok(out)
    }

    fn columns(&mut self, name: &str) -> Result<Vec<Column>, String> {
        let (schema, table) = self.resolve(name);
        let rows = self
            .client
            .query(COLUMNS_SQL, &[&schema, &table])
            .map_err(|e| format!("reading columns: {e}"))?;
        Ok(rows
            .iter()
            .map(|row| Column {
                name: row.get("name"),
                decl_type: row.get("decl_type"),
                notnull: row.get("notnull"),
                pk: row.get("pk"),
            })
            .collect())
    }

    fn page(&mut self, name: &str) -> Result<Page, String> {
        let (schema, table) = self.resolve(name);
        // Reuse the schema query for the header names, in declaration order.
        let colnames: Vec<String> = self.columns(name)?.into_iter().map(|c| c.name).collect();
        if colnames.is_empty() {
            return Ok(Page {
                columns: vec![],
                rows: vec![],
                truncated: false,
            });
        }
        // Cast every column to text so one generic query renders any column type
        // to a display string; NULL comes back as SQL NULL → "NULL". Fetch one
        // extra row to detect truncation without a second count.
        let select_list = colnames
            .iter()
            .map(|c| format!("{}::text", quote_ident(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {select_list} FROM {}.{} LIMIT {}",
            quote_ident(&schema),
            quote_ident(&table),
            ROW_LIMIT + 1
        );
        let rows = self
            .client
            .query(&sql, &[])
            .map_err(|e| format!("querying rows: {e}"))?;
        let truncated = rows.len() > ROW_LIMIT;
        let out = rows
            .iter()
            .take(ROW_LIMIT)
            .map(|row| {
                (0..colnames.len())
                    .map(|i| render_cell(row.get::<_, Option<String>>(i)))
                    .collect()
            })
            .collect();
        Ok(Page {
            columns: colnames,
            rows: out,
            truncated,
        })
    }

    fn is_live(&self) -> bool {
        !self.client.is_closed()
    }
}

/// Render a text-cast cell to a display string; a SQL NULL becomes the literal
/// `NULL`, matching the SQLite backend.
fn render_cell(v: Option<String>) -> String {
    v.unwrap_or_else(|| "NULL".to_string())
}

/// Quote an SQL identifier — doubling any embedded quote — so a schema/table/
/// column name with spaces, keywords, or quotes interpolates safely.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The database name for the pane title/header, from a connection URL: the path
/// component (`…/dbname`), else the host, else a generic fallback.
pub fn url_display_name(url: &str) -> String {
    let rest = url.splitn(2, "://").nth(1).unwrap_or(url);
    // Drop any `user:pass@` prefix (rsplit keeps the host side).
    let host_and_path = rest.rsplitn(2, '@').next().unwrap_or(rest);
    if let Some(slash) = host_and_path.find('/') {
        let db = &host_and_path[slash + 1..];
        let db = db.split(['?', '#']).next().unwrap_or(db);
        if !db.is_empty() {
            return db.to_string();
        }
    }
    let hostport = host_and_path
        .split(['/', '?'])
        .next()
        .unwrap_or(host_and_path);
    let host = hostport.split(':').next().unwrap_or(hostport);
    if host.is_empty() {
        "postgres".to_string()
    } else {
        host.to_string()
    }
}

/// A connection URL with any password redacted, for display in the header.
/// `postgres://user:secret@host/db` → `postgres://user:***@host/db`.
pub fn redact_url(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some(parts) => parts,
        None => return url.to_string(),
    };
    if let Some(at) = rest.find('@') {
        let (userinfo, host_part) = rest.split_at(at); // host_part starts with '@'
        if let Some(colon) = userinfo.find(':') {
            let user = &userinfo[..colon];
            return format!("{scheme}://{user}:***{host_part}");
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_ident_escapes_quotes() {
        assert_eq!(quote_ident("plain"), "\"plain\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn display_name_prefers_dbname_then_host() {
        assert_eq!(
            url_display_name("postgres://u:p@localhost:5432/shop"),
            "shop"
        );
        assert_eq!(
            url_display_name("postgresql://host/analytics?sslmode=require"),
            "analytics"
        );
        assert_eq!(
            url_display_name("postgres://u:p@db.internal:5432/"),
            "db.internal"
        );
        assert_eq!(url_display_name("postgres://db.internal"), "db.internal");
    }

    #[test]
    fn redact_hides_password_only() {
        assert_eq!(
            redact_url("postgres://alice:s3cret@host:5432/shop"),
            "postgres://alice:***@host:5432/shop"
        );
        // No password → unchanged.
        assert_eq!(
            redact_url("postgres://alice@host/shop"),
            "postgres://alice@host/shop"
        );
        assert_eq!(redact_url("postgres://host/shop"), "postgres://host/shop");
    }
}
