/**
 * Every petal-ui draw command reaches the web Canvas2D renderer.
 *
 * The renderer is a `switch` on `op`, and a command with no arm is dropped
 * silently: polylines, polygons, fans, ellipses, arcs and images all used to be.
 * The first test reads petal-ui's `DrawCommand` enum from its source and holds
 * the renderer to an arm for each, so a new command fails here rather than
 * vanishing in the browser. The rest check the geometry of the arms that were
 * added, on a Canvas2D stub that records calls (no DOM).
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  renderCommands,
  type DrawCommand,
} from "../../integrations/petal-web-canvas/src/canvas-renderer.js";

const root = fileURLToPath(new URL("../../", import.meta.url));

/** Commands the renderer never sees: petal-ui's drain lowers the batching and
 * caching ones (`compose.rs`) for a host that has not declared them in
 * `set_host_caps`, removes `mute` spans, and `host` is an embedder's private
 * extension, which this renderer has none of. */
const NEVER_REACH_THE_RENDERER = new Set([
  "rects",
  "layer",
  "cache_begin",
  "cache_end",
  "cache_draw",
  "mute",
  "host",
]);

function drawCommandOps(): string[] {
  const src = readFileSync(`${root}core-libs/petal-ui/src/draw.rs`, "utf8");
  const body = src.slice(src.indexOf("pub enum DrawCommand {"));
  const end = body.indexOf("\n}\n");
  return [...body.slice(0, end).matchAll(/^ {4}([A-Z][A-Za-z]*)\b/gm)].map((m) =>
    m[1].replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase(),
  );
}

describe("web canvas — draw vocabulary coverage", () => {
  it("has an arm for every DrawCommand that reaches a host", () => {
    const ops = drawCommandOps();
    expect(ops).toContain("rect");
    expect(ops).toContain("ellipse_outline");
    const renderer = readFileSync(
      `${root}integrations/petal-web-canvas/src/canvas-renderer.ts`,
      "utf8",
    );
    const handled = new Set([...renderer.matchAll(/case "([a-z_]+)"/g)].map((m) => m[1]));
    const missing = ops.filter((op) => !NEVER_REACH_THE_RENDERER.has(op) && !handled.has(op));
    expect(missing).toEqual([]);
  });
});

/** A Canvas2D stub that logs each call as `name(args)` and each property set
 * as `name=value`. */
function makeCtx() {
  const log: string[] = [];
  const target: Record<string, unknown> = { canvas: { width: 200, height: 200 }, globalAlpha: 1 };
  const ctx = new Proxy(target, {
    get(t, key: string) {
      if (key in t) return t[key];
      return (...args: unknown[]) => {
        log.push(`${key}(${args.map((a) => JSON.stringify(a)).join(",")})`);
      };
    },
    set(t, key: string, value) {
      t[key] = value;
      if (key !== "globalAlpha") log.push(`${key}=${JSON.stringify(value)}`);
      return true;
    },
  });
  return { ctx: ctx as unknown as CanvasRenderingContext2D, log };
}

function render(cmd: DrawCommand): string[] {
  const { ctx, log } = makeCtx();
  renderCommands(ctx, [cmd], 200, 200);
  return log;
}

const red = { r: 255, g: 0, b: 0 };

describe("web canvas — shapes", () => {
  it("strokes a polyline once, with round joins and caps", () => {
    const log = render({ op: "polyline", points: [[0, 0], [10, 0], [10, 10]], width: 4, ...red });
    expect(log.filter((l) => l === "stroke()")).toHaveLength(1);
    expect(log).toContain('lineJoin="round"');
    expect(log).toContain('lineCap="round"');
    expect(log).toContain("lineWidth=4");
    expect(log).toContain("lineTo(10,10)");
  });

  it("fills a polygon as one closed path", () => {
    const log = render({ op: "polygon", points: [[0, 0], [20, 0], [10, 5], [20, 20], [0, 20]], ...red });
    expect(log.filter((l) => l === "fill()")).toHaveLength(1);
    expect(log).toContain("closePath()");
  });

  it("runs a fan from its hub through the points and back", () => {
    const log = render({ op: "fan", cx: 50, cy: 50, points: [[60, 50], [50, 60], [40, 50]], ...red });
    const path = log.filter((l) => l.startsWith("moveTo") || l.startsWith("lineTo"));
    expect(path).toEqual(["moveTo(50,50)", "lineTo(60,50)", "lineTo(50,60)", "lineTo(40,50)"]);
    expect(log).toContain("fill()");
  });

  it("fills an ellipse, and strokes an outline inside its boundary", () => {
    expect(render({ op: "ellipse", cx: 10, cy: 20, rx: 8, ry: 4, ...red })).toContain(
      `ellipse(10,20,8,4,0,0,${Math.PI * 2})`,
    );
    const outline = render({ op: "ellipse_outline", cx: 10, cy: 20, rx: 8, ry: 6, width: 2, ...red });
    expect(outline).toContain(`ellipse(10,20,7,5,0,0,${Math.PI * 2})`);
    expect(outline).toContain("stroke()");
    // A stroke as thick as the smaller semi-axis fills solid.
    const thick = render({ op: "ellipse_outline", cx: 10, cy: 20, rx: 8, ry: 6, width: 6, ...red });
    expect(thick).toContain("fill()");
    expect(thick).not.toContain("stroke()");
  });

  it("draws an arc as an annular sector, or a pie slice when r_in is 0", () => {
    const ring = render({ op: "arc", cx: 0, cy: 0, r_in: 5, r_out: 10, a0: 0, a1: 1, ...red });
    expect(ring).toContain("arc(0,0,10,0,1,false)");
    expect(ring).toContain("arc(0,0,5,1,0,true)");
    const pie = render({ op: "arc", cx: 0, cy: 0, r_in: 0, r_out: 10, a0: 0, a1: 1, ...red });
    expect(pie).toContain("lineTo(0,0)");
    // A sweep past a full turn is one ring, not an overdraw.
    const over = render({ op: "arc", cx: 0, cy: 0, r_in: 0, r_out: 10, a0: 0, a1: 10, ...red });
    expect(over).toContain(`arc(0,0,10,0,${Math.PI * 2},false)`);
    expect(render({ op: "arc", cx: 0, cy: 0, r_in: 10, r_out: 5, a0: 0, a1: 1, ...red })).toEqual([]);
  });

  it("skips an image that has not loaded rather than throwing", () => {
    expect(render({ op: "image", source: "missing.png", x: 0, y: 0, w: 10, h: 10 })).toEqual([]);
  });
});
