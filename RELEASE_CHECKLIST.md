# Release Checklist

This checklist ensures all rust-brain subsystems are verified before any release claim. Complete all mandatory sections and obtain sign-off.

**Release Version**: `___________`  
**Release Date**: `___________`  
**Release Branch**: `___________`  
**Last Commit**: `___________`

---

## Sign-Off Section

| Role | Name | Signature | Date |
|------|------|-----------|------|
| Release Owner | _____________ | ☐ | ____/____/________ |
| QA Verification | _____________ | ☐ | ____/____/________ |
| Technical Review | _____________ | ☐ | ____/____/________ |
| Documentation Review | _____________ | ☐ | ____/____/________ |

---

## 1. Infrastructure Health

### 1.1 All Services Running

**Prerequisite**: `docker compose up -d` completed successfully.

| Service | Container Name | Port | Healthcheck Status | Notes |
|---------|----------------|------|-------------------|-------|
| Postgres | rustbrain-postgres | 5432 | ☐ Pass / ☐ Fail | |
| Pgweb | rustbrain-pgweb | 8085 | ☐ Pass / ☐ Fail | Depends on postgres |
| Neo4j | rustbrain-neo4j | 7474, 7687 | ☐ Pass / ☐ Fail | |
| Qdrant | rustbrain-qdrant | 6333, 6334 | ☐ Pass / ☐ Fail | |
| Ollama | rustbrain-ollama | 11434 | ☐ Pass / ☐ Fail | GPU optional |
| Prometheus | rustbrain-prometheus | 9090 | ☐ Pass / ☐ Fail | |
| Grafana | rustbrain-grafana | 3000 | ☐ Pass / ☐ Fail | |
| API Server | rustbrain-api | 8088 | ☐ Pass / ☐ Fail | Core service |
| MCP (stdio) | rustbrain-mcp | N/A | ☐ Verified / ☐ N/A | Manual test |
| MCP-SSE | rustbrain-mcp-sse | 3001 | ☐ Pass / ☐ Fail | |
| OpenCode | rustbrain-opencode | 4096 | ☐ Pass / ☐ Fail | |
| Playground UI | rustbrain-playground-ui | 8090 | ☐ Pass / ☐ Fail | |
| **Total Running** | ______ / 13 | | | |

### 1.2 Resource Allocation

Verify container memory limits (from docker-compose.yml):

| Resource | Limit | Current Usage | Status |
|----------|-------|---------------|--------|
| Postgres | 6G | _____ | ☐ |
| Neo4j | 12G | _____ | ☐ |
| Qdrant | 12G | _____ | ☐ |
| Ollama | 16G | _____ | ☐ |
| API | 2G | _____ | ☐ |
| Ingestion | 32G | _____ | ☐ (only during ingest) |

---

## 2. Ingestion Pipeline Verification

### 2.1 Clean Ingestion Test

**Command**:
```bash
./scripts/ingest.sh <path-to-test-repo>
```

| Stage | Expected Output | Actual Output | Status |
|-------|-----------------|---------------|--------|
| Parse | Items extracted | _______ items | ☐ Pass / ☐ Fail |
| Typecheck | Types resolved | _______ types | ☐ Pass / ☐ Fail |
| Extract | Relationships created | _______ edges | ☐ Pass / ☐ Fail |
| Embed | Vectors generated | _______ vectors | ☐ Pass / ☐ Fail |
| Graph | Neo4j populated | _______ nodes | ☐ Pass / ☐ Fail |

### 2.2 Cross-Store Verification

Run consistency checks via API:

| Check | Query/API | Expected Result | Status |
|-------|-----------|-----------------|--------|
| Postgres has items | `SELECT COUNT(*) FROM items` | >0 | ☐ |
| Neo4j has nodes | `MATCH (n) RETURN count(n)` | >0 | ☐ |
| Qdrant has points | List collection count | >0 | ☐ |
| Embedding dims | Verify collection config | 2560 | ☐ |

