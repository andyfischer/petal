//! Petal-IDE session state: the live-program **inspector** that backs the IR
//! panel (`ir_view.ptl`), plus the `host_data` provider that panel pulls from.
//!
//! The App, in IDE mode, holds one [`IdeState`]. Each tick it republishes the
//! IDE target editor's live buffer into the shared [`IrState`] (hash-gated); the
//! IR panel's data-provider closure — attached when a panel pane on the seeded
//! `ir_view.ptl` path is built — renders the selected stage lazily on demand.
//! The `Rc<RefCell<…>>` sharing mirrors the GPP query cache
//! ([`crate::script_client::Shared`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use garden_script::inspect::{self, Stage};
use garden_script::{DataProvider, PanelData};

/// The bundled IR-inspector drawer, seeded to disk on first IDE launch so the
/// pane is a normal `panel(path)` node (see [`crate::App::enable_ide`]).
pub const IR_VIEW_SCRIPT: &str = include_str!("ir_view.ptl");

/// Shared inspector state: the source under inspection plus a per-stage render
/// cache. Written by the App (from the live editor buffer) and read by the IR
/// panel's provider closure.
pub type IrShared = Rc<RefCell<IrState>>;

/// The source last published for inspection and its per-stage render cache.
#[derive(Default)]
pub struct IrState {
    /// The IDE target editor's live buffer text; empty until the first publish.
    source: String,
    /// Rendered text (or compile error) per stage, cleared when `source` changes.
    cache: HashMap<Stage, Result<String, String>>,
}

impl IrState {
    /// Publish new source, clearing the stage cache if it actually changed.
    /// Returns whether it changed (so the caller can request a redraw).
    pub fn set_source(&mut self, source: &str) -> bool {
        if self.source == source {
            return false;
        }
        self.source = source.to_string();
        self.cache.clear();
        true
    }

    /// Render `stage` for the current source, memoizing the result. Empty/blank
    /// source renders empty text (not an error), so a fresh IDE shows nothing
    /// rather than a spurious parse failure.
    fn rendered(&mut self, stage: Stage) -> Result<String, String> {
        if self.source.trim().is_empty() {
            return Ok(String::new());
        }
        if let Some(cached) = self.cache.get(&stage) {
            return cached.clone();
        }
        let result = inspect::render(&self.source, stage);
        self.cache.insert(stage, result.clone());
        result
    }
}

/// The Petal-IDE session's inspector state, held by [`crate::App`] in IDE mode.
pub struct IdeState {
    /// The program the IDE is editing — the editor buffer the IR panel inspects.
    pub target: PathBuf,
    /// Absolute path the bundled IR drawer is seeded to; a panel pane on this
    /// path is the IR inspector and receives an [`ir_data_provider`].
    pub ir_view_path: PathBuf,
    /// Shared inspector state (source + render cache).
    pub ir: IrShared,
    /// Whether an IR inspector pane is currently open (for the toolbar highlight
    /// and to decide open-vs-close on the toolbar's IR button).
    pub ir_open: bool,
}

impl IdeState {
    pub fn new(target: PathBuf, ir_view_path: PathBuf) -> IdeState {
        IdeState {
            target,
            ir_view_path,
            ir: Rc::new(RefCell::new(IrState::default())),
            ir_open: false,
        }
    }
}

/// Build the `host_data(kind, arg)` provider a fresh IR panel is given, capturing
/// a clone of the shared inspector state. It answers:
/// - `host_data("stages", "")` → a list of `{ label }` records (the menu tabs).
/// - `host_data("inspect", <label>)` → `{ text, error }` — the rendered stage
///   text (one field empty), so the drawer reads both keys unconditionally.
pub fn ir_data_provider(shared: IrShared) -> DataProvider {
    Box::new(move |kind: &str, arg: &str| -> PanelData {
        match kind {
            "stages" => PanelData::List(
                inspect::stages()
                    .iter()
                    .map(|(label, _)| {
                        PanelData::Record(vec![("label".into(), PanelData::Str((*label).into()))])
                    })
                    .collect(),
            ),
            "inspect" => {
                let Some(stage) = inspect::stage_from_label(arg) else {
                    return inspect_record(String::new(), format!("unknown stage: {arg}"));
                };
                match shared.borrow_mut().rendered(stage) {
                    Ok(text) => inspect_record(text, String::new()),
                    Err(err) => inspect_record(String::new(), err),
                }
            }
            _ => PanelData::Nil,
        }
    })
}

/// The `{ text, error }` record every `inspect` answer returns (one field empty).
fn inspect_record(text: String, error: String) -> PanelData {
    PanelData::Record(vec![
        ("text".into(), PanelData::Str(text)),
        ("error".into(), PanelData::Str(error)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_source_change_detection_and_cache_clear() {
        let mut st = IrState::default();
        assert!(st.set_source("let a = 1\n"));
        assert!(!st.set_source("let a = 1\n")); // unchanged
                                                // Prime the cache, then a change must clear it.
        let _ = st.rendered(Stage::Ir);
        assert!(!st.cache.is_empty());
        assert!(st.set_source("let a = 2\n"));
        assert!(st.cache.is_empty());
    }

    #[test]
    fn blank_source_renders_empty_not_error() {
        let mut st = IrState::default();
        st.set_source("   \n");
        assert_eq!(st.rendered(Stage::Ir), Ok(String::new()));
    }

    #[test]
    fn provider_answers_stages_and_inspect() {
        let shared: IrShared = Rc::new(RefCell::new(IrState::default()));
        shared.borrow_mut().set_source("let a = 1 + 2\n");
        let mut provider = ir_data_provider(shared);
        // The menu list.
        match provider("stages", "") {
            PanelData::List(items) => assert_eq!(items.len(), inspect::stages().len()),
            other => panic!("expected a list, got {other:?}"),
        }
        // A real stage renders `text`.
        match provider("inspect", "IR") {
            PanelData::Record(fields) => {
                assert!(fields.iter().any(|(k, _)| k == "text"));
            }
            other => panic!("expected a record, got {other:?}"),
        }
        // An unknown stage reports `error`.
        match provider("inspect", "bogus") {
            PanelData::Record(fields) => assert!(fields.iter().any(|(k, _)| k == "error")),
            other => panic!("expected an error record, got {other:?}"),
        }
    }
}
