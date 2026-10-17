//! The host a document is written for, as its project describes it.
//!
//! The language server (`petal lsp`) and `petal check` compile each file
//! alone, but a host may put names in scope
//! that the file never imports: a module it binds as an implicit prelude, or
//! library files it joins ahead of the script into one program. Compiled
//! alone, such a script calls nothing but unknown functions. The project's
//! `petal.toml` can say what its host does, in a `[host]` section
//! ([`crate::package::HostScope`]):
//!
//! ```toml
//! [host]
//! imports = ["ui", "garden"]
//! prelude = ["host/garden.ptl", "lib/theme.ptl", "lib/widgets.ptl"]
//! ```
//!
//! and both compile the document the way that host would:
//!
//! - each of `imports` is bound like the core prelude `std` — every export
//!   bare and weak, under the file's own imports and declarations, and only
//!   when the file names one of its exports ([`HostView::apply`]);
//! - the `prelude` files are joined ahead of the document as one program
//!   ([`HostView::join`]), each one's import lines moved to the top since
//!   imports must come first. A document that is itself one of the files
//!   sees only the files listed before it.
//!
//! A relative import in a prelude file (`import ./widgets`) names a file
//! beside that prelude file, so when the file is joined into a document in
//! another directory its path is rewritten to reach the same file from the
//! document's directory ([`HostView::join`]).
//!
//! The joined text keeps every line whole, so a position in it maps back to
//! the line of the file it came from ([`Joined::origin`]) with the same column
//! (but on a rewritten import line). A findings list or a load error of the
//! joined program is moved back onto the document with
//! [`Joined::relocate_program`] and [`Joined::relocate_load_error`].

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::source_map::{ENTRY_FILE, SourceSpan};

/// One file of the host's prelude.
#[derive(Debug, Clone)]
pub struct PreludeFile {
    /// As the manifest writes it, relative to the manifest's directory.
    pub name: String,
    pub path: PathBuf,
    pub text: String,
}

/// What a document's host puts in scope.
#[derive(Debug, Clone, Default)]
pub struct HostView {
    pub imports: Vec<String>,
    pub prelude: Vec<PreludeFile>,
    /// What could not be read (a prelude file that does not exist), worded
    /// for a diagnostic.
    pub problems: Vec<String>,
    /// The document's directory, canonical: what a relative import in a
    /// prelude file is rewritten against. `None` leaves those imports as
    /// they are written.
    pub doc_dir: Option<PathBuf>,
}

impl HostView {
    /// The host of the document at `doc`, from the nearest `petal.toml` at or
    /// above it. `open` stands in for files on disk. Empty when there is no
    /// manifest, it has no `[host]`, or it does not parse (the module loader
    /// reports a broken manifest itself).
    pub fn for_document(doc: &Path, open: &[(PathBuf, &str)]) -> HostView {
        let Some(manifest_path) = crate::package::find_manifest(doc) else {
            return HostView::default();
        };
        let Some(dir) = manifest_path.parent() else {
            return HostView::default();
        };
        let Ok(manifest) = crate::package::Manifest::load(dir) else {
            return HostView::default();
        };
        let scope = manifest.host;
        let doc = crate::module::canonical_path(doc);
        let mut view = HostView {
            imports: scope.imports,
            doc_dir: doc.parent().map(Path::to_path_buf),
            ..HostView::default()
        };
        for name in scope.prelude {
            let path = crate::module::canonical_path(&dir.join(&name));
            // A file of the prelude sees only the files before it.
            if path == doc {
                break;
            }
            let from_open = open
                .iter()
                .find(|(p, _)| crate::module::canonical_path(p) == path)
                .map(|(_, text)| text.to_string());
            let text = match from_open {
                Some(text) => text,
                None => match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(e) => {
                        view.problems.push(format!(
                            "petal.toml [host] prelude: cannot read {name}: {e}"
                        ));
                        continue;
                    }
                },
            };
            view.prelude.push(PreludeFile { name, path, text });
        }
        view
    }

    /// Bind the host's implicit imports in `env`, as a host's prelude.
    pub fn apply(&self, env: &mut crate::env::Env) {
        env.add_tooling_implicit_imports(&self.imports);
    }

    /// The program the host would compile for `source`: the prelude files
    /// and then the document, with every file's leading import lines moved
    /// to the top. With no prelude files it is `source` itself.
    ///
    /// A prelude file's relative imports are rewritten to name the same file
    /// from the document's directory, since the joined program is compiled
    /// as the document.
    pub fn join(&self, source: &str) -> Joined {
        if self.prelude.is_empty() {
            return Joined {
                text: source.to_string(),
                origins: Vec::new(),
            };
        }
        let pieces = self
            .prelude
            .iter()
            .enumerate()
            .map(|(i, f)| (Some(i), f.text.as_str()))
            .chain(std::iter::once((None, source)));
        let mut hoisted: Vec<(Cow<str>, LineOrigin)> = Vec::new();
        let mut body: Vec<(Cow<str>, LineOrigin)> = Vec::new();
        for (file, text) in pieces {
            let header = header_lines(text);
            for (i, line) in text.split('\n').enumerate() {
                let n = i as u32 + 1;
                let origin = match file {
                    Some(f) => LineOrigin::Prelude(f, n),
                    None => LineOrigin::Doc(n),
                };
                if header.contains(&i) {
                    let line = match (file, &self.doc_dir) {
                        (Some(f), Some(doc_dir)) => {
                            rebase_import(line, &self.prelude[f].path, doc_dir)
                        }
                        _ => Cow::Borrowed(line),
                    };
                    hoisted.push((line, origin));
                    body.push((Cow::Borrowed(""), origin));
                } else {
                    body.push((Cow::Borrowed(line), origin));
                }
            }
        }
        let all: Vec<(Cow<str>, LineOrigin)> = hoisted.into_iter().chain(body).collect();
        Joined {
            text: all
                .iter()
                .map(|(l, _)| l.as_ref())
                .collect::<Vec<_>>()
                .join("\n"),
            origins: all.into_iter().map(|(_, o)| o).collect(),
        }
    }
}

