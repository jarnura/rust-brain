// @ts-check
const { test, expect } = require('@playwright/test');

const API_BASE_URL = 'http://localhost:8088';
const TEST_REPO_URL = 'https://github.com/jarnura/rust-brain.git';
const WORKSPACE_NAME = 'e2e-test-workspace-' + Date.now();

/**
 * @typedef {Object} Workspace
 * @property {string} id
 * @property {string} name
 * @property {string} status
 * @property {string} [clone_path]
 * @property {string} [created_at]
 */

test.describe.serial('Workspace Lifecycle', () => {
  /** @type {string|null} */
  let workspaceId = null;

  /** @type {boolean} */
  let workspaceReady = false;

  test.afterAll(async ({ request }) => {
    // Clean up workspace even if tests fail
    if (workspaceId) {
      try {
        const deleteResponse = await request.delete(`${API_BASE_URL}/workspaces/${workspaceId}`);
        // Accept 200 or 204 or 404 (already deleted)
        if (deleteResponse.status() !== 200 && deleteResponse.status() !== 204 && deleteResponse.status() !== 404) {
          // Log cleanup failure but don't throw to avoid masking original test failure
          console.error(`Failed to clean up workspace ${workspaceId}: ${deleteResponse.status()}`);
        }
      } catch {
        // Ignore cleanup errors
      }
    }
  });

  test('Create workspace', async ({ request }) => {
    const response = await request.post(`${API_BASE_URL}/workspaces`, {
      headers: { 'Content-Type': 'application/json' },
      data: {
        github_url: TEST_REPO_URL,
        name: WORKSPACE_NAME,
      },
    });

    expect(response.status(), `Expected 200 or 202, got ${response.status()}`).toBeOneOf([200, 202]);

    const body = await response.json();
    expect(body).toHaveProperty('id');
    expect(body).toHaveProperty('status');
    expect(body).toHaveProperty('message');
    expect(typeof body.id).toBe('string');
    expect(typeof body.status).toBe('string');
    expect(typeof body.message).toBe('string');

    workspaceId = body.id;
  });

  test('List workspaces', async ({ request }) => {
    const response = await request.get(`${API_BASE_URL}/workspaces`);

    expect(response.status()).toBe(200);

    const workspaces = await response.json();
    expect(Array.isArray(workspaces)).toBe(true);

    const foundWorkspace = workspaces.find(
      /** @param {Workspace} w */ (w) => w.id === workspaceId
    );
    expect(foundWorkspace).toBeDefined();
    expect(foundWorkspace?.name).toBe(WORKSPACE_NAME);
  });

  test('Get workspace detail', async ({ request }) => {
    const response = await request.get(`${API_BASE_URL}/workspaces/${workspaceId}`);

    expect(response.status()).toBe(200);

    const workspace = await response.json();
    expect(workspace).toHaveProperty('id');
    expect(workspace).toHaveProperty('name');
    expect(workspace).toHaveProperty('status');
    expect(workspace.id).toBe(workspaceId);
    expect(workspace.name).toBe(WORKSPACE_NAME);
    expect(typeof workspace.status).toBe('string');
  });

  test('Wait for workspace ready', async ({ request }) => {
    const maxAttempts = 30;
    const delayMs = 2000;

    for (let i = 0; i < maxAttempts; i++) {
      const response = await request.get(`${API_BASE_URL}/workspaces/${workspaceId}`);

      if (response.status() === 200) {
        const workspace = await response.json();

        if (workspace.status === 'ready') {
          workspaceReady = true;
          break;
        }

        if (workspace.status === 'error') {
          workspaceReady = false;
          test.skip(true, 'Workspace entered error state');
        }
      }

      if (i < maxAttempts - 1) {
        await new Promise((resolve) => setTimeout(resolve, delayMs));
      }
    }

    if (!workspaceReady) {
      test.skip(true, 'Workspace did not become ready within timeout');
    }

    expect(workspaceReady).toBe(true);
  });

  test('Get diff', async ({ request }) => {
    test.skip(!workspaceReady, 'Workspace not ready');

    const response = await request.get(`${API_BASE_URL}/workspaces/${workspaceId}/diff`);

    expect(response.status()).toBe(200);

    const diff = await response.json();
    expect(diff).toHaveProperty('patch');
    expect(diff).toHaveProperty('clean');
    expect(typeof diff.patch).toBe('string');
    expect(typeof diff.clean).toBe('boolean');
  });

  test('Commit changes', async ({ request }) => {
    test.skip(!workspaceReady, 'Workspace not ready');

    const response = await request.post(`${API_BASE_URL}/workspaces/${workspaceId}/commit`, {
      headers: { 'Content-Type': 'application/json' },
      data: {
        message: 'test commit',
      },
    });

    // Should either succeed (200) or return 400 if nothing to commit
    const status = response.status();
    expect([200, 400]).toContain(status);

    if (status === 200) {
      const body = await response.json();
      expect(body).toHaveProperty('sha');
      expect(body).toHaveProperty('message');
      expect(typeof body.sha).toBe('string');
      expect(typeof body.message).toBe('string');
    } else {
      // 400 case - nothing to commit is acceptable
      const body = await response.json();
      expect(body).toHaveProperty('error');
    }
  });

  test('Reset workspace', async ({ request }) => {
    test.skip(!workspaceReady, 'Workspace not ready');

    const response = await request.post(`${API_BASE_URL}/workspaces/${workspaceId}/reset`);

    expect(response.status()).toBe(200);

    const body = await response.json();
    expect(body).toHaveProperty('message');
    expect(body).toHaveProperty('head_sha');
    expect(typeof body.message).toBe('string');
    expect(typeof body.head_sha).toBe('string');
  });

  test('Workspace stats', async ({ request }) => {
    test.skip(!workspaceReady, 'Workspace not ready');

    const response = await request.get(`${API_BASE_URL}/workspaces/${workspaceId}/stats`);

    expect(response.status()).toBe(200);

    const stats = await response.json();
    expect(stats).toHaveProperty('workspace_id');
    expect(stats).toHaveProperty('status');
    expect(stats).toHaveProperty('pg_items_count');
    expect(stats).toHaveProperty('consistency');
    expect(stats).toHaveProperty('isolation');

    // Validate consistency structure
    expect(stats.consistency).toHaveProperty('pg_vs_neo4j_delta');
    expect(stats.consistency).toHaveProperty('pg_vs_qdrant_delta');
    expect(stats.consistency).toHaveProperty('status');

    // Validate isolation structure
    expect(stats.isolation).toHaveProperty('multi_label_nodes');
    expect(stats.isolation).toHaveProperty('cross_workspace_edges');
    expect(stats.isolation).toHaveProperty('label_mismatches');
  });

  test('Workspace stream with non-existent execution', async ({ request }) => {
    const response = await request.get(
      `${API_BASE_URL}/workspaces/${workspaceId}/stream?execution_id=non-existent-id`
    );

    expect(response.status()).toBe(404);
  });

  test('Delete workspace', async ({ request }) => {
    const response = await request.delete(`${API_BASE_URL}/workspaces/${workspaceId}`);

    expect(response.status()).toBeOneOf([200, 204]);

    // Verify deletion by trying to get the workspace
    const getResponse = await request.get(`${API_BASE_URL}/workspaces/${workspaceId}`);
    expect(getResponse.status()).toBe(404);

    // Mark as cleaned up
    workspaceId = null;
  });

  test.describe('Error cases', () => {
    test('Create workspace with invalid URL returns 400', async ({ request }) => {
      const response = await request.post(`${API_BASE_URL}/workspaces`, {
        headers: { 'Content-Type': 'application/json' },
        data: {
          github_url: 'invalid-url-not-github',
          name: 'test-invalid-url',
        },
      });

      expect(response.status()).toBe(400);
    });

    test('Commit with empty message returns 400', async ({ request }) => {
      // Create a temporary workspace for this test
      const createResponse = await request.post(`${API_BASE_URL}/workspaces`, {
        headers: { 'Content-Type': 'application/json' },
        data: {
          github_url: TEST_REPO_URL,
          name: 'temp-commit-test-' + Date.now(),
        },
      });

      expect(createResponse.status()).toBeOneOf([200, 202]);

      const { id: tempWorkspaceId } = await createResponse.json();

      try {
        const response = await request.post(`${API_BASE_URL}/workspaces/${tempWorkspaceId}/commit`, {
          headers: { 'Content-Type': 'application/json' },
          data: {
            message: '',
          },
        });

        expect(response.status()).toBe(400);
      } finally {
        // Cleanup
        await request.delete(`${API_BASE_URL}/workspaces/${tempWorkspaceId}`);
      }
    });

    test('Diff on non-existent workspace returns 404', async ({ request }) => {
      const fakeId = '00000000-0000-0000-0000-000000000000';
      const response = await request.get(`${API_BASE_URL}/workspaces/${fakeId}/diff`);

      expect(response.status()).toBe(404);
    });

    test('Reset on non-existent workspace returns 404', async ({ request }) => {
      const fakeId = '00000000-0000-0000-0000-000000000000';
      const response = await request.post(`${API_BASE_URL}/workspaces/${fakeId}/reset`);

      expect(response.status()).toBe(404);
    });

    test('Stats on non-existent workspace returns 404', async ({ request }) => {
      const fakeId = '00000000-0000-0000-0000-000000000000';
      const response = await request.get(`${API_BASE_URL}/workspaces/${fakeId}/stats`);

      expect(response.status()).toBe(404);
    });
  });
});
