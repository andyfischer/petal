import { execSync } from "child_process";
import { resolve } from "path";

export default function globalSetup() {
  const root = resolve(__dirname, "..");
  execSync("cargo build --manifest-path rust/Cargo.toml", {
    cwd: root,
    stdio: "pipe",
  });
  try {
    execSync("bash petal-diagram-canvas/build-wasm.sh", {
      cwd: root,
      stdio: "pipe",
    });
  } catch {
    // wasm-pack may not be installed in all environments (e.g. CI);
    // the core test suite does not require the WASM build.
  }
}