/// Where a line of a [`Joined`] program came from (lines 1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOrigin {
    /// The document's own line.
    Doc(u32),
    /// Line of the prelude file at this index of [`HostView::prelude`].
    Prelude(usize, u32),
}

/// A document joined with its host's prelude files.
#[derive(Debug, Clone)]
pub struct Joined {
    pub text: String,
    /// Per line of `text`; empty when `text` is the document unchanged.
    origins: Vec<LineOrigin>,
}

impl Joined {
    /// Where line `line` (1-based) of the joined text came from.
    pub fn origin(&self, line: u32) -> LineOrigin {
        if self.origins.is_empty() {
            return LineOrigin::Doc(line);
        }
        let i = (line as usize)
            .saturating_sub(1)
            .min(self.origins.len() - 1);
        self.origins[i]
    }

    /// `span` of the joined program as a span of the document, when it is in
    /// the document: the same columns on the document's own lines.
    pub fn doc_span(&self, span: SourceSpan) -> Option<SourceSpan> {
        if span.file != ENTRY_FILE {
            return None;
        }
        let (LineOrigin::Doc(start), LineOrigin::Doc(end)) =
            (self.origin(span.start.line), self.origin(span.end.line))
        else {
            return None;
        };
        let mut out = span;
        out.start.line = start;
        out.end.line = end;
        Some(out)
    }

    /// `span` of the joined program as a span of a prelude file: the file's
    /// index and the span on its own lines.
    pub fn prelude_span(&self, span: SourceSpan) -> Option<(usize, SourceSpan)> {
        if span.file != ENTRY_FILE {
            return None;
        }
        let LineOrigin::Prelude(file, start) = self.origin(span.start.line) else {
            return None;
        };
        let end = match self.origin(span.end.line) {
            LineOrigin::Prelude(f, end) if f == file => end,
            _ => start,
        };
        let mut out = span;
        out.start.line = start;
        out.end.line = end;
        Some((file, out))
    }
}

impl Joined {
    /// Whether the document was joined with anything (there are prelude
    /// files), so that positions need moving back.
    pub fn is_joined(&self) -> bool {
        !self.origins.is_empty()
    }

    /// Move the findings of the joined program back onto the document, as
    /// `petal check` reports them: each one in the document gets its own
    /// line (every span it cites moves with it), one in a prelude file is
    /// dropped (checking that file reports it), and one in an imported
    /// module is kept. The program's entry source becomes `doc_source`, so
    /// that a report's snippets quote the document.
    ///
    /// For a report only: the program's term spans still index the joined
    /// text, so it must not be run or edited afterwards.
    pub fn relocate_program(&self, program: &mut crate::program::Program, doc_source: &str) {
        if !self.is_joined() {
            return;
        }
        let warnings = std::mem::take(&mut program.warnings);
        program.warnings = warnings
            .into_iter()
            .filter_map(|w| {
                if w.span.file != ENTRY_FILE {
                    return Some(w);
                }
                self.doc_span(w.span)?;
                w.relocated(|s| self.doc_span(s).or(Some(s)))
            })
            .collect();
        program.source = doc_source.to_string();
        if let Some(entry) = program.source_map.files.get_mut(ENTRY_FILE.0 as usize) {
            entry.source = doc_source.to_string();
        }
    }

    /// Move a failed compile of the joined program back onto the files it
    /// came from: an item in the document gets its own line, and one in a
    /// prelude file is named by that file (as the manifest writes it) with
    /// its line there.
    pub fn relocate_load_error(&self, err: &mut crate::error::LoadError, host: &HostView) {
        if !self.is_joined() {
            return;
        }
        for item in &mut err.items {
            let (Some(span), None) = (item.span, &item.file) else {
                continue;
            };
            if let Some(doc) = self.doc_span(span) {
                item.span = Some(doc);
            } else if let Some((index, local)) = self.prelude_span(span)
                && let Some(file) = host.prelude.get(index)
            {
                item.span = Some(local);
                item.file = Some(file.name.clone());
            }
        }
    }
}

