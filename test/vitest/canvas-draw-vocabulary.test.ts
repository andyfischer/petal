/**
 * The web Canvas2D renderer's half of the extended draw vocabulary: gradients,
 * box-shadows and the nesting clip stack.
 *
 * These ops reach the renderer as plain `{op, ...}` records (petal-ui's
 * `DrawCommand`, `#[serde(tag = "op")]`), and a `switch` with no arm for one of
 * them drops it **silently** — the failure mode this file exists to catch. The
 * context is stubbed rather than real (no DOM, no jsdom): what is asserted is
 * the sequence of Canvas2D calls, which is where the geometry lives.
 */
import { describe, expect, it } from "vitest";
import {
  renderCommands,
  type DrawCommand,
} from "../../integrations/petal-web-canvas/src/canvas-renderer.js";

interface Gradient {
  kind: "linear" | "radial";
  coords: number[];
  stops: Array<[number, string]>;
}

/** A Canvas2D stub that records every call the new ops make. */
function makeCtx() {
  const log: string[] = [];
  const gradients: Gradient[] = [];
  const ctx: any = {
    canvas: { width: 200, height: 200 },
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 0,
    shadowColor: "",
    shadowBlur: 0,
    font: "",
    textBaseline: "",
    // What the assertions read:
    log,
    gradients,
    paths: [] as Array<Record<string, unknown>>,
    fills: [] as Array<{ style: unknown; shadowColor: string; shadowBlur: number }>,
    fillRects: [] as Array<[number, number, number, number, unknown]>,
    saves: 0,
    restores: 0,
    clips: 0,

    beginPath() {},
    moveTo() {},
    lineTo() {},
    closePath() {},
    arc(cx: number, cy: number, r: number) {
      this.paths.push({ shape: "arc", cx, cy, r });
    },
    rect(x: number, y: number, w: number, h: number) {
      this.paths.push({ shape: "rect", x, y, w, h });
    },
    roundRect(x: number, y: number, w: number, h: number, r: number) {
      this.paths.push({ shape: "roundRect", x, y, w, h, r });
    },
    fill() {
      this.fills.push({
        style: this.fillStyle,
        shadowColor: this.shadowColor,
        shadowBlur: this.shadowBlur,
      });
      log.push("fill");
    },
    fillRect(x: number, y: number, w: number, h: number) {
      this.fillRects.push([x, y, w, h, this.fillStyle]);
      log.push("fillRect");
    },
    stroke() {},
    strokeRect() {},
    fillText() {},
    save() {
      this.saves++;
      log.push("save");
    },
    restore() {
      this.restores++;
      log.push("restore");
    },
    clip() {
      this.clips++;
      log.push("clip");
    },
    createLinearGradient(x0: number, y0: number, x1: number, y1: number) {
      const g: Gradient = { kind: "linear", coords: [x0, y0, x1, y1], stops: [] };
      gradients.push(g);
      return { addColorStop: (o: number, c: string) => g.stops.push([o, c]) };
    },
    createRadialGradient(
      x0: number, y0: number, r0: number, x1: number, y1: number, r1: number,
    ) {
      const g: Gradient = { kind: "radial", coords: [x0, y0, r0, x1, y1, r1], stops: [] };
      gradients.push(g);
      return { addColorStop: (o: number, c: string) => g.stops.push([o, c]) };
    },
  };
  return ctx;
}

function render(commands: DrawCommand[]) {
  const ctx = makeCtx();
  renderCommands(ctx, commands, 200, 200);
  return ctx;
}

describe("web canvas — gradients", () => {
  it("lays a rect gradient along the CSS axis through the box centre", () => {
    // Angle 0 runs left → right, so the axis spans the box's full width at
    // its vertical centre.
    const ctx = render([
      {
        op: "rect_gradient",
        x: 20, y: 40, w: 100, h: 60,
        r0: 255, g0: 0, b0: 0,
        r1: 0, g1: 0, b1: 255, a1: 128,
        angle: 0,
      },
    ]);
    expect(ctx.gradients.length).toBe(1);
    const g = ctx.gradients[0];
    expect(g.kind).toBe("linear");
    expect(g.coords).toEqual([20, 70, 120, 70]);
    expect(g.stops).toEqual([
      [0, "rgb(255,0,0)"],
      [1, "rgba(0,0,255,0.5019607843137255)"],
    ]);
    // Square corners fill as a rect; the gradient object is the fill style.
    expect(ctx.fillRects.length).toBe(1);
    expect(ctx.fillRects[0].slice(0, 4)).toEqual([20, 40, 100, 60]);
  });

  it("rounds a gradient's corners through the rounded-rect path", () => {
    const ctx = render([
      {
        op: "rect_gradient",
        x: 0, y: 0, w: 40, h: 40, radius: 12,
        r0: 1, g0: 2, b0: 3, r1: 4, g1: 5, b1: 6,
        angle: Math.PI / 2,
      },
    ]);
    expect(ctx.fillRects).toEqual([]);
    expect(ctx.paths).toEqual([{ shape: "roundRect", x: 0, y: 0, w: 40, h: 40, r: 12 }]);
    // PI/2 runs top → bottom (screen y grows downward).
    expect(ctx.gradients[0].coords.map(Math.round)).toEqual([20, 0, 20, 40]);
  });

  it("draws a circle gradient as a disc from centre to rim", () => {
    const ctx = render([
      {
        op: "circle_gradient",
        cx: 50, cy: 60, radius: 30,
        r0: 255, g0: 255, b0: 255,
        r1: 0, g1: 0, b1: 0, a1: 0,
      },
    ]);
    expect(ctx.gradients[0].kind).toBe("radial");
    expect(ctx.gradients[0].coords).toEqual([50, 60, 0, 50, 60, 30]);
    expect(ctx.paths).toEqual([{ shape: "arc", cx: 50, cy: 60, r: 30 }]);
  });
});

