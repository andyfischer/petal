//! LSP language server for Petal.
//!
//! [`Server`] is transport-agnostic: callers feed raw JSON-RPC messages in via
//! [`Server::handle_message`] and collect outgoing messages from the returned
//! vec. The server manages open documents, re-compiles on change, and responds
//! with diagnostics, hover info, go-to-definition, and completions.
//!
//! [`stdio::serve`] wraps that core in the standard LSP stdio transport
//! (Content-Length framing over stdin/stdout), which is what `petal lsp` runs.

pub mod document;
pub mod lsp_types;
pub mod protocol;
pub mod server;
pub mod stdio;

pub use server::Server;
