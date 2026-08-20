#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
//
// calc-to-ir — a REFERENCE EXTERNAL EMITTER for Petal IR (milestone M4 of
// "the dataflow IR as a legible target", ticket idea-34b8348d).
//
// This is a tiny, self-contained front-end for a toy arithmetic language
// ("calc") that compiles straight to Petal IR JSON (schema 0.2). It
// deliberately shares NO code with Petal's own lexer/parser/compiler — its
// only contract with Petal is the documented IR import format
// (docs/dev/ir-as-target.md). Pipe its output into `petal run --ir -` and the
// program runs; because the loaded IR is identical to a compiled Program, it
// also gets provenance, slicing, ExplainTerm, and state-preserving
// live-reload for free.
//
//   calc grammar (one statement per line; '#' starts a comment):
//     let <name> = <expr>
//     print <expr>
//   expr   := term (('+' | '-') term)*
//   term   := factor (('*' | '/') factor)*
//   factor := INT | NAME | '(' expr ')' | '-' factor
//
// Usage:
//   tsx ts/tools/calc-to-ir.ts program.calc | petal run --ir -
//   echo 'print 1 + 2 * 3' | tsx ts/tools/calc-to-ir.ts | petal run --ir -
//
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

type Expr =
  | { kind: "int"; value: number }
  | { kind: "var"; name: string }
  | { kind: "neg"; operand: Expr }
  | { kind: "bin"; op: "+" | "-" | "*" | "/"; left: Expr; right: Expr };

type Stmt =
  | { kind: "let"; name: string; expr: Expr }
  | { kind: "print"; expr: Expr };

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

type Tok =
  | { t: "int"; v: number }
  | { t: "name"; v: string }
  | { t: "op"; v: string }
  | { t: "eol" };

function lex(line: string): Tok[] {
  const toks: Tok[] = [];
  let i = 0;
  while (i < line.length) {
    const c = line[i];
    if (c === "#") break; // comment to end of line
    if (c === " " || c === "\t" || c === "\r") {
      i++;
      continue;
    }
    if (c >= "0" && c <= "9") {
      let j = i;
      while (j < line.length && line[j] >= "0" && line[j] <= "9") j++;
      toks.push({ t: "int", v: Number(line.slice(i, j)) });
      i = j;
      continue;
    }
    if (/[A-Za-z_]/.test(c)) {
      let j = i;
      while (j < line.length && /[A-Za-z0-9_]/.test(line[j])) j++;
      toks.push({ t: "name", v: line.slice(i, j) });
      i = j;
      continue;
    }
    if ("+-*/()=".includes(c)) {
      toks.push({ t: "op", v: c });
      i++;
      continue;
    }
    throw new Error(`calc: unexpected character ${JSON.stringify(c)} in: ${line}`);
  }
  toks.push({ t: "eol" });
  return toks;
}

// ---------------------------------------------------------------------------
// Parser (recursive descent over a single line's tokens)
// ---------------------------------------------------------------------------

class Parser {
  private pos = 0;
  constructor(private toks: Tok[]) {}

  private peek(): Tok {
    return this.toks[this.pos];
  }
  private next(): Tok {
    return this.toks[this.pos++];
  }
  private eatOp(v: string): void {
    const t = this.next();
    if (t.t !== "op" || t.v !== v) throw new Error(`calc: expected '${v}'`);
  }
  private atOp(...vs: string[]): boolean {
    const t = this.peek();
    return t.t === "op" && vs.includes(t.v);
  }

  parseStmt(): Stmt | null {
    const t = this.peek();
    if (t.t === "eol") return null; // blank / comment-only line
    if (t.t === "name" && t.v === "let") {
      this.next();
      const name = this.next();
      if (name.t !== "name") throw new Error("calc: expected name after 'let'");
      this.eatOp("=");
      const expr = this.parseExpr();
      this.expectEol();
      return { kind: "let", name: name.v, expr };
    }
    if (t.t === "name" && t.v === "print") {
      this.next();
      const expr = this.parseExpr();
      this.expectEol();
      return { kind: "print", expr };
    }
    throw new Error(`calc: expected 'let' or 'print' statement`);
  }

  private expectEol(): void {
    if (this.peek().t !== "eol") throw new Error("calc: trailing tokens after statement");
  }

  parseExpr(): Expr {
    let left = this.parseTerm();
    while (this.atOp("+", "-")) {
      const op = (this.next() as Tok & { t: "op" }).v as "+" | "-";
      const right = this.parseTerm();
      left = { kind: "bin", op, left, right };
    }
    return left;
  }

  private parseTerm(): Expr {
    let left = this.parseFactor();
    while (this.atOp("*", "/")) {
      const op = (this.next() as Tok & { t: "op" }).v as "*" | "/";
      const right = this.parseFactor();
      left = { kind: "bin", op, left, right };
    }
    return left;
  }

