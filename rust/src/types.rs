//! Static type representation for optional type declarations.
//!
//! See docs/dev/type-declarations-plan.md. This is a compile-time-only notion
//! layered on top of the dynamically-typed runtime `Value` (see `crate::value`);
//! it does not appear in the serialized IR, but it *does* appear in the
//! serialized AST (`show-ast --json`), so it derives `Serialize`. `Any` is the
//! dynamic escape hatch: it is compatible with every type in both directions and
//! suppresses checking.

use std::borrow::Cow;

use serde::Serialize;

use crate::classes::{ClassId, ClassTable};

/// A declared or inferred static type.
///
/// The concrete variants mirror the runtime type tags reported by
/// [`Value::type_name`](crate::value::Value::type_name) — [`Type::name`] returns
/// exactly those strings — so "the name you see at runtime is the name you write
/// in an annotation". Parameterized forms (element types, arrow types,
/// structural records) are intentionally deferred; see the plan.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize)]
pub enum Type {
    /// The dynamic type. Compatible with everything; suppresses checking.
    Any,
    Nil,
    Bool,
    Int,
    Float,
    String,
    List,
    /// A `Map` value at runtime (`type_name` == "record").
    Record,
    /// Any callable (closure, overload set, or native fn).
    Function,
    Enum,
    Vec2,
    F64Array,
    Element,
    Symbol,
    Dual,
    Handle,
    Pending,
    /// An instance of a declared `class` — a record carrying that class's tag.
    /// The payload indexes the [`ClassTable`](crate::classes::ClassTable) of the
    /// compilation that produced it, which is why printing one needs that table
    /// ([`Type::display`]) and [`Type::name`] cannot spell it.
    Class(ClassId),
}

impl Type {
    /// Parse a type-position identifier into a [`Type`].
    ///
    /// Names match [`Type::name`] (which mirrors `Value::type_name`), plus:
    /// - `"any"` → [`Type::Any`]
    /// - `"str"` is accepted as an alias for `"string"` (the cast builtin is
    ///   `str()` while the runtime type name is `string`).
    ///
    /// Returns `None` for an unknown name; a class name is *also* `None` here,
    /// because resolving one needs the compilation's
    /// [`ClassTable`](crate::classes::ClassTable) — see [`Type::resolve`], the
    /// context-aware form the checker uses. An unresolved name becomes a
    /// warning and is treated as `Any`.
    pub fn from_name(name: &str) -> Option<Type> {
        let ty = match name {
            "any" => Type::Any,
            "nil" => Type::Nil,
            "bool" => Type::Bool,
            "int" => Type::Int,
            "float" => Type::Float,
            "string" | "str" => Type::String,
            "list" => Type::List,
            "record" => Type::Record,
            "function" => Type::Function,
            "enum" => Type::Enum,
            "vec2" => Type::Vec2,
            "f64_array" => Type::F64Array,
            "element" => Type::Element,
            "symbol" => Type::Symbol,
            "dual" => Type::Dual,
            "handle" => Type::Handle,
            "pending" => Type::Pending,
            _ => return None,
        };
        Some(ty)
    }

    /// Resolve a type-position name against a class table: the built-in
    /// vocabulary first (so a class may not shadow `int`), then declared
    /// classes. This is what turns `rect: Rect` into [`Type::Class`].
    pub fn resolve(name: &str, classes: &ClassTable) -> Option<Type> {
        Type::from_name(name).or_else(|| classes.lookup(name).map(Type::Class))
    }

