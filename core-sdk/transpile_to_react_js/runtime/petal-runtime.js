// petal-runtime.js — the Petal builtins that transpiled React code calls.
//
// Written by transpile_to_react_js next to the generated components. Every
// `export` here is a name the transpiler may import; it reads this file to
// know which builtins it can convert, so a builtin added here is picked up
// with no change on the Rust side.
//
// Values: nil is `null`, lists are arrays, records are plain objects, maps
// are `PMap` (keys compared by content, as Petal's `==`), colors are
// `{r, g, b, a?}` records with 0–255 channels.

// ── equality and types ──────────────────────────────────────────────────────

export function eq(a, b) {
  if (a === b) return true;
  if (a == null || b == null) return a == null && b == null;
  if (typeof a !== "object" || typeof b !== "object") return false;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    return a.every((x, i) => eq(x, b[i]));
  }
  if (a instanceof PMap || b instanceof PMap) {
    if (!(a instanceof PMap && b instanceof PMap) || a.size !== b.size) return false;
    return a.entries().every(([k, v]) => eq(b.get(k), v));
  }
  const ka = Object.keys(a).filter((k) => a[k] !== undefined);
  const kb = Object.keys(b).filter((k) => b[k] !== undefined);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => eq(a[k], b[k]));
}

export function type(v) {
  if (v == null) return "nil";
  if (typeof v === "boolean") return "bool";
  if (typeof v === "number") return Number.isInteger(v) ? "int" : "float";
  if (typeof v === "string") return "string";
  if (typeof v === "function") return "function";
  if (Array.isArray(v)) return "list";
  if (v instanceof PMap) return "map";
  if (v.$$typeof) return "element";
  if (v.__class) return v.__class;
  return "record";
}

// ── numbers ─────────────────────────────────────────────────────────────────

/** Petal's `/`: integer division truncates when both sides are ints. */
export function div(a, b) {
  return Number.isInteger(a) && Number.isInteger(b) ? Math.trunc(a / b) : a / b;
}
export const pi = Math.PI;
export function int(x) { return typeof x === "string" ? parseInt(x, 10) : Math.trunc(x); }
export function float(x) { return typeof x === "string" ? parseFloat(x) : Number(x); }
export function parse_int(s) { const n = parseInt(s, 10); return Number.isNaN(n) ? null : n; }
export function parse_float(s) { const n = parseFloat(s); return Number.isNaN(n) ? null : n; }
export function clamp(v, lo, hi) { return Math.min(Math.max(v, lo), hi); }
export function clamp01(v) { return clamp(v, 0, 1); }
export function sign(x) { return Math.sign(x); }
export function fract(x) { return x - Math.floor(x); }
export function radians(d) { return (d * Math.PI) / 180; }
export function degrees(r) { return (r * 180) / Math.PI; }
export function safe_div(a, b, fallback = 0) { return b === 0 ? fallback : a / b; }
export function map_range(v, inLo, inHi, outLo, outHi) {
  return outLo + ((v - inLo) / (inHi - inLo)) * (outHi - outLo);
}
export function smoothstep(e0, e1, x) {
  const t = clamp01((x - e0) / (e1 - e0));
  return t * t * (3 - 2 * t);
}
export function lerp(a, b, t) {
  if (typeof a === "number") return a + (b - a) * t;
  if (Array.isArray(a)) return a.map((x, i) => lerp(x, b[i], t));
  const out = {};
  for (const k of Object.keys(a)) out[k] = typeof a[k] === "number" || typeof a[k] === "object" ? lerp(a[k], b[k], t) : a[k];
  if ("r" in a && "g" in a && "b" in a) for (const k of ["r", "g", "b", "a"]) if (k in out) out[k] = Math.round(out[k]);
  return out;
}
export function interp(table, x) {
  if (x <= table[0][0]) return table[0][1];
  for (let i = 1; i < table.length; i++) {
    const [x0, y0] = table[i - 1];
    const [x1, y1] = table[i];
    if (x < x1) return x1 === x0 ? y0 : y0 + ((x - x0) / (x1 - x0)) * (y1 - y0);
  }
  return table[table.length - 1][1];
}
export function distance(a, b) { return Math.hypot(b.x - a.x, b.y - a.y, (b.z ?? 0) - (a.z ?? 0)); }
export function mag(v) { return Math.hypot(v.x, v.y, v.z ?? 0); }
export function random(a, b) {
  if (a === undefined) return Math.random();
  if (b === undefined) return Math.random() * a;
  return a + Math.random() * (b - a);
}
export function random_int(a, b) {
  return b === undefined ? Math.floor(Math.random() * a) : a + Math.floor(Math.random() * (b - a + 1));
}
export function choose(xs) { return xs[Math.floor(Math.random() * xs.length)]; }
export function fixed(v, places = 2) { return Number(v).toFixed(places); }
export function commas(v, places) {
  const n = places === undefined ? v : Number(Number(v).toFixed(places));
  return n.toLocaleString("en-US", places === undefined ? undefined : { minimumFractionDigits: places, maximumFractionDigits: places });
}
export function min(...xs) { return Math.min(...(xs.length === 1 && Array.isArray(xs[0]) ? xs[0] : xs)); }
export function max(...xs) { return Math.max(...(xs.length === 1 && Array.isArray(xs[0]) ? xs[0] : xs)); }
export function round(x, places) {
  if (places === undefined) return Math.sign(x) * Math.round(Math.abs(x));
  const f = 10 ** places;
  return (Math.sign(x) * Math.round(Math.abs(x) * f)) / f;
}

