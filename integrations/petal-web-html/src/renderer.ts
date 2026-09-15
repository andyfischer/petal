/**
 * Render a Petal element tree (JSON) into a container by patching the DOM
 * that is already there, rather than rebuilding it from a string.
 *
 * Every run of the script produces the whole tree again, so the renderer's job
 * is to make the live DOM match it with as few operations as possible: an
 * unchanged subtree costs one walk and no mutations, a changed attribute is
 * one `setAttribute`, a reordered list is a handful of `insertBefore`s. That
 * is what keeps focus, selection, scroll position and CSS transitions alive
 * across runs — an `innerHTML` rebuild destroyed all of them on every click.
 *
 * Children are reconciled by key when an element carries a `key` prop (kept
 * on the node as `data-key`), and by position + tag otherwise. Moves are done
 * with plain in-order `insertBefore`, which is O(n) per list and never wrong;
 * a longest-increasing-subsequence pass would cut the moves for large
 * reorders and can be added without changing the contract.
 */

interface ElementNode {
  type: "element";
  tag: string;
  props: Record<string, unknown>;
  children: unknown[];
}

/** A node the reconciler can place: an element vnode or a text run. */
type VNode = ElementNode | string;

function isElement(node: unknown): node is ElementNode {
  return (
    typeof node === "object" &&
    node !== null &&
    (node as ElementNode).type === "element"
  );
}

/** Void elements never receive children. */
const VOID_ELEMENTS = new Set([
  "area", "base", "br", "col", "embed", "hr", "img", "input",
  "link", "meta", "param", "source", "track", "wbr",
]);

/** The DOM attribute a Petal prop maps to. */
function attrName(prop: string): string {
  // `eid` drives the click delegation in runtime.ts; `key` is the reconciler's.
  if (prop === "eid") return "data-eid";
  if (prop === "key") return "data-key";
  return prop;
}

/** Flatten a children list (arrays from `map` nest one level or more) into
 *  placeable vnodes, dropping nil and folding adjacent text into one run so
 *  text nodes match one-to-one with what the DOM holds. */
function flattenChildren(children: unknown[], out: VNode[] = []): VNode[] {
  for (const child of children) {
    if (Array.isArray(child)) {
      flattenChildren(child, out);
    } else if (typeof child === "string" || typeof child === "number") {
      const text = String(child);
      const last = out.length - 1;
      if (last >= 0 && typeof out[last] === "string") {
        out[last] = (out[last] as string) + text;
      } else {
        out.push(text);
      }
    } else if (isElement(child)) {
      out.push(child);
    }
    // null / undefined / anything else: nothing to place.
  }
  return out;
}

function keyOf(vnode: ElementNode): string | null {
  const k = vnode.props.key;
  return k == null ? null : String(k);
}

/** Make `el`'s attributes match `props`: set what differs, drop what is gone. */
function patchAttributes(el: Element, props: Record<string, unknown>): void {
  const wanted = new Set<string>();
  for (const [prop, value] of Object.entries(props)) {
    const name = attrName(prop);
    if (value === false || value == null) continue;
    wanted.add(name);
    const text = value === true ? "" : String(value);
    if (el.getAttribute(name) !== text) el.setAttribute(name, text);
  }
  for (const name of el.getAttributeNames()) {
    if (!wanted.has(name)) el.removeAttribute(name);
  }
}

/** Create a fresh DOM node for `vnode`, children included. */
function createNode(vnode: VNode): Node {
  if (typeof vnode === "string") return document.createTextNode(vnode);
  const el = document.createElement(vnode.tag);
  patchAttributes(el, vnode.props);
  if (!VOID_ELEMENTS.has(vnode.tag)) patchChildren(el, vnode.children);
  return el;
}

/** Whether the existing DOM node can be patched into `vnode` in place. */
function canPatch(node: Node, vnode: VNode): boolean {
  if (typeof vnode === "string") return node.nodeType === Node.TEXT_NODE;
  return (
    node.nodeType === Node.ELEMENT_NODE &&
    (node as Element).tagName.toLowerCase() === vnode.tag.toLowerCase()
  );
}

/** Patch an existing, compatible node (see `canPatch`) to match `vnode`. */
function patchNode(node: Node, vnode: VNode): void {
  if (typeof vnode === "string") {
    if ((node as Text).data !== vnode) (node as Text).data = vnode;
    return;
  }
  const el = node as Element;
  patchAttributes(el, vnode.props);
  if (!VOID_ELEMENTS.has(vnode.tag)) patchChildren(el, vnode.children);
}

/** Reconcile `parent`'s child nodes with `children`. */
function patchChildren(parent: Element, children: unknown[]): void {
  const next = flattenChildren(children);

  // Index the keyed elements already present, so a keyed child that moved
  // is found by identity rather than rebuilt.
  const keyed = new Map<string, Element>();
  for (const node of Array.from(parent.childNodes)) {
    if (node.nodeType === Node.ELEMENT_NODE) {
      const k = (node as Element).getAttribute("data-key");
      if (k !== null && !keyed.has(k)) keyed.set(k, node as Element);
    }
  }

  const claimed = new Set<Node>();
  for (let i = 0; i < next.length; i++) {
    const vnode = next[i];
    const key = typeof vnode === "string" ? null : keyOf(vnode);
    const atSlot = parent.childNodes[i] ?? null;
    let node: Node | null = null;

    if (key !== null) {
      const found = keyed.get(key);
      if (found && !claimed.has(found) && canPatch(found, vnode)) {
        node = found;
        patchNode(node, vnode);
      }
    } else if (
      atSlot &&
      !claimed.has(atSlot) &&
      canPatch(atSlot, vnode) &&
      !(atSlot.nodeType === Node.ELEMENT_NODE &&
        (atSlot as Element).hasAttribute("data-key"))
    ) {
      // Unkeyed: reuse whatever sits at this position if it is the same
      // kind of node (and not a keyed node that belongs to some other key).
      node = atSlot;
      patchNode(node, vnode);
    }

    if (node === null) node = createNode(vnode);
    claimed.add(node);
    if (node !== atSlot) parent.insertBefore(node, atSlot);
  }

  // Whatever is left past the new length was not claimed: remove it.
  while (parent.childNodes.length > next.length) {
    parent.removeChild(parent.childNodes[parent.childNodes.length - 1]);
  }
}

/** Make `container`'s contents match the element tree `elementJson`. */
export function renderToContainer(container: HTMLElement, elementJson: unknown): void {
  if (isElement(elementJson) || Array.isArray(elementJson)) {
    patchChildren(container, Array.isArray(elementJson) ? elementJson : [elementJson]);
  } else {
    patchChildren(container, [String(elementJson ?? "")]);
  }
}

/** Serialize an element tree to HTML — for tests and server-side snapshots;
 *  the live path patches the DOM instead (see `renderToContainer`). */
export function renderToString(node: unknown): string {
  const escapeHtml = (s: string) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  if (typeof node === "string") return escapeHtml(node);
  if (typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(renderToString).join("");
  if (!isElement(node)) return "";
  const { tag, props, children } = node;
  let html = `<${tag}`;
  for (const [key, value] of Object.entries(props)) {
    if (value === false || value == null) continue;
    html += value === true ? ` ${attrName(key)}` : ` ${attrName(key)}="${escapeHtml(String(value))}"`;
  }
  if (VOID_ELEMENTS.has(tag)) return html + " />";
  return html + ">" + children.map(renderToString).join("") + `</${tag}>`;
}
