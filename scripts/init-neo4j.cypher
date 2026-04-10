// =============================================================================
// rust-brain — Neo4j Initialization Script
// =============================================================================

// Constraints
CREATE CONSTRAINT crate_name_unique IF NOT EXISTS FOR (c:Crate) REQUIRE c.name IS UNIQUE;
CREATE CONSTRAINT module_fqn_unique IF NOT EXISTS FOR (m:Module) REQUIRE m.fqn IS UNIQUE;
CREATE CONSTRAINT function_fqn_unique IF NOT EXISTS FOR (f:Function) REQUIRE f.fqn IS UNIQUE;
CREATE CONSTRAINT struct_fqn_unique IF NOT EXISTS FOR (s:Struct) REQUIRE s.fqn IS UNIQUE;
CREATE CONSTRAINT enum_fqn_unique IF NOT EXISTS FOR (e:Enum) REQUIRE e.fqn IS UNIQUE;
CREATE CONSTRAINT trait_fqn_unique IF NOT EXISTS FOR (t:Trait) REQUIRE t.fqn IS UNIQUE;
CREATE CONSTRAINT impl_id_unique IF NOT EXISTS FOR (i:Impl) REQUIRE i.id IS UNIQUE;
CREATE CONSTRAINT type_fqn_unique IF NOT EXISTS FOR (t:Type) REQUIRE t.fqn IS UNIQUE;
CREATE CONSTRAINT type_alias_fqn_unique IF NOT EXISTS FOR (t:TypeAlias) REQUIRE t.fqn IS UNIQUE;
CREATE CONSTRAINT const_fqn_unique IF NOT EXISTS FOR (c:Const) REQUIRE c.fqn IS UNIQUE;
CREATE CONSTRAINT static_fqn_unique IF NOT EXISTS FOR (s:Static) REQUIRE s.fqn IS UNIQUE;
CREATE CONSTRAINT macro_fqn_unique IF NOT EXISTS FOR (m:Macro) REQUIRE m.fqn IS UNIQUE;

// Indexes
CREATE INDEX function_name_idx IF NOT EXISTS FOR (f:Function) ON (f.name);
CREATE INDEX struct_name_idx IF NOT EXISTS FOR (s:Struct) ON (s.name);
CREATE INDEX enum_name_idx IF NOT EXISTS FOR (e:Enum) ON (e.name);
CREATE INDEX trait_name_idx IF NOT EXISTS FOR (t:Trait) ON (t.name);
CREATE INDEX module_crate_idx IF NOT EXISTS FOR (m:Module) ON (m.crate_name);
CREATE INDEX function_visibility_idx IF NOT EXISTS FOR (f:Function) ON (f.visibility);
CREATE INDEX function_async_idx IF NOT EXISTS FOR (f:Function) ON (f.is_async);
CREATE INDEX function_unsafe_idx IF NOT EXISTS FOR (f:Function) ON (f.is_unsafe);
CREATE INDEX function_generic_idx IF NOT EXISTS FOR (f:Function) ON (f.is_generic);

// Relationship property indexes
CREATE INDEX crate_deps_idx IF NOT EXISTS FOR ()-[r:DEPENDS_ON]-() ON (r.is_dev);
CREATE INDEX trait_methods_idx IF NOT EXISTS FOR ()-[r:HAS_METHOD]-() ON (r.is_required);

// =============================================================================
// Read-only API user
// =============================================================================
// Used by the API service for the query_graph endpoint.
// CREATE USER works on Community and Enterprise editions (Neo4j 4.0+).
// Update the password here AND set NEO4J_READONLY_PASSWORD in your .env before deploying.
CREATE USER rustbrain_readonly IF NOT EXISTS
  SET PASSWORD 'rustbrain_readonly_dev_2024'
  CHANGE NOT REQUIRED;
// GRANT ROLE is Enterprise Edition only. Uncomment on Enterprise to enforce read-only at the DB level:
// GRANT ROLE reader TO rustbrain_readonly;
