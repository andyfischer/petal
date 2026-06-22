import { execSync } from "child_process";
import { resolve, dirname } from "path";

/**
 * Locate a usable `cargo`. Portable across CI and local machines:
 *   1. An explicit CARGO_PATH override, if set.
 *   2. `rustup which cargo` — the active toolchain's cargo (with rustc beside
 *      it), wherever rustup installed it.
 *   3. Plain `cargo` from PATH (CI images that install cargo directly).
 */
function locateCargo(): string {
  if (process.env.CARGO_PATH) return process.env.CARGO_PATH;
  try {
    const p = execSync("rustup which cargo", {
      stdio: ["ignore", "pipe", "ignore"],
    })
      .toString()
      .trim();
    if (p) return p;
  } catch {
    // rustup not installed — fall through to PATH lookup.
  }
  return "cargo";
}

export default function globalSetup() {
  const root = resolve(__dirname, "..", "..");
  const cargo = locateCargo();

  // Ensure the directory holding the chosen cargo (and its sibling rustc) is on
  // PATH, so the build finds rustc even when launched with a minimal environment.
  const cargoDir = cargo.includes("/") ? dirname(cargo) : null;
  const env = cargoDir
    ? { ...process.env, PATH: `${cargoDir}:${process.env.PATH ?? ""}` }
    : process.env;

  execSync(`${cargo} build --manifest-path rust/Cargo.toml`, {
    cwd: root,
    stdio: "pipe",
    env,
  });

  try {
    execSync("bash apps/petal-diagram-canvas/build-wasm.sh", {
      cwd: root,
      stdio: "pipe",
      env,
    });
  } catch {
    // wasm-pack may not be installed in all environments (e.g. CI);
    // the core test suite does not require the WASM build.
  }
}
