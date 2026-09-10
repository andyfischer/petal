/** Measure the browser's fonts and hand the numbers to the Petal runtime.
 *
 * The canvas rasterizes text; only the canvas knows how wide a glyph is. Petal
 * scripts, meanwhile, measure with `text_width(s, size)` — which without host
 * metrics assumes monospace (0.6 × size). Since this host renders a
 * *proportional* face, that assumption was wrong for every centered or
 * right-aligned label. So at startup we walk the codepoints with
 * `ctx.measureText`, normalize each advance by the probe size (advance scales
 * linearly with font size, so one table serves every size), and bind the
 * result — measurement and rasterization then agree by construction.
 *
 * Role → family stacks live here too: scripts select faces by the portable
 * role names (`ui`, `mono`, `serif`), and each host maps them onto what it can
 * actually render. */

/** Font stack per role name. `ui` is also the default font (what a plain
 * `draw_text` / `text_width` uses), so the renderer draws with `FONT_STACKS.ui`
 * and the default advance table is measured from it. */
export const FONT_STACKS: Record<string, string> = {
  ui: "sans-serif",
  mono: "ui-monospace, SFMono-Regular, Menlo, monospace",
  serif: "serif",
};

/** The role a plain `draw_text` (no font specified) renders and measures in. */
export const DEFAULT_ROLE = "ui";

/** Codepoints covered by an advance table: ASCII through Latin Extended-B and
 * the IPA/spacing-modifier blocks. Beyond it, `text_width` uses the fallback
 * ratio — see the plan's note on CJK/emoji. */
const TABLE_LIMIT = 0x300;

/** Font size the glyphs are measured at. Large enough that sub-pixel advances
 * survive the division into a ratio. */
const PROBE_SIZE = 100;

/** Advance ratio for codepoints past the table (a mid-width glyph). */
const FALLBACK_RATIO = 0.5;

/** CSS regular weight — the weight plain text is drawn and measured at. */
export const REGULAR_WEIGHT = 400;

/** The (weight, italic) variants measured for every role. The browser
 * synthesizes any face it doesn't have, so all four always exist; measuring
 * them is what keeps `text_width` honest about a bold or italic run. */
const VARIANTS: Array<[number, boolean]> = [
  [REGULAR_WEIGHT, false],
  [700, false],
  [REGULAR_WEIGHT, true],
  [700, true],
];

/** The name one variant of a face is registered under — regular upright is the
 * bare role name. Mirrors `font_variant_key` in petal-ui's `draw.rs`; the two
 * must agree, since that is the key `text_width` looks a style up by. */
export function fontVariantKey(
  role: string,
  weight: number,
  italic: boolean,
): string {
  if (weight === REGULAR_WEIGHT) return italic ? `${role}@i` : role;
  return italic ? `${role}@${weight}i` : `${role}@${weight}`;
}

/** A CSS font shorthand for a face at a size — the one place the renderer and
 * the measurer agree on how a style becomes a font string. */
export function cssFont(
  fontStack: string,
  size: number,
  weight: number,
  italic: boolean,
): string {
  const slant = italic ? "italic " : "";
  return `${slant}${weight} ${size}px ${fontStack}`;
}

export interface FontTable {
  /** `advances[codepoint]` = glyph advance ÷ font size. */
  advances: Float64Array;
  /** Ratio for codepoints the table doesn't cover. */
  fallback: number;
  /** Where the ink of a run sits relative to the `y` it is drawn at, each a
   * ratio of the font size. The vertical half of the same problem the advance
   * table solves: without it a script has to assume a run is `size` px tall
   * starting at `y`, and every vertically centred label lands high. */
  vertical: VerticalMetrics;
}

/** @see FontTable.vertical */
export interface VerticalMetrics {
  /** `y` -> baseline. This renderer draws with `textBaseline = "top"`, so it
   * is the font bounding box's ascent. */
  baseline: number;
  /** Baseline -> the bottom of the line box. */
  descent: number;
  /** One line's `y` -> the next line's. */
  lineHeight: number;
  /** Baseline -> the top of a flat capital. */
  capHeight: number;
  /** Baseline -> the top of a lowercase "x". */
  xHeight: number;
}

/** Measure a face's vertical proportions with the same context and font
 * shorthand the advance table was measured with.
 *
 * `fontBoundingBox*` describes the *face* and is what `textBaseline = "top"`
 * positions against; `actualBoundingBoxAscent` of "H" and "x" gives the cap
 * and x heights, which is what a UI label wants to be centred on. A browser
 * missing either falls back to the usual proportions rather than to zero,
 * which would collapse every centred label onto the baseline. */
