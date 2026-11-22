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
/// The design package's data tables, generated from its Petal source.
pub const DESIGN_DATA: &str = include_str!("../runtime/petal-design-data.js");

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
            if dialect == Dialect::Design {
                runtime.push(("petal-design-data.js", DESIGN_DATA));
            }
            runtime.push(("petal-runtime.js", RUNTIME));
            runtime.push(("petal-react.js", REACT_RUNTIME));
        }
    }
    runtime.sort();
    runtime.dedup();
    let mut interface = std::mem::take(&mut e.exports);
    // `pub import m` re-exports what m exports.
    for s in &stmts {
        if let petal::ast::StmtKind::Import(d) = &s.kind {
            if d.exported {
                if let Some(m) = opts.modules.get(&d.module) {
                    for (k, v) in &m.fns {
                        interface.fns.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                    for (k, v) in &m.values {
                        interface.values.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                    for (k, v) in &m.design_components {
                        interface.design_components.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
        }
    }
    let own: HashMap<String, String> = e.design_components.iter().filter(|(_, js)| interface.fns.values().any(|f| &f.js == *js)).map(|(a, b)| (a.clone(), b.clone())).collect();
    interface.design_components = own;
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
///
/// A package import (`import design_ios`) is followed too when the
/// nearest `petal.toml` names it as a path dependency: its modules are
/// written under `packages/<name>/`. The packages a runtime file stands for
/// (`design`, `web`) and Petal's bundled ones are not.
pub fn transpile_project(entry: &Path, opts: &Options) -> Result<Vec<ProjectFile>, Error> {
    let entry = resolve_entry(entry)?;
    // Absolute, so a bare `app.ptl` has a folder (its parent would be "",
    // and every output path relative to it would climb out of the output).
    let entry = entry.canonicalize().unwrap_or(entry);
    let root = entry.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut p = Project { root, opts: opts.clone(), done: HashMap::new(), out: Vec::new(), stack: Vec::new(), packages: Vec::new() };
    p.visit(&entry)?;
    Ok(p.out)
}

/// Packages that are not transpiled: a runtime file stands for them, or
/// they are Petal's own (bundled, or drawn by a host).
const NOT_FOLLOWED: &[&str] = &[
    "design", "web", "ui", "bloom", "color", "vec2", "noise", "json", "io", "core", "std", "glsl",
    "regex", "text_layout", "query", "ptl_dataflow", "garden_config",
];

struct Project {
    root: PathBuf,
    opts: Options,
    done: HashMap<PathBuf, ModuleInterface>,
    out: Vec<ProjectFile>,
    stack: Vec<PathBuf>,
    /// (package folder, package name) of the packages followed.
    packages: Vec<(PathBuf, String)>,
}

impl Project {
    /// Where a module's output goes, relative to the output folder.
    fn rel_out(&self, p: &Path) -> PathBuf {
        let canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let root = self.root.canonicalize().unwrap_or_else(|_| self.root.clone());
        if let Ok(rel) = canon.strip_prefix(&root) {
            return rel.with_extension("jsx");
        }
        for (dir, name) in &self.packages {
            let dir = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if let Ok(rel) = canon.strip_prefix(&dir) {
                return Path::new("packages").join(name).join(rel).with_extension("jsx");
            }
        }
        // Outside the entry's folder: under `external/`, by its path from
        // the entry's folder's parent (`../shared/parts.ptl` →
        // `external/shared/parts.jsx`).
        let parent = root.parent().unwrap_or(&root);
        match canon.strip_prefix(parent) {
            Ok(rel) => Path::new("external").join(rel).with_extension("jsx"),
            Err(_) => Path::new("external").join(p.file_name().unwrap_or_default()).with_extension("jsx"),
        }
    }

    /// The root module of a package import (`design_ios`, `bloom/menu`)
    /// that the nearest petal.toml lists as a path dependency.
    fn package_module(&mut self, from_dir: &Path, module: &str) -> Option<PathBuf> {
        let mut segs = module.split('/');
        let name = segs.next()?;
        if NOT_FOLLOWED.contains(&name) {
            return None;
        }
        let rest: Vec<&str> = segs.collect();
        let dir = find_dependency(from_dir, name)?;
        let file = if rest.is_empty() { format!("{name}.ptl") } else { format!("{}.ptl", rest.join("/")) };
        let path = [dir.join(&file), dir.join("src").join(&file)].into_iter().find(|p| p.exists())?;
        if !self.packages.iter().any(|(d, _)| d == &dir) {
            self.packages.push((dir, name.to_string()));
        }
        Some(path)
    }

    fn visit(&mut self, file: &Path) -> Result<ModuleInterface, Error> {
        self.visit_as(file, None)
    }

    /// `inherit`: the importer's dialect, for a module that does not say its
    /// own (a design's helper module of plain functions and elements).
    fn visit_as(&mut self, file: &Path, inherit: Option<Dialect>) -> Result<ModuleInterface, Error> {
        let canon = file.canonicalize().map_err(|e| Error(format!("{}: {e}", file.display())))?;
        if let Some(i) = self.done.get(&canon) {
            return Ok(i.clone());
        }
        if self.stack.contains(&canon) {
            return Err(Error(format!("{}: import cycle", file.display())));
        }
        self.stack.push(canon.clone());
        let src = std::fs::read_to_string(file).map_err(|e| Error(format!("{}: {e}", file.display())))?;
        let stmts = parse(&src).map_err(|e| Error(format!("{}: {e}", file.display())))?;
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut modules = HashMap::new();
        for s in &stmts {
            let petal::ast::StmtKind::Import(d) = &s.kind else { continue };
            let target = if d.module.starts_with("./") || d.module.starts_with("../") {
                let mut p = dir.join(&d.module);
                if p.is_dir() {
                    p = p.join(format!("{}.ptl", petal::ast::module_local_name(&d.module)));
                } else {
                    p.set_extension("ptl");
                }
                Some(p)
            } else {
                self.package_module(&dir, &d.module)
            };
            let Some(p) = target else { continue };
            let own = self.opts.dialect.unwrap_or_else(|| Dialect::detect(&stmts));
            let mut iface = self.visit_as(&p, Some(own))?;
            iface.path = relative_import(&self.rel_out(file), &self.rel_out(&p));
            modules.insert(d.module.clone(), iface);
        }
        let mut o = self.opts.clone();
        if o.dialect.is_none() && Dialect::detect(&stmts) == Dialect::Generic {
            o.dialect = inherit.filter(|d| *d == Dialect::Design || *d == Dialect::Web);
        }
        o.file_name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        o.modules = modules;
        let rel = self.rel_out(file);
        let depth = rel.components().count().saturating_sub(1);
        if depth > 0 && o.runtime_dir == "./" {
            o.runtime_dir = "../".repeat(depth);
        }
        let output = transpile(&src, &o).map_err(|e| Error(format!("{}: {e}", file.display())))?;
        let iface = output.interface.clone();
        self.out.push(ProjectFile { source: file.to_path_buf(), out: rel, output });
        self.stack.pop();
        self.done.insert(canon, iface.clone());
        Ok(iface)
    }
}

/// The folder of a path dependency `name` in the nearest petal.toml above
/// `dir`.
fn find_dependency(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut d = Some(dir);
    while let Some(cur) = d {
        let manifest = cur.join("petal.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            let mut in_deps = false;
            for line in text.lines() {
                let t = line.trim();
                if t.starts_with('[') {
                    in_deps = t == "[dependencies]";
                    continue;
                }
                if !in_deps {
                    continue;
                }
                let Some((k, v)) = t.split_once('=') else { continue };
                if k.trim().trim_matches('"') != name {
                    continue;
                }
                let v = v.trim();
                if let Some(i) = v.find("path") {
                    let after = &v[i + 4..];
                    let q: Vec<&str> = after.split('"').collect();
                    if q.len() >= 2 {
                        return Some(cur.join(q[1]));
                    }
                }
            }
            // The nearest manifest decides, as Petal's loader does.
            return None;
        }
        d = cur.parent();
    }
    None
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
