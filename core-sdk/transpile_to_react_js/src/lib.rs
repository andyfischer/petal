//! Transpile a Petal program into React.js source.
//!
//! The program is read, not run: the output is source an engineer can keep,
//! with the shape the Petal program had.
//!
//! - A function that returns an element is a function component
//!   (`fn workout_card(w)` → `function WorkoutCard({ w })`), and a call of
//!   it is JSX (`<WorkoutCard w={w} />`).
//! - `state x = init` is `const [x, setX] = useState(init)`. A function
//!   that keeps state but returns a value is a custom hook (`useX`). A
//!   top-level `state var` is a shared cell, read with `useCell`.
//! - `map`/`for` over a list builds keyed children (the loop's `key`, else
//!   the index, which is how Petal keys unkeyed state too).
//! - Each dialect (`dialect.rs`) says what the elements mean: a Mixit
//!   design's `<vstack>`, `<instance of=…>` and `component(...)`; a
//!   petal-web-html page's HTML and `if clicked(id) then …` click polls,
//!   which become `onClick` handlers that call the state setters.
//!
//! Whatever has no React form is converted as far as it goes and reported as
//! a [`Warning`], which also appears as a `TODO(petal-to-react)` comment in
//! the output.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

mod analysis;
pub mod dialect;
mod emit;
mod expr;
mod jsx;
pub mod names;
pub mod sigs;

pub use analysis::{FnInfo, FnKind};
pub use dialect::Dialect;

/// The Petal builtins, as JS.
pub const RUNTIME: &str = include_str!("../runtime/petal-runtime.js");
/// The React helpers (`useCell`).
pub const REACT_RUNTIME: &str = include_str!("../runtime/petal-react.js");
/// The design vocabulary as React components.
pub const DESIGN_RUNTIME: &str = include_str!("../runtime/petal-design.jsx");

#[derive(Debug, Clone)]
pub struct Options {
    /// The dialect; detected from the imports when `None`.
    pub dialect: Option<Dialect>,
    /// The file's name, for messages and the default component's name.
    pub file_name: String,
    /// Where the runtime files are imported from, ending in `/`.
    pub runtime_dir: String,
    /// The default export's name (default: the file name in PascalCase).
    pub component_name: Option<String>,
    /// Where tags the dialect does not know are imported from.
    pub elements_module: String,
    /// The modules this one imports with a relative path, by the path as
    /// written (`./tokens`): what each exports. [`transpile_project`] fills
    /// it in.
    pub modules: HashMap<String, ModuleInterface>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            dialect: None,
            file_name: "main.ptl".into(),
            runtime_dir: "./".into(),
            component_name: None,
            elements_module: "./elements.jsx".into(),
            modules: HashMap::new(),
        }
    }
}

/// What a transpiled module exports, for the modules that import it.
#[derive(Debug, Clone, Default)]
pub struct ModuleInterface {
    /// The JS import path (`./tokens.jsx`).
    pub path: String,
    pub fns: HashMap<String, FnInfo>,
    /// Petal name → JS name.
    pub values: HashMap<String, String>,
    /// Design component name → React component.
    pub design_components: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    pub line: u32,
    pub column: u32,
    /// A short kind: `state-in-loop`, `unknown-name`, `immediate-mode`…
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: warning[{}]: {}", self.line, self.column, self.code, self.message)
    }
}

