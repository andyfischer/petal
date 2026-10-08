import { defineConfig, type Plugin } from "vite";
import { WebSocketServer } from "ws";
import wasm from "vite-plugin-wasm";

/** Serve .ptl files as text/plain so fetch() gets the source code. */
function petalMimePlugin(): Plugin {
  return {
    name: "petal-mime",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        if (req.url?.endsWith(".ptl")) {
          res.setHeader("Content-Type", "text/plain; charset=utf-8");
        }
        next();
      });
    },
  };
}

/**
 * WebSocket bridge for the debug protocol (dev only).
 *
 * Architecture: external tools connect to ws://localhost:4012/debug.
 * The browser also connects to the same endpoint. Messages from
 * external clients are forwarded to the browser client, and the
 * browser's responses are forwarded back to the external client.
 */
function debugWebSocketPlugin(): Plugin {
  return {
    name: "petal-debug-ws",
    configureServer(server) {
      const wss = new WebSocketServer({ noServer: true });

      // The browser debug client (first to connect, or the one that identifies itself)
      let browserClient: import("ws").WebSocket | null = null;
      const pendingResponses = new Map<number, import("ws").WebSocket>();
      let nextMsgId = 1;

      wss.on("connection", (ws, req) => {
        const isBrowser = req.headers["sec-websocket-protocol"] === "petal-debug-browser";

        if (isBrowser) {
          browserClient = ws;
          console.log("[petal-debug] Browser client connected");
          ws.on("message", (data) => {
            // Response from browser — route back to the requesting external client
            try {
              const msg = JSON.parse(data.toString());
              const requestId = msg._requestId;
              if (requestId != null) {
                const externalClient = pendingResponses.get(requestId);
                pendingResponses.delete(requestId);
                delete msg._requestId;
                externalClient?.send(JSON.stringify(msg));
              }
            } catch {}
          });
          ws.on("close", () => {
            browserClient = null;
            console.log("[petal-debug] Browser client disconnected");
          });
        } else {
          // External tool client
          console.log("[petal-debug] External client connected");
          ws.on("message", (data) => {
            if (!browserClient || browserClient.readyState !== 1) {
              ws.send(JSON.stringify({ ok: false, error: "No browser client connected" }));
              return;
            }
            try {
              const cmd = JSON.parse(data.toString());
              const requestId = nextMsgId++;
              cmd._requestId = requestId;
              pendingResponses.set(requestId, ws);
              browserClient.send(JSON.stringify(cmd));
            } catch (e) {
              ws.send(JSON.stringify({ ok: false, error: "Invalid JSON" }));
            }
          });
          ws.on("close", () => {
            console.log("[petal-debug] External client disconnected");
          });
        }
      });

      server.httpServer?.on("upgrade", (req, socket, head) => {
        if (req.url === "/debug") {
          wss.handleUpgrade(req, socket, head, (ws) => {
            wss.emit("connection", ws, req);
          });
        }
      });
    },
  };
}

export default defineConfig({
  plugins: [wasm(), petalMimePlugin(), debugWebSocketPlugin()],
  server: {
    port: 4012,
  },
  build: {
    target: "esnext",
  },
});
