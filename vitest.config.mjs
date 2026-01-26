import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['ts/**/*.test.ts'],
    forceRerunTriggers: ['dist/wasm-node/main.js', 'dist/wasm-node/main.wasm'],
  },
}) 
