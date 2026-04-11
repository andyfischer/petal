//! Eval - Step-based IR evaluator.
//!
//! Executes a program by walking the term graph one term at a time.

use indexmap::IndexMap;

use smallvec::SmallVec;

use crate::ast::*;
use crate::builtins;
use crate::constant_table::{ConstantId, ConstantValue};
use crate::heap::Heap;
use crate::native_fn::{NativeFnTable, PetalCxt};
use crate::program::*;
use crate::stack::{Frame, LoopKeyPart, LoopState, RuntimeStateKey, Stack};
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

/// The evaluator operates on Env's data.
pub struct Evaluator;

impl Evaluator {
    /// Walk the stack frames collecting iteration indices from active loops.
    /// Returns a SmallVec of LoopKeyParts for building RuntimeStateKeys.
    fn resolve_loop_context(stack: &Stack) -> smallvec::SmallVec<[LoopKeyPart; 2]> {
        let mut parts = smallvec::SmallVec::new();
        for frame in &stack.frames {
            for (_, loop_state) in &frame.loop_states {
                match loop_state {
                    LoopState::For { index, .. } => {
                        // index is 1-past-current (already incremented), so current = index - 1
                        parts.push(LoopKeyPart::Index(index.saturating_sub(1)));
                    }
                    LoopState::WhileCondition { iteration }
                    | LoopState::WhileBody { iteration } => {
                        parts.push(LoopKeyPart::Index(*iteration));
                    }
                }
            }
        }
        parts
    }

    /// Build the RuntimeStateKey for a state term, taking into account loop context
    /// and explicit keys.
    fn resolve_runtime_state_key(
        term: &Term,
        inputs: &[Value],
        stack: &Stack,
        heap: &Heap,
    ) -> RuntimeStateKey {
        let base = term.state_key.unwrap();
        if term.inputs.len() > 1 && inputs.len() > 1 {
            // Explicit key (Phase 2): use hashed value instead of loop indices.
            // The key value is the last input (index 1 for StateInit, index 1 for StateWrite).
            let key_val = inputs.last().unwrap();
            let hash = value::hash_value(key_val, heap);
            RuntimeStateKey {
                base,
                loop_indices: smallvec::smallvec![LoopKeyPart::Explicit(hash)],
            }
        } else if term.in_loop {
            RuntimeStateKey {
                base,
                loop_indices: Self::resolve_loop_context(stack),
            }
        } else {
            RuntimeStateKey {
                base,
                loop_indices: smallvec::SmallVec::new(),
            }
        }
    }

