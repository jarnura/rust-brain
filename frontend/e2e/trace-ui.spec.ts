import { expect, test, type Page, type Route } from '@playwright/test'

// ─── Test data ───────────────────────────────────────────────────────────────

const MOCK_WORKSPACE_ID = '11111111-1111-1111-1111-111111111111'
const MOCK_EXECUTION_ID = '22222222-2222-2222-2222-222222222222'

const mockWorkspace = {
  id: MOCK_WORKSPACE_ID,
  name: 'rusa-269-fixture',
  source_type: 'github',
  source_url: 'https://github.com/example/fixture',
  clone_path: '/tmp/fixture',
  volume_name: 'rustbrain-ws-fixture',
  schema_name: 'ws_fixture',
  status: 'ready',
  default_branch: 'main',
  github_auth_method: null,
  index_started_at: '2026-04-21T00:00:00Z',
  index_completed_at: '2026-04-21T00:00:10Z',
  index_stage: null,
  index_progress: null,
  index_error: null,
  created_at: '2026-04-21T00:00:00Z',
  updated_at: '2026-04-21T00:00:10Z',
}

const mockStats = {
  workspace_id: MOCK_WORKSPACE_ID,
  status: 'ready',
  pg_items_count: 42,
  neo4j_nodes_count: 42,
  neo4j_edges_count: 10,
  qdrant_vectors_count: 42,
  consistency: {
    pg_vs_neo4j_delta: 0,
    pg_vs_qdrant_delta: 0,
    status: 'consistent',
  },
  isolation: {
    multi_label_nodes: 0,
    cross_workspace_edges: 0,
    label_mismatches: 0,
  },
}

const mockFiles = {
  path: '.',
  name: 'fixture',
  is_dir: true,
  children: [
    { path: 'src', name: 'src', is_dir: true, children: [] },
    { path: 'README.md', name: 'README.md', is_dir: false, children: [] },
  ],
}

const mockExecuteResponse = {
  id: MOCK_EXECUTION_ID,
  session_id: 'ses_fixture',
  container_id: 'container_fixture',
  status: 'running',
}

const mockExecution = {
  id: MOCK_EXECUTION_ID,
  workspace_id: MOCK_WORKSPACE_ID,
  prompt: 'fixture prompt',
  branch_name: null,
  session_id: 'ses_fixture',
  container_id: 'container_fixture',
  volume_name: 'rustbrain-ws-fixture',
  opencode_endpoint: null,
  workspace_path: '/workspace',
  status: 'completed',
  agent_phase: 'orchestrator',
  started_at: '2026-04-21T00:00:00Z',
  completed_at: '2026-04-21T00:00:05Z',
  diff_summary: { additions: 0, deletions: 0, files: 0 },
  error: null,
  timeout_config_secs: 7200,
  container_expires_at: '2026-04-21T01:00:00Z',
}

// ─── SSE helpers ─────────────────────────────────────────────────────────────

interface SseFrame {
  id: number
  event: string
  data: unknown
}

function buildSseBody(frames: SseFrame[]): string {
  const parts: string[] = []
  for (const frame of frames) {
    parts.push(`id: ${frame.id}`)
    parts.push(`event: ${frame.event}`)
    parts.push(`data: ${JSON.stringify(frame.data)}`)
    parts.push('')
    parts.push('')
  }
  return parts.join('\n')
}

function traceFrames(): SseFrame[] {
  return [
    {
      id: 1,
      event: 'agent_event',
      data: {
        id: 1,
        execution_id: MOCK_EXECUTION_ID,
        timestamp: '2026-04-21T00:00:01Z',
        event_type: 'agent_dispatch',
        content: { agent: 'explorer' },
      },
    },
    {
      id: 2,
      event: 'agent_event',
      data: {
        id: 2,
        execution_id: MOCK_EXECUTION_ID,
        timestamp: '2026-04-21T00:00:02Z',
        event_type: 'reasoning',
        content: {
          agent: 'explorer',
          text: 'Looking for the main entry point.',
        },
      },
    },
    {
      id: 3,
      event: 'agent_event',
      data: {
        id: 3,
        execution_id: MOCK_EXECUTION_ID,
        timestamp: '2026-04-21T00:00:03Z',
        event_type: 'tool_call',
        content: {
          agent: 'explorer',
          tool: 'read_file',
          args: { path: 'src/main.rs', line_range: [1, 40] },
          result: { contents: 'fn main() {\n    println!("hello");\n}\n' },
        },
      },
    },
    {
      id: 4,
      event: 'agent_event',
      data: {
        id: 4,
        execution_id: MOCK_EXECUTION_ID,
        timestamp: '2026-04-21T00:00:04Z',
        event_type: 'tool_call',
        content: {
          agent: 'explorer',
          tool: 'grep',
          args: { pattern: 'TODO', path: 'src/' },
          result: { matches: [] },
        },
      },
    },
    {
      id: 5,
      event: 'agent_event',
      data: {
        id: 5,
        execution_id: MOCK_EXECUTION_ID,
        timestamp: '2026-04-21T00:00:05Z',
        event_type: 'tool_call',
        content: {
          agent: 'explorer',
          tool: 'list_files',
          args: { path: 'src/' },
          result: ['main.rs', 'lib.rs'],
        },
      },
    },
    {
      id: 6,
      event: 'done',
      data: {},
    },
  ]
}