// ── strings ─────────────────────────────────────────────────────────────────

/** Petal's `str`: nil is "nil", lists and records are written as Petal writes them. */
export function str(v) {
  if (v == null) return "nil";
  if (typeof v === "string") return v;
  if (typeof v === "number") return String(v);
  if (Array.isArray(v)) return `[${v.map(show).join(", ")}]`;
  if (v instanceof PMap) return `Map(${show(v.entries())})`;
  if (typeof v === "object" && !v.$$typeof) return `{${Object.entries(v).map(([k, x]) => `${k}: ${show(x)}`).join(", ")}}`;
  return String(v);
}
function show(v) { return typeof v === "string" ? JSON.stringify(v) : str(v); }
export function upper(s) { return s.toUpperCase(); }
export function lower(s) { return s.toLowerCase(); }
export function trim(s) { return s.trim(); }
export function split(s, sep) { return s.split(sep); }
export function chars(s) { return Array.from(s); }
export function char_len(s) { return Array.from(s).length; }
export function starts_with(s, p) { return s.startsWith(p); }
export function ends_with(s, p) { return s.endsWith(p); }
export function replace(s, a, b) { return s.split(a).join(b); }
export function repeat(s, n) { return n > 0 ? s.repeat(n) : ""; }
export function pad_start(v, w, fill = " ") { return str(v).padStart(w, fill); }
export function pad_end(v, w, fill = " ") { return str(v).padEnd(w, fill); }
export function plural(n, word, many) { return n === 1 ? `${n} ${word}` : `${n} ${many ?? word + "s"}`; }
export function list_and(xs) {
  if (xs.length <= 1) return xs.join("");
  return `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`;
}
/** printf-style: `%s %d %f %x %X %%`, with `,` grouping, a width (`-` pads right, `0` with zeros) and `.N` decimals. */
export function format(t, ...args) {
  let i = 0;
  return t.replace(/%(,?)(-?)(0?)(\d*)(?:\.(\d+))?([sdfxX%])/g, (_, comma, left, zero, width, places, c) => {
    if (c === "%") return "%";
    const v = args[i++];
    let out;
    if (c === "s") out = str(v);
    else if (c === "d") out = String(Math.trunc(v));
    else if (c === "f") out = places !== undefined ? Number(v).toFixed(+places) : String(Number(v));
    else out = Math.trunc(v).toString(16)[c === "X" ? "toUpperCase" : "toLowerCase"]();
    if (comma && (c === "d" || c === "f")) {
      const [a, b] = out.split(".");
      out = a.replace(/\B(?=(\d{3})+(?!\d))/g, ",") + (b !== undefined ? "." + b : "");
    }
    const w = +width || 0;
    if (out.length < w) out = left ? out.padEnd(w) : out.padStart(w, zero ? "0" : " ");
    return out;
  });
}
export function char_at(s, i) { const cs = Array.from(s); return cs[i < 0 ? cs.length + i : i] ?? null; }
export function char_slice(s, start, end) { return Array.from(s).slice(start, end).join(""); }
export function byte_len(s) { return new TextEncoder().encode(s).length; }
export function byte_slice(s, start, end) { return new TextDecoder().decode(new TextEncoder().encode(s).slice(start, end)); }
export function hash_str(s) { let h = 2166136261; for (const c of s) { h ^= c.codePointAt(0); h = Math.imul(h, 16777619); } return h >>> 0; }
export function hash_int(n) { let x = (n | 0) ^ 0x9e3779b9; x = Math.imul(x ^ (x >>> 16), 0x85ebca6b); x = Math.imul(x ^ (x >>> 13), 0xc2b2ae35); return (x ^ (x >>> 16)) >>> 0; }
export function hash01(n) { return hash_int(typeof n === "string" ? hash_str(n) : n) / 4294967296; }
export function stable_id(...parts) { return parts.map(str).join(":"); }
export function ord(c) { return c.codePointAt(0); }
export function chr(n) { return String.fromCodePoint(n); }

