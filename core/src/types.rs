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

use crate::classes::{ClassId, ClassTable, EnumId};

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
    /// `int` **or** `float` — the numeric contract most arithmetic actually
    /// has. Unlike the other variants this has no runtime tag: no `Value`
    /// reports `"num"`, and nothing infers it. It exists to be *written*, for
    /// the very common case of a slot that legitimately takes either width and
    /// that "no implicit casting" forbids from picking one (the built-in
    /// `Rect`'s edges are the motivating case — see `crate::classes`).
    Num,
    String,
    List,
    /// A `Value::Map` at runtime (`type_name` == "record").
    Record,
    /// A `map` built by `Map()`: `Value::Dict` at runtime (`type_name` ==
    /// "map").
    Map,
    /// Any callable (closure, overload set, or native fn).
    Function,
    Enum,
    Vec2,
    Vec3,
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
    /// A variant of one declared `enum` — any variant of it. Like
    /// [`Type::Class`], the payload indexes the compilation's
    /// [`ClassTable`](crate::classes::ClassTable), which holds the enum's name
    /// and shape. [`Type::Enum`] stays the type of a variant of *some* enum.
    EnumOf(EnumId),
    /// `T?`: a `T` or `nil` (docs/proposals/nullable-types.md, phase 1). The
    /// base is a [`NonNil`], so neither `nil?`, `any?` nor `T??` can be
    /// represented — [`Type::or_nil`] normalizes the first two away.
    ///
    /// Phase 1 is annotation-only: `nil` and `T` both fill a `T?` slot, and a
    /// `T?` value fills a `T` slot silently, because nothing yet tracks where
    /// nil can flow. Nothing infers `T?` either; it only comes from what an
    /// author wrote (and from `petal suggest`'s widening).
    OrNil(NonNil),
}

/// The base of a nullable type [`Type::OrNil`]: every [`Type`] except `any`
/// and `nil` (which already include `nil`) and `T?` itself.
///
/// A separate `Copy` enum rather than a boxed or interned `Type`, so that
/// [`Type`] stays `Copy` and needs no table to compare or assign. The
/// variants mirror [`Type`]'s one for one; [`NonNil::ty`] and
/// [`Type::non_nil`] convert, and a test pins the round trip.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize)]
pub enum NonNil {
    Bool,
    Int,
    Float,
    Num,
    String,
    List,
    Record,
    Map,
    Function,
    Enum,
    Vec2,
    Vec3,
    F64Array,
    Element,
    Symbol,
    Dual,
    Handle,
    Pending,
    Class(ClassId),
    EnumOf(EnumId),
}

impl NonNil {
    /// The base as a plain [`Type`].
    pub fn ty(self) -> Type {
        match self {
            NonNil::Bool => Type::Bool,
            NonNil::Int => Type::Int,
            NonNil::Float => Type::Float,
            NonNil::Num => Type::Num,
            NonNil::String => Type::String,
            NonNil::List => Type::List,
            NonNil::Record => Type::Record,
            NonNil::Map => Type::Map,
            NonNil::Function => Type::Function,
            NonNil::Enum => Type::Enum,
            NonNil::Vec2 => Type::Vec2,
            NonNil::Vec3 => Type::Vec3,
            NonNil::F64Array => Type::F64Array,
            NonNil::Element => Type::Element,
            NonNil::Symbol => Type::Symbol,
            NonNil::Dual => Type::Dual,
            NonNil::Handle => Type::Handle,
            NonNil::Pending => Type::Pending,
            NonNil::Class(id) => Type::Class(id),
            NonNil::EnumOf(id) => Type::EnumOf(id),
        }
    }

    /// [`Type::name`] of `T?`: the base's name with a `?`.
    fn nullable_name(self) -> &'static str {
        match self {
            NonNil::Bool => "bool?",
            NonNil::Int => "int?",
            NonNil::Float => "float?",
            NonNil::Num => "num?",
            NonNil::String => "string?",
            NonNil::List => "list?",
            NonNil::Record => "record?",
            NonNil::Map => "map?",
            NonNil::Function => "function?",
            NonNil::Enum | NonNil::EnumOf(_) => "enum?",
            NonNil::Vec2 => "vec2?",
            NonNil::Vec3 => "vec3?",
            NonNil::F64Array => "f64_array?",
            NonNil::Element => "element?",
            NonNil::Symbol => "symbol?",
            NonNil::Dual => "dual?",
            NonNil::Handle => "handle?",
            NonNil::Pending => "pending?",
            NonNil::Class(_) => "class?",
        }
    }
}