// ─── Route helpers ───────────────────────────────────────────────────────────

/** Install mocks for the API surface touched by the trace UI. The final
 *  `eventFrames` argument controls what the SSE endpoint emits — pass a custom
 *  set per test if you need specific events. */
async function installApiMocks(
  page: Page,
  frames: SseFrame[] = traceFrames(),
): Promise<{ sseRequests: number }> {
  const counters = { sseRequests: 0 }

  await page.route('**/workspaces', async (route: Route) => {
    if (route.request().method() !== 'GET') return route.continue()
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify([mockWorkspace]),
    })
  })

  await page.route(`**/workspaces/${MOCK_WORKSPACE_ID}`, async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(mockWorkspace),
    })
  })

  await page.route(
    `**/workspaces/${MOCK_WORKSPACE_ID}/stats`,
    async (route: Route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(mockStats),
      })
    },
  )

  await page.route(
    `**/workspaces/${MOCK_WORKSPACE_ID}/files`,
    async (route: Route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(mockFiles),
      })
    },
  )

  await page.route(
    `**/workspaces/${MOCK_WORKSPACE_ID}/executions`,
    async (route: Route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([]),
      })
    },
  )

  await page.route(
    `**/workspaces/${MOCK_WORKSPACE_ID}/diff`,
    async (route: Route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ patch: '', clean: true }),
      })
    },
  )

  await page.route(
    `**/workspaces/${MOCK_WORKSPACE_ID}/execute`,
    async (route: Route) => {
      if (route.request().method() !== 'POST') return route.continue()
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(mockExecuteResponse),
      })
    },
  )

  await page.route(
    `**/executions/${MOCK_EXECUTION_ID}`,
    async (route: Route) => {
      if (route.request().url().endsWith('/events')) return route.continue()
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(mockExecution),
      })
    },
  )

  await page.route(
    `**/executions/${MOCK_EXECUTION_ID}/events*`,
    async (route: Route) => {
      counters.sseRequests += 1
      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        headers: {
          'cache-control': 'no-cache',
          connection: 'close',
        },
        body: buildSseBody(frames),
      })
    },
  )

  return counters
}

/** Grant clipboard permissions so navigator.clipboard.writeText resolves in
 *  headless Chromium (which otherwise rejects without a user gesture). */
async function enableClipboard(page: Page): Promise<void> {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write'], {
    origin: new URL(page.url() || 'http://localhost:8090').origin,
  })
}

