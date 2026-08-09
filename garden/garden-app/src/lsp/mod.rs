//! LSP client: language servers as subprocesses, one per language.
//!
//! Three layers, none of them Petal-specific:
//!
//! - [`framing`] — the `Content-Length` wire format (NOT GPP's
//!   newline-delimited JSON; see the module docs for why they can't be shared).
//! - [`client`] — [`LspClient`], one server child process: handshake, document
//!   notifications, completion requests, and a reader thread.
//! - [`registry`] — which server serves which file, as data.
//!
//! Above these sits `app/lsp.rs`, which owns the per-language clients and
//! drives document sync off the poll tick.
//!

pub mod client;
pub mod framing;
pub mod registry;

pub use client::LspClient;
pub use registry::path_to_uri;
