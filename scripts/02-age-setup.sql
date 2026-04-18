-- =============================================================================
-- rust-brain — Apache AGE Graph Extension Setup
-- =============================================================================
-- Runs automatically on first Postgres container startup via
-- docker-entrypoint-initdb.d (after 01-init.sql).
-- =============================================================================

-- Load the AGE extension into the current session
LOAD 'age';

-- Create the AGE extension in the database
CREATE EXTENSION IF NOT EXISTS age;

-- Set search_path so ag_catalog is available for Cypher queries
-- Uses current_database() instead of psql variable for portability
DO $$
BEGIN
  EXECUTE format('ALTER DATABASE %I SET search_path = ag_catalog, "$user", public', current_database());
END
$$;
