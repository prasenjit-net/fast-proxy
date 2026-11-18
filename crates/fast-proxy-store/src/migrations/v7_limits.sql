-- Schema 6 to 7: limits (M7, ADRs 0030 and 0031). Additive only: two nullable
-- columns (NULL keeps today's behaviour: unlimited) and one new table.

ALTER TABLE upstream_group ADD COLUMN max_requests INTEGER;
ALTER TABLE route ADD COLUMN max_body_bytes INTEGER;

-- Rate-limit policies (M7, schema 7; ADR 0030). Config: a revision holds them and
-- a rollback restores them. One per subsystem (route_id NULL, shared by its routes
-- that have none of their own) and at most one per route, which replaces it.
CREATE TABLE rate_limit_policy (
  id           INTEGER PRIMARY KEY,
  subsystem_id INTEGER NOT NULL REFERENCES subsystem(id) ON DELETE CASCADE,
  route_id     INTEGER REFERENCES route(id) ON DELETE CASCADE,
  rate         INTEGER NOT NULL,
  period_secs  INTEGER NOT NULL,
  burst        INTEGER NOT NULL,
  key_kind     TEXT    NOT NULL CHECK (key_kind IN ('client', 'header')),
  key_header   TEXT,
  ipv6_prefix  INTEGER NOT NULL DEFAULT 64
);
-- UNIQUE treats NULLs as distinct, so the subsystem's own policy needs its own index.
CREATE UNIQUE INDEX rate_limit_policy_subsystem ON rate_limit_policy (subsystem_id)
  WHERE route_id IS NULL;
CREATE UNIQUE INDEX rate_limit_policy_route ON rate_limit_policy (route_id)
  WHERE route_id IS NOT NULL;