impl Type {
    /// Parse a type-position identifier into a [`Type`].
    ///
    /// Names match [`Type::name`] (which mirrors `Value::type_name`), plus:
    /// - `"any"` → [`Type::Any`]
    /// - `"str"` is accepted as an alias for `"string"` (the cast builtin is
    ///   `str()` while the runtime type name is `string`).
    ///
    /// - a trailing `?` makes the type nullable ([`Type::or_nil`]): `int?` →
    ///   `Type::OrNil(NonNil::Int)`. `nil?` and `any?` normalize to `nil` and
    ///   `any` (the checker warns on both spellings).
    ///
    /// Returns `None` for an unknown name; a class name is *also* `None` here,
    /// because resolving one needs the compilation's
    /// [`ClassTable`](crate::classes::ClassTable) — see [`Type::resolve`], the
    /// context-aware form the checker uses. An unresolved name becomes a
    /// warning and is treated as `Any`.
    pub fn from_name(name: &str) -> Option<Type> {
        if let Some(base) = name.strip_suffix('?') {
            return Type::from_name(base).and_then(Type::nullable);
        }
        let ty = match name {
            "any" => Type::Any,
            "nil" => Type::Nil,
            "bool" => Type::Bool,
            "int" => Type::Int,
            "float" => Type::Float,
            "num" => Type::Num,
            "string" | "str" => Type::String,
            "list" => Type::List,
            "record" => Type::Record,
            "map" => Type::Map,
            "function" => Type::Function,
            "enum" => Type::Enum,
            "vec2" => Type::Vec2,
            "vec3" => Type::Vec3,
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
    /// classes, then declared enums. This is what turns `rect: Rect` into
    /// [`Type::Class`] and `k: Kind` into [`Type::EnumOf`].
    pub fn resolve(name: &str, classes: &ClassTable) -> Option<Type> {
        Type::from_name(name).or_else(|| classes.declared_type(name))
    }

    /// `T?` for a written `T?` base: `None` when `self` is itself already
    /// nullable, so that `int??` (which cannot be lexed as one name anyway)
    /// never resolves. `nil` and `any` stay themselves — see [`Type::or_nil`].
    fn nullable(self) -> Option<Type> {
        match self {
            Type::OrNil(_) => None,
            other => Some(other.or_nil()),
        }
    }

    /// `T?`: this type or `nil`. `nil`, `any` and an already-nullable type
    /// include `nil` already and are returned unchanged.
    pub fn or_nil(self) -> Type {
        match self.non_nil() {
            Some(base) => Type::OrNil(base),
            None => self,
        }
    }

    /// The base of a `T?` (`T`); any other type unchanged. A phase-1 `T?` is
    /// read as its base wherever a value's shape matters (a field read, a
    /// builtin's argument), because nil is not tracked yet.
    pub fn strip_nil(self) -> Type {
        match self {
            Type::OrNil(base) => base.ty(),
            other => other,
        }
    }

    /// This type as a nullable base: `None` for `any`, `nil` and `T?`.
    pub fn non_nil(self) -> Option<NonNil> {
        Some(match self {
            Type::Any | Type::Nil | Type::OrNil(_) => return None,
            Type::Bool => NonNil::Bool,
            Type::Int => NonNil::Int,
            Type::Float => NonNil::Float,
            Type::Num => NonNil::Num,
            Type::String => NonNil::String,
            Type::List => NonNil::List,
            Type::Record => NonNil::Record,
            Type::Map => NonNil::Map,
            Type::Function => NonNil::Function,
            Type::Enum => NonNil::Enum,
            Type::Vec2 => NonNil::Vec2,
            Type::Vec3 => NonNil::Vec3,
            Type::F64Array => NonNil::F64Array,
            Type::Element => NonNil::Element,
            Type::Symbol => NonNil::Symbol,
            Type::Dual => NonNil::Dual,
            Type::Handle => NonNil::Handle,
            Type::Pending => NonNil::Pending,
            Type::Class(id) => NonNil::Class(id),
            Type::EnumOf(id) => NonNil::EnumOf(id),
        })
    }

    /// How this type is spelled in a diagnostic. Identical to [`Type::name`]
    /// except for [`Type::Class`] and [`Type::EnumOf`], whose names live in
    /// `classes` (and `T?` of either).
    pub fn display<'a>(&self, classes: &'a ClassTable) -> Cow<'a, str> {
        match self {
            Type::OrNil(base @ (NonNil::Class(_) | NonNil::EnumOf(_))) => {
                Cow::Owned(format!("{}?", base.ty().display(classes)))
            }
            Type::Class(id) => Cow::Borrowed(classes.name_of(*id)),
            Type::EnumOf(id) => Cow::Borrowed(classes.enum_name(*id)),
            other => Cow::Borrowed(other.name()),
        }
    }