/// `line`, an import line of the prelude file at `prelude`, with a relative
/// module path rewritten to reach the same file from `doc_dir`. Any other
/// line is returned as it is.
fn rebase_import<'a>(line: &'a str, prelude: &Path, doc_dir: &Path) -> Cow<'a, str> {
    let lead = line.len() - line.trim_start().len();
    let mut at = lead;
    for kw in ["pub ", "export "] {
        if line[at..].starts_with(kw) {
            at += kw.len();
        }
    }
    let Some(rest) = line[at..].strip_prefix("import ") else {
        return Cow::Borrowed(line);
    };
    at += "import ".len() + (rest.len() - rest.trim_start().len());
    let len = line[at..]
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '/' || c == '.'))
        .unwrap_or(line.len() - at);
    let written = &line[at..at + len];
    if !crate::module::is_relative_import(written) {
        return Cow::Borrowed(line);
    }
    let Some(target) = prelude
        .parent()
        .and_then(|dir| crate::module::normalize_lexical(&dir.join(written)))
    else {
        return Cow::Borrowed(line);
    };
    let name = crate::module::relative_module_name(doc_dir, &target);
    let rebased = if name.starts_with("../") {
        name
    } else {
        format!("./{name}")
    };
    Cow::Owned(format!("{}{}{}", &line[..at], rebased, &line[at + len..]))
}

/// The 0-based indices of the lines of `text`'s leading import statements:
/// the run of `import` lines (with the continuation lines of a wrapped name
/// list) that may be interleaved with comments and blank lines, as
/// [`super::document`]'s `import_header` reads it.
pub(crate) fn header_lines(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut continues = false;
    for (i, line) in text.split('\n').enumerate() {
        let code = line.split("//").next().unwrap_or("").trim();
        let is_import = ["import ", "pub import ", "export import "]
            .iter()
            .any(|kw| code.starts_with(kw));
        if !(is_import || continues || code.is_empty()) {
            break;
        }
        if !code.is_empty() {
            out.push(i);
        }
        continues = code.ends_with(',') || code.ends_with(':');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(files: &[(&str, &str)]) -> HostView {
        HostView {
            prelude: files
                .iter()
                .map(|(name, text)| PreludeFile {
                    name: name.to_string(),
                    path: PathBuf::from(name),
                    text: text.to_string(),
                })
                .collect(),
            ..HostView::default()
        }
    }

    #[test]
    fn joining_hoists_imports_and_keeps_every_line_mapped() {
        let host = view(&[
            ("a.ptl", "import noise\nfn a() 1 end"),
            ("b.ptl", "fn b() a() end\n"),
        ]);
        let doc = "import color\n\nprint(b())";
        let joined = host.join(doc);
        let lines: Vec<&str> = joined.text.split('\n').collect();
        assert_eq!(lines[0], "import noise");
        assert_eq!(lines[1], "import color");
        assert_eq!(joined.origin(1), LineOrigin::Prelude(0, 1));
        assert_eq!(joined.origin(2), LineOrigin::Doc(1));
        let print = lines.iter().position(|l| *l == "print(b())").unwrap() as u32 + 1;
        assert_eq!(joined.origin(print), LineOrigin::Doc(3));
        let b = lines.iter().position(|l| l.starts_with("fn b")).unwrap() as u32 + 1;
        assert_eq!(joined.origin(b), LineOrigin::Prelude(1, 1));
        // With no prelude files the document is compiled as it is.
        let alone = HostView::default().join(doc);
        assert_eq!(alone.text, doc);
        assert_eq!(alone.origin(3), LineOrigin::Doc(3));
    }

    #[test]
    fn a_prelude_files_relative_imports_are_rebased_on_the_document() {
        let prelude = Path::new("/p/lib/more.ptl");
        let rebase =
            |line: &str, doc: &str| rebase_import(line, prelude, Path::new(doc)).into_owned();
        assert_eq!(rebase("import ./util", "/p/screens"), "import ../lib/util");
        assert_eq!(
            rebase("pub import ../x/y as z", "/p/screens"),
            "pub import ../x/y as z"
        );
        assert_eq!(rebase("import ../x/y", "/p"), "import ./x/y");
        assert_eq!(rebase("import ./util", "/p/lib"), "import ./util");
        assert_eq!(rebase("import ./a: f, g", "/p"), "import ./lib/a: f, g");
        // Package imports are left alone.
        assert_eq!(rebase("import color", "/p/screens"), "import color");
        // The same file joined into a document in another directory.
        let host = HostView {
            doc_dir: Some(PathBuf::from("/p/screens")),
            ..view(&[("/p/lib/more.ptl", "import ./util\nfn f() 1 end")])
        };
        let joined = host.join("print(f())");
        assert_eq!(joined.text.lines().next(), Some("import ../lib/util"));
    }
}
