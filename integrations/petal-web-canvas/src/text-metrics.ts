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

export interface FontTable {
  /** `advances[codepoint]` = glyph advance ÷ font size. */
  advances: Float64Array;
  /** Ratio for codepoints the table doesn't cover. */
  fallback: number;
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
): FontTable {
  ctx.font = `${PROBE_SIZE}px ${fontStack}`;
  const advances = new Float64Array(TABLE_LIMIT);
  for (let cp = 0x20; cp < TABLE_LIMIT; cp++) {
    // Skip the C1 control block (0x80–0x9f) along with C0 — measuring a
    // control code yields a browser-dependent width for something invisible.
    if (cp >= 0x7f && cp <= 0x9f) continue;
    advances[cp] = ctx.measureText(String.fromCodePoint(cp)).width / PROBE_SIZE;
  }
  return { advances, fallback: FALLBACK_RATIO };
}

/** The subset of the WASM runtime this module drives (kept structural so the
 * measurement code is testable without instantiating WASM). */
export interface MetricsSink {
  set_default_font_metrics(advances: Float64Array, fallback: number): void;
  set_font_metrics(name: string, advances: Float64Array, fallback: number): void;
}

/**
 * Measure every role's font and bind the tables into the runtime: the default
 * role becomes the default font (plain `text_width`), and every role — the
 * default included — is also registered by name for
 * `text_width(s, size, "mono")`.
 *
 * Safe to call once per runtime, at init. A missing 2D context (a browser
 * without canvas support) leaves the runtime on its monospace estimate rather
 * than failing the load.
 */
export function bindFontMetrics(runtime: MetricsSink): void {
  const ctx = measuringContext();
  if (!ctx) return;
  for (const [role, stack] of Object.entries(FONT_STACKS)) {
    const { advances, fallback } = measureFontTable(ctx, stack);
    runtime.set_font_metrics(role, advances, fallback);
    if (role === DEFAULT_ROLE) {
      runtime.set_default_font_metrics(advances, fallback);
    }
  }
}
