# IR as a target

[`calc-to-ir.ts`](calc-to-ir.ts) is a reference external front end for Petal
IR. It compiles a toy arithmetic language ("calc": `let`, `print`, integer
arithmetic with precedence, parentheses and unary minus) straight to Petal IR
JSON. It shares no code with Petal's lexer, parser or compiler: its only
contract with Petal is the IR import format described in
[docs/dev/ir-as-target.md](../../docs/dev/ir-as-target.md).

```bash
echo 'print 1 + 2 * 3' | ./examples/ir-target/calc-to-ir.ts | petal run --ir -   # => 7
```

`test/vitest/calc-emitter.test.ts` runs it.
