//! Which language server serves which file.
//!
//! A [`LanguageServer`] is pure data: extensions, a command, args, and the LSP
//! `languageId`. Adding rust-analyzer should be a table entry, never code — if
//! anything Petal-specific leaks past this module, the seam has failed.
//!
//! Binary resolution follows the GPP convention (`process_pane::sibling_bin`):
//! an explicit environment override, else a sibling of the running executable,
//! else the bare name on `$PATH`. Garden neither builds nor vendors any server.

use std::path::Path;

/// A language server Garden knows how to spawn.
#[derive(Debug, Clone)]
pub struct LanguageServer {
    /// LSP `languageId`, sent on didOpen.
    pub language_id: &'static str,
    /// File extensions this server handles (no leading dot).
    pub extensions: &'static [&'static str],
    /// Executable name, before resolution.
    pub command: &'static str,
    /// Arguments that turn the executable into a server on stdio.
    pub args: &'static [&'static str],
    /// Environment variable overriding [`Self::command`] with a full path.
    pub env_override: &'static str,
}

impl LanguageServer {
    /// The command to actually spawn: `$<env_override>`, else a sibling of the
    /// running exe, else the bare name for `$PATH` to resolve.
    pub fn resolved_command(&self) -> String {
        if let Some(path) = std::env::var_os(self.env_override) {
            return path.to_string_lossy().into_owned();
        }
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(self.command)));
        match beside {
            Some(path) if path.exists() => path.to_string_lossy().into_owned(),
            _ => self.command.to_string(),
        }
    }

    pub fn args(&self) -> Vec<String> {
        self.args.iter().map(|a| a.to_string()).collect()
    }
}

/// Every server Garden can start.
///
/// Petal's server is the `petal` binary itself (`petal lsp`), not a separate
/// `petal-lsp` executable — so anyone with Petal installed already has it. See
pub const REGISTRY: &[LanguageServer] = &[LanguageServer {
    language_id: "petal",
    extensions: &["ptl"],
    command: "petal",
    args: &["lsp"],
    env_override: "GARDEN_PETAL_LSP_BIN",
}];

/// The server for `path`, by extension. `None` means "no LSP for this file",
/// which is the common case and never an error.
pub fn for_path(path: &Path) -> Option<&'static LanguageServer> {
    let ext = path.extension()?.to_str()?;
    REGISTRY
        .iter()
        .find(|s| s.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// `file://` URI for an absolute path — the document identity LSP uses.
///
/// Percent-encoding is deliberately minimal: only the characters that would
/// otherwise change how the URI parses. Over-encoding breaks the round trip
/// back to a path, which matters because responses are matched by URI.
pub fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().as_bytes() {
        match byte {
            b'%' => out.push_str("%25"),
            b'#' => out.push_str("%23"),
            b'?' => out.push_str("%3F"),
            b' ' => out.push_str("%20"),
            _ => out.push(*byte as char),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolves_petal_by_extension() {
        let server = for_path(&PathBuf::from("/proj/init.ptl")).expect("petal server");
        assert_eq!(server.language_id, "petal");
        assert_eq!(server.command, "petal");
        assert_eq!(server.args, &["lsp"]);
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert!(for_path(&PathBuf::from("/proj/Init.PTL")).is_some());
    }

    #[test]
    fn unknown_and_extensionless_files_have_no_server() {
        assert!(for_path(&PathBuf::from("/proj/notes.txt")).is_none());
        assert!(for_path(&PathBuf::from("/proj/README")).is_none());
        assert!(for_path(&PathBuf::from("/proj/.gitignore")).is_none());
    }

    #[test]
    fn an_env_override_wins_over_path_resolution() {
        let server = &REGISTRY[0];
        // SAFETY: single-threaded test; the var is scoped to this assertion.
        unsafe { std::env::set_var(server.env_override, "/custom/petal") };
        assert_eq!(server.resolved_command(), "/custom/petal");
        unsafe { std::env::remove_var(server.env_override) };
    }

    #[test]
    fn builds_a_file_uri() {
        assert_eq!(
            path_to_uri(&PathBuf::from("/a/b/c.ptl")),
            "file:///a/b/c.ptl"
        );
    }

    #[test]
    fn encodes_the_characters_that_would_reparse_the_uri() {
        assert_eq!(
            path_to_uri(&PathBuf::from("/a/my notes#1.ptl")),
            "file:///a/my%20notes%231.ptl"
        );
    }
}
