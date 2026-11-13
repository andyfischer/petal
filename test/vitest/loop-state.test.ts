import { describe, it, expect } from "vitest";
import {
  showIrJson,
  runPetal,
  termsByOp,
} from "./helpers";


describe("per-iteration loop state", () => {
  describe("for loops", () => {
    it("state inside for-loop gets separate value per iteration", () => {
      const output = runPetal(`
        for item in [10, 20, 30] do
          state count = 0
          count += 1
          print(count)
        end
      `);
      // Each iteration starts with count=0, increments to 1
      expect(output).toBe("1\n1\n1");
    });

    it("state inside nested for-loops gets separate value per (outer, inner) pair", () => {
      const output = runPetal(`
        for i in [1, 2] do
          for j in ["a", "b"] do
            state count = 0
            count += 1
            print(i, j, count)
          end
        end
      `);
      expect(output).toBe("1 a 1\n1 b 1\n2 a 1\n2 b 1");
    });

    it("state accumulates within a single iteration", () => {
      const output = runPetal(`
        for item in [1, 2, 3] do
          state total = 0
          total += item
          total += item
          print(total)
        end
      `);
      // Each iteration: total starts at 0, adds item twice
      expect(output).toBe("2\n4\n6");
    });
  });

  describe("while loops", () => {
    it("state inside while-loop gets separate value per iteration", () => {
      const output = runPetal(`
        let i = 0
        while i < 3 do
          state x = 100
          x += 1
          print(x)
          i += 1
        end
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
    // Loop nesting is no longer a flag on the state term: every loop pushes an
    // Index part onto the running frame's path, and the slot is that path. What
    // the IR still has to carry is `path_pop` — how many loop levels a write
    // sits below its declaration, so it commits to the declaration's slot.
    it("a write at the declaration's own loop level pops nothing", () => {
      const ir = showIrJson(`
        for x in [1, 2] do
          state count = 0
          count += 1
        end
      `);
      const writes = termsByOp(ir, "StateWrite");
      expect(writes.length).toBeGreaterThanOrEqual(1);
      expect(writes[0].path_pop).toBeUndefined(); // skipped when zero
    });

    it("a write to a top-level state inside a loop pops that loop", () => {
      const ir = showIrJson(`
        state total = 0
        for x in [1, 2] do
          total += x
        end
      `);
      const writes = termsByOp(ir, "StateWrite").filter(
        (w: any) => w.path_pop !== undefined,
      );
      expect(writes.length).toBe(1);
      expect(writes[0].path_pop).toBe(1);
    });

    it("a call term carries a stable callsite id", () => {
      const ir = showIrJson(`
        fn f()
          state n = 0
        end
        f()
        f()
      `);
      const calls = termsByOp(ir, "Call");
      expect(calls.length).toBe(2);
      // Both callsites are named, and the two are distinct — that is what
      // gives each its own slot.
      expect(typeof calls[0].call_site).toBe("number");
      expect(calls[0].call_site).not.toBe(calls[1].call_site);
    });
  });

  describe("memo(key)", () => {
    it("a memo's StateInit has 1 input (the key) and a child init block", () => {
      // Lazy init: the init value lives in StateInit.child_blocks[0]; only
      // the key value is an \`inputs\` (it's evaluated eagerly each visit so
      // the runtime can decide whether to enter the init block).
      const ir = showIrJson(`
        for x in [1] do
          memo(x) doubled = x * 2
        end
      `);
      const inits = termsByOp(ir, "StateInit");
      expect(inits.length).toBeGreaterThanOrEqual(1);
      expect(inits[0].inputs).toHaveLength(1);
      expect(inits[0].child_blocks).toHaveLength(1);
    });
  });
});
