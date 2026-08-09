//! Program inspection: render a Petal source string to display text for a
//! chosen compilation stage (IR term-graph, bytecode, AST). A thin re-export of
//! the upstream [`petal::inspect`] surface, so `garden-app` reaches the stage
//! renderers without a direct `petal` dependency — the same cross-crate
//! discipline the input/panel re-exports follow. Garden's Petal-IDE IR inspector
//! panel is the consumer.

pub use petal::inspect::{render, stage_from_label, stages, Stage};
