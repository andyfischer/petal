//! The emitter: one Petal module in, one JS module out.
//!
//! Statements are written here; expressions and calls in `expr.rs`;
//! elements in `jsx.rs`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use petal::ast::{
    AssignTarget, ElseBranch, Expr, ExprKind, Literal, MatchArm, Param, Pattern, Stmt, StmtKind,
};
use petal::source_map::SourceSpan;

use crate::analysis::{self, FnInfo, FnKind};
use crate::dialect::Dialect;
use crate::names::{self, Names, js_string, pascal};
use crate::sigs::JsSig;
use crate::{ModuleInterface, Options, Warning};

/// A writer with an indentation level. A multi-line string written as one
/// line keeps its own relative indentation under the current one.
#[derive(Default)]
pub struct W {
    pub buf: String,
    pub ind: usize,
}

impl W {
    pub fn line(&mut self, s: impl AsRef<str>) {
        for l in s.as_ref().split('\n') {
            if l.trim().is_empty() {
                self.buf.push('\n');
            } else {
                for _ in 0..self.ind {
                    self.buf.push_str("  ");
                }
                self.buf.push_str(l);
                self.buf.push('\n');
            }
        }
    }
    pub fn open(&mut self, s: impl AsRef<str>) {
        self.line(s);
        self.ind += 1;
    }
    pub fn close(&mut self, s: impl AsRef<str>) {
        self.ind = self.ind.saturating_sub(1);
        self.line(s);
    }
    pub fn append(&mut self, other: W) {
        self.buf.push_str(&other.buf);
    }
}

/// Indent every line after the first by two spaces.
pub fn indent_rest(s: &str) -> String {
    s.replace('\n', "\n  ")
}

pub fn indent_all(s: &str) -> String {
    format!("  {}", indent_rest(s))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Module,
    App,
    Component,
    Hook,
    Plain,
    Lambda,
    /// A click handler made from `if clicked(id) then …`.
    Handler,
}

impl Kind {
    /// Whether hooks may be called directly in this scope.
    pub fn hooks(self) -> bool {
        matches!(self, Kind::App | Kind::Component | Kind::Hook)
    }
}

#[derive(Debug, Clone)]
pub struct StateSlot {
    pub value: String,
    pub setter: String,
}

#[derive(Default)]
pub struct Scope {
    pub kind: Option<Kind>,
    pub locals: HashSet<String>,
    pub states: HashMap<String, StateSlot>,
    /// Names written through a temporary in this scope (a handler's copy of
    /// a state value, a function's copy of a captured name).
    pub temps: HashMap<String, String>,
    /// `useCell` values of top-level `state var`s read while rendering.
    pub cell_vals: HashMap<String, String>,
    /// Loops and branches entered inside this function: hooks are not
    /// allowed there.
    pub nested: usize,
    /// A design component's raw props.
    pub props_js: Option<String>,
    /// Statements written somewhere else (a click poll folded into its
    /// handler), by span start.
    pub consumed: HashSet<(u32, u32)>,
    /// `let id = next_id()` → the handler written in its place.
    pub handlers: HashMap<(u32, u32), String>,
    /// Ids whose clicks are polled outside the function's top level: they
    /// become `petal_event()`s.
    pub events: HashSet<String>,
    /// State names this function writes while rendering.
    pub render_writes: HashSet<String>,
    /// Names the function rebinds, or declares twice: written `let`.
    pub mutable: HashSet<String>,
    /// The names declared in each open block, innermost last.
    pub blocks: Vec<HashSet<String>>,
    /// The last top-level `state` of the function, and the handler lines
    /// held back to be written after it (React order: state, then
    /// handlers).
    pub last_state: Option<(u32, u32)>,
    pub deferred: Vec<String>,
}

impl Scope {
    fn new(kind: Kind) -> Scope {
        Scope { kind: Some(kind), ..Default::default() }
    }
    pub fn kind(&self) -> Kind {
        self.kind.unwrap_or(Kind::Module)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tail<'a> {
    /// The value is discarded.
    None,
    /// The value is returned, wrapped by a call when given
    /// (`instance_root(props, …)`).
    Return(Option<&'a str>),
}

pub struct Emitter<'a> {
    pub src: &'a str,
    pub opts: &'a Options,
    pub dialect: Dialect,
    pub names: Names,
    pub fns: HashMap<String, FnInfo>,
    pub runtime: HashMap<String, JsSig>,
    pub react_rt: HashMap<String, JsSig>,
    pub dialect_rt: HashMap<String, JsSig>,
    pub used_react: BTreeSet<String>,
    pub used_runtime: BTreeSet<String>,
    pub used_react_rt: BTreeSet<String>,
    pub used_dialect: BTreeSet<String>,
    pub used_elements: BTreeSet<String>,
    /// JS import path → (exported name, local name).
    pub module_imports: BTreeMap<String, BTreeSet<String>>,
    /// Bare names an imported module brings: Petal name → (path, JS name).
    pub imported: HashMap<String, (String, String)>,
    pub imported_fns: HashMap<String, FnInfo>,
    /// Qualifiers: `import ./theme` → `theme` → its interface.
    pub aliases: HashMap<String, String>,
    pub unknown_packages: Vec<String>,
    pub warnings: Vec<Warning>,
    pub notes: Vec<String>,
    pub stubs: BTreeMap<String, String>,
    /// Top-level `state var` → its cell's JS name.
    pub cells: HashMap<String, String>,
    pub module_names: HashSet<String>,
    pub enums: HashMap<String, Vec<(String, Vec<String>)>>,
    pub variant_fields: HashMap<String, Vec<String>>,
    pub classes: HashSet<String>,
    pub methods: HashMap<String, Vec<String>>,
    pub scopes: Vec<Scope>,
    /// Expressions written as given text instead (a component's `props:`
    /// record, moved to `Card.defaults`), by span.
    pub replacements: HashMap<(u32, u32), String>,
    /// Code written before the current top-level statement (lifted frames).
    pub hoisted: Vec<String>,
    pub uses_maps: bool,
    pub warned: HashSet<String>,
    /// Design component name → its React component.
    pub design_components: HashMap<String, String>,
    pub exports: ModuleInterface,
    pub comment_lines: HashSet<usize>,
    pub lines: Vec<&'a str>,
    /// Components lifted out of a module-level expression (frames).
    pub lifted_frames: Vec<(String, String)>,
}

impl<'a> Emitter<'a> {
    pub fn new(src: &'a str, opts: &'a Options, dialect: Dialect) -> Self {
        Emitter {
            src,
            opts,
            dialect,
            names: Names::default(),
            fns: HashMap::new(),
            runtime: crate::sigs::exports(crate::RUNTIME),
            react_rt: crate::sigs::exports(crate::REACT_RUNTIME),
            dialect_rt: match crate::dialect::runtime_file(dialect) {
                Some((_, src)) => crate::sigs::exports(src),
                None => HashMap::new(),
            },
            used_react: BTreeSet::new(),
            used_runtime: BTreeSet::new(),
            used_react_rt: BTreeSet::new(),
            used_dialect: BTreeSet::new(),
            used_elements: BTreeSet::new(),
            module_imports: BTreeMap::new(),
            imported: HashMap::new(),
            imported_fns: HashMap::new(),
            aliases: HashMap::new(),
            unknown_packages: Vec::new(),
            warnings: Vec::new(),
            notes: Vec::new(),
            stubs: BTreeMap::new(),
            cells: HashMap::new(),
            module_names: HashSet::new(),
            enums: HashMap::new(),
            variant_fields: HashMap::new(),
            classes: HashSet::new(),
            methods: HashMap::new(),
            scopes: vec![Scope::new(Kind::Module)],
            replacements: HashMap::new(),
            hoisted: Vec::new(),
            uses_maps: src.contains("Map("),
            warned: HashSet::new(),
            design_components: HashMap::new(),
            exports: ModuleInterface::default(),
            comment_lines: HashSet::new(),
            lines: src.lines().collect(),
            lifted_frames: Vec::new(),
        }
    }

    // ── diagnostics ─────────────────────────────────────────────────────────

    /// Report something that has no exact React form. The message also goes
    /// into the output, as a TODO comment above the statement.
    pub fn warn(&mut self, span: SourceSpan, code: &'static str, msg: impl Into<String>) {
        let msg = msg.into();
        self.notes.push(format!("// TODO(petal-to-react) [{code}]: {msg}"));
        self.warnings.push(Warning { line: span.start.line, column: span.start.column, code, message: msg });
    }

    /// Like `warn`, but once per key.
    pub fn warn_once(&mut self, key: &str, span: SourceSpan, code: &'static str, msg: impl Into<String>) {
        if self.warned.insert(key.to_string()) {
            self.warn(span, code, msg);
        }
    }

