/** Render petal-ui draw commands to a Canvas2D context.
 *
 * The WASM runtime serializes petal-ui's `DrawCommand` enum directly, so each
 * command already arrives in `{ op, ...fields }` form — no decoding step. Alpha
 * (`a`), corner radius (`radius`), and stroke width (`width`) are optional and
 * omitted from the JSON when at their defaults (opaque / square / hairline). */

export interface DrawCommand {
  op: string;
  // Color (shared)
  r?: number;
  g?: number;
  b?: number;
  /** Opacity 0–255; absent = 255 (opaque). */
  a?: number;
  // Rect / RectOutline / Clip
  x?: number;
  y?: number;
  w?: number;
  h?: number;
  /** Rect corner radius (px); RectOutline/Line stroke width lives in `width`. */
  radius?: number;
  width?: number;
  // Line / Triangle
  x1?: number;
  y1?: number;
  x2?: number;
  y2?: number;
  x3?: number;
  y3?: number;
  // Poly — serialized as [[x, y], ...]
  points?: number[][];
  // Circle
  cx?: number;
  cy?: number;
  // Text
  text?: string;
  size?: number;
  // Offscreen canvas (create_canvas / set_target / draw_canvas)
  id?: number;
}

function fillStyle(cmd: DrawCommand): string {
  const a = cmd.a ?? 255;
  if (a >= 255) return `rgb(${cmd.r},${cmd.g},${cmd.b})`;
  return `rgba(${cmd.r},${cmd.g},${cmd.b},${a / 255})`;
}

/** Trace a rounded-rectangle path (falls back to a plain rect when radius 0). */
function roundRectPath(
  ctx: CanvasRenderingContext2D,
  x: number, y: number, w: number, h: number, radius: number,
): void {
  const rr = Math.min(radius, w / 2, h / 2);
  ctx.beginPath();
  if (rr <= 0) {
    ctx.rect(x, y, w, h);
  } else {
    ctx.roundRect(x, y, w, h, rr);
  }
}

/** Draw a single primitive command into a 2D context (the active target). */
function renderPrimitive(
  ctx: CanvasRenderingContext2D,
  cmd: DrawCommand,
  width: number,
  height: number,
): void {
  switch (cmd.op) {
    case "clear":
      // clear ignores alpha — it repaints the whole target opaque.
      ctx.fillStyle = `rgb(${cmd.r},${cmd.g},${cmd.b})`;
      ctx.fillRect(0, 0, width, height);
      break;

    case "rect":
      ctx.fillStyle = fillStyle(cmd);
      if (cmd.radius && cmd.radius > 0) {
        roundRectPath(ctx, cmd.x!, cmd.y!, cmd.w!, cmd.h!, cmd.radius);
        ctx.fill();
      } else {
        ctx.fillRect(cmd.x!, cmd.y!, cmd.w!, cmd.h!);
      }
      break;

    case "rect_outline":
      ctx.strokeStyle = fillStyle(cmd);
      ctx.lineWidth = cmd.width ?? 1;
      if (cmd.radius && cmd.radius > 0) {
        roundRectPath(ctx, cmd.x!, cmd.y!, cmd.w!, cmd.h!, cmd.radius);
        ctx.stroke();
      } else {
        ctx.strokeRect(cmd.x!, cmd.y!, cmd.w!, cmd.h!);
      }
      break;

    case "line":
      ctx.strokeStyle = fillStyle(cmd);
      ctx.lineWidth = cmd.width ?? 1;
      ctx.beginPath();
      ctx.moveTo(cmd.x1!, cmd.y1!);
      ctx.lineTo(cmd.x2!, cmd.y2!);
      ctx.stroke();
      break;

    case "circle":
      ctx.fillStyle = fillStyle(cmd);
      ctx.beginPath();
      ctx.arc(cmd.cx!, cmd.cy!, Math.abs(cmd.radius!), 0, Math.PI * 2);
      ctx.fill();
      break;

    case "triangle":
      ctx.fillStyle = fillStyle(cmd);
      ctx.beginPath();
      ctx.moveTo(cmd.x1!, cmd.y1!);
      ctx.lineTo(cmd.x2!, cmd.y2!);
      ctx.lineTo(cmd.x3!, cmd.y3!);
      ctx.closePath();
      ctx.fill();
      break;

    case "poly": {
      const points = cmd.points!;
      if (points.length >= 3) {
        ctx.fillStyle = fillStyle(cmd);
        ctx.beginPath();
        ctx.moveTo(points[0][0], points[0][1]);
        for (let i = 1; i < points.length; i++) {
          ctx.lineTo(points[i][0], points[i][1]);
        }
        ctx.closePath();
        ctx.fill();
      }
      break;
    }

    case "text":
      ctx.fillStyle = fillStyle(cmd);
      ctx.font = `${cmd.size}px sans-serif`;
      ctx.textBaseline = "top";
      ctx.fillText(cmd.text!, cmd.x!, cmd.y!);
      break;
  }
}

