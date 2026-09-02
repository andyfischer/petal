//! Human- and machine-readable rendering of lowered bytecode, for the
//! `show-bytecode` CLI command and the `ShowBytecode` MCP tool. Mirrors the role
//! of `ir_display` for the term graph.

use serde_json::{Value as Json, json};

use super::isa::{BytecodeFn, BytecodeProgram, Inst};
use crate::program::{Program, base_fn_name};

/// Render a lowered program as annotated text, one function per section.
pub fn render_text(bc: &BytecodeProgram, program: &Program) -> String {
    let mut out = String::new();
    render_fn_text(&mut out, &bc.root, program);
    for f in &bc.fns {
        out.push('\n');
        render_fn_text(&mut out, f, program);
    }
    out
}

fn render_fn_text(out: &mut String, f: &BytecodeFn, program: &Program) {
    let title = match f.func_id {
        Some(id) => format!("fn f{}{}", id.0, name_suffix(&f.name)),
        None => "fn <root>".to_string(),
    };
    out.push_str(&format!(
        "{}  ({} regs, {} loop slots)\n",
        title, f.reg_count, f.loop_slots
    ));
    if !f.param_regs.is_empty() {
        out.push_str(&format!("  params:   {}\n", regs(&f.param_regs)));
    }
    if !f.capture_regs.is_empty() {
        out.push_str(&format!("  captures: {}\n", regs(&f.capture_regs)));
    }
    if let Some(r) = f.self_ref_reg {
        out.push_str(&format!("  self:     r{}\n", r));
    }
    for (i, inst) in f.code.iter().enumerate() {
        out.push_str(&format!("  {:>4}  {}\n", i, render_inst(inst, program)));
    }
}

fn name_suffix(name: &Option<String>) -> String {
    match name {
        // An overload variant is `box#1` internally; the listing names the
        // function as the source wrote it.
        Some(n) => format!(" {}", base_fn_name(n)),
        None => String::new(),
    }
}

fn regs(rs: &[u16]) -> String {
    rs.iter()
        .map(|r| format!("r{}", r))
        .collect::<Vec<_>>()
        .join(", ")
}

fn reglist(rs: &[u16]) -> String {
    format!("[{}]", regs(rs))
}

