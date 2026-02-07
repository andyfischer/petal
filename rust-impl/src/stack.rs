//! Stack - Runtime evaluation context.
//!
//! See docs/tech_outline/data_structures/Stack.md

use crate::program::ProgramId;

/// Unique identifier for a stack within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StackKey(pub u32);

/// Runtime execution state for a program.
pub struct Stack {
    pub id: StackKey,
    pub program_id: ProgramId,
}

impl Stack {
    pub fn new(id: StackKey, program_id: ProgramId) -> Self {
        Self { id, program_id }
    }
}