    /// Execute one step: evaluate the current term and advance.
    pub fn step(
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> StepResult {
        let frame_idx = match stack.frames.len().checked_sub(1) {
            Some(idx) => idx,
            None => return StepResult::Complete(Value::Nil),
        };

        let current_term_id = match stack.frames[frame_idx].current_term {
            Some(tid) => tid,
            None => {
                // Block is done — pop frame
                return Self::pop_frame(program, stack);
            }
        };

        // If break_flag or continue_flag is set and the current frame is a direct loop body,
        // skip remaining terms and pop immediately so the parent loop term
        // can handle the break/continue on its next execution.
        if (stack.break_flag || stack.continue_flag) && stack.frames[frame_idx].is_loop_body {
            return Self::pop_frame(program, stack);
        }

        let term = program.get_term(current_term_id);

        // Read input values (SmallVec avoids heap allocation for ≤4 inputs)
        let input_values: SmallVec<[Value; 4]> = term
            .inputs
            .iter()
            .map(|&input_tid| Self::read_register(program, stack, input_tid))
            .collect();

        // Execute the term
        let result = Self::exec_term(
            term,
            &input_values,
            program,
            stack,
            heap,
            closures,
            overload_sets,
            native_fns,
            output,
            trace,
        );

        match result {
            ControlFlow::Advance => {
                // Store result and advance
                Self::advance(stack, term);
                // Record the executed term into the trace buffer (no-op when
                // disabled) and optionally print a line via the legacy
                // PETAL_TRACE eprintln path.
                if trace.enabled {
                    let result_val = stack
                        .frames
                        .last()
                        .and_then(|f| f.registers.get(term.register.0 as usize).copied())
                        .unwrap_or(Value::Nil);
                    trace.push(term.id, &input_values, result_val);
                }
                Self::trace_term(program, stack, heap, term, &input_values);
                StepResult::Continue
            }
            ControlFlow::FramePushed => {
                // New frame was pushed — continue executing it
                StepResult::Continue
            }
            ControlFlow::Return(val) => {
                // Pop frames to function boundary
                Self::handle_return(program, stack, val);
                StepResult::Continue
            }
            ControlFlow::Break => {
                stack.break_flag = true;
                // Advance past the break term then the loop handler will catch it
                Self::advance(stack, term);
                StepResult::Continue
            }
            ControlFlow::Continue => {
                stack.continue_flag = true;
                Self::advance(stack, term);
                StepResult::Continue
            }
            ControlFlow::Error(msg) => {
                // Annotate error with source position from the term that failed
                let mut error_msg = if let Some(span) = program.source_map.get(current_term_id) {
                    if span.start.line > 0 {
                        format!(
                            "{} [line {}, column {}]",
                            msg, span.start.line, span.start.column
                        )
                    } else {
                        msg
                    }
                } else {
                    msg
                };

                // Append provenance: up to 5 nearest named ancestors with spans
                let provenance = Self::format_provenance(program, current_term_id, 5);
                if !provenance.is_empty() {
                    error_msg.push_str("\nCaused by:");
                    for entry in &provenance {
                        error_msg.push_str(&format!("\n  {}", entry));
                    }
                }

                // Build stack trace from call frames
                let trace = Self::build_stack_trace(program, stack);
                if !trace.is_empty() {
                    error_msg.push_str("\nStack trace:");
                    for entry in &trace {
                        error_msg.push_str(&format!("\n  {}", entry));
                    }
                }

                StepResult::Error(error_msg)
            }
        }
    }

    /// Read a value from the register of the term that produced it.
    fn read_register(program: &Program, stack: &Stack, term_id: TermId) -> Value {
        let term = program.get_term(term_id);
        let term_block = term.block_id;
        let reg_idx = term.register.0 as usize;

        // Fast path: most reads are from the current frame's block
        let top = stack.frames.last().unwrap();
        if top.block_id == term_block {
            return if reg_idx < top.registers.len() {
                top.registers[reg_idx]
            } else {
                Value::Nil
            };
        }

        // Slow path: walk parent_frame links for cross-block reads
        let mut frame_idx = match top.parent_frame {
            Some(parent) => parent,
            None => return Value::Nil,
        };
        loop {
            let frame = &stack.frames[frame_idx];
            if frame.block_id == term_block {
                return if reg_idx < frame.registers.len() {
                    frame.registers[reg_idx]
                } else {
                    Value::Nil
                };
            }
            match frame.parent_frame {
                Some(parent) => frame_idx = parent,
                None => return Value::Nil,
            }
        }
    }

    /// Store result in current frame's register and advance to next term.
    fn advance(stack: &mut Stack, term: &Term) {
        if let Some(frame) = stack.frames.last_mut() {
            let reg = term.register.0 as usize;
            // Ensure register file is large enough
            if reg >= frame.registers.len() {
                frame.registers.resize(reg + 1, Value::Nil);
            }
            // Note: result was already written by exec_term for most ops
            frame.current_term = term.block_next;
        }
    }

    /// Write a value to the current term's register.
    fn write_register(stack: &mut Stack, term: &Term, value: Value) {
        if let Some(frame) = stack.frames.last_mut() {
            let reg = term.register.0 as usize;
            if reg >= frame.registers.len() {
                frame.registers.resize(reg + 1, Value::Nil);
            }
            frame.registers[reg] = value;
        }
    }

    /// Emit a one-line trace event to stderr when PETAL_TRACE=1.
    /// Reads the result value from the term's register post-advance.
    fn trace_term(
        program: &Program,
        stack: &Stack,
        heap: &Heap,
        term: &Term,
        inputs: &[Value],
    ) {
        use std::sync::OnceLock;
        static ENABLED: OnceLock<bool> = OnceLock::new();
        let enabled = *ENABLED.get_or_init(|| {
            std::env::var("PETAL_TRACE").is_ok() || std::env::var("PETAL_DEBUG").is_ok()
        });
        if !enabled {
            return;
        }

        // Read result from the current frame's register for this term
        let result = stack
            .frames
            .last()
            .and_then(|f| f.registers.get(term.register.0 as usize).copied())
            .unwrap_or(Value::Nil);

        let input_strs: Vec<String> = inputs
            .iter()
            .map(|v| value::value_to_display_string(v, heap))
            .collect();
        let result_str = value::value_to_display_string(&result, heap);

        let span = program.source_map.get(term.id);
        let loc = match span {
            Some(s) if s.start.line > 0 => format!("{}:{}", s.start.line, s.start.column),
            _ => "-".to_string(),
        };

        let name = term.name.as_deref().unwrap_or("");
        eprintln!(
            "[trace] t{:<3} {:<4} {:<20} {:?} inputs=[{}] -> {}",
            term.id.0,
            loc,
            name,
            term.op,
            input_strs.join(", "),
            result_str,
        );
    }

    /// Walk provenance of the failing term and format up to `max` nearest
    /// ancestors that have both a name and a source span. This surfaces the
    /// user-visible variables that fed into the failure so error messages
    /// point at causes, not just the failing operation.
    fn format_provenance(program: &Program, failing: TermId, max: usize) -> Vec<String> {
        let (ancestors, _edges) = program.trace_provenance(failing);
        let mut out = Vec::new();
        for aid in ancestors {
            if out.len() >= max {
                break;
            }
            let term = program.get_term(aid);
            let Some(name) = term.name.as_deref() else {
                continue;
            };
            let Some(span) = program.source_map.get(aid) else {
                continue;
            };
            if span.start.line == 0 {
                continue;
            }
            out.push(format!(
                "{} [line {}, column {}]",
                name, span.start.line, span.start.column
            ));
        }
        out
    }

    /// Build a stack trace from the current call frames.
    /// Returns a list of strings like "in foo() [line 5, column 1]".
    fn build_stack_trace(program: &Program, stack: &Stack) -> Vec<String> {
        let mut trace = Vec::new();

        // Walk frames from top to bottom, collecting call frames with function names
        for frame in stack.frames.iter().rev() {
            if let Some(ref name) = frame.fn_name {
                // Find the call site: the return_term is the Call term in the parent
                if let Some(return_tid) = frame.return_term {
                    if let Some(span) = program.source_map.get(return_tid) {
                        if span.start.line > 0 {
                            trace.push(format!("in {}() [line {}, column {}]", name, span.start.line, span.start.column));
                            continue;
                        }
                    }
                }
                trace.push(format!("in {}()", name));
            }
        }

        // Only show trace if there are actual function calls (not just the root frame)
        if trace.is_empty() {
            return trace;
        }

        trace
    }

    /// Pop the current frame and handle the result.
    fn pop_frame(program: &Program, stack: &mut Stack) -> StepResult {
        let frame = match stack.pop_frame() {
            Some(f) => f,
            None => return StepResult::Complete(Value::Nil),
        };

        // When a loop body pops due to continue, clear the flag immediately
        // so it doesn't propagate to outer loop bodies.
        if stack.continue_flag && frame.is_loop_body {
            stack.continue_flag = false;
        }

        // Get the last term's value as the block result
        let block = program.get_block(frame.block_id);
        let result = Self::get_last_register_value(&frame, block, program);

        // Always store the result for synchronous closure callers
        stack.last_pop_result = Some(result);

        if stack.frames.is_empty() {
            // Program complete
            return StepResult::Complete(result);
        }

        // Write result to the parent term's register
        if let Some(return_term) = frame.return_term {
            let parent_term = program.get_term(return_term);
            let reg = parent_term.register.0 as usize;
            if let Some(parent_frame) = stack.frames.last_mut() {
                if reg >= parent_frame.registers.len() {
                    parent_frame.registers.resize(reg + 1, Value::Nil);
                }
                parent_frame.registers[reg] = result;
            }
        }

        StepResult::Continue
    }

    fn get_last_register_value(frame: &Frame, block: &Block, program: &Program) -> Value {
        // Find the last term in this block and read its register
        let mut current = block.entry;
        let mut last_tid = None;
        while let Some(tid) = current {
            last_tid = Some(tid);
            current = program.get_term(tid).block_next;
        }
        if let Some(tid) = last_tid {
            let term = program.get_term(tid);
            let reg = term.register.0 as usize;
            if reg < frame.registers.len() {
                return frame.registers[reg];
            }
        }
        Value::Nil
    }

    fn handle_return(program: &Program, stack: &mut Stack, value: Value) {
        // Pop frames until we find a function call frame (parent_frame == None)
        loop {
            let frame = match stack.pop_frame() {
                Some(f) => f,
                None => return,
            };
            if frame.parent_frame.is_none() {
                // Store for synchronous closure callers
                stack.last_pop_result = Some(value);
                // This was a function frame — write return value to caller
                if let Some(return_term) = frame.return_term {
                    let parent_term = program.get_term(return_term);
                    let reg = parent_term.register.0 as usize;
                    if let Some(caller_frame) = stack.frames.last_mut() {
                        if reg >= caller_frame.registers.len() {
                            caller_frame.registers.resize(reg + 1, Value::Nil);
                        }
                        caller_frame.registers[reg] = value;
                        // Advance past the Call term
                        caller_frame.current_term = parent_term.block_next;
                    }
                }
                return;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Term execution
    // -----------------------------------------------------------------------

    fn exec_term(
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> ControlFlow {
        match &term.op {
            TermOp::Constant(cid) => {
                let val = Self::constant_to_value(*cid, program, heap);
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Error(cid) => {
                let msg = program.get_string_constant(*cid)
                    .unwrap_or("Unknown error")
                    .to_string();
                ControlFlow::Error(msg)
            }

            TermOp::Copy => {
                // Identity / variable reference
                let val = inputs.first().copied().unwrap_or(Value::Nil);
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Assign(target_tid) => {
                // Write value to the target term's register in its frame
                let val = inputs.first().copied().unwrap_or(Value::Nil);
                let target_term = program.get_term(*target_tid);
                let target_block = target_term.block_id;
                let target_reg = target_term.register.0 as usize;

                // Walk parent_frame links to find the frame holding the target block
                let mut frame_idx = stack.frames.len() - 1;
                loop {
                    if stack.frames[frame_idx].block_id == target_block {
                        if target_reg < stack.frames[frame_idx].registers.len() {
                            stack.frames[frame_idx].registers[target_reg] = val;
                        }
                        break;
                    }
                    match stack.frames[frame_idx].parent_frame {
                        Some(parent) => frame_idx = parent,
                        None => break,
                    }
                }

                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Add => Self::numeric_binop(term, inputs, stack,
                |a, b| Value::Int(a + b), |a, b| Value::Float(a + b)),
            TermOp::Sub => Self::numeric_binop(term, inputs, stack,
                |a, b| Value::Int(a - b), |a, b| Value::Float(a - b)),
            TermOp::Mul => Self::numeric_binop(term, inputs, stack,
                |a, b| Value::Int(a * b), |a, b| Value::Float(a * b)),
            TermOp::Div => {
                if inputs.len() < 2 {
                    return ControlFlow::Error("Div: missing inputs".into());
                }
                match (&inputs[0], &inputs[1]) {
                    (_, Value::Int(0)) => return ControlFlow::Error("Division by zero".into()),
                    (_, Value::Float(f)) if *f == 0.0 => return ControlFlow::Error("Division by zero".into()),
                    _ => {}
                }
                Self::numeric_binop(term, inputs, stack,
                    |a, b| Value::Int(a / b), |a, b| Value::Float(a / b))
            }
            TermOp::Mod => {
                if inputs.len() < 2 {
                    return ControlFlow::Error("Mod: missing inputs".into());
                }
                Self::numeric_binop(term, inputs, stack,
                    |a, b| Value::Int(a % b), |a, b| Value::Float(a % b))
            }

            TermOp::Neg => {
                let val = match inputs.first() {
                    Some(Value::Int(n)) => Value::Int(-n),
                    Some(Value::Float(f)) => Value::Float(-f),
                    Some(Value::Dual { value, derivative }) => {
                        Value::Dual { value: -value, derivative: -derivative }
                    }
                    Some(Value::Vec2(x, y)) => Value::Vec2(-x, -y),
                    Some(v) => return ControlFlow::Error(format!("Cannot negate {}", v.type_name())),
                    None => return ControlFlow::Error("Neg: missing input".into()),
                };
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Not => {
                let val = match inputs.first() {
                    Some(v) => Value::Bool(!v.is_truthy()),
                    None => return ControlFlow::Error("Not: missing input".into()),
                };
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Eq => {
                let val = Value::Bool(value::values_equal(&inputs[0], &inputs[1], heap));
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }
            TermOp::Ne => {
                let val = Value::Bool(!value::values_equal(&inputs[0], &inputs[1], heap));
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }
            TermOp::Lt => Self::comparison_op(term, inputs, stack, heap, |ord| ord == std::cmp::Ordering::Less),
            TermOp::Le => Self::comparison_op(term, inputs, stack, heap, |ord| ord != std::cmp::Ordering::Greater),
            TermOp::Gt => Self::comparison_op(term, inputs, stack, heap, |ord| ord == std::cmp::Ordering::Greater),
            TermOp::Ge => Self::comparison_op(term, inputs, stack, heap, |ord| ord != std::cmp::Ordering::Less),

            TermOp::Concat => {
                match (inputs[0], inputs[1]) {
                    (Value::List(a), Value::List(b)) => {
                        let mut combined = heap.get_list(a).to_vec();
                        combined.extend_from_slice(heap.get_list(b));
                        let id = heap.alloc_list(combined);
                        Self::write_register(stack, term, Value::List(id));
                    }
                    _ => {
                        let l = value::value_to_display_string(&inputs[0], heap);
                        let r = value::value_to_display_string(&inputs[1], heap);
                        let s = format!("{}{}", l, r);
                        let sid = heap.alloc_string(s);
                        Self::write_register(stack, term, Value::String(sid));
                    }
                }
                ControlFlow::Advance
            }

            TermOp::And => {
                let left = inputs[0];
                if !left.is_truthy() {
                    Self::write_register(stack, term, Value::Bool(false));
                    ControlFlow::Advance
                } else {
                    // Push frame for RHS block
                    let rhs_block = term.child_blocks[0];
                    Self::push_child_frame(program, stack, rhs_block, term);
                    ControlFlow::FramePushed
                }
            }

            TermOp::Or => {
                let left = inputs[0];
                if left.is_truthy() {
                    Self::write_register(stack, term, Value::Bool(true));
                    ControlFlow::Advance
                } else {
                    let rhs_block = term.child_blocks[0];
                    Self::push_child_frame(program, stack, rhs_block, term);
                    ControlFlow::FramePushed
                }
            }

            TermOp::Branch => {
                let cond = inputs[0];
                let block_idx = if cond.is_truthy() { 0 } else { 1 };
                let target_block = term.child_blocks[block_idx];
                Self::push_child_frame(program, stack, target_block, term);
                ControlFlow::FramePushed
            }

            TermOp::ForLoop => {
                Self::exec_for_loop(term, inputs, program, stack, heap)
            }

            TermOp::WhileLoop => {
                Self::exec_while_loop(term, program, stack)
            }

            TermOp::Break => ControlFlow::Break,
            TermOp::Continue => ControlFlow::Continue,

            TermOp::Return => {
                let val = inputs.first().copied().unwrap_or(Value::Nil);
                ControlFlow::Return(val)
            }

            TermOp::MakeOverloadSet => {
                // inputs are the closure values for each arity variant
                let mut entries = Vec::with_capacity(inputs.len());
                for &input in inputs {
                    if let Value::Closure(cid) = input {
                        let func = &program.functions[closures[cid.0 as usize].function_id.0 as usize];
                        entries.push(OverloadEntry {
                            arity: func.params.len(),
                            closure_id: cid,
                        });
                    }
                }
                let set_id = OverloadSetId(overload_sets.len() as u32);
                let overload_val = Value::OverloadSet(set_id);

                // Patch captures: each closure in the set may capture the base
                // function name (for recursion). At MakeClosure time that capture
                // was Nil because the overload set didn't exist yet. Fix it now.
                // Derive base name from internal name (e.g. "count#1" → "count").
                let base_name = entries.first().and_then(|e| {
                    let func = &program.functions[closures[e.closure_id.0 as usize].function_id.0 as usize];
                    func.name.as_ref().and_then(|n| n.rfind('#').map(|pos| n[..pos].to_string()))
                });
                if let Some(ref base) = base_name {
                    for entry in &entries {
                        let closure = &mut closures[entry.closure_id.0 as usize];
                        let func = &program.functions[closure.function_id.0 as usize];
                        for (i, cap_name) in func.capture_names.iter().enumerate() {
                            if cap_name == base {
                                closure.captures[i] = overload_val;
                            }
                        }
                    }
                }

                overload_sets.push(entries);
                Self::write_register(stack, term, overload_val);
                ControlFlow::Advance
            }

            TermOp::Call => {
                Self::exec_call(term, inputs, program, stack, heap, closures, overload_sets, native_fns, output, trace)
            }

            TermOp::MethodCall(method_cid) => {
                Self::exec_method_call(*method_cid, term, inputs, program, stack, heap, closures, overload_sets, native_fns, output, trace)
            }

            TermOp::MakeClosure(fn_id) => {
                let captures: Vec<Value> = inputs.to_vec();
                let closure_id = ClosureId(closures.len() as u32);
                closures.push(RuntimeClosure {
                    function_id: *fn_id,
                    captures,
                });
                Self::write_register(stack, term, Value::Closure(closure_id));
                ControlFlow::Advance
            }

            TermOp::StateInit => {
                let runtime_key = Self::resolve_runtime_state_key(term, inputs, stack, heap);
                if !stack.state.contains_key(&runtime_key) {
                    let init_val = inputs.first().copied().unwrap_or(Value::Nil);
                    stack.state.insert(runtime_key.clone(), init_val);
                }
                let val = stack.state[&runtime_key];
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::StateRead => {
                let runtime_key = Self::resolve_runtime_state_key(term, inputs, stack, heap);
                let val = stack.state.get(&runtime_key).copied().unwrap_or(Value::Nil);
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::StateWrite => {
                let runtime_key = Self::resolve_runtime_state_key(term, inputs, stack, heap);
                let val = inputs.first().copied().unwrap_or(Value::Nil);
                stack.state.insert(runtime_key, val);
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::AllocList => {
                let list_id = heap.alloc_list(inputs.to_vec());
                Self::write_register(stack, term, Value::List(list_id));
                ControlFlow::Advance
            }

            TermOp::AllocMap { fields } => {
                let mut map = IndexMap::new();
                for (i, field_cid) in fields.iter().enumerate() {
                    if let Some(key) = program.get_string_constant(*field_cid) {
                        let val = inputs.get(i).copied().unwrap_or(Value::Nil);
                        map.insert(key.to_string(), val);
                    }
                }
                let map_id = heap.alloc_map(map);
                Self::write_register(stack, term, Value::Map(map_id));
                ControlFlow::Advance
            }

            TermOp::AllocMapSpread { entries } => {
                let mut map = IndexMap::new();
                for entry in entries {
                    match entry {
                        MapSpreadEntry::Spread(idx) => {
                            let src = inputs.get(*idx).copied().unwrap_or(Value::Nil);
                            match src {
                                Value::Map(src_id) => {
                                    let src_map = heap.get_map(src_id);
                                    // Clone all fields from the source map
                                    let pairs: Vec<(String, Value)> = src_map
                                        .iter()
                                        .map(|(k, v)| (k.clone(), *v))
                                        .collect();
                                    for (k, v) in pairs {
                                        map.insert(k, v);
                                    }
                                }
                                Value::Nil => {} // Spreading nil is a no-op
                                _ => return ControlFlow::Error(format!(
                                    "Cannot spread {} into record (expected record)",
                                    src.type_name()
                                )),
                            }
                        }
                        MapSpreadEntry::Named(cid, idx) => {
                            if let Some(key) = program.get_string_constant(*cid) {
                                let val = inputs.get(*idx).copied().unwrap_or(Value::Nil);
                                map.insert(key.to_string(), val);
                            }
                        }
                    }
                }
                let map_id = heap.alloc_map(map);
                Self::write_register(stack, term, Value::Map(map_id));
                ControlFlow::Advance
            }

            TermOp::AllocElement { tag, prop_keys } => {
                let tag_str = match program.get_string_constant(*tag) {
                    Some(s) => s.to_string(),
                    None => return ControlFlow::Error("AllocElement: invalid tag".into()),
                };
                let tag_id = heap.alloc_string(tag_str);

                let num_props = prop_keys.len();
                let mut map = IndexMap::new();
                for (i, key_cid) in prop_keys.iter().enumerate() {
                    if let Some(key) = program.get_string_constant(*key_cid) {
                        let val = inputs.get(i).copied().unwrap_or(Value::Nil);
                        map.insert(key.to_string(), val);
                    }
                }
                let props_id = heap.alloc_map(map);

                let children_id = heap.alloc_list(inputs[num_props..].to_vec());

                let elem_id = heap.alloc_element(tag_id, props_id, children_id);
                Self::write_register(stack, term, Value::Element(elem_id));
                ControlFlow::Advance
            }

            TermOp::GetField(field_cid) => {
                let obj = inputs[0];
                let field_name = match program.get_string_constant(*field_cid) {
                    Some(s) => s,
                    None => return ControlFlow::Error("GetField: invalid field name".into()),
                };
                match obj {
                    Value::Map(map_id) => {
                        let map = heap.get_map(map_id);
                        let val = map
                            .get(field_name)
                            .copied()
                            .ok_or_else(|| format!("No field '{}' on record", field_name));
                        match val {
                            Ok(v) => {
                                Self::write_register(stack, term, v);
                                ControlFlow::Advance
                            }
                            Err(e) => ControlFlow::Error(e),
                        }
                    }
                    Value::Element(elem_id) => {
                        let val = match field_name {
                            "tag" => {
                                let tag_id = heap.get_element_tag(elem_id);
                                Value::String(tag_id)
                            }
                            "props" => Value::Map(heap.get_element_props(elem_id)),
                            "children" => Value::List(heap.get_element_children(elem_id)),
                            _ => {
                                return ControlFlow::Error(format!(
                                    "No field '{}' on element",
                                    field_name
                                ))
                            }
                        };
                        Self::write_register(stack, term, val);
                        ControlFlow::Advance
                    }
                    Value::List(list_id) if field_name == "length" => {
                        let len = heap.list_len(list_id) as i64;
                        Self::write_register(stack, term, Value::Int(len));
                        ControlFlow::Advance
                    }
                    Value::String(str_id) if field_name == "length" => {
                        let len = heap.get_string(str_id).len() as i64;
                        Self::write_register(stack, term, Value::Int(len));
                        ControlFlow::Advance
                    }
                    Value::Vec2(x, y) => {
                        let val = match field_name {
                            "x" => Value::Float(x),
                            "y" => Value::Float(y),
                            _ => return ControlFlow::Error(format!(
                                "No field '{}' on vec2 (available: x, y)", field_name
                            )),
                        };
                        Self::write_register(stack, term, val);
                        ControlFlow::Advance
                    }
                    _ => ControlFlow::Error(format!(
                        "Cannot access field '{}' on {}",
                        field_name,
                        obj.type_name()
                    )),
                }
            }

            TermOp::SetField(field_cid) => {
                let obj = inputs[0];
                let val = inputs[1];
                match obj {
                    Value::Map(map_id) => {
                        let field_name = match program.get_string_constant(*field_cid) {
                            Some(s) => s.to_string(),
                            None => return ControlFlow::Error("SetField: invalid field name".into()),
                        };
                        heap.get_map_mut(map_id).insert(field_name, val);
                        Self::write_register(stack, term, Value::Nil);
                        ControlFlow::Advance
                    }
                    _ => ControlFlow::Error(format!(
                        "Cannot set field on {}",
                        obj.type_name()
                    )),
                }
            }

            TermOp::GetIndex => {
                let obj = inputs[0];
                let idx = inputs[1];
                match (obj, idx) {
                    (Value::List(list_id), Value::Int(i)) => {
                        let list = heap.get_list(list_id);
                        let index = if i < 0 {
                            (list.len() as i64 + i) as usize
                        } else {
                            i as usize
                        };
                        match list.get(index) {
                            Some(&v) => {
                                Self::write_register(stack, term, v);
                                ControlFlow::Advance
                            }
                            None => ControlFlow::Error(format!(
                                "Index {} out of bounds (len {})",
                                i,
                                list.len()
                            )),
                        }
                    }
                    (Value::Map(map_id), Value::String(key_id)) => {
                        let key = heap.get_string(key_id).to_string();
                        let map = heap.get_map(map_id);
                        match map.get(&key) {
                            Some(&v) => {
                                Self::write_register(stack, term, v);
                                ControlFlow::Advance
                            }
                            None => ControlFlow::Error(format!("No key '{}' on record", key)),
                        }
                    }
                    _ => ControlFlow::Error(format!(
                        "Cannot index {} with {}",
                        obj.type_name(),
                        idx.type_name()
                    )),
                }
            }

            TermOp::SetIndex => {
                let obj = inputs[0];
                let idx = inputs[1];
                let val = inputs[2];
                match (obj, idx) {
                    (Value::List(list_id), Value::Int(i)) => {
                        let list = heap.get_list_mut(list_id);
                        let index = i as usize;
                        if index < list.len() {
                            list[index] = val;
                            Self::write_register(stack, term, Value::Nil);
                            ControlFlow::Advance
                        } else {
                            ControlFlow::Error(format!(
                                "Index {} out of bounds (len {})",
                                i,
                                list.len()
                            ))
                        }
                    }
                    _ => ControlFlow::Error(format!(
                        "Cannot index-assign {} with {}",
                        obj.type_name(),
                        idx.type_name()
                    )),
                }
            }

            TermOp::MakeEnumVariant(name_cid) => {
                let name_str = match program.get_string_constant(*name_cid) {
                    Some(s) => s.to_string(),
                    None => return ControlFlow::Error("MakeEnumVariant: invalid name".into()),
                };
                let tag = heap.alloc_string(name_str);
                let data = heap.alloc_list(inputs.to_vec());
                Self::write_register(stack, term, Value::EnumVariant { tag, data });
                ControlFlow::Advance
            }

            TermOp::Match => {
                Self::exec_match(term, inputs, program, stack, heap, closures, overload_sets, native_fns, output, trace)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Extracted term handlers
    // -----------------------------------------------------------------------

    fn exec_for_loop(
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
    ) -> ControlFlow {
        // Handle break from a completed iteration.
        if stack.break_flag {
            stack.break_flag = false;
            if let Some(frame) = stack.frames.last_mut() {
                frame.remove_loop_state(&term.id);
            }
            Self::write_register(stack, term, Value::Nil);
            return ControlFlow::Advance;
        }

        // Handle continue — just clear the flag and proceed to next iteration.
        if stack.continue_flag {
            stack.continue_flag = false;
        }

        let body_block = term.child_blocks[0];

        // Initialize loop state on the first visit.
        let needs_init = stack.frames.last()
            .map(|f| !f.has_loop_state(&term.id))
            .unwrap_or(false);
        if needs_init {
            match inputs[0] {
                Value::List(list_id) => {
                    let elements = heap.get_list(list_id).to_vec();
                    if let Some(frame) = stack.frames.last_mut() {
                        frame.set_loop_state(
                            term.id,
                            LoopState::For { elements, index: 0 },
                        );
                    }
                }
                other => {
                    return ControlFlow::Error(format!(
                        "Cannot iterate over {}",
                        other.type_name()
                    ))
                }
            }
        }

        // Get the next element (or detect loop completion).
        let maybe_elem: Option<Value> = {
            let frame = stack.frames.last_mut().unwrap();
            match frame.get_loop_state_mut(&term.id) {
                Some(LoopState::For { elements, index }) => {
                    if *index < elements.len() {
                        let elem = elements[*index];
                        *index += 1;
                        Some(elem)
                    } else {
                        frame.remove_loop_state(&term.id);
                        None
                    }
                }
                _ => None,
            }
        };

        match maybe_elem {
            Some(elem) => {
                // Push body frame for this iteration.
                let block = program.get_block(body_block);
                let parent_frame_idx = stack.frames.len() - 1;
                stack.push_frame(
                    Frame::new(body_block, block.entry, block.register_count as usize,
                        Some(term.id), Some(parent_frame_idx))
                    .as_loop_body()
                );
                // Set the loop variable in the first register.
                if let Some(frame) = stack.frames.last_mut() {
                    if !frame.registers.is_empty() {
                        frame.registers[0] = elem;
                    }
                }
                ControlFlow::FramePushed
            }
            None => {
                // All iterations complete.
                Self::write_register(stack, term, Value::Nil);
                ControlFlow::Advance
            }
        }
    }

    fn exec_while_loop(
        term: &Term,
        program: &Program,
        stack: &mut Stack,
    ) -> ControlFlow {
        // Handle break from the body.
        if stack.break_flag {
            stack.break_flag = false;
            if let Some(frame) = stack.frames.last_mut() {
                frame.remove_loop_state(&term.id);
            }
            Self::write_register(stack, term, Value::Nil);
            return ControlFlow::Advance;
        }

        // Handle continue — clear the flag and re-evaluate condition.
        if stack.continue_flag {
            stack.continue_flag = false;
        }

        let cond_block = term.child_blocks[0];
        let body_block = term.child_blocks[1];

        // Determine current loop phase from loop_states.
        let loop_phase = stack.frames.last()
            .and_then(|f| f.get_loop_state(&term.id))
            .map(|ls| match ls {
                LoopState::WhileCondition { iteration } => (true, *iteration),
                LoopState::WhileBody { iteration } => (false, *iteration),
                _ => (false, 0),
            });

        match loop_phase {
            Some((true, iteration)) => {
                // Condition block just returned; check its result.
                let cond_val = Self::read_register(program, stack, term.id);

                if !cond_val.is_truthy() {
                    // Condition false — loop done.
                    if let Some(frame) = stack.frames.last_mut() {
                        frame.remove_loop_state(&term.id);
                    }
                    Self::write_register(stack, term, Value::Nil);
                    return ControlFlow::Advance;
                }

                // Transition to WhileBody so resolve_loop_context sees the iteration.
                if let Some(frame) = stack.frames.last_mut() {
                    frame.set_loop_state(term.id, LoopState::WhileBody { iteration });
                }

                // Push body frame.
                let block = program.get_block(body_block);
                let parent_frame_idx = stack.frames.len() - 1;
                stack.push_frame(
                    Frame::new(body_block, block.entry, block.register_count as usize,
                        Some(term.id), Some(parent_frame_idx))
                    .as_loop_body()
                );
                ControlFlow::FramePushed
            }
            Some((false, iteration)) => {
                // Body just returned — push condition block for next iteration.
                let next_iteration = iteration + 1;
                let block = program.get_block(cond_block);
                let parent_frame_idx = stack.frames.len() - 1;
                stack.push_frame(Frame::new(
                    cond_block, block.entry, block.register_count as usize,
                    Some(term.id), Some(parent_frame_idx),
                ));
                if let Some(frame) = stack.frames.get_mut(parent_frame_idx) {
                    frame.set_loop_state(term.id, LoopState::WhileCondition { iteration: next_iteration });
                }
                ControlFlow::FramePushed
            }
            None => {
                // Fresh start — push condition block, iteration 0.
                let block = program.get_block(cond_block);
                let parent_frame_idx = stack.frames.len() - 1;
                stack.push_frame(Frame::new(
                    cond_block, block.entry, block.register_count as usize,
                    Some(term.id), Some(parent_frame_idx),
                ));
                if let Some(frame) = stack.frames.get_mut(parent_frame_idx) {
                    frame.set_loop_state(term.id, LoopState::WhileCondition { iteration: 0 });
                }
                ControlFlow::FramePushed
            }
        }
    }

    /// Resolve a callable to a ClosureId. Handles both plain closures and overload sets.
    fn resolve_callable(
        callable: Value,
        arg_count: usize,
        overload_sets: &[Vec<OverloadEntry>],
        closures: &[RuntimeClosure],
        program: &Program,
    ) -> Result<ClosureId, String> {
        match callable {
            Value::Closure(id) => Ok(id),
            Value::OverloadSet(set_id) => {
                Self::resolve_overload(&overload_sets[set_id.0 as usize], arg_count, closures, program)
            }
            _ => Err(format!("Expected a function, got {}", callable.type_name())),
        }
    }

    /// Build a closure frame and push it onto the stack, advancing the caller.
    fn push_closure_call(
        callable: Value,
        args: &[Value],
        term: &Term,
        program: &Program,
        stack: &mut Stack,
        closures: &[RuntimeClosure],
        overload_sets: &[Vec<OverloadEntry>],
    ) -> ControlFlow {
        let closure_id = match Self::resolve_callable(callable, args.len(), overload_sets, closures, program) {
            Ok(id) => id,
            Err(e) => return ControlFlow::Error(e),
        };
        match Self::build_closure_frame(Value::Closure(closure_id), args, program, closures, Some(term.id)) {
            Ok(frame) => {
                if let Some(caller_frame) = stack.frames.last_mut() {
                    caller_frame.current_term = term.block_next;
                }
                stack.push_frame(frame);
                ControlFlow::FramePushed
            }
            Err(e) => ControlFlow::Error(e),
        }
    }

    fn exec_call(
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> ControlFlow {
        let callable = inputs[0];
        let args = &inputs[1..];

        match callable {
            Value::Closure(_) | Value::OverloadSet(_) => {
                Self::push_closure_call(callable, args, term, program, stack, closures, overload_sets)
            }

            Value::NativeFunction(native_id) => {
                Self::call_native_or_intrinsic(
                    native_id, args, term, program, stack, heap, closures, overload_sets, native_fns, output, trace,
                )
            }

            Value::EnumVariant { .. } if args.is_empty() => {
                Self::write_register(stack, term, callable);
                ControlFlow::Advance
            }

            _ => ControlFlow::Error(format!("Cannot call {}", callable.type_name())),
        }
    }

    /// Resolve an overload set to the correct closure based on argument count.
    fn resolve_overload(
        entries: &[OverloadEntry],
        arg_count: usize,
        closures: &[RuntimeClosure],
        program: &Program,
    ) -> Result<ClosureId, String> {
        for entry in entries {
            if entry.arity == arg_count {
                return Ok(entry.closure_id);
            }
        }
        // Derive the base function name from the first entry's internal name (e.g. "foo#2" → "foo")
        let base_name = entries.first()
            .and_then(|e| {
                let func = &program.functions[closures[e.closure_id.0 as usize].function_id.0 as usize];
                func.name.as_ref().and_then(|n| n.split('#').next().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| "<anonymous>".to_string());
        let arities: Vec<String> = entries.iter().map(|e| e.arity.to_string()).collect();
        Err(format!(
            "{}() expects {} arguments, got {}",
            base_name,
            arities.join(" or "),
            arg_count,
        ))
    }

    fn exec_method_call(
        method_cid: ConstantId,
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> ControlFlow {
        let obj = inputs[0];
        let args = &inputs[1..];
        let method_name = match program.get_string_constant(method_cid) {
            Some(s) => s.to_string(),
            None => return ControlFlow::Error("Invalid method name".into()),
        };

        // 1) If obj is a map, check for a callable field first
        if let Value::Map(map_id) = obj {
            let map = heap.get_map(map_id);
            if let Some(&field_val) = map.get(&method_name) {
                match field_val {
                    Value::Closure(_) | Value::OverloadSet(_) => {
                        return Self::push_closure_call(
                            field_val, args, term, program, stack, closures, overload_sets,
                        );
                    }
                    Value::NativeFunction(native_id) => {
                        match Self::call_native_fn(native_id, args, native_fns, heap, output) {
                            Ok(val) => {
                                Self::write_register(stack, term, val);
                                return ControlFlow::Advance;
                            }
                            Err(e) => return ControlFlow::Error(e),
                        }
                    }
                    _ => {} // not callable, fall through to method lookup
                }
            }
        }

        // 2) Look up method as a native function, calling with obj prepended to args
        if let Some(native_id) = native_fns.lookup_name(&method_name) {
            let mut full_args = vec![obj];
            full_args.extend_from_slice(args);
            Self::call_native_or_intrinsic(
                native_id, &full_args, term, program, stack, heap, closures, overload_sets, native_fns, output, trace,
            )
        } else {
            let hint = match method_name.as_str() {
                "toString" => Some("use str() or the str() method instead"),
                "log" => Some("use print() instead of console.log()"),
                "indexOf" => Some("use contains() to check membership"),
                "concat" => Some("use the ++ operator to concatenate lists or strings"),
                _ => None,
            };
            let msg = if let Some(hint) = hint {
                format!("No method '{}' on type {} — {}", method_name, obj.type_name(), hint)
            } else {
                format!("No method '{}' on type {}", method_name, obj.type_name())
            };
            ControlFlow::Error(msg)
        }
    }

    fn exec_match(
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> ControlFlow {
        let subject = inputs[0];
        let arm_metas = match program.match_arms.get(&term.id) {
            Some(arms) => arms,
            None => return ControlFlow::Error("Match: no arm metadata".into()),
        };

        for arm_meta in arm_metas {
            // Try to match pattern
            let mut bindings = Vec::new();
            if Self::match_pattern(&arm_meta.pattern, subject, heap, &mut bindings) {
                // Check guard if present (with pattern bindings available)
                if let Some(guard_block) = arm_meta.guard_block {
                    // Push guard frame with pattern bindings
                    let gb = program.get_block(guard_block);
                    let gb_reg_count = gb.register_count as usize;
                    let parent_idx = stack.frames.len() - 1;
                    stack.push_frame(Frame::new(
                        guard_block, gb.entry, gb_reg_count,
                        Some(term.id), Some(parent_idx),
                    ));
                    if let Some(frame) = stack.frames.last_mut() {
                        Self::apply_pattern_bindings(program, guard_block, &bindings, frame);
                    }
                    // Run guard to completion
                    let target_depth = parent_idx + 1;
                    let mut guard_result = Value::Bool(false);
                    loop {
                        if stack.frames.len() <= target_depth {
                            if let Some(frame) = stack.frames.last() {
                                let reg = term.register.0 as usize;
                                if reg < frame.registers.len() {
                                    guard_result = frame.registers[reg];
                                }
                            }
                            break;
                        }
                        match Self::step(program, stack, heap, closures, overload_sets, native_fns, output, trace) {
                            StepResult::Continue => {}
                            StepResult::Complete(v) => { guard_result = v; break; }
                            StepResult::Error(e) => return ControlFlow::Error(e),
                        }
                    }
                    if !guard_result.is_truthy() {
                        continue;
                    }
                }

                // Advance parent frame past the Match term
                if let Some(parent_frame) = stack.frames.last_mut() {
                    parent_frame.current_term = term.block_next;
                }

                // Execute body block with bindings
                let body_block_id = arm_meta.body_block;
                let block = program.get_block(body_block_id);
                let reg_count = block.register_count as usize;
                let parent_frame_idx = stack.frames.len() - 1;

                stack.push_frame(Frame::new(
                    body_block_id, block.entry, reg_count,
                    Some(term.id), Some(parent_frame_idx),
                ));

                // Apply pattern bindings to the body frame's registers
                if let Some(frame) = stack.frames.last_mut() {
                    Self::apply_pattern_bindings(program, body_block_id, &bindings, frame);
                }

                return ControlFlow::FramePushed;
            }
        }

        ControlFlow::Error(format!(
            "No matching pattern for value: {}",
            value::value_to_display_string(&subject, heap)
        ))
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn constant_to_value(cid: ConstantId, program: &Program, heap: &mut Heap) -> Value {
        match program.constants.get(cid) {
            ConstantValue::Nil => Value::Nil,
            ConstantValue::Bool(b) => Value::Bool(*b),
            ConstantValue::Int(n) => Value::Int(*n),
            ConstantValue::Float(bits) => Value::Float(f64::from_bits(*bits)),
            ConstantValue::String(s) => {
                let sid = heap.alloc_string(s.clone());
                Value::String(sid)
            }
        }
    }

    fn numeric_binop(
        term: &Term,
        inputs: &[Value],
        stack: &mut Stack,
        int_op: impl Fn(i64, i64) -> Value,
        float_op: impl Fn(f64, f64) -> Value,
    ) -> ControlFlow {
        if inputs.len() < 2 {
            return ControlFlow::Error("Binary op: missing inputs".into());
        }
        let val = match (&inputs[0], &inputs[1]) {
            // Dual number arithmetic (forward-mode AD)
            (Value::Dual { .. }, _) | (_, Value::Dual { .. }) => {
                return Self::dual_binop(term, inputs, stack);
            }
            (Value::Int(a), Value::Int(b)) => int_op(*a, *b),
            (Value::Float(a), Value::Float(b)) => float_op(*a, *b),
            (Value::Int(a), Value::Float(b)) => float_op(*a as f64, *b),
            (Value::Float(a), Value::Int(b)) => float_op(*a, *b as f64),
            // Vec2 arithmetic
            (Value::Vec2(..), _) | (_, Value::Vec2(..)) => {
                return Self::vec2_binop(term, inputs, stack);
            }
            _ => {
                return ControlFlow::Error(format!(
                    "Cannot perform arithmetic on {} and {}",
                    inputs[0].type_name(),
                    inputs[1].type_name()
                ))
            }
        };
        Self::write_register(stack, term, val);
        ControlFlow::Advance
    }

    /// Forward-mode AD arithmetic for Dual numbers.
    fn dual_binop(
        term: &Term,
        inputs: &[Value],
        stack: &mut Stack,
    ) -> ControlFlow {
        let a_val = match inputs[0].as_f64() {
            Some(v) => v,
            None => return ControlFlow::Error(format!(
                "Cannot perform arithmetic on {} and {}",
                inputs[0].type_name(), inputs[1].type_name()
            )),
        };
        let b_val = match inputs[1].as_f64() {
            Some(v) => v,
            None => return ControlFlow::Error(format!(
                "Cannot perform arithmetic on {} and {}",
                inputs[0].type_name(), inputs[1].type_name()
            )),
        };
        let a_deriv = inputs[0].derivative();
        let b_deriv = inputs[1].derivative();

        let (value, derivative) = match &term.op {
            TermOp::Add => (a_val + b_val, a_deriv + b_deriv),
            TermOp::Sub => (a_val - b_val, a_deriv - b_deriv),
            TermOp::Mul => (a_val * b_val, a_deriv * b_val + a_val * b_deriv),
            TermOp::Div => {
                if b_val == 0.0 {
                    return ControlFlow::Error("Division by zero".into());
                }
                (a_val / b_val, (a_deriv * b_val - a_val * b_deriv) / (b_val * b_val))
            }
            TermOp::Mod => {
                // Mod derivative: d(a%b)/da = 1, d(a%b)/db is complex;
                // approximate: treat as a - floor(a/b)*b
                (a_val % b_val, a_deriv)
            }
            _ => return ControlFlow::Error("Unsupported dual operation".into()),
        };

        Self::write_register(stack, term, Value::Dual { value, derivative });
        ControlFlow::Advance
    }

    /// Vec2 arithmetic: component-wise add/sub, scalar multiply/divide, component-wise mul.
    fn vec2_binop(
        term: &Term,
        inputs: &[Value],
        stack: &mut Stack,
    ) -> ControlFlow {
        let val = match (&inputs[0], &inputs[1]) {
            // vec2 op vec2
            (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => match &term.op {
                TermOp::Add => Value::Vec2(ax + bx, ay + by),
                TermOp::Sub => Value::Vec2(ax - bx, ay - by),
                TermOp::Mul => Value::Vec2(ax * bx, ay * by),
                TermOp::Div => {
                    if *bx == 0.0 || *by == 0.0 {
                        return ControlFlow::Error("Division by zero in vec2".into());
                    }
                    Value::Vec2(ax / bx, ay / by)
                }
                _ => return ControlFlow::Error("Unsupported vec2 operation".into()),
            },
            // vec2 op scalar
            (Value::Vec2(x, y), other) => {
                let s = match other.as_f64() {
                    Some(v) => v,
                    None => return ControlFlow::Error(format!(
                        "Cannot perform arithmetic on vec2 and {}", other.type_name()
                    )),
                };
                match &term.op {
                    TermOp::Mul => Value::Vec2(x * s, y * s),
                    TermOp::Div => {
                        if s == 0.0 {
                            return ControlFlow::Error("Division by zero".into());
                        }
                        Value::Vec2(x / s, y / s)
                    }
                    TermOp::Add => Value::Vec2(x + s, y + s),
                    TermOp::Sub => Value::Vec2(x - s, y - s),
                    _ => return ControlFlow::Error("Unsupported vec2 operation".into()),
                }
            }
            // scalar op vec2
            (other, Value::Vec2(x, y)) => {
                let s = match other.as_f64() {
                    Some(v) => v,
                    None => return ControlFlow::Error(format!(
                        "Cannot perform arithmetic on {} and vec2", other.type_name()
                    )),
                };
                match &term.op {
                    TermOp::Mul => Value::Vec2(s * x, s * y),
                    TermOp::Add => Value::Vec2(s + x, s + y),
                    TermOp::Sub => Value::Vec2(s - x, s - y),
                    _ => return ControlFlow::Error("Unsupported vec2 operation".into()),
                }
            }
            _ => return ControlFlow::Error("Unsupported vec2 operation".into()),
        };
        Self::write_register(stack, term, val);
        ControlFlow::Advance
    }

    fn comparison_op(
        term: &Term,
        inputs: &[Value],
        stack: &mut Stack,
        heap: &Heap,
        pred: impl Fn(std::cmp::Ordering) -> bool,
    ) -> ControlFlow {
        match builtins::compare_values(&inputs[0], &inputs[1], heap) {
            Ok(ord) => {
                Self::write_register(stack, term, Value::Bool(pred(ord)));
                ControlFlow::Advance
            }
            Err(e) => ControlFlow::Error(e),
        }
    }

    fn push_child_frame(
        program: &Program,
        stack: &mut Stack,
        block_id: BlockId,
        parent_term: &Term,
    ) {
        let block = program.get_block(block_id);
        let reg_count = block.register_count as usize;
        let parent_frame_idx = stack.frames.len() - 1;

        // Advance parent frame past the control flow term
        if let Some(parent_frame) = stack.frames.last_mut() {
            parent_frame.current_term = parent_term.block_next;
        }

        stack.push_frame(Frame::new(
            block_id, block.entry, reg_count,
            Some(parent_term.id), Some(parent_frame_idx),
        ));
    }

    // -----------------------------------------------------------------------
    // Native function dispatch
    // -----------------------------------------------------------------------

    /// Call a native function (non-intrinsic) via PetalCxt, returning the result value.
    fn call_native_fn(
        native_id: crate::native_fn::NativeFnId,
        args: &[Value],
        native_fns: &NativeFnTable,
        heap: &mut Heap,
        output: &mut Vec<String>,
    ) -> Result<Value, String> {
        let func = native_fns.get_func(native_id);
        let mut state = PetalCxt::new(args, heap, output);
        let count = func(&mut state)?;
        let results = state.take_results();
        let val = if count > 0 && !results.is_empty() {
            results[0]
        } else {
            Value::Nil
        };
        Ok(val)
    }

    /// Dispatch a native function call, handling higher-order intrinsics (map, filter, reduce)
    /// specially since they need evaluator context to call closures.
    fn call_native_or_intrinsic(
        native_id: crate::native_fn::NativeFnId,
        args: &[Value],
        term: &Term,
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> ControlFlow {
        let result = if native_fns.intrinsic_map == Some(native_id) {
            Self::builtin_map(args, program, stack, heap, closures, overload_sets, native_fns, output, trace)
        } else if native_fns.intrinsic_filter == Some(native_id) {
            Self::builtin_filter(args, program, stack, heap, closures, overload_sets, native_fns, output, trace)
        } else if native_fns.intrinsic_reduce == Some(native_id) {
            Self::builtin_reduce(args, program, stack, heap, closures, overload_sets, native_fns, output, trace)
        } else if native_fns.intrinsic_for_each == Some(native_id) {
            Self::builtin_for_each(args, program, stack, heap, closures, overload_sets, native_fns, output, trace)
        } else {
            Self::call_native_fn(native_id, args, native_fns, heap, output)
        };

        match result {
            Ok(val) => {
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }
            Err(e) => ControlFlow::Error(e),
        }
    }

    // -----------------------------------------------------------------------
    // Closure call helpers
    // -----------------------------------------------------------------------

    /// Build a Frame for calling a closure with the given arguments.
    /// Handles parameter binding, capture registers, and self-reference.
    fn build_closure_frame(
        callable: Value,
        args: &[Value],
        program: &Program,
        closures: &[RuntimeClosure],
        return_term: Option<TermId>,
    ) -> Result<Frame, String> {
        let closure_id = match callable {
            Value::Closure(id) => id,
            _ => return Err(format!("Expected a function, got {}", callable.type_name())),
        };

        let closure = &closures[closure_id.0 as usize];
        let func = &program.functions[closure.function_id.0 as usize];
        let body_block = func.body_block;
        let block = program.get_block(body_block);

        if args.len() != func.params.len() {
            let name = func.name.as_deref().unwrap_or("<anonymous>");
            return Err(format!(
                "{}() expected {} argument{}, got {}",
                name,
                func.params.len(),
                if func.params.len() == 1 { "" } else { "s" },
                args.len()
            ));
        }

        let reg_count = block.register_count as usize;
        let mut registers = vec![Value::Nil; reg_count];

        // Set parameter registers
        for (i, arg) in args.iter().enumerate() {
            if i < registers.len() {
                registers[i] = *arg;
            }
        }

        // Set capture registers
        for (i, cap) in closure.captures.iter().enumerate() {
            if i < func.capture_registers.len() {
                let reg_idx = func.capture_registers[i].0 as usize;
                if reg_idx < registers.len() {
                    registers[reg_idx] = *cap;
                }
            }
        }

        // Self-reference for recursion
        if let Some(self_reg) = func.self_ref_register {
            let reg_idx = self_reg.0 as usize;
            if reg_idx < registers.len() {
                registers[reg_idx] = callable;
            }
        }

        let mut frame = Frame::new(
            body_block, block.entry, 0, return_term, None,
        );
        frame.registers = registers;
        // Strip internal "#arity" suffix from overloaded function names for display
        frame.fn_name = func.name.as_ref().map(|n| {
            if let Some(pos) = n.rfind('#') {
                n[..pos].to_string()
            } else {
                n.clone()
            }
        });
        Ok(frame)
    }

    // -----------------------------------------------------------------------
    // Higher-order builtin helpers
    // -----------------------------------------------------------------------

    /// Call a closure synchronously with the given arguments, returning the result.
    fn call_closure_sync(
        callable: Value,
        call_args: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> Result<Value, String> {
        let frame = Self::build_closure_frame(callable, call_args, program, closures, None)?;
        let target_depth = stack.frames.len();
        stack.push_frame(frame);

        stack.last_pop_result = None;

        loop {
            if stack.frames.len() <= target_depth {
                // Frame was popped — retrieve the result
                return Ok(stack.last_pop_result.take().unwrap_or(Value::Nil));
            }

            let step = Self::step(program, stack, heap, closures, overload_sets, native_fns, output, trace);
            match step {
                StepResult::Continue => {}
                StepResult::Complete(v) => return Ok(v),
                StepResult::Error(e) => return Err(e),
            }
        }
    }

    fn builtin_map(
        args: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> Result<Value, String> {
        if args.len() != 2 {
            return Err("map() expects 2 arguments (list, function)".into());
        }
        let list_id = match args[0] {
            Value::List(id) => id,
            _ => return Err("map() expects a list as first argument".into()),
        };
        let func = args[1];
        let elements = heap.get_list(list_id).to_vec();

        let mut results = Vec::with_capacity(elements.len());
        for elem in elements {
            let result = Self::call_closure_sync(
                func, &[elem], program, stack, heap, closures, overload_sets, native_fns, output, trace,
            )?;
            results.push(result);
        }

        let result_id = heap.alloc_list(results);
        Ok(Value::List(result_id))
    }

    fn builtin_filter(
        args: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> Result<Value, String> {
        if args.len() != 2 {
            return Err("filter() expects 2 arguments (list, function)".into());
        }
        let list_id = match args[0] {
            Value::List(id) => id,
            _ => return Err("filter() expects a list as first argument".into()),
        };
        let func = args[1];
        let elements = heap.get_list(list_id).to_vec();

        let mut results = Vec::new();
        for elem in elements {
            let keep = Self::call_closure_sync(
                func, &[elem], program, stack, heap, closures, overload_sets, native_fns, output, trace,
            )?;
            if keep.is_truthy() {
                results.push(elem);
            }
        }

        let result_id = heap.alloc_list(results);
        Ok(Value::List(result_id))
    }

    fn builtin_reduce(
        args: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> Result<Value, String> {
        if args.len() != 3 {
            return Err("reduce() expects 3 arguments (list, initial, function)".into());
        }
        let list_id = match args[0] {
            Value::List(id) => id,
            _ => return Err("reduce() expects a list as first argument".into()),
        };
        let func = args[2];
        let elements = heap.get_list(list_id).to_vec();
        let mut acc = args[1];

        for elem in elements {
            acc = Self::call_closure_sync(
                func, &[acc, elem], program, stack, heap, closures, overload_sets, native_fns, output, trace,
            )?;
        }

        Ok(acc)
    }

    fn builtin_for_each(
        args: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        overload_sets: &mut Vec<Vec<OverloadEntry>>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
        trace: &mut TraceBuffer,
    ) -> Result<Value, String> {
        if args.len() != 2 {
            return Err("forEach() expects 2 arguments (list, function)".into());
        }
        let list_id = match args[0] {
            Value::List(id) => id,
            _ => return Err("forEach() expects a list as first argument".into()),
        };
        let func = args[1];
        let elements = heap.get_list(list_id).to_vec();

        for elem in elements {
            Self::call_closure_sync(
                func, &[elem], program, stack, heap, closures, overload_sets, native_fns, output, trace,
            )?;
        }

        Ok(Value::Nil)
    }

    // -----------------------------------------------------------------------
    // Pattern matching (runtime)
    // -----------------------------------------------------------------------

    fn match_pattern(
        pattern: &Pattern,
        value: Value,
        heap: &mut Heap,
        bindings: &mut Vec<(String, Value)>,
    ) -> bool {
        match pattern {
            Pattern::Wildcard => true,

            Pattern::Literal(lit) => {
                match (lit, value) {
                    (Literal::Nil, Value::Nil) => true,
                    (Literal::Bool(a), Value::Bool(b)) => *a == b,
                    (Literal::Int(a), Value::Int(b)) => *a == b,
                    (Literal::Float(a), Value::Float(b)) => *a == b,
                    (Literal::String(a), Value::String(sid)) => a == heap.get_string(sid),
                    _ => false,
                }
            }

            Pattern::Variable(name) => {
                // Pure variable binding — always matches and captures the value.
                // (Known enum variant names are resolved to Pattern::Variant by the compiler.)
                bindings.push((name.clone(), value));
                true
            }

            Pattern::Variant { name, fields } => {
                if let Value::EnumVariant { tag, data } = value {
                    let variant_name = heap.get_string(tag);
                    if variant_name != name {
                        return false;
                    }
                    let data_fields = heap.get_list(data);
                    if data_fields.len() != fields.len() {
                        return false;
                    }
                    let data_copy: Vec<Value> = data_fields.to_vec();
                    for (pat, val) in fields.iter().zip(data_copy.iter()) {
                        if !Self::match_pattern(pat, *val, heap, bindings) {
                            return false;
                        }
                    }
                    true
                } else {
                    false
                }
            }

            Pattern::List { elements, rest } => {
                if let Value::List(list_id) = value {
                    let list = heap.get_list(list_id);
                    if let Some(rest_name) = rest {
                        if list.len() < elements.len() {
                            return false;
                        }
                        let list_copy: Vec<Value> = list.to_vec();
                        for (pat, val) in elements.iter().zip(list_copy.iter()) {
                            if !Self::match_pattern(pat, *val, heap, bindings) {
                                return false;
                            }
                        }
                        let rest_vals: Vec<Value> = list_copy[elements.len()..].to_vec();
                        let rest_list = Value::List(heap.alloc_list(rest_vals));
                        bindings.push((rest_name.clone(), rest_list));
                        true
                    } else {
                        if list.len() != elements.len() {
                            return false;
                        }
                        let list_copy: Vec<Value> = list.to_vec();
                        for (pat, val) in elements.iter().zip(list_copy.iter()) {
                            if !Self::match_pattern(pat, *val, heap, bindings) {
                                return false;
                            }
                        }
                        true
                    }
                } else {
                    false
                }
            }

            Pattern::Record(fields) => {
                if let Value::Map(map_id) = value {
                    // Copy relevant entries out before recursive matching
                    let entries: Vec<(String, Value)> = {
                        let map = heap.get_map(map_id);
                        fields
                            .iter()
                            .filter_map(|(key, _)| {
                                map.get(key).map(|&val| (key.clone(), val))
                            })
                            .collect()
                    };
                    if entries.len() != fields.len() {
                        return false; // Some fields missing
                    }
                    for ((_, pat), (_, val)) in fields.iter().zip(entries.iter()) {
                        if !Self::match_pattern(pat, *val, heap, bindings) {
                            return false;
                        }
                    }
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Apply pattern bindings to a frame's registers by matching names to
    /// terms in the block (including phantom terms not in the linked list).
    /// Uses the precomputed block_terms index for O(B) lookup instead of O(N)
    /// where B is the number of terms in the block and N is total program terms.
    fn apply_pattern_bindings(
        program: &Program,
        block_id: BlockId,
        bindings: &[(String, Value)],
        frame: &mut Frame,
    ) {
        if let Some(term_ids) = program.block_terms.get(&block_id) {
            for tid in term_ids {
                let term = program.get_term(*tid);
                if let Some(ref term_name) = term.name {
                    for (bind_name, bind_val) in bindings {
                        if term_name == bind_name {
                            let reg = term.register.0 as usize;
                            if reg < frame.registers.len() {
                                frame.registers[reg] = *bind_val;
                            }
                        }
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Match pattern needs mutable heap for rest-pattern list allocation
    // -----------------------------------------------------------------------
}

/// Runtime closure — captures + function reference.
pub struct RuntimeClosure {
    pub function_id: FunctionId,
    pub captures: Vec<Value>,
}
