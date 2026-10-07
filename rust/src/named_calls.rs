//! Static resolution of a call term's callee, and of the parameter each of
//! its arguments binds — "which function does this call run, and which slot
//! does each argument fill?", answered from the IR alone.
//!
//! Two users, deliberately sharing one answer:
//!
//! - `petal suggest`'s named-argument refactor ([`crate::suggest::named_args`])
//!   asks it to find the parameter names a positional call could be written
//!   with;
//! - [`crate::ir_equiv`]'s named-argument mode asks it to prove that a call
//!   written `f(x: 1, y: 2)` and one written `f(1, 2)` bind the same values to
//!   the same parameters of the same function.
//!
//! Everything here is conservative: `None` means "cannot tell", never "no".
//! A callee resolves only when the IR pins it to a fixed list of functions —
//! the dataflow edge into the call is followed back through copies, closure
//! captures and function cells to the `MakeClosure` / `MakeOverloadSet` that
//! produced the value. A parameter, a phi, the result of another call, a
//! record field: all opaque, all `None`. The IR is in SSA form, so a name
//! that is rebound or shadowed between its declaration and the call is a
//! *different term* and needs no separate check.
//!
//! Variant selection and argument binding are not re-derived: they are
//! [`crate::backend::calls::accepts_call`] and
//! [`crate::native_fn::bind_native_args`], the definitions the VM runs.

use std::collections::{HashMap, HashSet};

use crate::backend::calls::{accepts_call, skips_a_parameter};
use crate::constant_table::ConstantValue;
use crate::ir_validate::is_binding_phantom;
use crate::native_fn::{NativeSignature, bind_native_args};
use crate::program::{
    BlockId, FunctionDef, FunctionId, Program, Term, TermId, TermOp, base_fn_name,
};

/// How far a resolution may chase edges before giving up. Real chains are a
/// handful of links; the bound only exists so a cyclic capture cannot loop.
const FUEL: u32 = 64;

/// What a call term was resolved to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
    /// One Petal function — a `fn`, an overload variant, a lambda, or a class
    /// constructor.
    Function(FunctionId),
    /// A native, which reads its arguments by position.
    Native,
}

/// A call resolved down to what the callee's frame receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallBinding {
    pub callee: Callee,
    /// For each written argument, in written order, the parameter slot it
    /// fills. An all-positional call is `0, 1, 2, …`.
    pub slots: Vec<usize>,
}

/// The parameter lists a native declares, by name. `None` when the native is
/// unknown or its registration is out of sight; empty when it takes arguments
/// by position only.
pub type NativeSignatures<'a> = &'a dyn Fn(&str) -> Option<Vec<NativeSignature>>;

/// Per-program indexes for callee resolution. Building one is a single pass
/// over the terms; every query after that follows edges.
pub struct CallResolver<'p> {
    program: &'p Program,
    /// Function body block → the function.
    body_of: HashMap<BlockId, FunctionId>,
    /// Every `MakeClosure` site of a function.
    closure_sites: HashMap<FunctionId, Vec<TermId>>,
    /// `MakeClosure` term → the `MakeOverloadSet` terms that list it.
    sets_of: HashMap<TermId, Vec<TermId>>,
    /// Every `CellWrite` in the program.
    cell_writes: Vec<TermId>,
    /// Terms a phi carry-out writes into: their register is overwritten when
    /// a child block exits, so their own op does not say what they hold.
    phi_dests: HashSet<TermId>,
}

impl<'p> CallResolver<'p> {
    pub fn new(program: &'p Program) -> Self {
        let mut closure_sites: HashMap<FunctionId, Vec<TermId>> = HashMap::new();
        let mut sets_of: HashMap<TermId, Vec<TermId>> = HashMap::new();
        let mut cell_writes = Vec::new();
        for term in &program.terms {
            match term.op {
                TermOp::MakeClosure(f) => closure_sites.entry(f).or_default().push(term.id),
                TermOp::MakeOverloadSet => {
                    for &input in &term.inputs {
                        sets_of.entry(input).or_default().push(term.id);
                    }
                }
                TermOp::CellWrite => cell_writes.push(term.id),
                _ => {}
            }
        }
        let phi_dests = program
            .blocks
            .iter()
            .flat_map(|b| b.phi_outs.iter().map(|p| p.dest_term))
            .collect();
        CallResolver {
            program,
            body_of: program
                .functions
                .iter()
                .map(|f| (f.body_block, f.id))
                .collect(),
            closure_sites,
            sets_of,
            cell_writes,
            phi_dests,
        }
    }

