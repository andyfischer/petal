// Bundle a transpiled module and render it.
//
//   node render.mjs <entry.jsx> [--export Name] [--prop k=v ...] [--click "selector" ...] [--html]
//
// Renders the module's default export (or `--export Name`) into a jsdom
// document with react-dom/client, clicks each `--click` selector in turn
// (a selector may be `text=Label` to click the element whose text is
// Label), and prints a JSON report: the text, the number of elements, any
// errors React logged, and `--html` the markup. Exits 1 on an error.
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { pathToFileURL } from "node:url";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { createRequire } from "node:module";

const args = process.argv.slice(2);
const entry = resolve(args[0]);
let exportName = "default";
const clicks = [];
const props = {};
let html = false;
for (let i = 1; i < args.length; i++) {
  if (args[i] === "--export") exportName = args[++i];
  else if (args[i] === "--click") clicks.push(args[++i]);
  else if (args[i] === "--prop") { const [k, v] = args[++i].split("="); props[k] = v; }
  else if (args[i] === "--html") html = true;
}

const here = dirname(new URL(import.meta.url).pathname);
const out = join(mkdtempSync(join(tmpdir(), "petal-react-")), "bundle.mjs");
await build({
  entryPoints: [entry],
  bundle: true,
  format: "esm",
  platform: "node",
  outfile: out,
  jsx: "automatic",
  loader: { ".js": "jsx" },
  nodePaths: [join(here, "node_modules")],
  external: ["react", "react-dom", "react-dom/client", "react/jsx-runtime"],
  logLevel: "error",
});
// The bundle imports react from this folder's node_modules.
const req = createRequire(join(here, "package.json"));
const shim = out.replace(/bundle\.mjs$/, "run.mjs");
writeFileSync(shim, (await import("node:fs")).readFileSync(out, "utf8")
  .replaceAll('from "react/jsx-runtime"', `from ${JSON.stringify(pathToFileURL(req.resolve("react/jsx-runtime")).href)}`)
  .replaceAll('from "react-dom/client"', `from ${JSON.stringify(pathToFileURL(req.resolve("react-dom/client")).href)}`)
  .replaceAll('from "react"', `from ${JSON.stringify(pathToFileURL(req.resolve("react")).href)}`));

const dom = new JSDOM("<!doctype html><div id=root></div>", { pretendToBeVisual: true });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.HTMLElement = dom.window.HTMLElement;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const errors = [];
const origError = console.error;
console.error = (...a) => { errors.push(a.map(String).join(" ")); };

const React = await import(pathToFileURL(req.resolve("react")).href);
const { createRoot } = await import(pathToFileURL(req.resolve("react-dom/client")).href);
let mod;
try {
  mod = await import(pathToFileURL(shim).href);
} catch (e) {
  console.error = origError;
  console.log(JSON.stringify({ ok: false, stage: "import", error: String(e.stack ?? e) }));
  process.exit(1);
}
const C = mod[exportName];
if (typeof C !== "function") {
  console.log(JSON.stringify({ ok: false, stage: "export", error: `no export ${exportName}`, exports: Object.keys(mod) }));
  process.exit(1);
}
const root = createRoot(document.getElementById("root"));
const steps = [];
try {
  await React.act(async () => root.render(React.createElement(C, props)));
  steps.push({ after: "render", text: document.body.textContent });
  for (const sel of clicks) {
    let el;
    if (sel.startsWith("text=")) {
      const want = sel.slice(5);
      el = [...document.querySelectorAll("*")].reverse().find((n) => n.textContent.trim() === want);
    } else el = document.querySelector(sel);
    if (!el) throw new Error(`nothing matches ${sel}`);
    await React.act(async () => el.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })));
    steps.push({ after: `click ${sel}`, text: document.body.textContent });
  }
} catch (e) {
  console.error = origError;
  console.log(JSON.stringify({ ok: false, stage: "render", error: String(e.stack ?? e), errors }));
  process.exit(1);
}
console.error = origError;
const report = {
  ok: errors.length === 0,
  elements: document.querySelectorAll("#root *").length,
  steps,
  errors,
};
if (html) report.html = document.getElementById("root").innerHTML;
console.log(JSON.stringify(report, null, 1));
process.exit(report.ok ? 0 : 1);