    /// The canonical spelling of this type. For every variant with a runtime
    /// tag this equals the corresponding
    /// [`Value::type_name`](crate::value::Value::type_name); [`Type::Any`]
    /// spells `"any"` and [`Type::Num`] spells `"num"`, neither of which any
    /// `Value` reports.
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
            Type::Num => "num",
            Type::String => "string",
            Type::List => "list",
            Type::Record => "record",
            Type::Map => "map",
            Type::Function => "function",
            Type::Enum => "enum",
            Type::Vec2 => "vec2",
            Type::Vec3 => "vec3",
            Type::F64Array => "f64_array",
            Type::Element => "element",
            Type::Symbol => "symbol",
            Type::Dual => "dual",
            Type::Handle => "handle",
            Type::Pending => "pending",
            Type::Class(_) => "class",
            Type::EnumOf(_) => "enum",
            Type::OrNil(base) => base.nullable_name(),
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
    /// - `int`, `float` and `dual` all satisfy a [`Type::Num`] slot, and `num`
    ///   satisfies **none** of them: it is a widening only, and narrowing back
    ///   needs an explicit cast for the same reason `float`→`int` does.
    ///   `dual` is included deliberately — a dual is a number every arithmetic
    ///   path accepts, so rejecting one would warn on working autodiff code,
    ///   which is the outcome this pass exists to avoid. The runtime `number()`
    ///   guard on the built-in `Rect` is narrower (int/float only), so a rect
    ///   built from duals is a miss here rather than a false alarm — the
    ///   direction this checker always errs in.
    /// - A class instance *is* a record at runtime, so `Class(_)` satisfies a
    ///   `record` slot. The reverse is not true: a bare `{x: 1}` carries no
    ///   class tag, so it cannot fill a `Rect` slot.
    /// - A variant of a declared enum satisfies an `enum` slot, and — the
    ///   dynamic boundary again — an `enum` of unknown declaration is trusted
    ///   to fill a declared enum's slot. Two declared enums never mix.
    /// - `nil` fills a `T?` slot, and so does anything that fills `T`. A `T?`
    ///   value fills whatever its `T` fills — including `T` itself: phase 1 of
    ///   docs/proposals/nullable-types.md tracks no nil flow, so it cannot
    ///   tell that a `T?` *is* nil here, and stays silent rather than guess.
    /// - Otherwise the types must be equal — two distinct classes are never
    ///   interchangeable, however alike their fields.
    pub fn is_assignable_to(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Any, _) | (_, Type::Any) => true,
            (Type::Nil, Type::OrNil(_)) => true,
            (Type::OrNil(base), _) => base.ty().is_assignable_to(other),
            (_, Type::OrNil(base)) => self.is_assignable_to(&base.ty()),
            (Type::Int, Type::Float) => true,
            (Type::Int | Type::Float | Type::Dual, Type::Num) => true,
            (Type::Class(_), Type::Record) => true,
            (Type::EnumOf(_), Type::Enum) | (Type::Enum, Type::EnumOf(_)) => true,
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
    /// How many trailing parameters have a default value, and so may be left
    /// out of a call.
    pub optional: usize,
}

impl FnSignature {
    /// The signature of a wholly un-annotated callable of `arity` parameters:
    /// every slot `None`, no return type. Constrains nothing — the checker
    /// reads each `None` as [`Type::Any`] — so it is the honest stand-in
    /// wherever a signature is structurally required but none was written.
    pub fn untyped(arity: usize) -> FnSignature {
        FnSignature {
            params: vec![None; arity],
            ret: None,
            optional: 0,
        }
    }

