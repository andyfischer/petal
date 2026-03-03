import { PetalCanvas } from "./runtime.js";
import { SourceEditor } from "./editor.js";

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
  const editorPanel = document.getElementById("editor-panel") as HTMLElement;

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
