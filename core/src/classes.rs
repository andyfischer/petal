//! Classes — named record types with typed fields and methods.
//!
//! A `class` declares a *shape*, not a new runtime kind: an instance is an
//! ordinary record ([`Value::Map`](crate::value::Value::Map)) that carries a
//! class tag in the heap, so every record operation (`keys`, `len`, field
//! access, spread) keeps working on it. What the tag buys is
//!
//! - a name in type position (`fn f(r: Rect)` checks, see [`crate::types`]), and
//! - method dispatch: `r.center_x()` finds `fn Rect.center_x(r: Rect)`.
//!
//! This table is the compile-time registry of that knowledge. It is built per
//! compilation, pre-seeded with the built-in classes (see [`ClassTable::new`]),
//! then extended by the compiler's prescan with the module's own `class`
//! declarations — so forward references (`fn f(p: Point)` above `class Point`)
//! resolve.
//!
//! The runtime half lives in two places: the constructor and built-in methods
//! are natives (`crate::builtins::classes`), and user-declared methods register
//! into the VM's per-run table (`Stack::methods`) as the root block runs.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::program::EnumDef;
use crate::source_map::SourceSpan;
use crate::types::{FnSignature, Type};

/// Index of a class in a [`ClassTable`]. Small and `Copy` so [`Type`] stays
/// `Copy`; the name and fields live in the table entry it points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ClassId(pub u32);

/// Index of an enum in a [`ClassTable`], for [`Type::EnumOf`]. `Copy` for the
/// reason [`ClassId`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct EnumId(pub u32);

/// One declared field: a name and its optional declared type. `ty` is `None`
/// for an un-annotated field (`any`) or an unrecognized type name.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassField {
    pub name: String,
    pub ty: Option<Type>,
}

/// One method known to be callable on a class: `fn Rect.inset(r: Rect, n: int)`
/// is `MethodDef { name: "inset", arity: 2, … }` — the arity *includes* the
/// receiver, which is what the call site `r.inset(4)` supplies implicitly.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodDef {
    pub name: String,
    /// Parameter count including the receiver. Always `sig.params.len()`.
    pub arity: usize,
    /// What the declaration says this method takes and returns, positionally,
    /// with the receiver in slot 0. An absent or unrecognized annotation is a
    /// `None` slot, which the checker reads as `any` — so a wholly
    /// un-annotated method carries a signature of all-`None` and constrains
    /// nothing. This is what lets a method call be *typed* rather than
    /// inferring `any`; see `crate::typecheck`.
    pub sig: FnSignature,
    /// Whether this method is built into the language rather than declared by
    /// a `fn Class.name(...)` statement. A user declaration of the same name
    /// and arity replaces a built-in one — which is also what runtime dispatch
    /// does, consulting user methods before built-ins.
    pub builtin: bool,
}

/// A declared class: its name, its fields in declaration order (which is also
/// constructor-argument order), and the methods declared on it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassDef {
    pub name: String,
    pub fields: Vec<ClassField>,
    pub methods: Vec<MethodDef>,
    /// Whether this class is built into the language (no `class` statement
    /// declared it). Built-ins may not be redeclared.
    pub builtin: bool,
}

impl ClassDef {
    pub fn field(&self, name: &str) -> Option<&ClassField> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub fn method(&self, name: &str) -> Option<&MethodDef> {
        self.methods.iter().find(|m| m.name == name)
    }
}