/// A call's argument list, with each named argument prefixed `name: `. Falls
/// back to [`reglist`] verbatim for the all-positional case, so the disassembly
/// of an ordinary call is unchanged.
fn arglist(
    program: &Program,
    rs: &[u16],
    names: &crate::backend::bytecode::isa::ArgNames,
) -> String {
    if names.is_empty() {
        return reglist(rs);
    }
    let parts: Vec<String> = rs
        .iter()
        .enumerate()
        .map(|(i, r)| match names.get(i).and_then(|n| n.as_ref()) {
            // Bare, not quoted: it reads as the parameter it binds.
            Some(c) => match program.get_string_constant(*c) {
                Some(n) => format!("{}: r{}", n, r),
                None => format!("{}: r{}", kconst(program, *c), r),
            },
            None => format!("r{}", r),
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

/// Resolve a constant to a compact literal for display.
fn kconst(program: &Program, k: crate::constant_table::ConstantId) -> String {
    program.constants.get(k).display_compact()
}

fn render_inst(inst: &Inst, program: &Program) -> String {
    use Inst::*;
    match inst {
        LoadConst { dst, k } => format!("r{} = const {}", dst, kconst(program, *k)),
        LoadNil { dst } => format!("r{} = nil", dst),
        LoadBool { dst, val } => format!("r{} = {}", dst, val),
        Move { dst, src } => format!("r{} = r{}", dst, src),
        Add { dst, a, b } => format!("r{} = r{} + r{}", dst, a, b),
        Sub { dst, a, b } => format!("r{} = r{} - r{}", dst, a, b),
        Mul { dst, a, b } => format!("r{} = r{} * r{}", dst, a, b),
        Div { dst, a, b } => format!("r{} = r{} / r{}", dst, a, b),
        Mod { dst, a, b } => format!("r{} = r{} % r{}", dst, a, b),
        Neg { dst, a } => format!("r{} = -r{}", dst, a),
        Eq { dst, a, b } => format!("r{} = r{} == r{}", dst, a, b),
        Ne { dst, a, b } => format!("r{} = r{} != r{}", dst, a, b),
        Lt { dst, a, b } => format!("r{} = r{} < r{}", dst, a, b),
        Le { dst, a, b } => format!("r{} = r{} <= r{}", dst, a, b),
        Gt { dst, a, b } => format!("r{} = r{} > r{}", dst, a, b),
        Ge { dst, a, b } => format!("r{} = r{} >= r{}", dst, a, b),
        Not { dst, a } => format!("r{} = !r{}", dst, a),
        Concat { dst, a, b } => format!("r{} = r{} ++ r{}", dst, a, b),
        Jump { to } => format!("jump -> {}", to),
        JumpIfFalse { cond, to } => format!("jump_if_false r{} -> {}", cond, to),
        JumpIfTrue { cond, to } => format!("jump_if_true r{} -> {}", cond, to),
        JumpIfPresent { cond, to } => format!("jump_if_present r{} -> {}", cond, to),
        JumpIfPending { cond, to } => format!("jump_if_pending r{} -> {}", cond, to),
        ForEachInit { iter, slot } => format!("foreach_init r{} slot{}", iter, slot),
        ForEachNext { slot, var, exit } => {
            format!("foreach_next slot{} -> r{} else -> {}", slot, var, exit)
        }
        RangeInit { start, end, slot } => {
            format!("range_init r{}..r{} slot{}", start, end, slot)
        }
        RangeNext { slot, var, exit } => {
            format!("range_next slot{} -> r{} else -> {}", slot, var, exit)
        }
        WhileInit { slot } => format!("while_init slot{}", slot),
        LoopBumpIdx { slot } => format!("loop_bump_idx slot{}", slot),
        LoopPop { slot } => format!("loop_pop slot{}", slot),
        LoopCollect { slot, src } => format!("loop_collect slot{} <- r{}", slot, src),
        LoopCollectEnd { slot, dst } => format!("r{} = loop_collect_end slot{}", dst, slot),
        Call {
            dst,
            callee,
            args,
            arg_names,
        } => format!(
            "r{} = call r{} {}",
            dst,
            callee,
            arglist(program, args, arg_names)
        ),
        MethodCall {
            dst,
            recv,
            name,
            args,
            hint,
            arg_names,
        } => {
            let hint = match hint {
                Some(h) => format!(" ?{}", kconst(program, *h)),
                None => String::new(),
            };
            format!(
                "r{} = r{}.{} {}{}",
                dst,
                recv,
                kconst(program, *name),
                arglist(program, args, arg_names),
                hint
            )
        }
        BuiltinCall {
            dst,
            name,
            args,
            in_place,
            arg_names,
        } => {
            let tag = if *in_place {
                "builtin_in_place"
            } else {
                "builtin"
            };
            format!(
                "r{} = {} {} {}",
                dst,
                tag,
                kconst(program, *name),
                arglist(program, args, arg_names)
            )
        }
        MakeClosure { dst, func, caps } => {
            format!("r{} = closure f{} caps={}", dst, func.0, reglist(caps))
        }
        MakeOverloadSet { dst, closures } => {
            format!("r{} = overload_set {}", dst, reglist(closures))
        }
        Return { val } => match val {
            Some(r) => format!("return r{}", r),
            None => "return".to_string(),
        },
        AllocList { dst, elems } => format!("r{} = list {}", dst, reglist(elems)),
        AllocMap {
            dst,
            fields,
            vals,
            class,
        } => {
            let pairs: Vec<String> = fields
                .iter()
                .zip(vals.iter())
                .map(|(f, v)| format!("{}: r{}", kconst(program, *f), v))
                .collect();
            match class {
                Some(c) => format!(
                    "r{} = {} {{{}}}",
                    dst,
                    kconst(program, *c),
                    pairs.join(", ")
                ),
                None => format!("r{} = map {{{}}}", dst, pairs.join(", ")),
            }
        }
        AllocMapSpread { dst, ins, .. } => format!("r{} = map_spread {}", dst, reglist(ins)),
        AllocElement { dst, tag, ins, .. } => {
            format!(
                "r{} = element {} {}",
                dst,
                kconst(program, *tag),
                reglist(ins)
            )
        }
        MakeEnumVariant { dst, name, fields } => {
            format!(
                "r{} = enum {} {}",
                dst,
                kconst(program, *name),
                reglist(fields)
            )
        }
        GetField {
            dst,
            obj,
            field,
            opt,
        } => format!(
            "r{} = r{}{}{}",
            dst,
            obj,
            if *opt { "?." } else { "." },
            kconst(program, *field)
        ),
        SetField {
            dst,
            obj,
            field,
            val,
        } => {
            format!(
                "r{} = set r{}.{} = r{}",
                dst,
                obj,
                kconst(program, *field),
                val
            )
        }
        GetIndex { dst, obj, idx, opt } => {
            format!(
                "r{} = r{}{}[r{}]",
                dst,
                obj,
                if *opt { "?" } else { "" },
                idx
            )
        }
        SetIndex { dst, obj, idx, val } => {
            format!("r{} = set r{}[r{}] = r{}", dst, obj, idx, val)
        }
        SetFieldInPlace {
            dst,
            obj,
            field,
            val,
        } => {
            format!(
                "r{} = set_in_place r{}.{} = r{}",
                dst,
                obj,
                kconst(program, *field),
                val
            )
        }
        SetIndexInPlace { dst, obj, idx, val } => {
            format!("r{} = set_in_place r{}[r{}] = r{}", dst, obj, idx, val)
        }
        CellNew { dst, init } => format!("r{} = cell_new r{}", dst, init),
        CellRead { dst, cell } => format!("r{} = cell_read r{}", dst, cell),
        CellWrite { dst, cell, val } => format!("r{} = cell_write r{} <- r{}", dst, cell, val),
        StateInit {
            dst,
            base,
            after,
            key,
        } => format!(
            "r{} = state_init k{} after@{}{}",
            dst,
            base.0,
            after,
            opt_key(key)
        ),
        StateRead {
            dst,
            base,
            path_pop,
        } => format!("r{} = state_read k{}{}", dst, base.0, pop(path_pop)),
        StateWrite {
            dst,
            base,
            val,
            key,
            init,
            path_pop,
        } => format!(
            "r{} = state_write{} k{}{} = r{}{}",
            dst,
            if *init { " init" } else { "" },
            base.0,
            pop(path_pop),
            val,
            opt_key(key)
        ),
        MatchArm {
            subject,
            term,
            arm,
            next,
            dst,
        } => format!(
            "match_arm r{} t{} arm{} -> r{} else -> {}",
            subject, term.0, arm, dst, next
        ),
        MatchFail { subject } => format!("match_fail r{}", subject),
        Error { msg } => format!("error {}", kconst(program, *msg)),
    }
}

/// The path-pop suffix on a state access, printed only when it is nonzero (a
/// reassignment nested deeper in loops than its declaration).
fn pop(path_pop: &u32) -> String {
    match path_pop {
        0 => String::new(),
        n => format!(" pop{n}"),
    }
}

fn opt_key(key: &Option<u16>) -> String {
    match key {
        Some(r) => format!(" key=r{}", r),
        None => String::new(),
    }
}

#[cfg(test)]
mod inst_json_tests {
    use smallvec::smallvec;

    use super::*;
    use crate::constant_table::ConstantId;
    use crate::program::FunctionId;

    /// The `inst` field of a `show-bytecode --json` row is the externally-tagged
    /// serde form of [`Inst`]. These pin the encoding — variant name as the
    /// single key, operand structs inside — so a change to the derive setup (or
    /// to an operand type's serialization) breaks loudly instead of silently
    /// reshaping the tool-facing JSON.
    #[test]
    fn externally_tagged_with_named_operands() {
        assert_eq!(
            serde_json::to_value(Inst::Add { dst: 4, a: 0, b: 1 }).unwrap(),
            json!({ "Add": { "dst": 4, "a": 0, "b": 1 } })
        );
        assert_eq!(
            serde_json::to_value(Inst::Jump { to: 7 }).unwrap(),
            json!({ "Jump": { "to": 7 } })
        );
    }

    /// `ConstantId` operands serialize as the bare constant-table index (the
    /// table itself is documented in the IR JSON), and reg lists as arrays.
    #[test]
    fn constant_ids_are_numbers_and_reglists_are_arrays() {
        assert_eq!(
            serde_json::to_value(Inst::LoadConst {
                dst: 2,
                k: ConstantId(3)
            })
            .unwrap(),
            json!({ "LoadConst": { "dst": 2, "k": 3 } })
        );
        assert_eq!(
            serde_json::to_value(Inst::BuiltinCall {
                dst: 5,
                name: ConstantId(1),
                args: smallvec![0, 4],
                in_place: false,
                arg_names: smallvec![],
            })
            .unwrap(),
            json!({ "BuiltinCall": { "dst": 5, "name": 1, "args": [0, 4], "in_place": false } })
        );
        assert_eq!(
            serde_json::to_value(Inst::MakeClosure {
                dst: 0,
                func: FunctionId(2),
                caps: smallvec![],
            })
            .unwrap(),
            json!({ "MakeClosure": { "dst": 0, "func": 2, "caps": [] } })
        );
    }

    /// Optional operands stay present as `null` — consumers get a fixed field
    /// set per opcode rather than a shape that shifts with operand values.
    #[test]
    fn optional_operands_serialize_as_null() {
        assert_eq!(
            serde_json::to_value(Inst::Return { val: None }).unwrap(),
            json!({ "Return": { "val": null } })
        );
        assert_eq!(
            serde_json::to_value(Inst::Return { val: Some(3) }).unwrap(),
            json!({ "Return": { "val": 3 } })
        );
    }
}

/// Render a lowered program as JSON: one object per function with an array of
/// instruction rows plus register metadata. Each row carries the instruction
/// twice: `inst` is the structured operand-level encoding (the [`Inst`] enum's
/// externally-tagged serde form, for tooling) and `text` is the disassembled
/// string (for reading).
pub fn render_json(bc: &BytecodeProgram, program: &Program) -> Json {
    let mut fns = vec![fn_json(&bc.root, program)];
    for f in &bc.fns {
        fns.push(fn_json(f, program));
    }
    json!({ "functions": fns })
}

fn fn_json(f: &BytecodeFn, program: &Program) -> Json {
    let code: Vec<Json> = f
        .code
        .iter()
        .enumerate()
        .map(|(i, inst)| {
            json!({
                "ip": i,
                "inst": serde_json::to_value(inst).expect("Inst serializes"),
                "text": render_inst(inst, program),
            })
        })
        .collect();
    json!({
        "fn": f.func_id.map(|id| id.0),
        "name": f.name.as_deref().map(base_fn_name),
        "reg_count": f.reg_count,
        "loop_slots": f.loop_slots,
        "param_regs": f.param_regs,
        "capture_regs": f.capture_regs,
        "self_ref_reg": f.self_ref_reg,
        "code": code,
    })
}
