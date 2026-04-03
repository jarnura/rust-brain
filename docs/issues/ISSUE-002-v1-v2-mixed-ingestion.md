# ISSUE-002: Ingestion Pipeline Mixes v1 and v2 Feature Code via Fallback Logic

| Field | Value |
|-------|-------|
| **ID** | ISSUE-002 |
| **Status** | Open |
| **Severity** | Medium |
| **Priority** | P2 |
| **Created** | 2026-03-30 |
| **Revised** | 2026-03-31 (validated against source) |
| **Component** | Ingestion Pipeline — ExpandStage |
| **Affects** | Knowledge Base Purity, Semantic Search Quality |
| **Scope** | Hyperswitch-specific (v1/v2 feature flags) |

---

## Summary

When ingesting the Hyperswitch codebase with v1 features, some crates fail to expand with v1 due to feature conflicts. The pipeline's fallback logic (lines 614-627 of `stages.rs`) retries with v2 features, causing v2-only types to appear in a knowledge base that was intended to be v1-only.

This is **not a bug** — the fallback is a deliberate resilience mechanism. The issue is that there is no way for the user to control this behavior or be informed when it occurs.

---

## Mechanism

### 1. Per-Crate Expansion (correct by design)

The pipeline runs `cargo expand --lib -p <crate>` for each crate individually (line 854 of `stages.rs`). This is the only way `cargo expand` works — it does **not** support a `--workspace` flag.

### 2. Per-Crate Feature Detection (lines 642-658)

```rust
fn crate_has_feature(&self, crate_path: &Path, feature: &str) -> bool {
    let cargo_toml_path = crate_path.join("Cargo.toml");
    // Reads each crate's Cargo.toml independently
    // Returns true if the [features] table contains the requested feature
}
```

Features are detected per-crate. If crate A has `v1` and crate B has both `v1` and `v2`, they are handled independently.

### 3. Feature Priority and Fallback (lines 550-627)

The expansion logic tries features in this order:
1. `v1 + olap + frm` (most complete v1 profile)
2. `v1 + domain_features` (intermediate)
3. `v1` alone (minimal)
4. **`v2` (fallback on v1 conflict)** ← this is the issue

The v2 fallback triggers only when:
- v1 expansion fails with a **feature conflict error** (duplicate definitions, redefinitions)
- The crate also defines a `v2` feature
- The v2 expansion succeeds

```rust
// lines 614-627
Err(e) => {
    let err_str = e.to_string();
    if Self::is_feature_conflict_error(&err_str)
        && features.contains(&"v1".to_string())
        && has_v2
    {
        debug!("v1 has feature conflicts for {}, trying v2", crate_name);
        if let Ok(output) = self.run_cargo_expand(
            workspace_path, crate_name, &["--features", "v2"]
        ) {
            // v2 expansion cached and returned
            return Ok(output);
        }
    }
    last_error = Some(e);
}
```

### 4. Feature Conflict Detection (lines 699-706)

```rust
fn is_feature_conflict_error(error: &str) -> bool {
    error.contains("is defined multiple times") ||
    error.contains("redefined here") ||
    error.contains("duplicate") ||
    error.contains("must be defined only once")
}
```

Pattern-based string matching on compiler errors.

---

## Observed Result

| Crate | Expanded With | Result |
|-------|--------------|--------|
| `api_models` | v1 + olap + frm | v1 types ✅ |
| `router` | v1 → conflict → **v2 fallback** | v2 types in KB ⚠️ |
| `common_enums` | No feature gate | Both v1 and v2 types appear |

v2-only types observed in the knowledge base:

| Type | Present? | Expected (v1-only)? |
|------|----------|---------------------|
| `PaymentsCreateIntentRequest` | ✅ Yes | ❌ No |
| `PaymentsIntentResponse` | ✅ Yes | ❌ No |
| `payments_v2` module | ✅ Yes | ❌ No |

---

## Why This Is NOT a Simple Bug

The v1→v2 fallback exists for a good reason: **without it, conflicting crates produce zero indexed items**. The tradeoff is:

