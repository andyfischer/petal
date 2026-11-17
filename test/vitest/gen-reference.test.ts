// Guards the generated API references (tools/gen-reference.ts): the Markdown
// they render from `petal doc --json`, the coverage check that runs before a
// hand-written reference is replaced, and that every generated reference in
// the repo is current (as CI's `npm run reference:check` does).

import { describe, it, expect } from "vitest";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  GENERATED_MARKER,
  TARGETS,
  coverage,
  orderSections,
  renderTarget,
  reportsFor,
  slug,
  type Target,
} from "../../tools/gen-reference";

const repoRoot = resolve(import.meta.dirname, "..", "..");

// The `petal doc` fixture package (cli/tests/doc_cli.rs): every kind of item,
// documented and not.
const fixture: Target = {
  id: "fixture",
  out: "cli/tests/fixtures/doc_pkg/REFERENCE.md",
  title: "`doc_pkg` reference",
  intro: "The fixture.",
  sources: [{ target: "cli/tests/fixtures/doc_pkg" }],
};
const reports = reportsFor(fixture);
const page = renderTarget(fixture, reports);

/** The text of the `### \`name\`` section, up to the next heading. */
function section(name: string): string {
  const start = page.indexOf(`### \`${name}\`\n`);
  expect(start, `no section for ${name}`).toBeGreaterThanOrEqual(0);
  const next = page.slice(start + 1).search(/\n#{2,3} /);
  return next < 0 ? page.slice(start) : page.slice(start, start + 1 + next);
}

describe("gen-reference", () => {
  it("starts with the do-not-edit marker, which renders as nothing", () => {
    expect(page.startsWith(GENERATED_MARKER + "\n")).toBe(true);
    // A link reference definition, not an HTML comment: the website renders
    // raw HTML in Markdown as text.
    expect(page).not.toMatch(/<!--|<sub>/);
  });

  it("opens with the package doc and lists the modules", () => {
    expect(page).toContain("# `doc_pkg` reference\n");
    expect(page).toContain("Shapes and sums, documented.");
    expect(page).toContain("- [`doc_pkg/shapes`](#doc_pkgshapes): Classes, enums and overloads.");
    // The root module's doc is the page's intro, shown once.
    expect(page.split("The root module re-exports").length - 1).toBe(1);
  });

  it("renders signatures, parameters, returns and the source line", () => {
    const scale = section("scale");
    expect(scale).toContain("```petal\nscale(xs, k = 2)\n```");
    expect(scale).toContain("- `xs`: The numbers.");
    expect(scale).toContain("**Returns** The scaled list.");
    expect(scale).toContain("*Source: [`cli/tests/fixtures/doc_pkg/src/doc_pkg.ptl:18`](src/doc_pkg.ptl#L18)*");
    const measure = section("measure");
    expect(measure).toContain("measure(text: string, size?: num, ...rest) -> float");
    expect(measure).toContain("- `text` (`string`): What to measure.");
  });

  it("renders options as a table and every example as code", () => {
    const outline = section("outline");
    // One @doc on the first form covers both.
    expect(outline).toContain("```petal\noutline(b: Box) -> nil\noutline(b: Box, style: record) -> nil\n```");
    expect(outline).toContain("| `fill` | The fill colour. |");
    expect(outline).toContain("**Examples**");
    expect(outline).toContain("```petal\noutline(Box(0, 0, 4, 4), {fill: :f00})\n```");
  });

  it("gives each documented form of an overload set its own doc", () => {
    const grow = section("grow");
    expect(grow).toMatch(/grow\(b, n\)\n```\n\nGrow by one amount\.\n\n```petal\ngrow\(b, dx, dy\)\n```\n\nGrow by two amounts\./);
  });

  it("marks deprecated and undocumented items", () => {
    expect(section("sum_all")).toContain("**Deprecated:** Use `total`.");
    expect(section("total")).toContain("*Undocumented.*");
  });

  it("links see: entries that name something on the page", () => {
    expect(page).toContain("**See also:** [`doc_pkg/shapes`](#doc_pkgshapes)");
  });

  it("leaves out hidden and private items", () => {
    expect(page).not.toContain("_plumbing");
    expect(page).not.toContain("private_helper");
  });

  it("renders classes, enums and bindings", () => {
    expect(section("Box")).toContain("class Box(x: num, y: num, w: num, h: num)");
    expect(section("Shape")).toContain("enum Shape { circle(r), square(side), dot }");
    expect(section("drawn")).toContain("state drawn: int = 0");
    expect(section("PHI")).toContain("The golden ratio.");
  });

  it("lists sections in a target's moduleOrder, the rest where `*` is", () => {
    const names = (xs: Array<{ name: string }>) => xs.map((x) => x.name);
    const secs = ["p", "p/a", "p/b", "p/c"].map((name) => ({ name }));
    expect(names(orderSections(secs))).toEqual(["p", "p/a", "p/b", "p/c"]);
    expect(names(orderSections(secs, ["p/c", "*", "p"]))).toEqual(["p/c", "p/a", "p/b", "p"]);
    expect(names(orderSections(secs, ["p/b"]))).toEqual(["p/b", "p", "p/a", "p/c"]);
    const ordered = renderTarget({ ...fixture, moduleOrder: ["doc_pkg/shapes"] }, reports);
    expect(ordered.indexOf("## `doc_pkg/shapes`")).toBeLessThan(ordered.indexOf("## `doc_pkg`\n"));
  });

  it("slugs headings the way GitHub does", () => {
    expect(slug("`ui/draw`")).toBe("uidraw");
    expect(slug("`Box.area`")).toBe("boxarea");
    expect(slug("`draw_rect`")).toBe("draw_rect");
  });

  it("finds what a hand-written reference has that no @doc carries", () => {
    const handWritten = [
      "# Old reference",
      "",
      "### `scale(xs, k)`",
      "",
      "Scale every element by `k`. Returns a new list.",
      "",
      "### `total(xs)`",
      "",
      "Adds the numbers up, which no @doc says.",
      "",
      "```petal",
      "outline(Box(0, 0, 4, 4))",
      "```",
    ].join("\n");
    const cov = coverage(handWritten, reports);
    expect(cov.undocumented).toEqual(["total"]);
    expect(cov.paragraphs).toBe(3);
    expect(cov.missing.map((m) => m.text)).toEqual(["Adds the numbers up, which no @doc says."]);
  });

  it("keeps every generated reference in the repo current", () => {
    // A target still written by hand is not checked (it is replaced with
    // `npm run reference -- --replace <id>` once its prose is in @doc).
    for (const target of TARGETS) {
      const path = join(repoRoot, target.out);
      if (!existsSync(path)) continue;
      const text = readFileSync(path, "utf8");
      if (!text.startsWith(GENERATED_MARKER)) continue;
      expect(text, `${target.out} is stale: run npm run reference`).toBe(renderTarget(target, reportsFor(target)));
    }
  });
});