    /// How this type is spelled in a diagnostic. Identical to [`Type::name`]
    /// except for [`Type::Class`], whose name lives in `classes`.
    pub fn display<'a>(&self, classes: &'a ClassTable) -> Cow<'a, str> {
        match self {
            Type::Class(id) => Cow::Borrowed(classes.name_of(*id)),
            other => Cow::Borrowed(other.name()),
        }
    }

    /// The canonical spelling of this type. For every concrete variant this
    /// equals the corresponding [`Value::type_name`](crate::value::Value::type_name);
    /// [`Type::Any`] spells `"any"`.
    ///
    /// [`Type::Class`] has no static spelling — its name lives in the
    /// [`ClassTable`](crate::classes::ClassTable) — so it reports `"class"`
    /// here. Diagnostics should use [`Type::display`], which prints the real
    /// name.
    pub fn name(&self) -> &'static str {
        match self {
            Type::Any => "any",
            Type::Nil => "nil",
            Type::Bool => "bool",
            Type::Int => "int",
            Type::Float => "float",
            Type::String => "string",
            Type::List => "list",
            Type::Record => "record",
            Type::Function => "function",
            Type::Enum => "enum",
            Type::Vec2 => "vec2",
            Type::F64Array => "f64_array",
            Type::Element => "element",
            Type::Symbol => "symbol",
            Type::Dual => "dual",
            Type::Handle => "handle",
            Type::Pending => "pending",
            Type::Class(_) => "class",
        }
    }

    /// Whether a value of `self` may be used where `other` is expected.
    ///
    /// Warning-only semantics (see the plan): a `false` result is a diagnostic,
    /// never a hard error. Rules:
    /// - [`Type::Any`] on either side is always compatible (the dynamic ↔ static
    ///   boundary is trusted).
    /// - `int` is assignable to a `float` slot (documented numeric promotion),
    ///   but `float` is **not** assignable to `int` — that needs an explicit
    ///   `int()` cast (no implicit casting).
    /// - A class instance *is* a record at runtime, so `Class(_)` satisfies a
    ///   `record` slot. The reverse is not true: a bare `{x: 1}` carries no
    ///   class tag, so it cannot fill a `Rect` slot.
    /// - Otherwise the types must be equal — two distinct classes are never
    ///   interchangeable, however alike their fields.
    pub fn is_assignable_to(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Any, _) | (_, Type::Any) => true,
            (Type::Int, Type::Float) => true,
            (Type::Class(_), Type::Record) => true,
            _ => self == other,
        }
    }
}

/// The declared static signature of a named function.
///
/// Collected during the compiler's prescan (so forward references resolve) and
/// consulted by the type checker at call sites. Compile-time only — never
/// serialized into the IR. Elsewhere these are keyed by `(name, arity)` so
/// arity-based overloading (`Function_Overloading.md`) keeps distinct entries.
///
/// Each slot holds the *resolved* [`Type`], or `None` when the corresponding
/// annotation was absent or unrecognized — either way the checker treats it as
/// [`Type::Any`] (no check at that position).
#[derive(Clone, Debug, PartialEq)]
pub struct FnSignature {
    /// Declared parameter types, positionally.
    pub params: Vec<Option<Type>>,
    /// Declared return type.
    pub ret: Option<Type>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    /// Every concrete (non-`Any`) type and the runtime value that reports it, so
    /// we can assert `Type::name` stays in lockstep with `Value::type_name`.
    fn concrete_types() -> Vec<Type> {
        vec![
            Type::Nil,
            Type::Bool,
            Type::Int,
            Type::Float,
            Type::String,
            Type::List,
            Type::Record,
            Type::Function,
            Type::Enum,
            Type::Vec2,
            Type::F64Array,
            Type::Element,
            Type::Symbol,
            Type::Dual,
            Type::Handle,
            Type::Pending,
        ]
    }

    #[test]
    fn from_name_round_trips_every_type() {
        for ty in concrete_types() {
            assert_eq!(Type::from_name(ty.name()), Some(ty), "round-trip {ty:?}");
        }
        assert_eq!(Type::from_name("any"), Some(Type::Any));
    }

    #[test]
    fn from_name_accepts_str_alias_for_string() {
        assert_eq!(Type::from_name("str"), Some(Type::String));
        assert_eq!(Type::from_name("string"), Some(Type::String));
    }

    #[test]
    fn from_name_unknown_is_none() {
        assert_eq!(Type::from_name("banana"), None);
        assert_eq!(Type::from_name(""), None);
        assert_eq!(Type::from_name("Int"), None); // case-sensitive, lowercase only
    }

