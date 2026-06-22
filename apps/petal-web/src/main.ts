import { PetalWeb } from "./runtime.js";

async function main() {
  const app = document.getElementById("app")!;
  const output = document.getElementById("output")!;

  app.textContent = "Initializing WASM...";

  const petal = new PetalWeb();
  await petal.init();

  app.textContent = "Loading example...";

  // Fetch the example Petal source
  const response = await fetch("/examples/menu.ptl");
  const source = await response.text();

  petal.load(source);
  petal.mount(app, output);
  petal.render();
}

main().catch((e) => {
  console.error(e);
  document.getElementById("app")!.innerHTML =
    `<pre style="color:red">Error: ${e.message ?? e}</pre>`;
});
