// petal-design.jsx — Mixit's design vocabulary as React components.
//
// Written by transpile_to_react_js next to a transpiled Mixit document. It
// holds the elements (`<vstack>` → `VStack`), the token value forms
// (`modes`, `per`, `alpha`…), the paints and effects, and the component
// helpers (`component_props`, `instance_root`, `slot`, `useCx`). Layout
// follows Mixit's own HTML export (crates/design/src/export/view/html.rs):
// stacks are flexbox, a zstack is a one-cell grid, a frame's children sit
// at their x/y inside its safe area.
//
// The transpiler reads this file's exports: a design helper is converted
// when it is exported here, and its parameters are placed by this
// signature (a trailing `{ … } = {}` takes Petal's named arguments).

import { Children, cloneElement, createContext, createElement, Fragment, isValidElement, useContext } from "react";
import { css_color, hex } from "./petal-runtime.js";
import { DEVICES, IOS_TEXT_STYLES, M3_TYPE_SCALE } from "./petal-design-data.js";

// The design package's tables (devices, type ramps), generated from its
// Petal source by runtime/gen-design-data.sh.
export { DEFAULT_FONTS, DEVICES, IOS_CONTENT_SIZES, IOS_DYNAMIC_TYPE, IOS_SF_METRICS, IOS_TEXT_STYLES, LINE_RULES, M3_TYPE_SCALE, WEB_DEFAULTS } from "./petal-design-data.js";
export const PLATFORMS = ["ios", "android", "web"];
export const MODES = ["light", "dark"];

// ── context ─────────────────────────────────────────────────────────────────

const DesignContext = createContext({ doc: {}, registry: {}, mode: "light" });
const CxContext = createContext({ platform: "web", mode: "light", device: "web_390", size_class: "compact", os: "chrome", scale: 2 });
const ParentContext = createContext({ kind: "root" });
/**
 * What the host page wants of a document's default export, which takes no
 * props: `<ViewContext.Provider value={{ frame: "today_ios", mode: "dark" }}>`
 * around it shows one frame, or one page, in a mode. It is one context for
 * every copy of this file (each transpiled design writes its own), kept at
 * `Symbol.for("petal.design.view")`, so a host page can make it without
 * importing any design: `globalThis[Symbol.for("petal.design.view")] ??=
 * createContext({})`.
 */
export const ViewContext = (globalThis[Symbol.for("petal.design.view")] ??= createContext({}));
// Set while a document draws only one frame that it builds at run time (in
// a helper function): every other frame, and the page and gallery
// scaffolding around them, draws nothing.
const OnlyFrameContext = createContext(null);

/** The `cx` a component's render reads: the platform, mode and device it is drawn for. */
export function useCx() {
  return useContext(CxContext);
}

const DEFAULT_DEVICE = { ios: "iphone_16", android: "pixel_8", web: "web_390" };
const FONTS = {
  ios: '-apple-system, "SF Pro Text", "SF Pro", system-ui, sans-serif',
  android: 'Roboto, "Noto Sans", system-ui, sans-serif',
  web: "system-ui, sans-serif",
};

// ── tokens and value forms ──────────────────────────────────────────────────

const REGISTRY = new globalThis.Map();

function isColor(v) {
  return v != null && typeof v === "object" && "r" in v && "g" in v && "b" in v && !("kind" in v);
}
export function is_color(v) { return isColor(v); }
export function is_token(v) { return v != null && typeof v === "object" && "token" in v && "value" in v; }

/** `tokens({...})`: each leaf becomes a marker `{token: "color.ink", value}`. */
export function tokens(rec) {
  return mark(rec, "");
}
function mark(rec, prefix) {
  const out = {};
  for (const [k, v] of Object.entries(rec)) {
    const name = prefix ? `${prefix}.${k}` : k;
    if (v != null && typeof v === "object" && !isColor(v) && !("kind" in v) && !is_token(v) && !Array.isArray(v)) {
      out[k] = mark(v, name);
    } else {
      const m = { token: name, value: v };
      REGISTRY.set(name, m);
      if (!REGISTRY.has(k)) REGISTRY.set(k, m);
      out[k] = m;
    }
  }
  return out;
}
export function token_name(v) { return is_token(v) ? v.token : null; }
export function val(v) { return is_token(v) ? v.value : v; }

export function modes(light, { dark, light_hc, dark_hc } = {}) {
  return { kind: "modes", light, dark: dark ?? light, light_hc, dark_hc };
}
export function per({ ios, android, web, other, why } = {}) {
  return { kind: "per", ios: ios ?? other, android: android ?? other, web: web ?? other, why };
}
export function alpha(of, a) { return { kind: "alpha", of, a }; }
export function mix(a, b, t) { return { kind: "mix", a, b, t }; }
export function soft_tint(of, t, { over = "snow" } = {}) { return { kind: "soft_tint", of, t, over }; }
export function darken(of, by) { return { kind: "tone", of, by: -by }; }
export function lighten(of, by) { return { kind: "tone", of, by }; }
export function system(name, { level = "base" } = {}) { return { kind: "system", name, level }; }
export function m3(role) { return { kind: "m3", role }; }
export function m3_scheme({ seed, variant = "tonal_spot", light, dark } = {}) {
  return { kind: "m3_scheme", seed, variant, light: light ?? {}, dark: dark ?? {} };
}
export function type_style({ ios, android, web, other } = {}) {
  return { kind: "type_style", ios: ios ?? other, android: android ?? other, web: web ?? other };
}
export function p3(r, g, b, a = 1) { return { r: r * 255, g: g * 255, b: b * 255, a: a * 255, space: "p3" }; }

