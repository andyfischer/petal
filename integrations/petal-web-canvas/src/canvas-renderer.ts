/** Render petal-ui draw commands to a Canvas2D context.
 *
 * Commands are petal-ui's `DrawCommand` enum in `{ op, ...fields }` form, as
 * `decodeDrawCommands` (`draw-wire.ts`) rebuilds them from the runtime's
 * binary draw buffer. Alpha (`a`), corner radius (`radius`), and stroke width
 * (`width`) are optional and left out when at their defaults (opaque / square
 * / hairline).
 *
 * Coordinates may be fractional (a sub-pixel frame, `decodeFrameCommands`):
 * Canvas2D draws at any position, so every op takes them as they are, and a
 * command whose numbers are whole draws exactly as it always has. The one
 * difference is image resampling: an `image` or `draw_canvas` marked
 * `subpixel` (drawn under `pixel_snap(false)`) always draws with
 * `imageSmoothingEnabled`, since it sits between pixels; one marked
 * `pixelSnap` (drawn under `pixel_snap(true)`: pixel art) always draws
 * without it; any other keeps the context's own setting. */

import {
  cssFont,
  DEFAULT_ROLE,
  FONT_STACKS,
  REGULAR_WEIGHT,
} from "./text-metrics";

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
  // Circle / Ellipse / Arc / Fan (the fan's hub)
  cx?: number;
  cy?: number;
  /** Ellipse semi-axes. */
  rx?: number;
  ry?: number;
  /** Arc: inner and outer radius, start and end angle (radians, clockwise
   * from +x with y growing downward). */
  r_in?: number;
  r_out?: number;
  // (`a0`/`a1`, the angles, share their fields with the gradient stop alphas
  // below.)
  // Image
  /** Host-resolved asset path or logical name. */
  source?: string;
  /** Multiply color over the image's own (alpha kept). */
  tint?: [number, number, number];
  /** Source rect `[x, y, w, h]` in image pixels (an atlas cell). */
  src?: [number, number, number, number];
  /** Nine-slice insets `[left, top, right, bottom]` in image pixels. */
  slice?: [number, number, number, number];
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
  /** First gradient stop's alpha — or, on `arc`, the start angle. */
  a0?: number;
  r1?: number;
  g1?: number;
  b1?: number;
  /** Second gradient stop's alpha — or, on `arc`, the end angle. */
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
  // Offscreen canvas (create_canvas / set_target / draw_canvas / snapshot /
  // blur_canvas). `draw_canvas` also carries `a` (opacity) and `w`/`h` (a
  // destination size, 0 = the canvas's own); `blur_canvas` carries `radius`.
  id?: number;
  /** Set by `decodeFrameCommands` on a sub-pixel command: drawn under a snap
   * mode that leaves it unsnapped, so its geometry may be fractional. Absent
   * on every command of a whole-pixel frame. */
  subpixel?: boolean;
  /** Set by `decodeFrameCommands` on a command drawn under
   * `pixel_snap(true)`: pixel art, resampled without smoothing. */
  pixelSnap?: boolean;
}

/** Run `draw` with image smoothing on when `cmd` is sub-pixel and off when
 * it is pixel art (`pixelSnap`); any other command (and every command of a
 * whole-pixel frame) draws with the context's setting untouched. */