### 2.3 Known Ingestion Limits

Review [KNOWN_ISSUES.md](./KNOWN_ISSUES.md) and verify documentation is accurate:

- [ ] File size limits documented (>10MB pre-expansion, >2MB post-expansion)
- [ ] Feature flag limits documented (256 max)
- [ ] ISSUE-002 (v1/v2 mixed ingestion) noted
- [ ] ISSUE-003 (trait call resolution) noted

---

## 3. API Verification

### 3.1 Health Endpoints

| Endpoint | Method | URL | Expected | Status |
|----------|--------|-----|----------|--------|
| API Health | GET | `http://localhost:8088/health` | 200 OK | ☐ |
| MCP-SSE Health | GET | `http://localhost:3001/health` | 200 OK | ☐ |
| Playground | GET | `http://localhost:8090` | 200 OK | ☐ |
| Neo4j Browser | GET | `http://localhost:7474` | 200 OK | ☐ |

### 3.2 Tool API Endpoints (10 Routes)

| Endpoint | Method | Sample Query | Response OK | Data Valid |
|----------|--------|--------------|-------------|------------|
| `search_semantic` | POST | `{ "query": "auth middleware" }` | ☐ | ☐ |
| `aggregate_search` | POST | `{ "query": "config" }` | ☐ | ☐ |
| `get_function` | GET | `?fqn=crate::module::fn_name` | ☐ | ☐ |
| `get_callers` | GET | `?fqn=crate::module::fn_name` | ☐ | ☐ |
| `get_trait_impls` | GET | `?trait_name=Serialize` | ☐ | ☐ |
| `find_usages_of_type` | GET | `?type_name=String` | ☐ | ☐ |
| `get_module_tree` | GET | `?crate=my_crate` | ☐ | ☐ |
| `query_graph` | POST | `{ "cypher": "MATCH (n) LIMIT 5" }` | ☐ | ☐ |
| `find_calls_with_type` | GET | `?type_name=Vec<u8>` | ☐ | ☐ |
| `find_trait_impls_for_type` | GET | `?type_name=MyStruct` | ☐ | ☐ |

### 3.3 Chat API Endpoints (6 Routes)

| Endpoint | Method | Status |
|----------|--------|--------|
| `POST /tools/chat` | Create chat | ☐ |
| `GET /tools/chat/stream` | SSE stream | ☐ |
| `POST /tools/chat/send` | Send message | ☐ |
| `POST /tools/chat/sessions` | Create session | ☐ |
| `GET /tools/chat/sessions` | List sessions | ☐ |
| `DELETE /tools/chat/sessions/:id` | Delete session | ☐ |

### 3.4 Workspace API Endpoints (5 Routes)

| Endpoint | Status |
|----------|--------|
| Clone workspace | ☐ |
| Diff workspace | ☐ |
| Commit workspace | ☐ |
| Reset workspace | ☐ |
| Stream workspace events | ☐ |

### 3.5 Handler Verification Summary

**Handler files checked**: 20 files  
**Total handler functions**: 72  
**Routes tested**: _____ / ~72  
**Pass rate**: _______

---

## 4. MCP Tool Verification

### 4.1 All Tools Callable

Test via MCP Inspector or direct SSE connection to `http://localhost:3001/sse`:

| Tool | Name | Expected Args | Test Result | Status |
|------|------|---------------|-------------|--------|
| 1 | `pg_query` | sql (string) | ☐ | ☐ |
| 2 | `context_store` | operation, data | ☐ | ☐ |
| 3 | `status_check` | service_name | ☐ | ☐ |
| 4 | `task_update` | task_id, status | ☐ | ☐ |
| 5 | `aggregate_search` | query, filters | ☐ | ☐ |
| 6 | `query_graph` | cypher, params | ☐ | ☐ |
| 7 | `search_code` | query, limit | ☐ | ☐ |
| 8 | `get_function` | fqn | ☐ | ☐ |
| 9 | `get_callers` | fqn, depth | ☐ | ☐ |
| 10 | `get_trait_impls` | trait_name | ☐ | ☐ |
| 11 | `find_type_usages` | type_name | ☐ | ☐ |
| 12 | `get_module_tree` | crate_name | ☐ | ☐ |
| 13 | `consistency_check` | store (optional) | ☐ | ☐ |
| 14 | *(typecheck_tools)* | *various* | ☐ | ☐ |

