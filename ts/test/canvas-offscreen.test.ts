/**
 * Unit test for the web Canvas2D offscreen-canvas (PGraphics-style) renderer.
 *
 * The web canvas integration and the diagram-canvas sample app share an
 * identical `canvas-renderer.ts`. This test drives that renderer with a stubbed
 * Canvas2D API (no DOM/jsdom needed) and asserts that the offscreen-canvas
 * commands — create_canvas / set_target (draw_to) / draw_canvas — route drawing
 * into the right target and composite correctly onto the main context.
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  renderCommands,
  type DrawCommand,
} from "../../integrations/petal-web-canvas/src/canvas-renderer.js";

/**
 * Build a draw command in the `{ op, ...fields }` shape the WASM runtime
 * serializes (petal-ui's `DrawCommand`, `#[serde(tag = "op")]`). The renderer
 * consumes these named fields directly — there is no decoding step.
 */
function cmd(op: string, fields: Partial<DrawCommand> = {}): DrawCommand {
  return { op, ...fields };
}

/** A minimal Canvas2D context stub that records the calls we care about. */
interface FakeCtx {
  canvas: { width: number; height: number };
  fillStyle: string;
  strokeStyle: string;
  lineWidth: number;
  font: string;
  textBaseline: string;
  // Recorded operations:
  fillRects: Array<[number, number, number, number, string]>;
  drawImages: Array<{ src: object; x: number; y: number }>;
  // Stubbed no-op methods used by other primitives:
  beginPath(): void;
  moveTo(): void;
  lineTo(): void;
  closePath(): void;
  arc(): void;
  fill(): void;
  stroke(): void;
  strokeRect(): void;
  fillText(): void;
  fillRect(x: number, y: number, w: number, h: number): void;
  drawImage(src: { canvas: object }, x: number, y: number): void;
}

function makeCtx(width: number, height: number): FakeCtx {
  const ctx: FakeCtx = {
    canvas: { width, height },
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 0,
    font: "",
    textBaseline: "",
    fillRects: [],
    drawImages: [],
    beginPath() {},
    moveTo() {},
    lineTo() {},
    closePath() {},
    arc() {},
    fill() {},
    stroke() {},
    strokeRect() {},
    fillText() {},
    fillRect(x, y, w, h) {
      this.fillRects.push([x, y, w, h, this.fillStyle]);
    },
    drawImage(src, x, y) {
      this.drawImages.push({ src, x, y });
    },
  };
  return ctx;
}

// Track offscreen contexts created via document.createElement('canvas').
let createdOffscreen: FakeCtx[] = [];

beforeEach(() => {
  createdOffscreen = [];
  // Stub the DOM bits the renderer touches when allocating offscreen canvases.
  (globalThis as any).document = {
    createElement(tag: string) {
      if (tag !== "canvas") throw new Error(`unexpected element ${tag}`);
      const el: any = { width: 1, height: 1 };
      el.getContext = () => {
        const c = makeCtx(el.width, el.height);
        c.canvas = el; // drawImage receives the element as the image source
        createdOffscreen.push(c);
        return c;
      };
      return el;
    },
  };
});

afterEach(() => {
  delete (globalThis as any).document;
});

describe("offscreen canvas renderer", () => {
  it("routes drawing into an offscreen canvas and composites it onto main", () => {
    const main = makeCtx(100, 100);

    const commands: DrawCommand[] = [
      cmd("create_canvas", { id: 1, w: 32, h: 32 }),
      cmd("set_target", { id: 1 }),
      cmd("rect", { x: 0, y: 0, w: 8, h: 8, r: 255, g: 255, b: 255 }),
      cmd("set_target", { id: 0 }),
      cmd("draw_canvas", { id: 1, x: 20, y: 20 }),
    ];

    renderCommands(main as any, commands, 100, 100);

    // One offscreen canvas was allocated, sized 32x32.
    expect(createdOffscreen.length).toBe(1);
    const off = createdOffscreen[0];
    expect(off.canvas.width).toBe(32);
    expect(off.canvas.height).toBe(32);

    // The rect was drawn into the OFFSCREEN canvas, not the main one.
    expect(off.fillRects).toEqual([[0, 0, 8, 8, "rgb(255,255,255)"]]);
    expect(main.fillRects).toEqual([]);

    // The offscreen canvas was composited onto main at (20, 20).
    expect(main.drawImages.length).toBe(1);
    expect(main.drawImages[0].x).toBe(20);
    expect(main.drawImages[0].y).toBe(20);
    expect(main.drawImages[0].src).toBe(off.canvas);
  });

  it("draws to the main canvas when no target redirect is active", () => {
    const main = makeCtx(100, 100);
    renderCommands(
      main as any,
      [cmd("rect", { x: 1, y: 2, w: 3, h: 4, r: 10, g: 20, b: 30 })],
      100,
      100,
    );
    expect(main.fillRects).toEqual([[1, 2, 3, 4, "rgb(10,20,30)"]]);
    expect(main.drawImages).toEqual([]);
  });
});