#[derive(Debug, Clone)]
pub struct Output {
    /// The JS module (JSX).
    pub code: String,
    pub warnings: Vec<Warning>,
    pub dialect: Dialect,
    /// The runtime files `code` imports: (file name, contents).
    pub runtime: Vec<(&'static str, &'static str)>,
    /// Tags imported from the elements module, for a stub file.
    pub elements: Vec<String>,
    pub interface: ModuleInterface,
    /// Frames lifted into components: (frame id, component).
    pub frames: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Parse Petal source.
pub fn parse(source: &str) -> Result<Vec<petal::ast::Stmt>, Error> {
    let mut lexer = petal::lexer::Lexer::new(source);
    lexer.tokenize().map_err(Error)?;
    let mut parser = petal::parse::Parser::from_lexer(&lexer);
    parser.parse_program().map_err(Error)
}

/// Transpile one module.
pub fn transpile(source: &str, opts: &Options) -> Result<Output, Error> {
    let stmts = parse(source)?;
    let dialect = opts.dialect.unwrap_or_else(|| Dialect::detect(&stmts));
    let mut e = emit::Emitter::new(source, opts, dialect);
    let code = e.module(&stmts);
    let mut runtime = Vec::new();
    if !e.used_runtime.is_empty() {
        runtime.push(("petal-runtime.js", RUNTIME));
    }
    if !e.used_react_rt.is_empty() {
        runtime.push(("petal-react.js", REACT_RUNTIME));
        runtime.push(("petal-runtime.js", RUNTIME));
    }
    if !e.used_dialect.is_empty() {
        if let Some(f) = dialect::runtime_file(dialect) {
            runtime.push(f);
            runtime.push(("petal-runtime.js", RUNTIME));
            runtime.push(("petal-react.js", REACT_RUNTIME));
        }
    }
    runtime.sort();
    runtime.dedup();
    let mut interface = std::mem::take(&mut e.exports);
    interface.design_components = e.design_components.iter().filter(|(_, js)| interface.fns.values().any(|f| &f.js == *js)).map(|(a, b)| (a.clone(), b.clone())).collect();
    let mut warnings = e.warnings;
    warnings.sort_by_key(|w| (w.line, w.column));
    Ok(Output {
        code,
        warnings,
        dialect,
        runtime,
        elements: e.used_elements.into_iter().collect(),
        interface,
        frames: e.lifted_frames,
    })
}

/// One transpiled file of a project.
#[derive(Debug, Clone)]
pub struct ProjectFile {
    /// The Petal source path.
    pub source: PathBuf,
    /// The output path, relative to the output folder (`tokens.jsx`).
    pub out: PathBuf,
    pub output: Output,
}

/// Transpile an entry file and every module it reaches by a relative
/// import, dependencies first, each to its own `.jsx` beside the others.
/// An entry that is a folder means its `main.ptl` or the one `.ptl` that
/// calls `design(`.
pub fn transpile_project(entry: &Path, opts: &Options) -> Result<Vec<ProjectFile>, Error> {
    let entry = resolve_entry(entry)?;
    let root = entry.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut done: HashMap<PathBuf, ModuleInterface> = HashMap::new();
    let mut out = Vec::new();
    visit(&entry, &root, opts, &mut done, &mut out, &mut Vec::new())?;
    Ok(out)
}

fn resolve_entry(entry: &Path) -> Result<PathBuf, Error> {
    if !entry.is_dir() {
        return Ok(entry.to_path_buf());
    }
    let main = entry.join("main.ptl");
    if main.exists() {
        return Ok(main);
    }
    let mut found = Vec::new();
    for f in std::fs::read_dir(entry).map_err(|e| Error(e.to_string()))?.flatten() {
        let p = f.path();
        if p.extension().is_some_and(|x| x == "ptl") {
            let src = std::fs::read_to_string(&p).unwrap_or_default();
            if src.contains("design(") {
                found.push(p);
            }
        }
    }
    found.sort();
    found.into_iter().next().ok_or_else(|| Error(format!("{}: no main.ptl, and no .ptl that calls design(", entry.display())))
}

fn visit(
    file: &Path,
    root: &Path,
    opts: &Options,
    done: &mut HashMap<PathBuf, ModuleInterface>,
    out: &mut Vec<ProjectFile>,
    stack: &mut Vec<PathBuf>,
) -> Result<ModuleInterface, Error> {
    let canon = file.canonicalize().map_err(|e| Error(format!("{}: {e}", file.display())))?;
    if let Some(i) = done.get(&canon) {
        return Ok(i.clone());
    }
    if stack.contains(&canon) {
        return Err(Error(format!("{}: import cycle", file.display())));
    }
    stack.push(canon.clone());
    let src = std::fs::read_to_string(file).map_err(|e| Error(format!("{}: {e}", file.display())))?;
    let stmts = parse(&src).map_err(|e| Error(format!("{}: {e}", file.display())))?;
    let dir = file.parent().unwrap_or(Path::new("."));
    let rel_out = |p: &Path| -> PathBuf {
        let rel = p.strip_prefix(root).unwrap_or(p).with_extension("jsx");
        rel
    };
    let mut modules = HashMap::new();
    for s in &stmts {
        if let petal::ast::StmtKind::Import(d) = &s.kind {
            if d.module.starts_with("./") || d.module.starts_with("../") {
                let mut p = dir.join(&d.module);
                if p.is_dir() {
                    p = p.join(format!("{}.ptl", petal::ast::module_local_name(&d.module)));
                } else {
                    p.set_extension("ptl");
                }
                let mut iface = visit(&p, root, opts, done, out, stack)?;
                // The import path from this file's output to that one's.
                let from = rel_out(file);
                let to = rel_out(&p);
                iface.path = relative_import(&from, &to);
                modules.insert(d.module.clone(), iface);
            }
        }
    }
    let mut o = opts.clone();
    o.file_name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    o.modules = modules;
    let depth = rel_out(file).components().count().saturating_sub(1);
    if depth > 0 && o.runtime_dir == "./" {
        o.runtime_dir = "../".repeat(depth);
    }
    if out.is_empty() && stack.len() > 1 {
        // A dependency of the entry: no default component of its own.
    }
    let output = transpile(&src, &o).map_err(|e| Error(format!("{}: {e}", file.display())))?;
    let iface = output.interface.clone();
    out.push(ProjectFile { source: file.to_path_buf(), out: rel_out(file), output });
    stack.pop();
    done.insert(canon, iface.clone());
    Ok(iface)
}

fn relative_import(from: &Path, to: &Path) -> String {
    let from_dir: Vec<_> = from.parent().map(|p| p.components().collect()).unwrap_or_default();
    let to_parts: Vec<_> = to.components().collect();
    let common = from_dir.iter().zip(&to_parts).take_while(|(a, b)| a == b).count();
    let mut s = String::new();
    for _ in common..from_dir.len() {
        s.push_str("../");
    }
    if s.is_empty() {
        s.push_str("./");
    }
    let rest: Vec<String> = to_parts[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    s.push_str(&rest.join("/"));
    s
}
