//! Program projection and slicing
//!
//! Provides the ability to view subsets of a program based on data dependencies.

use std::collections::HashSet;

use crate::program::{Program, TermId, TermOp};
use crate::provenance::ExecutionTrace;

/// What to focus the projection on
#[derive(Debug, Clone)]
pub enum ProjectionFocus {
    /// Forward slice: what does this term influence?
    Forward(TermId),
    /// Backward slice: what influences this term?
    Backward(TermId),
    /// Dynamic slice: what was actually executed to produce this value?
    Dynamic {
        trace: ExecutionTrace,
        target_term: TermId,
    },
}

/// A projection (subset) of a program
#[derive(Debug, Clone)]
pub struct Projection {
    /// The terms included in this projection
    pub included_terms: HashSet<TermId>,
    /// Dataflow edges within the projection (from -> to)
    pub dataflow_edges: Vec<(TermId, TermId)>,
}

impl Projection {
    /// Create an empty projection
    pub fn new() -> Self {
        Self {
            included_terms: HashSet::new(),
            dataflow_edges: Vec::new(),
        }
    }

    /// Create a backward slice projection (what influences the target?)
    pub fn backward_slice(program: &Program, target: TermId) -> Self {
        let mut projection = Self::new();
        let mut to_visit = vec![target];
        let mut visited = HashSet::new();

        while let Some(term_id) = to_visit.pop() {
            if visited.contains(&term_id) {
                continue;
            }
            visited.insert(term_id);

            if let Some(term) = program.get_term(term_id) {
                projection.included_terms.insert(term_id);

                // Add inputs to visit list and record edges
                for &input in &term.inputs {
                    projection.dataflow_edges.push((input, term_id));
                    to_visit.push(input);
                }

                // For control flow, also follow branch bodies
                match &term.op {
                    TermOp::If => {
                        // Inputs are [condition, then, else]
                        // We already added them above
                    }
                    TermOp::ForLoop { body, .. } | TermOp::WhileLoop { body } => {
                        to_visit.push(*body);
                        projection.dataflow_edges.push((*body, term_id));
                    }
                    TermOp::FnDef { body, .. } => {
                        to_visit.push(*body);
                        projection.dataflow_edges.push((*body, term_id));
                    }
                    TermOp::Block => {
                        // Inputs are already added
                    }
                    _ => {}
                }
            }
        }

        projection
    }

    /// Create a forward slice projection (what does the target influence?)
    pub fn forward_slice(program: &Program, source: TermId) -> Self {
        let mut projection = Self::new();
        projection.included_terms.insert(source);

        let mut to_visit = vec![source];
        let mut visited = HashSet::new();

        // Build reverse dependency map
        let mut dependents: std::collections::HashMap<TermId, Vec<TermId>> =
            std::collections::HashMap::new();

        for term in program.terms() {
            for &input in &term.inputs {
                dependents.entry(input).or_default().push(term.id);
            }

            // Also track body references
            match &term.op {
                TermOp::ForLoop { body, .. } | TermOp::WhileLoop { body } => {
                    dependents.entry(*body).or_default().push(term.id);
                }
                TermOp::FnDef { body, .. } => {
                    dependents.entry(*body).or_default().push(term.id);
                }
                _ => {}
            }
        }

        while let Some(term_id) = to_visit.pop() {
            if visited.contains(&term_id) {
                continue;
            }
            visited.insert(term_id);

            if let Some(deps) = dependents.get(&term_id) {
                for &dep in deps {
                    projection.included_terms.insert(dep);
                    projection.dataflow_edges.push((term_id, dep));
                    to_visit.push(dep);
                }
            }
        }

        projection
    }

    /// Create a dynamic slice based on actual execution trace
    pub fn dynamic_slice(trace: &ExecutionTrace, target: TermId) -> Self {
        let mut projection = Self::new();

        // Get all terms that influenced the target
        let influences = trace.get_influences(target);
        projection.included_terms.insert(target);
        projection.included_terms.extend(influences.iter().copied());

        // Build edges from the trace
        for step in trace.all_steps() {
            if projection.included_terms.contains(&step.term_id) {
                for &input in &step.inputs {
                    if projection.included_terms.contains(&input) {
                        projection.dataflow_edges.push((input, step.term_id));
                    }
                }
            }
        }

        projection
    }

