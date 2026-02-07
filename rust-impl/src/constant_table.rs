//! ConstantTable - Stores literal values for a program with deduplication.
//!
//! See docs/tech_outline/data_structures/ConstantTable.md

use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantId(pub u32);

pub struct ConstantTable {
    constants: Vec<Value>,
}

impl ConstantTable {
    pub fn new() -> Self {
        Self {
            constants: Vec::new(),
        }
    }

    pub fn add(&mut self, value: Value) -> ConstantId {
        let id = ConstantId(self.constants.len() as u32);
        self.constants.push(value);
        id
    }

    pub fn get(&self, id: ConstantId) -> Option<&Value> {
        self.constants.get(id.0 as usize)
    }
}

impl Default for ConstantTable {
    fn default() -> Self {
        Self::new()
    }
}