/// The classes of one compilation, by identity.
///
/// A class or enum belongs to the module that declares it: its **identity**
/// is its bare name in the entry file (`Phase`) and `module::Name` in any
/// other module (`./level::Phase`, `ui::Align`), the module half being the
/// module's canonical name (see [`ClassTable::set_namespace`]). So two
/// modules may each declare `enum Phase`, and a [`ClassId`] still means the
/// same thing in every module. What a given file can *see*, and by which
/// spelling, is narrower: a class name resolves only where its constructor
/// does. [`ClassTable::set_scope`] installs that per-file view (written
/// spelling → identity) and [`ClassTable::lookup`] respects it, so a
/// module-private class is not a type name in an importer either (built-in
/// classes are always in scope).
#[derive(Debug, Clone)]
pub struct ClassTable {
    defs: Vec<ClassDef>,
    by_name: HashMap<String, ClassId>,
    /// The names of the built-in classes, which no scope hides — captured at
    /// construction so a user redeclaration of one (`class Rect … end`, which
    /// clears the `builtin` flag on the entry) stays universally visible.
    builtin_names: HashSet<String>,
    /// The file each user class was declared in, for naming both sides of a
    /// cross-module duplicate. Absent for built-ins and for a compilation that
    /// passes no module name.
    origins: HashMap<ClassId, String>,
    /// The type names in scope for the file currently being compiled, by
    /// the spelling the file writes (`Circle`, or `sh.Circle` through a
    /// module qualifier) to the identity it means, or `None` for "every
    /// declared class, by identity" — the default, which is what a
    /// single-file compilation and the unit tests want.
    scope: Option<HashMap<String, String>>,
    /// The canonical name of the module whose declarations are being
    /// collected, which prefixes their identities; `None` for the entry file.
    namespace: Option<String>,
    /// The compilation's enums: the other kind of declared, named type. They
    /// live here so that everything holding the table to resolve or spell a
    /// type (`k: Kind`, "expected `Kind`") knows them without a second table
    /// to pass. An enum's identity is unique in a compilation, as a class's is.
    enums: Vec<EnumEntry>,
    enum_by_name: HashMap<String, EnumId>,
    /// The module qualifiers the current file can write in type position
    /// (`sh` in `c: sh.Circle`): each with the module as the file spelled
    /// it, and the names that module exports. Set beside [`Self::scope`].
    qualifiers: HashMap<String, Qualifier>,
}

/// A module qualifier usable in a type annotation — see
/// [`ClassTable::set_qualifiers`].
#[derive(Debug, Clone, Default)]
pub struct Qualifier {
    /// The module as the importing file wrote it (`./shapes`, `ui`).
    pub module: String,
    /// Every name the module exports.
    pub exports: HashSet<String>,
}

#[derive(Debug, Clone)]
struct EnumEntry {
    name: String,
    def: EnumDef,
    /// The declaring file, as [`ClassTable::origins`] keeps for a class.
    origin: Option<String>,
    /// The declaring statement, which tells the declaration from a refused
    /// redeclaration of the name.
    span: SourceSpan,
    /// The methods declared on it (`fn Kind.describe(k: Kind)`), held to the
    /// rules a class's are: see [`ClassTable::declare_enum_method`].
    methods: Vec<MethodDef>,
}

impl Default for ClassTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassTable {
    /// A table holding only the built-in classes. Every compilation starts
    /// here — there is no Petal-source prelude, so `Rect` is available to every
    /// program without an import.
    pub fn new() -> Self {
        let mut table = ClassTable {
            defs: Vec::new(),
            by_name: HashMap::new(),
            builtin_names: HashSet::new(),
            origins: HashMap::new(),
            scope: None,
            namespace: None,
            enums: Vec::new(),
            enum_by_name: HashMap::new(),
            qualifiers: HashMap::new(),
        };
        for def in builtin_classes() {
            table.builtin_names.insert(def.name.clone());
            table.insert(def);
        }
        table
    }

    /// Restrict [`ClassTable::lookup`] to `names`, each a spelling the file
    /// may write mapped to the identity it means (plus the built-in classes,
    /// which no file has to import). Called once per module, with the type
    /// names that module declares or imports — see
    /// `Compiler::visible_class_names`.
    pub fn set_scope(&mut self, names: HashMap<String, String>) {
        self.scope = Some(names);
    }

    /// The module whose declarations [`Self::declare`] and
    /// [`Self::declare_enum`] take next: `Some(canonical name)` for an
    /// imported module, `None` for the entry file. It prefixes their
    /// identities, so `enum Phase` in `./level` is `./level::Phase`.
    pub fn set_namespace(&mut self, namespace: Option<String>) {
        self.namespace = namespace;
    }

    /// The identity a declaration of `name` takes in the current namespace.
    pub fn qualify(&self, name: &str) -> String {
        match &self.namespace {
            Some(m) => format!("{m}::{name}"),
            None => name.to_string(),
        }
    }

    /// The identity `written` names in the current scope, if any.
    fn resolve_spelling(&self, written: &str) -> Option<String> {
        match &self.scope {
            Some(scope) => match scope.get(written) {
                Some(identity) => Some(identity.clone()),
                None => self.builtin_names.contains(written).then(|| written.to_string()),
            },
            // Every identity is a bare name where no scope is set (one file,
            // the entry), so a qualifier has nothing to choose between.
            None => Some(written.rsplit('.').next().unwrap_or(written).to_string()),
        }
    }