    #[test]
    fn name_matches_value_type_name_for_concretes() {
        // Representative runtime values whose type_name must equal Type::name.
        let cases: &[(Type, &'static str)] = &[
            (Type::Nil, Value::Nil.type_name()),
            (Type::Bool, Value::Bool(true).type_name()),
            (Type::Int, Value::Int(1).type_name()),
            (Type::Float, Value::Float(1.0).type_name()),
            (Type::Vec2, Value::Vec2(0.0, 0.0).type_name()),
            (
                Type::Dual,
                Value::Dual {
                    value: 0.0,
                    derivative: 0.0,
                }
                .type_name(),
            ),
        ];
        for (ty, runtime_name) in cases {
            assert_eq!(ty.name(), *runtime_name, "{ty:?}");
        }
        // Names not easily constructed above are still asserted by string:
        assert_eq!(Type::String.name(), "string");
        assert_eq!(Type::List.name(), "list");
        assert_eq!(Type::Record.name(), "record"); // Map -> "record"
        assert_eq!(Type::Function.name(), "function"); // callables collapse
        assert_eq!(Type::Enum.name(), "enum");
        assert_eq!(Type::F64Array.name(), "f64_array");
        assert_eq!(Type::Element.name(), "element");
        assert_eq!(Type::Symbol.name(), "symbol");
        assert_eq!(Type::Handle.name(), "handle");
        assert_eq!(Type::Pending.name(), "pending");
    }

    #[test]
    fn any_is_assignable_in_both_directions() {
        for ty in concrete_types() {
            assert!(Type::Any.is_assignable_to(&ty), "any -> {ty:?}");
            assert!(ty.is_assignable_to(&Type::Any), "{ty:?} -> any");
        }
        assert!(Type::Any.is_assignable_to(&Type::Any));
    }

    #[test]
    fn int_promotes_to_float_but_not_the_reverse() {
        assert!(Type::Int.is_assignable_to(&Type::Float));
        assert!(!Type::Float.is_assignable_to(&Type::Int));
    }

    #[test]
    fn equal_concretes_are_assignable() {
        for ty in concrete_types() {
            assert!(ty.is_assignable_to(&ty), "{ty:?} -> {ty:?}");
        }
    }

    #[test]
    fn class_names_resolve_only_through_the_class_table() {
        let classes = crate::classes::ClassTable::new();
        assert_eq!(Type::from_name("Rect"), None);
        let rect = classes.lookup("Rect").unwrap();
        assert_eq!(Type::resolve("Rect", &classes), Some(Type::Class(rect)));
        assert_eq!(Type::resolve("int", &classes), Some(Type::Int));
        assert_eq!(Type::resolve("Nope", &classes), None);
    }

    #[test]
    fn a_class_displays_its_declared_name() {
        let classes = crate::classes::ClassTable::new();
        let rect = Type::Class(classes.lookup("Rect").unwrap());
        assert_eq!(rect.display(&classes), "Rect");
        assert_eq!(Type::Int.display(&classes), "int");
    }

    #[test]
    fn a_class_instance_is_a_record_but_not_the_reverse() {
        let classes = crate::classes::ClassTable::new();
        let rect = Type::Class(classes.lookup("Rect").unwrap());
        assert!(rect.is_assignable_to(&Type::Record));
        assert!(!Type::Record.is_assignable_to(&rect));
        assert!(rect.is_assignable_to(&rect));
        assert!(!Type::Int.is_assignable_to(&rect));
    }

    #[test]
    fn distinct_classes_are_not_interchangeable() {
        let mut classes = crate::classes::ClassTable::new();
        let a = classes
            .declare(crate::classes::ClassDef {
                name: "A".into(),
                fields: vec![],
                methods: vec![],
                builtin: false,
            })
            .unwrap();
        let b = classes
            .declare(crate::classes::ClassDef {
                name: "B".into(),
                fields: vec![],
                methods: vec![],
                builtin: false,
            })
            .unwrap();
        assert!(!Type::Class(a).is_assignable_to(&Type::Class(b)));
        assert!(Type::Class(a).is_assignable_to(&Type::Class(a)));
    }

    #[test]
    fn mismatched_concretes_are_not_assignable() {
        assert!(!Type::String.is_assignable_to(&Type::Int));
        assert!(!Type::Bool.is_assignable_to(&Type::Int));
        assert!(!Type::List.is_assignable_to(&Type::Record));
        assert!(!Type::Record.is_assignable_to(&Type::List));
        assert!(!Type::Int.is_assignable_to(&Type::String));
    }
}