function withSmoothing(
  ctx: CanvasRenderingContext2D,
  cmd: DrawCommand,
  draw: () => void,
): void {
  if (!cmd.subpixel && !cmd.pixelSnap) {
    draw();
    return;
  }
  const saved = ctx.imageSmoothingEnabled;
  ctx.imageSmoothingEnabled = !cmd.pixelSnap;
  draw();
  ctx.imageSmoothingEnabled = saved;
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

/** Trace an open or closed path through `points` (`[[x, y], ...]`). */
function tracePoints(ctx: CanvasRenderingContext2D, points: number[][]): void {
  ctx.moveTo(points[0][0], points[0][1]);
  for (let i = 1; i < points.length; i++) {
    ctx.lineTo(points[i][0], points[i][1]);
  }
}

/** Loaded images by source, shared across frames. A source that has not
 * finished loading draws nothing this frame; `onImageLoad` (set by the
 * runtime) asks for a repaint once it arrives. */
const imageCache = new Map<string, HTMLImageElement | "error">();
let imageLoaded: (() => void) | null = null;

/** Register a callback run when a `draw_image` source finishes loading, so a
 * host that skips unchanged frames can repaint. */
export function onImageLoad(cb: (() => void) | null): void {
  imageLoaded = cb;
}

function imageFor(source: string): HTMLImageElement | null {
  const hit = imageCache.get(source);
  if (hit === "error") return null;
  if (hit) return hit.complete && hit.naturalWidth > 0 ? hit : null;
  if (typeof Image === "undefined") return null;
  const img = new Image();
  img.onload = () => imageLoaded?.();
  img.onerror = () => imageCache.set(source, "error");
  img.src = source;
  imageCache.set(source, img);
  return null;
}

/** Draw `cmd` (an `image` command) from `img`: its `src` cell (or the whole
 * image), nine-sliced when `slice` is set, tinted, at opacity `a`, inside its
 * rounded corners. */
function drawImageCmd(
  ctx: CanvasRenderingContext2D,
  img: CanvasImageSource & { width: number; height: number },
  cmd: DrawCommand,
): void {
  const [sx, sy, sw, sh] = cmd.src ?? [0, 0, img.width, img.height];
  const x = cmd.x!, y = cmd.y!, w = cmd.w!, h = cmd.h!;
  if (w <= 0 || h <= 0 || sw <= 0 || sh <= 0) return;
  // Tint multiplies the color in and keeps the image's alpha: render the
  // cell into a scratch canvas, then multiply and re-mask it there.
  let from: CanvasImageSource = img;
  let [fx, fy] = [sx, sy];
  if (cmd.tint) {
    const scratch = createOffscreen(sw, sh);
    scratch.drawImage(img, sx, sy, sw, sh, 0, 0, sw, sh);
    scratch.globalCompositeOperation = "multiply";
    scratch.fillStyle = `rgb(${cmd.tint[0]},${cmd.tint[1]},${cmd.tint[2]})`;
    scratch.fillRect(0, 0, sw, sh);
    scratch.globalCompositeOperation = "destination-in";
    scratch.drawImage(img, sx, sy, sw, sh, 0, 0, sw, sh);
    from = scratch.canvas;
    [fx, fy] = [0, 0];
  }
  ctx.save();
  ctx.globalAlpha *= (cmd.a ?? 255) / 255;
  if (cmd.radius && cmd.radius > 0) {
    roundRectPath(ctx, x, y, w, h, cmd.radius);
    ctx.clip();
  }
  if (cmd.slice) {
    // Corners at their own size, edges stretched along one axis, the
    // centre along both. Insets clamp so they never cross on a small box.
    const [l, t, r, b] = cmd.slice;
    const kx = Math.min(1, w / Math.max(1, l + r));
    const ky = Math.min(1, h / Math.max(1, t + b));
    const dl = l * kx, dr = r * kx, dt = t * ky, db = b * ky;
    const sxs = [0, l, sw - r, sw], sys = [0, t, sh - b, sh];
    const dxs = [x, x + dl, x + w - dr, x + w], dys = [y, y + dt, y + h - db, y + h];
    for (let row = 0; row < 3; row++) {
      for (let col = 0; col < 3; col++) {
        const cw = sxs[col + 1] - sxs[col], ch = sys[row + 1] - sys[row];
        const dw = dxs[col + 1] - dxs[col], dh = dys[row + 1] - dys[row];
        if (cw > 0 && ch > 0 && dw > 0 && dh > 0) {
          ctx.drawImage(from, fx + sxs[col], fy + sys[row], cw, ch, dxs[col], dys[row], dw, dh);
        }
      }
    }
  } else {
    ctx.drawImage(from, fx, fy, sw, sh, x, y, w, h);
  }
  ctx.restore();
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

    case "polygon": {
      // Any simple polygon, concave included: the nonzero fill rule covers
      // exactly its interior, which is what the native hosts' ear-clipping
      // tessellates.
      const points = cmd.points!;
      if (points.length >= 3) {
        ctx.fillStyle = fillStyle(cmd);
        ctx.beginPath();
        tracePoints(ctx, points);
        ctx.closePath();
        ctx.fill();
      }
      break;
    }

    case "fan": {
      // Triangles from the hub to each consecutive pair of points. The fan
      // is open (repeat the first point to close it), so the path runs hub →
      // points → hub.
      const points = cmd.points!;
      if (points.length >= 2) {
        ctx.fillStyle = fillStyle(cmd);
        ctx.beginPath();
        ctx.moveTo(cmd.cx!, cmd.cy!);
        for (const [px, py] of points) ctx.lineTo(px, py);
        ctx.closePath();
        ctx.fill();
      }
      break;
    }

    case "polyline": {
      // One stroke, so a translucent line composites once, with round joins
      // and caps like the native hosts' wedge joins and half-disc caps.
      const points = cmd.points!;
      if (points.length >= 2) {
        ctx.save();
        ctx.strokeStyle = fillStyle(cmd);
        ctx.lineWidth = cmd.width ?? 1;
        ctx.lineJoin = "round";
        ctx.lineCap = "round";
        ctx.beginPath();
        tracePoints(ctx, points);
        ctx.stroke();
        ctx.restore();
      }
      break;
    }

    case "ellipse":
      ctx.fillStyle = fillStyle(cmd);
      ctx.beginPath();
      ctx.ellipse(cmd.cx!, cmd.cy!, Math.abs(cmd.rx!), Math.abs(cmd.ry!), 0, 0, Math.PI * 2);
      ctx.fill();
      break;

    case "ellipse_outline": {
      // Stroked *inside* the rx/ry boundary; a width at or past the smaller
      // semi-axis fills solid, as the native hosts do.
      const rx = Math.abs(cmd.rx!), ry = Math.abs(cmd.ry!);
      const t = cmd.width ?? 1;
      ctx.beginPath();
      if (t >= Math.min(rx, ry)) {
        ctx.fillStyle = fillStyle(cmd);
        ctx.ellipse(cmd.cx!, cmd.cy!, rx, ry, 0, 0, Math.PI * 2);
        ctx.fill();
      } else {
        ctx.strokeStyle = fillStyle(cmd);
        ctx.lineWidth = t;
        ctx.ellipse(cmd.cx!, cmd.cy!, rx - t / 2, ry - t / 2, 0, 0, Math.PI * 2);
        ctx.stroke();
      }
      break;
    }

    case "arc": {
      // An annular sector from a0 to a1 between r_in and r_out; r_in = 0 is
      // a pie slice. A sweep past a full turn is clamped to one ring.
      const rIn = Math.max(0, cmd.r_in ?? 0), rOut = Math.max(0, cmd.r_out ?? 0);
      const a0 = cmd.a0 ?? 0;
      const sweep = Math.max(-Math.PI * 2, Math.min(Math.PI * 2, (cmd.a1 ?? 0) - a0));
      if (rOut <= rIn || Math.abs(sweep) < 1e-6) break;
      const a1 = a0 + sweep, ccw = sweep < 0;
      ctx.fillStyle = fillStyle(cmd);
      ctx.beginPath();
      ctx.arc(cmd.cx!, cmd.cy!, rOut, a0, a1, ccw);
      if (rIn > 0) {
        ctx.arc(cmd.cx!, cmd.cy!, rIn, a1, a0, !ccw);
      } else {
        ctx.lineTo(cmd.cx!, cmd.cy!);
      }
      ctx.closePath();
      ctx.fill();
      break;
    }

    case "image": {
      const img = imageFor(cmd.source!);
      if (img) withSmoothing(ctx, cmd, () => drawImageCmd(ctx, img, cmd));
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
          // Group opacity: the layer composites once at `a`, so overlapping
          // shapes inside it read as one object. `w`/`h` of 0 mean the
          // canvas's own size.
          const w = cmd.w || src.canvas.width;
          const h = cmd.h || src.canvas.height;
          const a = cmd.a === undefined ? 1 : cmd.a / 255;
          const saved = dst.globalAlpha;
          dst.globalAlpha = saved * a;
          withSmoothing(dst, cmd, () => dst.drawImage(src.canvas, cmd.x!, cmd.y!, w, h));
          dst.globalAlpha = saved;
        }
        break;
      }

      case "snapshot": {
        // Copy the current target's pixels under the canvas rect into the
        // canvas: the backdrop a material blurs. Replace, don't blend — the
        // snapshot *is* what was there.
        const dstCanvas = offscreen.get(cmd.id!);
        const src = targetCtx();
        if (dstCanvas && src && src !== dstCanvas) {
          const w = dstCanvas.canvas.width;
          const h = dstCanvas.canvas.height;
          const op = dstCanvas.globalCompositeOperation;
          dstCanvas.globalCompositeOperation = "copy";
          dstCanvas.drawImage(src.canvas, cmd.x!, cmd.y!, w, h, 0, 0, w, h);
          dstCanvas.globalCompositeOperation = op;
        }
        break;
      }

      case "blur_canvas": {
        // In place, through the 2D context's own Gaussian: draw the canvas
        // onto itself with a blur filter. Edge clamping comes from padding
        // the source draw by the blur extent, so a backdrop at the edge does
        // not fade to transparent.
        const c = offscreen.get(cmd.id!);
        const radius = cmd.radius ?? 0;
        if (c && radius > 0) {
          const w = c.canvas.width;
          const h = c.canvas.height;
          const copy = createOffscreen(w, h);
          copy.drawImage(c.canvas, 0, 0);
          c.save();
          c.setTransform(1, 0, 0, 1, 0, 0);
          c.globalCompositeOperation = "copy";
          c.filter = `blur(${radius}px)`;
          const pad = Math.ceil(radius * 3);
          // Edge pixels stretched outward by `pad` on every side, so the blur
          // samples clamped colour rather than transparent black.
          c.drawImage(copy.canvas, 0, 0, 1, 1, -pad, -pad, pad, pad);
          c.drawImage(copy.canvas, 0, 0, w, 1, 0, -pad, w, pad);
          c.drawImage(copy.canvas, w - 1, 0, 1, 1, w, -pad, pad, pad);
          c.globalCompositeOperation = "source-over";
          c.drawImage(copy.canvas, 0, 0, 1, h, -pad, 0, pad, h);
          c.drawImage(copy.canvas, w - 1, 0, 1, h, w, 0, pad, h);
          c.drawImage(copy.canvas, 0, h - 1, 1, 1, -pad, h, pad, pad);
          c.drawImage(copy.canvas, 0, h - 1, w, 1, 0, h, w, pad);
          c.drawImage(copy.canvas, w - 1, h - 1, 1, 1, w, h, pad, pad);
          c.drawImage(copy.canvas, 0, 0);
          c.restore();
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
