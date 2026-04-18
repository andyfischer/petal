//! ConstantTable - Stores literal values for a program with deduplication.
//!
//! See docs/Architecture.md for the surrounding compiler design.

use std::collections::HashMap;

use serde::Serialize;

/// Unique identifier for a constant value within a Program's constant table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ConstantId(pub u32);

/// A literal value stored in the constant table.
/// Float is stored as u64 bits for Eq/Hash (per docs spec).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum ConstantValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(u64), // f64 bits
    String(String),
}

impl ConstantValue {
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

#[derive(Serialize)]
pub struct ConstantTable {
    values: Vec<ConstantValue>,
    #[serde(skip)]
    dedup: HashMap<ConstantValue, ConstantId>,
}

impl ConstantTable {
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
            dedup: HashMap::new(),
        }
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

    pub fn values(&self) -> &[ConstantValue] {
        &self.values
    }
}

impl Default for ConstantTable {
    fn default() -> Self {
        Self::new()
    }
}
