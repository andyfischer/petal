import { PetalCanvas } from "./runtime.js";
import { SourceEditor } from "./editor.js";
import { PetalDebugAPI } from "./debug.js";
import { DebugPanel, injectDebugStyles } from "./debug-panel.js";
import { connectDebugWebSocket } from "./debug-ws.js";

const EXAMPLES = [
  { name: "Flowchart", path: "/examples/flowchart.ptl" },
  { name: "Org Chart", path: "/examples/org-chart.ptl" },
  { name: "Interactive", path: "/examples/interactive.ptl" },
];

async function main() {
  const canvas = document.getElementById("canvas") as HTMLCanvasElement;
  const errorEl = document.getElementById("error-display") as HTMLElement;
  const picker = document.getElementById("example-picker") as HTMLSelectElement;
  const toggleBtn = document.getElementById("toggle-source") as HTMLButtonElement;
  const toggleDebugBtn = document.getElementById("toggle-debug") as HTMLButtonElement;
  const editorPanel = document.getElementById("editor-panel") as HTMLElement;
  const debugPanelEl = document.getElementById("debug-panel") as HTMLElement;

  // Populate example picker
  for (const ex of EXAMPLES) {
    const opt = document.createElement("option");
    opt.value = ex.path;
    opt.textContent = ex.name;
    picker.appendChild(opt);
  }

  // Init runtime
  const petal = new PetalCanvas();
  await petal.init();
  petal.start(canvas, errorEl);

  // --- Debug API ---
  const debugApi = new PetalDebugAPI({
    runOneFrame: (dt) => petal.runOneFrame(dt),
    getCanvas: () => petal.canvas,
    getRuntime: () => petal.runtime,
    getController: () => petal.debug,
    getFrameCount: () => petal.frameCount,
  });

  // Expose on window for console access
  (window as any).petalDebug = debugApi;

  // --- Debug panel ---
  injectDebugStyles();
  const debugPanel = new DebugPanel(debugPanelEl, debugApi);
  let debugVisible = false;

  // Refresh debug panel after each frame
  petal.onFrameComplete = () => {
    if (debugVisible) debugPanel.refresh();
  };

  toggleDebugBtn.addEventListener("click", () => {
    debugVisible = !debugVisible;
    toggleDebugBtn.classList.toggle("active", debugVisible);
    debugPanelEl.classList.toggle("visible", debugVisible);
    if (debugVisible) debugPanel.refresh();
    requestAnimationFrame(() => petal.resize());
  });

  // --- WebSocket bridge ---
  connectDebugWebSocket(debugApi, 4012);

  // Current source tracking
  let currentSource = "";
  let editor: SourceEditor | null = null;
  let sourceVisible = false;

  // Load example on selection
  picker.addEventListener("change", async () => {
    const path = picker.value;
    if (!path) return;
    try {
      const resp = await fetch(path);
      currentSource = await resp.text();
      petal.load(currentSource);
      if (editor && sourceVisible) {
        editor.setSource(currentSource);
      }
    } catch (err) {
      errorEl.textContent = String(err);
      errorEl.style.display = "block";
    }
  });

  // Toggle source editor
  toggleBtn.addEventListener("click", () => {
    sourceVisible = !sourceVisible;
    toggleBtn.classList.toggle("active", sourceVisible);

    if (sourceVisible) {
      editorPanel.classList.add("visible");
      if (!editor) {
        editor = new SourceEditor(editorPanel, (source) => {
          currentSource = source;
          petal.load(source);
        });
      }
      editor.setSource(currentSource);
    } else {
      editorPanel.classList.remove("visible");
    }

    // Let layout settle, then resize canvas
    requestAnimationFrame(() => petal.resize());
  });

  // Auto-load first example
  if (EXAMPLES.length > 0) {
    picker.value = EXAMPLES[0].path;
    picker.dispatchEvent(new Event("change"));
  }
}

main();
