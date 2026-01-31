use crate::term::{Term, TermId};
use crate::value::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramKey(pub usize);

#[derive(Debug, Clone)]
pub struct Program {
    pub id: ProgramKey,
    pub terms: Vec<Term>,
    pub entry: TermId,
    pub constants: ConstantTable,
    pub functions: Vec<FunctionDef>,
    pub source: String,
    pub has_errors: bool,
}

#[derive(Debug, Clone)]
pub struct ConstantTable {
    values: Vec<Value>,
}

impl ConstantTable {
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
        }
    }

    pub fn add(&mut self, value: Value) -> usize {
        // Check if constant already exists (deduplication)
        if let Some(idx) = self.values.iter().position(|v| v == &value) {
            return idx;
        }
        let idx = self.values.len();
        self.values.push(value);
        idx
    }

    pub fn get(&self, idx: usize) -> Option<&Value> {
        self.values.get(idx)
    }
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<TermId>,
    pub entry: TermId,
}

impl Program {
    pub fn new(id: ProgramKey, source: String) -> Self {
        Self {
            id,
            terms: Vec::new(),
            entry: TermId(0),
            constants: ConstantTable::new(),
            functions: Vec::new(),
            source,
            has_errors: false,
        }
    }

    pub fn add_term(&mut self, term: Term) -> TermId {
        let id = term.id;
        self.terms.push(term);
        id
    }

    pub fn get_term(&self, id: TermId) -> Option<&Term> {
        self.terms.get(id.0)
    }

    pub fn get_term_mut(&mut self, id: TermId) -> Option<&mut Term> {
        self.terms.get_mut(id.0)
    }

    pub fn add_function(&mut self, func: FunctionDef) -> usize {
        let idx = self.functions.len();
        self.functions.push(func);
        idx
    }

    pub fn get_function(&self, idx: usize) -> Option<&FunctionDef> {
        self.functions.get(idx)
    }
}