    pub fn function(&self, id: FunctionId) -> &'p FunctionDef {
        &self.program.functions[id.0 as usize]
    }

    /// The functions the value of `term` can be called as, in overload-set
    /// order — one entry for a plain closure. `None` unless the IR fixes it.
    pub fn resolve(&self, term: TermId) -> Option<Vec<FunctionId>> {
        self.resolve_at(term, FUEL)
    }

    fn resolve_at(&self, id: TermId, fuel: u32) -> Option<Vec<FunctionId>> {
        let fuel = fuel.checked_sub(1)?;
        if self.phi_dests.contains(&id) {
            return None;
        }
        let term = self.program.get_term(id);
        match term.op {
            TermOp::MakeClosure(f) => Some(vec![f]),
            TermOp::MakeOverloadSet => {
                // The runtime builds the set from the closures among its
                // inputs; anything else there is not a shape the compiler
                // emits, so it is not one to reason about.
                let mut fns = Vec::with_capacity(term.inputs.len());
                for &input in &term.inputs {
                    match self.program.get_term(input).op {
                        TermOp::MakeClosure(f) => fns.push(f),
                        _ => return None,
                    }
                }
                (!fns.is_empty()).then_some(fns)
            }
            TermOp::Copy if term.inputs.len() == 1 => self.resolve_at(term.inputs[0], fuel),
            TermOp::Copy if is_binding_phantom(term) => self.resolve_binding(term, fuel),
            TermOp::CellRead => {
                let cell = self.cell_of(*term.inputs.first()?, fuel)?;
                self.cell_contents(cell, fuel)
            }
            _ => None,
        }
    }

    /// A binding phantom in a function body: the function's own self
    /// reference, or one of its captures. A parameter is neither, and opaque.
    fn resolve_binding(&self, term: &Term, fuel: u32) -> Option<Vec<FunctionId>> {
        let f = *self.body_of.get(&term.block_id)?;
        let func = self.function(f);
        // The VM seeds the self-reference register with the closure being
        // called — this one variant, never the overload set around it.
        if func.self_ref_register == Some(term.register) {
            return Some(vec![f]);
        }
        let index = func
            .capture_registers
            .iter()
            .position(|r| *r == term.register)?;
        self.agree(f, |site| {
            let input = *self.program.get_term(site).inputs.get(index)?;
            self.captured_value(func, index, site, input, fuel)
        })
    }

    /// What capture `index` of `func` holds when the closure was made at
    /// `site` from `input`.
    fn captured_value(
        &self,
        func: &FunctionDef,
        index: usize,
        site: TermId,
        input: TermId,
        fuel: u32,
    ) -> Option<Vec<FunctionId>> {
        let source = self.program.get_term(input);
        // An overload variant that calls its own name captures a placeholder
        // that is still nil when the closure is made;
        // `backend::calls::make_overload_set` then writes the finished set
        // into exactly that capture. The placeholder is a binding phantom
        // outside any function body, named as the function is.
        if is_binding_phantom(source) && !self.body_of.contains_key(&source.block_id) {
            let base = func.name.as_deref().map(base_fn_name)?;
            if source.name.as_deref() != Some(base)
                || func.capture_names.get(index).map(String::as_str) != Some(base)
            {
                return None;
            }
            return match self.sets_of.get(&site).map(Vec::as_slice) {
                Some([set]) => self.resolve_at(*set, fuel),
                _ => None,
            };
        }
        self.resolve_at(input, fuel)
    }

    /// Ask the same question of every site that makes a closure of `f`, and
    /// answer only when they all agree.
    fn agree<T: PartialEq>(
        &self,
        f: FunctionId,
        mut ask: impl FnMut(TermId) -> Option<T>,
    ) -> Option<T> {
        let mut found: Option<T> = None;
        for &site in self.closure_sites.get(&f)? {
            let answer = ask(site)?;
            match &found {
                None => found = Some(answer),
                Some(prev) if *prev == answer => {}
                Some(_) => return None,
            }
        }
        found
    }

    /// The cell a term denotes, when it denotes exactly one: the `CellNew`
    /// that allocates it, or — for a `state var`, whose slot holds a cell
    /// allocated inside the state's own init block — the `StateInit`. The two
    /// never name the same cell, which is all a caller compares them for.
    fn cell_of(&self, id: TermId, fuel: u32) -> Option<TermId> {
        let fuel = fuel.checked_sub(1)?;
        if self.phi_dests.contains(&id) {
            return None;
        }
        let term = self.program.get_term(id);
        match term.op {
            TermOp::CellNew | TermOp::StateInit => Some(id),
            TermOp::Copy if term.inputs.len() == 1 => self.cell_of(term.inputs[0], fuel),
            TermOp::Copy if is_binding_phantom(term) => {
                let f = *self.body_of.get(&term.block_id)?;
                let index = self
                    .function(f)
                    .capture_registers
                    .iter()
                    .position(|r| *r == term.register)?;
                self.agree(f, |site| {
                    self.cell_of(*self.program.get_term(site).inputs.get(index)?, fuel)
                })
            }
            _ => None,
        }
    }

    /// What a cell holds whenever it holds a function: every value ever
    /// written to it must resolve, and to the same list. A `nil` initializer
    /// is the hoisting placeholder — a call that reads it fails the same way
    /// however its arguments are written — so it does not count against the
    /// cell.
    fn cell_contents(&self, cell: TermId, fuel: u32) -> Option<Vec<FunctionId>> {
        let mut found: Option<Vec<FunctionId>> = None;
        let mut admit = |value: TermId| -> Option<()> {
            let fns = self.resolve_at(value, fuel)?;
            match &found {
                None => found = Some(fns),
                Some(prev) if *prev == fns => {}
                Some(_) => return None,
            }
            Some(())
        };
        let allocation = self.program.get_term(cell);
        // A persisted cell holds whatever an earlier run left in it.
        if !matches!(allocation.op, TermOp::CellNew) {
            return None;
        }
        let init = *allocation.inputs.first()?;
        if !self.is_nil_constant(init) {
            admit(init)?;
        }
        for &write in &self.cell_writes {
            let term = self.program.get_term(write);
            // A write whose target cannot be pinned down might be to this
            // cell.
            let target = self.cell_of(*term.inputs.first()?, fuel)?;
            if target == cell {
                admit(*term.inputs.get(1)?)?;
            }
        }
        found
    }

    fn is_nil_constant(&self, id: TermId) -> bool {
        match self.program.get_term(id).op {
            TermOp::Constant(c) => matches!(self.program.constants.get(c), ConstantValue::Nil),
            _ => false,
        }
    }

    /// The written name of each argument of a call term (`None` =
    /// positional), as long as its argument slice.
    pub fn arg_names(&self, term: &Term) -> Option<Vec<Option<&'p str>>> {
        let offset = term.op.arg_offset()?;
        let count = term.inputs.len().checked_sub(offset)?;
        if term.arg_names.is_empty() {
            return Some(vec![None; count]);
        }
        if term.arg_names.len() != count {
            return None;
        }
        term.arg_names
            .iter()
            .map(|n| match n {
                None => Some(None),
                Some(c) => self.program.get_string_constant(*c).map(Some),
            })
            .collect()
    }

    /// Resolve a `Call` or `BuiltinCall` term to what its callee receives.
    /// `None` for a method call (dispatched on the receiver's runtime class),
    /// for a callee the IR does not fix, and for a call that would fail to
    /// bind — there is nothing to preserve about a call that errors.
    pub fn binding(&self, id: TermId, natives: NativeSignatures) -> Option<CallBinding> {
        let term = self.program.get_term(id);
        let names = self.arg_names(term)?;
        match term.op {
            TermOp::Call => {
                let fns = self.resolve(*term.inputs.first()?)?;
                let defs: Vec<&FunctionDef> = fns.iter().map(|f| self.function(*f)).collect();
                let chosen = select_variant(&defs, names.len(), &names)?;
                Some(CallBinding {
                    callee: Callee::Function(fns[chosen]),
                    slots: fn_slots(defs[chosen], &names)?,
                })
            }
            TermOp::BuiltinCall(name) => {
                let name = self.program.get_string_constant(name)?;
                let slots = if names.iter().all(Option::is_none) {
                    (0..names.len()).collect()
                } else {
                    native_slots(name, &natives(name)?, &names)?
                };
                Some(CallBinding {
                    callee: Callee::Native,
                    slots,
                })
            }
            _ => None,
        }
    }
}