// iOS semantic colours, light and dark (a few; others fall back to grey).
const SYSTEM = {
  label: ["#000000", "#FFFFFF"],
  secondaryLabel: ["#3C3C4399", "#EBEBF599"],
  tertiaryLabel: ["#3C3C434D", "#EBEBF54D"],
  systemBackground: ["#FFFFFF", "#000000"],
  secondarySystemBackground: ["#F2F2F7", "#1C1C1E"],
  tertiarySystemBackground: ["#FFFFFF", "#2C2C2E"],
  systemGroupedBackground: ["#F2F2F7", "#000000"],
  secondarySystemGroupedBackground: ["#FFFFFF", "#1C1C1E"],
  separator: ["#3C3C434A", "#54545899"],
  systemFill: ["#78788033", "#7878805C"],
  systemBlue: ["#007AFF", "#0A84FF"],
  systemGreen: ["#34C759", "#30D158"],
  systemRed: ["#FF3B30", "#FF453A"],
  systemOrange: ["#FF9500", "#FF9F0A"],
  systemGray: ["#8E8E93", "#8E8E93"],
};
const M3 = {
  primary: ["#6750A4", "#D0BCFF"], on_primary: ["#FFFFFF", "#381E72"],
  surface: ["#FEF7FF", "#141218"], on_surface: ["#1D1B20", "#E6E0E9"],
  surface_container: ["#F3EDF7", "#211F26"], surface_container_high: ["#ECE6F0", "#2B2930"],
  secondary_container: ["#E8DEF8", "#4A4458"], outline: ["#79747E", "#938F99"],
};

/** A value resolved for where it is drawn: markers, modes, per-platform, colour forms. */
export function resolve(v, cx) {
  if (v == null) return v;
  if (typeof v === "string" && cx && REGISTRY.has(v)) return resolve(REGISTRY.get(v), cx);
  if (typeof v !== "object" || Array.isArray(v)) return v;
  if (is_token(v)) return resolve(v.value, cx);
  const dark = cx?.mode === "dark";
  switch (v.kind) {
    case "modes": return resolve((cx?.contrast === "high" ? v[`${cx.mode}_hc`] : null) ?? v[cx?.mode ?? "light"] ?? v.light, cx);
    case "per": return resolve(v[cx?.platform] ?? v.other, cx);
    case "alpha": { const c = color(v.of, cx); return c && { ...c, a: Math.round((c.a ?? 255) * v.a) }; }
    case "mix": { const a = color(v.a, cx), b = color(v.b, cx); return a && b && blend(a, b, v.t); }
    case "soft_tint": { const a = color(v.of, cx), o = color(v.over, cx) ?? hex("FFFFFF"); return a && blend(o, a, v.t); }
    case "tone": { const c = color(v.of, cx); return c && blend(c, v.by > 0 ? hex("FFFFFF") : hex("000000"), Math.min(1, Math.abs(v.by) / 100)); }
    case "system": { const s = SYSTEM[v.name]; return hex((s ? s[dark ? 1 : 0] : "#8E8E93").slice(1)); }
    case "m3": { const key = v.role.replace(/[A-Z]/g, (c) => "_" + c.toLowerCase()); const s = M3[key]; return hex((s ? s[dark ? 1 : 0] : "#79747E").slice(1)); }
    default: return v;
  }
}
function blend(a, b, t) {
  const l = (x, y) => Math.round(x + (y - x) * t);
  return { r: l(a.r, b.r), g: l(a.g, b.g), b: l(a.b, b.b), a: l(a.a ?? 255, b.a ?? 255) };
}
function color(v, cx) {
  const c = resolve(v, cx);
  return isColor(c) ? c : typeof c === "string" && /^#?[0-9a-f]{3,8}$/i.test(c) ? hex(c) : null;
}
function cssColor(v, cx) {
  const c = color(v, cx);
  return c ? css_color(c) : undefined;
}

// ── paints, strokes, effects ────────────────────────────────────────────────

export function linear(angle, stops, { over } = {}) { return { kind: "linear", angle, stops, over }; }
export function radial(stops, { center = [0.5, 0.5], radius = 0.5 } = {}) { return { kind: "radial", stops, center, radius }; }
export function angular(stops, { center = [0.5, 0.5], angle = 0 } = {}) { return { kind: "angular", stops, center, angle }; }
export function image_fill(src, { fit = "fill", opacity = 1 } = {}) { return { kind: "image", src, fit, opacity }; }
export function stroke(color, { width = 1, align = "inside", dash, cap = "butt", join = "miter" } = {}) {
  return { kind: "stroke", color, width, align, dash, cap, join };
}
export function shadow(color = hex("00000054"), { radius, blur, x = 0, y = 0, spread = 0, model } = {}) {
  const sigma = radius ?? (blur != null ? blur / 2 : 10);
  return { kind: "shadow", color, sigma, x, y, spread, model: model ?? (blur != null ? "css" : "ios"), inset: false };
}
export function inner_shadow(color = hex("00000054"), { radius, blur, x = 0, y = 0, spread = 0 } = {}) {
  return { ...shadow(color, { radius, blur, x, y, spread }), inset: true };
}
export function elevation(dp) { return { kind: "elevation", dp }; }
export function blur(radius) { return { kind: "blur", radius }; }
export function material(style = "regular") { return { kind: "material", style }; }
export function backdrop(of) { return { kind: "backdrop", of }; }
export function backdrop_blur(radius, saturate = 1) { return { kind: "backdrop_blur", radius, saturate }; }
export function glass({ style = "regular", tint } = {}) { return { kind: "glass", style, tint }; }

function stopsCss(stops, cx) {
  return stops.map(([c, at]) => `${cssColor(c, cx)} ${+(at * 100).toFixed(2)}%`).join(", ");
}
function paintCss(p, cx) {
  const v = resolve(p, cx);
  if (v == null) return null;
  if (isColor(v) || typeof v === "string") return { layer: cssColor(v, cx), solid: true };
  switch (v.kind) {
    case "linear": return { layer: `linear-gradient(${v.angle + 90}deg, ${stopsCss(v.stops, cx)})` };
    case "radial": return { layer: `radial-gradient(circle at ${v.center[0] * 100}% ${v.center[1] * 100}%, ${stopsCss(v.stops, cx)})` };
    case "angular": return { layer: `conic-gradient(from ${v.angle + 90}deg at ${v.center[0] * 100}% ${v.center[1] * 100}%, ${stopsCss(v.stops, cx)})` };
    case "image": return { layer: `url(${JSON.stringify(v.src)}) center / ${v.fit === "fit" ? "contain" : "cover"} no-repeat` };
    default: return null;
  }
}
function fillStyle(fill, cx) {
  if (fill == null) return {};
  const list = Array.isArray(fill) ? fill : [fill];
  const layers = list.map((p) => paintCss(p, cx)).filter(Boolean);
  if (!layers.length) return {};
  if (layers.length === 1 && layers[0].solid) return { background: layers[0].layer };
  // CSS paints the first layer on top; Petal lists bottom first.
  return { background: layers.reverse().map((l) => (l.solid ? `linear-gradient(${l.layer}, ${l.layer})` : l.layer)).join(", ") };
}
function px(v) { return typeof v === "number" ? `${v}px` : v; }

