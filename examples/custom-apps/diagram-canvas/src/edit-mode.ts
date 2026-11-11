/** Edit mode: drag a drawn item, and the code that drew it is edited.
 *
 * The page's half of Petal's edit mode (docs/features/direct-manipulation.md,
 * "Hosts"); the runtime's half is `PetalRuntime.edit_*` in petal-web-canvas,
 * over the shared `petal_ui::edit_mode`.
 *
 * - Entering turns on emit tracing and the pre-run checkpoint, runs one
 *   traced frame, and then freezes the frame loop: the program stops, input
 *   no longer reaches the script, and the drag re-runs a stable frame.
 * - The lines selected in the source editor are the lines whose literals a
 *   drag may change (the request's allow set, `line:N`). The selection lives
 *   only in this page: it is never written into the source. With nothing
 *   selected (or the editor closed), the request runs best effort: `tunable`
 *   literals first, never `const(…)` or `rigid(…)`, an equal split across
 *   ties, one answer or an error.
 * - Clicking a drawn item names the call that drew it and selects that line,
 *   which a shift-click on more lines in the editor widens.
 * - Dragging it proposes edits. The best one is put into the editor at once
 *   (one undo takes it back) and the others are listed to pick from. A
 *   failure says why and changes nothing.
 */

import type { PetalCanvas } from "petal-web-canvas";
import type { SourceEditor } from "./editor.js";

/** Pointer travel (pixels, both axes summed) below which a press is a click. */
const DRAG_SLOP = 3;

interface Pick {
  ok: boolean;
  error?: string;
  index?: number;
  callee?: string | null;
  line?: number | null;
  label?: string;
  bounds?: [number, number, number, number] | null;
}

interface Rewrite {
  line: number | null;
  new_text: string;
  description: string;
}

export interface Alternative {
  description: string;
  verified: boolean;
  moves_others: number;
  sketch_only: boolean;
  rewrites: Rewrite[];
  source: string | null;
  apply_error: string | null;
}

export interface Outcome {
  ok: boolean;
  error?: string;
  alternatives?: Alternative[];
}

export interface EditModeHost {
  /** The source editor, when it is open (a closed one has no selection). */
  editor(): SourceEditor | null;
  /** The sketch text running now. */
  source(): string;
  /** Put `source` in the editor (when open) and run it. */
  applySource(source: string): void;
}

interface Press {
  x: number;
  y: number;
  pick: Pick;
  /** `edit_begin` ran (the press became a drag). */
  begun: boolean;
  /** `edit_begin` refused; the drag does nothing more. */
  failed: boolean;
  dx: number;
  dy: number;
}

export class EditMode {
  active = false;
  private frameRequested = false;
  private press: Press | null = null;
  private picked: Pick | null = null;
  private preview = "";
  private note: { kind: "ok" | "error"; text: string } | null = null;
  /** The last drag's answers, and which of them is in the editor. */
  alternatives: Alternative[] = [];
  applied = -1;
  /** The last outcome, as the runtime reported it (for tests and tools). */
  lastOutcome: Outcome | null = null;

  constructor(
    private petal: PetalCanvas,
    private overlay: HTMLCanvasElement,
    private panel: HTMLElement,
    private host: EditModeHost,
  ) {
    overlay.addEventListener("pointerdown", (e) => this.onDown(e));
    overlay.addEventListener("pointermove", (e) => this.onMove(e));
    overlay.addEventListener("pointerup", (e) => this.onUp(e));
    overlay.addEventListener("pointercancel", () => this.cancelPress());
    overlay.addEventListener("contextmenu", (e) => e.preventDefault());
  }

  /** The frame gate while edit mode is on: one frame when one was asked for
   * (entering, or the program was reloaded by an edit), else frozen. */
  gate(): number | null {
    if (!this.frameRequested) return null;
    this.frameRequested = false;
    return 1 / 60;
  }

  /** A traced frame was asked for and has not run yet: a pick now would see
   * the frame before it. */
  get pending(): boolean {
    return this.frameRequested;
  }

  /** Run one frame (traced, with a fresh checkpoint) at the next tick. */
  requestFrame(): void {
    this.frameRequested = true;
    this.petal.invalidate();
  }

  toggle(): void {
    if (this.active) this.exit();
    else this.enter();
  }