    /// Drop the per-file view: every declared class resolves again.
    pub fn clear_scope(&mut self) {
        self.scope = None;
        self.qualifiers.clear();
    }

    /// The module qualifiers the current file binds (plain, `as` and
    /// implicit imports), for `q.Name` in type position. Replaces the
    /// previous file's.
    pub fn set_qualifiers(&mut self, qualifiers: HashMap<String, Qualifier>) {
        self.qualifiers = qualifiers;
    }

    fn insert(&mut self, def: ClassDef) -> ClassId {
        let id = ClassId(self.defs.len() as u32);
        self.by_name.insert(def.name.clone(), id);
        self.defs.push(def);
        id
    }

    /// Declare a user class.
    ///
    /// Declaring one that shadows a **built-in** class replaces it, keeping the
    /// same [`ClassId`] — the same rule the rest of the language follows, where
    /// a user binding shadows a builtin of the same name. So a program may spell
    /// out `class Rect … end` even though `Rect` is built in, and gets its own
    /// definition. (The built-in *methods* stay registered and remain reachable
    /// on instances tagged with that name; a redeclaration with different fields
    /// therefore wants its own methods too.)
    ///
    /// Declaring the same *user* class twice is an error: that is a mistake, not
    /// an override. Because the namespace spans the compilation, "twice"
    /// includes two different modules — so the error names both files when
    /// `module` (the declaring file's display name) is known.
    ///
    /// A class may not take a **built-in type name** (`int`, `list`, `string`,
    /// …). [`Type::resolve`] puts the built-in vocabulary first, so such a
    /// class could never be reached in type position: the annotation would
    /// keep meaning the primitive while the constructor produced a record, and
    /// the mismatch would print as "expected `int`, found `int`".
    pub fn declare(&mut self, mut def: ClassDef, module: Option<&str>) -> Result<ClassId, String> {
        if let Some(message) = self.name_taken("class", &def.name, module) {
            return Err(message);
        }
        def.name = self.qualify(&def.name);
        if let Some(&builtin) = self.by_name.get(&def.name) {
            self.defs[builtin.0 as usize] = def;
            self.remember_origin(builtin, module);
            return Ok(builtin);
        }
        let id = self.insert(def);
        self.remember_origin(id, module);
        Ok(id)
    }

    fn remember_origin(&mut self, id: ClassId, module: Option<&str>) {
        match module {
            Some(m) => {
                self.origins.insert(id, m.to_string());
            }
            None => {
                self.origins.remove(&id);
            }
        }
    }

    /// Record `fn Class.name(...)` on an already-declared class. Overriding a
    /// *built-in* method replaces it (dispatch prefers user methods, so the
    /// table has to agree); declaring the same *user* method twice is an error.
    pub fn declare_method(
        &mut self,
        id: ClassId,
        name: &str,
        sig: FnSignature,
    ) -> Result<(), String> {
        let def = &mut self.defs[id.0 as usize];
        push_method(&def.name, &mut def.methods, name, sig)
    }

    /// Record `fn Kind.name(...)` on an already-declared enum, under the rules
    /// [`Self::declare_method`] holds a class method to (an enum has no
    /// built-in methods, so a second declaration of one arity is always an
    /// error). The naming rules a method on an enum also has to meet are the
    /// compiler's (`collect_type_decls`), which has the declaration's spans.
    pub fn declare_enum_method(
        &mut self,
        id: EnumId,
        name: &str,
        sig: FnSignature,
    ) -> Result<(), String> {
        let entry = &mut self.enums[id.0 as usize];
        push_method(&entry.name, &mut entry.methods, name, sig)
    }

    /// The methods declared on the enum `id`.
    pub fn enum_methods(&self, id: EnumId) -> &[MethodDef] {
        &self.enums[id.0 as usize].methods
    }

    /// The methods callable on a value of the declared type `ty` — a class's
    /// (built-in ones included) or an enum's — or `None` for any other type.
    /// The one list the checker pins a `recv.m()` site against, whichever
    /// kind of declared type the receiver is.
    pub fn methods_of(&self, ty: Type) -> Option<&[MethodDef]> {
        match ty {
            Type::Class(id) => Some(&self.get(id).methods),
            Type::EnumOf(id) => Some(self.enum_methods(id)),
            _ => None,
        }
    }