### 4.2 Tool Return Data Validation

| Tool | Schema Valid | Data Accurate | Edge Cases Handled |
|------|--------------|---------------|-------------------|
| search_code | ☐ | ☐ | ☐ |
| get_function | ☐ | ☐ | ☐ |
| get_callers | ☐ | ☐ | ☐ |
| aggregate_search | ☐ | ☐ | ☐ |

---

## 5. E2E Test Suite

### 5.1 Integration Tests

| Test Suite | Location | Tests | Passed | Failed | Ignored |
|------------|----------|-------|--------|--------|---------|
| API Integration | `services/api/tests/` | ~40+ | ___ | ___ | ___ |
| MCP Integration | `services/mcp/tests/` | ~21 | ___ | ___ | ___ |
| Consistency | `services/api/tests/consistency_*` | ~28 | ___ | ___ | ___ |

**Known Ignored Tests**: Document any test skips in release notes.

### 5.2 E2E Verification Commands

```bash
# Run integration tests
cargo test --package rustbrain-api --test api_integration
cargo test --package rustbrain-mcp --test mcp_integration
cargo test --package rustbrain-api --test consistency_integration

# Verify consistency across stores
./scripts/verify-consistency.sh
```

| Check | Command | Result |
|-------|---------|--------|
| Postgres-Neo4j sync | `verify-consistency.sh` | ☐ Pass / ☐ Fail |
| Postgres-Qdrant sync | `verify-consistency.sh` | ☐ Pass / ☐ Fail |
| No orphaned items | Manual check | ☐ Pass / ☐ Fail |

---

## 6. Known Issues Review

### 6.1 Known Issues Document

- [ ] [KNOWN_ISSUES.md](./KNOWN_ISSUES.md) exists in repo root
- [ ] All open issues are documented
- [ ] Resolved issues are marked accordingly
- [ ] Version-specific issues noted for this release

### 6.2 Issue Status Matrix

| Issue | Status | Release Blocker? | Mitigation |
|-------|--------|------------------|------------|
| ISSUE-001 | Resolved | No | N/A |
| ISSUE-002 | Open | ☐ Yes / ☐ No | v1 ingestion disabled |
| ISSUE-003 | Open | ☐ Yes / ☐ No | Partial resolution |
| GAP-001 | Open | ☐ Yes / ☐ No | Placeholder nodes documented |
| ... | | | |

---

## 7. Cross-Store Consistency

### 7.1 Automated Consistency Checks

Run via API or MCP `consistency_check` tool:

| Check | Description | Result |
|-------|-------------|--------|
| Item Counts | Postgres items ≈ Neo4j nodes | ☐ |
| Embedding Coverage | Items with embeddings = Qdrant points | ☐ |
| Relationship Integrity | All relationships point to valid nodes | ☐ |
| Orphan Detection | No unreferenced items in any store | ☐ |

### 7.2 Manual Spot Checks

| Sample Item | In Postgres | In Neo4j | In Qdrant | Consistent? |
|-------------|-------------|----------|-----------|-------------|
| Example 1 | ☐ | ☐ | ☐ | ☐ |
| Example 2 | ☐ | ☐ | ☐ | ☐ |
| Example 3 | ☐ | ☐ | ☐ | ☐ |

---

## 8. CI/CD Verification

### 8.1 GitHub Actions

| Workflow | Status | URL |
|----------|--------|-----|
| CI Pipeline | ☐ Pass / ☐ Fail | |
| Docker Build | ☐ Pass / ☐ Fail | |
| Integration Tests | ☐ Pass / ☐ Fail | |

