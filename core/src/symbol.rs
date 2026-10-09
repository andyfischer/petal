//! Symbols — interned `(name, ordinal)` pairs.
//!
//! A symbol is the binding key between a Petal script and its embedding host,
//! analogous to a GLSL variable name shared by CPU and GPU code. Both sides
//! intern the same string and get back the same `SymbolId` ordinal, which can
//! then key host-visible runtime state (such as buffered output channels) on
//! the `Env` instead of in process-global thread-locals.

use std::collections::HashMap;

use serde::Serialize;

/// Identifier for an interned symbol (a stable ordinal for a name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct SymbolId(pub u32);

/// Interns symbol names to stable ordinals and back. Interning the same name
/// twice returns the same `SymbolId`.
#[derive(Default)]
pub struct SymbolTable {
    names: Vec<String>,
    by_name: HashMap<String, SymbolId>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern `name`, returning its (stable) id. Idempotent.
    pub fn intern(&mut self, name: &str) -> SymbolId {
        if let Some(&id) = self.by_name.get(name) {
            return id;
        }
        let id = SymbolId(self.names.len() as u32);
        self.names.push(name.to_string());
        self.by_name.insert(name.to_string(), id);
        id
    }

    /// Look up an already-interned name without creating a new entry.
    pub fn lookup(&self, name: &str) -> Option<SymbolId> {
        self.by_name.get(name).copied()
    }

    /// Resolve an id back to its name.
    pub fn name(&self, id: SymbolId) -> Option<&str> {
        self.names.get(id.0 as usize).map(|s| s.as_str())
    }

    /// Number of interned symbols.
    pub fn count(&self) -> usize {
        self.names.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent() {
        let mut t = SymbolTable::new();
        let a = t.intern("draw_commands");
        let b = t.intern("draw_commands");
        assert_eq!(a, b);
        assert_eq!(t.count(), 1);
    }

    #[test]
    fn distinct_names_get_distinct_ids() {
        let mut t = SymbolTable::new();
        let a = t.intern("a");
        let b = t.intern("b");
        assert_ne!(a, b);
        assert_eq!(t.count(), 2);
    }

    #[test]
    fn name_round_trips() {
        let mut t = SymbolTable::new();
        let id = t.intern("frame");
        assert_eq!(t.name(id), Some("frame"));
        assert_eq!(t.lookup("frame"), Some(id));
        assert_eq!(t.lookup("missing"), None);
    }
}