    /// The identity of the declared type `ty` (`Rect`, `./level::Phase`): the
    /// name its methods are published under at runtime. `None` for a type
    /// that is not a class or an enum.
    pub fn type_identity(&self, ty: Type) -> Option<&str> {
        match ty {
            Type::Class(id) => Some(self.name_of(id)),
            Type::EnumOf(id) => Some(self.enum_name(id)),
            _ => None,
        }
    }

    /// The class `name` refers to *in the current scope*: a bare name
    /// (`Circle`) or one through a module qualifier (`sh.Circle`). A name the
    /// scope does not carry is `None` even though the table holds it — that
    /// is what keeps a module-private class from resolving as a type in an
    /// importer.
    pub fn lookup(&self, name: &str) -> Option<ClassId> {
        self.by_name.get(&self.resolve_spelling(name)?).copied()
    }

    /// The class whose identity is `identity` (`Rect`, `./shapes::Circle`),
    /// ignoring the current scope. Used where the question is "which class is
    /// this tag?" rather than "can this file see it?".
    pub fn lookup_anywhere(&self, identity: &str) -> Option<ClassId> {
        self.by_name.get(identity).copied()
    }

    /// How the current file spells the class `id`: its bare name, when the
    /// scope reaches it by one. `None` for a class the file cannot name bare.
    pub fn spelling_of(&self, id: ClassId) -> Option<&str> {
        let identity = self.name_of(id);
        match &self.scope {
            None => Some(crate::program::strip_module(identity)),
            Some(scope) => scope
                .iter()
                .find(|(written, to)| *to == identity && !written.contains('.'))
                .map(|(written, _)| written.as_str())
                .or_else(|| {
                    (self.builtin_names.contains(identity) && !scope.contains_key(identity))
                        .then_some(identity)
                }),
        }
    }

    /// How the current file writes the class or enum `identity` in an
    /// annotation: its bare name where the scope reaches it by one, else
    /// `q.Name` through a module qualifier the file binds (`lv.Phase` for
    /// `./level::Phase` under `import ./level as lv`). `None` for a type the
    /// file cannot name, such as another module's private class. Unlike
    /// [`Self::display_name`], which spells an identity for a diagnostic,
    /// this is source text that resolves back to `identity`.
    pub fn written_name(&self, identity: &str) -> Option<String> {
        let Some(scope) = &self.scope else {
            return Some(crate::program::strip_module(identity).to_string());
        };
        let mut bare: Vec<&str> = Vec::new();
        let mut qualified: Vec<&str> = Vec::new();
        for (written, to) in scope {
            if to == identity {
                if written.contains('.') { qualified.push(written) } else { bare.push(written) }
            }
        }
        // Several spellings can reach one type (`import m: Kind` beside
        // `import m as q`); the shortest, then the first in order, is stable.
        let pick = |v: &mut Vec<&str>| {
            v.sort_by_key(|w| (w.len(), *w));
            v.first().map(|w| w.to_string())
        };
        pick(&mut bare)
            .or_else(|| {
                (self.builtin_names.contains(identity) && !scope.contains_key(identity))
                    .then(|| identity.to_string())
            })
            .or_else(|| pick(&mut qualified))
    }

    /// Why `name` cannot be declared as a `kind` (`class` or `enum`) in the
    /// current namespace, if it cannot: it is a built-in type name, or a user
    /// class or an enum of the same module already has it. The two share the
    /// one namespace of declared type names within a module, whichever is
    /// declared first. Another module's `Phase` is a different identity, so
    /// it never collides.
    fn name_taken(&self, kind: &str, name: &str, module: Option<&str>) -> Option<String> {
        // `Foo?` in type position means "a `Foo` or nil", so a type named
        // `Foo?` could never be written (docs/proposals/nullable-types.md).
        if let Some(base) = name.strip_suffix('?') {
            return Some(format!(
                "{kind} `{name}` may not end in `?`: `{base}?` in a type annotation \
                 means \"a `{base}` or nil\". Pick another name"
            ));
        }
        if Type::from_name(name).is_some() {
            return Some(format!(
                "{kind} `{name}` collides with the built-in type name `{name}`: pick another name"
            ));
        }
        let identity = self.qualify(name);
        let class = self.by_name.get(&identity).filter(|id| !self.get(**id).builtin);
        let (taken_by, prev) = match (class, self.enum_by_name.get(&identity)) {
            (Some(id), _) => ("class", self.origins.get(id)),
            (None, Some(id)) => ("enum", self.enums[id.0 as usize].origin.as_ref()),
            (None, None) => return None,
        };
        Some(match (prev, module) {
            _ if taken_by != kind => {
                format!("{kind} `{name}` has the same name as {taken_by} `{name}`: pick another name")
            }
            (Some(prev), Some(now)) if prev != now => format!(
                "{kind} `{name}` is already declared in `{prev}`, so `{now}` may not declare it too"
            ),
            _ => format!("{kind} `{name}` is already declared"),
        })
    }

