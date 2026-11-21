//! `embed_package!`: a Petal package directory compiled into a host binary.
//! Use it through the `petal` crate (`petal::embed_package!`); its docs are
//! there.

use std::path::Path;
use std::str::FromStr;

use proc_macro::{Span, TokenStream, TokenTree};

#[path = "../../build_support/package_files.rs"]
mod package_files;
#[path = "../../build_support/toml_lite.rs"]
#[allow(dead_code)]
mod toml_lite;

/// See `petal::embed_package!`.
#[proc_macro]
pub fn embed_package(input: TokenStream) -> TokenStream {
    match expand(input) {
        Ok(code) => TokenStream::from_str(&code).expect("the expansion parses"),
        Err(e) => TokenStream::from_str(&format!("::core::compile_error!({e:?})")).expect("an error parses"),
    }
}

fn expand(input: TokenStream) -> Result<String, String> {
    let rel = path_literal(input)?;
    // Relative to the file the macro is written in, as `include_str!` is.
    let file = Span::call_site()
        .local_file()
        .ok_or("embed_package!: the calling file has no path on disk to resolve the directory from")?;
    let base = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let base = if base.is_absolute() {
        base
    } else {
        // A relative file path is relative to where rustc runs: the crate's
        // directory, or its workspace's root for a crate in a workspace.
        std::env::current_dir().map_err(|e| format!("embed_package!: no working directory ({e})"))?.join(base)
    };
    let dir = base.join(&rel);
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("embed_package!(\"{rel}\"): {} ({e})", dir.display()))?;
    let manifest = std::fs::read_to_string(dir.join("petal.toml"))
        .map_err(|_| format!("embed_package!(\"{rel}\"): {} has no petal.toml", dir.display()))?;
    let name = toml_lite::table_entries(&manifest, "package")
        .into_iter()
        .find(|(k, _)| k == "name")
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
        .ok_or_else(|| format!("embed_package!(\"{rel}\"): {}/petal.toml has no [package] name", dir.display()))?;
    let files = package_files::package_files(&dir).map_err(|e| format!("embed_package!(\"{rel}\"): {e}"))?;
    if !files.iter().any(|f| f.ends_with(".ptl")) {
        return Err(format!("embed_package!(\"{rel}\"): {} holds no .ptl modules", dir.display()));
    }
    let mut code = format!("::petal::package::EmbeddedPackage {{ name: {name:?}, files: &[");
    for f in files {
        let abs = dir.join(&f);
        let abs = abs.to_str().ok_or_else(|| format!("embed_package!: {} is not UTF-8", abs.display()))?;
        code.push_str(&format!("({f:?}, ::core::include_str!({abs:?})),"));
    }
    code.push_str("] }");
    Ok(code)
}

/// The one string literal the macro takes.
fn path_literal(input: TokenStream) -> Result<String, String> {
    let mut tokens = input.into_iter();
    let (Some(TokenTree::Literal(lit)), None) = (tokens.next(), tokens.next()) else {
        return Err("embed_package! takes one string literal: the package directory, relative to this file".into());
    };
    let text = lit.to_string();
    match text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        Some(inner) if !inner.contains('\\') => Ok(inner.to_string()),
        _ => Err(format!("embed_package!: expected a plain string literal path, got {text}")),
    }
}
