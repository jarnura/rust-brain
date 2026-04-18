# ISSUE-003: Trait Impl Method Calls Not Resolved to Concrete Implementations

| Field | Value |
|-------|-------|
| **ID** | ISSUE-003 |
| **Status** | Open |
| **Severity** | Medium |
| **Priority** | P2 |
| **Created** | 2026-04-01 |
| **Component** | Ingestion Pipeline — GraphStage (CALLS resolution) |
| **Affects** | Call Graph Precision, Explorer Agent, Reviewer Impact Analysis |

---

## Summary

When a function calls a trait method (e.g., `data.get_customer_acceptance()`), the GraphStage resolves the CALLS edge to the **first matching function name** rather than the **specific trait impl method** for the receiver type. This leaves concrete trait impl methods with zero callers in Neo4j, even when they are called via static dispatch.

---

## Reproduction

```cypher
-- This trait impl method has 0 callers:
MATCH (caller)-[:CALLS]->(target)
WHERE target.fqn = 'router::core::payments::flows::setup_mandate_flow::types :: SetupMandateRequestData::get_customer_acceptance'
RETURN caller.fqn
-- Result: (empty)

-- But a standalone function with the same name HAS a caller:
MATCH (caller)-[:CALLS]->(target)
WHERE target.name = 'get_customer_acceptance'
RETURN caller.fqn, target.fqn
-- Result: save_payment_method -> router::core::mandate::get_customer_acceptance
```

The call in `save_payment_method` actually goes through a trait bound (static dispatch), so it should resolve to the impl method on the concrete type being used, not to an unrelated standalone function.

---

## Root Cause

In `services/ingestion/src/pipeline/stages.rs`, the `extract_function_calls` method in GraphStage resolves method calls by matching the method name against all known function FQNs. When multiple functions share the same short name (e.g., `get_customer_acceptance` exists as both a standalone fn and as trait impl methods on multiple types), the resolution picks the first match or the standalone function.

The GraphStage does not:
1. Track the receiver type of method calls (e.g., `self.method()` where `self: SetupMandateRequestData`)
2. Use the TypecheckStage's monomorphization data (`call_sites` table) to inform Neo4j CALLS edge creation
3. Use IMPLEMENTS relationships to narrow method call resolution to the correct impl

---

## Impact

### Affected Queries
- `get_callers` for any trait impl method returns empty or incomplete results
- Impact analysis (`what breaks if I change X`) underestimates blast radius for trait impls
- Explorer agent's call chain tracing has gaps at trait method boundaries

### Scale
```cypher
-- Count of impl method Function nodes with zero callers
MATCH (f:Function)
WHERE f.fqn CONTAINS '::' AND f.name = split(f.fqn, '::')[-1]
AND NOT EXISTS { MATCH ()-[:CALLS]->(f) }
AND f.visibility IN ['pub', 'pub_crate']
RETURN count(f)
```
Estimated: thousands of pub impl methods with zero callers (many should have callers).

### Workaround
The Explorer agent prompt includes a "grep fallback" — when Neo4j returns empty callers for a PG-confirmed pub function, it falls back to `grep`/`rg` to find call sites. This works but is slower and less structured than graph traversal.

---

## Proposed Fix

### Option A: Post-process CALLS using TypecheckStage data (Recommended)

After the GraphStage creates CALLS edges from name matching, add a refinement pass:

1. For each CALLS edge where the target is a standalone function AND a trait impl method with the same name exists:
   - Query `call_sites` table for the caller FQN
   - If `call_sites` has a monomorphized call with concrete type args pointing to the impl type, redirect the CALLS edge to the impl method
2. For each trait impl method with 0 callers:
   - Query `call_sites` for callers of the impl's trait method
   - Create CALLS edges based on the monomorphized data

### Option B: Receiver type tracking in GraphStage

Enhance `extract_function_calls` to parse method call expressions and track receiver types:
- `self.method()` → receiver is the impl's self type
- `variable.method()` → trace variable type from function signature or local bindings
- Use the type information to disambiguate which impl's method is being called

Option B is more correct but significantly more complex (requires type inference during graph construction).

### Option C: CALLS_TRAIT relationship type

Add a new relationship `CALLS_TRAIT` that links callers to trait definitions (not impls), and let consuming agents resolve to concrete impls via IMPLEMENTS:
```cypher
MATCH (caller)-[:CALLS_TRAIT]->(trait_method)
MATCH (impl)-[:IMPLEMENTS]->(trait)<-[:HAS_METHOD]-(trait_method)
WHERE impl.for_type = $concrete_type
RETURN impl, trait_method
```
This requires HAS_METHOD (from our recent addition) and preserves the trait-level call info even when concrete resolution isn't possible.

---

## Affected Files

| File | Relevance |
|------|-----------|
| `services/ingestion/src/pipeline/stages.rs` | GraphStage CALLS resolution logic (~line 2916+) |
| `services/ingestion/src/pipeline/stages.rs` | `extract_function_calls` helper method |
| `services/ingestion/src/typecheck/resolver.rs` | Has monomorphization data that could inform resolution |
| `.opencode/agents/explorer.md` | Grep fallback protocol (current workaround) |

---

## Relationship to Other Issues

- **ISSUE-001** (Closed): Fixed TypecheckStage populating `call_sites` and `trait_implementations`. This data is available but not yet used by GraphStage for CALLS refinement.
- **HAS_METHOD** relationship (recently added to code, pending re-ingestion): Required for Option C approach.
- **DEPENDS_ON** relationship: Not directly related.

---

## Priority Justification

P2 (not P1) because:
- The Explorer agent has a grep fallback that mitigates the gap
- The data exists in PG (`call_sites` has 99K monomorphized calls) — it's a matter of connecting it to the graph
- Not blocking Phase 1 (Explorer) or Phase 2 (Research + Planner)
- Becomes more important in Phase 3 (Developer + Reviewer) where impact analysis precision matters