### 8.2 Build Verification

```bash
# Verify clean builds
docker compose build --no-cache api
docker compose build --no-cache ingestion
docker compose build --no-cache mcp
```

| Component | Build Time | Image Size | Status |
|-----------|------------|------------|--------|
| API | _____ min | _____ MB | ☐ |
| Ingestion | _____ min | _____ MB | ☐ |
| MCP | _____ min | _____ MB | ☐ |

---

## 9. Documentation Verification

### 9.1 User-Facing Docs

| Document | Updated | Accurate | Links Work |
|----------|---------|----------|------------|
| README.md | ☐ | ☐ | ☐ |
| CHANGELOG.md | ☐ | ☐ | ☐ |
| KNOWN_ISSUES.md | ☐ | ☐ | ☐ |
| INGESTION_GUIDE.md | ☐ | ☐ | ☐ |
| getting-started.md | ☐ | ☐ | ☐ |

### 9.2 API Documentation

- [ ] All endpoints documented
- [ ] Example requests/responses valid
- [ ] Error codes documented

---

## 10. Snapshot Testing (If Applicable)

### 10.1 Pre-Built Snapshot

| Test | Command | Result |
|------|---------|--------|
| Download snapshot | `./scripts/run-with-snapshot.sh` | ☐ |
| Databases restored | Check postgres/neo4j/qdrant | ☐ |
| API serves data | Query endpoints | ☐ |
| MCP tools work | Test via inspector | ☐ |

### 10.2 Snapshot Creation

| Step | Status |
|------|--------|
| Create snapshot | `./scripts/create-snapshot.sh <project> <commit>` | ☐ |
| Snapshot size < 2GB | _____ MB | ☐ |
| Split archives created | ☐ Yes / ☐ N/A | ☐ |

---

## 11. Security Checklist

| Check | Status | Notes |
|-------|--------|-------|
| No hardcoded secrets | ☐ | Review .env.example |
| No debug logging in prod | ☐ | Check RUST_LOG settings |
| Docker images scanned | ☐ | CVE check |
| Cypher injection tests pass | ☐ | See api_integration tests |
| Rate limiting enabled | ☐ | Via nginx or API |

---

## 12. Release Notes Template

```markdown
## [X.Y.Z] - YYYY-MM-DD

### Added
- Feature 1
- Feature 2

### Changed
- Change 1

### Fixed
- Fix 1
- Fix 2

### Known Issues
- [Link to KNOWN_ISSUES.md section]

### Breaking Changes
- None / List here

### Dependencies
- Service versions
- Model versions
```

---

## Release Decision

| Criterion | Status | Notes |
|-----------|--------|-------|
| All services healthy | ☐ Pass / ☐ Fail | |
| Ingestion pipeline works | ☐ Pass / ☐ Fail | |
| API responds correctly | ☐ Pass / ☐ Fail | |
| MCP tools callable | ☐ Pass / ☐ Fail | |
| E2E tests pass | ☐ Pass / ☐ Fail | |
| Known issues documented | ☐ Pass / ☐ Fail | |
| Cross-store consistent | ☐ Pass / ☐ Fail | |
| CI green | ☐ Pass / ☐ Fail | |

**Overall Status**: ☐ **APPROVED FOR RELEASE** / ☐ **BLOCKED**

**Blockers** (if any):

_____________________________________________________________________________

_____________________________________________________________________________

---

## Appendix: Quick Reference Commands

```bash
# Health checks
curl http://localhost:8088/health
curl http://localhost:3001/health
docker compose ps

# Ingestion
./scripts/ingest.sh /path/to/repo

# Consistency
./scripts/verify-consistency.sh

# Tests
cargo test --package rustbrain-api --test api_integration
cargo test --package rustbrain-mcp --test mcp_integration

# Logs
docker compose logs -f api
docker compose logs -f mcp-sse
```

---

*This checklist was generated as part of [RUSA-163](/RUSA/issues/RUSA-163).*
*Last updated: 2026-04-10*