// ── layout ──────────────────────────────────────────────────────────────────

export function pct(n) { return `${n}%`; }
export function pin(h = "left", v = "top") { return { h, v }; }
export function pin_all() { return { h: "left_right", v: "top_bottom" }; }
export function per_platform(cx, { ios, android, web, other } = {}) {
  return ({ ios, android, web })[cx.platform] ?? other;
}
export function platform_corners(cx) { return per_platform(cx, { ios: "continuous", other: "circular" }); }

const ALIGN = { leading: "flex-start", start: "flex-start", left: "flex-start", top: "flex-start", center: "center", trailing: "flex-end", end: "flex-end", right: "flex-end", bottom: "flex-end", stretch: "stretch" };
const JUSTIFY = { start: "flex-start", center: "center", end: "flex-end", space_between: "space-between", space_around: "space-around" };

function padding(p, cx) {
  p = resolve(p, cx);
  if (p == null) return undefined;
  if (typeof p === "number") return px(p);
  if (!Array.isArray(p)) return undefined;
  const v = p.map((x) => px(resolve(x, cx)));
  return v.length === 2 ? `${v[0]} ${v[1]}` : v.join(" ");
}

/** The node's own box: size, position in its parent, paint, effects. */
function boxStyle(props, parent, cx, { text = false, stackLike = false } = {}) {
  const s = { position: "relative", boxSizing: "border-box" };
  const w = resolve(props.width, cx), h = resolve(props.height, cx);
  const fixed = (d) => (typeof d === "number" ? px(d) : typeof d === "string" && d.endsWith("%") ? d : undefined);
  if (fixed(w)) s.width = fixed(w);
  if (fixed(h)) s.height = fixed(h);
  for (const [k, c] of [["min_width", "minWidth"], ["max_width", "maxWidth"], ["min_height", "minHeight"], ["max_height", "maxHeight"]]) {
    if (props[k] != null) s[c] = px(resolve(props[k], cx));
  }
  const fillW = w === "fill", fillH = h === "fill";
  const absolute = parent.kind === "absolute" || props.position === "absolute" || (parent.kind === "overlay" && (props.x != null || props.y != null));
  if (absolute) {
    s.position = "absolute";
    s.left = px(resolve(props.x, cx) ?? 0);
    s.top = px(resolve(props.y, cx) ?? 0);
    if (fillW) s.right = 0;
    if (fillH) s.bottom = 0;
    // Constraints keep a child to its parent's edges, measured in the
    // parent's designed size (before an instance resized it).
    const c = props.constraints;
    const num = (v) => (typeof v === "number" ? v : null);
    const pw = num(parent.width), ph = num(parent.height);
    if (c && pw != null && (c.h === "right" || c.h === "left_right")) {
      s.right = px(pw - (props.x ?? 0) - (num(props.width) ?? 0));
      if (c.h === "right") delete s.left; else delete s.width;
    }
    if (c && ph != null && (c.v === "bottom" || c.v === "top_bottom")) {
      s.bottom = px(ph - (props.y ?? 0) - (num(props.height) ?? 0));
      if (c.v === "bottom") delete s.top; else delete s.height;
    }
    if (c && pw != null && c.h === "center") { s.left = `calc(50% + ${px((props.x ?? 0) + (num(props.width) ?? 0) / 2 - pw / 2)})`; s.transform = "translateX(-50%)"; }
  } else if (parent.kind === "flow") {
    // A scrolling frame's content: in flow, so the frame grows with it.
    if (fillW) s.width = "100%";
    if (props.x) s.marginLeft = px(props.x);
    if (props.y) s.marginTop = px(props.y);
  } else if (parent.kind === "flex") {
    const [main, cross] = parent.axis === "h" ? [fillW, fillH] : [fillH, fillW];
    if (main) s.flex = "1 1 0px";
    else if (!text) s.flexShrink = 0;
    if (cross) s.alignSelf = "stretch";
  } else if (parent.kind === "overlay") {
    s.gridArea = "1 / 1";
    if (fillW) s.justifySelf = "stretch";
    if (fillH) s.alignSelf = "stretch";
  } else if (parent.kind === "grid") {
    s.justifySelf = fillW ? "stretch" : "start";
    s.alignSelf = fillH ? "stretch" : "start";
  } else if (parent.kind === "root") {
    if (fillW) s.width = "100%";
    if (fillH) s.height = "100%";
  }
  // `align` is the node's own alignment in its stack, except on a stack
  // (where it aligns the children) and on text (where it aligns the lines).
  if (props.align && parent.kind === "flex" && !stackLike) s.alignSelf = ALIGN[props.align] ?? props.align;
  if (!text) Object.assign(s, fillStyle(props.fill, cx));
  const r = resolve(props.radius, cx);
  if (Array.isArray(r)) s.borderRadius = r.map(px).join(" ");
  else if (r) s.borderRadius = px(r);
  const st = resolve(props.stroke, cx);
  if (st != null) {
    const k = isColor(st) || typeof st === "string" ? { color: st, width: 1, align: "inside" } : st;
    const width = resolve(k.width, cx) ?? 1;
    s.outline = `${px(width)} ${k.dash ? "dashed" : "solid"} ${cssColor(k.color, cx)}`;
    s.outlineOffset = k.align === "outside" ? 0 : k.align === "center" ? px(-width / 2) : px(-width);
  }
  if (props.clip) s.overflow = "hidden";
  const shadows = [], filters = [], backdrops = [];
  for (const e of [].concat(props.effects ?? [])) {
    const v = resolve(e, cx);
    if (!v) continue;
    if (v.kind === "shadow") shadows.push(`${v.inset ? "inset " : ""}${px(v.x)} ${px(v.y)} ${px(2 * v.sigma)} ${px(v.spread)} ${cssColor(v.color, cx)}`);
    else if (v.kind === "elevation") shadows.push(`0 ${px(v.dp / 2)} ${px(v.dp)} rgb(0 0 0 / 0.18), 0 ${px(v.dp / 6)} ${px(v.dp / 3)} rgb(0 0 0 / 0.12)`);
    else if (v.kind === "blur") filters.push(`blur(${px(v.radius)})`);
    else if (v.kind === "backdrop_blur") backdrops.push(`blur(${px(v.radius)}) saturate(${v.saturate})`);
    else if (v.kind === "backdrop" || v.kind === "glass" || v.kind === "material") backdrops.push("blur(20px) saturate(1.8)");
  }
  if (shadows.length) s.boxShadow = shadows.join(", ");
  if (filters.length) s.filter = filters.join(" ");
  if (backdrops.length) s.backdropFilter = backdrops.join(" ");
  if (props.opacity != null && props.opacity < 1) s.opacity = resolve(props.opacity, cx);
  if (props.rotation) s.transform = `rotate(${props.rotation}deg)`;
  if (props.blend && props.blend !== "normal") s.mixBlendMode = props.blend.replace(/_/g, "-");
  return s;
}

