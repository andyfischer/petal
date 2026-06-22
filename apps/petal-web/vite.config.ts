import { defineConfig, type Plugin } from "vite";
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

export default defineConfig({
  plugins: [wasm(), petalMimePlugin()],
  build: {
    target: "esnext",
  },
});
