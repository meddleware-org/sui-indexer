// Vitest settings for the contract tests: they start real processes and a database, so the timeouts are generous.
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    // The tests run in Node (they spawn the API binary and read it over HTTP).
    environment: 'node',
    // Only the contract test files in this directory.
    include: ['*.test.ts'],
    testTimeout: 20_000,
    hookTimeout: 60_000,
  },
})
