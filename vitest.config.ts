import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/vitest/**/*.test.ts"],
    testTimeout: 30000,
    globalSetup: ["test/vitest/global-setup.ts"],
  },
});
