import { execSync } from "child_process";
import { resolve } from "path";

export default function globalSetup() {
  const root = resolve(__dirname, "..");
  execSync("cargo build --manifest-path rust/Cargo.toml", {
    cwd: root,
    stdio: "pipe",
  });
  execSync("bash petal-diagram-canvas/build-wasm.sh", {
    cwd: root,
    stdio: "pipe",
  });
}
