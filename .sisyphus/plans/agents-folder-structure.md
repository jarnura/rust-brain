# Agent Configuration Folder Structure

## TL;DR

> **Quick Summary**: Organize agent configuration files under `agents/` with separate subfolders for OpenClaw and OpenCode, using standard file conventions for each system.
>
> **Deliverables**:
> - `agents/openclaw/` with 6 config files (already moved)
> - `agents/opencode/` with OpenCode-compatible config files
> - Single `AGENTS.md` at project root serving both systems
> - Symlinks for OpenCode to read from standard locations
>
> **Estimated Effort**: Quick
> **Parallel Execution**: YES - 2 waves
> **Critical Path**: Task 1 → Task 3 → Task 4

---

## Context

### Original Request
Create organized agent configuration files under `agents/` folder with separate subfolders for OpenClaw and OpenCode agent systems.

### Interview Summary
**Key Discussions**:
- Structure: Separate folders per system (`agents/openclaw/`, `agents/opencode/`)
- Purpose: Functional config that OpenCode reads
- Location: Files in `agents/opencode/` but follow OpenCode standards
- AGENTS.md: Single file at project root serving both systems

**Research Findings**:
- OpenClaw files already moved to `agents/openclaw/`
- OpenCode uses different config model: `AGENTS.md` + `.opencode/` folder
- OpenCode expects files at project root, not in `agents/` subfolder

### Metis Review
**Identified Gaps** (addressed):
- OpenCode doesn't use SOUL.md, IDENTITY.md, USER.md, TOOLS.md, HEARTBEAT.md
- Need symlinks or config to make OpenCode read from `agents/opencode/`
- AGENTS.md at root needs to serve both systems

---

## Work Objectives

### Core Objective
Create organized agent configuration structure that works for both OpenClaw and OpenCode.

### Concrete Deliverables
- `agents/openclaw/` — OpenClaw config files (done)
- `agents/opencode/` — OpenCode config files
- `AGENTS.md` at project root — shared workspace rules
- `.opencode/` symlinks pointing to `agents/opencode/`

### Definition of Done
- [ ] All OpenCode config files created in `agents/opencode/`
- [ ] Symlinks created in `.opencode/` for OpenCode to read config
- [ ] `AGENTS.md` at root with combined instructions
- [ ] Files committed to `feature/chat-agent` branch

### Must Have
- OpenCode-readable configuration
- Clear separation between OpenClaw and OpenCode specific settings
- Shared AGENTS.md for workspace rules

### Must NOT Have (Guardrails)
- Duplicate configuration files
- Files that serve no purpose
- Configuration that conflicts between systems

---

## Verification Strategy

### Test Decision
- **Infrastructure exists**: NO (no automated tests for config files)
- **Automated tests**: None
- **Agent-Executed QA**: YES - verify files exist and symlinks work

### QA Policy
Verify file structure and symlink targets are correct.

---

## Execution Strategy

### Parallel Execution Waves

```
Wave 1 (Start Immediately — OpenCode config files):
├── Task 1: Create AGENTS.md at project root [quick]
├── Task 2: Create agents/opencode/AGENTS.md [quick]
└── Task 3: Create .opencode/ folder with symlinks [quick]

Wave 2 (After Wave 1 — commit changes):
├── Task 4: Commit all agent config changes [quick]
```

### Dependency Matrix

- **1-3**: — — 4
- **4**: 1, 2, 3 — —

### Agent Dispatch Summary

- **1**: **3** — T1-T3 → `quick`
- **2**: **1** — T4 → `quick`

---

## TODOs

- [x] 1. Create AGENTS.md at project root

  **What to do**:
  - Create `AGENTS.md` at project root with combined workspace rules
  - Include RustBrain MCP tool priority for both systems
  - Reference both `agents/openclaw/` and `agents/opencode/` for system-specific config

  **Must NOT do**:
  - Duplicate content from agents/openclaw/AGENTS.md
  - Create conflicting instructions

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: Simple file creation task
  - **Skills**: []

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 1 (with Tasks 2, 3)
  - **Blocks**: Task 4
  - **Blocked By**: None

  **References**:
  - `agents/openclaw/AGENTS.md` - Existing workspace rules to incorporate
  - `README.md` - Project context for combined instructions

  **Acceptance Criteria**:
  - [ ] File exists at `/home/jarnura/projects/rust-brain/AGENTS.md`
  - [ ] Contains RustBrain MCP tool priority section
  - [ ] References both agent system folders

  **QA Scenarios**:
  ```
  Scenario: Verify AGENTS.md exists at root
    Tool: Bash
    Steps:
      1. test -f AGENTS.md && echo "EXISTS" || echo "MISSING"
    Expected Result: "EXISTS"
    Evidence: .sisyphus/evidence/task-1-agents-exists.txt
  ```

  **Commit**: NO (groups with Task 4)

---