    /// Declare an enum, under the rules [`Self::declare`] holds a class to.
    /// `span` is the declaring statement.
    pub fn declare_enum(
        &mut self,
        name: &str,
        def: EnumDef,
        module: Option<&str>,
        span: SourceSpan,
    ) -> Result<EnumId, String> {
        if let Some(message) = self.name_taken("enum", name, module) {
            return Err(message);
        }
        let identity = self.qualify(name);
        let id = EnumId(self.enums.len() as u32);
        self.enum_by_name.insert(identity.clone(), id);
        self.enums.push(EnumEntry {
            name: identity,
            def,
            origin: module.map(str::to_string),
            span,
            methods: Vec::new(),
        });
        Ok(id)
    }

    /// The enum whose identity is `identity` (`Phase`, `./level::Phase`),
    /// whatever the current scope.
    pub fn enum_named(&self, identity: &str) -> Option<&EnumDef> {
        self.enum_by_name.get(identity).map(|id| self.enum_def(*id))
    }

    /// Whether the statement at `span` is the declaration of the enum `name`
    /// in the current namespace — false for a redeclaration, which was
    /// refused and declares nothing.
    pub fn is_enum_declaration(&self, name: &str, span: SourceSpan) -> bool {
        self.enum_by_name
            .get(&self.qualify(name))
            .is_some_and(|id| self.enums[id.0 as usize].span == span)
    }

    /// Every enum, by identity, in declaration order.
    pub fn enums(&self) -> impl Iterator<Item = (&str, &EnumDef)> {
        self.enums.iter().map(|e| (e.name.as_str(), &e.def))
    }

    /// The class or enum that `name`, written in type position, refers to in
    /// the current scope. `Foo?` is the nullable `Foo` ([`Type::or_nil`]);
    /// no class or enum may itself be named with a trailing `?`.
    pub fn declared_type(&self, name: &str) -> Option<Type> {
        if let Some(base) = name.strip_suffix('?') {
            return self
                .declared_type(base)
                .filter(|t| !matches!(t, Type::OrNil(_)))
                .map(Type::or_nil);
        }
        if let Some((q, member)) = name.split_once('.') {
            return self.qualified_type(q, member);
        }
        let class = self.lookup(name).map(Type::Class);
        class.or_else(|| self.lookup_enum(name).map(Type::EnumOf))
    }

    /// `q.Name` in type position: the class or enum `Name` that the module
    /// bound to qualifier `q` exports. It names the same type a bare `Name`
    /// would where nothing else claims the bare name
    /// (docs/proposals/module-name-collisions.md, parts A and C).
    fn qualified_type(&self, q: &str, member: &str) -> Option<Type> {
        if !self.qualifiers.get(q)?.exports.contains(member) {
            return None;
        }
        // The scope maps `q.Name` to the export's module-scoped identity
        // (part C), so this is the module's own `Name` even where the file
        // declares or imports another type of that bare name.
        let written = format!("{q}.{member}");
        let class = self.lookup(&written).map(Type::Class);
        class.or_else(|| self.lookup_enum(&written).map(Type::EnumOf))
    }

    /// Why a qualified type name (`sh.Hidden`, `nope.Circle`) resolves to
    /// nothing, for the `unknown type name` warning. `None` for a name that is
    /// not qualified.
    pub fn qualified_miss(&self, name: &str) -> Option<String> {
        let (q, member) = name.trim_end_matches('?').split_once('.')?;
        Some(match self.qualifiers.get(q) {
            None => format!("no import binds the module qualifier `{q}`"),
            Some(qual) if !qual.exports.contains(member) => {
                format!("module '{}' has no export `{member}`", qual.module)
            }
            Some(qual) => format!(
                "`{member}` exported by module '{}' is not a class or enum",
                qual.module
            ),
        })
    }