/** Whether a node is drawn here: `hidden`, `platform` and `mode` filters. */
function shown(props, cx) {
  if (resolve(props.hidden, cx)) return false;
  const only = (v, x) => v == null || (Array.isArray(v) ? v.includes(x) : v === x);
  return only(props.platform, cx.platform) && only(props.mode, cx.mode);
}

function dataAttrs(props, tag) {
  const a = { "data-petal": tag, "data-id": props.id };
  // `link="page/frame"`, a prototype navigation: kept on the DOM node for
  // the host page to follow (see ViewContext).
  if (typeof props.link === "string") a["data-link"] = props.link;
  // `on_tap={fn() … end}`: a tap runs the callback, which writes a top-level
  // `state var` (`set tab = "saved"`); every component that reads it draws
  // again. The innermost tapped node wins.
  if (typeof props.on_tap === "function") {
    a["data-tap"] = "";
    a.onClick = (e) => {
      e.stopPropagation();
      props.on_tap();
    };
  }
  return a;
}

const STACKS = new Set(["vstack", "hstack", "zstack", "grid"]);

function Box({ tag, props, parent: kind, children, extra = {}, text = false }) {
  const cx = useCx();
  const parent = useContext(ParentContext);
  if (parent.kind === "outside" || !shown(props, cx)) return null;
  const style = { ...boxStyle(props, parent, cx, { text, stackLike: STACKS.has(tag) }), ...extra };
  const inner = kind ? <ParentContext.Provider value={kind}>{children}</ParentContext.Provider> : children;
  return <div style={style} {...dataAttrs(props, tag)}>{inner}</div>;
}

function stack(axis) {
  return function StackNode(props) {
    const cx = useCx();
    const extra = {
      display: "flex",
      flexDirection: axis === "h" ? (props.reverse ? "row-reverse" : "row") : props.reverse ? "column-reverse" : "column",
      gap: px(resolve(props.gap, cx) ?? 0),
      padding: padding(props.padding, cx),
      alignItems: ALIGN[props.align ?? "center"] ?? "center",
      justifyContent: JUSTIFY[props.justify ?? "start"] ?? "flex-start",
    };
    if (props.wrap) Object.assign(extra, { flexWrap: "wrap", alignContent: "flex-start", rowGap: px(resolve(props.row_gap ?? props.gap, cx) ?? 0) });
    return <Box tag={axis === "h" ? "hstack" : "vstack"} props={props} parent={{ kind: "flex", axis }} extra={extra}>{props.children}</Box>;
  };
}
export const VStack = stack("v");
export const HStack = stack("h");

export function ZStack(props) {
  const cx = useCx();
  const [v, h] = zAlign(props.align ?? "center");
  const parent = { kind: "overlay", width: props.design_width ?? props.width, height: props.design_height ?? props.height };
  return (
    <Box tag="zstack" props={props} parent={parent} extra={{ display: "grid", gridTemplate: "auto / auto", justifyItems: h, alignItems: v, padding: padding(props.padding, cx) }}>
      {props.children}
    </Box>
  );
}
function zAlign(a) {
  const parts = a.split("_");
  let v = "center", h = "center";
  for (const p of parts) {
    if (p === "top") v = "start";
    if (p === "bottom") v = "end";
    if (p === "leading" || p === "left") h = "start";
    if (p === "trailing" || p === "right") h = "end";
  }
  return [v, h];
}

export function Grid(props) {
  const cx = useCx();
  const track = props.min != null ? `minmax(${px(props.min)}, 1fr)` : "minmax(0, 1fr)";
  const cols = props.columns != null ? `repeat(${props.columns}, ${track})` : props.min != null ? `repeat(auto-fill, ${track})` : `repeat(auto-fill, minmax(340px, 1fr))`;
  return (
    <Box tag="grid" props={props} parent={{ kind: "grid" }} extra={{ display: "grid", gridTemplateColumns: cols, gap: px(resolve(props.gap, cx) ?? 18), rowGap: px(resolve(props.row_gap ?? props.gap, cx) ?? 18), padding: padding(props.padding, cx) }}>
      {props.children}
    </Box>
  );
}