/// The variant of `fns` a call of `count` arguments named as `names` runs —
/// [`crate::backend::calls::resolve_overload`] restated over definitions, with
/// every failure (no variant accepts, or more than one does and none takes
/// exactly this many) folded into `None`.
pub fn select_variant(fns: &[&FunctionDef], count: usize, names: &[Option<&str>]) -> Option<usize> {
    let exact = |i: &usize| fns[*i].params.len() == count;
    let positional = names.iter().all(Option::is_none);
    // The VM hands an all-positional call an empty name list.
    let names: &[Option<&str>] = if positional { &[] } else { names };
    if let [only] = fns {
        // A bare closure is called as it is; binding decides whether the call
        // is one it takes.
        return accepts_call(only, count, names).then_some(0);
    }
    if positional && let Some(i) = (0..fns.len()).find(exact) {
        return Some(i);
    }
    let accepting: Vec<usize> = (0..fns.len())
        .filter(|&i| accepts_call(fns[i], count, names))
        .collect();
    match accepting.as_slice() {
        [] => None,
        [only] => Some(*only),
        many => {
            let exact = many.iter().copied().find(exact)?;
            let mut skipping = many.iter().copied().filter(|&i| {
                skips_a_parameter(&fns[i].params, count, names.iter().flatten().copied())
            });
            match (skipping.next(), skipping.next()) {
                (None, _) => Some(exact),
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
    }
}

/// The parameter slot each argument of an accepted call fills.
pub fn fn_slots(func: &FunctionDef, names: &[Option<&str>]) -> Option<Vec<usize>> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| match name {
            None => Some(i),
            Some(name) => func.params.iter().position(|p| p == name),
        })
        .collect()
}

