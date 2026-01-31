# Type System

A TypeScript-inspired type system with optional type annotations. Supports primitives, arrays, objects, generics, and record types with optional/nullable fields.

## Related Data Structures

- [[Term]] - Terms have inferred or annotated types
- [[Value]] - Runtime values are checked against types
- [[StateSchema]] - State nodes have type information

## Definition

```rust
/// Core type representation
pub enum Type {
    // Primitives
    Nil,
    Bool,
    Int,
    Float,
    String,

    // Compound types
    Array(Box<Type>),
    Tuple(Vec<Type>),

    // Object/Record types
    Object(ObjectType),

    // Function types
    Function {
        params: Vec<FunctionParam>,
        result: Box<Type>,
    },

    // Generic types
    Generic {
        name: String,
        constraints: Vec<TypeConstraint>,
    },
    GenericInstance {
        base: Box<Type>,
        type_args: Vec<Type>,
    },

    // Type combinators
    Union(Vec<Type>),
    Intersection(Vec<Type>),
    Optional(Box<Type>),  // T | nil

    // Special types
    Any,
    Never,
    Unknown,

    // Reference to a named type
    Named(TypeId),
}

/// Object/record type with fields
pub struct ObjectType {
    /// Named fields
    pub fields: HashMap<String, ObjectField>,
    /// Whether additional properties are allowed
    pub extensible: bool,
}

pub struct ObjectField {
    pub field_type: Type,
    pub optional: bool,    // field?: Type
    pub readonly: bool,    // readonly field: Type
}

/// Function parameter
pub struct FunctionParam {
    pub name: Option<String>,
    pub param_type: Type,
    pub optional: bool,
    pub rest: bool,  // ...args: Type[]
}

/// Constraints on generic types
pub enum TypeConstraint {
    Extends(Type),        // T extends SomeType
    Implements(TraitId),  // T implements SomeTrait
}

/// Type alias / named type definition
pub struct TypeDefinition {
    pub id: TypeId,
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub definition: Type,
}

pub struct GenericParam {
    pub name: String,
    pub constraint: Option<Type>,
    pub default: Option<Type>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);
```

## Type Registry

```rust
/// Type registry for named types
pub struct TypeRegistry {
    types: HashMap<TypeId, TypeDefinition>,
    by_name: HashMap<String, TypeId>,
    next_id: u32,
}

impl TypeRegistry {
    pub fn define(&mut self, name: &str, definition: Type) -> TypeId;
    pub fn lookup(&self, id: TypeId) -> Option<&TypeDefinition>;
    pub fn lookup_by_name(&self, name: &str) -> Option<TypeId>;
}
```

## Type Checker

```rust
/// Type checking context
pub struct TypeChecker {
    registry: TypeRegistry,
}

impl TypeChecker {
    /// Check if a value type is assignable to an expected type
    pub fn is_assignable(&self, value_type: &Type, expected: &Type) -> bool;

    /// Infer the type of a term
    pub fn infer_term_type(&self, program: &Program, term_id: TermId) -> Type;

    /// Check a program and report type errors
    pub fn check_program(&self, program: &Program) -> Vec<TypeError>;
}
```

---

See also: [[Outline|Implementation Plan]]