- [x] 2. Create agents/opencode/AGENTS.md

  **What to do**:
  - Create OpenCode-specific AGENTS.md in `agents/opencode/`
  - Include OpenCode tool priority (rustbrain MCP tools)
  - Reference OpenCode skills: /playwright, /git-master, /refactor

  **Must NOT do**:
  - Copy OpenClaw-specific content verbatim
  - Create files OpenCode doesn't use (SOUL.md, IDENTITY.md, etc.)

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: Simple file creation task
  - **Skills**: []

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 1 (with Tasks 1, 3)
  - **Blocks**: Task 4
  - **Blocked By**: None

  **References**:
  - `agents/openclaw/SOUL.md` - Tool priority pattern to follow
  - `agents/openclaw/IDENTITY.md` - Identity structure to adapt

  **Acceptance Criteria**:
  - [ ] File exists at `agents/opencode/AGENTS.md`
  - [ ] Contains rustbrain MCP tool priority
  - [ ] Lists available OpenCode skills

  **QA Scenarios**:
  ```
  Scenario: Verify OpenCode AGENTS.md exists
    Tool: Bash
    Steps:
      1. test -f agents/opencode/AGENTS.md && echo "EXISTS" || echo "MISSING"
    Expected Result: "EXISTS"
    Evidence: .sisyphus/evidence/task-2-opencode-agents.txt
  ```

  **Commit**: NO (groups with Task 4)

---

- [x] 3. Create .opencode/ folder with symlinks

  **What to do**:
  - Create `.opencode/` folder at project root
  - Create symlink: `.opencode/AGENTS.md` → `../agents/opencode/AGENTS.md`
  - Create `.opencode/agents/` and `.opencode/skills/` directories (empty, for future use)

  **Must NOT do**:
  - Create broken symlinks
  - Create files OpenCode doesn't expect

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: Simple file/folder creation
  - **Skills**: []

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 1 (with Tasks 1, 2)
  - **Blocks**: Task 4
  - **Blocked By**: None

  **References**:
  - OpenCode documentation for expected folder structure

  **Acceptance Criteria**:
  - [ ] `.opencode/` directory exists
  - [ ] `.opencode/AGENTS.md` symlink points to `../agents/opencode/AGENTS.md`
  - [ ] Symlink target is readable

  **QA Scenarios**:
  ```
  Scenario: Verify symlinks work
    Tool: Bash
    Steps:
      1. test -L .opencode/AGENTS.md && echo "SYMLINK_OK" || echo "NOT_SYMLINK"
      2. cat .opencode/AGENTS.md | head -5
    Expected Result: "SYMLINK_OK" and file content readable
    Evidence: .sisyphus/evidence/task-3-symlink.txt
  ```

  **Commit**: NO (groups with Task 4)

---

- [ ] 4. Commit all agent config changes

  **What to do**:
  - Stage all new files: AGENTS.md, agents/opencode/, .opencode/
  - Commit with descriptive message
  - Files are on `feature/chat-agent` branch

  **Must NOT do**:
  - Commit unrelated files
  - Push without user request

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: Simple git commit
  - **Skills**: []

  **Parallelization**:
  - **Can Run In Parallel**: NO
  - **Parallel Group**: Sequential
  - **Blocks**: None
  - **Blocked By**: Tasks 1, 2, 3

  **References**:
  - Git commit conventions from recent history

  **Acceptance Criteria**:
  - [ ] All files staged and committed
  - [ ] Commit message describes agent config organization

  **QA Scenarios**:
  ```
  Scenario: Verify commit
    Tool: Bash
    Steps:
      1. git log --oneline -1
      2. git status
    Expected Result: Latest commit shows agent config changes, no uncommitted files
    Evidence: .sisyphus/evidence/task-4-commit.txt
  ```

  **Commit**: YES
  - Message: `chore: Organize agent config files under agents/ folder`
  - Files: `AGENTS.md`, `agents/opencode/`, `.opencode/`
  - Pre-commit: None

---

## Final Verification Wave

- [ ] F1. **Plan Compliance Audit** — `oracle`
  Verify all files created, symlinks work, AGENTS.md serves both systems.

- [ ] F2. **File Structure Review** — `unspecified-high`
  Check all files have purpose, no duplicates, no conflicts.

---

## Commit Strategy

- **1**: `chore: Organize agent config files under agents/ folder` — AGENTS.md, agents/opencode/, .opencode/

---

## Success Criteria

### Verification Commands
```bash
ls -la agents/openclaw/   # 6 files
ls -la agents/opencode/   # AGENTS.md
ls -la .opencode/         # AGENTS.md symlink, agents/, skills/
cat AGENTS.md             # Combined workspace rules
```

### Final Checklist
- [ ] agents/openclaw/ has 6 config files
- [ ] agents/opencode/ has AGENTS.md
- [ ] .opencode/ symlink works
- [ ] Root AGENTS.md exists
- [ ] All committed to feature/chat-agent