    /// The enum `name` refers to *in the current scope*: like a class, an
    /// enum the file neither declares nor imports is not a type it can name.
    pub fn lookup_enum(&self, name: &str) -> Option<EnumId> {
        self.enum_by_name.get(&self.resolve_spelling(name)?).copied()
    }

    /// How a diagnostic spells the type `identity`: the bare name, as a file
    /// writes it — unless another class or enum of the compilation shares
    /// that bare name, when "expected `Circle`, found `Circle`" would say
    /// nothing, and the identity (`./shapes::Circle`) is spelled out.
    pub fn display_name<'a>(&self, identity: &'a str) -> &'a str {
        let bare = crate::program::strip_module(identity);
        let shares = |other: &str| other != identity && crate::program::strip_module(other) == bare;
        let shared = self.defs.iter().any(|d| shares(&d.name)) || self.enums.iter().any(|e| shares(&e.name));
        if shared { identity } else { bare }
    }

    pub fn enum_name(&self, id: EnumId) -> &str {
        &self.enums[id.0 as usize].name
    }

    pub fn enum_def(&self, id: EnumId) -> &EnumDef {
        &self.enums[id.0 as usize].def
    }

    pub fn get(&self, id: ClassId) -> &ClassDef {
        &self.defs[id.0 as usize]
    }

    /// The identity of `id` (`Rect`, `./shapes::Circle`): the tag its
    /// instances carry. A diagnostic prints [`crate::program::strip_module`]
    /// of it.
    pub fn name_of(&self, id: ClassId) -> &str {
        &self.defs[id.0 as usize].name
    }

    pub fn iter(&self) -> impl Iterator<Item = (ClassId, &ClassDef)> {
        self.defs
            .iter()
            .enumerate()
            .map(|(i, d)| (ClassId(i as u32), d))
    }
}

/// The `Rect` field names, in constructor order. Shared by the class
/// definition and the native constructor so the two cannot drift.
pub const RECT_FIELDS: [&str; 4] = ["x", "y", "w", "h"];

/// The declared type of a `Rect` field.
///
/// A rect edge is a *number* — `int` for the pixel geometry most UI code
/// writes, `float` for the sub-pixel geometry layout and animation produce.
/// [`Type::Num`] is the type that says exactly that; declaring `int` would be a
/// lie the constructor could only keep by truncating its argument — an implicit
/// cast, which Petal does not do.
///
/// The constructor still checks numeric-ness at runtime, and must: this is a
/// warning-only projection.
const RECT_FIELD_TYPE: Option<Type> = Some(Type::Num);

/// The built-in `Rect` methods as `(name, arity-including-receiver)`. The
/// native implementations are registered under the qualified names
/// `Rect.<name>` in `crate::builtins::classes`; a unit test there asserts this
/// list and that registration agree.
pub const RECT_METHODS: [(&str, usize); 6] = [
    ("center_x", 1),
    ("center_y", 1),
    ("right", 1),
    ("bottom", 1),
    ("inset", 2),
    ("offset", 3),
];

/// `Rect` is the first entry of [`builtin_classes`], so it is always
/// `ClassId(0)`. Naming it lets the built-in method signatures below refer to
/// the class they return before any table exists to look it up in;
/// `the_rect_class_id_is_stable` pins the assumption.
const RECT_CLASS_ID: ClassId = ClassId(0);

/// The declared signature of one built-in `Rect` method, receiver first.
///
/// The four edge accessors return [`Type::Num`] rather than a fixed width
/// because they run the *same* arithmetic the language does
/// (`crate::builtins::classes::arith`): an int rect yields ints — `/` truncates
/// — and a float anywhere yields a float. `num` is precisely that contract.
/// `inset` and `offset` rebuild the rect, so they return `Rect` and chain.
fn rect_method_sig(name: &str, arity: usize) -> FnSignature {
    let recv = Some(Type::Class(RECT_CLASS_ID));
    match name {
        "center_x" | "center_y" | "right" | "bottom" => FnSignature {
            params: vec![recv],
            ret: Some(Type::Num),
            optional: 0,
        },
        // The margin and the deltas go through the same `number()` guard the
        // fields do, so they are `num` for the same reason.
        "inset" | "offset" => FnSignature {
            params: std::iter::once(recv)
                .chain(std::iter::repeat_n(Some(Type::Num), arity - 1))
                .collect(),
            ret: Some(Type::Class(RECT_CLASS_ID)),
            optional: 0,
        },
        // A built-in method with no signature written here constrains nothing,
        // exactly like an un-annotated user method.
        _ => FnSignature::untyped(arity),
    }
}

