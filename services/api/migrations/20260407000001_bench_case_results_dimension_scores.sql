-- Migration: Add dimension_scores to bench_case_results
-- Stores per-dimension LLM judge scores for detailed result analysis.

ALTER TABLE bench_case_results
ADD COLUMN dimension_scores JSONB;

COMMENT ON COLUMN bench_case_results.dimension_scores IS 'Per-dimension LLM judge scores as JSON array: [{"dimension": "File Precision", "score": 4.0, "reasoning": "..."}, ...]';
