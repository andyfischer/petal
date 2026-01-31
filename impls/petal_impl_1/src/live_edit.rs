//! Live editing support with state reconciliation
//!
//! Enables modifying source code while a program is running,
//! preserving as much state as possible.

use std::collections::HashMap;

use crate::program::{Program, ProgramKey, StateKey, TermId, TermOp};
use crate::stack::{RuntimeStateKey, Stack};
use crate::value::Value;

/// A source code edit
#[derive(Debug, Clone)]
pub struct SourceEdit {
    /// Start offset in the source
    pub start: usize,
    /// End offset in the source
    pub end: usize,
    /// New text to insert
    pub new_text: String,
}

impl SourceEdit {
    pub fn new(start: usize, end: usize, new_text: String) -> Self {
        Self { start, end, new_text }
    }

    /// Create an insertion at a position
    pub fn insert(position: usize, text: String) -> Self {
        Self::new(position, position, text)
    }

    /// Create a deletion of a range
    pub fn delete(start: usize, end: usize) -> Self {
        Self::new(start, end, String::new())
    }

    /// Create a replacement
    pub fn replace(start: usize, end: usize, text: String) -> Self {
        Self::new(start, end, text)
    }

    /// Apply this edit to a source string
    pub fn apply(&self, source: &str) -> String {
        let mut result = String::with_capacity(source.len() + self.new_text.len());
        result.push_str(&source[..self.start]);
        result.push_str(&self.new_text);
        result.push_str(&source[self.end..]);
        result
    }
}

/// Result of applying a live edit
#[derive(Debug, Clone)]
pub struct LiveEditResult {
    /// New program key
    pub new_program: ProgramKey,
    /// Terms that were added
    pub added_terms: Vec<TermId>,
    /// Terms that were removed
    pub removed_terms: Vec<TermId>,
    /// Terms that changed
    pub modified_terms: Vec<TermId>,
}

/// Result of state reconciliation
#[derive(Debug, Clone)]
pub struct StateReconciliation {
    /// State that was preserved
    pub preserved: Vec<StateKey>,
    /// State that needs initialization (new state declarations)
    pub needs_init: Vec<StateKey>,
    /// State that was orphaned (removed state declarations)
    pub orphaned: Vec<StateKey>,
}

/// State schema for a program
#[derive(Debug, Clone)]
pub struct StateSchema {
    /// State declarations: maps state key to (name, term_id)
    pub declarations: HashMap<StateKey, StateDeclaration>,
}

/// A state declaration
#[derive(Debug, Clone)]
pub struct StateDeclaration {
    pub key: StateKey,
    pub name: String,
    pub term_id: TermId,
}

impl StateSchema {
    /// Build a state schema from a program
    pub fn from_program(program: &Program) -> Self {
        let mut declarations = HashMap::new();

        for term in program.terms() {
            if let TermOp::StateDecl { name, key } = &term.op {
                declarations.insert(
                    *key,
                    StateDeclaration {
                        key: *key,
                        name: name.clone(),
                        term_id: term.id,
                    },
                );
            }
        }

        Self { declarations }
    }

    /// Reconcile state between old and new schemas
    pub fn reconcile(&self, new_schema: &StateSchema) -> StateReconciliation {
        let mut preserved = Vec::new();
        let mut needs_init = Vec::new();
        let mut orphaned = Vec::new();

        // Build a map from name to key for the old schema
        let old_by_name: HashMap<&str, StateKey> = self
            .declarations
            .values()
            .map(|d| (d.name.as_str(), d.key))
            .collect();

        // Build a map from name to key for the new schema
        let new_by_name: HashMap<&str, StateKey> = new_schema
            .declarations
            .values()
            .map(|d| (d.name.as_str(), d.key))
            .collect();

        // Check each new state declaration
        for (name, &new_key) in &new_by_name {
            if old_by_name.contains_key(name) {
                // State with this name exists in old schema - preserve it
                preserved.push(new_key);
            } else {
                // New state declaration - needs initialization
                needs_init.push(new_key);
            }
        }

        // Check for orphaned state
        for (name, &old_key) in &old_by_name {
            if !new_by_name.contains_key(name) {
                // State was removed
                orphaned.push(old_key);
            }
        }

        StateReconciliation {
            preserved,
            needs_init,
            orphaned,
        }
    }
}

/// Live editor for modifying running programs
pub struct LiveEditor {
    /// Original source code
    pub source: String,
}

impl LiveEditor {
    pub fn new(source: String) -> Self {
        Self { source }
    }

    /// Apply an edit and return the new source
    pub fn apply_edit(&mut self, edit: &SourceEdit) -> String {
        self.source = edit.apply(&self.source);
        self.source.clone()
    }

    /// Get the current source
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Migrate state from old stack to new program
pub fn migrate_state(
    old_stack: &Stack,
    old_program: &Program,
    new_program: &Program,
) -> HashMap<RuntimeStateKey, Value> {
    let old_schema = StateSchema::from_program(old_program);
    let new_schema = StateSchema::from_program(new_program);
    let reconciliation = old_schema.reconcile(&new_schema);

    let mut new_state = HashMap::new();

    // Build name-to-key mapping for old and new schemas
    let old_key_by_name: HashMap<&str, StateKey> = old_schema
        .declarations
        .values()
        .map(|d| (d.name.as_str(), d.key))
        .collect();

    let _new_key_by_name: HashMap<&str, StateKey> = new_schema
        .declarations
        .values()
        .map(|d| (d.name.as_str(), d.key))
        .collect();

    // For each preserved state, copy the value from old to new
    for &new_key in &reconciliation.preserved {
        // Find the name for this new key
        if let Some(decl) = new_schema.declarations.get(&new_key) {
            // Find the old key with the same name
            if let Some(&old_key) = old_key_by_name.get(decl.name.as_str()) {
                // Copy all state values with this old key to the new key
                for (runtime_key, value) in &old_stack.state_storage {
                    if runtime_key.base == old_key {
                        // Create new runtime key with updated base
                        let new_runtime_key = RuntimeStateKey {
                            base: new_key,
                            iterations: runtime_key.iterations.clone(),
                        };
                        new_state.insert(new_runtime_key, value.clone());
                    }
                }
            }
        }
    }

    new_state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_edit() {
        let source = "let x = 10";

        let edit = SourceEdit::replace(8, 10, "20".to_string());
        let result = edit.apply(source);
        assert_eq!(result, "let x = 20");

        let edit = SourceEdit::insert(10, " + 5".to_string());
        let result = edit.apply(source);
        assert_eq!(result, "let x = 10 + 5");

        let edit = SourceEdit::delete(4, 10);
        let result = edit.apply(source);
        assert_eq!(result, "let ");
    }
}
