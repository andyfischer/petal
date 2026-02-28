import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  runPetal,
  termsByOp,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("per-iteration loop state", () => {
  describe("for loops", () => {
    it("state inside for-loop gets separate value per iteration", () => {
      const output = runPetal(`
        for item in [10, 20, 30] {
          state count = 0
          count += 1
          print(count)
        }
      `);
      // Each iteration starts with count=0, increments to 1
      expect(output).toBe("1\n1\n1");
    });

    it("state inside nested for-loops gets separate value per (outer, inner) pair", () => {
      const output = runPetal(`
        for i in [1, 2] {
          for j in ["a", "b"] {
            state count = 0
            count += 1
            print(i, j, count)
          }
        }
      `);
      expect(output).toBe("1 a 1\n1 b 1\n2 a 1\n2 b 1");
    });

    it("state accumulates within a single iteration", () => {
      const output = runPetal(`
        for item in [1, 2, 3] {
          state total = 0
          total += item
          total += item
          print(total)
        }
      `);
      // Each iteration: total starts at 0, adds item twice
      expect(output).toBe("2\n4\n6");
    });
  });

  describe("while loops", () => {
    it("state inside while-loop gets separate value per iteration", () => {
      const output = runPetal(`
        let i = 0
        while i < 3 {
          state x = 100
          x += 1
          print(x)
          i += 1
        }
      `);
      // Each iteration starts with x=100, increments to 101
      expect(output).toBe("101\n101\n101");
    });
  });

  describe("top-level state (not in loop)", () => {
    it("still works as before — shared across re-reads", () => {
      const output = runPetal(`
        state counter = 0
        counter += 5
        print(counter)
        counter += 10
        print(counter)
      `);
      expect(output).toBe("5\n15");
    });
  });

  describe("IR properties", () => {
    it("state inside for-loop has in_loop flag set", () => {
      const ir = showIrJson(`
        for x in [1, 2] {
          state count = 0
        }
      `);
      const inits = termsByOp(ir, "StateInit");
      expect(inits.length).toBeGreaterThanOrEqual(1);
      expect(inits[0].in_loop).toBe(true);
    });

    it("top-level state does not have in_loop flag", () => {
      const ir = showIrJson("state count = 0");
      const inits = termsByOp(ir, "StateInit");
      expect(inits[0].in_loop).toBeUndefined(); // skipped when false
    });

    it("StateWrite inside loop inherits in_loop from StateInit", () => {
      const ir = showIrJson(`
        for x in [1, 2] {
          state count = 0
          count += 1
        }
      `);
      const writes = termsByOp(ir, "StateWrite");
      expect(writes.length).toBeGreaterThanOrEqual(1);
      expect(writes[0].in_loop).toBe(true);
    });
  });

  describe("explicit key: state(expr)", () => {
    it("state(key) uses explicit key instead of iteration index", () => {
      const output = runPetal(`
        let items = [{id: 1, name: "a"}, {id: 2, name: "b"}, {id: 1, name: "a2"}]
        for item in items {
          state(item.id) clicks = 0
          clicks += 1
          print(item.name, clicks)
        }
      `);
      // id=1 appears twice, so second occurrence has clicks=2
      expect(output).toBe("a 1\nb 1\na2 2");
    });

    it("explicit key survives list reordering", () => {
      const output = runPetal(`
        let items = [{id: "x"}, {id: "y"}]
        for item in items {
          state(item.id) count = 0
          count += 1
        }
        // Reversed order
        let items2 = [{id: "y"}, {id: "x"}]
        for item in items2 {
          state(item.id) count = 0
          count += 1
          print(item.id, count)
        }
      `);
      // Both were incremented once in first loop, now get incremented again
      expect(output).toBe("y 2\nx 2");
    });

    it("StateInit with explicit key has 2 inputs in IR", () => {
      const ir = showIrJson(`
        for x in [1] {
          state(x) count = 0
        }
      `);
      const inits = termsByOp(ir, "StateInit");
      expect(inits.length).toBeGreaterThanOrEqual(1);
      expect(inits[0].inputs).toHaveLength(2);
    });
  });
});