export function Spacer(props) {
  const min = props.min ?? 0;
  return <Box tag="spacer" props={props} extra={{ flex: `1 1 ${px(min)}`, minWidth: px(min), minHeight: px(min) }} />;
}
export function Rect(props) { return <Box tag="rect" props={props}>{props.children}</Box>; }
export function Ellipse(props) { return <Box tag="ellipse" props={props} extra={{ borderRadius: "50%" }} />; }
export function Capsule(props) { return <Box tag="capsule" props={props} extra={{ borderRadius: "9999px" }} />; }
export function Line(props) {
  const cx = useCx();
  const st = resolve(props.stroke, cx);
  const k = st == null ? { color: hex("000000"), width: 1 } : isColor(st) ? { color: st, width: 1 } : st;
  const c = cssColor(k.color, cx), w = px(k.width ?? 1);
  return <Box tag="line" props={{ ...props, stroke: null }} extra={{ background: `linear-gradient(${c}, ${c}) center / 100% ${w} no-repeat`, height: props.height == null ? w : undefined }} />;
}
export function Path(props) {
  const cx = useCx();
  const vb = props.viewbox ? props.viewbox.join(" ") : undefined;
  const st = resolve(props.stroke, cx);
  return (
    <Box tag="path" props={{ ...props, fill: null, stroke: null }}>
      <svg width="100%" height="100%" viewBox={vb} preserveAspectRatio="none" style={{ display: "block" }}>
        <path d={props.d} fill={cssColor(Array.isArray(props.fill) ? props.fill[0] : props.fill, cx) ?? "none"} fillRule={props.fill_rule} stroke={st ? cssColor(st.color ?? st, cx) : undefined} strokeWidth={st?.width} />
      </svg>
    </Box>
  );
}
export function Image(props) {
  const fit = { fill: "cover", cover: "cover", fit: "contain", contain: "contain", stretch: "fill", none: "none" }[props.fit ?? "fill"];
  if (!props.src || props.generate) {
    return <Box tag="image" props={props} extra={{ background: placeholder(props.generate, props.seed ?? props.id ?? "") }} />;
  }
  return (
    <Box tag="image" props={props} extra={{ overflow: "hidden" }}>
      <img src={props.src} alt="" style={{ width: "100%", height: "100%", objectFit: fit, display: "block" }} />
    </Box>
  );
}
/**
 * `generate="scene" | "satellite" | "avatar"`: Mixit draws deterministic art
 * per `seed`; here a gradient of the same kind whose hues follow the seed.
 */
function placeholder(kind, seed) {
  let h = 0;
  for (const ch of String(seed)) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  const hue = h % 360, alt = (hue + 40 + (h >> 9) % 80) % 360;
  if (kind === "scene") return `linear-gradient(180deg, hsl(${hue % 60 + 190} 60% 78%) 0%, hsl(${hue % 60 + 200} 35% 70%) 55%, hsl(${alt % 90 + 90} 25% 55%) 56%, hsl(${alt % 90 + 90} 30% 42%) 100%)`;
  if (kind === "satellite") return `radial-gradient(circle at 30% 40%, hsl(${hue % 50 + 80} 30% 45%), transparent 45%), radial-gradient(circle at 75% 70%, hsl(${alt % 40 + 30} 25% 50%), transparent 50%), hsl(${hue % 40 + 100} 22% 34%)`;
  if (kind === "avatar") return `radial-gradient(circle at 50% 38%, hsl(${hue} 40% 82%) 0 18%, transparent 19%), radial-gradient(ellipse at 50% 100%, hsl(${hue} 40% 82%) 0 38%, transparent 39%), hsl(${alt} 45% 62%)`;
  return "linear-gradient(135deg, #a7b4c2, #6b7b8c)";
}

/** SF Symbols and Material icons are not on the web: a placeholder of the icon's size and colour. */
export function Icon(props) {
  const cx = useCx();
  const size = props.size ?? 17;
  const c = cssColor((props.colors ?? [])[0] ?? props.color ?? hex("8E8E93"), cx);
  return (
    <Box tag="icon" props={{ ...props, fill: null }} extra={{ width: px(size), height: px(size), display: "grid", placeItems: "center" }}>
      <div title={props.name} style={{ width: "70%", height: "70%", borderRadius: "30%", background: c, opacity: 0.85 }} />
    </Box>
  );
}
export function CanvasDoc(props) { return <Box tag="canvas_doc" props={props} extra={{ background: "#eee" }} />; }
export function Unknown() { return null; }

// ── text ────────────────────────────────────────────────────────────────────


function typeOf(style, cx) {
  const v = resolve(style, cx);
  if (!v) return {};
  const t = v.kind === "type_style" ? v[cx.platform] ?? {} : v;
  const ios = { title: "title1", caption: "caption1" }[t.text_style] ?? t.text_style;
  const named = t.text_style && (IOS_TEXT_STYLES[ios] ?? M3_TYPE_SCALE[t.text_style.replace(/[A-Z]/g, (c) => "_" + c.toLowerCase())]);
  const out = { ...t };
  if (named) {
    out.size ??= named.size;
    out.line ??= named.line;
    out.weight ??= named.weight;
    if (named.tracking) out.tracking ??= named.tracking;
  }
  return out;
}

function textCss(props, cx) {
  const t = { ...typeOf(props.style, cx) };
  for (const k of ["font", "size", "weight", "italic", "tracking", "design"]) if (props[k] != null) t[k] = resolve(props[k], cx);
  if (props.line_height != null) t.line = props.line_height;
  const s = {};
  const design = { rounded: 'ui-rounded, "SF Pro Rounded", ', serif: 'ui-serif, "New York", Georgia, ', mono: 'ui-monospace, "SF Mono", Menlo, ' }[t.design] ?? "";
  s.fontFamily = t.font ? `"${t.font}", ${FONTS[cx.platform] ?? FONTS.web}` : design + (FONTS[cx.platform] ?? FONTS.web);
  const size = t.size ?? (cx.platform === "ios" ? 17 : cx.platform === "android" ? 14 : 16);
  s.fontSize = px(size);
  if (t.weight != null) s.fontWeight = t.weight;
  if (t.italic) s.fontStyle = "italic";
  if (t.tracking) s.letterSpacing = px(t.tracking);
  if (t.line != null) s.lineHeight = t.line < 4 ? t.line : px(t.line);
  if (props.color != null) s.color = cssColor(props.color, cx);
  if (props.align) s.textAlign = { leading: "left", center: "center", trailing: "right", justify: "justify" }[props.align] ?? props.align;
  if (props.transform) s.textTransform = props.transform;
  if (props.features) s.fontFeatureSettings = props.features.map((f) => `"${f}"`).join(", ");
  return s;
}

