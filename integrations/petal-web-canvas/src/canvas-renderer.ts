/** Render petal-ui draw commands to a Canvas2D context.
 *
 * The WASM runtime serializes petal-ui's `DrawCommand` enum directly, so each
 * command already arrives in `{ op, ...fields }` form — no decoding step. Alpha
 * (`a`), corner radius (`radius`), and stroke width (`width`) are optional and
 * omitted from the JSON when at their defaults (opaque / square / hairline). */

import {
  cssFont,
  DEFAULT_ROLE,
  FONT_STACKS,
  REGULAR_WEIGHT,
} from "./text-metrics.js";

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
  /** Face: a role (`ui` / `mono` / `serif`) or a CSS-style fallback list.
   * Absent = the default role, which is what plain text has always meant. */
  font?: string;
  /** CSS numeric weight; absent = 400 (regular). */
  weight?: number;
  /** Absent = upright. */
  italic?: boolean;
  /** Letter-spacing in px; absent = 0. */
  spacing?: number;
  // Gradients (rect_gradient / circle_gradient) — stop 0 then stop 1; the
  // alphas are omitted when opaque, like `a`.
  r0?: number;
  g0?: number;
  b0?: number;
  a0?: number;
  r1?: number;
  g1?: number;
  b1?: number;
  a1?: number;
  /** Gradient axis in radians, clockwise from +x with y growing downward. */
  angle?: number;
  // Shadow
  /** Falloff distance outward from the (spread) shape boundary. */
  blur?: number;
  /** Grow the casting shape by this many px before blurring; may be negative. */
  spread?: number;
  dx?: number;
  dy?: number;
  // Offscreen canvas (create_canvas / set_target / draw_canvas)
  id?: number;
}

function fillStyle(cmd: DrawCommand): string {
  const a = cmd.a ?? 255;
  if (a >= 255) return `rgb(${cmd.r},${cmd.g},${cmd.b})`;
  return `rgba(${cmd.r},${cmd.g},${cmd.b},${a / 255})`;
}

/** One gradient stop's CSS color, from the `*0` / `*1` field group. */
function stopColor(r: number, g: number, b: number, a: number | undefined): string {
  const alpha = a ?? 255;
  if (alpha >= 255) return `rgb(${r},${g},${b})`;
  return `rgba(${r},${g},${b},${alpha / 255})`;
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

    case "rect_gradient": {
      const x = cmd.x!, y = cmd.y!, w = cmd.w!, h = cmd.h!;
      // The CSS `linear-gradient` axis: through the box centre, its length the
      // box's projection onto that direction, so both stops land exactly on
      // opposite corners at 45°. Same geometry the native hosts tessellate.
      const angle = cmd.angle ?? 0;
      const ux = Math.cos(angle), uy = Math.sin(angle);
      const len = Math.abs(w * ux) + Math.abs(h * uy);
      const mx = x + w / 2, my = y + h / 2;
      const grad = ctx.createLinearGradient(
        mx - (ux * len) / 2, my - (uy * len) / 2,
        mx + (ux * len) / 2, my + (uy * len) / 2,
      );
      grad.addColorStop(0, stopColor(cmd.r0!, cmd.g0!, cmd.b0!, cmd.a0));
      grad.addColorStop(1, stopColor(cmd.r1!, cmd.g1!, cmd.b1!, cmd.a1));
      ctx.fillStyle = grad;
      if (cmd.radius && cmd.radius > 0) {
        roundRectPath(ctx, x, y, w, h, cmd.radius);
        ctx.fill();
      } else {
        ctx.fillRect(x, y, w, h);
      }
      break;
    }

    case "circle_gradient": {
      const radius = Math.abs(cmd.radius!);
      const grad = ctx.createRadialGradient(cmd.cx!, cmd.cy!, 0, cmd.cx!, cmd.cy!, radius);
      grad.addColorStop(0, stopColor(cmd.r0!, cmd.g0!, cmd.b0!, cmd.a0));
      grad.addColorStop(1, stopColor(cmd.r1!, cmd.g1!, cmd.b1!, cmd.a1));
      ctx.fillStyle = grad;
      ctx.beginPath();
      ctx.arc(cmd.cx!, cmd.cy!, radius, 0, Math.PI * 2);
      ctx.fill();
      break;
    }

    case "shadow": {
      // Canvas' own shadow is CSS `box-shadow`'s blur, so the offset and the
      // spread go into the *path* and the blur into `shadowBlur`. Painting the
      // (offset, spread) shape itself as well as its halo matches what the
      // native hosts tessellate — a solid core plus its falloff — and is
      // invisible under the card that will be drawn over it.
      const spread = cmd.spread ?? 0;
      const x = cmd.x! + (cmd.dx ?? 0) - spread;
      const y = cmd.y! + (cmd.dy ?? 0) - spread;
      const w = cmd.w! + 2 * spread;
      const h = cmd.h! + 2 * spread;
      if (w <= 0 || h <= 0) break;
      const color = fillStyle(cmd);
      ctx.save();
      ctx.shadowColor = color;
      ctx.shadowBlur = cmd.blur ?? 0;
      ctx.fillStyle = color;
      roundRectPath(ctx, x, y, w, h, Math.max(0, (cmd.radius ?? 0) + spread));
      ctx.fill();
      ctx.restore();
      break;
    }

    case "text": {
      ctx.fillStyle = fillStyle(cmd);
      // The same stack, weight and slant the matching advance table was
      // measured from — drawing and `text_width()` must not disagree about the
      // face. An unknown role falls back to the default one, exactly as the
      // measurement side falls back to the default font.
      const weight = cmd.weight ?? REGULAR_WEIGHT;
      ctx.font = cssFont(
        resolveStack(cmd.font),
        cmd.size!,
        weight,
        cmd.italic ?? false,
      );
      ctx.textBaseline = "top";
      // `letterSpacing` is a recent 2D-context property; where it is missing
      // the run simply draws unspaced rather than throwing.
      const spacing = cmd.spacing ?? 0;
      const spacingCapable = "letterSpacing" in ctx;
      if (spacing !== 0 && spacingCapable) {
        ctx.letterSpacing = `${spacing}px`;
      }
      ctx.fillText(cmd.text!, cmd.x!, cmd.y!);
      if (spacing !== 0 && spacingCapable) {
        ctx.letterSpacing = "0px";
      }
      break;
    }
  }
}

