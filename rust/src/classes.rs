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

use crate::types::{FnSignature, Type};

/// Index of a class in a [`ClassTable`]. Small and `Copy` so [`Type`] stays
/// `Copy`; the name and fields live in the table entry it points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ClassId(pub u32);

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

/// The classes of one compilation, by name.
///
/// The *namespace* spans the compilation — a [`ClassId`] means the same thing
/// in every module, and two modules may not declare the same class name — but
/// what a given file can *see* is narrower: a class name resolves only where
/// its constructor does. [`ClassTable::set_scope`] installs that per-file view
/// and [`ClassTable::lookup`] respects it, so a module-private class is not a
/// type name in an importer either (built-in classes are always in scope).
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
    /// The class names in scope for the file currently being compiled, or
    /// `None` for "every declared class" — the default, which is what a
    /// single-file compilation and the unit tests want.
    scope: Option<HashSet<String>>,
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
        };
        for def in builtin_classes() {
            table.builtin_names.insert(def.name.clone());
            table.insert(def);
        }
        table
    }

    /// Restrict [`ClassTable::lookup`] to `names` (plus the built-in classes,
    /// which no file has to import). Called once per module, with the class
    /// names that module declares or imports — see
    /// `Compiler::visible_class_names`.
    pub fn set_scope(&mut self, names: HashSet<String>) {
        self.scope = Some(names);
    }

    /// Drop the per-file view: every declared class resolves again.
    pub fn clear_scope(&mut self) {
        self.scope = None;
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
    pub fn declare(&mut self, def: ClassDef, module: Option<&str>) -> Result<ClassId, String> {
        if Type::from_name(&def.name).is_some() {
            return Err(format!(
                "class `{}` collides with the built-in type name `{}`: pick another name",
                def.name, def.name
            ));
        }
        if let Some(&existing) = self.by_name.get(&def.name) {
            if !self.defs[existing.0 as usize].builtin {
                return Err(match (self.origins.get(&existing), module) {
                    (Some(prev), Some(now)) if prev != now => format!(
                        "class `{}` is already declared in `{prev}`, so `{now}` may not \
                         declare it too",
                        def.name
                    ),
                    _ => format!("class `{}` is already declared", def.name),
                });
            }
            self.defs[existing.0 as usize] = def;
            self.remember_origin(existing, module);
            return Ok(existing);
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
        let arity = sig.params.len();
        let new = MethodDef {
            name: name.to_string(),
            arity,
            sig,
            builtin: false,
        };
        if let Some(existing) = def
            .methods
            .iter_mut()
            .find(|m| m.name == name && m.arity == arity)
        {
            if !existing.builtin {
                return Err(format!(
                    "method `{}.{}` is already declared with {} parameter{}",
                    def.name,
                    name,
                    arity,
                    if arity == 1 { "" } else { "s" }
                ));
            }
            *existing = new;
            return Ok(());
        }
        def.methods.push(new);
        Ok(())
    }

    /// The class `name` refers to *in the current scope*. A name the scope
    /// does not carry is `None` even though the table holds it — that is what
    /// keeps a module-private class from resolving as a type in an importer.
    pub fn lookup(&self, name: &str) -> Option<ClassId> {
        let id = self.by_name.get(name).copied()?;
        match &self.scope {
            Some(scope) if !scope.contains(name) && !self.builtin_names.contains(name) => None,
            _ => Some(id),
        }
    }

    /// The class `name` refers to anywhere in the compilation, ignoring the
    /// current scope. Used where the question is "does this name already
    /// belong to a class?" rather than "can this file see it?".
    pub fn lookup_anywhere(&self, name: &str) -> Option<ClassId> {
        self.by_name.get(name).copied()
    }

    pub fn get(&self, id: ClassId) -> &ClassDef {
        &self.defs[id.0 as usize]
    }

    /// The declared name of `id` — what a diagnostic prints for
    /// [`Type::Class`](crate::types::Type::Class).
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
/// warning-only projection, and it is deliberately the wider of the two checks
/// (a `dual` satisfies `num` but not the runtime guard — see
/// [`Type::is_assignable_to`]).
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
        },
        // The margin and the deltas go through the same `number()` guard the
        // fields do, so they are `num` for the same reason.
        "inset" | "offset" => FnSignature {
            params: std::iter::once(recv)
                .chain(std::iter::repeat_n(Some(Type::Num), arity - 1))
                .collect(),
            ret: Some(Type::Class(RECT_CLASS_ID)),
        },
        // A built-in method with no signature written here constrains nothing,
        // exactly like an un-annotated user method.
        _ => FnSignature {
            params: vec![None; arity],
            ret: None,
        },
    }
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
/// the dot is unambiguous.
pub fn split_qualified_method_name(name: &str) -> Option<(&str, &str)> {
    let (class, method) = name.split_once('.')?;
    if class.is_empty() || method.is_empty() || method.contains('.') {
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
