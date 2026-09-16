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
  panel?: {
    /** The GPP client app driving this pane (its spawn command), or null for
     *  an in-process panel. */
    client?: string | null;
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
  /** The focused pane (index), and its cursor and selection repeated at the top
   *  level — the default input acknowledgment is exactly these three. */
  focus?: number;
  cursor?: { line: number; col: number } | null;
  selection?: { text?: string } | null;
  command_line?: string | null;
  status_note?: string | null;
  status_error?: string | null;
}

/** `GET /version` — what the binary on the other end of the socket is. */
export interface VersionReport {
  version: string;
  build: {
    version: string;
    commit: string;
    commit_date: string;
    build_date: string;
    dirty: boolean;
    prelude_level: number;
  };
  /** Named capabilities this build has, e.g. `cli.panel-wake`. */
  features: string[];
  prelude: {
    level: number;
    ui_version: number;
    /** `name/arity` per exported prelude function, `name` per exported value. */
    exports: string[];
  };
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
  clip?: Rect;
  /** False when the primitive is provably clipped away — a row of a scrolling
   *  list scrolled past its viewport, say. The scene carries those primitives
   *  with the clip that removes them, so counting runs without this counts
   *  things nobody can see. */
  visible?: boolean;
}

/** A text run `GET /scene?find=` matched: the primitive plus the rect it
 *  occupies and the point a click should land on, in the reply's coordinates
 *  (window, or pane-relative under `pane=`). */
export interface SceneMatch extends ScenePrimitive {
  id: number;
  text: string;
  rect: Rect;
  center: [number, number];
}

/** The `GET /scene?find=` reply. */
export interface SceneFindReply {
  primitives: SceneMatch[];
  matches: number;
  frame?: number;
  /** Present under `pane=`: which pane, and its rect in window coordinates. */
  pane?: { index: number; rect: Rect };
}

/** A located text run, in window coordinates — what `click` takes. */
export interface Located {
  x: number;
  y: number;
  rect: Rect;
  text: string;
}

export interface LocateOptions {
  /** Search only this pane (the result is still in window coordinates). */
  pane?: number;
  /** Match runs containing `text`, rather than runs that are exactly it. */
  contains?: boolean;
  /** Which match, in draw order, when several are visible (default 0). */
  nth?: number;
}

export interface MouseReply {
  selection?: { text?: string } | null;
}

/** One step of `POST /batch` (feature `debug.batch`): an endpoint's own body
 *  plus its `path` (query and a per-step `select=` allowed) and `method`
 *  (default POST). */
export interface BatchStep {
  path: string;
  method?: "GET" | "POST";
  [field: string]: unknown;
}

export interface BatchReply {
  ok: boolean;
  /** One entry per step that ran, in order. A text reply is `{ok, text}`. */
  results: unknown[];
  /** On a failed step: its index and error; `results` holds the steps before it. */
  failed?: number;
  error?: string;
}

/** A `/mouse` body at pane-local coordinates, for [`DebugClient.gesturePaneLocal`]. */
export interface PaneMouseStep {
  op: string;
  x?: number;
  y?: number;
  to?: { x: number; y: number };
  /** Project the settled snapshot right after this step (e.g. a mid-drag value). */
  select?: string[];
  [field: string]: unknown;
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

  /** Any JSON endpoint projected by `?select=` (feature `state.select`): only
   *  the given dotted paths (`panes.0.cursor`, `panes.*.panel.values.sel`,
   *  `values.obs_*`), each kept where it was, so `reply.panes[0].cursor` reads
   *  the same as on the full reply. Unmatched paths are absent. */
  select<T = Partial<AppState>>(path: string, fields: string[]): Promise<T> {
    const sep = path.includes("?") ? "&" : "?";
    return this.getJson<T>(`${path}${sep}select=${encodeURIComponent(fields.join(","))}`);
  }