  enter(): void {
    const rt = this.petal.runtime;
    if (!rt || this.active) return;
    rt.set_emit_trace(true);
    rt.set_edit_checkpoint(true);
    this.active = true;
    this.picked = null;
    this.note = null;
    this.alternatives = [];
    this.applied = -1;
    this.overlay.classList.add("active");
    this.panel.classList.add("visible");
    this.host.editor()?.setEditMode(true);
    this.requestFrame();
    this.render();
  }

  exit(): void {
    const rt = this.petal.runtime;
    if (!this.active) return;
    this.cancelPress();
    rt?.edit_cancel();
    rt?.set_edit_checkpoint(false);
    rt?.set_emit_trace(false);
    this.active = false;
    this.frameRequested = false;
    this.overlay.classList.remove("active");
    this.panel.classList.remove("visible");
    this.host.editor()?.setEditMode(false);
    this.petal.invalidate();
    this.clearOverlay();
  }

  /** The program was reloaded (an edit, or typing in the editor): show its
   * first frame, traced, and forget what was picked on the old one. */
  reloaded(): void {
    if (!this.active) return;
    this.cancelPress();
    this.picked = null;
    this.requestFrame();
    this.render();
  }

  /** The lines a drag may change: the editor's selection, if it is open. */
  allowLines(): number[] {
    return this.host.editor()?.selectedLines() ?? [];
  }

  /** Put alternative `i` of the last drag into the editor and run it. */
  apply(i: number): void {
    const alt = this.alternatives[i];
    if (!alt) return;
    if (alt.source === null) {
      this.note = { kind: "error", text: alt.apply_error ?? "this answer edits another file" };
      this.render();
      return;
    }
    this.applied = i;
    this.note = { kind: "ok", text: alt.description };
    this.host.applySource(alt.source);
    this.render();
  }

  // --- Pointer -----------------------------------------------------------

  private pos(e: PointerEvent): [number, number] {
    const r = this.overlay.getBoundingClientRect();
    return [Math.round(e.clientX - r.left), Math.round(e.clientY - r.top)];
  }

  private onDown(e: PointerEvent): void {
    if (!this.active || e.button !== 0) return;
    const rt = this.petal.runtime;
    if (!rt) return;
    e.preventDefault();
    this.overlay.setPointerCapture(e.pointerId);
    const [x, y] = this.pos(e);
    const pick: Pick = JSON.parse(rt.edit_pick(x, y));
    this.press = { x, y, pick, begun: false, failed: false, dx: 0, dy: 0 };
    this.picked = pick.ok ? pick : null;
    this.preview = "";
    if (!pick.ok) this.note = { kind: "error", text: pick.error ?? "nothing is drawn there" };
    this.render();
  }

  private onMove(e: PointerEvent): void {
    const p = this.press;
    if (!this.active || !p || !p.pick.ok || p.failed) return;
    const rt = this.petal.runtime;
    if (!rt) return;
    const [x, y] = this.pos(e);
    p.dx = x - p.x;
    p.dy = y - p.y;
    if (!p.begun) {
      if (Math.abs(p.dx) + Math.abs(p.dy) < DRAG_SLOP) return;
      // The expensive step: screening and the Jacobian (bounded by the
      // engine's run and wall-clock budgets).
      const lines = this.allowLines();
      const begun = JSON.parse(rt.edit_begin(p.pick.index!, JSON.stringify(lines)));
      if (!begun.ok) {
        p.failed = true;
        this.note = { kind: "error", text: begun.error };
        this.lastOutcome = begun;
        this.render();
        return;
      }
      p.begun = true;
    }
    const rewrites: Rewrite[] = JSON.parse(rt.edit_preview(p.dx, p.dy));
    this.preview = rewrites.map((r) => r.description).join("; ");
    this.render();
  }

  private onUp(e: PointerEvent): void {
    const p = this.press;
    this.press = null;
    if (!this.active || !p) return;
    const rt = this.petal.runtime;
    if (!rt) return;
    this.preview = "";
    if (p.begun) {
      const [x, y] = this.pos(e);
      const out: Outcome = JSON.parse(rt.edit_finish(x - p.x, y - p.y));
      this.finish(out);
    } else if (!p.failed && p.pick.ok && Math.abs(p.dx) + Math.abs(p.dy) < DRAG_SLOP) {
      // A click: select the call's line, the start of a selection the author
      // can widen in the editor.
      if (p.pick.line) this.host.editor()?.selectLine(p.pick.line);
    }
    this.render();
  }

