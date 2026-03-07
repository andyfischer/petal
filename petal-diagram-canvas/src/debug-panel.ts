/** Debug panel UI — collapsible sidebar panel with playback controls and state inspector. */

import type { PetalDebugAPI } from "./debug.js";

export class DebugPanel {
  private container: HTMLElement;
  private api: PetalDebugAPI;
  private stateEl: HTMLElement;
  private frameEl: HTMLElement;
  private pauseBtn: HTMLButtonElement;
  private outputEl: HTMLElement;

  constructor(container: HTMLElement, api: PetalDebugAPI) {
    this.container = container;
    this.api = api;

    container.innerHTML = `
      <div class="debug-toolbar">
        <button class="debug-btn" data-action="pause">Pause</button>
        <button class="debug-btn" data-action="step">Step</button>
        <button class="debug-btn" data-action="resume">Resume</button>
        <span class="debug-frame">Frame: <span data-frame>0</span></span>
      </div>
      <div class="debug-section">
        <h4>State</h4>
        <pre class="debug-state"></pre>
      </div>
      <div class="debug-section">
        <h4>Output</h4>
        <pre class="debug-output"></pre>
      </div>
    `;

    this.stateEl = container.querySelector(".debug-state")!;
    this.frameEl = container.querySelector("[data-frame]")!;
    this.pauseBtn = container.querySelector('[data-action="pause"]')!;
    this.outputEl = container.querySelector(".debug-output")!;

    container.querySelector('[data-action="pause"]')!.addEventListener("click", () => {
      this.api.pause();
      this.refresh();
    });
    container.querySelector('[data-action="step"]')!.addEventListener("click", () => {
      this.api.step(1);
      this.refresh();
    });
    container.querySelector('[data-action="resume"]')!.addEventListener("click", () => {
      this.api.resume();
      this.refresh();
    });
  }

  refresh(): void {
    this.frameEl.textContent = String(this.api.frameCount());

    const resp = this.api.state();
    if (resp.state) {
      this.stateEl.textContent = JSON.stringify(resp.state, null, 2);
    }

    this.pauseBtn.textContent = this.api.isPaused() ? "Paused" : "Pause";
  }
}

/** Inject styles for the debug panel. */
export function injectDebugStyles(): void {
  const style = document.createElement("style");
  style.textContent = `
    #debug-panel {
      display: none;
      width: 300px;
      min-width: 200px;
      border-left: 1px solid #2a2a44;
      background: #16162a;
      flex-direction: column;
      overflow-y: auto;
      padding: 12px;
      gap: 12px;
      font-size: 13px;
    }
    #debug-panel.visible {
      display: flex;
    }
    .debug-toolbar {
      display: flex;
      gap: 6px;
      align-items: center;
      flex-wrap: wrap;
    }
    .debug-btn {
      padding: 4px 10px;
      border-radius: 4px;
      border: 1px solid #444;
      background: #2a2a3e;
      color: #e0e0e0;
      font-size: 12px;
      cursor: pointer;
    }
    .debug-btn:hover { border-color: #666; background: #333350; }
    .debug-frame {
      margin-left: auto;
      font-size: 11px;
      color: #888;
    }
    .debug-section h4 {
      font-size: 11px;
      text-transform: uppercase;
      letter-spacing: 1px;
      color: #888;
      margin-bottom: 6px;
    }
    .debug-state, .debug-output {
      background: #111;
      border-radius: 4px;
      padding: 8px;
      font-family: monospace;
      font-size: 12px;
      color: #ccc;
      white-space: pre-wrap;
      word-break: break-all;
      max-height: 300px;
      overflow-y: auto;
      margin: 0;
    }
  `;
  document.head.appendChild(style);
}
