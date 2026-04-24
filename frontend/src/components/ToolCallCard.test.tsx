import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ToolCallContent } from '../types/events'
import {
  INLINE_MAX_BYTES,
  copyToClipboard,
  deriveToolCallStatus,
  formatValue,
  isErrorResult,
  semanticToolSummary,
  summarizeArgs,
  tokenizeJson,
  truncateText,
} from './ToolCallCard'

function content(partial: Partial<ToolCallContent>): ToolCallContent {
  return {
    kind: 'tool_call',
    agent: 'coder',
    tool: 'read_file',
    ...partial,
  }
}

describe('deriveToolCallStatus', () => {
  it('returns pending when result is absent', () => {
    expect(deriveToolCallStatus(content({}))).toBe('pending')
  })

  it('returns pending when result is explicitly null', () => {
    expect(deriveToolCallStatus(content({ result: null }))).toBe('pending')
  })

  it('returns completed for a plain string result', () => {
    expect(deriveToolCallStatus(content({ result: 'hello' }))).toBe('completed')
  })

  it('returns completed for a plain object result', () => {
    expect(deriveToolCallStatus(content({ result: { rows: [] } }))).toBe('completed')
  })

  it('returns error when result has is_error: true', () => {
    expect(deriveToolCallStatus(content({ result: { is_error: true, text: 'x' } }))).toBe(
      'error',
    )
  })

  it('returns error when result has an error field', () => {
    expect(deriveToolCallStatus(content({ result: { error: 'boom' } }))).toBe('error')
  })

  it('returns error when result string begins with "error"', () => {
    expect(deriveToolCallStatus(content({ result: 'Error: file not found' }))).toBe('error')
  })
})

describe('isErrorResult', () => {
  it('detects error strings case-insensitively', () => {
    expect(isErrorResult('error: x')).toBe(true)
    expect(isErrorResult('ERROR: x')).toBe(true)
    expect(isErrorResult('success')).toBe(false)
  })

  it('does not treat empty error fields as errors', () => {
    expect(isErrorResult({ error: '' })).toBe(false)
  })

  it('handles nested error objects', () => {
    expect(isErrorResult({ error: { code: 500 } })).toBe(true)
  })

  it('returns false for null, undefined, numbers, arrays', () => {
    expect(isErrorResult(null)).toBe(false)
    expect(isErrorResult(undefined)).toBe(false)
    expect(isErrorResult(42)).toBe(false)
    expect(isErrorResult([])).toBe(false)
  })
})

describe('formatValue', () => {
  it('returns strings verbatim', () => {
    expect(formatValue('hello world')).toBe('hello world')
  })

  it('pretty-prints objects with 2-space indent', () => {
    expect(formatValue({ a: 1, b: [2, 3] })).toBe('{\n  "a": 1,\n  "b": [\n    2,\n    3\n  ]\n}')
  })

  it('returns empty string for undefined', () => {
    expect(formatValue(undefined)).toBe('')
  })

  it('does not throw on circular objects', () => {
    const obj: Record<string, unknown> = {}
    obj.self = obj
    expect(() => formatValue(obj)).not.toThrow()
    expect(typeof formatValue(obj)).toBe('string')
  })
})

describe('truncateText', () => {
  it('returns full text when under the budget', () => {
    const r = truncateText('short')
    expect(r.truncated).toBe(false)
    expect(r.text).toBe('short')
    expect(r.originalLength).toBe(5)
  })

  it('truncates when over the default budget', () => {
    const big = 'x'.repeat(INLINE_MAX_BYTES + 100)
    const r = truncateText(big)
    expect(r.truncated).toBe(true)
    expect(r.text.length).toBe(INLINE_MAX_BYTES)
    expect(r.originalLength).toBe(INLINE_MAX_BYTES + 100)
  })

  it('honors a custom budget', () => {
    const r = truncateText('abcdefghij', 4)
    expect(r.truncated).toBe(true)
    expect(r.text).toBe('abcd')
    expect(r.originalLength).toBe(10)
  })
})

describe('summarizeArgs', () => {
  it('returns empty string for null/undefined', () => {
    expect(summarizeArgs(null)).toBe('')
    expect(summarizeArgs(undefined)).toBe('')
  })

  it('truncates long strings with an ellipsis', () => {
    const long = 'a'.repeat(100)
    const summary = summarizeArgs(long)
    expect(summary.length).toBeLessThanOrEqual(61)
    expect(summary.endsWith('…')).toBe(true)
  })

  it('compactly stringifies small objects', () => {
    expect(summarizeArgs({ path: '/tmp/x' })).toBe('{"path":"/tmp/x"}')
  })

  it('truncates compact JSON when too long', () => {
    const big = { data: 'x'.repeat(200) }
    const summary = summarizeArgs(big)
    expect(summary.endsWith('…')).toBe(true)
    expect(summary.length).toBeLessThanOrEqual(61)
  })
})