    pub fn span_text(&self, span: SourceSpan) -> &'a str {
        let (s, e) = (span.start.offset as usize, span.end.offset as usize);
        self.src.get(s..e).unwrap_or("")
    }

    /// The `#` comment lines directly above a line, as `//` comments.
    pub fn comments_before(&mut self, line: u32) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = line as usize;
        while i >= 2 {
            let l = self.lines.get(i - 2).map(|l| l.trim()).unwrap_or("");
            if l.starts_with('#') && !l.starts_with("#!") && !self.comment_lines.contains(&(i - 2)) {
                out.push((i - 2, format!("//{}", &l[1..])));
                i -= 1;
            } else {
                break;
            }
        }
        out.reverse();
        out.into_iter()
            .map(|(n, l)| {
                self.comment_lines.insert(n);
                l
            })
            .collect()
    }

    // ── scopes ──────────────────────────────────────────────────────────────

    pub fn scope(&mut self) -> &mut Scope {
        self.scopes.last_mut().unwrap()
    }

    pub fn push_scope(&mut self, kind: Kind) {
        self.scopes.push(Scope::new(kind));
    }

    pub fn pop_scope(&mut self) -> Scope {
        self.scopes.pop().unwrap()
    }

    /// The innermost function scope (not a lambda or handler).
    pub fn fn_scope_kind(&self) -> Kind {
        for s in self.scopes.iter().rev() {
            match s.kind() {
                Kind::Lambda | Kind::Handler => continue,
                k => return k,
            }
        }
        Kind::Module
    }

    /// Whether the code being written runs while rendering (not in a
    /// handler or a callback).
    pub fn in_render(&self) -> bool {
        self.scopes.last().is_some_and(|s| s.kind().hooks())
    }

    pub fn in_handler(&self) -> bool {
        self.scopes.iter().any(|s| s.kind() == Kind::Handler)
    }

    pub fn declare(&mut self, name: &str) -> String {
        self.scope().locals.insert(name.to_string());
        self.names.local(name)
    }

    /// Whether `name` is bound in a function scope (a parameter or local).
    pub fn is_local(&self, name: &str) -> bool {
        self.scopes.iter().skip(1).any(|s| s.locals.contains(name) || s.states.contains_key(name) || s.temps.contains_key(name))
    }

    pub fn state_slot(&self, name: &str) -> Option<StateSlot> {
        for s in self.scopes.iter().rev() {
            if s.temps.contains_key(name) || (s.locals.contains(name) && !s.states.contains_key(name)) {
                return None;
            }
            if let Some(slot) = s.states.get(name) {
                return Some(slot.clone());
            }
        }
        None
    }

    // ── imports ─────────────────────────────────────────────────────────────

    pub fn react(&mut self, name: &str) -> String {
        self.used_react.insert(name.to_string());
        name.to_string()
    }

    pub fn rt(&mut self, name: &str) -> String {
        if self.runtime.contains_key(name) {
            self.used_runtime.insert(name.to_string());
        } else if self.react_rt.contains_key(name) {
            self.used_react_rt.insert(name.to_string());
        } else if self.dialect_rt.contains_key(name) {
            self.used_dialect.insert(name.to_string());
        }
        name.to_string()
    }

    pub fn design_rt(&mut self, name: &str) -> String {
        self.used_dialect.insert(name.to_string());
        name.to_string()
    }

    // ── the module ──────────────────────────────────────────────────────────

    pub fn module(&mut self, stmts: &[Stmt]) -> String {
        self.collect(stmts);
        let output = self.output_index(stmts);
        let app = self.partition(stmts, output);
        let has_app = app.iter().any(|a| *a);

        let mut body = W::default();
        for (i, s) in stmts.iter().enumerate() {
            if app[i] || Some(i) == output {
                continue;
            }
            self.top_stmt(&mut body, s);
        }

        let default_name = self.default_name();
        let mut tail = W::default();
        if has_app {
            let app_stmts: Vec<&Stmt> = stmts.iter().enumerate().filter(|(i, _)| app[*i]).map(|(_, s)| s).collect();
            self.app(&mut tail, &default_name, &app_stmts, output.map(|i| &stmts[i]));
        } else if let Some(i) = output {
            self.default_export(&mut tail, &default_name, &stmts[i]);
        }

        let mut out = String::new();
        out.push_str(&format!(
            "// Generated from {} by transpile_to_react_js ({} dialect).\n// Regenerate after changing the Petal source, or take this file over by hand.\n",
            self.opts.file_name,
            self.dialect.name()
        ));
        out.push_str(&self.import_lines());
        out.push('\n');
        out.push_str(&body.buf);
        if !tail.buf.is_empty() {
            out.push('\n');
            out.push_str(&tail.buf);
        }
        if !self.stubs.is_empty() {
            out.push_str("\n// Not converted: calling one of these throws, naming it. See the transpile warnings.\n");
            let stubs: Vec<(String, String)> = self.stubs.iter().map(|(a, b)| (a.clone(), b.clone())).collect();
            for (petal, js) in stubs {
                let u = self.rt("unconverted");
                out.push_str(&format!("const {js} = {u}({});\n", js_string(&petal)));
            }
            // The stub lines use `unconverted`, so the import list is
            // rebuilt to include it.
            let imports = self.import_lines();
            let header_end = out.find("\n\n").unwrap_or(0);
            let header = out[..header_end + 1].to_string();
            let first = header.lines().take(2).collect::<Vec<_>>().join("\n");
            let rest_start = out.find("\n\n").map(|i| i + 2).unwrap_or(0);
            out = format!("{first}\n{imports}\n{}", &out[rest_start..]);
        }
        while out.contains("\n\n\n") {
            out = out.replace("\n\n\n", "\n\n");
        }
        out
    }

    fn default_name(&mut self) -> String {
        let base = self.opts.component_name.clone().unwrap_or_else(|| {
            let stem = std::path::Path::new(&self.opts.file_name)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "App".into());
            let p = pascal(&stem);
            if self.dialect == Dialect::Design { format!("{p}Design") } else { p }
        });
        self.names.fresh(&base)
    }

    fn import_lines(&self) -> String {
        let mut out = String::new();
        if !self.used_react.is_empty() {
            out.push_str(&format!(
                "import {{ {} }} from \"react\";\n",
                self.used_react.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        let dir = &self.opts.runtime_dir;
        if !self.used_runtime.is_empty() {
            out.push_str(&format!(
                "import {{ {} }} from \"{dir}petal-runtime.js\";\n",
                self.used_runtime.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if !self.used_react_rt.is_empty() {
            out.push_str(&format!(
                "import {{ {} }} from \"{dir}petal-react.js\";\n",
                self.used_react_rt.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if !self.used_dialect.is_empty() {
            if let Some((file, _)) = crate::dialect::runtime_file(self.dialect) {
                out.push_str(&format!(
                    "import {{ {} }} from \"{dir}{file}\";\n",
                    self.used_dialect.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        for (path, names) in &self.module_imports {
            out.push_str(&format!("import {{ {} }} from \"{path}\";\n", names.iter().cloned().collect::<Vec<_>>().join(", ")));
        }
        if !self.used_elements.is_empty() {
            out.push_str(&format!(
                "import {{ {} }} from \"{}\";\n",
                self.used_elements.iter().cloned().collect::<Vec<_>>().join(", "),
                self.opts.elements_module
            ));
        }
        out
    }

    /// Learn the module's declarations before writing any of it.
    fn collect(&mut self, stmts: &[Stmt]) {
        // The names the imports and runtimes bring are taken first, so a
        // component of the document never shadows one.
        for s in stmts {
            if let StmtKind::Import(d) = &s.kind {
                self.import(d, s.span);
            }
        }
        let mut reserved: Vec<String> = self.dialect_rt.keys().filter(|k| k.starts_with(|c: char| c.is_ascii_uppercase())).cloned().collect();
        reserved.extend(["useState", "useMemo", "useEffect"].map(String::from));
        for r in reserved {
            self.names.reserve(&r);
        }
        let design = if self.dialect == Dialect::Design { analysis::design_components(stmts) } else { HashMap::new() };
        let mut elem_fns: HashSet<String> = HashSet::new();
        let mut decls: Vec<(&String, &Vec<Param>, &Vec<Stmt>, SourceSpan)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for s in stmts {
            match &s.kind {
                StmtKind::FnDecl { name, params, body, class: None, .. } => {
                    if !seen.insert(name.clone()) {
                        self.warn(s.span, "overload", format!("`{name}` is defined more than once (overloaded by arity); JS has one function per name, so only the first definition is kept"));
                        continue;
                    }
                    decls.push((name, params, body, s.span));
                }
                StmtKind::FnDecl { name, class: Some(c), .. } => {
                    let m = name.rsplit('.').next().unwrap_or(name).to_string();
                    self.methods.entry(m).or_default().push(c.clone());
                }
                StmtKind::Let { name, .. } | StmtKind::State { name, .. } | StmtKind::Memo { name, .. } => {
                    self.module_names.insert(name.clone());
                }
                StmtKind::EnumDecl { name, variants } => {
                    self.module_names.insert(name.clone());
                    let vs: Vec<(String, Vec<String>)> = variants.iter().map(|v| (v.name.clone(), v.fields.clone())).collect();
                    for (v, f) in &vs {
                        self.variant_fields.insert(v.clone(), f.clone());
                    }
                    self.enums.insert(name.clone(), vs);
                }
                StmtKind::ClassDecl { name, .. } => {
                    self.module_names.insert(name.clone());
                    self.classes.insert(name.clone());
                }
                _ => {}
            }
            if let StmtKind::State { name, is_var: true, .. } = &s.kind {
                let js = self.names.fresh(&format!("{}Cell", names::camel(name)));
                self.cells.insert(name.clone(), js);
            }
        }
        for (name, ..) in &decls {
            self.module_names.insert((*name).clone());
        }
        // Components: what returns an element, to a fixpoint (a function
        // that returns another's element is one too).
        for (name, ..) in &decls {
            if design.contains_key(*name) {
                elem_fns.insert((*name).clone());
            }
        }
        let imported_elem: HashSet<String> =
            self.imported_fns.iter().filter(|(_, f)| f.kind == FnKind::Component).map(|(n, _)| n.clone()).collect();
        elem_fns.extend(imported_elem);
        loop {
            let before = elem_fns.len();
            for (name, params, body, _) in &decls {
                if elem_fns.contains(*name) {
                    continue;
                }
                let params: HashSet<String> = HashSet::new();
                let _ = &params;
                if analysis::block_is_element(body, &elem_fns, &HashSet::new()) {
                    elem_fns.insert((*name).clone());
                }
            }
            let _ = &decls.iter().map(|d| d.1).count();
            if elem_fns.len() == before {
                break;
            }
        }
        // Hooks: what keeps state and is not a component, to a fixpoint
        // (a function that calls a hook is one).
        let mut stateful: HashSet<String> = decls.iter().filter(|d| analysis::declares_state(d.2)).map(|d| d.0.clone()).collect();
        stateful.extend(self.imported_fns.iter().filter(|(_, f)| f.kind == FnKind::Hook).map(|(n, _)| n.clone()));
        loop {
            let before = stateful.len();
            for (name, _, body, _) in &decls {
                if stateful.contains(*name) || elem_fns.contains(*name) {
                    continue;
                }
                if analysis::called_names(body).iter().any(|c| stateful.contains(c) && !elem_fns.contains(c)) {
                    stateful.insert((*name).clone());
                }
            }
            if stateful.len() == before {
                break;
            }
        }
        // Names: components and hooks take theirs first.
        for (name, params, _, _) in &decls {
            let kind = if elem_fns.contains(*name) {
                FnKind::Component
            } else if stateful.contains(*name) {
                FnKind::Hook
            } else {
                FnKind::Plain
            };
            let design_component = design.get(*name).cloned();
            let js = match kind {
                FnKind::Component => {
                    let base = pascal(design_component.as_deref().unwrap_or(name));
                    let base = if self.names.is_taken(&base) { format!("{base}View") } else { base };
                    self.names.fresh(&base)
                }
                FnKind::Hook => self.names.fresh(&format!("use{}", pascal(name))),
                FnKind::Plain => self.names.local(name),
            };
            if let Some(c) = &design_component {
                self.design_components.insert(c.clone(), js.clone());
            }
            self.fns.insert(
                (*name).clone(),
                FnInfo { js, params: params.iter().map(|p| p.name.clone()).collect(), kind, design_component },
            );
        }
    }

    fn import(&mut self, d: &petal::ast::ImportDecl, span: SourceSpan) {
        let m = d.module.as_str();
        let relative = m.starts_with("./") || m.starts_with("../");
        let qualifier = d.alias.clone().unwrap_or_else(|| petal::ast::module_local_name(m).to_string());
        if relative {
            let iface = self.opts.modules.get(m).cloned();
            let Some(iface) = iface else {
                self.warn(span, "import", format!("`import {m}`: the module was not transpiled with this one, so its names are unknown here (transpile the folder, or the entry with `transpile_project`)"));
                return;
            };
            let names: Vec<String> = match &d.names {
                Some(list) => list.clone(),
                None => iface.values.keys().chain(iface.fns.keys()).cloned().collect(),
            };
            for n in names {
                if let Some(f) = iface.fns.get(&n) {
                    self.imported.insert(n.clone(), (iface.path.clone(), f.js.clone()));
                    self.imported_fns.insert(n.clone(), f.clone());
                    self.names.reserve(&f.js);
                } else if let Some(js) = iface.values.get(&n) {
                    self.imported.insert(n.clone(), (iface.path.clone(), js.clone()));
                    self.names.pin(&n, js);
                }
            }
            for (c, js) in &iface.design_components {
                self.design_components.insert(c.clone(), js.clone());
            }
            self.aliases.insert(qualifier, m.to_string());
            return;
        }
        let root = m.split('/').next().unwrap_or(m);
        match (self.dialect, root) {
            (_, "design") | (Dialect::Web, "web") => {}
            (_, "ui") | (_, "bloom") => {
                self.warn(span, "immediate-mode", format!("`import {m}` draws immediate-mode UI (calls such as draw_rect, button(r, …), mouse_x) that has no React equivalent: those calls become stubs. Elements (a Mixit design, petal-web-html) convert; draw calls do not"));
            }
            (_, "color") | (_, "vec2") | (_, "noise") | (_, "json") | (_, "io") | (_, "core") | (_, "std") => {}
            _ => {
                self.unknown_packages.push(root.to_string());
                self.warn(span, "package", format!("package `{m}` is not converted: its functions become stubs and its components placeholders. Transpile its source too, or provide React versions"));
            }
        }
        if root != "design" && root != "web" {
            self.aliases.insert(qualifier, m.to_string());
        }
    }

    /// The statement that is the program's output: the last expression
    /// statement, when it builds an element (or is `design(...)`).
    fn output_index(&self, stmts: &[Stmt]) -> Option<usize> {
        let elem_fns: HashSet<String> = self.fns.iter().filter(|(_, f)| f.kind == FnKind::Component).map(|(n, _)| n.clone()).collect();
        let mut elem_names = HashSet::new();
        for s in stmts {
            if let StmtKind::Let { name, value, .. } = &s.kind {
                if analysis::is_element_expr(value, &elem_fns, &elem_names) {
                    elem_names.insert(name.clone());
                }
            }
        }
        let (i, s) = stmts.iter().enumerate().rev().find(|(_, s)| !matches!(s.kind, StmtKind::FnDecl { .. } | StmtKind::EnumDecl { .. } | StmtKind::ClassDecl { .. } | StmtKind::NativeFn(_) | StmtKind::Import(_)))?;
        let StmtKind::Expr(e) = &s.kind else { return None };
        if s.test.is_some() {
            return None;
        }
        let is_design = matches!(&e.kind, ExprKind::Call { function, .. } if matches!(&function.kind, ExprKind::Ident(n) if n == "design"));
        if is_design || analysis::is_element_expr(e, &elem_fns, &elem_names) { Some(i) } else { None }
    }

    /// Which top-level statements must run inside the app component: state,
    /// and everything that reads or writes it.
    fn partition(&mut self, stmts: &[Stmt], output: Option<usize>) -> Vec<bool> {
        let mut dynamic: HashSet<String> = HashSet::new();
        let mut app = vec![false; stmts.len()];
        let hooks: HashSet<String> = self
            .fns
            .iter()
            .chain(self.imported_fns.iter())
            .filter(|(_, f)| f.kind == FnKind::Hook)
            .map(|(n, _)| n.clone())
            .collect();
        let dynamic_calls: HashSet<&str> = match self.dialect {
            Dialect::Web => ["next_id", "clicked"].into_iter().collect(),
            _ => HashSet::new(),
        };
        loop {
            let mut changed = false;
            for (i, s) in stmts.iter().enumerate() {
                if app[i] || Some(i) == output {
                    continue;
                }
                let is_app = match &s.kind {
                    StmtKind::State { is_var: false, .. } => true,
                    StmtKind::FnDecl { .. }
                    | StmtKind::EnumDecl { .. }
                    | StmtKind::ClassDecl { .. }
                    | StmtKind::Import(_)
                    | StmtKind::NativeFn(_)
                    | StmtKind::State { is_var: true, .. } => false,
                    _ => {
                        let mut ids = HashSet::new();
                        analysis::idents_in_stmt(s, &mut ids);
                        let declares = match &s.kind {
                            StmtKind::Let { name, .. } | StmtKind::Memo { name, .. } => dynamic.contains(name),
                            _ => false,
                        };
                        declares
                            || ids.iter().any(|n| dynamic.contains(n))
                            || analysis::any_expr_in_stmt(s, &mut |e| {
                                matches!(&e.kind, ExprKind::Call { function, .. }
                                    if matches!(&function.kind, ExprKind::Ident(n) if hooks.contains(n) || dynamic_calls.contains(n.as_str())))
                            })
                    }
                };
                if is_app {
                    app[i] = true;
                    changed = true;
                    match &s.kind {
                        StmtKind::Let { name, .. } | StmtKind::State { name, .. } | StmtKind::Memo { name, .. } => {
                            dynamic.insert(name.clone());
                        }
                        _ => {}
                    }
                    dynamic.extend(analysis::assigned_names(std::slice::from_ref(s), false));
                }
            }
            if !changed {
                break;
            }
        }
        // A module-level function that reads a name the app now owns cannot
        // see it.
        for s in stmts {
            if let StmtKind::FnDecl { name, body, params, .. } = &s.kind {
                let mut ids = HashSet::new();
                body.iter().for_each(|b| analysis::idents_in_stmt(b, &mut ids));
                let own: HashSet<String> = analysis::declared_names(body).into_iter().chain(params.iter().map(|p| p.name.clone())).collect();
                let mut bad: Vec<&String> = ids.iter().filter(|n| dynamic.contains(*n) && !own.contains(*n)).collect();
                bad.sort();
                if let Some(b) = bad.first() {
                    self.warn(s.span, "top-level-state-read", format!("`{name}` reads `{b}`, which is state (or computed from state) of the top level; in React it lives inside the app component, out of this function's reach. Pass it as a parameter"));
                }
            }
        }
        app
    }

    fn top_stmt(&mut self, w: &mut W, s: &Stmt) {
        let comments = self.comments_before(s.span.start.line);
        // A blank line above it in the source stays.
        let first = s.span.start.line as usize - comments.len();
        if first >= 2 && self.lines.get(first - 2).is_some_and(|l| l.trim().is_empty()) && !w.buf.is_empty() && !w.buf.ends_with("\n\n") {
            w.line("");
        }
        for c in comments {
            w.line(c);
        }
        let mut inner = W { buf: String::new(), ind: w.ind };
        let notes_before = self.notes.len();
        self.stmt(&mut inner, s, Tail::None, true);
        let notes: Vec<String> = self.notes.drain(notes_before..).collect();
        for h in std::mem::take(&mut self.hoisted) {
            w.line(h);
            w.line("");
        }
        for n in notes {
            w.line(n);
        }
        w.append(inner);
        if matches!(s.kind, StmtKind::FnDecl { .. } | StmtKind::EnumDecl { .. } | StmtKind::ClassDecl { .. }) {
            w.line("");
        }
    }

    /// The app component: the top level's state and what depends on it.
    fn app(&mut self, w: &mut W, name: &str, stmts: &[&Stmt], output: Option<&Stmt>) {
        w.open(format!("export default function {name}() {{"));
        self.push_scope(Kind::App);
        let mut all: Vec<Stmt> = stmts.iter().map(|s| (*s).clone()).collect();
        if let Some(o) = output {
            all.push(o.clone());
        }
        self.cell_hooks(w, &all);
        self.scope().render_writes = render_written(&all);
        self.scope().mutable = mutable_names(&all);
        self.register_states(&all);
        self.plan_events(&all);
        let n = all.len();
        for (i, s) in all.iter().enumerate() {
            let last = output.is_some() && i + 1 == n;
            self.body_stmt(w, s, if last { Tail::Return(None) } else { Tail::None });
        }
        self.pop_scope();
        w.close("}");
        self.exports.values.insert("default".into(), name.to_string());
    }

    fn default_export(&mut self, w: &mut W, name: &str, s: &Stmt) {
        let StmtKind::Expr(e) = &s.kind else { return };
        // `app()` with no arguments: the component itself.
        if let ExprKind::Call { function, args, .. } = &e.kind {
            if let ExprKind::Ident(f) = &function.kind {
                if args.is_empty() {
                    if let Some(info) = self.fns.get(f).filter(|i| i.kind == FnKind::Component) {
                        w.line(format!("export default {};", info.js));
                        return;
                    }
                }
            }
        }
        self.push_scope(Kind::App);
        let notes_before = self.notes.len();
        let mut inner = W { buf: String::new(), ind: 1 };
        self.stmt(&mut inner, s, Tail::Return(None), false);
        let notes: Vec<String> = self.notes.drain(notes_before..).collect();
        self.pop_scope();
        for h in std::mem::take(&mut self.hoisted) {
            w.line(h);
            w.line("");
        }
        w.open(format!("export default function {name}() {{"));
        for n in notes {
            w.line(n);
        }
        w.append(inner);
        w.close("}");
    }

    // ── bodies ──────────────────────────────────────────────────────────────

    /// A statement inside a function body: its comments and notes first.
    pub fn body_stmt(&mut self, w: &mut W, s: &Stmt, tail: Tail) {
        let key = (s.span.start.offset, s.span.end.offset);
        if self.scope().consumed.contains(&key) {
            return;
        }
        let deferred = self.scope().handlers.contains_key(&key) && self.scope().last_state.is_some_and(|ls| ls.0 > key.0);
        for c in self.comments_before(s.span.start.line) {
            if deferred {
                self.scope().deferred.push(c);
            } else {
                w.line(c);
            }
        }
        let notes_before = self.notes.len();
        let mut inner = W { buf: String::new(), ind: w.ind };
        self.stmt(&mut inner, s, tail, false);
        let notes: Vec<String> = self.notes.drain(notes_before..).collect();
        for n in notes {
            w.line(n);
        }
        w.append(inner);
    }

    pub fn block(&mut self, w: &mut W, body: &[Stmt], tail: Tail) {
        self.scope().blocks.push(HashSet::new());
        self.block_inner(w, body, tail);
        self.scope().blocks.pop();
    }

    fn block_inner(&mut self, w: &mut W, body: &[Stmt], tail: Tail) {
        let n = body.len();
        for (i, s) in body.iter().enumerate() {
            let t = if i + 1 == n { tail } else { Tail::None };
            self.body_stmt(w, s, t);
        }
        if n == 0 {
            if let Tail::Return(_) = tail {
                w.line("return null;");
            }
        }
    }

    /// `const v = useCell(c)` for each top-level `state var` the body reads
    /// while rendering.
    pub fn cell_hooks(&mut self, w: &mut W, body: &[Stmt]) {
        if self.cells.is_empty() {
            return;
        }
        let (reads, _) = analysis::cell_uses(body);
        let mut ids = HashSet::new();
        body.iter().for_each(|s| analysis::idents_in_stmt(s, &mut ids));
        let mut used: Vec<String> = self.cells.keys().filter(|c| reads.contains(*c) || ids.contains(*c)).cloned().collect();
        used.sort();
        for c in used {
            let cell = self.cells[&c].clone();
            let v = self.names.local(&c);
            let u = self.rt("useCell");
            w.line(format!("const {v} = {u}({cell});"));
            self.scope().cell_vals.insert(c, v);
        }
    }

    /// Fold petal-web-html's click polls into handlers:
    /// `let id = next_id()` … `if clicked(id) then body` becomes
    /// `const id = () => { body }`, and `eid={id}` becomes `onClick={id}`.
    pub fn plan_events(&mut self, body: &[Stmt]) {
        if self.dialect != Dialect::Web {
            return;
        }
        let mut ids: HashMap<String, (u32, u32)> = HashMap::new();
        for s in body {
            if let StmtKind::Let { name, value, .. } = &s.kind {
                if is_call_of(value, "next_id") {
                    ids.insert(name.clone(), (s.span.start.offset, s.span.end.offset));
                }
            }
        }
        let mut polls: HashMap<String, Vec<&Stmt>> = HashMap::new();
        for s in body {
            if let Some((id, _)) = click_poll(s) {
                if ids.contains_key(&id) {
                    polls.entry(id).or_default().push(s);
                }
            }
        }
        // A poll anywhere else (nested, or of an id from a parameter)
        // makes its id an event object.
        let mut nested: HashSet<String> = HashSet::new();
        for s in body {
            if click_poll(s).is_some() {
                continue;
            }
            analysis::any_expr_in_stmt(s, &mut |e| {
                if let ExprKind::Call { function, args, .. } = &e.kind {
                    if matches!(&function.kind, ExprKind::Ident(n) if n == "clicked") {
                        if let Some(ExprKind::Ident(id)) = args.first().map(|a| &a.kind) {
                            nested.insert(id.clone());
                        }
                    }
                }
                false
            });
        }
        for (id, span) in ids {
            if nested.contains(&id) {
                self.scope().events.insert(id);
                continue;
            }
            let stmts = polls.remove(&id).unwrap_or_default();
            for s in &stmts {
                self.scope().consumed.insert((s.span.start.offset, s.span.end.offset));
            }
            let bodies: Vec<&[Stmt]> = stmts.iter().filter_map(|s| click_poll(s).map(|(_, b)| b)).collect();
            let handler = if bodies.is_empty() { "null".to_string() } else { self.handler(&bodies) };
            self.scope().handlers.insert(span, handler);
        }
    }

    /// A click handler from the bodies of the polls of one id. A write to
    /// state is made on a copy, and the copy given to the setter at the end,
    /// so later statements read the new value as they did in Petal.
    pub fn handler(&mut self, bodies: &[&[Stmt]]) -> String {
        let all: Vec<Stmt> = bodies.iter().flat_map(|b| b.iter().cloned()).collect();
        // One write of state: `() => setX(v)`.
        if let [Stmt { kind: StmtKind::Assign { target: AssignTarget::Name(n), value }, .. }] = all.as_slice() {
            if let Some(slot) = self.state_slot(n) {
                self.push_scope(Kind::Handler);
                let v = self.expr(value);
                self.pop_scope();
                return format!("() => {}({v})", slot.setter);
            }
        }
        self.push_scope(Kind::Handler);
        let mut w = W::default();
        let written = analysis::assigned_names(&all, false);
        let declared = analysis::declared_names(&all);
        let mut setters = Vec::new();
        let mut written: Vec<String> = written.into_iter().filter(|n| !declared.contains(n)).collect();
        written.sort();
        for n in written {
            if let Some(slot) = self.state_slot(&n) {
                let t = self.names.fresh(&format!("next{}", pascal(&n)));
                w.line(format!("let {t} = {};", slot.value));
                self.scope().temps.insert(n.clone(), t.clone());
                setters.push(format!("{}({t});", slot.setter));
            } else if !self.cells.contains_key(&n) {
                let span = all[0].span;
                self.warn(span, "write-in-handler", format!("a click changes `{n}`, which is read while rendering but is not state: React renders from state alone, so make `{n}` a `state`"));
                let t = self.names.fresh(&format!("{}Copy", names::camel(&n)));
                let cur = self.ident(&n, all[0].span);
                w.line(format!("let {t} = {cur};"));
                self.scope().temps.insert(n.clone(), t);
            }
        }
        for s in &all {
            self.body_stmt(&mut w, s, Tail::None);
        }
        for s in setters {
            w.line(s);
        }
        self.pop_scope();
        format!("() => {{\n{}}}", indent_lines(&w.buf))
    }

    // ── statements ──────────────────────────────────────────────────────────

    pub fn stmt(&mut self, w: &mut W, s: &Stmt, tail: Tail, top: bool) {
        let export = if top && self.scopes.len() == 1 { "export " } else { "" };
        match &s.kind {
            StmtKind::Import(_) => {}
            StmtKind::NativeFn(d) => {
                self.warn(s.span, "native", format!("`native fn {}` is implemented by the Petal host; a call of it becomes a stub", d.name));
            }
            StmtKind::Let { name, value, is_var, .. } => {
                if s.test.is_some() {
                    return;
                }
                let key = (s.span.start.offset, s.span.end.offset);
                if let Some(h) = self.scope().handlers.remove(&key) {
                    let js = self.declare(name);
                    let line = format!("const {js} = {h};");
                    match self.scope().last_state {
                        Some(ls) if ls.0 > key.0 => self.scope().deferred.push(line),
                        _ => w.line(line),
                    }
                    return;
                }
                if self.scope().events.contains(name) && is_call_of(value, "next_id") {
                    let js = self.declare(name);
                    let ev = self.rt("petal_event");
                    w.line(format!("const {js} = {ev}();"));
                    return;
                }
                let reassigned = *is_var || self.reassigned(name);
                let already = self.declared_in_block(name);
                if let Some(b) = self.scope().blocks.last_mut() {
                    b.insert(name.clone());
                }
                let v = self.expr_stmt_value(value);
                let js = self.declare(name);
                if top && self.scopes.len() == 1 {
                    self.exports.values.insert(name.clone(), js.clone());
                }
                if already {
                    w.line(format!("{js} = {v};"));
                } else {
                    let kw = if reassigned { "let" } else { "const" };
                    w.line(format!("{export}{kw} {js} = {v};"));
                }
                self.tail_value(w, tail, &js);
            }
            StmtKind::State { name, init, is_var, .. } => self.state(w, s, name, init, *is_var, export),
            StmtKind::Memo { name, key, init, .. } => {
                let js = self.declare(name);
                let v = self.expr(init);
                if self.in_render() {
                    let k = self.expr(key);
                    let m = self.react("useMemo");
                    w.line(format!("const {js} = {m}(() => {v}, [{k}]);"));
                } else {
                    self.warn(s.span, "memo", format!("`memo({})` outside a component is computed every time", self.span_text(key.span)));
                    w.line(format!("const {js} = {v};"));
                }
            }
            StmtKind::Assign { target, value } => self.assign(w, s.span, target, value, false),
            StmtKind::Set { target, value } => self.assign(w, s.span, target, value, true),
            StmtKind::Expr(e) => self.expr_stmt(w, e, tail, s.span),
            StmtKind::FnDecl { name, class, params, body, .. } => {
                if let Some(c) = class {
                    let m = name.rsplit('.').next().unwrap_or(name);
                    let js = format!("{c}_{}", names::camel(m));
                    self.plain_fn(w, &js, params, body, export);
                    w.line(format!("{c}.{} = {js};", names::js_key(m)));
                    return;
                }
                if s.test.is_some() {
                    self.warn(s.span, "test", format!("test \"{}\" is not converted", s.test.clone().unwrap_or_default()));
                    return;
                }
                if self.scopes.len() > 1 {
                    // A function inside a function: plain, called as written.
                    let js = self.declare(name);
                    if analysis::declares_state(body) {
                        self.warn(s.span, "nested-state", format!("`{name}` keeps state but is defined inside another function; React hooks need a top-level component"));
                    }
                    self.plain_fn(w, &js, params, body, "");
                    return;
                }
                let Some(info) = self.fns.get(name).cloned() else { return };
                self.exports.fns.insert(name.clone(), info.clone());
                match info.kind {
                    FnKind::Component => self.component(w, s, &info, params, body),
                    FnKind::Hook => self.hook_fn(w, &info, params, body),
                    FnKind::Plain => self.plain_fn(w, &info.js, params, body, "export "),
                }
            }
            StmtKind::EnumDecl { name, variants } => {
                let js = self.names.local(name);
                self.exports.values.insert(name.clone(), js.clone());
                let mut parts = Vec::new();
                for v in variants {
                    let mut fields: Vec<String> = vec![format!("tag: {}", js_string(&v.name))];
                    for d in &v.data {
                        let val = self.expr(&d.value);
                        fields.push(format!("{}: {val}", names::js_key(&d.key)));
                    }
                    let key = names::js_key(&v.name);
                    if v.fields.is_empty() {
                        parts.push(format!("{key}: Object.freeze({{ {} }})", fields.join(", ")));
                    } else {
                        let ps: Vec<String> = v.fields.iter().map(|f| names::camel(f)).collect();
                        for (f, p) in v.fields.iter().zip(&ps) {
                            fields.push(if names::js_key(f) == *p { p.clone() } else { format!("{}: {p}", names::js_key(f)) });
                        }
                        parts.push(format!("{key}: ({}) => ({{ {} }})", ps.join(", "), fields.join(", ")));
                    }
                }
                w.open(format!("{export}const {js} = Object.freeze({{"));
                for p in parts {
                    w.line(format!("{p},"));
                }
                w.close("});");
            }
            StmtKind::ClassDecl { name, fields } => {
                let ps: Vec<String> = fields.iter().map(|f| names::camel(&f.name)).collect();
                let mut kv = vec![format!("__class: {}", js_string(name))];
                for (f, p) in fields.iter().zip(&ps) {
                    kv.push(if names::js_key(&f.name) == *p { p.clone() } else { format!("{}: {p}", names::js_key(&f.name)) });
                }
                w.open(format!("{export}function {name}({}) {{", ps.join(", ")));
                w.line(format!("return {{ {} }};", kv.join(", ")));
                w.close("}");
            }
            StmtKind::For { var, iter, body, key, flatten } => {
                if let Tail::Return(wrap) = tail {
                    // A `for` in tail position collects a list.
                    let e = Expr {
                        kind: ExprKind::For { var: var.clone(), iter: Box::new(iter.clone()), key: key.clone(), body: body.clone(), flatten: *flatten },
                        span: s.span,
                    };
                    let v = self.expr(&e);
                    w.line(ret(wrap, &v));
                    return;
                }
                let it = self.expr(iter);
                self.scope().nested += 1;
                let mutable = analysis::assigned_names(body, false).contains(var);
                let v = self.declare(var);
                w.open(format!("for ({} {v} of {it}) {{", if mutable { "let" } else { "const" }));
                self.block(w, body, Tail::None);
                w.close("}");
                self.scope().nested -= 1;
            }
            StmtKind::While { condition, body } => {
                let c = self.expr(condition);
                self.scope().nested += 1;
                w.open(format!("while ({c}) {{"));
                self.block(w, body, Tail::None);
                w.close("}");
                self.scope().nested -= 1;
            }
            StmtKind::Return(e) => {
                let wrap = match tail {
                    Tail::Return(w) => w,
                    Tail::None => self.return_wrap(),
                };
                match e {
                    Some(e) => {
                        let v = self.expr(e);
                        w.line(ret(wrap, &v));
                    }
                    None => w.line("return null;"),
                }
            }
            StmtKind::Break => w.line("break;"),
            StmtKind::Continue => w.line("continue;"),
        }
    }

    /// The wrapper a `return` in the current component uses.
    fn return_wrap(&self) -> Option<&'static str> {
        let props = self.scopes.iter().rev().find(|s| !matches!(s.kind(), Kind::Lambda | Kind::Handler))?.props_js.clone()?;
        Some(Box::leak(format!("instance_root({props}").into_boxed_str()))
    }

    fn tail_value(&mut self, w: &mut W, tail: Tail, js: &str) {
        if let Tail::Return(wrap) = tail {
            w.line(ret(wrap, js));
        }
    }

    /// Whether the current function rebinds `name` (so it is a `let`).
    fn reassigned(&self, name: &str) -> bool {
        self.scopes.last().is_some_and(|s| s.mutable.contains(name))
    }

    /// Whether `name` was declared in the innermost open block.
    fn declared_in_block(&self, name: &str) -> bool {
        self.scopes.last().and_then(|s| s.blocks.last()).is_some_and(|b| b.contains(name))
    }

    fn expr_stmt_value(&mut self, value: &Expr) -> String {
        let v = self.expr(value);
        paren_multiline(&v)
    }

    fn expr_stmt(&mut self, w: &mut W, e: &Expr, tail: Tail, span: SourceSpan) {
        // A click poll left over (nested): an event handler registration.
        if let ExprKind::If { condition, then_body, else_body: None } = &e.kind {
            if let Some(id) = clicked_id(condition) {
                if self.dialect == Dialect::Web && tail == Tail::None {
                    let idj = self.ident(&id, span);
                    let body: &[Stmt] = then_body;
                    let h = self.handler(&[body]);
                    w.line(format!("{idj}.on({h});"));
                    return;
                }
            }
        }
        match (&e.kind, tail) {
            (ExprKind::If { condition, then_body, else_body }, _) => {
                let c = self.expr(condition);
                self.scope().nested += 1;
                w.open(format!("if ({c}) {{"));
                self.block(w, then_body, tail);
                let mut eb = else_body.as_ref();
                loop {
                    match eb {
                        None => {
                            if let Tail::Return(_) = tail {
                                w.close("}");
                                w.line("return null;");
                            } else {
                                w.close("}");
                            }
                            break;
                        }
                        Some(ElseBranch::Block(b)) => {
                            w.close("} else {");
                            w.ind += 1;
                            self.block(w, b, tail);
                            w.close("}");
                            break;
                        }
                        Some(ElseBranch::ElseIf(e2)) => {
                            if let ExprKind::If { condition, then_body, else_body } = &e2.kind {
                                let c = self.expr(condition);
                                w.close(format!("}} else if ({c}) {{"));
                                w.ind += 1;
                                self.block(w, then_body, tail);
                                eb = else_body.as_ref();
                            } else {
                                w.close("} else {");
                                w.ind += 1;
                                self.expr_stmt(w, e2, tail, e2.span);
                                w.close("}");
                                break;
                            }
                        }
                    }
                }
                self.scope().nested -= 1;
            }
            (ExprKind::Match { subject, arms }, _) => {
                self.match_stmt(w, subject, arms, tail);
            }
            (_, Tail::Return(wrap)) => {
                let v = self.expr(e);
                w.line(ret(wrap, &v));
            }
            (ExprKind::For { var, iter, body, .. }, Tail::None) => {
                let it = self.expr(iter);
                self.scope().nested += 1;
                let v = self.declare(var);
                w.open(format!("for (const {v} of {it}) {{"));
                self.block(w, body, Tail::None);
                w.close("}");
                self.scope().nested -= 1;
            }
            (_, Tail::None) => {
                let v = self.expr(e);
                w.line(format!("{};", paren_multiline(&v)));
            }
        }
    }

    pub fn match_stmt(&mut self, w: &mut W, subject: &Expr, arms: &[MatchArm], tail: Tail) {
        let sj = self.expr(subject);
        let m = self.names.fresh("matched");
        let label = format!("{m}Arms");
        if tail == Tail::None {
            w.open(format!("{label}: {{"));
        }
        w.line(format!("const {m} = {sj};"));
        self.scope().nested += 1;
        for arm in arms {
            let (cond, binds) = self.pattern(&arm.pattern, &m);
            w.open(format!("if ({cond}) {{"));
            for (n, v) in binds {
                let js = self.declare(&n);
                w.line(format!("const {js} = {v};"));
            }
            if let Some(g) = &arm.guard {
                let gc = self.expr(g);
                w.open(format!("if ({gc}) {{"));
            }
            self.block(w, &arm.body, tail);
            if tail == Tail::None {
                w.line(format!("break {label};"));
            }
            if arm.guard.is_some() {
                w.close("}");
            }
            w.close("}");
        }
        self.scope().nested -= 1;
        if tail == Tail::None {
            w.close("}");
        } else {
            w.line(format!("throw new Error(`no match arm for ${{JSON.stringify({m})}}`);"));
        }
    }

    /// A pattern as a condition on `subject` and the names it binds.
    pub fn pattern(&mut self, p: &Pattern, subject: &str) -> (String, Vec<(String, String)>) {
        match p {
            Pattern::Wildcard => ("true".into(), vec![]),
            Pattern::Variable(n) => {
                if self.variant_fields.contains_key(n) {
                    return (format!("{subject}?.tag === {}", js_string(n)), vec![]);
                }
                ("true".into(), vec![(n.clone(), subject.to_string())])
            }
            Pattern::Literal(l) => match l {
                Literal::Nil => (format!("{subject} == null"), vec![]),
                _ => (format!("{subject} === {}", literal(l)), vec![]),
            },
            Pattern::Variant { name, fields } => {
                let v = name.rsplit('.').next().unwrap_or(name).to_string();
                let mut cond = vec![format!("{subject}?.tag === {}", js_string(&v))];
                let mut binds = vec![];
                let names = self.variant_fields.get(&v).cloned().unwrap_or_default();
                for (i, f) in fields.iter().enumerate() {
                    let field = names.get(i).cloned().unwrap_or_else(|| format!("_{i}"));
                    let (c, b) = self.pattern(f, &format!("{subject}.{}", field));
                    if c != "true" {
                        cond.push(c);
                    }
                    binds.extend(b);
                }
                (cond.join(" && "), binds)
            }
            Pattern::List { elements, rest } => {
                let n = elements.len();
                let mut cond = vec![format!(
                    "Array.isArray({subject}) && {subject}.length {} {n}",
                    if rest.is_some() { ">=" } else { "===" }
                )];
                let mut binds = vec![];
                for (i, e) in elements.iter().enumerate() {
                    let (c, b) = self.pattern(e, &format!("{subject}[{i}]"));
                    if c != "true" {
                        cond.push(c);
                    }
                    binds.extend(b);
                }
                if let Some(r) = rest {
                    binds.push((r.clone(), format!("{subject}.slice({n})")));
                }
                (cond.join(" && "), binds)
            }
            Pattern::Record(fields) => {
                let mut cond = vec![format!("{subject} != null && typeof {subject} === \"object\"")];
                let mut binds = vec![];
                for (k, fp) in fields {
                    cond.push(format!("{} in {subject}", js_string(k)));
                    let acc = if names::is_js_ident(k) { format!("{subject}.{k}") } else { format!("{subject}[{}]", js_string(k)) };
                    let (c, b) = self.pattern(fp, &acc);
                    if c != "true" {
                        cond.push(c);
                    }
                    binds.extend(b);
                }
                (cond.join(" && "), binds)
            }
            Pattern::Or(alts) => {
                let mut conds = vec![];
                let mut binds = vec![];
                for (i, a) in alts.iter().enumerate() {
                    let (c, b) = self.pattern(a, subject);
                    conds.push(format!("({c})"));
                    if i == 0 {
                        binds = b;
                    }
                }
                (conds.join(" || "), binds)
            }
        }
    }

    fn state(&mut self, w: &mut W, s: &Stmt, name: &str, init: &Expr, is_var: bool, export: &str) {
        // A top-level `state var`: a cell every component shares.
        if self.scopes.len() == 1 {
            if is_var {
                let cell = self.cells.get(name).cloned().unwrap_or_else(|| self.names.fresh(name));
                let v = self.expr(init);
                let c = self.rt("cell");
                w.line(format!("{export}const {cell} = {c}({v});"));
                self.exports.values.insert(name.to_string(), cell);
            }
            return;
        }
        let kind = self.scope().kind();
        if !kind.hooks() {
            self.warn(s.span, "state-outside-component", format!("`state {name}` is in a function React does not render (a callback or a plain helper); it becomes a plain variable that does not persist"));
            let js = self.declare(name);
            let v = self.expr(init);
            w.line(format!("let {js} = {v};"));
            return;
        }
        if self.scope().nested > 0 {
            self.warn(s.span, "state-in-loop", format!("`state {name}` is inside a loop or branch, where Petal keeps one slot per iteration (or drops it when the branch is not taken). React hooks cannot be called conditionally: move this into a component rendered per item (with a `key`) or per branch"));
        }
        let lazy = !is_cheap(init);
        let v = self.expr(init);
        let us = self.react("useState");
        let init_js = if lazy { format!("() => {v}") } else { v };
        let base = names::camel(name);
        let slot = match self.scope().states.get(name).cloned() {
            Some(slot) => slot,
            None => StateSlot { value: self.names.local(name), setter: self.names.fresh(&format!("set{}", pascal(name))) },
        };
        let setter = slot.setter.clone();
        // Written while rendering? Then the render works on a copy.
        if self.scope().render_writes.contains(name) {
            let stored = self.names.fresh(&format!("{base}State"));
            let js = slot.value.clone();
            w.line(format!("const [{stored}, {setter}] = {us}({init_js});"));
            w.line(format!("let {js} = {stored};"));
            self.warn(s.span, "state-write-in-render", format!("`{name}` is changed while rendering. Petal re-runs every frame and keeps the new value; React state changes only in an event handler or effect, so this change lasts for the current render only"));
        } else {
            w.line(format!("const [{}, {setter}] = {us}({init_js});", slot.value));
        }
        self.scope().locals.remove(name);
        self.scope().states.insert(name.to_string(), slot);
        if self.scope().last_state == Some((s.span.start.offset, s.span.end.offset)) {
            for l in std::mem::take(&mut self.scope().deferred) {
                w.line(l);
            }
        }
    }

    // ── assignments ─────────────────────────────────────────────────────────

    fn assign(&mut self, w: &mut W, span: SourceSpan, target: &AssignTarget, value: &Expr, set: bool) {
        let base = analysis::target_base(target).map(|b| b.to_string());
        // A top-level `state var`.
        if let Some(b) = base.as_deref().filter(|b| self.cells.contains_key(*b) && !self.is_local(b)) {
            let cell = self.cells[b].clone();
            let v = self.expr(value);
            let new = match target {
                AssignTarget::Name(_) => v,
                _ => {
                    let cur = format!("{cell}.get()");
                    self.rebuild(target, &v, &cur)
                }
            };
            if self.in_render() {
                self.warn(span, "state-write-in-render", format!("`set {b}` while rendering: React state changes only in an event handler or effect"));
            }
            w.line(format!("{cell}.set({new});"));
            return;
        }
        let _ = set;
        let Some(b) = base else {
            self.warn(span, "assign", "an assignment to a computed place is not converted");
            return;
        };
        // State written in a handler: the handler's copy. In render: the
        // render's copy (warned at the declaration).
        let target_js = if let Some(t) = self.temp(&b) {
            t
        } else if let Some(slot) = self.state_slot(&b) {
            if self.in_handler() {
                // Unplanned (a nested handler): call the setter directly.
                let v = self.expr(value);
                let new = match target {
                    AssignTarget::Name(_) => v,
                    _ => self.rebuild(target, &v, &slot.value),
                };
                w.line(format!("{}({new});", slot.setter));
                return;
            }
            slot.value
        } else {
            self.ident(&b, span)
        };
        let v = self.expr(value);
        let new = match target {
            AssignTarget::Name(_) => v,
            _ => self.rebuild(target, &v, &target_js),
        };
        w.line(format!("{target_js} = {new};"));
    }

    fn temp(&self, name: &str) -> Option<String> {
        for s in self.scopes.iter().rev() {
            if let Some(t) = s.temps.get(name) {
                return Some(t.clone());
            }
            if s.locals.contains(name) {
                return None;
            }
        }
        None
    }

    /// The new value of the base of `r.a[i].b = v`: records and lists are
    /// values, so the path is rebuilt (`{ ...r, a: with_at(r.a, i, …) }`).
    fn rebuild(&mut self, target: &AssignTarget, v: &str, base_js: &str) -> String {
        match target {
            AssignTarget::Name(_) => v.to_string(),
            AssignTarget::Field(obj, f) => {
                let cur = self.place(obj, base_js);
                let inner = format!("{{ ...{cur}, {}: {v} }}", names::js_key(f));
                self.rebuild_expr(obj, &inner, base_js)
            }
            AssignTarget::Index(obj, idx) => {
                let cur = self.place(obj, base_js);
                let i = self.expr(idx);
                let wa = self.rt("with_at");
                let inner = format!("{wa}({cur}, {i}, {v})");
                self.rebuild_expr(obj, &inner, base_js)
            }
        }
    }

    fn rebuild_expr(&mut self, obj: &Expr, inner: &str, base_js: &str) -> String {
        match &obj.kind {
            ExprKind::Ident(_) | ExprKind::CellGet(_) => inner.to_string(),
            ExprKind::FieldAccess { object, field } => {
                let t = AssignTarget::Field(object.clone(), field.clone());
                self.rebuild(&t, inner, base_js)
            }
            ExprKind::IndexAccess { object, index } => {
                let t = AssignTarget::Index(object.clone(), index.clone());
                self.rebuild(&t, inner, base_js)
            }
            _ => inner.to_string(),
        }
    }

    /// The current value of a place, read through the base's JS name.
    fn place(&mut self, e: &Expr, base_js: &str) -> String {
        match &e.kind {
            ExprKind::Ident(_) | ExprKind::CellGet(_) => base_js.to_string(),
            ExprKind::FieldAccess { object, field } => {
                let o = self.place(object, base_js);
                if names::is_js_ident(field) { format!("{o}.{field}") } else { format!("{o}[{}]", js_string(field)) }
            }
            ExprKind::IndexAccess { object, index } => {
                let o = self.place(object, base_js);
                let i = self.expr(index);
                let at = self.rt("at");
                format!("{at}({o}, {i})")
            }
            _ => self.expr(e),
        }
    }

    // ── functions ───────────────────────────────────────────────────────────

    /// `(a, b = 2)` — declares the parameters in the current scope.
    pub fn params(&mut self, params: &[Param]) -> Vec<String> {
        let mut out = Vec::new();
        for p in params {
            let d = p.default.as_ref().map(|d| self.expr(d));
            let js = self.declare(&p.name);
            out.push(match d {
                Some(d) => format!("{js} = {d}"),
                None => js,
            });
        }
        out
    }

    fn plain_fn(&mut self, w: &mut W, js: &str, params: &[Param], body: &[Stmt], export: &str) {
        self.push_scope(Kind::Plain);
        let ps = self.params(params);
        w.open(format!("{export}function {js}({}) {{", ps.join(", ")));
        self.captured_writes(w, body);
        self.fn_body(w, body, Tail::Return(None));
        w.close("}");
        self.pop_scope();
    }

    fn hook_fn(&mut self, w: &mut W, info: &FnInfo, params: &[Param], body: &[Stmt]) {
        self.push_scope(Kind::Hook);
        let ps = self.params(params);
        w.open(format!("export function {}({}) {{", info.js, ps.join(", ")));
        self.cell_hooks(w, body);
        self.captured_writes(w, body);
        self.fn_body(w, body, Tail::Return(None));
        w.close("}");
        self.pop_scope();
    }

    fn component(&mut self, w: &mut W, s: &Stmt, info: &FnInfo, params: &[Param], body: &[Stmt]) {
        self.push_scope(Kind::Component);
        if info.design_component.is_some() {
            // A design component: `render(p, cx)` with its props merged over
            // the defaults and `cx` from the frame it is drawn in.
            // `props` unless the render function binds that name itself.
            let own = analysis::declared_names(body);
            let props = if own.contains("props") || params.iter().any(|p| p.name == "props") { "props_".to_string() } else { "props".to_string() };
            w.open(format!("export function {}({props}) {{", info.js));
            if let Some(p) = params.first() {
                let pj = self.declare(&p.name);
                let cp = self.design_rt("component_props");
                w.line(format!("const {pj} = {cp}({props}, {}.defaults);", info.js));
            }
            if let Some(c) = params.get(1) {
                let cj = self.declare(&c.name);
                let u = self.design_rt("useCx");
                w.line(format!("const {cj} = {u}();"));
            }
            if params.len() > 2 {
                self.warn(s.span, "component-params", format!("design component `{}` takes more than (p, cx); the rest are undefined", info.js));
                for p in &params[2..] {
                    self.declare(&p.name);
                }
            }
            self.scope().props_js = Some(props);
            self.cell_hooks(w, body);
            self.captured_writes(w, body);
            self.design_rt("instance_root");
            let wrap: &'static str = Box::leak(format!("instance_root({}", self.scope().props_js.clone().unwrap_or_default()).into_boxed_str());
            self.fn_body(w, body, Tail::Return(Some(wrap)));
        } else {
            let mut ps = Vec::new();
            for p in params {
                let d = p.default.as_ref().map(|d| self.expr(d));
                let js = self.declare(&p.name);
                let prop = prop_name(&p.name);
                let head = if prop == js { js.clone() } else { format!("{prop}: {js}") };
                ps.push(match d {
                    Some(d) => format!("{head} = {d}"),
                    None => head,
                });
            }
            let sig = if ps.is_empty() { String::new() } else { format!("{{ {} }}", ps.join(", ")) };
            w.open(format!("export function {}({sig}) {{", info.js));
            self.cell_hooks(w, body);
            self.captured_writes(w, body);
            self.fn_body(w, body, Tail::Return(None));
        }
        w.close("}");
        self.pop_scope();
    }

    /// Names a function rebinds that it did not declare (captured from the
    /// module, or a parameter): Petal rebinds a local copy, so the JS does
    /// too, instead of writing the outer variable.
    fn captured_writes(&mut self, w: &mut W, body: &[Stmt]) {
        let written = analysis::assigned_names(body, false);
        let declared = analysis::declared_names(body);
        let mut names: Vec<String> = written.into_iter().filter(|n| !declared.contains(n)).collect();
        names.sort();
        for n in names {
            if self.cells.contains_key(&n) || self.state_slot(&n).is_some() {
                continue;
            }
            if self.scope().locals.contains(&n) {
                // A parameter: JS parameters are already local.
                continue;
            }
            let t = self.names.fresh(&format!("{}Local", names::camel(&n)));
            let cur = self.names.local(&n);
            w.line(format!("let {t} = {cur};"));
            self.scope().temps.insert(n, t);
        }
    }

    pub fn fn_body(&mut self, w: &mut W, body: &[Stmt], tail: Tail) {
        if self.scope().kind().hooks() {
            self.scope().render_writes = render_written(body);
        }
        self.scope().mutable = mutable_names(body);
        self.register_states(body);
        self.plan_events(body);
        self.block(w, body, tail);
    }

    /// Give each top-level `state` of a body its names before the body is
    /// written, so code above the declaration (a click handler) can call
    /// its setter.
    pub fn register_states(&mut self, body: &[Stmt]) {
        if !self.scope().kind().hooks() {
            return;
        }
        let writes = self.scope().render_writes.clone();
        for st in body {
            if let StmtKind::State { name, .. } = &st.kind {
                let setter = self.names.fresh(&format!("set{}", pascal(name)));
                let value = self.names.local(name);
                let _ = writes.contains(name);
                self.scope().states.insert(name.clone(), StateSlot { value, setter });
                self.scope().last_state = Some((st.span.start.offset, st.span.end.offset));
            }
        }
    }
}

/// Names written by plain assignment outside lambdas and click polls.
pub fn render_written(body: &[Stmt]) -> HashSet<String> {
    let filtered: Vec<Stmt> = body.iter().filter(|s| click_poll(s).is_none()).cloned().collect();
    analysis::assigned_names(&filtered, false)
}

/// Names a body rebinds, or declares more than once.
pub fn mutable_names(body: &[Stmt]) -> HashSet<String> {
    let mut out = analysis::assigned_names(body, false);
    let mut seen = HashSet::new();
    for s in body {
        petal::ast::for_each_expr_in_stmt(s, &mut |_| {});
        if let StmtKind::Let { name, .. } = &s.kind {
            if !seen.insert(name.clone()) {
                out.insert(name.clone());
            }
        }
    }
    out
}

pub fn ret(wrap: Option<&str>, v: &str) -> String {
    let v = paren_multiline(v);
    match wrap {
        Some(f) => format!("return {f}, {v});"),
        None => format!("return {v};"),
    }
}

/// A multi-line JSX expression in parentheses, as JSX is usually written.
pub fn paren_multiline(v: &str) -> String {
    if v.contains('\n') && v.trim_start().starts_with('<') {
        format!("(\n{}\n)", indent_all(v))
    } else {
        v.to_string()
    }
}

pub fn indent_lines(s: &str) -> String {
    s.lines().map(|l| if l.trim().is_empty() { "\n".to_string() } else { format!("  {l}\n") }).collect()
}

pub fn literal(l: &Literal) -> String {
    match l {
        Literal::Nil => "null".into(),
        Literal::Bool(b) => b.to_string(),
        Literal::Int(i) => i.to_string(),
        Literal::Float(f) => {
            if f.is_infinite() {
                if *f > 0.0 { "Infinity".into() } else { "-Infinity".into() }
            } else if f.is_nan() {
                "NaN".into()
            } else {
                let s = format!("{f:?}");
                s
            }
        }
        Literal::String(s) => js_string(s),
    }
}

/// A React prop name for a Petal parameter (`key` and `ref` are React's).
pub fn prop_name(p: &str) -> String {
    let c = names::camel(p);
    match c.as_str() {
        "key" | "ref" => format!("{c}_"),
        _ if names::is_reserved(&c) || !names::is_js_ident(&c) => format!("{c}_"),
        _ => c,
    }
}

/// An initializer cheap enough to evaluate on every render.
fn is_cheap(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) => true,
        ExprKind::UnaryOp { operand, .. } => is_cheap(operand),
        ExprKind::Record(fs) => fs.iter().all(|f| match f {
            petal::ast::RecordField::Named(_, v) => is_cheap(v),
            petal::ast::RecordField::Spread(v) => is_cheap(v),
        }),
        ExprKind::List(xs) => xs.len() <= 4 && xs.iter().all(is_cheap),
        _ => false,
    }
}

pub fn is_call_of(e: &Expr, name: &str) -> bool {
    matches!(&e.kind, ExprKind::Call { function, .. } if matches!(&function.kind, ExprKind::Ident(n) if n == name))
}

fn clicked_id(cond: &Expr) -> Option<String> {
    if let ExprKind::Call { function, args, .. } = &cond.kind {
        if matches!(&function.kind, ExprKind::Ident(n) if n == "clicked") {
            if let Some(ExprKind::Ident(id)) = args.first().map(|a| &a.kind) {
                return Some(id.clone());
            }
        }
    }
    None
}

/// `if clicked(id) then body` (no else) → (id, body).
pub fn click_poll(s: &Stmt) -> Option<(String, &[Stmt])> {
    if let StmtKind::Expr(Expr { kind: ExprKind::If { condition, then_body, else_body: None }, .. }) = &s.kind {
        return clicked_id(condition).map(|id| (id, then_body.as_slice()));
    }
    None
}