    /// Check if a term is included in the projection
    pub fn includes(&self, term_id: TermId) -> bool {
        self.included_terms.contains(&term_id)
    }

    /// Get the number of terms in the projection
    pub fn size(&self) -> usize {
        self.included_terms.len()
    }

    /// Compute the intersection with another projection
    pub fn intersection(&self, other: &Projection) -> Projection {
        let included: HashSet<_> = self
            .included_terms
            .intersection(&other.included_terms)
            .copied()
            .collect();

        let edges: Vec<_> = self
            .dataflow_edges
            .iter()
            .filter(|(from, to)| included.contains(from) && included.contains(to))
            .copied()
            .collect();

        Projection {
            included_terms: included,
            dataflow_edges: edges,
        }
    }

    /// Compute the union with another projection
    pub fn union(&self, other: &Projection) -> Projection {
        let included: HashSet<_> = self
            .included_terms
            .union(&other.included_terms)
            .copied()
            .collect();

        let mut edges = self.dataflow_edges.clone();
        edges.extend(other.dataflow_edges.iter().copied());
        edges.sort();
        edges.dedup();

        Projection {
            included_terms: included,
            dataflow_edges: edges,
        }
    }

    /// Generate a DOT graph representation
    pub fn to_dot(&self, program: &Program) -> String {
        let mut dot = String::new();
        dot.push_str("digraph projection {\n");
        dot.push_str("  rankdir=TB;\n");
        dot.push_str("  node [shape=box];\n");
        dot.push('\n');

        // Add nodes
        for &term_id in &self.included_terms {
            if let Some(term) = program.get_term(term_id) {
                let label = format_term_label(&term.op);
                dot.push_str(&format!(
                    "  t{} [label=\"t{}: {}\"];\n",
                    term_id.0, term_id.0, label
                ));
            }
        }

        dot.push('\n');

        // Add edges
        for (from, to) in &self.dataflow_edges {
            dot.push_str(&format!("  t{} -> t{};\n", from.0, to.0));
        }

        dot.push_str("}\n");
        dot
    }
}

impl Default for Projection {
    fn default() -> Self {
        Self::new()
    }
}

fn format_term_label(op: &TermOp) -> String {
    match op {
        TermOp::Constant(_) => "const".to_string(),
        TermOp::Var(name) => format!("var:{}", name),
        TermOp::Let { name } => format!("let:{}", name),
        TermOp::Assign { name } => format!("assign:{}", name),
        TermOp::StateDecl { name, .. } => format!("state:{}", name),
        TermOp::Add => "+".to_string(),
        TermOp::Sub => "-".to_string(),
        TermOp::Mul => "*".to_string(),
        TermOp::Div => "/".to_string(),
        TermOp::Mod => "%".to_string(),
        TermOp::Neg => "neg".to_string(),
        TermOp::Eq => "==".to_string(),
        TermOp::NotEq => "!=".to_string(),
        TermOp::Lt => "<".to_string(),
        TermOp::LtEq => "<=".to_string(),
        TermOp::Gt => ">".to_string(),
        TermOp::GtEq => ">=".to_string(),
        TermOp::And => "&&".to_string(),
        TermOp::Or => "||".to_string(),
        TermOp::Not => "!".to_string(),
        TermOp::If => "if".to_string(),
        TermOp::Block => "block".to_string(),
        TermOp::ForLoop { var_name, .. } => format!("for:{}", var_name),
        TermOp::WhileLoop { .. } => "while".to_string(),
        TermOp::FnDef { name, .. } => format!("fn:{}", name),
        TermOp::Call { function, .. } => format!("call:{}", function),
        TermOp::List => "list".to_string(),
        TermOp::Map => "map".to_string(),
        TermOp::Index => "index".to_string(),
        TermOp::Field { name } => format!(".{}", name),
        TermOp::Print => "print".to_string(),
        _ => "?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_projection_operations() {
        let mut p1 = Projection::new();
        p1.included_terms.insert(TermId(0));
        p1.included_terms.insert(TermId(1));

        let mut p2 = Projection::new();
        p2.included_terms.insert(TermId(1));
        p2.included_terms.insert(TermId(2));

        let intersection = p1.intersection(&p2);
        assert_eq!(intersection.size(), 1);
        assert!(intersection.includes(TermId(1)));

        let union = p1.union(&p2);
        assert_eq!(union.size(), 3);
    }
}
