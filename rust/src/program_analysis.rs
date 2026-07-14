//! Dataflow-graph analysis over a compiled [`Program`].
//!
//! Term lookup (`find_term`/`named_terms`) and the backward/forward slicing
//! queries (`trace_provenance`, `trace_dependents`, `slice`) that back the
//! `explain`, `show-provenance`, `show-dependents`, and `show-slice` tooling.
//! These are read-only graph walks, kept separate from the IR data structures
//! in [`crate::program`] and the import validator in [`crate::ir_validate`].

use crate::program::{Program, Term, TermId, TermOp};

impl Program {
    /// Find a term by name (e.g. variable name like "x") or by id string (e.g. "t24").
    pub fn find_term(&self, query: &str) -> Option<TermId> {
        // Try "tN" id format first
        if let Some(id_str) = query.strip_prefix('t')
            && let Ok(id) = id_str.parse::<u32>()
            && (id as usize) < self.terms.len()
        {
            return Some(TermId(id));
        }
        // Try a bare numeric ID (e.g. `--term 72`)
        if let Ok(id) = query.parse::<u32>()
            && (id as usize) < self.terms.len()
        {
            return Some(TermId(id));
        }
        // Search by name (last match wins — like variable shadowing)
        let mut found = None;
        for term in &self.terms {
            if term.name.as_deref() == Some(query) {
                found = Some(term.id);
            }
        }
        found
    }

    /// Return the list of distinct user-visible names bound to terms in this
    /// program. Filters out phantom builtin terms by requiring a real source
    /// span (line > 0). Used for "did you mean?" hints on `--term` misses.
    pub fn named_terms(&self) -> Vec<String> {
        use std::collections::BTreeSet;
        let mut set = BTreeSet::new();
        for term in &self.terms {
            let Some(name) = &term.name else { continue };
            match self.source_map.get(term.id) {
                Some(span) if span.start.line > 0 => {
                    set.insert(name.clone());
                }
                _ => {}
            }
        }
        set.into_iter().collect()
    }

    /// Trace provenance: collect all transitive input ancestors of a term.
    /// Returns (ancestor_ids_in_order, edges) where each edge is (from, to)
    /// meaning `from` is an input of `to`.
    pub fn trace_provenance(&self, root_id: TermId) -> (Vec<TermId>, Vec<(TermId, TermId)>) {
        use std::collections::{HashSet, VecDeque};

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut ancestors = Vec::new();
        let mut edges = Vec::new();

        // Collect all `src_term` entries from `phi_outs` blocks across the
        // program that target a given phi term — these are the branch/loop
        // rebind candidates that update the phi's register on child-frame
        // pop, and must be treated as ancestors for provenance purposes.
        let phi_sources = |dest: TermId| -> Vec<TermId> {
            let mut out = Vec::new();
            for block in &self.blocks {
                for po in &block.phi_outs {
                    if po.dest_term == dest {
                        out.push(po.src_term);
                    }
                }
            }
            out
        };

        let push_term_inputs = |term_id: TermId,
                                visited: &mut HashSet<TermId>,
                                queue: &mut VecDeque<TermId>,
                                edges: &mut Vec<(TermId, TermId)>,
                                terms: &[Term]| {
            let term = &terms[term_id.0 as usize];
            for &input_id in &term.inputs {
                edges.push((input_id, term_id));
                if visited.insert(input_id) {
                    queue.push_back(input_id);
                }
            }
            if matches!(term.op, TermOp::Phi) {
                let srcs = phi_sources(term_id);
                for src_id in srcs {
                    edges.push((src_id, term_id));
                    if visited.insert(src_id) {
                        queue.push_back(src_id);
                    }
                }
            }
        };

        push_term_inputs(root_id, &mut visited, &mut queue, &mut edges, &self.terms);

        while let Some(term_id) = queue.pop_front() {
            ancestors.push(term_id);
            push_term_inputs(term_id, &mut visited, &mut queue, &mut edges, &self.terms);
        }

        (ancestors, edges)
    }

