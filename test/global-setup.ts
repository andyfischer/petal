import { execSync } from "child_process";
import { resolve } from "path";

export default function globalSetup() {
  execSync("cargo build --manifest-path rust/Cargo.toml", {
    cwd: resolve(__dirname, ".."),
    stdio: "pipe",
  });
}
