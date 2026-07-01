//! Human- and machine-readable rendering of lowered bytecode, for the
//! `show-bytecode` CLI command and the `ShowBytecode` MCP tool. Mirrors the role
//! of `ir_display` for the term graph.

use serde_json::{json, Value as Json};

use super::isa::{BytecodeFn, BytecodeProgram, Inst};
use crate::constant_table::ConstantValue;
use crate::program::Program;

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
        Some(n) => format!(" {}", n),
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

/// Resolve a constant to a compact literal for display.
fn kconst(program: &Program, k: crate::constant_table::ConstantId) -> String {
    match program.constants.get(k) {
        ConstantValue::Nil => "nil".to_string(),
        ConstantValue::Bool(b) => b.to_string(),
        ConstantValue::Int(n) => n.to_string(),
        ConstantValue::Float(bits) => f64::from_bits(*bits).to_string(),
        ConstantValue::String(s) => format!("{:?}", s),
    }
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
        ForEachInit { iter, slot, idx_ctx } => {
            format!("foreach_init r{} slot{} idx_ctx={}", iter, slot, idx_ctx)
        }
        ForEachNext { slot, var, exit } => {
            format!("foreach_next slot{} -> r{} else -> {}", slot, var, exit)
        }
        RangeInit { start, end, slot, idx_ctx } => {
            format!("range_init r{}..r{} slot{} idx_ctx={}", start, end, slot, idx_ctx)
        }
        RangeNext { slot, var, exit } => {
            format!("range_next slot{} -> r{} else -> {}", slot, var, exit)
        }
        WhileInit { slot } => format!("while_init slot{}", slot),
        LoopBumpIdx { slot } => format!("loop_bump_idx slot{}", slot),
        LoopPop { slot } => format!("loop_pop slot{}", slot),
        Call { dst, callee, args } => format!("r{} = call r{} {}", dst, callee, reglist(args)),
        MethodCall { dst, recv, name, args } => {
            format!("r{} = r{}.{}{}", dst, recv, kconst(program, *name), reglist(args))
        }
        BuiltinCall { dst, name, args } => {
            format!("r{} = builtin {}{}", dst, kconst(program, *name), reglist(args))
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
        AllocMap { dst, fields, vals } => {
            let pairs: Vec<String> = fields
                .iter()
                .zip(vals.iter())
                .map(|(f, v)| format!("{}: r{}", kconst(program, *f), v))
                .collect();
            format!("r{} = map {{{}}}", dst, pairs.join(", "))
        }
        AllocMapSpread { dst, ins, .. } => format!("r{} = map_spread {}", dst, reglist(ins)),
        AllocElement { dst, tag, ins, .. } => {
            format!("r{} = element {} {}", dst, kconst(program, *tag), reglist(ins))
        }
        MakeEnumVariant { dst, name, fields } => {
            format!("r{} = enum {}{}", dst, kconst(program, *name), reglist(fields))
        }
        GetField { dst, obj, field } => format!("r{} = r{}.{}", dst, obj, kconst(program, *field)),
        SetField { dst, obj, field, val } => {
            format!("r{} = set r{}.{} = r{}", dst, obj, kconst(program, *field), val)
        }
        GetIndex { dst, obj, idx } => format!("r{} = r{}[r{}]", dst, obj, idx),
        SetIndex { dst, obj, idx, val } => {
            format!("r{} = set r{}[r{}] = r{}", dst, obj, idx, val)
        }
        SetFieldInPlace { dst, obj, field, val } => {
            format!("r{} = set_in_place r{}.{} = r{}", dst, obj, kconst(program, *field), val)
        }
        SetIndexInPlace { dst, obj, idx, val } => {
            format!("r{} = set_in_place r{}[r{}] = r{}", dst, obj, idx, val)
        }
        StateInit { dst, base, in_loop, init, key } => format!(
            "r{} = state_init k{} in_loop={} init@{}{}",
            dst, base.0, in_loop, init, opt_key(key)
        ),
        StateRead { dst, base, in_loop } => {
            format!("r{} = state_read k{} in_loop={}", dst, base.0, in_loop)
        }
        StateWrite { dst, base, in_loop, val, key } => format!(
            "r{} = state_write k{} in_loop={} = r{}{}",
            dst, base.0, in_loop, val, opt_key(key)
        ),
        MatchArm { subject, term, arm, next, dst } => format!(
            "match_arm r{} t{} arm{} -> r{} else -> {}",
            subject, term.0, arm, dst, next
        ),
        MatchFail { subject } => format!("match_fail r{}", subject),
        Error { msg } => format!("error {}", kconst(program, *msg)),
    }
}

fn opt_key(key: &Option<u16>) -> String {
    match key {
        Some(r) => format!(" key=r{}", r),
        None => String::new(),
    }
}

/// Render a lowered program as JSON: one object per function with an array of
/// disassembled instruction strings plus register metadata.
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
        .map(|(i, inst)| json!({ "ip": i, "text": render_inst(inst, program) }))
        .collect();
    json!({
        "fn": f.func_id.map(|id| id.0),
        "name": f.name,
        "reg_count": f.reg_count,
        "loop_slots": f.loop_slots,
        "param_regs": f.param_regs,
        "capture_regs": f.capture_regs,
        "self_ref_reg": f.self_ref_reg,
        "code": code,
    })
}
