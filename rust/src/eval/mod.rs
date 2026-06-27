//! Eval — step-based IR evaluator.
//!
//! Executes a program by walking the term graph one term at a time.
//!
//! The evaluator is split by concern:
//! - `exec`    — per-term dispatch and data-structure ops
//! - `ops`     — arithmetic and comparisons (incl. dual numbers and vec2)
//! - `loops`   — for / numeric-for / while loop terms
//! - `call`    — calls, closures, native functions, map/filter/reduce
//! - `state`   — persistent `state` variables and runtime state keys
//! - `pattern` — `match` execution and pattern matching
//! - `error`   — error annotation: snippets, provenance, stack traces

mod call;
mod error;
mod exec;
mod loops;
mod ops;
mod pattern;
mod state;

pub use error::format_source_snippet;

use smallvec::SmallVec;

use std::collections::HashMap;

use crate::heap::Heap;
use crate::native_fn::NativeFnTable;
use crate::program::*;
use crate::stack::{Frame, Stack};
use crate::symbol::{SymbolId, SymbolTable};
use crate::trace::TraceBuffer;
use crate::value::{self, Value};

/// Result of a single evaluation step.
#[derive(Debug)]
pub enum StepResult {
    Continue,
    Complete(Value),
    Error(String),
}

/// Signal for control flow within the evaluator.
enum ControlFlow {
    /// Normal — advance to next term
    Advance,
    /// Frame was pushed — don't advance, execute new frame
    FramePushed,
    /// Return from function
    Return(Value),
    /// Break from loop
    Break,
    /// Continue to next iteration
    Continue,
    /// Fatal error
    Error(String),
}

/// Runtime closure — captures + function reference.
pub struct RuntimeClosure {
    pub function_id: FunctionId,
    pub captures: Vec<Value>,
}

/// The evaluator: a bundle of borrows over the runtime data owned by `Env`,
/// built fresh for each `step` call. All evaluation logic lives in methods
/// on this struct so handlers can reach any part of the runtime without
/// threading individual references around.
pub struct Evaluator<'a> {
    pub program: &'a Program,
    pub stack: &'a mut Stack,
    pub heap: &'a mut Heap,
    pub closures: &'a mut Vec<RuntimeClosure>,
    pub overload_sets: &'a mut Vec<Vec<OverloadEntry>>,
    pub native_fns: &'a NativeFnTable,
    pub output: &'a mut Vec<String>,
    pub trace: &'a mut TraceBuffer,
    /// Symbol table for binding native/host state to interned names.
    pub symbols: &'a mut SymbolTable,
    /// Per-symbol buffered output channels (script pushes, host pulls).
    pub output_buffers: &'a mut HashMap<SymbolId, Vec<Value>>,
}

