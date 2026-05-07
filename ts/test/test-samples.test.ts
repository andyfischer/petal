import { describe, it, expect, beforeAll } from "vitest";
import { execSync } from "child_process";
import { resolve, basename } from "path";
import { readdirSync } from "fs";

const PETAL = resolve(__dirname, "../../rust/target/debug/petal");
const EXAMPLES_DIR = resolve(__dirname, "../../examples");
const TIMEOUT = 3000;

import { ensureBuild } from "./helpers";

beforeAll(() => ensureBuild());

const samples = readdirSync(EXAMPLES_DIR)
  .filter((f) => f.endsWith(".ptl"))
  .sort();

describe("example programs", () => {
  it.each(samples)("%s runs without error", (file) => {
    const filePath = resolve(EXAMPLES_DIR, file);
    const result = execSync(`${PETAL} ${filePath}`, {
      encoding: "utf-8",
      timeout: TIMEOUT,
      stdio: ["pipe", "pipe", "pipe"],
    });
    // If we get here without throwing, the program ran successfully
  });
});
