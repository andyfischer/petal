//! Per-term dispatch and data-structure operations.

use indexmap::IndexMap;

use super::*;
use crate::constant_table::{ConstantId, ConstantValue};

impl<'a> Evaluator<'a> {
    /// Execute a single term. Most ops compute a value and finish via
    /// `produce` (write result, advance); control-flow ops push frames or
    /// signal break/continue/return instead.
    pub(super) fn exec_term(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        match &term.op {
            TermOp::Constant(cid) => {
                let val = self.constant_to_value(*cid);
                self.produce(term, val)
            }

            TermOp::Error(cid) => {
                let msg = self
                    .program
                    .get_string_constant(*cid)
                    .unwrap_or("Unknown error")
                    .to_string();
                ControlFlow::Error(msg)
            }

            // Identity / variable reference
            TermOp::Copy => self.produce(term, inputs.first().copied().unwrap_or(Value::Nil)),

            // Initialize the phi's register from inputs[0] — the
            // pre-control-flow value of the name being joined. When a
            // child frame later rebinds the name, its pop's phi_outs
            // overwrite this register; branches that don't rebind leave
            // the init value in place.
            TermOp::Phi => self.produce(term, inputs.first().copied().unwrap_or(Value::Nil)),

            TermOp::Add | TermOp::Sub | TermOp::Mul | TermOp::Div | TermOp::Mod => {
                self.numeric_binop(term, inputs)
            }
            TermOp::Neg => self.exec_neg(term, inputs),

            TermOp::Not => match inputs.first() {
                Some(v) => {
                    let val = Value::Bool(!v.is_truthy());
                    self.produce(term, val)
                }
                None => ControlFlow::Error("Not: missing input".into()),
            },

            TermOp::Eq => {
                let val = Value::Bool(value::values_equal(&inputs[0], &inputs[1], self.heap));
                self.produce(term, val)
            }
            TermOp::Ne => {
                let val = Value::Bool(!value::values_equal(&inputs[0], &inputs[1], self.heap));
                self.produce(term, val)
            }
            TermOp::Lt | TermOp::Le | TermOp::Gt | TermOp::Ge => self.comparison_op(term, inputs),

            TermOp::Concat => self.exec_concat(term, inputs),

            // Short-circuit: when the left side decides the answer, produce
            // it; otherwise run the RHS block.
            TermOp::And => {
                if !inputs[0].is_truthy() {
                    self.produce(term, Value::Bool(false))
                } else {
                    self.push_child_frame(term.child_blocks[0], term);
                    ControlFlow::FramePushed
                }
            }
            TermOp::Or => {
                if inputs[0].is_truthy() {
                    self.produce(term, Value::Bool(true))
                } else {
                    self.push_child_frame(term.child_blocks[0], term);
                    ControlFlow::FramePushed
                }
            }

            TermOp::Branch => {
                let block_idx = if inputs[0].is_truthy() { 0 } else { 1 };
                self.push_child_frame(term.child_blocks[block_idx], term);
                ControlFlow::FramePushed
            }

            TermOp::ForLoop => self.exec_for_loop(term, inputs),
            TermOp::NumericForLoop => self.exec_numeric_for_loop(term, inputs),
            TermOp::WhileLoop => self.exec_while_loop(term),

            TermOp::Break => ControlFlow::Break,
            TermOp::Continue => ControlFlow::Continue,
            TermOp::Return => {
                ControlFlow::Return(inputs.first().copied().unwrap_or(Value::Nil))
            }

            TermOp::MakeOverloadSet => self.exec_make_overload_set(term, inputs),
            TermOp::Call => self.exec_call(term, inputs),
            TermOp::MethodCall(method_cid) => self.exec_method_call(*method_cid, term, inputs),
            TermOp::BuiltinCall(name_cid) => self.exec_builtin_call(*name_cid, term, inputs),

            TermOp::MakeClosure(fn_id) => {
                let closure_id = ClosureId(self.closures.len() as u32);
                self.closures.push(RuntimeClosure {
                    function_id: *fn_id,
                    captures: inputs.to_vec(),
                });
                self.produce(term, Value::Closure(closure_id))
            }

            TermOp::StateInit => self.exec_state_init(term, inputs),
            TermOp::StateRead => self.exec_state_read(term),
            TermOp::StateWrite => self.exec_state_write(term, inputs),

            TermOp::AllocList => {
                let list_id = self.heap.alloc_list(inputs.to_vec());
                self.produce(term, Value::List(list_id))
            }
            TermOp::AllocMap { fields } => self.exec_alloc_map(fields, term, inputs),
            TermOp::AllocMapSpread { entries } => {
                self.exec_alloc_map_spread(entries, term, inputs)
            }
            TermOp::AllocElement { tag, prop_keys } => {
                self.exec_alloc_element(*tag, prop_keys, term, inputs)
            }

            TermOp::GetField(field_cid) => self.exec_get_field(*field_cid, term, inputs),
            TermOp::SetField(field_cid) => self.exec_set_field(*field_cid, term, inputs),
            TermOp::GetIndex => self.exec_get_index(term, inputs),
            TermOp::SetIndex => self.exec_set_index(term, inputs),

            TermOp::MakeEnumVariant(name_cid) => {
                let name_str = match self.program.get_string_constant(*name_cid) {
                    Some(s) => s.to_string(),
                    None => return ControlFlow::Error("MakeEnumVariant: invalid name".into()),
                };
                let tag = self.heap.alloc_string(name_str);
                let data = self.heap.alloc_list(inputs.to_vec());
                self.produce(term, Value::EnumVariant { tag, data })
            }

            TermOp::Match => self.exec_match(term, inputs),
        }
    }

