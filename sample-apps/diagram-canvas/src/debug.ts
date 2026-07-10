/** Debug protocol for petal-diagram-canvas.
 *
 * DebugController — manages pause/step/resume state for the frame loop.
 * PetalDebugAPI   — high-level API exposed as window.petalDebug and used
 *                   by the WebSocket bridge.
 */

import type { PetalRuntime } from "petal-web-canvas";

// ---------------------------------------------------------------------------
// DebugController
// ---------------------------------------------------------------------------

export class DebugController {
  paused = false;
  stepsRemaining = 0;

  /** Returns the dt to use for this frame, or null to skip. */
  shouldRunFrame(realDt: number): number | null {
    if (!this.paused) return realDt;
    if (this.stepsRemaining > 0) {
      this.stepsRemaining--;
      return 1 / 60; // fixed dt for deterministic stepping
    }
    return null; // skip frame
  }
}

// ---------------------------------------------------------------------------
// PetalDebugAPI — exposed as window.petalDebug
// ---------------------------------------------------------------------------

export interface DebugHooks {
  /** Run one frame with a given dt, returning draw commands JSON. */
  runOneFrame(dt: number): string;
  /** Get the canvas element. */
  getCanvas(): HTMLCanvasElement | null;
  /** Get the PetalRuntime. */
  getRuntime(): PetalRuntime | null;
  /** Get the DebugController. */
  getController(): DebugController;
  /** Get current frame count. */
  getFrameCount(): number;
}

export class PetalDebugAPI {
  private hooks: DebugHooks;

  constructor(hooks: DebugHooks) {
    this.hooks = hooks;
  }

  pause(): DebugResponse {
    this.hooks.getController().paused = true;
    this.hooks.getController().stepsRemaining = 0;
    return this.buildResponse();
  }

  resume(): DebugResponse {
    this.hooks.getController().paused = false;
    this.hooks.getController().stepsRemaining = 0;
    return this.buildResponse();
  }

  step(n = 1): DebugResponse {
    const ctrl = this.hooks.getController();
    ctrl.paused = true;
    // Run N frames synchronously right now
    const allCmds: any[] = [];
    for (let i = 0; i < n; i++) {
      const cmdsJson = this.hooks.runOneFrame(1 / 60);
      try {
        allCmds.push(...JSON.parse(cmdsJson));
      } catch {}
    }
    return this.buildResponse({ draw_commands: allCmds });
  }

  state(): DebugResponse {
    const runtime = this.hooks.getRuntime();
    if (!runtime) return this.buildResponse();
    try {
      const stateJson = runtime.get_state_json();
      return this.buildResponse({ state: JSON.parse(stateJson) });
    } catch (e) {
      return this.buildResponse({ state: {} });
    }
  }

  setState(name: string, value: any): DebugResponse {
    const runtime = this.hooks.getRuntime();
    if (!runtime) return this.buildResponse();
    runtime.set_state_json(name, JSON.stringify(value));
    return this.state();
  }

  captureDrawCommands(): DebugResponse {
    const runtime = this.hooks.getRuntime();
    if (!runtime) return this.buildResponse();
    try {
      const cmdsJson = runtime.run_speculative();
      return this.buildResponse({ draw_commands: JSON.parse(cmdsJson) });
    } catch (e) {
      return this.buildResponse({ draw_commands: [] });
    }
  }

  input(opts: { keysDown?: string[]; mouse?: { x: number; y: number; buttons?: number[] } }): DebugResponse {
    const runtime = this.hooks.getRuntime();
    if (!runtime) return this.buildResponse();
    if (opts.keysDown) {
      // Clear all keys first, then set the ones specified
      for (const key of opts.keysDown) {
        runtime.set_key_state(key, true);
      }
    }
    if (opts.mouse) {
      runtime.set_mouse_position(opts.mouse.x, opts.mouse.y);
      if (opts.mouse.buttons) {
        for (const btn of opts.mouse.buttons) {
          runtime.set_mouse_button(btn, true);
        }
      }
    }
    return this.buildResponse();
  }

  screenshot(): DebugResponse {
    const canvas = this.hooks.getCanvas();
    if (!canvas) return this.buildResponse();
    const dataUrl = canvas.toDataURL("image/png");
    return this.buildResponse({ screenshot: dataUrl });
  }

  isPaused(): boolean {
    return this.hooks.getController().paused;
  }

  frameCount(): number {
    return this.hooks.getFrameCount();
  }

  /** Handle a JSON command from WebSocket or console. */
  handleCommand(cmd: DebugCommand): DebugResponse {
    switch (cmd.cmd) {
      case "pause": return this.pause();
      case "resume": return this.resume();
      case "step": return this.step(cmd.n ?? 1);
      case "state": return this.state();
      case "set_state": return this.setState(cmd.name!, cmd.value);
      case "capture_draw_commands": return this.captureDrawCommands();
      case "input": return this.input({ keysDown: cmd.keys_down, mouse: cmd.mouse });
      case "screenshot": return this.screenshot();
      default: return { ok: false, error: `Unknown command: ${(cmd as any).cmd}` } as any;
    }
  }

  private buildResponse(extra?: Partial<DebugResponse>): DebugResponse {
    return {
      ok: true,
      paused: this.hooks.getController().paused,
      frame: this.hooks.getFrameCount(),
      ...extra,
    };
  }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface DebugCommand {
  cmd: "pause" | "resume" | "step" | "state" | "set_state" | "capture_draw_commands" | "input" | "screenshot";
  n?: number;
  name?: string;
  value?: any;
  keys_down?: string[];
  mouse?: { x: number; y: number; buttons?: number[] };
}

export interface DebugResponse {
  ok: boolean;
  paused: boolean;
  frame: number;
  state?: Record<string, any>;
  draw_commands?: any[];
  output?: string[];
  screenshot?: string;
  error?: string;
}