    /// How many leading parameters every call must supply.
    pub fn required(&self) -> usize {
        self.params.len().saturating_sub(self.optional)
    }

    /// Whether a call writing `count` arguments is within this signature's
    /// range — which says nothing about whether the *named* ones fit.
    pub fn takes_count(&self, count: usize) -> bool {
        (self.required()..=self.params.len()).contains(&count)
    }

    /// The argument counts this signature takes, as a message spells them:
    /// `2`, or `1-3` when trailing parameters have defaults. The same spelling
    /// as the runtime's arity error.
    pub fn arity_range(&self) -> String {
        crate::backend::calls::arity_range(self.required(), self.params.len())
    }
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
            Type::Map,
            Type::Function,
            Type::Enum,
            Type::Vec2,
            Type::Vec3,
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
            (Type::Vec2, crate::heap::Heap::new().vec2_value(0.0, 0.0).type_name()),
            (Type::Dual, crate::heap::Heap::new().dual_value(0.0, 0.0).type_name()),
        ];
        for (ty, runtime_name) in cases {
            assert_eq!(ty.name(), *runtime_name, "{ty:?}");
        }
        // Names not easily constructed above are still asserted by string:
        assert_eq!(Type::String.name(), "string");
        assert_eq!(Type::List.name(), "list");
        assert_eq!(Type::Record.name(), "record"); // Value::Map -> "record"
        assert_eq!(Type::Function.name(), "function"); // callables collapse
        assert_eq!(Type::Enum.name(), "enum");
        assert_eq!(Type::F64Array.name(), "f64_array");
        let mut heap = crate::heap::Heap::new();
        assert_eq!(Type::Vec3.name(), heap.vec3_value(0.0, 0.0, 0.0).type_name());
        let map = Value::Dict(heap.alloc_dict(Default::default()));
        assert_eq!(Type::Map.name(), map.type_name());
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
            .declare(
                crate::classes::ClassDef {
                    name: "A".into(),
                    fields: vec![],
                    methods: vec![],
                    builtin: false,
                },
                None,
            )
            .unwrap();
        let b = classes
            .declare(
                crate::classes::ClassDef {
                    name: "B".into(),
                    fields: vec![],
                    methods: vec![],
                    builtin: false,
                },
                None,
            )
            .unwrap();
        assert!(!Type::Class(a).is_assignable_to(&Type::Class(b)));
        assert!(Type::Class(a).is_assignable_to(&Type::Class(a)));
    }

    #[test]
    fn num_is_spelled_num_and_resolves_from_that_name() {
        assert_eq!(Type::from_name("num"), Some(Type::Num));
        assert_eq!(Type::Num.name(), "num");
        let classes = crate::classes::ClassTable::new();
        assert_eq!(Type::resolve("num", &classes), Some(Type::Num));
    }

    #[test]
    fn int_and_float_satisfy_num() {
        assert!(Type::Int.is_assignable_to(&Type::Num));
        assert!(Type::Float.is_assignable_to(&Type::Num));
        assert!(Type::Num.is_assignable_to(&Type::Num));
    }

    /// A dual is a number that every arithmetic path accepts, so warning on one
    /// in a `num` slot would fire on working autodiff code — the outcome this
    /// pass avoids. See the `is_assignable_to` docs.
    #[test]
    fn a_dual_satisfies_num() {
        assert!(Type::Dual.is_assignable_to(&Type::Num));
    }

    /// `num` is a *widening* only. Narrowing back to a concrete numeric type
    /// needs an explicit cast, or `int(x)` would be implicit.
    #[test]
    fn num_narrows_to_nothing() {
        assert!(!Type::Num.is_assignable_to(&Type::Int));
        assert!(!Type::Num.is_assignable_to(&Type::Float));
        assert!(!Type::Num.is_assignable_to(&Type::Dual));
    }

    #[test]
    fn num_is_not_a_catch_all() {
        for ty in [
            Type::String,
            Type::Bool,
            Type::List,
            Type::Record,
            Type::Nil,
        ] {
            assert!(!ty.is_assignable_to(&Type::Num), "{ty:?} -> num");
            assert!(!Type::Num.is_assignable_to(&ty), "num -> {ty:?}");
        }
    }

    /// `num` has no runtime tag — no `Value` reports `"num"` — so it must stay
    /// out of the `Value::type_name` lockstep set, like `any` and `class`.
    #[test]
    fn num_has_no_runtime_type_name() {
        assert!(!concrete_types().contains(&Type::Num));
        assert!(Value::Int(1).type_name() != Type::Num.name());
        assert!(Value::Float(1.0).type_name() != Type::Num.name());
    }

    /// Every base `T?` can take, as a plain type.
    fn nullable_bases() -> Vec<Type> {
        concrete_types()
            .into_iter()
            .chain([Type::Num, Type::Class(ClassId(0)), Type::EnumOf(EnumId(0))])
            .filter(|t| *t != Type::Nil)
            .collect()
    }

    #[test]
    fn non_nil_round_trips_every_base() {
        for ty in nullable_bases() {
            let base = ty.non_nil().expect("a nullable base");
            assert_eq!(base.ty(), ty, "{ty:?}");
            assert_eq!(ty.or_nil(), Type::OrNil(base));
            assert_eq!(ty.or_nil().strip_nil(), ty);
        }
        for ty in [Type::Any, Type::Nil, Type::Int.or_nil()] {
            assert_eq!(ty.non_nil(), None, "{ty:?}");
            assert_eq!(ty.or_nil(), ty, "{ty:?}");
        }
    }

    #[test]
    fn a_trailing_question_mark_makes_a_name_nullable() {
        for ty in concrete_types().into_iter().chain([Type::Num]) {
            if ty == Type::Nil {
                continue;
            }
            let written = format!("{}?", ty.name());
            assert_eq!(Type::from_name(&written), Some(ty.or_nil()), "{written}");
            assert_eq!(ty.or_nil().name(), written);
        }
        assert_eq!(Type::from_name("str?"), Some(Type::String.or_nil()));
        // `nil?` is just `nil`, and `any` already includes nil.
        assert_eq!(Type::from_name("nil?"), Some(Type::Nil));
        assert_eq!(Type::from_name("any?"), Some(Type::Any));
        assert_eq!(Type::from_name("int??"), None);
        assert_eq!(Type::from_name("banana?"), None);
        assert_eq!(Type::from_name("?"), None);
    }

    #[test]
    fn a_nullable_class_resolves_and_displays_through_the_class_table() {
        let classes = crate::classes::ClassTable::new();
        let rect = Type::Class(classes.lookup("Rect").unwrap());
        assert_eq!(Type::resolve("Rect?", &classes), Some(rect.or_nil()));
        assert_eq!(rect.or_nil().display(&classes), "Rect?");
        assert_eq!(Type::Int.or_nil().display(&classes), "int?");
        assert_eq!(Type::resolve("Nope?", &classes), None);
    }

    /// The phase-1 table of docs/proposals/nullable-types.md.
    #[test]
    fn nullable_assignability_follows_the_phase_one_rules() {
        let int_q = Type::Int.or_nil();
        let float_q = Type::Float.or_nil();
        let classes = crate::classes::ClassTable::new();
        let rect = Type::Class(classes.lookup("Rect").unwrap());
        // nil → T?, T → T?
        assert!(Type::Nil.is_assignable_to(&int_q));
        assert!(Type::Int.is_assignable_to(&int_q));
        // U → T? warns exactly when U → T does.
        assert!(!Type::String.is_assignable_to(&int_q));
        assert!(!Type::Float.is_assignable_to(&int_q));
        // T? → T is silent in phase 1.
        assert!(int_q.is_assignable_to(&Type::Int));
        // any both ways.
        assert!(int_q.is_assignable_to(&Type::Any));
        assert!(Type::Any.is_assignable_to(&int_q));
        // The existing widenings carry through.
        assert!(Type::Int.is_assignable_to(&float_q));
        assert!(Type::Int.is_assignable_to(&Type::Num.or_nil()));
        assert!(rect.is_assignable_to(&Type::Record.or_nil()));
        assert!(int_q.is_assignable_to(&float_q));
        assert!(!float_q.is_assignable_to(&int_q));
        assert!(!float_q.is_assignable_to(&Type::Int));
        // A `T?` is not `nil`.
        assert!(!int_q.is_assignable_to(&Type::Nil));
        assert!(int_q.is_assignable_to(&int_q));
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
