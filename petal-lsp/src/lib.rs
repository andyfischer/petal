//! petal-lsp — LSP language server for the Petal programming language.
//!
//! A transport-agnostic library: callers feed raw JSON-RPC messages in via
//! [`Server::handle_message`] and collect outgoing messages from the returned
//! vec. The server manages open documents, re-compiles on change, and
//! responds with diagnostics, hover info, go-to-definition, and completions.

pub mod document;
pub mod lsp_types;
pub mod protocol;
pub mod server;

pub use server::Server;
