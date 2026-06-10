import { describe, test, expect, beforeAll } from "vitest";
import { resolve } from "path";
import {
  ensureBuild,
  runPetal,
  showIrJsonRaw,
  runIr,
  runIrFile,
  runIrError,
} from "./helpers";

beforeAll(() => ensureBuild());

const FIXTURES = resolve(__dirname, "fixtures/ir");

// Each of these is compiled to JSON IR (`show-ir --json`) and then loaded and
// executed via `run --ir -`. The output must match running the source directly.
// This is the M2 round-trip guarantee from docs/ir-as-target.md.
const SNIPPETS: [string, string][] = [
  ["arithmetic", "print(1 + 2 * 3)"],
  [
    "if-expression",
    `let x = 10
let y = if x > 5 then "big" else "small" end
print(y)`,
  ],
  [
    "for-loop with carry",
    `let total = 0
for i in range(0, 5) do
  total = total + i
end
print(total)`,
  ],
  [
    "while-loop",
    `let i = 0
while i < 3 do
  print(i)
  i = i + 1
end`,
  ],
  [
    "function call",
    `fn add(a, b)
  a + b
end
print(add(3, 4))`,
  ],
  [
    "closure + higher-order",
    `let xs = [1, 2, 3, 4]
print(map(xs, fn(x) -> x * x))`,
  ],
  [
    "record field access",
    `let p = { x: 1, y: 2 }
print(p.x + p.y)`,
  ],
  [
    "match",
    `fn classify(n)
  match n
    when 0 -> "zero"
    when _ -> "nonzero"
  end
end
print(classify(0))
print(classify(7))`,
  ],
  [
    "state across calls",
    `fn counter()
  state c = 0
  c = c + 1
  c
end
print(counter())
print(counter())
print(counter())`,
  ],
  ["string interpolation", `print("sum is {1 + 2} and {3 * 3}")`],
];

describe("IR round-trip: show-ir --json | run --ir", () => {
  test.each(SNIPPETS)("%s", (_name, code) => {
    const direct = runPetal(code);
    const viaIr = runIr(showIrJsonRaw(code));
    expect(viaIr).toBe(direct);
  });
});

describe("IR golden fixtures (run --ir <file>)", () => {
  const cases: [string, string][] = [
    ["print_arith.ir.json", "42"],
    ["branch_phi.ir.json", "big"],
    ["state_counter.ir.json", "1\n2\n3"],
  ];
  test.each(cases)("%s", (file, expected) => {
    expect(runIrFile(resolve(FIXTURES, file))).toBe(expected);
  });
});

describe("hand-authored IR loads (optional fields)", () => {
  // No `source`, `source_map`, `functions`, or `has_errors` — proving a
  // third-party emitter need only supply the graph itself. One named Constant
  // term, never printed, so it runs cleanly with no output.
  test("minimal graph with omitted optional fields", () => {
    const ir = JSON.stringify({
      id: 0,
      terms: [
        {
          id: 0,
          op: { Constant: 0 },
          inputs: [],
          block_id: 0,
          block_next: null,
          block_prev: null,
          name: "x",
          register: 0,
          state_key: null,
          child_blocks: [],
        },
      ],
      blocks: [
        {
          id: 0,
          parent_term_id: null,
          entry: 0,
          param_names: [],
          register_count: 1,
        },
      ],
      root_block: 0,
      constants: { values: [{ Int: 42 }] },
      match_arms: {},
    });
    expect(runIr(ir)).toBe("");
  });
});

describe("IR validation rejects malformed graphs", () => {
  const base = {
    id: 0,
    blocks: [
      {
        id: 0,
        parent_term_id: null,
        entry: 0,
        param_names: [],
        register_count: 1,
      },
    ],
    root_block: 0,
    constants: { values: [] as any[] },
    match_arms: {},
  };
  const term = (op: any, extra: any = {}) => ({
    id: 0,
    op,
    inputs: [],
    block_id: 0,
    block_next: null,
    block_prev: null,
    name: null,
    register: 0,
    state_key: null,
    child_blocks: [],
    ...extra,
  });

  test("dangling input", () => {
    const ir = JSON.stringify({ ...base, terms: [term("Neg", { inputs: [99] })] });
    expect(runIrError(ir)).toMatch(/input t99 out of range/);
  });

  test("has_errors=true", () => {
    const ir = JSON.stringify({ ...base, terms: [], has_errors: true });
    expect(runIrError(ir)).toMatch(/has_errors/);
  });

  test("constant out of range", () => {
    const ir = JSON.stringify({ ...base, terms: [term({ Constant: 5 })] });
    expect(runIrError(ir)).toMatch(/constant c5 out of range/);
  });

  test("malformed JSON", () => {
    expect(runIrError("{not json")).toMatch(/invalid IR JSON/);
  });

  test("StateWrite without a StateInit", () => {
    const ir = JSON.stringify({
      ...base,
      terms: [term("StateWrite", { state_key: 7 })],
    });
    expect(runIrError(ir)).toMatch(/state key 7 has no StateInit/);
  });
});
