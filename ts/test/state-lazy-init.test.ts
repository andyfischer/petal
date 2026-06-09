// Lazy state initialization: the RHS of `state x = expr` should only be
// evaluated on first encounter of (state_key, loop_indices). Subsequent
// encounters skip the init expression entirely.
//
// This matters for:
//   - Performance: per-iteration `state x = expensive_init(item)` runs N times
//     on the first frame, then ~0 per frame.
//   - Correctness: `state x = random(0, 100)` should pin a random value at
//     init time, not roll a new one each frame. (Today the new value is
//     ignored because the key is already set, but the work is wasted.)
//   - Top-level state with large literal initializers (e.g. petal-fps's
//     `state buildings = [{...12 records...}]`).
//
// Implementation: StateInit becomes a control-flow term with
// `child_blocks: [init_block]`. Eval pushes the init block only when the
// runtime key isn't yet set; the init block's last term writes the init
// value into StateInit's register on pop. A pop-time hook inserts that
// value into the persistent state map.

import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  runPetal,
  termsByOp,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("lazy state init — IR shape", () => {
  it("StateInit has a child_block holding the init expression", () => {
    const ir = showIrJson("state x = 1 + 2");
    const inits = termsByOp(ir, "StateInit");
    expect(inits.length).toBe(1);
    expect(inits[0].child_blocks.length).toBe(1);
  });

  it("StateInit with no explicit key has empty inputs (init moved to child block)", () => {
    const ir = showIrJson("state x = 42");
    const inits = termsByOp(ir, "StateInit");
    expect(inits[0].inputs).toEqual([]);
  });

  it("StateInit with explicit key has [key] inputs (init moved to child block)", () => {
    const ir = showIrJson(`
      for x in [1] do
        state(x) count = 0
      end
    `);
    const inits = termsByOp(ir, "StateInit");
    expect(inits.length).toBe(1);
    expect(inits[0].inputs.length).toBe(1);
    expect(inits[0].child_blocks.length).toBe(1);
  });

  it("init block's last term is the init expression's value", () => {
    const ir = showIrJson("state x = 100");
    const inits = termsByOp(ir, "StateInit");
    const initBlockId = inits[0].child_blocks[0];
    const initBlock = ir.blocks.find((b: any) => b.id === initBlockId);
    expect(initBlock).toBeDefined();
    expect(initBlock.entry).not.toBeNull();
  });
});

describe("lazy state init — runtime", () => {
  // Note: we observe init runs via `print` inside the init expression.
  // Using a counter `state` variable + a rebinding assignment doesn't
  // surface the new value to the outer scope (block expressions don't
  // emit phi joins), so print-based observability is the cleanest signal.

  it("init expression with a side effect runs only once at top level", () => {
    const out = runPetal(`
      state x = fn()
        print("init x")
        99
      end()
      state y = x + 1
      print(x, y)
    `);
    expect(out).toBe("init x\n99 100");
  });

  it("init runs only once across multiple top-level state reads", () => {
    const out = runPetal(`
      state value = fn()
        print("init value")
        7
      end()
      print(value)
      print(value)
    `);
    expect(out).toBe("init value\n7\n7");
  });

  it("per-iteration init runs once per iteration index", () => {
    const out = runPetal(`
      for i in [1, 2, 3] do
        state x = fn()
          print("init")
          i * 10
        end()
        print(x)
      end
    `);
    // Three iterations, three first-time inits per iteration key.
    expect(out).toBe("init\n10\ninit\n20\ninit\n30");
  });

  it("explicit-key init runs once per unique key, not per visit", () => {
    const out = runPetal(`
      let visits = ["a", "b", "a", "c", "b", "a"]
      for v in visits do
        state(v) seen = fn()
          print("init", v)
          0
        end()
      end
    `);
    // Only "a", "b", "c" are unique → 3 inits despite 6 visits.
    expect(out).toBe("init a\ninit b\ninit c");
  });

  it("expensive init only runs on first encounter (idempotent in absence of side effects)", () => {
    // sqrt is deterministic; we just check the value is correctly preserved.
    const out = runPetal(`
      state x = sqrt(16.0)
      x = x + 1.0
      state y = sqrt(16.0)
      print(x, y)
    `);
    // x is initialized to 4.0, then mutated to 5.0. y init also runs (different name).
    expect(out).toBe("5.0 4.0");
  });
});

describe("lazy state init — preserves existing semantics", () => {
  it("StateRead still returns the latest written value", () => {
    const out = runPetal(`
      state x = 0
      x = 10
      print(x)
      x = 20
      print(x)
    `);
    expect(out).toBe("10\n20");
  });

  it("compound assignment on state still works", () => {
    const out = runPetal(`
      state count = 0
      count += 5
      count += 3
      print(count)
    `);
    expect(out).toBe("8");
  });

  it("per-iteration state still gets fresh value per iteration", () => {
    const out = runPetal(`
      for item in [10, 20, 30] do
        state count = 0
        count += 1
        print(count)
      end
    `);
    expect(out).toBe("1\n1\n1");
  });

  it("explicit-key state preserves value across re-encounters", () => {
    const out = runPetal(`
      let items = [{id: 1}, {id: 2}, {id: 1}]
      for item in items do
        state(item.id) clicks = 0
        clicks += 1
        print(clicks)
      end
    `);
    expect(out).toBe("1\n1\n2");
  });
});
