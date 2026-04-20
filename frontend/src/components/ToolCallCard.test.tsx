import { describe, expect, it } from 'vitest'
import type { ToolCallContent } from '../types/events'
import {
  INLINE_MAX_BYTES,
  deriveToolCallStatus,
  formatValue,
  isErrorResult,
  summarizeArgs,
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
