const { defineConfig } = require('@playwright/test');

module.exports = defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  retries: 0,
  use: {
    headless: true,
  },
  projects: [
    {
      name: 'unit-tests',
      testMatch: '**/*.unit.spec.js',
    },
    {
      name: 'e2e-tests',
      testMatch: '**/*.e2e.spec.js',
    },
  ],
});