// ── collections ─────────────────────────────────────────────────────────────

export function len(x) {
  if (x == null) return 0;
  if (typeof x === "string") return Array.from(x).length;
  if (Array.isArray(x)) return x.length;
  if (x instanceof PMap) return x.size;
  return Object.keys(x).length;
}
export function range(a, b, step = 1) {
  const [lo, hi] = b === undefined ? [0, a] : [a, b];
  const out = [];
  if (step > 0) for (let i = lo; i < hi; i += step) out.push(i);
  else for (let i = lo; i > hi; i += step) out.push(i);
  return out;
}
/** `a ++ b`: lists and strings. */
export function concat(...parts) {
  if (parts.every((p) => typeof p === "string")) return parts.join("");
  return parts.flatMap((p) => (Array.isArray(p) ? p : [p]));
}
export function append(xs, v) { return [...xs, v]; }
export const push = append;
export function prepend(xs, v) { return [v, ...xs]; }
export function pop(xs) { return xs.slice(0, -1); }
export function last(xs) { return xs.length ? xs[xs.length - 1] : null; }
export function first(xs) { return xs.length ? xs[0] : null; }
export function drop_last(xs) { return xs.slice(0, -1); }
export function is_empty(x) { return len(x) === 0; }
export function take(xs, n) { return xs.slice(0, Math.max(0, n)); }
export function drop(xs, n) { return xs.slice(Math.max(0, n)); }
export function slice(xs, start, end) {
  if (typeof xs === "string") return Array.from(xs).slice(start, end).join("");
  return xs.slice(start, end);
}
export function reverse(xs) { return typeof xs === "string" ? Array.from(xs).reverse().join("") : [...xs].reverse(); }
export function sort(xs, cmp) {
  const c = cmp ?? ((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  return [...xs].sort(cmp ? (a, b) => (cmp(a, b) === true ? -1 : cmp(a, b) === false ? 1 : cmp(a, b)) : c);
}
export function sort_by(xs, key, descending = false) {
  const keyed = xs.map((x, i) => [key(x), i, x]);
  keyed.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : a[1] - b[1]) * (descending ? -1 : 1));
  return keyed.map((k) => k[2]);
}
export function contains(xs, needle) {
  if (typeof xs === "string") return xs.includes(needle);
  if (xs instanceof PMap) return xs.has(needle);
  if (Array.isArray(xs)) return xs.some((x) => eq(x, needle));
  return Object.prototype.hasOwnProperty.call(xs, needle);
}
export const includes = contains;
export function index_of(xs, needle) {
  if (typeof xs === "string") return Array.from(xs.slice(0, Math.max(0, xs.indexOf(needle)))).length - (xs.indexOf(needle) < 0 ? 1 + Array.from(xs).length : 0);
  return xs.findIndex((x) => eq(x, needle));
}
export function join(xs, sep = "") { return xs.map(str).join(sep); }
export function enumerate(xs) { return xs.map((x, i) => [i, x]); }
export function zip(a, b) { return a.slice(0, Math.min(a.length, b.length)).map((x, i) => [x, b[i]]); }
export function flat(xs) { return xs.flat(1); }
export function flat_map(xs, f) { return xs.flatMap((x) => f(x)); }
export function unique(xs) { return xs.filter((x, i) => xs.findIndex((y) => eq(x, y)) === i); }
export function unique_by(xs, key) {
  const seen = [];
  return xs.filter((x) => { const k = key(x); if (seen.some((s) => eq(s, k))) return false; seen.push(k); return true; });
}
export function map(xs, f) { return xs.map((x) => f(x)); }
export function filter(xs, f) { return xs.filter((x) => f(x)); }
export function reduce(xs, init, f) { return xs.reduce((acc, x) => f(acc, x), init); }
export function forEach(xs, f) { xs.forEach((x) => f(x)); }
export function any(xs, f) { return xs.some((x) => f(x)); }
export function all(xs, f) { return xs.every((x) => f(x)); }
export function find(xs, f) { return xs.find((x) => f(x)) ?? null; }
export function count(xs, f) { return xs.filter((x) => f(x)).length; }
export function sum(xs) { return xs.reduce((a, b) => a + b, 0); }
export function product(xs) { return xs.reduce((a, b) => a * b, 1); }
export function mean(xs) { return xs.length ? sum(xs) / xs.length : null; }
export function minimum(xs) { return xs.length ? Math.min(...xs) : null; }
export function maximum(xs) { return xs.length ? Math.max(...xs) : null; }
export function min_by(xs, key) { return xs.length ? xs.reduce((a, b) => (key(b) < key(a) ? b : a)) : null; }
export function max_by(xs, key) { return xs.length ? xs.reduce((a, b) => (key(b) > key(a) ? b : a)) : null; }
export function top_k(xs, k, key) { return sort_by(xs, key, true).slice(0, k); }
export function group_by(xs, key) {
  const out = {};
  for (const x of xs) { const k = str(key(x)); (out[k] ??= []).push(x); }
  return out;
}
export function top_indices(xs, k) { return xs.map((x, i) => [x, i]).sort((a, b) => b[0] - a[0]).slice(0, k).map((p) => p[1]); }
export function group_by_keys(xs, keyFn) { return group_by(xs, keyFn); }
export function unique_by_keys(xs, keyFn) { return unique_by(xs, keyFn); }
export function argmin(xs) { return xs.indexOf(Math.min(...xs)); }
export function argmax(xs) { return xs.indexOf(Math.max(...xs)); }
export function fill(n, v) { return Array.from({ length: n }, () => v); }
export function set_at(xs, i, v) { const out = [...xs]; out[i < 0 ? xs.length + i : i] = v; return out; }
export function remove_at(xs, i) { return xs.filter((_, j) => j !== (i < 0 ? xs.length + i : i)); }
export function swap(xs, i, j) { const out = [...xs]; [out[i], out[j]] = [out[j], out[i]]; return out; }

