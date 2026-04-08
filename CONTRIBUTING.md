# Contributing to rust-brain

Thank you for your interest in contributing to rust-brain! This document provides guidelines and instructions for contributing.

## Table of Contents

- [Development Setup](#development-setup)
- [Code Standards](#code-standards)
- [Commit Format](#commit-format)
- [Pull Request Process](#pull-request-process)
- [Testing Requirements](#testing-requirements)
- [Architecture Overview](#architecture-overview)
- [Code of Conduct](#code-of-conduct)
- [License](#license)

## Development Setup

### Prerequisites

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| Rust | 1.75+ | Latest stable |
| Docker | 24.0+ | Latest |
| Docker Compose | 2.20+ | Latest |
| RAM | 16 GB | 32 GB+ |
| Disk | 20 GB free | 50+ GB SSD |

### Initial Setup

```bash
# Clone the repository
git clone https://github.com/jarnura/rust-brain.git
cd rust-brain

# Copy environment file
cp .env.example .env

# Build all services
cargo build --workspace

# Start infrastructure services
docker compose up -d postgres neo4j qdrant

# Run health check
./scripts/healthcheck.sh
```

### Running Locally

```bash
# Run the API server
cargo run --bin rustbrain-api

# Run the ingestion pipeline
cargo run --bin rustbrain-ingestion -- --crate-path /path/to/crate

# Run tests
cargo test --workspace
```

## Code Standards

See [CLAUDE.md](./CLAUDE.md#coding-standards) for the full coding standards. Key points:

- **Error handling**: Use `anyhow` for application code, `thiserror` for library crates
- **No unsafe code**: Without an Architecture Decision Record (ADR) in `docs/`
- **Doc comments**: All public APIs must have documentation
- **Linting**: `cargo clippy --all-targets` must be clean
- **Formatting**: `cargo fmt --check` must pass

```bash
# Check formatting
cargo fmt --check

# Run linter
cargo clippy --all-targets -- -D warnings

# Run all checks
cargo check --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo test --workspace
```

## Commit Format

We use [Conventional Commits](https://www.conventionalcommits.org/). Format:

```
<type>(<scope>): <description>

[optional body]

Co-Authored-By: Paperclip <noreply@paperclip.ing>
```

### Types

| Type | Description |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `refactor` | Code change without feature/fix |
| `test` | Adding or updating tests |
| `docs` | Documentation changes |
| `chore` | Maintenance, dependencies, CI |
| `style` | Formatting, no code change |
| `perf` | Performance improvement |
| `security` | Security-related changes |

### Examples

```bash
feat(api): add workspace volume orchestration endpoints
fix(ingestion): resolve O(N²) infinite-loop in CALLS extraction
docs: update API documentation for new endpoints
test(mcp): add integration tests for all MCP tools
```

## Pull Request Process

### Before Submitting

1. **Create a branch** from `main`:
   ```bash
   git checkout -b feat/my-feature
   ```

2. **Run all checks locally**:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test --workspace
   ```

3. **Update documentation** if your changes affect:
   - API endpoints → update `docs/api-spec.md`
   - Configuration → update `.env.example` and relevant docs
   - Architecture → update `docs/architecture.md`

### PR Description Template

```markdown
## Summary
Brief description of changes.

## Changes
- Bullet list of specific changes

## Testing
- How you tested these changes

## Related Issues
Fixes #123
```

### Branch Naming

| Pattern | Example |
|---------|---------|
| `feat/<description>` | `feat/workspace-volumes` |
| `fix/<description>` | `fix/call-graph-bug` |
| `refactor/<description>` | `refactor/embedding-service` |
| `docs/<description>` | `docs/api-update` |

### Review Process

1. All PRs require at least one approval
2. CI must pass (clippy, tests, formatting)
3. Address all review comments
4. Squash and merge on approval

## Testing Requirements

### TDD Workflow

We follow test-driven development:

1. **Write failing test first** — Define expected behavior
2. **Implement** — Make the test pass
3. **Verify** — Run the full test suite

### Coverage Target

- **Minimum**: 80% coverage on critical paths
- Focus on: ingestion pipeline, API handlers, MCP tools

### Running Tests

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p rustbrain-api

# Integration tests only
cargo test --workspace --test '*'

# With coverage (requires cargo-llvm-cov)
cargo llvm-cov --workspace
```

### Test Categories

See [docs/TESTING_GUIDE.md](./docs/TESTING_GUIDE.md) for the three-layer verification approach:

1. **Unit tests**: Fast, isolated, no external dependencies
2. **Integration tests**: Service-level, Docker-based
3. **End-to-end tests**: Full stack, snapshot-based

## Architecture Overview

rust-brain uses a **triple-storage design** where each database is optimized for its access pattern:

| Database | Role | Query Type |
|----------|------|------------|
| Postgres 16 | Relational store | SQL (sqlx 0.8) |
| Neo4j 5 | Code graph | Cypher (neo4rs 0.7) |
| Qdrant 1.12 | Vector store | REST API |

### Ingestion Pipeline

Six stages run containerized:

1. **Expand** — `cargo expand` to resolve macros
2. **Parse** — DualParser: syn + tree-sitter
3. **Typecheck** — rust-analyzer subprocess
4. **Extract** — Combine results → Postgres
5. **Graph** — Neo4j nodes and edges
6. **Embed** — Ollama embeddings → Qdrant

See [docs/architecture.md](./docs/architecture.md) for full system design.

## Code of Conduct

### Our Pledge

We as members, contributors, and leaders pledge to make participation in our community a harassment-free experience for everyone, regardless of age, body size, visible or invisible disability, ethnicity, sex characteristics, gender identity and expression, level of experience, education, socio-economic status, nationality, personal appearance, race, religion, or sexual identity and orientation.

### Our Standards

Examples of behavior that contributes to a positive environment:

- Using welcoming and inclusive language
- Being respectful of differing viewpoints and experiences
- Gracefully accepting constructive criticism
- Focusing on what is best for the community
- Showing empathy towards other community members

Examples of unacceptable behavior:

- The use of sexualized language or imagery
- Trolling, insulting/derogatory comments, and personal attacks
- Public or private harassment
- Publishing others' private information without explicit permission
- Other conduct which could reasonably be considered inappropriate

### Enforcement

Instances of abusive, harassing, or otherwise unacceptable behavior may be reported to the project maintainers. All complaints will be reviewed and investigated and will result in a response that is deemed necessary and appropriate.

## License

By contributing to rust-brain, you agree that your contributions will be licensed under the [Apache License 2.0](./LICENSE).

```
Copyright 2025 rust-brain Contributors

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```