  /** What build is answering: version, git stamp, feature flags, prelude
   *  exports. Ask this before using a newer endpoint or flag rather than
   *  reading its error — see `docs/debug-server.md`. */
  version(): Promise<VersionReport> {
    return this.getJson<VersionReport>("/version");
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

  /** `GET /scene?find=text:…`: the text runs matching `text` exactly (trimmed),
   *  or containing it with `contains`, each with its rect and center. */
  findText(text: string, opts: { pane?: number; contains?: boolean } = {}): Promise<SceneFindReply> {
    const params = new URLSearchParams();
    params.set("find", `${opts.contains ? "text~" : "text"}:${text}`);
    if (opts.pane !== undefined) params.set("pane", String(opts.pane));
    return this.getJson<SceneFindReply>(`/scene?${params.toString()}`);
  }

  /** Where a visible text run is drawn, as a clickable window-space point (the
   *  run's center), or null when no visible run matches. The locator a test
   *  uses instead of hard-coding where a label was last laid out. */
  async locate(text: string, opts: LocateOptions = {}): Promise<Located | null> {
    const reply = await this.findText(text, opts);
    const hit = (reply.primitives ?? []).filter((p) => p.visible !== false)[opts.nth ?? 0];
    if (!hit) return null;
    const dx = reply.pane?.rect.x ?? 0;
    const dy = reply.pane?.rect.y ?? 0;
    return {
      x: hit.center[0] + dx,
      y: hit.center[1] + dy,
      rect: { ...hit.rect, x: hit.rect.x + dx, y: hit.rect.y + dy },
      text: hit.text,
    };
  }

  /** `locate`, but a missing label is an error naming it. */
  async mustLocate(text: string, opts: LocateOptions = {}): Promise<Located> {
    const at = await this.locate(text, opts);
    if (!at) throw new Error(`no visible text run ${opts.contains ? "containing" : "reading"} ${JSON.stringify(text)}`);
    return at;
  }

  /** Click the center of a visible text run. */
  async clickText(
    text: string,
    opts: LocateOptions & { clicks?: number; button?: number } = {},
  ): Promise<MouseReply | null> {
    const { clicks, button, ...where } = opts;
    const at = await this.mustLocate(text, where);
    return await this.click(at.x, at.y, { clicks, button });
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

  /** Every text run the clip actually keeps. */
  async sceneVisibleTexts(): Promise<ScenePrimitive[]> {
    const { primitives } = await this.scene();
    return primitives.filter((p) => p.type === "text" && p.visible !== false);
  }

  /** On-screen text runs whose text is exactly `text` (clipped-away runs, which
   *  the scene still carries, do not count). */
  async sceneTextCount(text: string): Promise<number> {
    return (await this.sceneVisibleTexts()).filter((p) => p.text === text).length;
  }

  /** On-screen text runs mentioning "error" — how a panel runtime error shows. */
  async sceneErrorCount(): Promise<number> {
    return (await this.sceneVisibleTexts()).filter((p) =>
      (p.text ?? "").toLowerCase().includes("error"),
    ).length;
  }

  /** `GET /capture?format=…` — the one capture `/scene` and `/screenshot` alias.
   *  `json` is the scene dump (parsed); `png` / `text` come back as the raw
   *  response, whose `x-garden-frame` header carries the captured frame. A
   *  frontend that cannot make a format (png under `--term`, text on a pixel
   *  frontend) answers 400, which throws here. Needs `debug.capture`. */
  async capture(format: "png" | "text", opts?: { pane?: number }): Promise<Response>;
  async capture(format: "json", opts?: { pane?: number }): Promise<{ primitives: ScenePrimitive[]; frame: number }>;
  async capture(
    format: "png" | "json" | "text",
    opts: { pane?: number } = {},
  ): Promise<Response | { primitives: ScenePrimitive[]; frame: number }> {
    const params = new URLSearchParams({ format });
    if (opts.pane !== undefined) params.set("pane", String(opts.pane));
    const path = `/capture?${params.toString()}`;
    if (format === "json") return this.getJson(path);
    const res = await fetch(this.base + path);
    if (!res.ok) throw new Error(`GET ${path} -> ${res.status}: ${await res.text()}`);
    return res;
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

  /** Send a key that is meant to end the process (Cmd-Q). The app can tear the
   *  debug server down before the reply is written, so a closed connection here
   *  is the expected outcome, not a failure — the caller waits on the process. */
  async keyQuitting(key: string, mods: string[] = []): Promise<void> {
    try {
      await this.key(key, mods);
    } catch {
      // connection closed by the exiting app
    }
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

  /** A command with `?select=` (feature `debug.command-select`): the reply is
   *  that projection of the `/state` snapshot taken after the command ran and
   *  panels settled, with the command's own receipt fields (`panel_frames`,
   *  `action`, …) on top — one round trip instead of a POST then a GET. */
  async postSelect<T = Partial<AppState>>(
    path: string,
    body: unknown,
    fields: string[],
  ): Promise<T> {
    const sep = path.includes("?") ? "&" : "?";
    const reply = await this.post<T>(`${path}${sep}select=${encodeURIComponent(fields.join(","))}`, body);
    if (reply === null) throw new Error(`POST ${path}: no JSON reply`);
    return reply;
  }

  /** A pane-local click that answers with the named observed values of that
   *  pane's panel as they stand after the click settled (feature
   *  `debug.command-select`). A name the frame did not bind is absent. */
  async clickPaneLocalValues(
    x: number,
    y: number,
    names: string[],
    pane = 0,
  ): Promise<Record<string, unknown>> {
    const r = (await this.pane(pane)).rect;
    const reply = await this.postSelect<{ panes?: ({ panel?: { values?: Record<string, unknown> } } | null)[] }>(
      "/mouse",
      { op: "click", x: Math.round(r.x + x), y: Math.round(r.y + y) },
      names.map((n) => `panes.${pane}.panel.values.${n}`),
    );
    return reply.panes?.[pane]?.panel?.values ?? {};
  }

  async rightClickPaneLocal(x: number, y: number, pane = 0): Promise<void> {
    await this.clickPaneLocal(x, y, { button: Button.right, pane });
  }

  async scrollPaneLocal(x: number, y: number, lines: number, pane = 0): Promise<void> {
    await this.gesturePaneLocal([{ op: "scroll", x, y, lines }], pane);
  }

  async mousePaneLocal(op: string, x: number, y: number, pane = 0): Promise<void> {
    await this.gesturePaneLocal([{ op, x, y }], pane);
  }

  /** Several commands in one event-loop visit (feature `debug.batch`): no frame
   *  and no other request runs between the steps. Throws when a step fails,
   *  after the steps before it have run. */
  async batch(steps: BatchStep[]): Promise<unknown[]> {
    const reply = await this.post<BatchReply>("/batch", steps);
    if (reply === null) throw new Error("POST /batch: no JSON reply");
    if (!reply.ok) throw new Error(`POST /batch: ${reply.error ?? JSON.stringify(reply)}`);
    return reply.results;
  }

  /** A whole mouse gesture at pane-local coordinates as one atomic batch — e.g.
   *  `[{op: "down", ...}, {op: "move", ..., select: ["panes.0.panel.values.w"]},
   *  {op: "up"}]`. The pane origin is read once, not once per step. Returns
   *  each step's reply; a step with `select` answers with that projection of
   *  the settled snapshot taken right after it. */
  async gesturePaneLocal(steps: PaneMouseStep[], pane = 0): Promise<unknown[]> {
    const r = (await this.pane(pane)).rect;
    const round = (v: number | undefined, o: number) => (v === undefined ? undefined : Math.round(o + v));
    return await this.batch(
      steps.map(({ select, to, x, y, ...rest }) => ({
        path: select?.length ? `/mouse?select=${encodeURIComponent(select.join(","))}` : "/mouse",
        ...rest,
        x: round(x, r.x),
        y: round(y, r.y),
        ...(to ? { to: { x: Math.round(r.x + to.x), y: Math.round(r.y + to.y) } } : {}),
      })),
    );
  }
}

/** Split buffer text into lines, dropping the trailing empty line a final
 *  newline leaves behind (so `lines[0]` is the shell tests' `head -1`). */
export function splitLines(text: string): string[] {
  const out = text.split("\n");
  if (out.length > 0 && out[out.length - 1] === "") out.pop();
  return out;
}
