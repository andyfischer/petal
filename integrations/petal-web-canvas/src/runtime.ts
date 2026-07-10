/** PetalCanvas — wraps PetalRuntime for canvas-based frame-loop execution. */

import init, { PetalRuntime } from "../pkg/petal_web_canvas.js";
import { renderCommands } from "./canvas-renderer.js";
import { InputTracker } from "./input.js";

export class PetalCanvas {
  runtime: PetalRuntime | null = null;
  private stackId: number | null = null;
  canvas: HTMLCanvasElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private input = new InputTracker();
  private animId: number | null = null;
  frameCount = 0;
  private lastTime = 0;
  private errorEl: HTMLElement | null = null;
  private currentSource = "";
  private errored = false;

  /**
   * Optional frame gate. Given the real elapsed dt, return the dt to run the
   * frame with, or `null` to skip the frame body while keeping the rAF loop
   * alive. Defaults to clamping dt at 0.1s. Hosts (e.g. a debug pause/step
   * controller) can override it.
   */
  frameGate: ((realDt: number) => number | null) | null = null;

  /** Optional callback invoked after each frame renders (debug panels use it). */
  onFrameComplete: (() => void) | null = null;

  async init(): Promise<void> {
    await init();
    this.runtime = new PetalRuntime();
    this.input.setRuntime(this.runtime);
  }

  load(source: string): void {
    if (!this.runtime) throw new Error("Runtime not initialized");
    this.currentSource = source;

    // A previous WASM panic poisons the module — surface a clear message.
    if (this.errored) {
      this.showError(
        "Previous script panicked — reload the page to recover.\n(WASM cannot be re-entered after an unreachable trap.)",
      );
      return;
    }

    try {
      const programId = this.runtime.load_program(source);
      this.stackId = this.runtime.create_stack(programId);
    } catch (err: any) {
      this.showError(String(err));
      return;
    }

    this.errored = false;
    this.frameCount = 0;
    this.lastTime = performance.now();
    this.clearError();

    if (this.animId === null) this.loop();
  }

  start(canvas: HTMLCanvasElement, errorEl?: HTMLElement): void {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d")!;
    this.errorEl = errorEl ?? null;
    this.input.attach(canvas);
    this.resizeCanvas();

    const ro = new ResizeObserver(() => this.resizeCanvas());
    ro.observe(canvas.parentElement ?? canvas);

    this.lastTime = performance.now();
    this.loop();
  }

  resize(): void {
    this.resizeCanvas();
  }

  private resizeCanvas(): void {
    if (!this.canvas) return;
    const wrap = this.canvas.parentElement;
    if (wrap) {
      this.canvas.width = wrap.clientWidth;
      this.canvas.height = wrap.clientHeight;
    } else {
      this.canvas.width = window.innerWidth;
      this.canvas.height = window.innerHeight;
    }
  }

  private loop = (): void => {
    this.animId = requestAnimationFrame(this.loop);

    if (!this.runtime || this.stackId === null || !this.ctx || !this.canvas) return;
    if (this.errored) return;

    const now = performance.now();
    const realDt = (now - this.lastTime) / 1000;
    this.lastTime = now;

    const dt = this.frameGate ? this.frameGate(realDt) : Math.min(realDt, 0.1);
    if (dt === null) return; // gated off (e.g. paused) — skip body, keep rAF alive

    this.runFrame(dt);
  };

  /**
   * Run one frame with an explicit dt and return its draw commands as JSON.
   * Used by the loop and by debug stepping (which drives frames while the
   * loop is gated off). Bypasses the frame gate.
   */
  runOneFrame(dt: number): string {
    return this.runFrame(dt);
  }

  /** Stop the animation loop. */
  stop(): void {
    if (this.animId !== null) {
      cancelAnimationFrame(this.animId);
      this.animId = null;
    }
  }

  private runFrame(dt: number): string {
    if (!this.runtime || this.stackId === null || !this.ctx || !this.canvas) return "[]";
    if (this.errored) return "[]";

    try {
      // Stage timing/dimensions first (begin_frame advances the input clock by
      // dt to promote this frame's pending input edges), then run.
      this.runtime.set_frame_info(dt, this.frameCount, this.canvas.width, this.canvas.height);
      this.runtime.begin_frame();
      this.runtime.reset_and_run(this.stackId);
      this.frameCount++;

      const cmdsJson = this.runtime.take_draw_commands();
      const commands = JSON.parse(cmdsJson);
      renderCommands(this.ctx, commands, this.canvas.width, this.canvas.height);

      this.clearError();
      this.onFrameComplete?.();
      return cmdsJson;
    } catch (err: any) {
      this.showError(String(err));
      this.errored = true;
      return "[]";
    }
  }

  private showError(msg: string): void {
    if (!this.errorEl) return;

    const match = msg.match(/\[line (\d+)(?:, column (\d+))?\]/);
    const line = match ? parseInt(match[1], 10) : null;

    if (line !== null && this.currentSource) {
      const lines = this.currentSource.split("\n");
      const start = Math.max(0, line - 4);
      const end = Math.min(lines.length, line + 3);

      let snippetHtml = "";
      for (let i = start; i < end; i++) {
        const lineNum = i + 1;
        const escaped = lines[i]
          .replace(/&/g, "&amp;")
          .replace(/</g, "&lt;")
          .replace(/>/g, "&gt;");
        const cls = lineNum === line ? ' class="error-line"' : "";
        snippetHtml += `<div${cls}><span class="line-num">${String(lineNum).padStart(3)}</span> ${escaped}</div>`;
      }
      const cleanMsg = msg.replace(/\s*\[line \d+(?:, column \d+)?\]/, "");
      this.errorEl.innerHTML =
        `<div class="error-header">${cleanMsg.replace(/</g, "&lt;")} <span class="error-loc">line ${line}</span></div>` +
        `<div class="error-source">${snippetHtml}</div>`;
    } else {
      this.errorEl.textContent = msg;
    }
    this.errorEl.style.display = "block";
  }

  private clearError(): void {
    if (this.errorEl) this.errorEl.style.display = "none";
  }
}
