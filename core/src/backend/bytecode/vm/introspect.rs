//! `fn_info(f)` and `fn_ast(f)` — function introspection (experimental).
//!
//! VM intrinsics rather than natives because they read what `PetalCxt` does
//! not carry: the closure table (which function, which captured values) and
//! the program (its source, and where each function is written). The
//! source-level half lives in [`crate::fn_introspect`]; this file joins it
//! with the run-time half and builds the Petal values.
//!
//! Both are pure functions of the closure and the program text, so they do
//! not disturb a memoized scope.

use super::*;

use crate::fn_introspect::Fnv;
use crate::program::{FunctionId, base_fn_name};
use crate::value::json_to_value;

impl<'a> Vm<'a> {
    /// The closure behind a function value, or `None` for a callable with no
    /// single Petal function behind it (a builtin, an overload set).
    fn inspectable(&self, name: &str, args: &[Value]) -> Result<Option<(FunctionId, Vec<Value>)>, String> {
        let [f] = args else {
            return Err(format!("{name}() expects 1 argument, got {}", args.len()));
        };
        match *f {
            Value::Closure(cid) => {
                let c = self.closures.closure(cid);
                Ok(Some((c.function_id, c.captures.clone())))
            }
            Value::OverloadSet(_) | Value::NativeFunction(_) => Ok(None),
            other => Err(format!(
                "{name}() expects a function, got {}",
                other.type_name()
            )),
        }
    }

    /// What a capture holds, as user code would read it: a `var` cell is read
    /// through (a cell itself never reaches Petal code).
    fn capture_value(&self, v: Value) -> Value {
        match v {
            Value::Cell(id) => self.heap.cell_read(id),
            other => other,
        }
    }

    /// Fold the code of `fid` and of every function it captures into `h`.
    /// Returns false when some function on the way has no source to hash.
    fn hash_code(&self, fid: FunctionId, captures: &[Value], seen: &mut Vec<u32>, h: &mut Fnv) -> bool {
        if seen.contains(&fid.0) {
            // A cycle (mutual recursion): already folded in.
            h.write(b"<");
            return true;
        }
        seen.push(fid.0);
        let Some(meta) = self.program.introspect.meta(self.program, fid) else {
            return false;
        };
        h.write(&meta.code_hash.to_le_bytes());
        let mut ok = true;
        for cap in captures {
            match self.capture_value(*cap) {
                Value::Closure(cid) => {
                    let c = self.closures.closure(cid);
                    let (inner, caps) = (c.function_id, c.captures.clone());
                    h.write(b"(");
                    ok &= self.hash_code(inner, &caps, seen, h);
                    h.write(b")");
                }
                Value::OverloadSet(sid) => {
                    let entries: Vec<_> = self.closures.set(sid).iter().map(|e| e.closure_id).collect();
                    for cid in entries {
                        let c = self.closures.closure(cid);
                        let (inner, caps) = (c.function_id, c.captures.clone());
                        h.write(b"(");
                        ok &= self.hash_code(inner, &caps, seen, h);
                        h.write(b")");
                    }
                }
                // A captured value is data, not code: it is not hashed.
                _ => h.write(b"."),
            }
        }
        ok
    }

    fn str_value(&mut self, s: &str) -> Value {
        Value::String(self.heap.alloc_string(s.to_string()))
    }

    fn opt_str_value(&mut self, s: Option<&str>) -> Value {
        match s {
            Some(s) => self.str_value(s),
            None => Value::Nil,
        }
    }

    /// `fn_info(f)` — the cheap description of a function value: its name,
    /// where it is written, its parameters (annotation, default), its
    /// declared return type, what it captured, and a hash of its code.
    /// `nil` for a builtin or an overload set.
    pub(super) fn builtin_fn_info(&mut self, args: &[Value]) -> Result<Value, String> {
        let Some((fid, captures)) = self.inspectable("fn_info", args)? else {
            return Ok(Value::Nil);
        };
        let program = self.program;
        let def = &program.functions[fid.0 as usize];
        let meta = program.introspect.meta(program, fid);

        let mut hash = Fnv::new();
        let hashed = self.hash_code(fid, &captures, &mut Vec::new(), &mut hash);
        let code_hash = if hashed {
            self.str_value(&format!("{:016x}", hash.0))
        } else {
            Value::Nil
        };

        let optional_from = def.required_params();
        let mut params = Vec::with_capacity(def.params.len());
        for (i, name) in def.params.iter().enumerate() {
            let pm = meta.as_ref().and_then(|m| m.params.get(i));
            let mut rec = crate::heap::record_map_with_capacity(5);
            let v = self.str_value(name);
            rec.insert("name".to_string(), v);
            let v = self.opt_str_value(pm.and_then(|p| p.ty.as_deref()));
            rec.insert("type".to_string(), v);
            rec.insert("has_default".to_string(), Value::Bool(i >= optional_from));
            let v = match pm.and_then(|p| p.default.as_ref()) {
                Some(json) => json_to_value(json, self.heap)?,
                None => Value::Nil,
            };
            rec.insert("default".to_string(), v);
            let v = self.opt_str_value(pm.and_then(|p| p.default_source.as_deref()));
            rec.insert("default_source".to_string(), v);
            params.push(Value::Map(self.heap.alloc_map(rec)));
        }

        let mut caps = Vec::with_capacity(captures.len());
        for (name, value) in def.capture_names.iter().zip(captures.iter()) {
            let mut rec = crate::heap::record_map_with_capacity(2);
            let v = self.str_value(name);
            rec.insert("name".to_string(), v);
            rec.insert("value".to_string(), self.capture_value(*value));
            caps.push(Value::Map(self.heap.alloc_map(rec)));
        }

        let mut rec = crate::heap::record_map_with_capacity(8);
        let name = meta
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| def.name.as_deref().map(|n| base_fn_name(n).to_string()));
        let v = self.opt_str_value(name.as_deref());
        rec.insert("name".to_string(), v);
        let v = self.opt_str_value(meta.as_ref().and_then(|m| m.file.as_deref()));
        rec.insert("file".to_string(), v);
        let pos = |n: Option<u32>| n.map_or(Value::Nil, |n| Value::Int(n as i64));
        rec.insert("line".to_string(), pos(meta.as_ref().map(|m| m.line)));
        rec.insert("column".to_string(), pos(meta.as_ref().map(|m| m.column)));
        rec.insert("code_hash".to_string(), code_hash);
        rec.insert("params".to_string(), Value::List(self.heap.alloc_list(params)));
        let v = self.opt_str_value(meta.as_ref().and_then(|m| m.returns.as_deref()));
        rec.insert("returns".to_string(), v);
        rec.insert("captures".to_string(), Value::List(self.heap.alloc_list(caps)));
        Ok(Value::Map(self.heap.alloc_map(rec)))
    }

    /// `fn_ast(f)` — the function's syntax tree as records and lists (the
    /// shapes are in docs/function-introspection.md). `nil` when there is no
    /// source for it: a builtin, an overload set, a class constructor, a
    /// program loaded from IR.
    pub(super) fn builtin_fn_ast(&mut self, args: &[Value]) -> Result<Value, String> {
        let Some((fid, _)) = self.inspectable("fn_ast", args)? else {
            return Ok(Value::Nil);
        };
        let program = self.program;
        match program.introspect.meta(program, fid) {
            Some(meta) => json_to_value(&meta.ast, self.heap),
            None => Ok(Value::Nil),
        }
    }
}
