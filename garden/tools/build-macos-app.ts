#!/usr/bin/env node
//
// Build Garden as a macOS .app bundle with its Dock/Finder icon.
//
// Usage:  node tools/build-macos-app.ts
//         GARDEN_PROFILE=dev node tools/build-macos-app.ts

import { chmodSync, existsSync } from "node:fs";
import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { platform } from "node:os";
import { join } from "node:path";

import { GARDEN_DIR } from "./lib/app.ts";
import { die, runOrDie } from "./lib/util.ts";

const profile = process.env.GARDEN_PROFILE ?? "release";
const cargoProfileArgs = profile === "release" ? ["--release"] : ["--profile", profile];
const targetProfileDir = profile;

const appDir = join(GARDEN_DIR, "target", targetProfileDir, "bundle", "macos", "Garden.app");
const contentsDir = join(appDir, "Contents");
const macosDir = join(contentsDir, "MacOS");
const resourcesDir = join(contentsDir, "Resources");
const iconSrc = join(GARDEN_DIR, "garden-app", "assets", "macos", "Garden.icns");

// `git-viewers` produces `git-log` (`:Git`); `garden-diff` is the diff/review
// client behind `:Diff`, `:Review*`, `:PR`, `garden diff`, and `garden pr`.
const CRATES = ["garden-app", "directory-browser", "git-viewers", "garden-diff"];
const BINARIES = ["garden", "directory-browser", "git-log", "garden-diff"];

if (platform() !== "darwin") die("macOS app bundles can only be built on Darwin/macOS");
if (!existsSync(iconSrc)) die(`expected icon at ${iconSrc}`);

console.log(`==> Building Garden binaries (${profile})`);
await runOrDie(
  "cargo",
  [
    "build",
    ...cargoProfileArgs,
    ...CRATES.flatMap((c) => ["-p", c]),
    "--manifest-path",
    join(GARDEN_DIR, "Cargo.toml"),
  ],
  { message: "cargo build failed" },
);

for (const name of BINARIES) {
  const built = join(GARDEN_DIR, "target", targetProfileDir, name);
  if (!existsSync(built)) die(`expected built binary at ${built}`);
}

console.log(`==> Creating app bundle -> ${appDir}`);
await mkdir(macosDir, { recursive: true });
await mkdir(resourcesDir, { recursive: true });
for (const name of BINARIES) {
  const dest = join(macosDir, name);
  await copyFile(join(GARDEN_DIR, "target", targetProfileDir, name), dest);
  chmodSync(dest, 0o755);
}
await copyFile(iconSrc, join(resourcesDir, "Garden.icns"));
chmodSync(join(resourcesDir, "Garden.icns"), 0o644);

await writeFile(
  join(contentsDir, "Info.plist"),
  `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Garden</string>
  <key>CFBundleExecutable</key>
  <string>garden</string>
  <key>CFBundleIconFile</key>
  <string>Garden</string>
  <key>CFBundleIdentifier</key>
  <string>com.andyfischer.garden</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Garden</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
`,
);

await writeFile(join(contentsDir, "PkgInfo"), "APPL????");

console.log(`Done. Open ${appDir}`);
