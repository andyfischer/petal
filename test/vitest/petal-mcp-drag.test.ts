// The dev MCP server's DragExplain tool (tools/petal-mcp.ts), through its
// engine in tools/petal-mcp-drag.ts: a ui/drag app is run headlessly by
// petal-ui-run with a drag driven into it, and the engine's spec, target
// table and chosen branch come back as data.

import { beforeAll, describe, expect, it } from "vitest";
import { dragExplain, dragScenario, ensureUiRun, parseTrace } from "../../tools/petal-mcp-drag.ts";

const LIST = `import ui
import ui/drag
state items = ["A", "B", "C", "D", "E"]
fn view(items)
  for i in range(0, len(items)) key items[i] do
    let k = items[i]
    let r = place(k, rect(40, 60 + i * 52, 240, 44))
    draggable(k, r, fn() closest(moves(items, i)) |> floating() end)
    place(k, r, fn(p) draw_rect(p, #446688) end)
  end
  nil
end
let d = drag_session(items, view)
view(d.preview)
if d.dropped then items = d.drop end
`;

describe("DragExplain (MCP)", () => {
  beforeAll(async () => {
    const err = await ensureUiRun();
    if (err) throw new Error(err);
  }, 600_000);

  it("explains the last frame of a drag", async () => {
    const out = await dragExplain({ code: LIST, drag: { target: "A", to: "D", frames: 6 }, size: [320, 340] });
    expect(out.error).toBeNull();
    expect(out.sessions).toHaveLength(1);
    const s: any = out.sessions[0];
    expect(s.key).toBe("A");
    expect(s.choice).toBe(4);
    expect(s.branch).toEqual(["floating", "closest", "fixed #4"]);
    expect(s.spec.d2).toBe("floating");
    expect(s.targets).toHaveLength(5);
    expect(s.targets[3]).toMatchObject({ id: 4, kind: "fixed", active: true, drop: ["B", "C", "D", "A", "E"] });
    expect(s.targets[0].at).toEqual({ x: 40, y: 60, w: 240, h: 44 });
    // Every frame of the gesture, and the drop it ended in.
    expect(out.timeline.map(([, c]) => c).at(-1)).toBe(4);
    expect(out.all_drops).toEqual([{ frame: 9, key: "A", drop: ["B", "C", "D", "A", "E"] }]);
    expect(out.final_state).toEqual({ items: ["B", "C", "D", "A", "E"] });
  }, 60_000);

  it("reports a chosen frame and a scenario's own events", async () => {
    const out = await dragExplain({
      code: LIST,
      size: [320, 340],
      scenario: { frames: 12, events: [{ at: 2, drag: { target: "A", to: [160, 135], frames: 4 } }] },
      frame: 3,
    });
    expect(out.frame).toBe(3);
    expect((out.sessions[0] as any).choice).toBe(1);
  }, 60_000);

  it("says what is wrong with the input", async () => {
    await expect(dragExplain({ code: LIST })).rejects.toThrow(/drag/);
    const out = await dragExplain({ code: LIST, drag: { target: "nope", to: [0, 0] } });
    expect(out.error).toMatch(/no ui target named "nope"/);
  }, 60_000);

  it("builds the scenario and reads a trace", () => {
    expect(dragScenario({ drag: { target: "A", to: "B" } })).toEqual({
      events: [{ at: 2, drag: { target: "A", to: "B", frames: 10 } }],
      frames: 16,
    });
    const t = parseTrace('{"frame":0,"drag":null,"state":{"x":1,"ui/drag::_anim":{}},"error":null}\n');
    expect(t).toMatchObject({ frame: null, sessions: [], final_state: { x: 1 }, error: null });
  });
});
