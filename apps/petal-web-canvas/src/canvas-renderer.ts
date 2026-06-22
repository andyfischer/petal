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
  // Poly — serde serializes Vec<(i32,i32)> as [[x,y],...]
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
