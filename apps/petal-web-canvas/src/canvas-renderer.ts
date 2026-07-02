/** Render an array of draw-command JSON objects to a Canvas2D context. */

export interface DrawCommand {
  op: string;
  // Color fields (shared by all commands)
  r?: number;
  g?: number;
  b?: number;
  // Rect / RectOutline
  x?: number;
  y?: number;
  w?: number;
  h?: number;
  // Line
  x1?: number;
  y1?: number;
  x2?: number;
  y2?: number;
  // Triangle (also uses x1,y1,x2,y2 above)
  x3?: number;
  y3?: number;
  // Poly — decoded to [[x,y],...] from the buffered points list
  points?: number[][];
  // Circle
  cx?: number;
  cy?: number;
  radius?: number;
  // Text
  text?: string;
  size?: number;
  // Offscreen canvas (create_canvas / draw_to / draw_canvas)
  id?: number;
}

/** Raw command as emitted by the WASM runtime: a `Value::EnumVariant`,
 * serialized as `{ type: "enum", tag, data }` where `data` is the flat argument
 * list. `decodeCommand` maps this into the named-field `DrawCommand` above. */
export interface RawCommand {
  type: string;
  tag: string;
  data: any[];
}

/** Decode a point from the buffered list. Vec2 values serialize as
 * `{ type: "vec2", x, y }`; `[x, y]` lists serialize as `[x, y]`. */
function decodePoint(p: any): [number, number] {
  return Array.isArray(p) ? [p[0], p[1]] : [p.x, p.y];
}

/** Map a raw `{tag, data}` command to a named-field `DrawCommand`. Mirrors the
 * argument order emitted by the native draw functions (see lib.rs). */
function decodeCommand(raw: RawCommand): DrawCommand {
  const d = raw.data ?? [];
  switch (raw.tag) {
    case "clear":
      return { op: "clear", r: d[0], g: d[1], b: d[2] };
    case "rect":
      return { op: "rect", x: d[0], y: d[1], w: d[2], h: d[3], r: d[4], g: d[5], b: d[6] };
    case "rect_outline":
      return { op: "rect_outline", x: d[0], y: d[1], w: d[2], h: d[3], r: d[4], g: d[5], b: d[6] };
    case "line":
      return { op: "line", x1: d[0], y1: d[1], x2: d[2], y2: d[3], r: d[4], g: d[5], b: d[6] };
    case "circle":
      return { op: "circle", cx: d[0], cy: d[1], radius: d[2], r: d[3], g: d[4], b: d[5] };
    case "triangle":
      return { op: "triangle", x1: d[0], y1: d[1], x2: d[2], y2: d[3], x3: d[4], y3: d[5], r: d[6], g: d[7], b: d[8] };
    case "poly":
      return { op: "poly", points: (d[0] ?? []).map(decodePoint), r: d[1], g: d[2], b: d[3] };
    case "text":
      return { op: "text", text: d[0], x: d[1], y: d[2], size: d[3], r: d[4], g: d[5], b: d[6] };
    case "create_canvas":
      return { op: "create_canvas", id: d[0], w: d[1], h: d[2] };
    case "set_target":
      return { op: "set_target", id: d[0] };
    case "draw_canvas":
      return { op: "draw_canvas", id: d[0], x: d[1], y: d[2] };
    default:
      return { op: raw.tag };
  }
}

function rgb(r: number, g: number, b: number): string {
  return `rgb(${r},${g},${b})`;
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
      ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
      ctx.fillRect(0, 0, width, height);
      break;

    case "rect":
      ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
      ctx.fillRect(cmd.x!, cmd.y!, cmd.w!, cmd.h!);
      break;

    case "rect_outline":
      ctx.strokeStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
      ctx.lineWidth = 1;
      ctx.strokeRect(cmd.x!, cmd.y!, cmd.w!, cmd.h!);
      break;

    case "line":
      ctx.strokeStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(cmd.x1!, cmd.y1!);
      ctx.lineTo(cmd.x2!, cmd.y2!);
      ctx.stroke();
      break;

    case "circle":
      ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
      ctx.beginPath();
      ctx.arc(cmd.cx!, cmd.cy!, Math.abs(cmd.radius!), 0, Math.PI * 2);
      ctx.fill();
      break;

    case "triangle":
      ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
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
        ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
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
      ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
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
  rawCommands: RawCommand[],
  canvasWidth: number,
  canvasHeight: number,
): void {
  const commands = rawCommands.map(decodeCommand);
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

  const targetCtx = (): CanvasRenderingContext2D | null => {
    if (target === 0) return ctx;
    return offscreen.get(target) ?? null;
  };
  const targetSize = (): [number, number] => {
    if (target === 0) return [canvasWidth, canvasHeight];
    const t = offscreen.get(target);
    return t ? [t.canvas.width, t.canvas.height] : [0, 0];
  };

  for (const cmd of commands) {
    switch (cmd.op) {
      case "create_canvas":
        offscreen.set(cmd.id!, createOffscreen(cmd.w!, cmd.h!));
        break;

      case "set_target":
        target = cmd.id!;
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
}