    fn constant_to_value(&mut self, cid: ConstantId) -> Value {
        let program = self.program;
        match program.constants.get(cid) {
            ConstantValue::Nil => Value::Nil,
            ConstantValue::Bool(b) => Value::Bool(*b),
            ConstantValue::Int(n) => Value::Int(*n),
            ConstantValue::Float(bits) => Value::Float(f64::from_bits(*bits)),
            ConstantValue::String(s) => Value::String(self.heap.alloc_string(s.clone())),
        }
    }

    /// `++`: list concatenation, or string concatenation with display
    /// conversion for non-string operands.
    fn exec_concat(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        match (inputs[0], inputs[1]) {
            (Value::List(a), Value::List(b)) => {
                let mut combined = self.heap.get_list(a).to_vec();
                combined.extend_from_slice(self.heap.get_list(b));
                let id = self.heap.alloc_list(combined);
                self.produce(term, Value::List(id))
            }
            _ => {
                let l = value::value_to_display_string(&inputs[0], self.heap);
                let r = value::value_to_display_string(&inputs[1], self.heap);
                let sid = self.heap.alloc_string(format!("{}{}", l, r));
                self.produce(term, Value::String(sid))
            }
        }
    }

    fn exec_neg(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        let val = match inputs.first() {
            Some(Value::Int(n)) => Value::Int(-n),
            Some(Value::Float(f)) => Value::Float(-f),
            Some(Value::Dual { value, derivative }) => Value::Dual {
                value: -value,
                derivative: -derivative,
            },
            Some(Value::Vec2(x, y)) => Value::Vec2(-x, -y),
            Some(v) => return ControlFlow::Error(format!("Cannot negate {}", v.type_name())),
            None => return ControlFlow::Error("Neg: missing input".into()),
        };
        self.produce(term, val)
    }

    // -----------------------------------------------------------------------
    // Records, elements, indexing
    // -----------------------------------------------------------------------

