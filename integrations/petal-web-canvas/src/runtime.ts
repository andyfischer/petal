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

  // --- Host→script prop feed (built on set_state_json) ---
  //
  // A generic, one-way data channel: the host stages named props, and each
  // frame — just before the run — every prop whose value changed is pushed
  // into the script's like-named `state` variable. This is the "controlled
  // prop" model: the host owns the value, the script reads it. Because
  // `set_state` writes committed state and `reset_stack` preserves it, a prop
  // set before the first run also wins over the script's `state x = <init>`
  // initializer, so the script never flashes a default.
  //
  //   host (TS):    canvas.setProp("cubeState", cube)   // any JSON value
  //   script (ptl): state cubeState = {}                // read each frame
  //
  /** Last JSON pushed per prop — the dirty-check baseline. */
  private propBaseline = new Map<string, string>();
  /** Props changed since the last flush, awaiting the next frame. */
  private propPending = new Map<string, string>();
  /** Prop names already warned about (missing `state` var), to avoid spam. */
  private propWarned = new Set<string>();

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

  /**
   * Stage a host-owned prop for the script. `value` is any JSON-serializable
   * value; it's pushed into the script's `state <name>` variable just before
   * the next frame runs. Pushes are deduped by serialized value, so calling
   * this every frame with an unchanged value costs nothing. Safe to call
   * before `load()` — staged props flush on the first frame.
   *
   * The script must declare `state <name> = <default>`; if it doesn't, the
   * push is skipped and warned once (the script simply won't see the prop).
   */
  setProp(name: string, value: unknown): void {
    const json = JSON.stringify(value ?? null);
    if (this.propBaseline.get(name) === json) {
      this.propPending.delete(name); // reverted to the last-pushed value
      return;
    }
    this.propPending.set(name, json);
  }

  /** Stage several props at once — `setProp` for each key. */
  setProps(props: Record<string, unknown>): void {
    for (const [name, value] of Object.entries(props)) this.setProp(name, value);
  }

  /**
   * Read the script's committed state back as a plain object keyed by state
   * variable name. The inverse of `setProp`, for hosts that need to observe
   * script-owned state (debug panels, two-way sync). Returns `{}` before a
   * program is loaded.
   */
  getState(): Record<string, unknown> {
    if (!this.runtime || this.stackId === null) return {};
    try {
      return JSON.parse(this.runtime.get_state_json());
    } catch {
      return {};
    }
  }

  /**
   * Push every pending prop into the script's committed state. Called once per
   * frame, before `reset_and_run` (which preserves state). A prop naming a
   * non-existent `state` var throws from the runtime; we warn once and drop it
   * so one bad name can't stall the others or spam the console.
   */
  private flushProps(): void {
    if (!this.runtime || this.stackId === null || this.propPending.size === 0) return;
    for (const [name, json] of this.propPending) {
      try {
        this.runtime.set_state_json(name, json);
        this.propBaseline.set(name, json);
      } catch (err) {
        if (!this.propWarned.has(name)) {
          this.propWarned.add(name);
          console.warn(`[petal] setProp("${name}") skipped: ${String(err)}`);
        }
      }
    }
    this.propPending.clear();
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

    // A fresh stack starts with empty state, so re-stage every known prop to
    // push onto it. Clear the missing-var warnings too — the reloaded program
    // may now declare a `state` var it previously lacked.
    for (const [name, json] of this.propBaseline) this.propPending.set(name, json);
    this.propBaseline.clear();
    this.propWarned.clear();

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
      // Absolute clock in seconds since page load (monotonic, read fresh — not
      // an accumulation of dt), backing time()/elapsed() in the script.
      const time = performance.now() / 1000;
      this.runtime.set_frame_info(
        dt,
        time,
        this.frameCount,
        this.canvas.width,
        this.canvas.height,
      );
      this.runtime.begin_frame();
      // Push host-owned props into committed state, then run. reset_and_run's
      // reset preserves state, so the values reach this frame's run.
      this.flushProps();
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
