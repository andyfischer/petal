//! Execution trace and provenance tracking
//!
//! Tracks which terms influenced which values during execution.

use std::collections::{HashMap, HashSet};

use crate::program::TermId;
use crate::value::Value;

/// A single step in the execution trace
#[derive(Debug, Clone)]
pub struct TraceStep {
    /// The term that was evaluated
    pub term_id: TermId,
    /// Input term IDs that this term depends on
    pub inputs: Vec<TermId>,
    /// The value produced by this term
    pub output: Value,
    /// Timestamp (step number)
    pub timestamp: u64,
}

/// Execution trace for a program run
#[derive(Debug, Clone, Default)]
pub struct ExecutionTrace {
    /// All trace steps in order
    steps: Vec<TraceStep>,
    /// Step counter
    next_timestamp: u64,
    /// Index: term -> steps where it was evaluated
    term_steps: HashMap<TermId, Vec<usize>>,
}

impl ExecutionTrace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all trace data
    pub fn clear(&mut self) {
        self.steps.clear();
        self.next_timestamp = 0;
        self.term_steps.clear();
    }

    /// Record a trace step
    pub fn record(&mut self, term_id: TermId, inputs: Vec<TermId>, output: Value) {
        let step_idx = self.steps.len();
        let step = TraceStep {
            term_id,
            inputs,
            output,
            timestamp: self.next_timestamp,
        };
        self.next_timestamp += 1;
        self.steps.push(step);

        // Update term index
        self.term_steps
            .entry(term_id)
            .or_default()
            .push(step_idx);
    }

    /// Get all steps for a term
    pub fn get_steps_for_term(&self, term_id: TermId) -> Vec<&TraceStep> {
        self.term_steps
            .get(&term_id)
            .map(|indices| indices.iter().map(|&i| &self.steps[i]).collect())
            .unwrap_or_default()
    }

    /// Get the last step for a term
    pub fn get_last_step_for_term(&self, term_id: TermId) -> Option<&TraceStep> {
        self.term_steps
            .get(&term_id)
            .and_then(|indices| indices.last())
            .map(|&i| &self.steps[i])
    }

    /// Get all terms that influenced a given term (backward provenance)
    pub fn get_influences(&self, term_id: TermId) -> HashSet<TermId> {
        let mut influences = HashSet::new();
        let mut to_visit = vec![term_id];
        let mut visited = HashSet::new();

        while let Some(current) = to_visit.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current);

            // Find the last evaluation of this term
            if let Some(step) = self.get_last_step_for_term(current) {
                for &input in &step.inputs {
                    influences.insert(input);
                    to_visit.push(input);
                }
            }
        }

        influences
    }

    /// Get all terms that a given term influences (forward provenance)
    pub fn get_influenced_by(&self, term_id: TermId) -> HashSet<TermId> {
        let mut influenced = HashSet::new();

        for step in &self.steps {
            if step.inputs.contains(&term_id) {
                influenced.insert(step.term_id);
            }
        }

        // Transitively expand
        let mut to_add: Vec<TermId> = influenced.iter().copied().collect();
        while let Some(current) = to_add.pop() {
            for step in &self.steps {
                if step.inputs.contains(&current) && !influenced.contains(&step.term_id) {
                    influenced.insert(step.term_id);
                    to_add.push(step.term_id);
                }
            }
        }

        influenced
    }

    /// Get all trace steps
    pub fn all_steps(&self) -> &[TraceStep] {
        &self.steps
    }

    /// Get number of steps
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Check if trace is empty
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Get a summary of the trace
    pub fn summary(&self) -> TraceSummary {
        let unique_terms: HashSet<_> = self.steps.iter().map(|s| s.term_id).collect();
        TraceSummary {
            total_steps: self.steps.len(),
            unique_terms: unique_terms.len(),
        }
    }
}

/// Summary of an execution trace
#[derive(Debug, Clone)]
pub struct TraceSummary {
    pub total_steps: usize,
    pub unique_terms: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_recording() {
        let mut trace = ExecutionTrace::new();

        trace.record(TermId(0), vec![], Value::Int(1));
        trace.record(TermId(1), vec![], Value::Int(2));
        trace.record(TermId(2), vec![TermId(0), TermId(1)], Value::Int(3));

        assert_eq!(trace.len(), 3);

        let influences = trace.get_influences(TermId(2));
        assert!(influences.contains(&TermId(0)));
        assert!(influences.contains(&TermId(1)));
    }
}
