import { describe, it, expect } from "vitest";
import { resolve } from "path";
import { readdirSync } from "fs";
import { runPetalFile } from "./helpers";

const EXAMPLES_DIR = resolve(__dirname, "../../examples/console");

const samples = readdirSync(EXAMPLES_DIR)
  .filter((f) => f.endsWith(".ptl"))
  .sort();

describe("example programs", () => {
  it.each(samples)("%s runs without error", (file) => {
    // runPetalFile throws if the program fails.
    runPetalFile(resolve(EXAMPLES_DIR, file));
  });
});