describe("web canvas — shadow", () => {
  it("puts the offset and spread in the path and the blur in shadowBlur", () => {
    const ctx = render([
      {
        op: "shadow",
        x: 100, y: 100, w: 50, h: 40, radius: 8,
        blur: 24, spread: 2, dx: 0, dy: 6,
        r: 0, g: 0, b: 0, a: 128,
      },
    ]);
    // (x+dx-spread, y+dy-spread, w+2s, h+2s), radius grown by the spread.
    expect(ctx.paths).toEqual([
      { shape: "roundRect", x: 98, y: 104, w: 54, h: 44, r: 10 },
    ]);
    expect(ctx.fills.length).toBe(1);
    expect(ctx.fills[0].shadowBlur).toBe(24);
    expect(ctx.fills[0].shadowColor).toBe("rgba(0,0,0,0.5019607843137255)");
    // …and the shadow state is scoped: nothing after it inherits the blur.
    expect(ctx.saves).toBe(1);
    expect(ctx.restores).toBe(1);
  });

  it("skips a shadow shrunk out of existence by a negative spread", () => {
    const ctx = render([
      {
        op: "shadow",
        x: 0, y: 0, w: 10, h: 10, blur: 4, spread: -6,
        r: 0, g: 0, b: 0,
      },
    ]);
    expect(ctx.fills).toEqual([]);
    expect(ctx.paths).toEqual([]);
  });
});

describe("web canvas — the clip stack", () => {
  it("nests clip_push inside the enclosing clip and gives it back on pop", () => {
    const ctx = render([
      { op: "clip_push", x: 0, y: 0, w: 100, h: 100 },
      { op: "clip_push", x: 10, y: 10, w: 20, h: 20, radius: 6 },
      { op: "rect", x: 0, y: 0, w: 5, h: 5, r: 0, g: 0, b: 0 },
      { op: "clip_pop" },
      { op: "rect", x: 0, y: 0, w: 5, h: 5, r: 0, g: 0, b: 0 },
      { op: "clip_pop" },
    ]);
    // Two pushes, two pops, and the frame ends with nothing left pushed.
    expect(ctx.saves).toBe(2);
    expect(ctx.clips).toBe(2);
    expect(ctx.restores).toBe(2);
    // The inner push is a rounded path, the outer a square one.
    expect(ctx.paths).toEqual([
      { shape: "rect", x: 0, y: 0, w: 100, h: 100 },
      { shape: "roundRect", x: 10, y: 10, w: 20, h: 20, r: 6 },
    ]);
  });

  it("treats an unmatched clip_pop as a no-op, not a stray restore", () => {
    const ctx = render([
      { op: "clip_pop" },
      { op: "clip_pop" },
      { op: "rect", x: 0, y: 0, w: 5, h: 5, r: 0, g: 0, b: 0 },
    ]);
    expect(ctx.restores).toBe(0);
    expect(ctx.fillRects.length).toBe(1);
  });

  it("releases a clip left pushed when the frame ends", () => {
    const ctx = render([{ op: "clip_push", x: 0, y: 0, w: 10, h: 10 }]);
    expect(ctx.saves).toBe(1);
    expect(ctx.restores).toBe(1);
  });

  it("`clip` replaces the whole stack, where `clip_push` nests", () => {
    const ctx = render([
      { op: "clip_push", x: 0, y: 0, w: 100, h: 100 },
      { op: "clip_push", x: 10, y: 10, w: 20, h: 20 },
      { op: "clip", x: 5, y: 5, w: 50, h: 50 },
    ]);
    // Both pushes are unwound before the replacement clip is applied…
    expect(ctx.log.slice(0, 6)).toEqual([
      "save", "clip", "save", "clip", "restore", "restore",
    ]);
    // …and the replacement is itself released at end of frame.
    expect(ctx.saves).toBe(3);
    expect(ctx.restores).toBe(3);
  });
});
