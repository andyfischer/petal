# Value

The runtime representation of data.

## Related Data Structures

- [[Stack]] - Stores values in registers and state
- [[Term]] - Produces values during evaluation
- [[DiffGraph]] - Uses differentiable values for backpropagation
- [[Heap|Heap]] - Stores strings, lists, and maps

## Definition

```rust
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(StringId),
    List(ListId),
    Map(MapId),
    Function(FunctionId),

    /// For gradients/backpropagation
    Differentiable {
        value: Box<Value>,
        gradient: Option<Box<Value>>,
        source_term: TermId,
    },
}
```

## Heap References

`StringId`, `ListId`, and `MapId` are references into the garbage-collected [[Heap|Heap]]:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapId(pub u32);
```

These IDs are cheap to copy and compare, but the actual data lives on the heap. The heap is owned by [[Env]], so these IDs are only valid within the Env that created them.

## Notes

- Basic types (Nil, Bool, Int, Float) are stored inline - no heap allocation
- Complex types (String, List, Map) use IDs referencing the [[Heap|Heap]]
- Values are `Copy` - passing a list doesn't clone it, just copies the ID
- The `Differentiable` variant supports automatic differentiation (see [[DiffGraph]])

---

See also: [[Outline|Implementation Plan]]
