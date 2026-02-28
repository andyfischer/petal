/** Render a Petal element tree (JSON) to an HTML string. */

interface ElementNode {
  type: "element";
  tag: string;
  props: Record<string, unknown>;
  children: unknown[];
}

function isElement(node: unknown): node is ElementNode {
  return (
    typeof node === "object" &&
    node !== null &&
    (node as ElementNode).type === "element"
  );
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Void elements that must not have a closing tag. */
const VOID_ELEMENTS = new Set([
  "area", "base", "br", "col", "embed", "hr", "img", "input",
  "link", "meta", "param", "source", "track", "wbr",
]);

function renderNode(node: unknown): string {
  if (typeof node === "string") return escapeHtml(node);
  if (typeof node === "number") return String(node);
  if (node === null || node === undefined) return "";

  if (!isElement(node)) return "";

  const { tag, props, children } = node;
  let html = `<${tag}`;

  for (const [key, value] of Object.entries(props)) {
    // Map Petal's "eid" prop to "data-eid" for DOM event delegation
    const attrName = key === "eid" ? "data-eid" : key;
    if (value === true) {
      html += ` ${attrName}`;
    } else if (value !== false && value != null) {
      html += ` ${attrName}="${escapeHtml(String(value))}"`;
    }
  }

  if (VOID_ELEMENTS.has(tag)) {
    html += " />";
    return html;
  }

  html += ">";
  for (const child of children) {
    // Flatten arrays (e.g. from map() producing a list of elements)
    if (Array.isArray(child)) {
      for (const item of child) {
        html += renderNode(item);
      }
    } else {
      html += renderNode(child);
    }
  }
  html += `</${tag}>`;
  return html;
}

export function renderToContainer(container: HTMLElement, elementJson: unknown): void {
  if (isElement(elementJson)) {
    container.innerHTML = renderNode(elementJson);
  } else if (Array.isArray(elementJson)) {
    container.innerHTML = elementJson.map(renderNode).join("");
  } else {
    container.innerHTML = String(elementJson ?? "");
  }
}
