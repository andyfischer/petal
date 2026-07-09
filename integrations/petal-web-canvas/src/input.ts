/** Translate browser input events into the petal-ui InputState in the runtime.
 *
 * Events are fed to the WASM runtime *as they arrive* (not re-pushed each
 * frame): the runtime's petal-ui `InputState` latches press/release edges until
 * the next `begin_frame` promotes them, so a click that goes down and up
 * between two animation frames still fires `mouse_pressed`. */

import type { PetalRuntime } from "../pkg/petal_web_canvas.js";

// Map DOM key names to canonical petal-ui key names.
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
  Meta: "cmd",
};

function mapKey(key: string): string {
  if (KEY_MAP[key]) return KEY_MAP[key];
  // Single character (letter, digit, or symbol like "=" / "-") → lowercase.
  return key.toLowerCase();
}

export class InputTracker {
  private runtime: PetalRuntime | null = null;

  setRuntime(runtime: PetalRuntime): void {
    this.runtime = runtime;
  }

  attach(canvas: HTMLCanvasElement): void {
    canvas.addEventListener("mousemove", (e) => {
      this.runtime?.set_mouse_position(e.offsetX, e.offsetY);
    });

    canvas.addEventListener("mousedown", (e) => {
      this.runtime?.set_mouse_button(e.button, true);
    });

    // Listen on window for mouseup so a drag that releases off-canvas still
    // clears the button (and its edge is still delivered).
    window.addEventListener("mouseup", (e) => {
      this.runtime?.set_mouse_button(e.button, false);
    });

    // Suppress the context menu so right-click (button 2) is usable by sketches.
    canvas.addEventListener("contextmenu", (e) => e.preventDefault());

    canvas.addEventListener("wheel", (e) => {
      // Wheel deltas are in pixels; petal-ui scroll is in lines (~40px/line).
      this.runtime?.scroll(-e.deltaX / 40, -e.deltaY / 40);
      e.preventDefault();
    }, { passive: false });

    // Use window for keyboard so the canvas doesn't need focus tricks.
    window.addEventListener("keydown", (e) => {
      this.runtime?.set_key_state(mapKey(e.key), true);
      // Feed printable characters as typed text for text_input().
      if (e.key.length === 1) this.runtime?.type_text(e.key);
    });

    window.addEventListener("keyup", (e) => {
      this.runtime?.set_key_state(mapKey(e.key), false);
    });
  }
}
