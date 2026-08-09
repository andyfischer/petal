//! The [`App`] side of the per-session event log ([`crate::event_log`]): the
//! recording helper threaded through the input/command paths, the flush poll
//! tick the frontends drive, and the `:report` command handler.

use crate::event_log::EventLog;

use super::App;

impl App {
    /// Attach the per-session event log, called by each frontend right after
    /// [`App::new`](super::App::new). `None` disables logging — the case in unit
    /// tests and when the state database is unavailable.
    pub fn set_event_log(&mut self, log: Option<EventLog>) {
        self.event_log = log;
    }

    /// Attach the set of save-protected files (see [`App::save_as_paths`]),
    /// called by each frontend right after [`App::new`](super::App::new). Saving
    /// a pane whose file is in this set prompts for a filename instead of
    /// overwriting it — how the Petal-IDE scratch is protected.
    pub fn set_save_as_paths(&mut self, paths: std::collections::HashSet<std::path::PathBuf>) {
        self.save_as_paths = paths;
    }

    /// Record one event in the log, if one is attached (otherwise a no-op).
    /// `category` is a stable kind tag ("key", "command", "mouse", "file",
    /// "layout", "script"); `detail` is a short human description for replay.
    pub(in crate::app) fn log_event(&mut self, category: &'static str, detail: impl Into<String>) {
        if let Some(log) = self.event_log.as_mut() {
            log.log(category, detail.into());
        }
    }

    /// Flush the event log on the frontend poll tick (the same ~200ms cadence as
    /// [`poll_script`](super::App::poll_script)). The log writes only once its
    /// [`FLUSH_INTERVAL`](crate::event_log::FLUSH_INTERVAL) has elapsed, so this
    /// is cheap to call every tick.
    pub fn poll_event_log(&mut self) {
        if let Some(log) = self.event_log.as_mut() {
            log.maybe_flush();
        }
    }

    /// `:report <text>` — file a bug/feature report capturing the recent event
    /// log as context, and acknowledge it in the status bar. With no event log
    /// attached (state unavailable), report a friendly error instead.
    pub(in crate::app) fn file_report(&mut self, message: &str) {
        let Some(log) = self.event_log.as_mut() else {
            self.status_error = Some("E: reporting unavailable (no state database)".to_string());
            return;
        };
        match log.file_report(message) {
            Ok(summary) => {
                self.status_note = Some(format!(
                    "report #{} filed with {} event{} of context",
                    summary.id,
                    summary.events,
                    if summary.events == 1 { "" } else { "s" },
                ));
            }
            Err(err) => self.status_error = Some(format!("report failed: {err}")),
        }
    }
}
