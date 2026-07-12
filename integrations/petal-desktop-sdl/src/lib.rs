//! petal-sdl — an SDL2 desktop host for Petal programs.
//!
//! This crate is layered so apps can build on it without copying it:
//!
//! - The **generic game loop** ([`game_loop`]) owns platform policy — SDL init,
//!   the window/canvas, the event pump, frame timing, the
//!   agent/headless/screenshot/record modes, hot reload, and pointer grab. It
//!   drives a [`Host`] for the per-app parts (natives, painting, capture).
//! - The **default host** ([`DefaultHost`]) is what the shipped `petal-sdl`
//!   binary runs: an SDL-canvas renderer over the `petal-ui` draw vocabulary,
//!   plus an example browser and sandboxed file I/O. Pure-Petal sample apps use
//!   this binary unchanged.
//! - Apps that need a **different renderer or native set** (e.g. `petal-fps`'s
//!   software 3D rasterizer) implement their own [`Host`] and reuse everything
//!   else from this crate — see `docs/building-on-integrations.md`.
//!
//! The reusable building blocks ([`input`] SDL translation, [`protocol`] agent
//! JSON, [`watcher`] hot reload, [`screenshot`] PNG encoding, [`font`] ladder,
//! [`renderer`] SDL-canvas primitives) are public so hosts can compose them.

pub mod commands;
pub mod default_host;
pub mod font;
pub mod game_loop;
pub mod input;
pub mod native_fns;
pub mod protocol;
pub mod renderer;
pub mod screenshot;
pub mod watcher;

pub use default_host::DefaultHost;
pub use game_loop::{
    EscapeAction, GameConfig, Host, ScriptSwitch, run_agent, run_game, run_headless, run_record,
    run_screenshot,
};