/** The family stack a command's `font` selects: the first role in a CSS-style
 * fallback list (`"Inter, mono"`) this host offers, else the default role. */
function resolveStack(font: string | undefined): string {
  for (const name of font?.split(",") ?? []) {
    const stack = FONT_STACKS[name.trim()];
    if (stack) return stack;
  }
  return FONT_STACKS[DEFAULT_ROLE];
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
  // How many clips are pushed on the active target — one `save()` each, so a
  // `clip_pop` is one `restore()` and Canvas' own intersect-with-current clip
  // semantics give the nesting for free. Per target, because `set_target`
  // switches contexts and each keeps its own save stack.
  const clipDepth = new Map<number, number>();

  const targetCtx = (): CanvasRenderingContext2D | null => {
    if (target === 0) return ctx;
    return offscreen.get(target) ?? null;
  };
  const targetSize = (): [number, number] => {
    if (target === 0) return [canvasWidth, canvasHeight];
    const t = offscreen.get(target);
    return t ? [t.canvas.width, t.canvas.height] : [0, 0];
  };
  /** Drop every clip on the active target, back to the bare drawable. */
  const clearClip = (): void => {
    const dst = targetCtx();
    const depth = clipDepth.get(target) ?? 0;
    if (dst) {
      for (let i = 0; i < depth; i++) dst.restore();
    }
    clipDepth.set(target, 0);
  };
  /** Intersect the active target's clip with `cmd`'s (rounded) rect. */
  const pushClip = (dst: CanvasRenderingContext2D, cmd: DrawCommand): void => {
    dst.save();
    dst.beginPath();
    roundRectPath(dst, cmd.x!, cmd.y!, cmd.w!, cmd.h!, cmd.radius ?? 0);
    dst.clip();
    clipDepth.set(target, (clipDepth.get(target) ?? 0) + 1);
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

      // `clip` *replaces* whatever is in force; `clip_push` nests inside it.
      case "clip": {
        const dst = targetCtx();
        if (dst) {
          clearClip();
          pushClip(dst, cmd);
        }
        break;
      }

      case "clip_push": {
        const dst = targetCtx();
        if (dst) pushClip(dst, cmd);
        break;
      }

      case "clip_pop": {
        const dst = targetCtx();
        const depth = clipDepth.get(target) ?? 0;
        // An unmatched pop is a no-op, per the draw protocol — never a
        // `restore()` that would unwind a save this renderer did not make.
        if (dst && depth > 0) {
          dst.restore();
          clipDepth.set(target, depth - 1);
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
  // Every clip left pushed at the end of the frame simply ends with it.
  for (const id of clipDepth.keys()) {
    target = id;
    clearClip();
  }
}
