// @ts-check
const { test, expect } = require('@playwright/test');
const path = require('path');
const http = require('http');
const fs = require('fs');

const STATIC_ROOT = path.resolve(__dirname, '../services/api/static');
const PORT = 9876;

const MIME_TYPES = {
  '.html': 'text/html',
  '.js': 'application/javascript',
  '.css': 'text/css',
  '.json': 'application/json',
};

/**
 * Starts a minimal static file server so ES module imports work
 * (file:// protocol blocks cross-origin module loads).
 */
function startServer() {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const url = new URL(req.url, `http://localhost:${PORT}`);
      const filePath = path.join(STATIC_ROOT, url.pathname);

      if (!fs.existsSync(filePath) || fs.statSync(filePath).isDirectory()) {
        res.writeHead(404);
        res.end('Not Found');
        return;
      }

      const ext = path.extname(filePath);
      const mime = MIME_TYPES[ext] || 'application/octet-stream';
      res.writeHead(200, { 'Content-Type': mime });
      fs.createReadStream(filePath).pipe(res);
    });

    server.listen(PORT, () => resolve(server));
  });
}

test.describe('ChatPanel streaming state management', () => {
  /** @type {import('http').Server} */
  let server;

  test.beforeAll(async () => {
    server = await startServer();
  });

  test.afterAll(async () => {
    if (server) await new Promise(r => server.close(r));
  });

  test('all streaming state tests pass', async ({ page }) => {
    // Capture console errors for debugging
    const errors = [];
    page.on('console', msg => {
      if (msg.type() === 'error') errors.push(msg.text());
    });
    page.on('pageerror', err => {
      errors.push(err.message);
    });

    // Suppress the prompt() dialog that ChatPanel._createNewSession() fires
    page.on('dialog', async dialog => {
      await dialog.accept('Test Session');
    });

    await page.goto(`http://localhost:${PORT}/js/__tests__/chat-streaming.test.html`);

    // Wait for tests to complete (they set window.__TEST_RESULTS__)
    try {
      await page.waitForFunction(
        () => window.__TEST_RESULTS__ !== undefined,
        { timeout: 15_000 }
      );
    } catch {
      // If tests didn't run, report console errors
      let output = '';
      try {
        output = await page.locator('#results').innerText();
      } catch { /* page may be closed */ }
      const errText = errors.join('\n');
      throw new Error(
        `Tests did not complete within 15s.\nConsole errors:\n${errText}\nPage output:\n${output}`
      );
    }

    const results = await page.evaluate(() => window.__TEST_RESULTS__);

    // Log test output for visibility
    const output = await page.locator('#results').innerText();
    console.log('\n--- Chat Streaming Test Output ---');
    console.log(output);
    console.log('--- End Test Output ---\n');

    if (results.error) {
      throw new Error(`Test harness error: ${results.error}`);
    }

    expect(results.failed, `${results.failed} test(s) failed:\n${output}`).toBe(0);
    expect(results.passed).toBeGreaterThan(0);
  });
});