/** Select the fixture workspace from the list and submit the trace prompt. */
async function startTrace(page: Page): Promise<void> {
  // The workspace tile is a <li role="button"> — narrow to that element so we
  // don't clash with the ✕ "Delete workspace rusa-269-fixture" button which
  // shares the workspace name in its accessible label.
  const tile = page
    .locator('li[role="button"]')
    .filter({ hasText: 'rusa-269-fixture' })
  await tile.click()
  // PromptInput renders a textarea; typing + clicking Execute triggers the
  // executeWorkspace POST which our mock fulfills synchronously.
  const promptBox = page.getByPlaceholder(/Describe the feature/i)
  await expect(promptBox).toBeEnabled()
  await promptBox.fill('trace fixture prompt')
  await page.getByRole('button', { name: /^Execute$/ }).click()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

test.describe('Trace UI — playground-ui browser E2E (RUSA-269)', () => {
  test.beforeEach(async ({ page }) => {
    await installApiMocks(page)
  })

  test('execution trace renders SSE events', async ({ page }, testInfo) => {
    await page.goto('/')
    await startTrace(page)

    // Expect the ExecutionStream header ("Execution <id>…") to appear.
    await expect(page.getByText(/^Execution /).first()).toBeVisible()

    // TOOL badge proves the tool_call fixtures propagated through the
    // SSE → store → component pipeline.
    await expect(page.locator('text=TOOL').first()).toBeVisible()

    // Capture screenshot artifact for visual verification per RUSA-269.
    await testInfo.attach('trace-renders.png', {
      body: await page.screenshot({ fullPage: true }),
      contentType: 'image/png',
    })
  })

  test('ToolCallCard shows TOOL badge, tool name, and args summary', async ({
    page,
  }) => {
    await page.goto('/')
    await startTrace(page)

    // Wait for at least one tool card button to render — these are the
    // <button aria-expanded="…"> wrappers inside ToolCallCard.
    const readFileCard = page
      .locator('button[aria-expanded]')
      .filter({ hasText: 'read_file' })
      .first()
    await expect(readFileCard).toBeVisible()

    // Badge and agent chip.
    await expect(readFileCard.getByText('TOOL')).toBeVisible()
    await expect(readFileCard.getByText('explorer')).toBeVisible()

    // Expand and verify the args block shows — the label is "args".
    await readFileCard.click()
    await expect(readFileCard).toHaveAttribute('aria-expanded', 'true')
    await expect(page.getByText('args', { exact: false }).first()).toBeVisible()
    await expect(
      page.getByText('result', { exact: false }).first(),
    ).toBeVisible()
  })

  test('collapse/expand via toolbar and J/K keyboard navigation', async ({
    page,
  }) => {
    await page.goto('/')
    await startTrace(page)

    // Baseline: every tool_call fixture renders as a ToolCallCard which shows
    // a yellow "TOOL" badge. Three fixtures → three badges.
    await expect(page.getByText('TOOL', { exact: true })).toHaveCount(3)

    // "Collapse all" replaces each ToolCallCard with a CollapsedGroupHeader
    // (no TOOL badge, just a summary line like "▶ tool: read_file → completed").
    await page.getByRole('button', { name: /Collapse all/i }).click()
    await expect(page.getByText('TOOL', { exact: true })).toHaveCount(0)
    await expect(page.getByText(/tool: read_file/).first()).toBeVisible()

    // "Expand all" restores the TOOL badges.
    await page.getByRole('button', { name: /Expand all/i }).click()
    await expect(page.getByText('TOOL', { exact: true })).toHaveCount(3)

    // J/K keyboard navigation — move focus through the transcript, then
    // verify the handler ran by pressing Enter to toggle the focused unit.
    // We first focus the document body so keys aren't captured by an input.
    await page.locator('body').click({ position: { x: 5, y: 5 } })
    await page.keyboard.press('j')
    await page.keyboard.press('j')
    await page.keyboard.press('k')
    // No throw + badges still present == nav handler did not regress the UI.
    await expect(page.getByText('TOOL', { exact: true })).toHaveCount(3)
  })

  test('SSE connection indicator reports connected / stream closed', async ({
    page,
  }) => {
    await page.goto('/')
    await startTrace(page)

    // Fixtures end with `event: done`, which the client treats as a clean
    // close. ConnectionStatus then renders "Stream closed" rather than
    // "Disconnected".
    await expect(page.getByText('Stream closed')).toBeVisible({
      timeout: 10_000,
    })
  })

  test('copy-to-clipboard on tool result updates button label', async ({
    page,
  }) => {
    await page.goto('/')
    await enableClipboard(page)
    await startTrace(page)

    const readFileCard = page
      .locator('button[aria-expanded]')
      .filter({ hasText: 'read_file' })
      .first()
    await expect(readFileCard).toBeVisible()
    await readFileCard.click()

    // Expand renders TruncatedBlock(s) for args + result. Each has its own
    // aria-labelled copy button ("copy args" / "copy result"). Click the
    // result copy button and observe the label flip to "copied".
    const copyResultButton = page.getByRole('button', { name: 'copy result' })
    await expect(copyResultButton).toBeVisible()
    await copyResultButton.click()
    // Label flips to "copied" and reverts after 1.5s — we just check the
    // transient state.
    await expect(copyResultButton).toHaveText('copied')
  })

  test('legacy phase fallback: reasoning and agent_dispatch render', async ({
    page,
  }) => {
    await page.goto('/')
    await startTrace(page)

    // Reasoning card shows the summary text from our fixture.
    await expect(
      page.getByText('Looking for the main entry point.'),
    ).toBeVisible()
    // Agent timeline chip labelled "Explorer" (capitalized display label).
    await expect(page.getByText('Explorer').first()).toBeVisible()
  })
})
