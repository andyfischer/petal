//! Per-session event log and the bug/feature reports filed from it, persisted
//! in the `~/.garden/state` SQLite database (see [`crate::state`]).
//!
//! The goal is visibility and replay for bug reports: every window records the
//! actions and events it processes — key presses, ex commands, file opens,
//! script reloads, layout changes — so a later report carries the context that
//! led up to it.
//!
//! Writes are **buffered in memory** and flushed to the `events` table on a
//! [`FLUSH_INTERVAL`] timer (and on drop), so a steady keystroke stream costs
//! one small batched transaction every few seconds rather than a write per key.
//! The [`App`](crate::app::App) calls [`maybe_flush`](EventLog::maybe_flush) on
//! its poll tick; the timer inside decides when an actual write happens.
//!
//! The `:report` command calls [`file_report`](EventLog::file_report), which
//! snapshots the previous [`REPORT_WINDOW`] of events into the `reports` table
//! alongside the user's description — the same path serving both bug reports
//! and feature requests.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

/// How often the in-memory buffer is flushed to the database. The log only
/// writes once this much time has elapsed since the last flush, so the App can
/// call [`maybe_flush`](EventLog::maybe_flush) cheaply on every poll tick.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// How far back a `:report` reaches for context: the previous five minutes of
/// events are attached to the report.
pub const REPORT_WINDOW: Duration = Duration::from_secs(5 * 60);

/// One buffered event awaiting the next flush.
struct Event {
    /// Wall-clock time, milliseconds since the Unix epoch.
    at_ms: i64,
    /// Stable kind tag ("key", "command", "mouse", "file", "layout", "script").
    category: &'static str,
    /// Short human-readable description, for replay.
    detail: String,
}

/// The result of filing a report, for the status-bar acknowledgement.
pub struct ReportSummary {
    /// The new row's id in the `reports` table.
    pub id: i64,
    /// How many events of context the report captured.
    pub events: usize,
}

/// A window's event log: an open connection to the state database, the
/// window/session id every row is tagged with, the pending in-memory buffer,
/// and the flush timer.
pub struct EventLog {
    conn: Connection,
    window_id: i64,
    buffer: Vec<Event>,
    last_flush: Instant,
}

impl EventLog {
    /// Wrap an already-open (and migrated) state-database connection as the
    /// event log for `window_id`. Built by
    /// [`State::into_event_log`](crate::state::State::into_event_log).
    pub fn new(conn: Connection, window_id: i64) -> EventLog {
        EventLog {
            conn,
            window_id,
            buffer: Vec::new(),
            last_flush: Instant::now(),
        }
    }

    /// Record one event, timestamped now. Buffered in memory and written on the
    /// next flush.
    pub fn log(&mut self, category: &'static str, detail: String) {
        self.buffer.push(Event {
            at_ms: now_ms(),
            category,
            detail,
        });
    }

    /// Flush the buffer if at least [`FLUSH_INTERVAL`] has elapsed since the
    /// last write. Cheap to call on every frontend poll tick.
    pub fn maybe_flush(&mut self) {
        if self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush();
        }
    }

    /// Write every buffered event to the database in one transaction, then
    /// clear the buffer and reset the flush timer. Best-effort: a database
    /// error is logged and the buffer dropped, so a wedged DB can never grow
    /// memory without bound or take the editor down.
    pub fn flush(&mut self) {
        self.last_flush = Instant::now();
        if self.buffer.is_empty() {
            return;
        }
        if let Err(err) = self.write_buffer() {
            eprintln!("garden: event log flush failed: {err}");
        }
        self.buffer.clear();
    }

    fn write_buffer(&mut self) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO events (window_id, at_ms, category, detail) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for ev in &self.buffer {
                stmt.execute(rusqlite::params![
                    self.window_id,
                    ev.at_ms,
                    ev.category,
                    ev.detail
                ])?;
            }
        }
        tx.commit()
    }

    /// File a `:report` bug/feature report. Flushes pending events first (so the
    /// just-typed `:report` itself is included), gathers the previous
    /// [`REPORT_WINDOW`] of events as a formatted context block, stores the
    /// report with the user's `message`, and returns its id and event count.
    pub fn file_report(&mut self, message: &str) -> Result<ReportSummary, String> {
        // Flush first so everything up to and including the `:report` keystroke
        // is in the table and shows up in the gathered context.
        self.flush();
        let at_ms = now_ms();
        let cutoff = at_ms - REPORT_WINDOW.as_millis() as i64;
        let (context, events) = self
            .gather_context(cutoff)
            .map_err(|e| format!("gather report context: {e}"))?;
        self.conn
            .execute(
                "INSERT INTO reports (window_id, at_ms, message, context) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![self.window_id, at_ms, message, context],
            )
            .map_err(|e| format!("store report: {e}"))?;
        Ok(ReportSummary {
            id: self.conn.last_insert_rowid(),
            events,
        })
    }

    /// Build the human-readable context block from this window's events at or
    /// after `cutoff`, oldest first — one `YYYY-MM-DD HH:MM:SS  [category]
    /// detail` line each (UTC). Returns the block and the line count.
    fn gather_context(&self, cutoff: i64) -> rusqlite::Result<(String, usize)> {
        let mut stmt = self.conn.prepare(
            "SELECT strftime('%Y-%m-%d %H:%M:%S', at_ms / 1000.0, 'unixepoch'), category, detail
               FROM events
              WHERE window_id = ?1 AND at_ms >= ?2
              ORDER BY at_ms ASC, id ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![self.window_id, cutoff], |row| {
            let ts: String = row.get(0)?;
            let category: String = row.get(1)?;
            let detail: String = row.get(2)?;
            Ok(format!("{ts}  [{category}] {detail}"))
        })?;
        let mut lines = Vec::new();
        for line in rows {
            lines.push(line?);
        }
        Ok((lines.join("\n"), lines.len()))
    }
}