/// The built-in class named `name`, if there is one: what a user `class` or
/// `enum` of that name would replace (docs/proposals/module-name-collisions.md,
/// part B1).
pub fn builtin_class(name: &str) -> Option<ClassDef> {
    builtin_classes().into_iter().find(|def| def.name == name)
}

/// Every class built into the language. Ordering is load-bearing only in that
/// [`ClassId`]s are positional within one table; nothing serializes them.
fn builtin_classes() -> Vec<ClassDef> {
    vec![ClassDef {
        name: "Rect".to_string(),
        fields: RECT_FIELDS
            .iter()
            .map(|f| ClassField {
                name: f.to_string(),
                ty: RECT_FIELD_TYPE,
            })
            .collect(),
        methods: RECT_METHODS
            .iter()
            .map(|(name, arity)| MethodDef {
                name: name.to_string(),
                arity: *arity,
                sig: rect_method_sig(name, *arity),
                builtin: true,
            })
            .collect(),
        builtin: true,
    }]
}

/// Add `name` to `methods`, the list of the declared type `owner`: a user
/// declaration replaces a built-in method of the same name and arity, and a
/// second user declaration of one is an error.
fn push_method(
    owner: &str,
    methods: &mut Vec<MethodDef>,
    name: &str,
    sig: FnSignature,
) -> Result<(), String> {
    let arity = sig.params.len();
    let new = MethodDef {
        name: name.to_string(),
        arity,
        sig,
        builtin: false,
    };
    if let Some(existing) = methods.iter_mut().find(|m| m.name == name && m.arity == arity) {
        if !existing.builtin {
            return Err(format!(
                "method `{}.{}` is already declared with {} parameter{}",
                owner,
                name,
                arity,
                if arity == 1 { "" } else { "s" }
            ));
        }
        *existing = new;
        return Ok(());
    }
    methods.push(new);
    Ok(())
}

/// The builtin the compiler emits to publish `fn Class.method` at runtime.
/// Underscore-prefixed and never documented as callable — it is an internal
/// declaration form, not an API. The VM intercepts it (see the native table's
/// `intrinsic_declare_method`).
pub const DECLARE_METHOD_BUILTIN: &str = "__declare_method";

/// The name a built-in class method's native is registered under: `Rect.inset`.
/// Dotted, so it is unreachable as a bare identifier from Petal source and can
/// only be found through method dispatch.
pub fn qualified_method_name(class: &str, method: &str) -> String {
    format!("{class}.{method}")
}

