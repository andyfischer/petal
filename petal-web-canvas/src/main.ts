import { PetalCanvas } from "./runtime.js";

interface Example {
  name: string;
  path: string;
}

const EXAMPLES: Example[] = [
  { name: "Starfield", path: "/examples/starfield.ptl" },
  { name: "Bouncing Balls", path: "/examples/bouncing_balls.ptl" },
  { name: "Paint", path: "/examples/paint.ptl" },
  { name: "Flow Field", path: "/examples/flow_field.ptl" },
  { name: "Snake", path: "/examples/snake.ptl" },
];

async function main() {
  const canvas = document.getElementById("canvas") as HTMLCanvasElement;
  const errorEl = document.getElementById("error-display") as HTMLElement;
  const list = document.getElementById("example-list") as HTMLElement;

  const petal = new PetalCanvas();
  await petal.init();
  petal.start(canvas, errorEl);

  let currentBtn: HTMLButtonElement | null = null;

  async function loadExample(ex: Example, btn: HTMLButtonElement) {
    try {
      const resp = await fetch(ex.path);
      if (!resp.ok) throw new Error(`Failed to load ${ex.path}: ${resp.status}`);
      const source = await resp.text();
      petal.load(source);
      currentBtn?.classList.remove("active");
      btn.classList.add("active");
      currentBtn = btn;
      canvas.focus();
    } catch (err: any) {
      errorEl.textContent = String(err);
      errorEl.style.display = "block";
    }
  }

  for (const ex of EXAMPLES) {
    const btn = document.createElement("button");
    btn.className = "example-btn";
    btn.textContent = ex.name;
    btn.addEventListener("click", () => loadExample(ex, btn));
    list.appendChild(btn);
  }

  // Auto-load first example
  if (EXAMPLES.length > 0) {
    const firstBtn = list.firstElementChild as HTMLButtonElement;
    loadExample(EXAMPLES[0], firstBtn);
  }
}

main().catch((e) => {
  console.error(e);
  const errorEl = document.getElementById("error-display") as HTMLElement;
  errorEl.textContent = `Init error: ${e.message ?? e}`;
  errorEl.style.display = "block";
});