export function Text(props) {
  const cx = useCx();
  const parent = useContext(ParentContext);
  if (parent.kind === "outside" || !shown(props, cx)) return null;
  const s = { ...boxStyle({ ...props, fill: null, align: undefined }, parent, cx, { text: true }), ...textCss(props, cx), margin: 0, whiteSpace: "pre-wrap" };
  if (props.color == null) s.color = cssColor(hex("000000"), cx);
  if (props.lines === 1) Object.assign(s, { whiteSpace: "pre", overflow: "hidden", textOverflow: "ellipsis", maxWidth: "100%" });
  else if (props.lines > 1) Object.assign(s, { maxWidth: "100%", display: "-webkit-box", WebkitBoxOrient: "vertical", WebkitLineClamp: props.lines, overflow: "hidden" });
  return <div style={s} {...dataAttrs(props, "text")}>{props.children}</div>;
}
export function Span(props) {
  const cx = useCx();
  const s = textCss(props, cx);
  delete s.fontFamily;
  if (props.size == null) delete s.fontSize;
  return <span style={s}>{props.children}</span>;
}

// ── frames and pages ────────────────────────────────────────────────────────

export function Frame(props) {
  const only = useContext(OnlyFrameContext);
  const outer = useCx();
  const design = useContext(DesignContext);
  if (only != null && props.id !== only) return null;
  const platform = props.platform ?? DEVICES[props.device]?.platform ?? outer.platform;
  const deviceName = props.device ?? design.doc?.default_device?.[platform] ?? DEFAULT_DEVICE[platform] ?? "web_390";
  const d = DEVICES[deviceName] ?? { width: 390, height: 844, scale: 2, safe_area: { top: 0, bottom: 0, left: 0, right: 0 } };
  const landscape = props.orientation === "landscape";
  const [w, h] = landscape ? [d.height, d.width] : [d.width, d.height];
  const cx = {
    platform,
    mode: props.mode ?? design.mode ?? outer.mode,
    contrast: props.contrast,
    device: deviceName,
    size_class: props.size_class ?? (w >= 700 ? "regular" : "compact"),
    os: props.os ?? design.doc?.os?.[platform],
    scale: d.scale,
  };
  if (resolve(props.hidden, cx)) return null;
  const safe = d.safe_area;
  const kids = Children.toArray(props.children);
  const ignoring = (k) => isValidElement(k) && k.props.safe_area === "ignore";
  const style = {
    position: "relative", width: px(w), height: props.scroll ? undefined : px(h), minHeight: px(h), overflow: props.scroll ? "visible" : "hidden",
    ...fillStyle(props.fill ?? hex("FFFFFF"), cx), borderRadius: props.radius != null ? px(props.radius) : undefined,
    fontFamily: FONTS[platform] ?? FONTS.web, flexShrink: 0,
  };
  return (
    <CxContext.Provider value={cx}>
      <div style={style} data-petal="frame" data-id={props.id}>
        <ParentContext.Provider value={{ kind: "absolute", width: w, height: h }}>
          {kids.filter(ignoring)}
          <div style={props.scroll ? { position: "relative", padding: `${px(safe.top)} ${px(safe.right)} ${px(safe.bottom)} ${px(safe.left)}` } : { position: "absolute", left: px(safe.left), top: px(safe.top), right: px(safe.right), bottom: px(safe.bottom) }}>
            <ParentContext.Provider value={{ kind: props.scroll ? "flow" : "absolute", width: w - safe.left - safe.right, height: h - safe.top - safe.bottom }}>
              {kids.filter((k) => !ignoring(k))}
            </ParentContext.Provider>
          </div>
        </ParentContext.Provider>
      </div>
    </CxContext.Provider>
  );
}

export function Page(props) {
  // Drawing one frame: what sits on the page outside a frame draws nothing.
  if (useContext(OnlyFrameContext) != null) return <ParentContext.Provider value={{ kind: "outside" }}>{props.children}</ParentContext.Provider>;
  const canvas = props.layout === "canvas";
  return (
    <section data-petal="page" data-id={props.id} style={{ padding: 32, background: props.background ? cssColor(props.background, { mode: "light" }) : undefined }}>
      {props.title != null && <h2 style={{ font: "600 20px system-ui", margin: "0 0 4px" }}>{props.title}</h2>}
      {props.blurb != null && <p style={{ font: "14px system-ui", color: "#666", margin: "0 0 20px" }}>{props.blurb}</p>}
      <div style={canvas ? { display: "flex", flexWrap: "wrap", gap: 48, alignItems: "flex-start" } : { display: "flex", flexDirection: "column", gap: 32 }}>
        <ParentContext.Provider value={{ kind: "root" }}>{props.children}</ParentContext.Provider>
      </div>
    </section>
  );
}

