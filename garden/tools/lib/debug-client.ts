// A client for Garden's debug server (see docs/debug-server.md).
//
// The server is the seam every functional test drives the app through: inject
// input the way a user would, then read back the observable state (/state,
// /buffer, /scene, /screenshot). Everything here is a thin, typed wrapper over
// those endpoints — the interesting judgement lives in the tests.

import { writeFile } from "node:fs/promises";

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Cursor {
  line: number;
  col: number;
}

export interface PaneState {
  kind: string;
  rect: Rect;
  mode?: string;
  file?: string;
  dirty?: boolean;
  cursor?: Cursor;
  line_count?: number;
  process?: { name?: string };
  panel?: {
    values?: Record<string, unknown>;
    /** Which panel frame `values` came from, and whether that is the frame
     *  that just ran: a key missing from a *stale* map means the frame that
     *  would have bound it raised, not that its branch never ran. */
    values_frame?: number | null;
    values_stale?: boolean;
    /** The failing frame's own bindings, as far as it got. */
    values_partial?: { frame: number; values: Record<string, unknown> } | null;
    frame?: number;
    awake?: boolean;
    error?: string | null;
  } | null;
}

export interface AppState {
  panes: PaneState[];
  cell: { width: number; height: number };
  window: { scale: number };
  frame?: number;
  command_line?: string | null;
  status_note?: string | null;
  status_error?: string | null;
}

export interface ScenePrimitive {
  type?: string;
  text?: string;
  pos?: [number, number];
  color?: [number, number, number, number];
  rect?: Rect;
  /** For a `mesh`: the batch split back into one entry per fill (consecutive
   *  same-colour triangles), each with its own bounds. Panel fills are all
   *  meshes, and consecutive ones are batched, so this — not the primitive's
   *  own `rect` — is what a layout assertion searches. */
  shapes?: { rect: Rect; color: [number, number, number, number]; triangles: number }[];
}

export interface MouseReply {
  selection?: { text?: string } | null;
}

export interface WindowInfo {
  window: number;
  focused: boolean;
}

/** A pointer button in the debug protocol's numbering (`petal-ui`'s): 0 is the
 *  primary click, 1 the context gesture that panels see as a right-click. */
export const Button = { left: 0, right: 1 } as const;

export class DebugClient {
  base: string;

  constructor(base: string) {
    this.base = base;
  }

  // --- raw transport --------------------------------------------------------

  async getText(path: string): Promise<string> {
    const res = await fetch(this.base + path);
    return await res.text();
  }

  async getJson<T = unknown>(path: string): Promise<T> {
    const res = await fetch(this.base + path);
    return (await res.json()) as T;
  }