// ── records ─────────────────────────────────────────────────────────────────

export function keys(r) { return r instanceof PMap ? r.keys() : Object.keys(r); }
export function values(r) { return r instanceof PMap ? r.values() : Object.values(r); }
export function entries(r) { return r instanceof PMap ? r.entries() : Object.entries(r); }
export function has_field(r, k) { return r != null && typeof r === "object" && Object.prototype.hasOwnProperty.call(r, k); }
export function field(r, k, fallback = null) { return has_field(r, k) ? r[k] : fallback; }
export function remove(r, k) {
  if (r instanceof PMap) { const m = new PMap(r.entries()); m.delete(k); return m; }
  const { [k]: _, ...rest } = r;
  return rest;
}
export function get_in(v, path) { return path.reduce((acc, step) => at(acc, step), v); }
export function set_in(v, path, x) {
  if (path.length === 0) return x;
  const [step, ...rest] = path;
  return with_at(v, step, set_in(at(v, step), rest, x));
}
/** `v[k]`, as Petal reads it: a negative list index counts from the end. */
export function at(v, k) {
  if (v == null) return null;
  if (v instanceof PMap) return v.get(k);
  if (Array.isArray(v) && typeof k === "number" && k < 0) return v[v.length + k];
  return v[k];
}
/** The value `v[k] = x` leaves: a new list, record or map. */
export function with_at(v, k, x) {
  if (v instanceof PMap) { const m = new PMap(v.entries()); m.set(k, x); return m; }
  if (Array.isArray(v)) return set_at(v, k, x);
  return { ...v, [k]: x };
}
export function freeze(v) { return v; }
export function frozen(v) { return v; }