| Strategy | Conflicting Crate Result | Knowledge Base |
|----------|-------------------------|----------------|
| Current (fallback) | v2 items indexed | Has extra v2 types |
| No fallback | Zero items indexed | Missing entire crate |
| Fail entire ingestion | Pipeline aborts | No KB at all |

The current behavior (partial v2 data) is arguably better than the alternatives for exploratory use.

---

## Additional Context

### Hyperswitch-Specific Pre-Patching

The pipeline also contains ~200 lines of Hyperswitch-specific `Cargo.toml` patching logic (lines 1017-1301) that adds `olap`/`frm` features to transitive dependencies like `hyperswitch_domain_models`, `storage_impl`, and `hyperswitch_interfaces`. This patching is hardcoded and not configurable.

### `cargo expand --workspace` Is Not an Option

The original issue proposed `cargo expand --workspace --features v1`. This is **not viable** — `cargo expand` only supports per-crate expansion via `-p <crate>`. There is no `--workspace` flag.

---

## Proposed Solutions

### Option A: Make Feature Strategy Configurable (Recommended)

Add a `FEATURE_STRATEGY` env var / CLI flag (~30 lines of changes in `stages.rs`):

```bash
# .env or CLI
FEATURE_STRATEGY=v1_only     # Never fall back to v2; skip conflicting crates
FEATURE_STRATEGY=v1_prefer   # Fall back to v2 on conflict (current default)
FEATURE_STRATEGY=v2_only     # Use v2 exclusively
```

**Effort:** Low
**Risk:** Low — additive change, current behavior preserved as default

### Option B: Add Post-Ingestion Validation

After ingestion, run a validation query that checks for unexpected feature-gated types and warns the user. No pipeline changes needed — can be a standalone script.

```bash
# Example: check for v2 types in a v1-intended KB
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "SELECT fqn FROM extracted_items WHERE fqn LIKE '%_v2%' OR fqn LIKE '%Intent%' LIMIT 20;"
```

**Effort:** Low
**Risk:** None — purely informational

### Option C: Extract Hyperswitch-Specific Patching (Longer-term)

Move the ~200 lines of hardcoded Hyperswitch pre-patching into a configurable JSON/TOML file so other projects don't carry dead code.

**Effort:** Medium
**Risk:** Low — refactor only

### What NOT to Do

- **Do not remove the fallback entirely** — turns partial indexing into zero indexing for conflicting crates
- **Do not attempt `--workspace` expansion** — the flag does not exist in `cargo expand`
- **Do not treat as P1/blocking** — the KB works, it just has some extra v2 items

---

## Recommended Approach

**Phase 1 (immediate):** Option A — add `FEATURE_STRATEGY` config. ~30 lines in `stages.rs`.
**Phase 2 (quick follow-up):** Option B — add a validation script to `scripts/`.
**Phase 3 (when time allows):** Option C — extract Hyperswitch patching to config file.

---

## Related Files

| File | Lines | Description |
|------|-------|-------------|
| `services/ingestion/src/pipeline/stages.rs` | 545-548 | Feature detection (`has_v1`, `has_v2`) |
| `services/ingestion/src/pipeline/stages.rs` | 550-627 | Feature priority and fallback logic |
| `services/ingestion/src/pipeline/stages.rs` | 642-658 | `crate_has_feature()` per-crate check |
| `services/ingestion/src/pipeline/stages.rs` | 699-706 | `is_feature_conflict_error()` detection |
| `services/ingestion/src/pipeline/stages.rs` | 854 | `cargo expand --lib -p <crate>` invocation |
| `services/ingestion/src/pipeline/stages.rs` | 1017-1301 | Hyperswitch-specific Cargo.toml patching |

---

## History

| Date | Event |
|------|-------|
| 2026-03-30 | Original issue drafted |
| 2026-03-31 | Validated against source code; severity downgraded P1→P2; corrected proposed solutions (removed invalid `--workspace` option); added root cause analysis |