impl Drop for EventLog {
    fn drop(&mut self) {
        // Persist whatever is still buffered so a clean quit loses nothing.
        self.flush();
    }
}

/// Milliseconds since the Unix epoch, or 0 if the clock predates it.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;

    /// Open a temp state DB and turn it into an event log for a fresh window.
    fn temp_log() -> (tempfile::TempDir, EventLog) {
        let dir = tempfile::tempdir().unwrap();
        let state = State::open(dir.path()).unwrap();
        let id = state.new_window_id().unwrap();
        (dir, state.into_event_log(id))
    }

    fn report_count(log: &EventLog) -> i64 {
        log.conn
            .query_row("SELECT count(*) FROM reports", [], |r| r.get(0))
            .unwrap()
    }

    fn event_count(log: &EventLog) -> i64 {
        log.conn
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn events_buffer_until_flushed() {
        let (_dir, mut log) = temp_log();
        log.log("key", "a".to_string());
        log.log("key", "b".to_string());
        // Nothing is written until an explicit flush (the timer hasn't fired).
        assert_eq!(event_count(&log), 0);
        log.flush();
        assert_eq!(event_count(&log), 2);
        // Flushing again with an empty buffer is a no-op.
        log.flush();
        assert_eq!(event_count(&log), 2);
    }

    #[test]
    fn maybe_flush_respects_the_interval() {
        let (_dir, mut log) = temp_log();
        log.log("key", "a".to_string());
        // The timer was just reset by `new`, so a poll right away writes nothing.
        log.maybe_flush();
        assert_eq!(event_count(&log), 0);
        // Pretend the interval elapsed.
        log.last_flush = Instant::now() - (FLUSH_INTERVAL + Duration::from_secs(1));
        log.maybe_flush();
        assert_eq!(event_count(&log), 1);
    }

    #[test]
    fn report_flushes_pending_events_and_captures_them_as_context() {
        let (_dir, mut log) = temp_log();
        log.log("command", "typed report".to_string());
        let summary = log.file_report("dd deletes the wrong line").unwrap();
        assert_eq!(report_count(&log), 1);
        // The pending event was flushed before the snapshot, so it is context.
        assert_eq!(summary.events, 1);

        let (message, context): (String, String) = log
            .conn
            .query_row(
                "SELECT message, context FROM reports WHERE id = ?1",
                [summary.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(message, "dd deletes the wrong line");
        assert!(
            context.contains("[command] typed report"),
            "context was: {context}"
        );
    }

    #[test]
    fn report_context_excludes_events_older_than_the_window() {
        let (_dir, mut log) = temp_log();
        // One stale event (well before the window) and one recent event.
        let now = now_ms();
        log.buffer.push(Event {
            at_ms: now - (REPORT_WINDOW.as_millis() as i64) - 60_000,
            category: "key",
            detail: "old".to_string(),
        });
        log.buffer.push(Event {
            at_ms: now,
            category: "key",
            detail: "fresh".to_string(),
        });
        let summary = log.file_report("context window check").unwrap();
        // Only the in-window event is captured.
        assert_eq!(summary.events, 1);
        let context: String = log
            .conn
            .query_row(
                "SELECT context FROM reports WHERE id = ?1",
                [summary.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(context.contains("fresh"));
        assert!(!context.contains("old"), "stale event leaked in: {context}");
    }

    #[test]
    fn drop_flushes_remaining_events() {
        let dir = tempfile::tempdir().unwrap();
        {
            let state = State::open(dir.path()).unwrap();
            let id = state.new_window_id().unwrap();
            let mut log = state.into_event_log(id);
            log.log("key", "z".to_string());
            // No explicit flush — rely on Drop.
        }
        // Reopen and confirm the event survived.
        let state = State::open(dir.path()).unwrap();
        let id = state.new_window_id().unwrap();
        let log = state.into_event_log(id);
        assert_eq!(event_count(&log), 1);
    }
}
