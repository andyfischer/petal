// Call-path keyed `state` (docs/dev/state-callsite-keying-plan.md, Phase 2),
// observed end to end through the CLI.
//
// A slot is `(declaration id, path)` — the chain of callsites and loop
// iterations that reached the declaration. The Rust suite
// (`rust/tests/state_call_paths.rs`) pins the key *shapes* and the
// across-runs/hot-reload behaviour that needs an `Env`; these are the
// single-run behaviours a script author actually sees, plus the IR fields the
// scheme rests on.

import { describe, it, expect } from "vitest";
import { runPetal, runIr, showIrJson, showIrJsonRaw, termsByOp } from "./helpers";

describe("per-callsite state", () => {
  it("gives each callsite of a helper its own counter", () => {
    // The headline change: under name keying this printed 1, 2, 3.
    const out = runPetal(`
      fn counter()
        state n = 0
        n += 1
        n
      end
      print(counter())
      print(counter())
      print(counter())
    `);
    expect(out).toBe("1\n1\n1");
  });

  it("no longer lets an accessor function launder one shared slot", () => {
    // The negative test for the idiom the preludes used to use: wrap the
    // declaration in a single function so "there is exactly one `state`". The
    // writer and the reader are two callsites, so they are two slots.
    const out = runPetal(`
      fn slot(writing, v)
        state cell = 0
        if writing then
          cell = v
        end
        cell
      end
      slot(true, 42)
      print(slot(false, 0))
    `);
    expect(out).toBe("0");
  });

  it("shares one cell when the declaration is hoisted to the top level", () => {
    // §2.4's migration idiom, the replacement for the accessor pattern above.
    const out = runPetal(`
      state var shared = 0
      fn put(v)
        set shared = v
      end
      fn look()
        get shared
      end
      put(42)
      print(look())
    `);
    expect(out).toBe("42");
  });

  it("gives each recursion depth its own slot", () => {
    const out = runPetal(`
      fn down(n)
        state hits = 0
        hits += 1
        if n > 0 then
          down(n - 1)
        end
        print(n, hits)
      end
      down(2)
    `);
    expect(out).toBe("0 1\n1 1\n2 1");
  });

  it("gives a widget called inside a for loop per-iteration slots", () => {
    const out = runPetal(`
      fn widget(label)
        state seen = 0
        seen += 1
        seen
      end
      for x in ["a", "b", "c"] do
        print(x, widget(x))
      end
    `);
    expect(out).toBe("a 1\nb 1\nc 1");
  });

  it("gives a widget called inside a while loop per-iteration slots", () => {
    const out = runPetal(`
      fn widget()
        state seen = 0
        seen += 1
        seen
      end
      var i = 0
      while i < 3 do
        print(widget())
        set i = i + 1
      end
    `);
    expect(out).toBe("1\n1\n1");
  });

  it("gives per-iteration slots to a loop used as an expression", () => {
    // A `for` in value position still pushes an `Index` part per iteration
    // while it collects, so the widget it calls is keyed per row.
    const out = runPetal(`
      fn widget(x)
        state n = 0
        n += 1
        str(x) ++ ":" ++ str(n)
      end
      print(for x in [1, 2, 3] do
        widget(x)
      end)
    `);
    expect(out).toBe(`["1:1", "2:1", "3:1"]`);
  });

  it("keeps an explicit key absolute across callsites and loops", () => {
    // Same key ⇒ same slot however deep the caller is: the plant.ptl lineage /
    // nes.ptl btn_repeat contract (§2.2).
    const out = runPetal(`
      fn bump(id)
        state(id) n = 0
        n += 1
        n
      end
      fn indirect(id)
        bump(id)
      end
      print(bump("x"))
      print(indirect("x"))
      for i in range(0, 2) do
        print(bump("x"))
      end
    `);
    expect(out).toBe("1\n2\n3\n4");
  });
});

describe("top-level state is untouched by the path (§2.3)", () => {
  it("accumulates into one slot when the write sits inside a loop", () => {
    const out = runPetal(`
      state items = []
      for i in range(0, 3) do
        items = append(items, i)
      end
      print(items)
    `);
    expect(out).toBe("[0, 1, 2]");
  });

  it("accumulates through two levels of loop nesting", () => {
    const out = runPetal(`
      state total = 0
      for i in range(0, 2) do
        for j in range(0, 3) do
          total = total + 1
        end
      end
      print(total)
    `);
    expect(out).toBe("6");
  });

  it("accumulates across a `while` loop's iterations", () => {
    // `while` pushes an `Index` part per iteration like any other loop, so the
    // accumulator has to pop back out of it exactly the same way. (§9's open
    // question about while-loop state: this is the answer.)
    const out = runPetal(`
      state total = 0
      var i = 0
      while i < 4 do
        total = total + i
        set i = i + 1
      end
      print(total)
    `);
    expect(out).toBe("6");
  });

  it("accumulates across a loop that a `continue` skips through", () => {
    const out = runPetal(`
      state total = 0
      for i in range(0, 5) do
        if i == 3 then
          continue
        end
        total = total + i
      end
      print(total)
    `);
    expect(out).toBe("7");
  });

  it("still sees the accumulated value after the loop ends", () => {
    // The read after the loop addresses the declaration's slot, which is where
    // the in-loop writes committed.
    const out = runPetal(`
      state seen = 0
      for i in range(0, 4) do
        seen = seen + i
      end
      print(seen)
    `);
    expect(out).toBe("6");
  });
});

describe("call_site on the IR", () => {
  it("is name-derived, so an edit above a call does not move it", () => {
    const before = showIrJson(`
      fn f()
        state n = 0
      end
      f()
    `);
    const after = showIrJson(`
      fn f()
        state n = 0
      end
      let unrelated = 1 + 1
      f()
    `);
    expect(termsByOp(before, "Call")[0].call_site).toBe(
      termsByOp(after, "Call")[0].call_site,
    );
  });

  it("changes when an earlier call to the same callee is inserted", () => {
    // The accepted loss of §3.1: the ordinal among identically-spelled callees
    // shifts, so the surviving call gets a new id (and a fresh slot on reload).
    const before = termsByOp(
      showIrJson(`
        fn f()
          state n = 0
        end
        f()
      `),
      "Call",
    );
    const after = termsByOp(
      showIrJson(`
        fn f()
          state n = 0
        end
        f()
        f()
      `),
      "Call",
    );
    expect(after).toHaveLength(2);
    expect(after[0].call_site).toBe(before[0].call_site);
    expect(after[1].call_site).not.toBe(before[0].call_site);
  });

  it("distinguishes calls of the same callee inside one function", () => {
    const ir = showIrJson(`
      fn leaf()
        state n = 0
      end
      fn twice()
        leaf()
        leaf()
      end
      twice()
    `);
    const sites = termsByOp(ir, "Call").map((c: any) => c.call_site);
    expect(new Set(sites).size).toBe(sites.length);
  });

  it("round-trips through the IR so a `run --ir` keeps per-callsite slots", () => {
    const src = `
      fn counter()
        state n = 0
        n += 1
        n
      end
      print(counter())
      print(counter())
    `;
    expect(runIr(showIrJsonRaw(src))).toBe(runPetal(src));
    expect(runIr(showIrJsonRaw(src))).toBe("1\n1");
  });
});