describe('tokenizeJson', () => {
  it('returns empty array for empty input', () => {
    expect(tokenizeJson('')).toEqual([])
  })

  it('falls back to a single text token for non-JSON input', () => {
    const tokens = tokenizeJson('not json at all')
    expect(tokens).toEqual([{ type: 'text', value: 'not json at all' }])
  })

  it('tokenizes a simple JSON object distinguishing keys from string values', () => {
    const tokens = tokenizeJson('{"name":"ada"}')
    const significant = tokens.filter((t) => t.type !== 'whitespace')
    expect(significant).toEqual([
      { type: 'punct', value: '{' },
      { type: 'key', value: '"name"' },
      { type: 'punct', value: ':' },
      { type: 'string', value: '"ada"' },
      { type: 'punct', value: '}' },
    ])
  })

  it('recognizes numbers, booleans, and null', () => {
    const tokens = tokenizeJson('{"n":42,"f":true,"g":null}')
    const types = tokens.filter((t) => t.type !== 'whitespace').map((t) => t.type)
    expect(types).toContain('number')
    expect(types).toContain('boolean')
    expect(types).toContain('null')
  })

  it('preserves the complete input when concatenating token values', () => {
    const pretty = JSON.stringify({ a: 1, b: [2, 3], c: 'x' }, null, 2)
    const tokens = tokenizeJson(pretty)
    expect(tokens.map((t) => t.value).join('')).toBe(pretty)
  })

  it('handles strings containing escaped quotes', () => {
    const pretty = JSON.stringify({ msg: 'she said "hi"' })
    const tokens = tokenizeJson(pretty)
    expect(tokens.map((t) => t.value).join('')).toBe(pretty)
    expect(tokens.some((t) => t.type === 'string' && t.value.includes('\\"hi\\"'))).toBe(true)
  })
})

describe('semanticToolSummary', () => {
  it('summarizes Read with file path and offset', () => {
    expect(semanticToolSummary('Read', { file_path: '/src/main.rs', offset: 95 }))
      .toBe('Read main.rs:95')
  })

  it('summarizes read_file with path and line', () => {
    expect(semanticToolSummary('read_file', { path: '/src/lib.rs', line: 10 }))
      .toBe('Read lib.rs:10')
  })

  it('summarizes Read without line number', () => {
    expect(semanticToolSummary('Read', { file_path: '/src/config.ts' }))
      .toBe('Read config.ts')
  })

  it('summarizes Write tool', () => {
    expect(semanticToolSummary('Write', { file_path: '/src/new.rs' }))
      .toBe('Write new.rs')
  })

  it('summarizes Edit tool', () => {
    expect(semanticToolSummary('Edit', { file_path: '/src/lib.rs' }))
      .toBe('Edit lib.rs')
  })

  it('summarizes Bash with command', () => {
    expect(semanticToolSummary('Bash', { command: 'cargo test' }))
      .toBe('$ cargo test')
  })

  it('truncates long Bash commands', () => {
    const longCmd = 'a'.repeat(60)
    const result = semanticToolSummary('Bash', { command: longCmd })
    expect(result).not.toBeNull()
    expect(result!.length).toBeLessThanOrEqual(53)
  })

  it('summarizes Grep with pattern', () => {
    expect(semanticToolSummary('Grep', { pattern: 'fn main' }))
      .toBe('Grep "fn main"')
  })

  it('summarizes Glob with pattern', () => {
    expect(semanticToolSummary('Glob', { pattern: '**/*.rs' }))
      .toBe('Glob **/*.rs')
  })

  it('summarizes Agent dispatch', () => {
    expect(semanticToolSummary('Agent', { description: 'explore code' }))
      .toBe('Agent: explore code')
  })

  it('returns null for unknown tools', () => {
    expect(semanticToolSummary('custom_tool', { data: 'x' }))
      .toBeNull()
  })

  it('returns null when args is null or undefined', () => {
    expect(semanticToolSummary('Read', null)).toBeNull()
    expect(semanticToolSummary('Read', undefined)).toBeNull()
  })

  it('returns null when required field is missing', () => {
    expect(semanticToolSummary('Read', { wrong_field: 'x' })).toBeNull()
  })
})

describe('copyToClipboard', () => {
  const originalClipboard = navigator.clipboard

  beforeEach(() => {
    vi.restoreAllMocks()
  })

  afterEach(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: originalClipboard,
    })
  })

  it('writes text via navigator.clipboard and returns true on success', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    })
    const result = await copyToClipboard('hello')
    expect(writeText).toHaveBeenCalledWith('hello')
    expect(result).toBe(true)
  })

  it('returns false when the clipboard API rejects', async () => {
    const writeText = vi.fn().mockRejectedValue(new Error('denied'))
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    })
    const result = await copyToClipboard('hello')
    expect(result).toBe(false)
  })

  it('returns false when the clipboard API is unavailable', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: undefined,
    })
    const result = await copyToClipboard('hello')
    expect(result).toBe(false)
  })
})