/** Petal's `Map`: a lookup table keyed by content. */
export class PMap {
  constructor(pairs = []) { this._e = []; for (const [k, v] of pairs) this.set(k, v); }
  _i(k) { return this._e.findIndex(([x]) => eq(x, k)); }
  get size() { return this._e.length; }
  get(k) { const i = this._i(k); return i < 0 ? null : this._e[i][1]; }
  has(k) { return this._i(k) >= 0; }
  set(k, v) { const i = this._i(k); if (i < 0) this._e.push([k, v]); else this._e[i][1] = v; return this; }
  delete(k) { const i = this._i(k); if (i >= 0) this._e.splice(i, 1); }
  keys() { return this._e.map(([k]) => k); }
  values() { return this._e.map(([, v]) => v); }
  entries() { return this._e.map(([k, v]) => [k, v]); }
  [Symbol.iterator]() { return this.entries()[Symbol.iterator](); }
}
export function Map(pairs) { return new PMap(pairs ?? []); }

// ── colors and vectors ──────────────────────────────────────────────────────

/** A color literal, `:FF9500` → `hex("FF9500")`: `{r, g, b}` (and `a`) in 0–255. */
export function hex(h) {
  let s = h.replace(/^[:#]/, "");
  if (s.length <= 4) s = Array.from(s).map((c) => c + c).join("");
  const n = (i) => parseInt(s.slice(i, i + 2), 16);
  const c = { r: n(0), g: n(2), b: n(4) };
  if (s.length === 8) c.a = n(6);
  return c;
}
/** A color record (0–255 channels, `a` optional) as CSS. */
export function css_color(c) {
  if (c == null) return undefined;
  if (typeof c === "string") return c;
  const a = c.a === undefined ? 1 : c.a / 255;
  return a >= 1 ? `rgb(${c.r} ${c.g} ${c.b})` : `rgb(${c.r} ${c.g} ${c.b} / ${+a.toFixed(3)})`;
}
export function vec2(x, y) { return { x, y }; }
export function vec3(x, y, z) { return { x, y, z }; }
export function hsv(h, s, v, a) {
  const f = (n) => { const k = (n + h * 6) % 6; return v - v * s * Math.max(0, Math.min(k, 4 - k, 1)); };
  const c = { r: Math.round(f(5) * 255), g: Math.round(f(3) * 255), b: Math.round(f(1) * 255) };
  if (a !== undefined) c.a = Math.round(a * 255);
  return c;
}

// ── output and annotations ──────────────────────────────────────────────────

export function print(...xs) { console.log(...xs.map(str)); }
export function tunable(v) { return v; }
export function rigid(v) { return v; }
export function assert(c, msg) { if (!c) throw new Error(msg ?? "assertion failed"); }
export function assert_eq(a, b) { if (!eq(a, b)) throw new Error(`assert_eq: ${str(a)} != ${str(b)}`); }
let nextUnique = 0;
export function unique_id() { return ++nextUnique; }

/** A function the transpiler could not convert: calling it says so. */
export function unconverted(name) {
  return () => { throw new Error(`petal: \`${name}\` was not converted to React (see the transpile warnings)`); };
}

// ── state shared by several components (`state var` at the top level) ──────

/** A top-level `state var`: one value every component reads and writes. */
export function cell(init) {
  let value = init;
  const subs = new Set();
  return {
    get: () => value,
    set: (v) => { value = v; subs.forEach((f) => f()); },
    update: (f) => { value = f(value); subs.forEach((s) => s()); },
    subscribe: (f) => { subs.add(f); return () => subs.delete(f); },
  };
}

/**
 * An event an element raises (petal-web-html's `next_id()` / `clicked(id)`),
 * when the handlers could not be written as one function: each render
 * registers them with `.on(fn)`; calling the event runs them.
 */
export function petal_event() {
  const handlers = [];
  const fire = (...args) => handlers.forEach((h) => h(...args));
  fire.on = (h) => { handlers.push(h); };
  return fire;
}
