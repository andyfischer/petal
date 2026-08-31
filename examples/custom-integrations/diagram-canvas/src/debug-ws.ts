/** Browser-side WebSocket client for the debug protocol.
 *
 * Connects to ws://localhost:<port>/debug with subprotocol "petal-debug-browser"
 * so the Vite relay knows this is the browser client. External tools connect
 * without the subprotocol and their commands are forwarded here.
 */

import type { PetalDebugAPI, DebugCommand } from "./debug.js";

export function connectDebugWebSocket(api: PetalDebugAPI, port: number): void {
  let ws: WebSocket | null = null;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  function connect() {
    try {
      ws = new WebSocket(`ws://localhost:${port}/debug`, "petal-debug-browser");
    } catch {
      scheduleReconnect();
      return;
    }

    ws.onopen = () => {
      console.log("[petal-debug] WebSocket connected");
    };

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        const requestId = msg._requestId;
        delete msg._requestId;

        const cmd: DebugCommand = msg;
        const response = api.handleCommand(cmd);

        // Attach requestId so the relay can route the response back
        if (requestId != null) {
          (response as any)._requestId = requestId;
        }
        ws?.send(JSON.stringify(response));
      } catch (e) {
        ws?.send(JSON.stringify({ ok: false, error: String(e) }));
      }
    };

    ws.onclose = () => {
      scheduleReconnect();
    };

    ws.onerror = () => {
      ws?.close();
    };
  }

  function scheduleReconnect() {
    if (reconnectTimer) return;
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, 2000);
  }

  connect();
}