  private parseFactor(): Expr {
    const t = this.peek();
    if (t.t === "op" && t.v === "-") {
      this.next();
      return { kind: "neg", operand: this.parseFactor() };
    }
    if (t.t === "op" && t.v === "(") {
      this.next();
      const e = this.parseExpr();
      this.eatOp(")");
      return e;
    }
    if (t.t === "int") {
      this.next();
      return { kind: "int", value: t.v };
    }
    if (t.t === "name") {
      this.next();
      return { kind: "var", name: t.v };
    }
    throw new Error(`calc: unexpected token in expression: ${JSON.stringify(t)}`);
  }
}

function parse(source: string): Stmt[] {
  const stmts: Stmt[] = [];
  for (const line of source.split("\n")) {
    const stmt = new Parser(lex(line)).parseStmt();
    if (stmt) stmts.push(stmt);
  }
  return stmts;
}

// ---------------------------------------------------------------------------
// IR emitter
// ---------------------------------------------------------------------------
//
// Emits a single-block schema-0.2 Program, exercising everything the format
// lets an emitter leave out:
//
// - Ordering is declarative: the block's `terms` array lists term ids in
//   execution order. No linked list to maintain.
// - Registers are omitted entirely — the loader recomputes the assignment.
// - Builtins are called through `BuiltinCall` with the builtin's *name* as a
//   string constant. No phantom terms, and no dependence on Petal's builtin
//   registration order.
// - Defaulted fields (`name`, `inputs` when empty, `functions`, `has_errors`,
//   `source_map`, ...) are simply absent.

interface Term {
  id: number;
  op: string | { Constant: number } | { BuiltinCall: number };
  inputs?: number[];
  block_id: number;
  name?: string;
}

type Constant = { Int: number } | { String: string };

const BIN_OP: Record<string, string> = { "+": "Add", "-": "Sub", "*": "Mul", "/": "Div" };

class Emitter {
  private terms: Term[] = [];
  private constants: Constant[] = [];
  private env = new Map<string, number>(); // calc var name -> term id holding its value
  private order: number[] = []; // execution order (the block's `terms` array)

  /** Allocate a constant slot (deduped) and return its index. */
  private constId(value: number | string): number {
    const want = JSON.stringify(
      typeof value === "number" ? { Int: value } : { String: value }
    );
    const existing = this.constants.findIndex((c) => JSON.stringify(c) === want);
    if (existing >= 0) return existing;
    this.constants.push(JSON.parse(want));
    return this.constants.length - 1;
  }

  /** Append a term and record it in the block's execution order. */
  private add(op: Term["op"], inputs: number[], name?: string): number {
    const id = this.terms.length;
    const term: Term = { id, op, block_id: 0 };
    if (inputs.length > 0) term.inputs = inputs;
    if (name !== undefined) term.name = name;
    this.terms.push(term);
    this.order.push(id);
    return id;
  }

  private emitExpr(e: Expr): number {
    switch (e.kind) {
      case "int":
        return this.add({ Constant: this.constId(e.value) }, []);
      case "var": {
        const ref = this.env.get(e.name);
        if (ref === undefined) throw new Error(`calc: undefined variable '${e.name}'`);
        return ref;
      }
      case "neg": {
        // Lower unary minus to (0 - operand) so we need no Neg op.
        const zero = this.add({ Constant: this.constId(0) }, []);
        const operand = this.emitExpr(e.operand);
        return this.add("Sub", [zero, operand]);
      }
      case "bin": {
        const left = this.emitExpr(e.left);
        const right = this.emitExpr(e.right);
        return this.add(BIN_OP[e.op], [left, right]);
      }
    }
  }

  emit(stmts: Stmt[]): unknown {
    for (const stmt of stmts) {
      if (stmt.kind === "let") {
        const valueId = this.emitExpr(stmt.expr);
        // Bind the name to a named Copy so the value is legible in the graph.
        const bound = this.add("Copy", [valueId], stmt.name);
        this.env.set(stmt.name, bound);
      } else {
        const argId = this.emitExpr(stmt.expr);
        this.add({ BuiltinCall: this.constId("print") }, [argId]);
      }
    }

    return {
      schema: "0.2",
      id: 0,
      terms: this.terms,
      blocks: [{ id: 0, terms: this.order }],
      root_block: 0,
      constants: { values: this.constants },
    };
  }
}

/** Compile calc source to a Petal IR Program object. */
export function compileCalcToIr(source: string): unknown {
  return new Emitter().emit(parse(source));
}

/** Compile calc source to Petal IR JSON text. */
export function compileCalcToIrJson(source: string): string {
  return JSON.stringify(compileCalcToIr(source), null, 2);
}

// ---------------------------------------------------------------------------
// CLI: read calc source from a file arg or stdin, write IR JSON to stdout.
// ---------------------------------------------------------------------------

function main(): void {
  const arg = process.argv[2];
  const source =
    arg && arg !== "-" ? readFileSync(arg, "utf-8") : readFileSync(0, "utf-8");
  process.stdout.write(compileCalcToIrJson(source) + "\n");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main();
}
