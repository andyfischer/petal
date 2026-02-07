# Name Lookup

How to resolve a name to a [[Term]] at a given program location.

## Related Topics

- [[CodeManipulation]] - Working with program structure
- [[Execution]] - Runtime name resolution

## Related Data Structures

- [[Term]] - Terms can have optional names
- [[Block]] - Names are scoped to blocks
- [[Program]] - Contains the block and term structures

## Overview

Name lookup finds the definition of a variable name at a specific location in the program. Since each [[Term]] can have an optional `name` field and terms are organized into [[Block|Blocks]], name resolution involves searching through the program structure.

## Lookup Algorithm

To find the definition of a name at a given term:

```rust
impl Program {
    /// Look up a name visible at the given term's location.
    /// Returns the TermId of the defining term, if found.
    pub fn lookup_name(&self, at_term: TermId, name: &str) -> Option<TermId> {
        let term = self.get_term(at_term)?;
        let mut current_block_id = term.block_id;

        loop {
            // Search within the current block
            if let Some(found) = self.search_block_for_name(current_block_id, at_term, name) {
                return Some(found);
            }

            // Move to the parent block
            let block = self.get_block(current_block_id)?;
            match block.parent_term_id {
                Some(parent_term) => {
                    // Continue searching in the parent block, before the parent term
                    let parent = self.get_term(parent_term)?;
                    current_block_id = parent.block_id;
                    at_term = parent_term;
                }
                None => {
                    // Reached the root block, name not found
                    return None;
                }
            }
        }
    }

    /// Search for a name within a single block, looking only at terms
    /// that come before the given term in execution order.
    fn search_block_for_name(
        &self,
        block_id: BlockId,
        before_term: TermId,
        name: &str
    ) -> Option<TermId> {
        let term = self.get_term(before_term)?;

        // Walk backwards through block_prev links
        let mut current = term.block_prev;
        while let Some(term_id) = current {
            let t = self.get_term(term_id)?;
            if t.name.as_deref() == Some(name) {
                return Some(term_id);
            }
            current = t.block_prev;
        }

        None
    }
}
```

## Lookup Steps

1. **Check the current term's name** (optional, depending on use case)
2. **Search backwards in the current block**: Walk `block_prev` links to find terms with matching names that come before the lookup point
3. **Move to parent scope**: If not found, find the block's `parent_term_id`, then search in that term's block (before the parent term)
4. **Repeat until root**: Continue up the block hierarchy until found or root block is exhausted

## Scope Rules

Names follow lexical scoping:

```
let x = 1           // x is visible from here onward in this block
if condition {
    let y = 2       // y is visible only within the if-block
    let z = x + y   // Both x (from parent) and y (from this block) are visible
}
let w = x + 1       // x is visible, but y is not (different block)
```

Key rules:
- A name is visible from the term that defines it onward (within the same block)
- Names in parent blocks are visible in child blocks
- Names in sibling or child blocks are not visible

## Use Cases

- **Code completion**: Find all names visible at cursor position
- **Go to definition**: Navigate from usage to definition
- **Rename refactoring**: Find all usages of a name in scope
- **Type checking**: Resolve variable references during type analysis

---

See also: [[Outline|Implementation Plan]]
