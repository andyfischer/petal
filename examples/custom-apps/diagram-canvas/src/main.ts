import { PetalCanvas } from "petal-web-canvas";
import { SourceEditor } from "./editor.js";
import { PetalDebugAPI, DebugController } from "./debug.js";
import { DebugPanel, injectDebugStyles } from "./debug-panel.js";
import { connectDebugWebSocket } from "./debug-ws.js";
import { EditMode } from "./edit-mode.js";

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
  const toggleEditBtn = document.getElementById("toggle-edit") as HTMLButtonElement;
  const editorPanel = document.getElementById("editor-panel") as HTMLElement;
  const debugPanelEl = document.getElementById("debug-panel") as HTMLElement;
  const editOverlay = document.getElementById("edit-overlay") as HTMLCanvasElement;
  const editPanelEl = document.getElementById("edit-panel") as HTMLElement;

  // Populate example picker
  for (const ex of EXAMPLES) {
    const opt = document.createElement("option");
    opt.value = ex.path;
    opt.textContent = ex.name;
    picker.appendChild(opt);
  }

  // Current source tracking
  let currentSource = "";
  let editor: SourceEditor | null = null;
  let sourceVisible = false;

  // Init runtime. The debug controller gates the shared frame loop for
  // pause/step, and edit mode freezes it; the shared PetalCanvas has no
  // debug notion of its own.
  const debug = new DebugController();
  const petal = new PetalCanvas();
  const editMode = new EditMode(petal, editOverlay, editPanelEl, {
    editor: () => (sourceVisible ? editor : null),
    source: () => currentSource,
    applySource: (source) => {
      // Into the editor first (as one undoable change), then run it now
      // rather than after the editor's debounce.
      if (editor) editor.replaceSource(source);
      run(source);
    },
  });
  petal.frameGate = (dt) => (editMode.active ? editMode.gate() : debug.shouldRunFrame(dt));
  await petal.init();
  petal.start(canvas, errorEl);

  /** Load `source` as the running program (a no-op when it already is). */
  function run(source: string, force = false): void {
    if (!force && source === currentSource) return;
    currentSource = source;
    petal.load(source);
    editMode.reloaded();
  }

  // --- Debug API ---
  const debugApi = new PetalDebugAPI({
    runOneFrame: (dt) => petal.runOneFrame(dt),
    getCanvas: () => petal.canvas,
    getRuntime: () => petal.runtime,
    getController: () => debug,
    getFrameCount: () => petal.frameCount,
  });

  // Expose on window for console access (and for tests: `petalEdit`).
  (window as any).petalDebug = debugApi;
  (window as any).petalEdit = editMode;
  (window as any).petalDiagram = {
    /** Run `source` (and show it in the editor, when open). */
    load: (source: string) => {
      run(source, true);
      if (editor && sourceVisible) editor.setSource(source);
    },
    source: () => currentSource,
    editorText: () => editor?.getSource() ?? null,
  };

  // --- Debug panel ---
  injectDebugStyles();
  const debugPanel = new DebugPanel(debugPanelEl, debugApi);
  let debugVisible = false;

  // Refresh debug panel after each frame
  petal.onFrameComplete = () => {
    if (debugVisible) debugPanel.refresh();
    if (editMode.active) editMode.render();
  };

  toggleDebugBtn.addEventListener("click", () => {
    debugVisible = !debugVisible;
    toggleDebugBtn.classList.toggle("active", debugVisible);
    debugPanelEl.classList.toggle("visible", debugVisible);
    if (debugVisible) debugPanel.refresh();
    requestAnimationFrame(() => petal.resize());
  });

  // --- Edit mode ---
  toggleEditBtn.addEventListener("click", () => {
    editMode.toggle();
    toggleEditBtn.classList.toggle("active", editMode.active);
  });

  // --- WebSocket bridge ---
  connectDebugWebSocket(debugApi, 4012);

  // Load example on selection
  picker.addEventListener("change", async () => {
    const path = picker.value;
    if (!path) return;
    try {
      const resp = await fetch(path);
      const source = await resp.text();
      run(source, true);
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
        editor = new SourceEditor(editorPanel, (source) => run(source));
        editor.onSelectionChange(() => editMode.render());
      }
      editor.setSource(currentSource);
      editor.setEditMode(editMode.active);
    } else {
      editorPanel.classList.remove("visible");
    }
    editMode.render();

    // Let layout settle, then resize canvas
    requestAnimationFrame(() => {
      petal.resize();
      if (editMode.active) editMode.requestFrame();
    });
  });

  // Auto-load first example
  if (EXAMPLES.length > 0) {
    picker.value = EXAMPLES[0].path;
    picker.dispatchEvent(new Event("change"));
  }
}

main();