  private cancelPress(): void {
    if (this.press?.begun) this.petal.runtime?.edit_cancel();
    this.press = null;
    this.preview = "";
  }

  private finish(out: Outcome): void {
    this.lastOutcome = out;
    if (!out.ok || !out.alternatives?.length) {
      this.alternatives = [];
      this.applied = -1;
      this.note = { kind: "error", text: out.error ?? "no edit reaches the goal" };
      return;
    }
    this.alternatives = out.alternatives;
    this.applied = -1;
    this.apply(0);
  }

  // --- Drawing -----------------------------------------------------------

  private clearOverlay(): void {
    const ctx = this.overlay.getContext("2d");
    ctx?.clearRect(0, 0, this.overlay.width, this.overlay.height);
  }

  private drawOverlay(): void {
    const canvas = this.petal.canvas;
    if (canvas && (this.overlay.width !== canvas.width || this.overlay.height !== canvas.height)) {
      this.overlay.width = canvas.width;
      this.overlay.height = canvas.height;
    }
    const ctx = this.overlay.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, this.overlay.width, this.overlay.height);
    if (!this.active) return;
    const b = this.picked?.bounds;
    if (!b) return;
    const [x, y, w, h] = b;
    ctx.lineWidth = 2;
    ctx.strokeStyle = "#ffd23c";
    ctx.strokeRect(x - 3, y - 3, w + 6, h + 6);
    const p = this.press;
    if (p && p.begun && !p.failed) {
      ctx.setLineDash([4, 3]);
      ctx.strokeStyle = "#78dcff";
      ctx.strokeRect(x - 3 + p.dx, y - 3 + p.dy, w + 6, h + 6);
      ctx.beginPath();
      ctx.moveTo(p.x, p.y);
      ctx.lineTo(p.x + p.dx, p.y + p.dy);
      ctx.stroke();
      ctx.setLineDash([]);
    }
  }

  /** Redraw the overlay and the panel. */
  render(): void {
    this.drawOverlay();
    if (!this.active) return;
    const lines = this.allowLines();
    const scope = lines.length
      ? `May change: ${describeLines(lines)} (the editor's selection)`
      : this.host.editor()
        ? "Nothing selected: best effort (tunable first, never const or rigid)"
        : "No editor open: best effort (tunable first, never const or rigid)";
    const el = (tag: string, cls: string, text: string, testid?: string) => {
      const n = document.createElement(tag);
      n.className = cls;
      n.textContent = text;
      if (testid) n.dataset.testid = testid;
      return n;
    };
    this.panel.replaceChildren();
    this.panel.append(el("div", "edit-title", "Edit mode: drag a shape to edit the code that drew it"));
    this.panel.append(el("div", "edit-scope", scope, "edit-scope"));
    if (this.picked?.label) this.panel.append(el("div", "edit-picked", this.picked.label, "edit-picked"));
    if (this.preview) this.panel.append(el("div", "edit-preview", `→ ${this.preview}`, "edit-preview"));
    if (this.note) {
      const prefix = this.note.kind === "ok" ? "Applied: " : "No edit: ";
      this.panel.append(el("div", `edit-note ${this.note.kind}`, prefix + this.note.text, "edit-note"));
    }
    if (this.alternatives.length > 1) {
      const list = el("div", "edit-alts", "", "edit-alts");
      this.alternatives.forEach((alt, i) => {
        const btn = el("button", i === this.applied ? "edit-alt applied" : "edit-alt", "", "edit-alt");
        btn.textContent = `${i + 1}. ${alt.description}${alt.verified ? "" : " (unverified)"}`;
        btn.addEventListener("click", () => this.apply(i));
        list.append(btn);
      });
      this.panel.append(list);
    }
  }
}

/** "line 4", "lines 4, 7–9". */
function describeLines(lines: number[]): string {
  const parts: string[] = [];
  let i = 0;
  while (i < lines.length) {
    let j = i;
    while (j + 1 < lines.length && lines[j + 1] === lines[j] + 1) j++;
    parts.push(i === j ? `${lines[i]}` : `${lines[i]}–${lines[j]}`);
    i = j + 1;
  }
  return `${lines.length === 1 ? "line" : "lines"} ${parts.join(", ")}`;
}