    /// Forward slice: collect all terms that transitively depend on the given term.
    /// This is the complement of `trace_provenance` (backward slice).
    /// Returns (dependent_ids_in_order, edges) where each edge is (from, to)
    /// meaning `from` is an input of `to`.
    pub fn trace_dependents(&self, root_id: TermId) -> (Vec<TermId>, Vec<(TermId, TermId)>) {
        use std::collections::{HashMap as StdHashMap, HashSet, VecDeque};

        // Build a reverse index: term_id -> list of terms that use it as input
        let mut users: StdHashMap<TermId, Vec<TermId>> = StdHashMap::new();
        for term in &self.terms {
            for &input_id in &term.inputs {
                users.entry(input_id).or_default().push(term.id);
            }
        }

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut dependents = Vec::new();
        let mut edges = Vec::new();

        // Seed with the root's direct users
        if let Some(direct_users) = users.get(&root_id) {
            for &user_id in direct_users {
                if visited.insert(user_id) {
                    queue.push_back(user_id);
                }
                edges.push((root_id, user_id));
            }
        }

        // BFS forward through users
        while let Some(term_id) = queue.pop_front() {
            dependents.push(term_id);
            if let Some(term_users) = users.get(&term_id) {
                for &user_id in term_users {
                    edges.push((term_id, user_id));
                    if visited.insert(user_id) {
                        queue.push_back(user_id);
                    }
                }
            }
        }

        (dependents, edges)
    }

    /// Compute a dataflow slice: the minimal subgraph needed to compute the
    /// given target terms from their transitive inputs. Returns term IDs in
    /// topological order (inputs before outputs).
    pub fn slice(&self, targets: &[TermId]) -> Vec<TermId> {
        use std::collections::HashSet;

        // Collect all ancestors of all targets (backward slice)
        let mut needed: HashSet<TermId> = HashSet::new();
        for &target in targets {
            needed.insert(target);
            let (ancestors, _) = self.trace_provenance(target);
            for id in ancestors {
                needed.insert(id);
            }
        }

        // Return in program order (which is topological for a well-formed IR)
        let mut result: Vec<TermId> = needed.into_iter().collect();
        result.sort_by_key(|id| id.0);
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::constant_table::{ConstantId, ConstantTable};
    use crate::program::*;
    use crate::source_map::SourceMap;
    use smallvec::SmallVec;
    use std::collections::HashMap;

    /// Build a minimal program with the given terms for testing.
    fn test_program(terms: Vec<Term>) -> Program {
        let root_block = BlockId(0);
        let blocks = vec![Block {
            id: root_block,
            parent_term_id: None,
            entry: terms.first().map(|t| t.id),
            param_names: vec![],
            register_count: terms.len() as u16,
            phi_outs: vec![],
        }];
        Program {
            id: ProgramId(0),
            source: String::new(),
            terms,
            blocks,
            root_block,
            constants: ConstantTable::new(),
            source_map: SourceMap::new(),
            has_errors: false,
            functions: vec![],
            match_arms: HashMap::new(),
            block_terms: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    fn make_term(id: u32, op: TermOp, inputs: Vec<u32>, name: Option<&str>) -> Term {
        Term {
            id: TermId(id),
            op,
            inputs: inputs.into_iter().map(TermId).collect(),
            block_id: BlockId(0),
            block_next: None,
            block_prev: None,
            name: name.map(|s| s.to_string()),
            register: RegisterIndex(id as u16),
            state_key: None,
            child_blocks: SmallVec::new(),
            in_loop: false,
            collect: false,
        }
    }

    #[test]
    fn find_term_by_name() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
            make_term(1, TermOp::Copy, vec![0], Some("y")),
        ]);
        assert_eq!(prog.find_term("x"), Some(TermId(0)));
        assert_eq!(prog.find_term("y"), Some(TermId(1)));
        assert_eq!(prog.find_term("z"), None);
    }

    #[test]
    fn find_term_by_id_string() {
        let prog = test_program(vec![make_term(
            0,
            TermOp::Constant(ConstantId(0)),
            vec![],
            None,
        )]);
        assert_eq!(prog.find_term("t0"), Some(TermId(0)));
        assert_eq!(prog.find_term("t99"), None);
    }

