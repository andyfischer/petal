//! Draw-command decoding now lives in `petal_ui::draw` (the standard
//! vocabulary shared by every embedder); this module re-exports it under the
//! app's historical paths.

pub use petal_ui::draw::{
    clear_draw_commands, take_draw_commands, take_draw_commands_for, DrawCommand,
};
