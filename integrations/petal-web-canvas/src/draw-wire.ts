/** Decode petal-ui's binary draw-command encoding into `DrawCommand` objects.
 *
 * The runtime hands a frame over as two parts (`take_draw_buffer` and
 * `draw_strings`): a flat `Float64Array` of numbers, where each command is its
 * opcode followed by every one of its fields in a fixed order, and the frame's
 * strings, referenced from the numbers by index. The encoder and the full
 * format description are `core-libs/petal-ui/src/draw_wire.rs`; keep the
 * opcodes and field orders here in step with it.
 *
 * The result is exactly the `{ op, ...fields }` objects the JSON form had —
 * a field at its default (opaque alpha, zero radius, hairline width, …) is
 * left out — so the renderer and anything that inspects commands see no
 * difference. `test/vitest/draw-wire.test.ts` checks that against a fixture
 * the Rust side generates. */

import type { DrawCommand } from "./canvas-renderer";

/** Opcode → command name, indexed by opcode. Append-only, like the Rust side. */
const OPS = [
  "image", "clear", "rect", "rect_outline", "line", "circle", "text",
  "triangle", "poly", "polygon", "fan", "polyline", "ellipse",
  "ellipse_outline", "arc", "rect_gradient", "circle_gradient", "shadow",
  "clip", "clip_none", "clip_push", "clip_pop", "create_canvas",
  "set_target", "draw_canvas", "snapshot", "blur_canvas", "rects", "layer",
  "cache_begin", "cache_end", "cache_draw", "mute", "host",
] as const;

const REGULAR_WEIGHT = 400;

export function decodeDrawCommands(
  buf: ArrayLike<number>,
  strings: readonly string[],
): DrawCommand[] {
  const out: DrawCommand[] = [];
  let i = 0;
  const next = (): number => buf[i++];
  const str = (): string => strings[next()];

  // Field readers that leave a default out, matching the JSON form's
  // `skip_serializing_if`.
  const alpha = (cmd: any, key = "a"): void => {
    const a = next();
    if (a !== 255) cmd[key] = a;
  };
  const unlessZero = (cmd: any, key: string): void => {
    const v = next();
    if (v !== 0) cmd[key] = v;
  };
  const unlessOne = (cmd: any, key: string): void => {
    const v = next();
    if (v !== 1) cmd[key] = v;
  };
  const fields = (cmd: any, ...keys: string[]): void => {
    for (const k of keys) cmd[k] = next();
  };
  const rgba = (cmd: any, suffix = ""): void => {
    cmd[`r${suffix}`] = next();
    cmd[`g${suffix}`] = next();
    cmd[`b${suffix}`] = next();
    alpha(cmd, `a${suffix}`);
  };
  const points = (): number[][] => {
    const n = next();
    const pts: number[][] = new Array(n);
    for (let k = 0; k < n; k++) pts[k] = [next(), next()];
    return pts;
  };
  const optArray = (cmd: any, key: string, len: number): void => {
    if (next() === 0) return;
    const vals: number[] = new Array(len);
    for (let k = 0; k < len; k++) vals[k] = next();
    cmd[key] = vals;
  };

  while (i < buf.length) {
    const code = next();
    const op = OPS[code];
    if (op === undefined) {
      throw new Error(`draw buffer: unknown opcode ${code} at ${i - 1}`);
    }
    const cmd: any = { op };
    switch (op) {
      case "image":
        cmd.source = str();
        fields(cmd, "x", "y", "w", "h");
        alpha(cmd);
        unlessZero(cmd, "radius");
        optArray(cmd, "tint", 3);
        optArray(cmd, "src", 4);
        optArray(cmd, "slice", 4);
        break;
      case "clear":
        fields(cmd, "r", "g", "b");
        break;
      case "rect":
        fields(cmd, "x", "y", "w", "h");
        rgba(cmd);
        unlessZero(cmd, "radius");
        break;
      case "rect_outline":
        fields(cmd, "x", "y", "w", "h");
        rgba(cmd);
        unlessOne(cmd, "width");
        unlessZero(cmd, "radius");
        break;
      case "line":
        fields(cmd, "x1", "y1", "x2", "y2");
        rgba(cmd);
        unlessOne(cmd, "width");
        break;
      case "circle":
        fields(cmd, "cx", "cy", "radius");
        rgba(cmd);
        break;
      case "text": {
        cmd.text = str();
        fields(cmd, "x", "y", "size");
        rgba(cmd);
        const font = next();
        if (font >= 0) cmd.font = strings[font];
        const weight = next();
        if (weight !== REGULAR_WEIGHT) cmd.weight = weight;
        if (next() !== 0) cmd.italic = true;
        unlessZero(cmd, "spacing");
        break;
      }
      case "triangle":
        fields(cmd, "x1", "y1", "x2", "y2", "x3", "y3");
        rgba(cmd);
        break;
      case "poly":
      case "polygon":
        cmd.points = points();
        rgba(cmd);
        break;
      case "fan":
        fields(cmd, "cx", "cy");
        cmd.points = points();
        rgba(cmd);
        break;
      case "polyline":
        cmd.points = points();
        rgba(cmd);
        unlessOne(cmd, "width");
        break;
      case "ellipse":
        fields(cmd, "cx", "cy", "rx", "ry");
        rgba(cmd);
        break;
      case "ellipse_outline":
        fields(cmd, "cx", "cy", "rx", "ry");
        rgba(cmd);
        unlessOne(cmd, "width");
        break;
      case "arc":
        fields(cmd, "cx", "cy", "r_in", "r_out", "a0", "a1");
        rgba(cmd);
        break;
      case "rect_gradient":
        fields(cmd, "x", "y", "w", "h");
        unlessZero(cmd, "radius");
        rgba(cmd, "0");
        rgba(cmd, "1");
        cmd.angle = next();
        break;
      case "circle_gradient":
        fields(cmd, "cx", "cy", "radius");
        rgba(cmd, "0");
        rgba(cmd, "1");
        break;
      case "shadow":
        fields(cmd, "x", "y", "w", "h");
        unlessZero(cmd, "radius");
        cmd.blur = next();
        unlessZero(cmd, "spread");
        unlessZero(cmd, "dx");
        unlessZero(cmd, "dy");
        rgba(cmd);
        break;
      case "clip":
      case "clip_push":
        fields(cmd, "x", "y", "w", "h");
        unlessZero(cmd, "radius");
        break;
      case "clip_none":
      case "clip_pop":
        break;
      case "create_canvas":
      case "cache_begin":
        fields(cmd, "id", "w", "h");
        break;
      case "set_target":
      case "cache_end":
        fields(cmd, "id");
        break;
      case "draw_canvas":
        fields(cmd, "id", "x", "y");
        alpha(cmd);
        unlessZero(cmd, "w");
        unlessZero(cmd, "h");
        break;
      case "snapshot":
      case "cache_draw":
        fields(cmd, "id", "x", "y");
        break;
      case "blur_canvas":
        fields(cmd, "id", "radius");
        break;
      case "rects": {
        const n = next();
        const rects = new Array(n);
        for (let k = 0; k < n; k++) {
          rects[k] = {
            x: next(), y: next(), w: next(), h: next(),
            r: next(), g: next(), b: next(), a: next(),
          };
        }
        cmd.rects = rects;
        break;
      }
      case "layer":
        fields(cmd, "n");
        break;
      case "mute":
        cmd.on = next() !== 0;
        break;
      case "host":
        cmd.tag = str();
        break;
    }
    out.push(cmd as DrawCommand);
  }
  return out;
}
