/** PetalCanvas — wraps PetalRuntime for canvas-based frame-loop execution. */

import init, { PetalRuntime } from "../pkg/petal.js";
import { renderCommands, type DrawCommand } from "./canvas-renderer.js";
import { InputTracker } from "./input.js";

export class PetalCanvas {
  private runtime: PetalRuntime | null = null;
  private stackId: number | null = null;
  private canvas: HTMLCanvasElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private input = new InputTracker();
  private animId: number | null = null;
  private frameCount = 0;
  private lastTime = 0;
  private errorEl: HTMLElement | null = null;

  async init(): Promise<void> {
    await init();
    this.runtime = new PetalRuntime();
    this.input.setRuntime(this.runtime);
  }

  load(source: string): void {
    if (!this.runtime) throw new Error("Runtime not initialized");
    this.stop();
    this.frameCount = 0;
    this.lastTime = performance.now();

    const programId = this.runtime.load_program(source);
    this.stackId = this.runtime.create_stack(programId);
    this.clearError();

    // Restart the frame loop
    this.loop();
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

    const now = performance.now();
    const dt = (now - this.lastTime) / 1000;
    this.lastTime = now;
    this.frameCount++;

    try {
      // 1. Begin frame (snapshot prev input for edge detection)
      this.runtime.begin_frame();

      // 2. Feed input state
      this.input.feedToRuntime(this.runtime);

      // 3. Set frame info
      this.runtime.set_frame_info(
        dt,
        this.frameCount,
        this.canvas.width,
        this.canvas.height,
      );

      // 4. Reset and run the program
      this.runtime.reset_and_run(this.stackId);

      // 5. Take draw commands
      const cmdsJson = this.runtime.take_draw_commands();
      const commands: DrawCommand[] = JSON.parse(cmdsJson);

      // 6. Render to canvas
      renderCommands(this.ctx, commands, this.canvas.width, this.canvas.height);

      this.clearError();
    } catch (err: any) {
      this.showError(String(err));
      this.stop();
    }
  };

  private showError(msg: string): void {
    if (this.errorEl) {
      this.errorEl.textContent = msg;
      this.errorEl.style.display = "block";
    }
  }

  private clearError(): void {
    if (this.errorEl) {
      this.errorEl.style.display = "none";
    }
  }
}