  /** POST a JSON body; returns the parsed reply, or null when there isn't one. */
  async post<T = unknown>(path: string, body: unknown): Promise<T | null> {
    const res = await fetch(this.base + path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const text = await res.text();
    try {
      return JSON.parse(text) as T;
    } catch {
      return null;
    }
  }

  // --- reading state --------------------------------------------------------

  state(): Promise<AppState> {
    return this.getJson<AppState>("/state");
  }

  /** `/state` with each panel's `values` map narrowed — pass exact names and/or
   *  a prefix. Unfiltered, `values` is every binding the script made (seeded
   *  data and colour constants included), which on a real app is thousands of
   *  lines per read. `{ values: "none" }` drops the map entirely. */
  stateValues(opts: { values?: string[] | "none"; prefix?: string }): Promise<AppState> {
    const params = new URLSearchParams();
    if (opts.values === "none") params.set("values", "none");
    else if (opts.values?.length) params.set("values", opts.values.join(","));
    if (opts.prefix) params.set("values_prefix", opts.prefix);
    return this.getJson<AppState>(`/state?${params.toString()}`);
  }

  /** Advance every panel by `n` frames of `dt` seconds, ignoring the sleep/wake
   *  window and without fabricating any input — how to drive an animation or a
   *  game deterministically. */
  tick(n = 1, dt = 1 / 60): Promise<{ panel_frames?: number } | null> {
    return this.post<{ panel_frames?: number }>("/tick", { n, dt });
  }

  /** Restart every file-backed panel from source, discarding Petal `state` —
   *  the way to re-run a seeded-data generator without killing the process. */
  resetPanels(): Promise<{ panels_reset?: number } | null> {
    return this.post<{ panels_reset?: number }>("/panel/reset", {});
  }

  scene(): Promise<{ primitives: ScenePrimitive[] }> {
    return this.getJson("/scene");
  }

  windows(): Promise<{ windows: WindowInfo[] }> {
    return this.getJson("/windows");
  }

  frame(): Promise<number | undefined> {
    return this.getJson<{ frame?: number }>("/frame").then((d) => d.frame);
  }

  /** Full text of a pane's buffer (optionally in a specific window ordinal). */
  buffer(pane = 0, window?: number): Promise<string> {
    const q = window === undefined ? "" : `?window=${window}`;
    return this.getText(`/buffer/${pane}${q}`);
  }

  /** A pane's buffer split into lines, with the trailing empty line dropped. */
  async bufferLines(pane = 0, window?: number): Promise<string[]> {
    return splitLines(await this.buffer(pane, window));
  }

  /** First line of a pane's buffer ("" when the buffer is empty). */
  async firstLine(pane = 0, window?: number): Promise<string> {
    return (await this.bufferLines(pane, window))[0] ?? "";
  }

  async pane(i = 0): Promise<PaneState> {
    return (await this.state()).panes[i];
  }

  /**
   * One value the panel's drawer bound on its last frame, by name.
   *
   * Values keep their real JSON types (a bool reads as a bool, an int as an
   * int), so the tests can compare against real types rather than stringly.
   * A name whose term never executed this frame is simply absent; that reads
   * back as `undefined`, which is the sentinel the wait loops test against.
   */
  async panelValue(name: string, pane = 0): Promise<unknown> {
    const p = (await this.state()).panes[pane];
    return (p?.panel?.values ?? {})[name];
  }

  /** The status line's error slot — where a projection's refusal surfaces. */
  async statusError(): Promise<string> {
    return (await this.state()).status_error ?? "";
  }

  async statusNote(): Promise<string> {
    return (await this.state()).status_note ?? "";
  }

  async commandLine(): Promise<string> {
    return (await this.state()).command_line ?? "";
  }

  /** On-screen text runs whose text is exactly `text`. */
  async sceneTextCount(text: string): Promise<number> {
    const { primitives } = await this.scene();
    return primitives.filter((p) => p.type === "text" && p.text === text).length;
  }

  /** On-screen text runs mentioning "error" — how a panel runtime error shows. */
  async sceneErrorCount(): Promise<number> {
    const { primitives } = await this.scene();
    return primitives.filter(
      (p) => p.type === "text" && (p.text ?? "").toLowerCase().includes("error"),
    ).length;
  }

  /** GET /screenshot: writes the PNG to `path`, returns the X-Garden-Frame
   *  header the capture carries (or undefined when the header is missing). */
  async screenshot(path: string): Promise<number | undefined> {
    const res = await fetch(this.base + "/screenshot");
    const header = res.headers.get("x-garden-frame");
    await writeFile(path, Buffer.from(await res.arrayBuffer()));
    return header === null ? undefined : Number(header);
  }

  // --- injecting input ------------------------------------------------------

  async key(key: string, mods: string[] = []): Promise<void> {
    await this.post("/key", { key, mods });
  }

  async text(text: string): Promise<void> {
    await this.post("/text", { text });
  }

  async command(command: string): Promise<void> {
    await this.post("/command", { command });
  }

  /** Type a string one key at a time — command-line input has to be per-key,
   *  not /text, and a space goes in under its key name. */
  async keys(s: string): Promise<void> {
    for (const ch of s) await this.key(ch === " " ? "space" : ch);
  }

  /** Open the command line, type an ex command char by char, and run it. */
  async ex(command: string): Promise<void> {
    await this.key(":");
    await this.keys(command);
    await this.key("enter");
  }

  /** A click; the reply carries the resulting selection, which the multi-click
   *  checks read. */
  click(
    x: number,
    y: number,
    opts: { clicks?: number; button?: number } = {},
  ): Promise<MouseReply | null> {
    return this.post<MouseReply>("/mouse", {
      op: "click",
      x: Math.round(x),
      y: Math.round(y),
      ...opts,
    });
  }

  /** The context gesture: `button: 1`. Panels are the only thing that sees it. */
  rightClick(x: number, y: number): Promise<MouseReply | null> {
    return this.click(x, y, { button: Button.right });
  }

  async scroll(x: number, y: number, lines: number): Promise<void> {
    await this.post("/mouse", { op: "scroll", x: Math.round(x), y: Math.round(y), lines });
  }

  /** `op` is down | move | up, for dragging. */
  async mouse(op: string, x: number, y: number): Promise<void> {
    await this.post("/mouse", { op, x: Math.round(x), y: Math.round(y) });
  }

  /** A click at pane-local coordinates: /mouse takes window coordinates, so
   *  every panel-local hit target is offset by the pane's origin. */
  async clickPaneLocal(
    x: number,
    y: number,
    opts: { clicks?: number; button?: number; pane?: number } = {},
  ): Promise<MouseReply | null> {
    const { pane = 0, ...rest } = opts;
    const r = (await this.pane(pane)).rect;
    return await this.click(r.x + x, r.y + y, rest);
  }

  async rightClickPaneLocal(x: number, y: number, pane = 0): Promise<void> {
    await this.clickPaneLocal(x, y, { button: Button.right, pane });
  }

  async scrollPaneLocal(x: number, y: number, lines: number, pane = 0): Promise<void> {
    const r = (await this.pane(pane)).rect;
    await this.scroll(r.x + x, r.y + y, lines);
  }

  async mousePaneLocal(op: string, x: number, y: number, pane = 0): Promise<void> {
    const r = (await this.pane(pane)).rect;
    await this.mouse(op, r.x + x, r.y + y);
  }
}

/** Split buffer text into lines, dropping the trailing empty line a final
 *  newline leaves behind (so `lines[0]` is the shell tests' `head -1`). */
export function splitLines(text: string): string[] {
  const out = text.split("\n");
  if (out.length > 0 && out[out.length - 1] === "") out.pop();
  return out;
}
