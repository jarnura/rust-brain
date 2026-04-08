import { RepoManager } from '../components/RepoManager'

export function WorkspaceList() {

  return (
    <div className="flex-1 flex items-start justify-center p-8 overflow-y-auto">
      <div className="w-full max-w-xl">
        <div className="mb-8">
          <h1 className="text-2xl font-bold text-white mb-1">
            🧠 Editor Playground
          </h1>
          <p className="text-dark-400 text-sm">
            Clone a GitHub repository and run multi-agent code transformations.
          </p>
        </div>
        <div className="bg-dark-900 border border-dark-700 rounded-lg p-5">
          <RepoManager />
        </div>
      </div>
    </div>
  )
}
