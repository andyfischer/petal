//! ConstantTable - Stores literal values for a program with deduplication.
//!
//! See docs/Architecture.md for the surrounding compiler design.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Unique identifier for a constant value within a Program's constant table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConstantId(pub u32);

/// A literal value stored in the constant table.
/// Float is stored as u64 bits for Eq/Hash (per docs spec).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConstantValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(u64), // f64 bits
    String(String),
}

impl ConstantValue {
    /// Compact literal rendering for display: numbers and booleans bare,
    /// strings quoted, `nil` for Nil. Shared by the IR display and the
    /// bytecode disassembler.
    pub fn display_compact(&self) -> String {
        match self {
            ConstantValue::Nil => "nil".to_string(),
            ConstantValue::Bool(b) => b.to_string(),
            ConstantValue::Int(n) => n.to_string(),
            ConstantValue::Float(bits) => f64::from_bits(*bits).to_string(),
            ConstantValue::String(s) => format!("{:?}", s),
        }
    }

    pub fn from_f64(f: f64) -> Self {
        ConstantValue::Float(f.to_bits())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ConstantValue::Float(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct ConstantTable {
    values: Vec<ConstantValue>,
    #[serde(skip)]
    dedup: HashMap<ConstantValue, ConstantId>,
    /// Ids handed out by [`alloc_slot`](Self::alloc_slot): entries one term
    /// owns, whose value may be replaced while the program is loaded. Never
    /// in `dedup`, so `intern` cannot hand one to a second term.
    #[serde(skip)]
    slots: std::collections::HashSet<u32>,
}

impl ConstantTable {
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
            dedup: HashMap::new(),
            slots: std::collections::HashSet::new(),
        }
    }

    /// Append an entry that is *not* deduplicated: a slot one term reads, so
    /// its value can be replaced in place ([`set_slot`](Self::set_slot))
    /// without touching any other term that happens to hold an equal
    /// constant. This is the late binding behind value-only hot reload: the
    /// VM reads a constant through the table on every load, so writing the
    /// slot is the whole update (see `Env::apply_program_change`).
    pub fn alloc_slot(&mut self, value: ConstantValue) -> ConstantId {
        let id = ConstantId(self.values.len() as u32);
        self.values.push(value);
        self.slots.insert(id.0);
        id
    }

    /// Whether `id` came from [`alloc_slot`](Self::alloc_slot).
    pub fn is_slot(&self, id: ConstantId) -> bool {
        self.slots.contains(&id.0)
    }

    /// Replace a slot's value. Panics if `id` is not a slot: overwriting an
    /// interned entry would change every term that shares it.
    pub fn set_slot(&mut self, id: ConstantId, value: ConstantValue) {
        assert!(self.is_slot(id), "constant {} is not a slot", id.0);
        self.values[id.0 as usize] = value;
    }

    /// Intern a constant value, returning its ID. Deduplicates identical values.
    pub fn intern(&mut self, value: ConstantValue) -> ConstantId {
        if let Some(&id) = self.dedup.get(&value) {
            return id;
        }
        let id = ConstantId(self.values.len() as u32);
        self.dedup.insert(value.clone(), id);
        self.values.push(value);
        id
    }

    pub fn get(&self, id: ConstantId) -> &ConstantValue {
        &self.values[id.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn values(&self) -> &[ConstantValue] {
        &self.values
    }

    /// Rebuild the value→id dedup map after deserialization (it is
    /// `#[serde(skip)]` and so arrives empty). Keeps the first id for any
    /// duplicate values, matching `intern`'s first-wins behavior.
    pub fn rebuild_dedup(&mut self) {
        self.dedup.clear();
        for (i, value) in self.values.iter().enumerate() {
            self.dedup
                .entry(value.clone())
                .or_insert(ConstantId(i as u32));
        }
    }
}

impl Default for ConstantTable {
    fn default() -> Self {
        Self::new()
    }
}