// Gallery scaffolding: simple versions of DesignLibrary's.
export function Section(props) {
  if (useContext(OnlyFrameContext) != null) return null;
  return (
    <div data-petal="section" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      {props.title != null && <h3 style={{ font: "600 16px system-ui", margin: 0 }}>{props.title}</h3>}
      <ParentContext.Provider value={{ kind: "root" }}>{props.children}</ParentContext.Provider>
    </div>
  );
}
export function Specimen(props) {
  if (useContext(OnlyFrameContext) != null) return null;
  const design = useContext(DesignContext);
  const ground = props.ground ? design.doc?.gallery?.grounds?.[props.ground] ?? props.ground : hex("F4F4F6");
  return (
    <figure data-petal="specimen" style={{ margin: 0, display: "flex", flexDirection: "column", gap: 6 }}>
      <div style={{ padding: 24, borderRadius: 12, display: "flex", justifyContent: "center", ...fillStyle(ground, { mode: "light" }) }}>
        <ParentContext.Provider value={{ kind: "flex", axis: "v" }}>
          <div style={{ display: "flex", flexDirection: "column", alignItems: "stretch", width: "100%" }}>{props.children}</div>
        </ParentContext.Provider>
      </div>
      {props.title != null && <figcaption style={{ font: "600 13px system-ui" }}>{props.title}</figcaption>}
      {props.caption != null && <figcaption style={{ font: "12px ui-monospace, monospace", color: "#666" }}>{props.caption}</figcaption>}
    </figure>
  );
}
export function PhoneRow(props) {
  if (useContext(OnlyFrameContext) != null) return null;
  return <div data-petal="phone_row" style={{ display: "flex", gap: 32, flexWrap: "wrap" }}>{props.children}</div>;
}
export function Phone(props) {
  if (useContext(OnlyFrameContext) != null) return null;
  return (
    <div data-petal="phone" style={{ display: "flex", flexDirection: "column", gap: 8 }}>
      <Frame device={props.device ?? "iphone_16"} radius={44}>{props.children}</Frame>
      {props.title != null && <div style={{ font: "600 13px system-ui" }}>{props.title}</div>}
      {props.caption != null && <div style={{ font: "12px system-ui", color: "#666" }}>{props.caption}</div>}
    </div>
  );
}
export function Note(props) {
  if (useContext(OnlyFrameContext) != null) return null;
  return (
    <aside data-petal="note" style={{ padding: 16, borderRadius: 10, background: "#F4F4F6", font: "14px system-ui" }}>
      {props.title != null && <strong style={{ display: "block", marginBottom: 4 }}>{props.title}</strong>}
      {props.children}
    </aside>
  );
}
export function Guideline(props) {
  if (useContext(OnlyFrameContext) != null) return null;
  return (
    <div data-petal="guideline" style={{ display: "flex", flexDirection: "column", gap: 8 }}>
      {props.title != null && <h3 style={{ font: "600 16px system-ui", margin: 0 }}>{props.title}</h3>}
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 16 }}>{props.children}</div>
    </div>
  );
}
function verdict(tag, color, label) {
  return function Verdict(props) {
    return (
      <div data-petal={tag} style={{ borderTop: `3px solid ${color}`, padding: 12, font: "14px system-ui", display: "flex", flexDirection: "column", gap: 8 }}>
        <strong style={{ color }}>{label}</strong>
        <ParentContext.Provider value={{ kind: "flex", axis: "v" }}>{props.children}</ParentContext.Provider>
      </div>
    );
  };
}
export const Do = verdict("do", "#1F9D55", "Do");
export const Dont = verdict("dont", "#D33A2C", "Don't");

// ── components and instances ────────────────────────────────────────────────

/** `component(name, render, …)`: a component's entry in the document's list. */
export function component(name, render, { group = "Components", props = {}, variants = {}, docs = "" } = {}) {
  if (render && !render.defaults) render.defaults = props;
  return { name, render, group, props, variants, docs };
}

/** A component whose render is a lambda: `render(p, cx)` as a React component. */
export function component_from_render(render) {
  function Rendered(props) {
    return instance_root(props, render(component_props(props, Rendered.defaults ?? {}), useCx()));
  }
  return Rendered;
}

const INSTANCE_ATTRS = ["x", "y", "width", "height", "min_width", "max_width", "min_height", "max_height", "rotation", "opacity", "hidden", "position", "constraints", "align", "safe_area", "platform", "mode", "link", "on_tap"];

/** An instance's props: the defaults, the attributes it sets, its `props={…}`. */
export function component_props(props, defaults = {}) {
  const out = { ...defaults };
  for (const [k, v] of Object.entries(props)) {
    if (k === "props" || k === "overrides" || k === "of") continue;
    if (INSTANCE_ATTRS.includes(k) && !(k in defaults)) continue;
    out[k] = v;
  }
  return { ...out, ...(props.props ?? {}) };
}

/** The component's root, with the instance's own box (size, position…) and its `overrides`. */
export function instance_root(props, el) {
  if (!isValidElement(el)) return el ?? null;
  const own = {};
  for (const k of INSTANCE_ATTRS) if (props[k] !== undefined) own[k] = props[k];
  // The size the component was drawn at, which its children's constraints
  // are measured in.
  if (own.width !== undefined && typeof el.props.width === "number") own.design_width = el.props.width;
  if (own.height !== undefined && typeof el.props.height === "number") own.design_height = el.props.height;
  let out = Object.keys(own).length ? cloneElement(el, own) : el;
  if (props.overrides) out = applyOverrides(out, normalizeOverrides(props.overrides));
  return out;
}

const ATTR_NAMES = new Set(["color", "fill", "stroke", "hidden", "opacity", "weight", "size", "radius", "width", "height", "padding", "gap", "text", "effects", "name", "style", "align"]);

/** `{title: {color}}` and `{"badge.label.color": c}` → `{"title": {color}, "badge.label": {color}}`. */
function normalizeOverrides(o, prefix = "") {
  const out = {};
  for (const [k, v] of Object.entries(o)) {
    const path = prefix ? `${prefix}.${k}` : k;
    if (v != null && typeof v === "object" && !Array.isArray(v) && !isColor(v) && !("kind" in v) && !is_token(v)) {
      const attrs = {}, nested = {};
      for (const [a, x] of Object.entries(v)) {
        const isGroup = x != null && typeof x === "object" && !Array.isArray(x) && !isColor(x) && !is_token(x) && !("kind" in x);
        (ATTR_NAMES.has(a) || !isGroup ? attrs : nested)[a] = x;
      }
      if (Object.keys(attrs).length) out[path] = { ...(out[path] ?? {}), ...attrs };
      Object.assign(out, normalizeOverrides(nested, path));
    } else {
      const i = path.lastIndexOf(".");
      const id = path.slice(0, i), attr = path.slice(i + 1);
      out[id] = { ...(out[id] ?? {}), [attr]: v };
    }
  }
  return out;
}