/// The `(class, method)` a qualified method name spells, or `None` for an
/// ordinary function name. The inverse of [`qualified_method_name`], and the
/// one test for "does this function's first parameter come from a receiver the
/// call site supplied implicitly?" — a module-qualified function uses `::`, so
/// the dot is unambiguous. The class half may carry its module
/// (`./shapes::Circle.area`, whose module path may itself hold dots); only
/// the part after the last `::` must be a plain name.
pub fn split_qualified_method_name(name: &str) -> Option<(&str, &str)> {
    let (class, method) = name.rsplit_once('.')?;
    let bare = crate::program::strip_module(class);
    if bare.is_empty() || method.is_empty() || bare.contains('.') || method.contains(':') {
        return None;
    }
    Some((class, method))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_class(name: &str, fields: &[&str]) -> ClassDef {
        ClassDef {
            name: name.to_string(),
            fields: fields
                .iter()
                .map(|f| ClassField {
                    name: f.to_string(),
                    ty: None,
                })
                .collect(),
            methods: Vec::new(),
            builtin: false,
        }
    }

    #[test]
    fn rect_is_present_without_declaration() {
        let t = ClassTable::new();
        let id = t.lookup("Rect").expect("Rect is built in");
        let def = t.get(id);
        assert_eq!(
            def.fields
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            RECT_FIELDS.to_vec()
        );
        assert!(def.builtin);
    }

    /// [`RECT_CLASS_ID`] is hardcoded so the built-in method signatures can
    /// name the class they return before a table exists. Pin the assumption
    /// that made it safe: `Rect` is the first built-in class.
    #[test]
    fn the_rect_class_id_is_stable() {
        let t = ClassTable::new();
        assert_eq!(t.lookup("Rect"), Some(RECT_CLASS_ID));
        assert_eq!(t.get(RECT_CLASS_ID).name, "Rect");
    }

    /// Each built-in method's signature has one slot per parameter, receiver
    /// included, so `arity` and `sig` cannot drift apart.
    #[test]
    fn builtin_method_signatures_match_their_arity() {
        let t = ClassTable::new();
        let def = t.get(t.lookup("Rect").unwrap());
        for m in &def.methods {
            assert_eq!(m.sig.params.len(), m.arity, "method `{}`", m.name);
            assert_eq!(
                m.sig.params[0],
                Some(Type::Class(RECT_CLASS_ID)),
                "method `{}` receiver",
                m.name
            );
        }
    }

    /// A rect edge may be an int or a float, so the fields are declared `num` —
    /// the type that says "either" — and never `int`, which the constructor
    /// could only honour by truncating (see [`RECT_FIELD_TYPE`]).
    #[test]
    fn rect_fields_are_declared_num_never_int() {
        let t = ClassTable::new();
        let def = t.get(t.lookup("Rect").unwrap());
        for f in &def.fields {
            assert_eq!(f.ty, Some(Type::Num), "field `{}`", f.name);
        }
    }

    #[test]
    fn rect_declares_its_builtin_methods() {
        let t = ClassTable::new();
        let def = t.get(t.lookup("Rect").unwrap());
        for (name, arity) in RECT_METHODS {
            let m = def.method(name).unwrap_or_else(|| panic!("Rect.{name}"));
            assert_eq!(m.arity, arity);
        }
    }

    #[test]
    fn declaring_a_class_twice_is_an_error() {
        let mut t = ClassTable::new();
        t.declare(user_class("Point", &["x"]), None).expect("first");
        let err = t.declare(user_class("Point", &["y"]), None).unwrap_err();
        assert!(err.contains("already declared"), "{err}");
    }

    /// A user `class Rect … end` shadows the built-in rather than colliding
    /// with it, so a program is free to spell out a class it could have got for
    /// free. The id is stable, so an annotation resolved before the
    /// redeclaration still points at the right entry.
    #[test]
    fn redeclaring_a_builtin_class_replaces_it() {
        let mut t = ClassTable::new();
        let builtin = t.lookup("Rect").unwrap();
        let id = t
            .declare(user_class("Rect", &["left", "top"]), None)
            .unwrap();
        assert_eq!(id, builtin);
        let def = t.get(id);
        assert!(!def.builtin);
        assert!(def.field("left").is_some());
        assert!(def.field("w").is_none(), "the built-in fields are gone");
        // And redeclaring *that* is now an ordinary duplicate.
        let err = t.declare(user_class("Rect", &["x"]), None).unwrap_err();
        assert!(err.contains("already declared"), "{err}");
    }

    /// A user method may override a built-in one, matching what dispatch does.
    #[test]
    fn a_user_method_overrides_a_builtin_method() {
        let mut t = ClassTable::new();
        let rect = t.lookup("Rect").unwrap();
        t.declare_method(rect, "center_x", FnSignature::untyped(1))
            .expect("override");
        let def = t.get(rect);
        let m = def.method("center_x").unwrap();
        assert!(!m.builtin);
        assert_eq!(
            def.methods.iter().filter(|m| m.name == "center_x").count(),
            1,
            "the built-in entry was replaced, not duplicated"
        );
        // Doing it twice is now an ordinary duplicate.
        assert!(
            t.declare_method(rect, "center_x", FnSignature::untyped(1))
                .is_err()
        );
    }

    #[test]
    fn duplicate_method_of_the_same_arity_is_an_error() {
        let mut t = ClassTable::new();
        let id = t.declare(user_class("Point", &["x"]), None).unwrap();
        t.declare_method(id, "shifted", FnSignature::untyped(2))
            .expect("first");
        // A different arity is a legal overload.
        t.declare_method(id, "shifted", FnSignature::untyped(3))
            .expect("overload");
        let err = t
            .declare_method(id, "shifted", FnSignature::untyped(2))
            .unwrap_err();
        assert!(err.contains("already declared"), "{err}");
    }

    #[test]
    fn qualified_names_are_dotted() {
        assert_eq!(qualified_method_name("Rect", "inset"), "Rect.inset");
    }
}