/** Create an offscreen 2D rendering context of the given size. Uses the
 * standalone canvas element so it works in any browser; the context starts
 * fully transparent so only drawn pixels composite onto the destination. */
function createOffscreen(w: number, h: number): CanvasRenderingContext2D {
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, w);
  canvas.height = Math.max(1, h);
  return canvas.getContext("2d")!;
}

export function renderCommands(
  ctx: CanvasRenderingContext2D,
  commands: DrawCommand[],
  canvasWidth: number,
  canvasHeight: number,
): void {
  // The main canvas persists between frames: we only paint over it on an
  // explicit "clear" command. A sketch that never calls clear() therefore
  // accumulates its drawing (particle trails, attractors), matching petal-sdl's
  // persistent framebuffer. Game-style sketches clear() at the top of every
  // frame.
  //
  // Offscreen canvases (PGraphics-style render targets) are rebuilt fresh from
  // the command stream each frame, so the per-frame re-run model needs no extra
  // bookkeeping. `draw_to(id)` switches the active target; `draw_canvas(id,x,y)`
  // composites a finished offscreen canvas onto the current target.
  const offscreen = new Map<number, CanvasRenderingContext2D>();
  // The active target. `0` is the main canvas; any other value is an offscreen
  // canvas id.
  let target = 0;
  // Whether the active target currently has a clip pushed (needs a restore).
  let clipped = false;

  const targetCtx = (): CanvasRenderingContext2D | null => {
    if (target === 0) return ctx;
    return offscreen.get(target) ?? null;
  };
  const targetSize = (): [number, number] => {
    if (target === 0) return [canvasWidth, canvasHeight];
    const t = offscreen.get(target);
    return t ? [t.canvas.width, t.canvas.height] : [0, 0];
  };
  const clearClip = (): void => {
    const dst = targetCtx();
    if (clipped && dst) {
      dst.restore();
      clipped = false;
    }
  };

  for (const cmd of commands) {
    switch (cmd.op) {
      case "create_canvas":
        offscreen.set(cmd.id!, createOffscreen(cmd.w!, cmd.h!));
        break;

      case "set_target":
        clearClip();
        target = cmd.id!;
        break;

      case "clip": {
        const dst = targetCtx();
        if (dst) {
          clearClip();
          dst.save();
          dst.beginPath();
          dst.rect(cmd.x!, cmd.y!, cmd.w!, cmd.h!);
          dst.clip();
          clipped = true;
        }
        break;
      }

      case "clip_none":
        clearClip();
        break;

      case "draw_canvas": {
        const src = offscreen.get(cmd.id!);
        const dst = targetCtx();
        if (src && dst) {
          dst.drawImage(src.canvas, cmd.x!, cmd.y!);
        }
        break;
      }

      default: {
        const dst = targetCtx();
        if (dst) {
          const [w, h] = targetSize();
          renderPrimitive(dst, cmd, w, h);
        }
        break;
      }
    }
  }
  clearClip();
}