function applyOverrides(el, overrides) {
  if (!isValidElement(el) || !Object.keys(overrides).length) return el;
  const id = el.props.id;
  const props = {};
  const pass = {};
  for (const [path, attrs] of Object.entries(overrides)) {
    if (id != null && path === id) Object.assign(props, attrs);
    else if (id != null && path.startsWith(id + ".") && el.type?.defaults) pass[path.slice(id.length + 1)] = attrs;
  }
  if (Object.keys(pass).length) props.overrides = pass;
  const kids = el.props.children;
  if ("text" in props) {
    const { text, ...rest } = props;
    return cloneElement(el, rest, text);
  }
  if (kids === undefined || el.type?.defaults) return cloneElement(el, props);
  return cloneElement(el, props, ...[].concat(Children.map(kids, (k) => applyOverrides(k, overrides)) ?? []));
}

/** What fills a `<slot>`: the instance's children for that slot, else the slot's own. */
export function slot(children, name, fallback) {
  const kids = Children.toArray(children).filter((k) => (isValidElement(k) ? k.props.slot ?? null : null) === (name ?? null));
  return kids.length ? kids : fallback ?? null;
}

/** `<instance of={name}>` for a component found by name at render time. */
export function Instance(props) {
  const design = useContext(DesignContext);
  const c = design.registry[props.of];
  if (!c) {
    return (
      <div data-petal="instance" title={`no component "${props.of}"`} style={{ outline: "1px dashed #c33", padding: 8, font: "12px system-ui", color: "#c33" }}>
        {props.of}
      </div>
    );
  }
  const { of, ...rest } = props;
  return createElement(c.render, rest);
}

/** A grid of one component in each combination of its variant axes. */
export function variant_grid(components, name, { axes, props = {}, ground } = {}) {
  const c = components.find((x) => x.name === name);
  if (!c) return null;
  const names = axes ?? Object.keys(c.variants);
  let cells = [{}];
  for (const a of names) cells = cells.flatMap((cell) => (c.variants[a] ?? []).map((v) => ({ ...cell, [a]: v })));
  return (
    <Grid>
      {cells.map((cell, i) => (
        <Specimen key={i} title={Object.values(cell).join(" · ")} ground={ground}>
          {createElement(c.render, { ...props, ...cell })}
        </Specimen>
      ))}
    </Grid>
  );
}

/** The text/ground pairs a contrast table lists: each text colour on each ground. */
export function contrast_pairs(texts, on, { large = false, min } = {}) {
  const out = [];
  for (const bg of on) for (const t of texts) out.push(min == null ? { text: t, on: bg, large } : { text: t, on: bg, large, min });
  return out;
}

/** A guideline with a do and a don't, each with its example. */
export function rule(title, good, good_example, bad, bad_example, { why } = {}) {
  return (
    <Guideline title={title}>
      {why}
      <Do>{good}{good_example}</Do>
      <Dont>{bad}{bad_example}</Dont>
    </Guideline>
  );
}

// Brand rules are checked by Mixit, not drawn: the React side keeps them as data.
export function starter_rules() { return []; }
export function min_size(name, min, { only, why, severity = "error", width = true, height = true, platforms } = {}) {
  return { kind: "min_size", name, min, only, why, severity, width, height, platforms };
}
export function brand_rule(name, check, { why, severity = "error", on = "node", only, platforms } = {}) {
  return { kind: "custom", name, check, why, severity, on, only, platforms };
}
export function must_fit(...args) { return { kind: "must_fit", args }; }
export function min_contrast(...args) { return { kind: "min_contrast", args }; }
export function token_colors(...args) { return { kind: "token_colors", args }; }
export function touch_target(...args) { return { kind: "touch_target", args }; }
export function on_grid(...args) { return { kind: "on_grid", args }; }

// ── the document ────────────────────────────────────────────────────────────

/**
 * A whole document: its pages, or one frame of it (`frame="today_ios"`), in
 * a mode (`mode="dark"`).
 */
export function Design({ doc = {}, components = [], pages = [], ...asked }) {
  const outer = useContext(CxContext);
  const view = useContext(ViewContext);
  const frame = asked.frame ?? view.frame, page = asked.page ?? view.page, mode = asked.mode ?? view.mode;
  const registry = {};
  for (const c of components) registry[c.name] = c;
  const value = { doc, registry, mode: mode ?? doc.modes?.[0] ?? "light" };
  let list = [].concat(pages).flat(Infinity).filter(Boolean);
  if (page != null) list = list.filter((p) => p.props?.id === page);
  const cx = { ...outer, mode: value.mode, platform: doc.platforms?.[0] ?? "web" };
  if (frame != null) {
    // A frame written in the pages is drawn alone; one a function builds is
    // found by drawing the pages with every other frame left out.
    const f = findFrame(list, frame);
    return (
      <DesignContext.Provider value={value}>
        <CxContext.Provider value={cx}>
          {f ?? (
            <OnlyFrameContext.Provider value={frame}>
              {list.map((p, i) => (isValidElement(p) ? cloneElement(p, { key: p.key ?? i }) : p))}
            </OnlyFrameContext.Provider>
          )}
        </CxContext.Provider>
      </DesignContext.Provider>
    );
  }
  return (
    <DesignContext.Provider value={value}>
      <CxContext.Provider value={cx}>
        <div data-petal="design">{list.map((p, i) => (isValidElement(p) ? cloneElement(p, { key: p.key ?? i }) : p))}</div>
      </CxContext.Provider>
    </DesignContext.Provider>
  );
}

function findFrame(nodes, id) {
  for (const n of [].concat(nodes).flat(Infinity)) {
    if (!isValidElement(n)) continue;
    if (n.type?.frameId === id || (n.type === Frame && n.props.id === id)) return n;
    const inner = findFrame(Children.toArray(n.props.children), id);
    if (inner) return inner;
  }
  return null;
}

/** `design(doc, components, pages)`: the document as one element. */
export function design(doc, components, pages) {
  return <Design doc={doc} components={components} pages={pages} />;
}

export { Fragment };
