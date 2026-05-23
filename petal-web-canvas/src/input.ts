/** Track mouse and keyboard input, feed state to the WASM runtime each frame. */

import type { PetalRuntime } from "../pkg/petal_web_canvas.js";

// Map DOM key names to petal-sdl key names
const KEY_MAP: Record<string, string> = {
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  Enter: "return",
  Escape: "escape",
  Backspace: "backspace",
  Tab: "tab",
  " ": "space",
  Shift: "shift",
  Control: "ctrl",
  Alt: "alt",
};

function mapKey(key: string): string {
  if (KEY_MAP[key]) return KEY_MAP[key];
  // Single lowercase letter or digit
  if (key.length === 1) return key.toLowerCase();
  return key.toLowerCase();
}

export class InputTracker {
  mouseX = 0;
  mouseY = 0;
  private keysDown = new Set<string>();

  attach(canvas: HTMLCanvasElement): void {
    canvas.addEventListener("mousemove", (e) => {
      this.mouseX = e.offsetX;
      this.mouseY = e.offsetY;
    });

    canvas.addEventListener("mousedown", (e) => {
      this.feedMouseButton(e.button, true);
    });

    canvas.addEventListener("mouseup", (e) => {
      this.feedMouseButton(e.button, false);
    });

    // Use window for keyboard so canvas doesn't need focus tricks
    window.addEventListener("keydown", (e) => {
      const name = mapKey(e.key);
      this.keysDown.add(name);
    });

    window.addEventListener("keyup", (e) => {
      const name = mapKey(e.key);
      this.keysDown.delete(name);
    });
  }

  private runtime: PetalRuntime | null = null;
  private mouseButtonsDown = new Set<number>();

  setRuntime(runtime: PetalRuntime): void {
    this.runtime = runtime;
  }

  private feedMouseButton(button: number, down: boolean): void {
    if (down) {
      this.mouseButtonsDown.add(button);
    } else {
      this.mouseButtonsDown.delete(button);
    }
    this.runtime?.set_mouse_button(button, down);
  }

  /** Push current input state to the WASM runtime. */
  feedToRuntime(runtime: PetalRuntime): void {
    runtime.set_mouse_position(this.mouseX, this.mouseY);
    // Re-set all button states
    for (const btn of this.mouseButtonsDown) {
      runtime.set_mouse_button(btn, true);
    }
    // Key states are set via keydown/keyup events above,
    // but we need to push them each frame for the runtime
    for (const key of this.keysDown) {
      runtime.set_key_state(key, true);
    }
  }
}
