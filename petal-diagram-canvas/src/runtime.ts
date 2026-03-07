/** PetalCanvas — wraps PetalRuntime for canvas-based frame-loop execution. */

import init, { PetalRuntime } from "../pkg/petal.js";
import { renderCommands, type DrawCommand } from "./canvas-renderer.js";
import { InputTracker } from "./input.js";
import { DebugController } from "./debug.js";

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

  /** Debug controller — manages pause/step/resume. */
  debug = new DebugController();
  /** Called after each frame completes (used by debug panel). */
  onFrameComplete: (() => void) | null = null;

  async init(): Promise<void> {
    await init();
    this.runtime = new PetalRuntime();
    this.input.setRuntime(this.runtime);
  }

  load(source: string): void {
    if (!this.runtime) throw new Error("Runtime not initialized");
    this.currentSource = source;

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

    // Restart the frame loop if not already running
    if (this.animId === null) {
      this.loop();
    }
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

  stop(): void {
    if (this.animId !== null) {
      cancelAnimationFrame(this.animId);
      this.animId = null;
    }
  }

  resize(): void {
    this.resizeCanvas();
  }

  /** Run a single frame with given dt. Returns draw commands JSON. Used by debug step(). */
  runOneFrame(dt: number): string {
    if (!this.runtime || this.stackId === null || !this.ctx || !this.canvas) {
      return "[]";
    }

    try {
      this.runtime.begin_frame();
      this.input.feedToRuntime(this.runtime);
      this.runtime.set_frame_info(dt, this.frameCount, this.canvas.width, this.canvas.height);
      this.runtime.reset_and_run(this.stackId);
      this.frameCount++;

      const cmdsJson = this.runtime.take_draw_commands();
      const commands: DrawCommand[] = JSON.parse(cmdsJson);
      renderCommands(this.ctx, commands, this.canvas.width, this.canvas.height);

      this.clearError();
      this.onFrameComplete?.();
      return cmdsJson;
    } catch (err: any) {
      this.showError(String(err));
      return "[]";
    }
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

    // Check with debug controller whether we should run this frame
    const dt = this.debug.shouldRunFrame(realDt);
    if (dt === null) return; // paused, skip frame body but keep rAF alive

    try {
      this.runtime.begin_frame();
      this.input.feedToRuntime(this.runtime);
      this.runtime.set_frame_info(dt, this.frameCount, this.canvas.width, this.canvas.height);
      this.runtime.reset_and_run(this.stackId);
      this.frameCount++;

      const cmdsJson = this.runtime.take_draw_commands();
      const commands: DrawCommand[] = JSON.parse(cmdsJson);
      renderCommands(this.ctx, commands, this.canvas.width, this.canvas.height);

      this.clearError();
      this.onFrameComplete?.();
    } catch (err: any) {
      this.showError(String(err));
      this.errored = true;
    }
  };

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

      // Strip the [line N, column M] suffix for the header
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
    if (this.errorEl) {
      this.errorEl.style.display = "none";
    }
  }
}