    fn exec_alloc_map(
        &mut self,
        fields: &[ConstantId],
        term: &Term,
        inputs: &[Value],
    ) -> ControlFlow {
        let program = self.program;
        let mut map = IndexMap::new();
        for (i, field_cid) in fields.iter().enumerate() {
            if let Some(key) = program.get_string_constant(*field_cid) {
                let val = inputs.get(i).copied().unwrap_or(Value::Nil);
                map.insert(key.to_string(), val);
            }
        }
        let map_id = self.heap.alloc_map(map);
        self.produce(term, Value::Map(map_id))
    }

    fn exec_alloc_map_spread(
        &mut self,
        entries: &[MapSpreadEntry],
        term: &Term,
        inputs: &[Value],
    ) -> ControlFlow {
        let program = self.program;
        let mut map = IndexMap::new();
        for entry in entries {
            match entry {
                MapSpreadEntry::Spread(idx) => {
                    let src = inputs.get(*idx).copied().unwrap_or(Value::Nil);
                    match src {
                        Value::Map(src_id) => {
                            // Clone all fields from the source map
                            let pairs: Vec<(String, Value)> = self
                                .heap
                                .get_map(src_id)
                                .iter()
                                .map(|(k, v)| (k.clone(), *v))
                                .collect();
                            for (k, v) in pairs {
                                map.insert(k, v);
                            }
                        }
                        Value::Nil => {} // Spreading nil is a no-op
                        _ => {
                            return ControlFlow::Error(format!(
                                "Cannot spread {} into record (expected record)",
                                src.type_name()
                            ))
                        }
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
        let map_id = self.heap.alloc_map(map);
        self.produce(term, Value::Map(map_id))
    }

    fn exec_alloc_element(
        &mut self,
        tag: ConstantId,
        prop_keys: &[ConstantId],
        term: &Term,
        inputs: &[Value],
    ) -> ControlFlow {
        let program = self.program;
        let tag_str = match program.get_string_constant(tag) {
            Some(s) => s.to_string(),
            None => return ControlFlow::Error("AllocElement: invalid tag".into()),
        };
        let tag_id = self.heap.alloc_string(tag_str);

        let mut map = IndexMap::new();
        for (i, key_cid) in prop_keys.iter().enumerate() {
            if let Some(key) = program.get_string_constant(*key_cid) {
                let val = inputs.get(i).copied().unwrap_or(Value::Nil);
                map.insert(key.to_string(), val);
            }
        }
        let props_id = self.heap.alloc_map(map);

        // Inputs are [props..., children...]
        let children_id = self.heap.alloc_list(inputs[prop_keys.len()..].to_vec());

        let elem_id = self.heap.alloc_element(tag_id, props_id, children_id);
        self.produce(term, Value::Element(elem_id))
    }

    /// Field access on records, elements, lists/strings (`.length`), and vec2.
    fn exec_get_field(
        &mut self,
        field_cid: ConstantId,
        term: &Term,
        inputs: &[Value],
    ) -> ControlFlow {
        let program = self.program;
        let obj = inputs[0];
        let field_name = match program.get_string_constant(field_cid) {
            Some(s) => s,
            None => return ControlFlow::Error("GetField: invalid field name".into()),
        };
        let val = match obj {
            Value::Map(map_id) => match self.heap.get_map(map_id).get(field_name).copied() {
                Some(v) => v,
                None => {
                    return ControlFlow::Error(format!("No field '{}' on record", field_name))
                }
            },
            Value::Element(elem_id) => match field_name {
                "tag" => Value::String(self.heap.get_element_tag(elem_id)),
                "props" => Value::Map(self.heap.get_element_props(elem_id)),
                "children" => Value::List(self.heap.get_element_children(elem_id)),
                _ => {
                    return ControlFlow::Error(format!("No field '{}' on element", field_name))
                }
            },
            Value::List(list_id) if field_name == "length" => {
                Value::Int(self.heap.list_len(list_id) as i64)
            }
            Value::String(str_id) if field_name == "length" => {
                Value::Int(self.heap.get_string(str_id).len() as i64)
            }
            Value::Vec2(x, y) => match field_name {
                "x" => Value::Float(x),
                "y" => Value::Float(y),
                _ => {
                    return ControlFlow::Error(format!(
                        "No field '{}' on vec2 (available: x, y)",
                        field_name
                    ))
                }
            },
            _ => {
                return ControlFlow::Error(format!(
                    "Cannot access field '{}' on {}",
                    field_name,
                    obj.type_name()
                ))
            }
        };
        self.produce(term, val)
    }

    fn exec_set_field(
        &mut self,
        field_cid: ConstantId,
        term: &Term,
        inputs: &[Value],
    ) -> ControlFlow {
        let obj = inputs[0];
        let val = inputs[1];
        match obj {
            Value::Map(map_id) => {
                let field_name = match self.program.get_string_constant(field_cid) {
                    Some(s) => s.to_string(),
                    None => return ControlFlow::Error("SetField: invalid field name".into()),
                };
                // Value semantics: produce a new map rather than mutating in place.
                let new_id = self.heap.map_set(map_id, field_name, val);
                self.produce(term, Value::Map(new_id))
            }
            _ => ControlFlow::Error(format!("Cannot set field on {}", obj.type_name())),
        }
    }

    fn exec_get_index(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        let obj = inputs[0];
        let idx = inputs[1];
        match (obj, idx) {
            (Value::List(list_id), Value::Int(i)) => {
                let list = self.heap.get_list(list_id);
                // Negative indices count from the end
                let index = if i < 0 {
                    (list.len() as i64 + i) as usize
                } else {
                    i as usize
                };
                match list.get(index).copied() {
                    Some(v) => self.produce(term, v),
                    None => ControlFlow::Error(format!(
                        "Index {} out of bounds (len {})",
                        i,
                        self.heap.list_len(list_id)
                    )),
                }
            }
            (Value::F64Array(arr_id), Value::Int(i)) => {
                let data = self.heap.get_f64_array(arr_id);
                if i < 0 || i as usize >= data.len() {
                    ControlFlow::Error(format!("Index {} out of bounds (len {})", i, data.len()))
                } else {
                    let v = data[i as usize];
                    self.produce(term, Value::Float(v))
                }
            }
            (Value::Map(map_id), Value::String(key_id)) => {
                let key = self.heap.get_string(key_id).to_string();
                match self.heap.get_map(map_id).get(&key).copied() {
                    Some(v) => self.produce(term, v),
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

    fn exec_set_index(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        let obj = inputs[0];
        let idx = inputs[1];
        let val = inputs[2];
        match (obj, idx) {
            (Value::List(list_id), Value::Int(i)) => {
                let len = self.heap.list_len(list_id);
                // Negative indices count from the end, symmetric with GetIndex
                // (`exec_get_index`). Required so a negative index at a non-leaf
                // level of a nested assignment (`grid[-1][0] = v`) rebuilds the
                // same slot it read from.
                let index = if i < 0 { len as i64 + i } else { i };
                if index >= 0 && (index as usize) < len {
                    // Value semantics: produce a new list rather than mutating in place.
                    let new_id = self.heap.list_set(list_id, index as usize, val);
                    self.produce(term, Value::List(new_id))
                } else {
                    ControlFlow::Error(format!("Index {} out of bounds (len {})", i, len))
                }
            }
            (Value::F64Array(arr_id), Value::Int(i)) => {
                let v = match val {
                    Value::Float(f) => f,
                    Value::Int(n) => n as f64,
                    other => {
                        return ControlFlow::Error(format!(
                            "Cannot assign {} into f64_array",
                            other.type_name()
                        ))
                    }
                };
                if i >= 0 && (i as usize) < self.heap.f64_array_len(arr_id) {
                    // Value semantics: produce a new array rather than mutating in place.
                    let new_id = self.heap.f64_array_set(arr_id, i as usize, v);
                    self.produce(term, Value::F64Array(new_id))
                } else {
                    ControlFlow::Error(format!(
                        "Index {} out of bounds (len {})",
                        i,
                        self.heap.f64_array_len(arr_id)
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
}