impl<'a> Evaluator<'a> {
    /// Execute one step: evaluate the current term and advance.
    pub fn step(&mut self) -> StepResult {
        let program = self.program;

        let frame_idx = match self.stack.frames.len().checked_sub(1) {
            Some(idx) => idx,
            None => return StepResult::Complete(Value::Nil),
        };

        let current_term_id = match self.stack.frames[frame_idx].current_term {
            Some(tid) => tid,
            None => {
                // Block is done — pop frame
                return self.pop_frame();
            }
        };

        // If break_flag or continue_flag is set and the current frame is a
        // direct loop body, skip remaining terms and pop immediately so the
        // parent loop term can handle the break/continue on its next
        // execution. Exception: when the current term is itself a nested
        // loop, let it execute — its exec will consume the flag (an inner
        // loop that just broke returns control to its enclosing body via its
        // own exec path).
        if (self.stack.break_flag || self.stack.continue_flag)
            && self.stack.frames[frame_idx].is_loop_body
        {
            let cur = program.get_term(current_term_id);
            if !matches!(
                cur.op,
                TermOp::ForLoop | TermOp::NumericForLoop | TermOp::WhileLoop
            ) {
                return self.pop_frame();
            }
        }

        let term = program.get_term(current_term_id);

        // Read input values (SmallVec avoids heap allocation for ≤4 inputs)
        let input_values: SmallVec<[Value; 4]> = term
            .inputs
            .iter()
            .map(|&input_tid| self.read_register(input_tid))
            .collect();

        match self.exec_term(term, &input_values) {
            ControlFlow::Advance => {
                self.advance(term);
                // Record the executed term into the trace buffer (no-op when
                // disabled) and optionally print a line via the legacy
                // PETAL_TRACE eprintln path.
                if self.trace.enabled {
                    let result_val = self
                        .stack
                        .frames
                        .last()
                        .map(|f| f.get_register(term.register.0 as usize))
                        .unwrap_or(Value::Nil);
                    self.trace.push(term.id, &input_values, result_val);
                }
                self.trace_term(term, &input_values);
                StepResult::Continue
            }
            // New frame was pushed — continue executing it
            ControlFlow::FramePushed => StepResult::Continue,
            ControlFlow::Return(val) => {
                self.handle_return(val);
                StepResult::Continue
            }
            ControlFlow::Break => {
                // Set the flag and advance past the break term; the
                // enclosing loop term will catch it.
                self.stack.break_flag = true;
                self.advance(term);
                StepResult::Continue
            }
            ControlFlow::Continue => {
                self.stack.continue_flag = true;
                self.advance(term);
                StepResult::Continue
            }
            ControlFlow::Error(msg) => {
                StepResult::Error(self.annotate_error(msg, current_term_id))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Register access
    // -----------------------------------------------------------------------

    /// Read a value from the register of the term that produced it.
    fn read_register(&self, term_id: TermId) -> Value {
        let term = self.program.get_term(term_id);
        let term_block = term.block_id;
        let reg_idx = term.register.0 as usize;

        // Fast path: most reads are from the current frame's block.
        let top = self.stack.frames.last().unwrap();
        if top.block_id == term_block {
            return top.get_register(reg_idx);
        }

        // Slow path: walk parent_frame links for cross-block reads. Hit by
        // (a) captures — closures reading outer-function locals through a
        // phantom in the function body block, (b) loop-carry reads — bodies
        // referencing a `Phi` term that lives in the parent block before the
        // `ForLoop`/`WhileLoop`, and (c) ordinary reads from a child block
        // of a variable bound in an enclosing block.
        let mut frame_idx = match top.parent_frame {
            Some(parent) => parent,
            None => return Value::Nil,
        };
        loop {
            let frame = &self.stack.frames[frame_idx];
            if frame.block_id == term_block {
                return frame.get_register(reg_idx);
            }
            match frame.parent_frame {
                Some(parent) => frame_idx = parent,
                None => return Value::Nil,
            }
        }
    }

    /// Advance the current frame to the term after `term`.
    fn advance(&mut self, term: &Term) {
        if let Some(frame) = self.stack.frames.last_mut() {
            frame.current_term = term.block_next;
        }
    }

    /// Write a value to the current term's register.
    fn write_register(&mut self, term: &Term, value: Value) {
        if let Some(frame) = self.stack.frames.last_mut() {
            frame.set_register(term.register.0 as usize, value);
        }
    }

    /// Store `value` as the term's result and advance to the next term.
    /// The standard way for a term handler to finish.
    fn produce(&mut self, term: &Term, value: Value) -> ControlFlow {
        self.write_register(term, value);
        ControlFlow::Advance
    }

    // -----------------------------------------------------------------------
    // Frame lifecycle
    // -----------------------------------------------------------------------

    /// Push a frame for a child block of `parent_term`, advancing the parent
    /// frame past the control-flow term first.
    fn push_child_frame(&mut self, block_id: BlockId, parent_term: &Term) {
        let block = self.program.get_block(block_id);
        let parent_frame_idx = self.stack.frames.len() - 1;

        if let Some(parent_frame) = self.stack.frames.last_mut() {
            parent_frame.current_term = parent_term.block_next;
        }

        self.stack.push_frame(Frame::new(
            block_id,
            block.entry,
            block.register_count as usize,
            Some(parent_term.id),
            Some(parent_frame_idx),
        ));
    }

    /// Pop the current frame and handle the result.
    fn pop_frame(&mut self) -> StepResult {
        let program = self.program;

        // Process phi carry-outs before popping. Each slot copies a source
        // register (walked from the current child frame via read_register) to
        // a destination register in the parent frame. Done before the pop so
        // src reads can walk parent_frame links from the current top.
        if let Some(top) = self.stack.frames.last() {
            let child_block = program.get_block(top.block_id);
            if !child_block.phi_outs.is_empty() {
                let slots: Vec<(TermId, TermId)> = child_block
                    .phi_outs
                    .iter()
                    .map(|p| (p.src_term, p.dest_term))
                    .collect();
                for (src, dest) in slots {
                    let val = self.read_register(src);
                    let dest_reg = program.get_term(dest).register.0 as usize;
                    if self.stack.frames.len() >= 2 {
                        let parent_idx = self.stack.frames.len() - 2;
                        self.stack.frames[parent_idx].set_register(dest_reg, val);
                    }
                }
            }
        }

        let frame = match self.stack.pop_frame() {
            Some(f) => f,
            None => return StepResult::Complete(Value::Nil),
        };

        // When a loop body pops due to continue, clear the flag immediately
        // so it doesn't propagate to outer loop bodies.
        if self.stack.continue_flag && frame.is_loop_body {
            self.stack.continue_flag = false;
        }

        // The block's result is its last term's value
        let block = program.get_block(frame.block_id);
        let result = self.block_result(&frame, block);

        // Always store the result for synchronous closure callers
        self.stack.last_pop_result = Some(result);

        if self.stack.frames.is_empty() {
            // The top-level frame just popped. Capture top-level named
            // functions so the host can invoke them via `Env::call_function`
            // without re-running the program.
            if frame.block_id == program.root_block {
                self.capture_root_functions(&frame);
            }
            // Program complete
            return StepResult::Complete(result);
        }

        // Write result to the parent term's register
        if let Some(return_term) = frame.return_term {
            let parent_term = program.get_term(return_term);
            if let Some(parent_frame) = self.stack.frames.last_mut() {
                parent_frame.set_register(parent_term.register.0 as usize, result);
            }

            // Lazy-init bookkeeping: if we just popped a StateInit's init
            // block, the popped frame's last value (now in the parent's
            // register) is the value to bind into the persistent state map.
            if matches!(parent_term.op, TermOp::StateInit) {
                self.finish_state_init(parent_term, result);
            }
        }

        StepResult::Continue
    }

    /// A finished block's result: the value of the last term in the block.
    fn block_result(&self, frame: &Frame, block: &Block) -> Value {
        let mut current = block.entry;
        let mut last_tid = None;
        while let Some(tid) = current {
            last_tid = Some(tid);
            current = self.program.get_term(tid).block_next;
        }
        match last_tid {
            Some(tid) => frame.get_register(self.program.get_term(tid).register.0 as usize),
            None => Value::Nil,
        }
    }

    /// Pop frames up to and including the current function call frame, then
    /// deliver `value` to the caller and advance it past the Call term.
    fn handle_return(&mut self, value: Value) {
        loop {
            let frame = match self.stack.pop_frame() {
                Some(f) => f,
                None => return,
            };
            // Function call frames are the ones without a parent_frame link.
            if frame.parent_frame.is_none() {
                // Store for synchronous closure callers
                self.stack.last_pop_result = Some(value);
                if let Some(return_term) = frame.return_term {
                    let parent_term = self.program.get_term(return_term);
                    if let Some(caller_frame) = self.stack.frames.last_mut() {
                        caller_frame.set_register(parent_term.register.0 as usize, value);
                        // Advance past the Call term
                        caller_frame.current_term = parent_term.block_next;
                    }
                }
                return;
            }
        }
    }

    /// Record top-level named functions from the just-popped root frame into
    /// `stack.functions`, keyed by name. Only `Closure`/`OverloadSet` values
    /// are captured (a user-defined function or a lambda bound to a name);
    /// builtins and non-callable bindings are skipped. When a name has both a
    /// capture phantom and a `MakeClosure` term, the later (callable) one wins,
    /// matching normal name-resolution order.
    fn capture_root_functions(&mut self, root_frame: &Frame) {
        let root = self.program.root_block;
        let Some(term_ids) = self.program.block_terms.get(&root) else {
            return;
        };
        let captured: Vec<(String, Value)> = term_ids
            .iter()
            .filter_map(|&tid| {
                let term = self.program.get_term(tid);
                let name = term.name.as_ref()?;
                let val = root_frame.get_register(term.register.0 as usize);
                matches!(val, Value::Closure(_) | Value::OverloadSet(_))
                    .then(|| (name.clone(), val))
            })
            .collect();
        for (name, val) in captured {
            self.stack.functions.insert(name, val);
        }
    }
}
