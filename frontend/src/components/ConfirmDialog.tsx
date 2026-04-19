import { useEffect, useRef, useState } from 'react'

interface ConfirmDialogProps {
  /** Dialog title, e.g. "Delete workspace". */
  title: string
  /** Optional body text shown above the confirmation input. */
  message?: string
  /**
   * When set, the confirm button stays disabled until the user types this
   * string exactly — use the workspace name for destructive actions.
   */
  confirmPhrase?: string
  /** Confirm button label. Defaults to "Delete". */
  confirmLabel?: string
  /** Cancel button label. Defaults to "Cancel". */
  cancelLabel?: string
  /** Disables both buttons while an async action is in flight. */
  busy?: boolean
  /** Error message shown below the buttons (e.g. API failure). */
  error?: string | null
  onConfirm: () => void
  onCancel: () => void
}

export function ConfirmDialog({
  title,
  message,
  confirmPhrase,
  confirmLabel = 'Delete',
  cancelLabel = 'Cancel',
  busy = false,
  error = null,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const [typed, setTyped] = useState('')
  const inputRef = useRef<HTMLInputElement | null>(null)

  const needsPhrase = typeof confirmPhrase === 'string' && confirmPhrase.length > 0
  const phraseMatches = !needsPhrase || typed === confirmPhrase
  const canConfirm = !busy && phraseMatches

  useEffect(() => {
    if (needsPhrase) {
      inputRef.current?.focus()
    }
  }, [needsPhrase])

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape' && !busy) onCancel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [busy, onCancel])

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (canConfirm) onConfirm()
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="confirm-dialog-title"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={(e) => {
        if (e.target === e.currentTarget && !busy) onCancel()
      }}
    >
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-md mx-4 bg-dark-900 border border-dark-700 rounded-lg shadow-xl"
      >
        <div className="px-5 py-4 border-b border-dark-800">
          <h2
            id="confirm-dialog-title"
            className="text-base font-semibold text-red-400"
          >
            {title}
          </h2>
        </div>

        <div className="px-5 py-4 space-y-3">
          {message && <p className="text-sm text-dark-200">{message}</p>}

          {needsPhrase && (
            <div className="space-y-1.5">
              <label className="block text-xs text-dark-400">
                Type{' '}
                <span className="font-mono text-dark-100 bg-dark-800 px-1 rounded">
                  {confirmPhrase}
                </span>{' '}
                to confirm
              </label>
              <input
                ref={inputRef}
                type="text"
                autoComplete="off"
                value={typed}
                onChange={(e) => setTyped(e.target.value)}
                disabled={busy}
                className="w-full bg-dark-800 border border-dark-600 rounded px-3 py-1.5 text-sm text-dark-100 placeholder-dark-500 focus:outline-none focus:border-red-500"
                placeholder={confirmPhrase}
              />
            </div>
          )}

          {error && <p className="text-red-400 text-xs">{error}</p>}
        </div>

        <div className="px-5 py-3 border-t border-dark-800 flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            className="px-3 py-1.5 bg-dark-800 hover:bg-dark-700 disabled:opacity-50 rounded text-sm text-dark-200 transition-colors"
          >
            {cancelLabel}
          </button>
          <button
            type="submit"
            disabled={!canConfirm}
            className="px-3 py-1.5 bg-red-600 hover:bg-red-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium text-white transition-colors"
          >
            {busy ? 'Working…' : confirmLabel}
          </button>
        </div>
      </form>
    </div>
  )
}