/// The parameter slot each argument of a named native call fills, by the
/// rule the VM and the compiler bind it with (the first declared form the
/// call fits).
pub fn native_slots(
    name: &str,
    sigs: &[NativeSignature],
    names: &[Option<&str>],
) -> Option<Vec<usize>> {
    if sigs.is_empty() {
        return None;
    }
    let indices: Vec<usize> = (0..names.len()).collect();
    // `order[slot]` is the argument filling that slot; a native takes no gaps,
    // so the list is a permutation of the arguments.
    let order = bind_native_args(name, sigs, &indices, names).ok()?;
    if order.len() != names.len() {
        return None;
    }
    let mut slots = vec![0; names.len()];
    for (slot, &arg) in order.iter().enumerate() {
        slots[arg] = slot;
    }
    Some(slots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(src: &str) -> (crate::env::Env, crate::program::ProgramId) {
        let mut env = crate::env::Env::new();
        let pid = env.load_program(src).expect("compiles");
        (env, pid)
    }

    /// The parameter lists every `Call` in `src` resolves to, in term order —
    /// `None` for one the resolver leaves alone.
    fn calls(src: &str) -> Vec<Option<Vec<String>>> {
        let (env, pid) = compile(src);
        let program = env.get_program(pid).unwrap();
        let resolver = CallResolver::new(program);
        program
            .terms
            .iter()
            .filter(|t| matches!(t.op, TermOp::Call))
            .filter(|t| {
                program
                    .source_map
                    .get(t.id)
                    .is_some_and(|s| s.file == crate::source_map::ENTRY_FILE)
            })
            .map(|t| match resolver.binding(t.id, &|_| None)?.callee {
                Callee::Function(f) => Some(resolver.function(f).params.clone()),
                Callee::Native => None,
            })
            .collect()
    }

    fn params(names: &[&str]) -> Option<Vec<String>> {
        Some(names.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn a_module_function_resolves_to_the_variant_the_count_selects() {
        let src = "fn f(a, b)\n  a + b\nend\nfn f(a, b, c)\n  a + b + c\nend\nprint(f(1, 2))\nprint(f(1, 2, 3))\n";
        assert_eq!(calls(src), [params(&["a", "b"]), params(&["a", "b", "c"])]);
    }

    #[test]
    fn a_call_inside_a_function_resolves_through_the_capture() {
        let src =
            "fn inner(x, y, z)\n  x\nend\nfn outer()\n  inner(1, 2, 3)\nend\nprint(outer())\n";
        let got = calls(src);
        assert!(got.contains(&params(&["x", "y", "z"])), "{got:?}");
    }

    #[test]
    fn a_function_declared_later_resolves_through_its_cell() {
        let src = "fn early()\n  late(1, 2, 3)\nend\nfn late(x, y, z)\n  x\nend\nprint(early())\n";
        let got = calls(src);
        assert!(got.contains(&params(&["x", "y", "z"])), "{got:?}");
    }

    #[test]
    fn recursion_through_an_overload_set_resolves_to_the_set() {
        let src = "fn walk(n, acc, step)\n  if n <= 0 then acc else walk(n - 1, acc + step) end\nend\nfn walk(n, acc)\n  walk(n, acc, 1)\nend\nprint(walk(3, 0))\n";
        let got = calls(src);
        assert!(got.iter().all(Option::is_some), "{got:?}");
        assert!(got.contains(&params(&["n", "acc", "step"])), "{got:?}");
    }

    #[test]
    fn an_opaque_callee_does_not_resolve() {
        // A parameter, and a name rebound under a branch.
        let src = "fn apply(f)\n  f(1, 2, 3)\nend\nfn a(x, y, z)\n  x\nend\nfn b(x, y, z)\n  y\nend\nlet g = a\nif len([1]) > 0 then\n  g = b\nend\nprint(g(1, 2, 3))\nprint(apply(a))\n";
        let got = calls(src);
        // `f(1, 2, 3)` and `g(1, 2, 3)` are the two that must stay unresolved.
        assert_eq!(got.iter().filter(|c| c.is_none()).count(), 2, "{got:?}");
    }

    #[test]
    fn a_var_resolves_only_while_every_write_agrees() {
        let one = "var g = fn(x, y, z) x end\nprint(g(1, 2, 3))\n";
        assert_eq!(calls(one), [params(&["x", "y", "z"])]);
        let two = "var g = fn(x, y, z) x end\nset g = fn(p, q, r) p end\nprint(g(1, 2, 3))\n";
        assert_eq!(calls(two), [None]);
    }

    #[test]
    fn select_variant_follows_the_runtime_rule() {
        let def = |params: &[&str], optional: u16| FunctionDef {
            id: FunctionId(0),
            name: Some("f".into()),
            params: params.iter().map(|s| s.to_string()).collect(),
            optional_params: optional,
            body_block: BlockId(0),
            capture_names: Vec::new(),
            capture_registers: Vec::new(),
            self_ref_register: None,
            register_count: 0,
        };
        let (two, three) = (def(&["a", "b"], 0), def(&["x", "y", "z"], 1));
        let fns = [&two, &three];
        // Exact count wins for a positional call, though `three` also takes 2.
        assert_eq!(select_variant(&fns, 2, &[None, None]), Some(0));
        // Names pick the only variant that has them.
        assert_eq!(select_variant(&fns, 2, &[Some("x"), Some("y")]), Some(1));
        assert_eq!(select_variant(&fns, 2, &[Some("a"), Some("b")]), Some(0));
        assert_eq!(select_variant(&fns, 2, &[None, Some("nope")]), None);
        assert_eq!(select_variant(&fns, 4, &[None; 4]), None);
    }
}