export function measureVertical(
  ctx: CanvasRenderingContext2D,
  fontStack: string,
  weight: number = REGULAR_WEIGHT,
  italic: boolean = false,
): VerticalMetrics {
  ctx.font = cssFont(fontStack, PROBE_SIZE, weight, italic);
  const m = ctx.measureText("Hxdp");
  const baseline = (m.fontBoundingBoxAscent || PROBE_SIZE * 0.8) / PROBE_SIZE;
  const descent = (m.fontBoundingBoxDescent || PROBE_SIZE * 0.2) / PROBE_SIZE;
  const ascentOf = (s: string, fallback: number) => {
    const a = ctx.measureText(s).actualBoundingBoxAscent;
    return a > 0 ? a / PROBE_SIZE : baseline * fallback;
  };
  return {
    baseline,
    descent,
    // The face's own bounding box is its recommended line spacing; browsers
    // expose no separate line gap, and CSS `normal` line-height is this plus
    // a little, which reads as loose for a UI.
    lineHeight: baseline + descent,
    capHeight: ascentOf("H", 0.92),
    xHeight: ascentOf("x", 0.68),
  };
}

/** A detached 2D context to measure in — measurement must not touch the
 * rendering canvas's state (font, transform). */
function measuringContext(): CanvasRenderingContext2D | null {
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  return canvas.getContext("2d");
}

/** Measure per-codepoint advance ratios for a font stack. Control codes get 0
 * (they render nothing and must not contribute width). */
export function measureFontTable(
  ctx: CanvasRenderingContext2D,
  fontStack: string,
  weight: number = REGULAR_WEIGHT,
  italic: boolean = false,
): FontTable {
  ctx.font = cssFont(fontStack, PROBE_SIZE, weight, italic);
  const advances = new Float64Array(TABLE_LIMIT);
  for (let cp = 0x20; cp < TABLE_LIMIT; cp++) {
    // Skip the C1 control block (0x80–0x9f) along with C0 — measuring a
    // control code yields a browser-dependent width for something invisible.
    if (cp >= 0x7f && cp <= 0x9f) continue;
    advances[cp] = ctx.measureText(String.fromCodePoint(cp)).width / PROBE_SIZE;
  }
  return {
    advances,
    fallback: FALLBACK_RATIO,
    vertical: measureVertical(ctx, fontStack, weight, italic),
  };
}

/** The subset of the WASM runtime this module drives (kept structural so the
 * measurement code is testable without instantiating WASM). */
export interface MetricsSink {
  set_default_font_metrics(advances: Float64Array, fallback: number): void;
  set_default_font_vertical(
    baseline: number,
    descent: number,
    lineHeight: number,
    capHeight: number,
    xHeight: number,
  ): void;
  set_font_metrics(
    name: string,
    advances: Float64Array,
    fallback: number,
    baseline: number,
    descent: number,
    lineHeight: number,
    capHeight: number,
    xHeight: number,
  ): void;
  set_default_font_name(name: string): void;
}

/**
 * Measure every role's font — each in all four weight/italic variants — and
 * bind the tables into the runtime: the default role's regular variant becomes
 * the default font (plain `text_width`), and every variant is registered by
 * name for `text_width(s, {font: "mono", weight: 700})`.
 *
 * Safe to call once per runtime, at init. A missing 2D context (a browser
 * without canvas support) leaves the runtime on its monospace estimate rather
 * than failing the load.
 */
export function bindFontMetrics(runtime: MetricsSink): void {
  const ctx = measuringContext();
  if (!ctx) return;
  // The default font is the default role's regular variant, so a style with no
  // `font` resolves that role's variants (`ui@700`) instead of measuring
  // regular metrics for bold text.
  runtime.set_default_font_name(DEFAULT_ROLE);
  for (const [role, stack] of Object.entries(FONT_STACKS)) {
    for (const [weight, italic] of VARIANTS) {
      const { advances, fallback, vertical: v } = measureFontTable(ctx, stack, weight, italic);
      runtime.set_font_metrics(
        fontVariantKey(role, weight, italic),
        advances,
        fallback,
        v.baseline,
        v.descent,
        v.lineHeight,
        v.capHeight,
        v.xHeight,
      );
      if (role === DEFAULT_ROLE && weight === REGULAR_WEIGHT && !italic) {
        runtime.set_default_font_metrics(advances, fallback);
        runtime.set_default_font_vertical(
          v.baseline,
          v.descent,
          v.lineHeight,
          v.capHeight,
          v.xHeight,
        );
      }
    }
  }
}
