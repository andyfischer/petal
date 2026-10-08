#!/usr/bin/env node --experimental-strip-types
/**
 * MCP server for controlling petal-diagram-canvas via the debug protocol.
 *
 * Connects to ws://localhost:4012/debug and exposes tools:
 * DiagramPause, DiagramResume, DiagramStep, DiagramState,
 * DiagramSetState, DiagramScreenshot, DiagramCaptureDrawCommands, DiagramInput
 */
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import WebSocket from "ws";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const WS_URL = process.env.PETAL_DEBUG_URL ?? "ws://localhost:4012/debug";

type ToolResult = { content: { type: "text"; text: string }[]; isError?: boolean };

// ---------------------------------------------------------------------------
// WebSocket connection to the debug bridge
// ---------------------------------------------------------------------------

let ws: WebSocket | null = null;
let pendingResolve: ((data: any) => void) | null = null;

function ensureConnection(): Promise<void> {
  return new Promise((resolve, reject) => {
    if (ws && ws.readyState === WebSocket.OPEN) {
      resolve();
      return;
    }
    ws = new WebSocket(WS_URL);
    ws.on("open", () => resolve());
    ws.on("error", (err) => reject(new Error(`WebSocket error: ${err.message}`)));
    ws.on("message", (data) => {
      if (pendingResolve) {
        const cb = pendingResolve;
        pendingResolve = null;
        try {
          cb(JSON.parse(data.toString()));
        } catch {
          cb({ ok: false, error: "Invalid JSON response" });
        }
      }
    });
    ws.on("close", () => {
      ws = null;
    });
  });
}

function sendCommand(cmd: Record<string, any>): Promise<any> {
  return new Promise(async (resolve, reject) => {
    try {
      await ensureConnection();
    } catch (e) {
      reject(e);
      return;
    }
    pendingResolve = resolve;
    ws!.send(JSON.stringify(cmd));
    // Timeout after 10 seconds
    setTimeout(() => {
      if (pendingResolve === resolve) {
        pendingResolve = null;
        reject(new Error("Timeout waiting for debug response"));
      }
    }, 10_000);
  });
}

async function debugTool(cmd: Record<string, any>): Promise<ToolResult> {
  try {
    const resp = await sendCommand(cmd);
    return { content: [{ type: "text", text: JSON.stringify(resp, null, 2) }] };
  } catch (e: any) {
    return { content: [{ type: "text", text: `Error: ${e.message}` }], isError: true };
  }
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

const server = new McpServer({
  name: "petal-diagram",
  version: "1.0.0",
});

server.registerTool("DiagramPause", {
  title: "Pause Diagram",
  description: "Pause frame advancement. The canvas will show the last rendered frame.",
  inputSchema: {},
}, () => debugTool({ cmd: "pause" }));

server.registerTool("DiagramResume", {
  title: "Resume Diagram",
  description: "Resume normal-speed playback.",
  inputSchema: {},
}, () => debugTool({ cmd: "resume" }));

server.registerTool("DiagramStep", {
  title: "Step Diagram",
  description: "Advance exactly N frames with fixed dt=1/60s. Returns draw commands for the stepped frames.",
  inputSchema: {
    n: z.coerce.number().int().min(1).default(1).describe("Number of frames to advance"),
  },
}, ({ n }) => debugTool({ cmd: "step", n }));

server.registerTool("DiagramState", {
  title: "Get Diagram State",
  description: "Dump all state variables as a JSON map.",
  inputSchema: {},
}, () => debugTool({ cmd: "state" }));

server.registerTool("DiagramSetState", {
  title: "Set Diagram State",
  description: "Set a state variable by name.",
  inputSchema: {
    name: z.string().describe("The state variable name"),
    value: z.any().describe("The value to set (JSON)"),
  },
}, ({ name, value }) => debugTool({ cmd: "set_state", name, value }));

server.registerTool("DiagramCaptureDrawCommands", {
  title: "Capture Draw Commands",
  description: "Run one speculative frame (no state change) and return the draw commands.",
  inputSchema: {},
}, () => debugTool({ cmd: "capture_draw_commands" }));

server.registerTool("DiagramInput", {
  title: "Inject Input",
  description: "Inject keyboard and/or mouse input.",
  inputSchema: {
    keys_down: z.array(z.string()).optional().describe("Keys to press"),
    mouse: z.object({
      x: z.number(),
      y: z.number(),
      buttons: z.array(z.number()).optional(),
    }).optional().describe("Mouse position and buttons"),
  },
}, ({ keys_down, mouse }) => debugTool({ cmd: "input", keys_down, mouse }));

server.registerTool("DiagramScreenshot", {
  title: "Screenshot Diagram",
  description: "Capture the canvas as a PNG and save it to ./temp/. Returns the file path.",
  inputSchema: {},
}, async () => {
  try {
    const resp = await sendCommand({ cmd: "screenshot" });
    if (!resp.ok || !resp.screenshot) {
      return { content: [{ type: "text" as const, text: `Error: ${resp.error ?? "No screenshot data"}` }], isError: true };
    }
    // Strip data URL prefix and decode
    const base64 = resp.screenshot.replace(/^data:image\/png;base64,/, "");
    const buf = Buffer.from(base64, "base64");
    const tempDir = resolve(import.meta.dirname!, "..", "temp");
    mkdirSync(tempDir, { recursive: true });
    const filename = `screenshot-${Date.now()}.png`;
    const filePath = resolve(tempDir, filename);
    writeFileSync(filePath, buf);
    return { content: [{ type: "text" as const, text: JSON.stringify({ ok: true, paused: resp.paused, frame: resp.frame, file: filePath }, null, 2) }] };
  } catch (e: any) {
    return { content: [{ type: "text" as const, text: `Error: ${e.message}` }], isError: true };
  }
});

const transport = new StdioServerTransport();
await server.connect(transport);
