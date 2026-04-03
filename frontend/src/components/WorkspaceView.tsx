import { useEffect, useState } from 'react'
import { listFiles } from '../api/client'
import { useWorkspaceStore } from '../store/workspace'
import type { FileNode } from '../types'

// Simple recursive file tree — avoids the react-treeview peer-dep maze
// while still rendering a collapsible tree structure.

interface TreeNodeProps {
  node: FileNode
  depth: number
  onSelectFile: (path: string) => void
  selectedFile: string | null
}

function TreeNode({ node, depth, onSelectFile, selectedFile }: TreeNodeProps) {
  const [open, setOpen] = useState(depth < 2)

  const indent = depth * 12

  if (node.is_dir) {
    return (
      <div>
        <button
          onClick={() => setOpen((o) => !o)}
          className="flex items-center gap-1 w-full text-left py-0.5 px-1 rounded hover:bg-dark-700 transition-colors text-dark-300 hover:text-dark-100 text-xs"
          style={{ paddingLeft: `${indent + 4}px` }}
        >
          <span className="text-dark-500 w-3 shrink-0">{open ? '▾' : '▸'}</span>
          <span className="font-medium">{node.name}/</span>
        </button>
        {open && (
          <div>
            {node.children.map((child) => (
              <TreeNode
                key={child.path}
                node={child}
                depth={depth + 1}
                onSelectFile={onSelectFile}
                selectedFile={selectedFile}
              />
            ))}
          </div>
        )}
      </div>
    )
  }

  const isSelected = selectedFile === node.path

  return (
    <button
      onClick={() => onSelectFile(node.path)}
      className={`flex items-center gap-1 w-full text-left py-0.5 px-1 rounded transition-colors text-xs ${
        isSelected
          ? 'bg-brand-500 text-white'
          : 'text-dark-400 hover:text-dark-100 hover:bg-dark-700'
      }`}
      style={{ paddingLeft: `${indent + 16}px` }}
    >
      <span className="truncate">{node.name}</span>
    </button>
  )
}

export function WorkspaceView() {
  const { activeWorkspaceId, files, setFiles, selectedFile, setSelectedFile } =
    useWorkspaceStore()

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!activeWorkspaceId) return
    setLoading(true)
    setError(null)
    listFiles(activeWorkspaceId)
      .then(setFiles)
      .catch((err) => setError(err instanceof Error ? err.message : 'Failed to load files'))
      .finally(() => setLoading(false))
  }, [activeWorkspaceId, setFiles])

  if (!activeWorkspaceId) {
    return (
      <p className="text-dark-500 text-xs italic p-2">
        Select a workspace to browse files.
      </p>
    )
  }

  if (loading) {
    return <p className="text-dark-400 text-xs p-2 animate-pulse">Loading file tree…</p>
  }

  if (error) {
    return <p className="text-red-400 text-xs p-2">{error}</p>
  }

  if (files.length === 0) {
    return <p className="text-dark-500 text-xs italic p-2">No files found.</p>
  }

  return (
    <div className="overflow-y-auto max-h-full font-mono">
      {files.map((node) => (
        <TreeNode
          key={node.path}
          node={node}
          depth={0}
          onSelectFile={setSelectedFile}
          selectedFile={selectedFile}
        />
      ))}
    </div>
  )
}