    #[test]
    fn find_term_last_name_wins() {
        // Like variable shadowing: last definition with same name is found
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
            make_term(1, TermOp::Constant(ConstantId(1)), vec![], Some("x")),
        ]);
        assert_eq!(prog.find_term("x"), Some(TermId(1)));
    }

    #[test]
    fn trace_provenance_leaf_has_no_ancestors() {
        let prog = test_program(vec![make_term(
            0,
            TermOp::Constant(ConstantId(0)),
            vec![],
            Some("x"),
        )]);
        let (ancestors, edges) = prog.trace_provenance(TermId(0));
        assert!(ancestors.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn trace_provenance_single_input() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
        ]);
        let (ancestors, edges) = prog.trace_provenance(TermId(1));
        assert_eq!(ancestors, vec![TermId(0)]);
        assert_eq!(edges, vec![(TermId(0), TermId(1))]);
    }

    #[test]
    fn trace_provenance_diamond() {
        // c depends on a and b, both depend on const
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], None),
            make_term(1, TermOp::Copy, vec![0], Some("a")),
            make_term(2, TermOp::Copy, vec![0], Some("b")),
            make_term(3, TermOp::Add, vec![1, 2], Some("c")),
        ]);
        let (ancestors, edges) = prog.trace_provenance(TermId(3));
        // BFS order: 1, 2, 0 (1 and 2 are direct inputs, 0 is shared ancestor)
        assert_eq!(ancestors.len(), 3);
        assert!(ancestors.contains(&TermId(1)));
        assert!(ancestors.contains(&TermId(2)));
        assert!(ancestors.contains(&TermId(0)));
        // Should have 4 edges: (1,3), (2,3), (0,1), (0,2)
        assert_eq!(edges.len(), 4);
    }

    #[test]
    fn trace_dependents_leaf_has_no_dependents() {
        // Terminal node with no users
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
            make_term(1, TermOp::Copy, vec![0], Some("y")),
        ]);
        let (dependents, edges) = prog.trace_dependents(TermId(1));
        assert!(dependents.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn trace_dependents_single_user() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
        ]);
        let (dependents, edges) = prog.trace_dependents(TermId(0));
        assert_eq!(dependents, vec![TermId(1)]);
        assert_eq!(edges, vec![(TermId(0), TermId(1))]);
    }

    #[test]
    fn trace_dependents_transitive() {
        // a -> b -> c
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Copy, vec![1], Some("c")),
        ]);
        let (dependents, _edges) = prog.trace_dependents(TermId(0));
        assert_eq!(dependents.len(), 2);
        assert!(dependents.contains(&TermId(1)));
        assert!(dependents.contains(&TermId(2)));
    }

    #[test]
    fn trace_dependents_fan_out() {
        // a used by both b and c
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Copy, vec![0], Some("c")),
        ]);
        let (dependents, edges) = prog.trace_dependents(TermId(0));
        assert_eq!(dependents.len(), 2);
        assert!(dependents.contains(&TermId(1)));
        assert!(dependents.contains(&TermId(2)));
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn slice_returns_minimal_subgraph() {
        // a(0) -> b(1), c(2) -> d(3) = b + c, e(4) = unrelated
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Constant(ConstantId(1)), vec![], Some("c")),
            make_term(3, TermOp::Add, vec![1, 2], Some("d")),
            make_term(4, TermOp::Constant(ConstantId(2)), vec![], Some("e")),
        ]);
        let slice = prog.slice(&[TermId(3)]);
        // Should include a, b, c, d but NOT e
        assert!(slice.contains(&TermId(0))); // a
        assert!(slice.contains(&TermId(1))); // b
        assert!(slice.contains(&TermId(2))); // c
        assert!(slice.contains(&TermId(3))); // d
        assert!(!slice.contains(&TermId(4))); // e is unrelated
        assert_eq!(slice.len(), 4);
        // Should be in topological order
        assert_eq!(slice, vec![TermId(0), TermId(1), TermId(2), TermId(3)]);
    }

    #[test]
    fn slice_multiple_targets() {
        // a(0) -> b(1), c(2) -> d(3)
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Constant(ConstantId(1)), vec![], Some("c")),
            make_term(3, TermOp::Copy, vec![2], Some("d")),
        ]);
        // Slice for both b and d should include a, b, c, d
        let slice = prog.slice(&[TermId(1), TermId(3)]);
        assert_eq!(slice.len(), 4);
    }
}
