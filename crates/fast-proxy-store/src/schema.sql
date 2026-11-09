-- Schema v1. The project is not deployed, so changes edit this file directly
-- (no migrations); SCHEMA_VERSION in database.rs must be bumped with it.

CREATE TABLE subsystem (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  enabled     INTEGER NOT NULL DEFAULT 1,
  description TEXT NOT NULL DEFAULT '',
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE subsystem_host (
  id           INTEGER PRIMARY KEY,
  subsystem_id INTEGER NOT NULL REFERENCES subsystem(id) ON DELETE CASCADE,
  pattern      TEXT NOT NULL UNIQUE
);

CREATE TABLE upstream_group (
  id           INTEGER PRIMARY KEY,
  subsystem_id INTEGER NOT NULL REFERENCES subsystem(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  UNIQUE (subsystem_id, name)
);

CREATE TABLE upstream_endpoint (
  id       INTEGER PRIMARY KEY,
  group_id INTEGER NOT NULL REFERENCES upstream_group(id) ON DELETE CASCADE,
  addr     TEXT NOT NULL,
  weight   INTEGER NOT NULL DEFAULT 1
);

-- upstream_group_id uses the default NO ACTION rather than RESTRICT: both
-- reject deleting a group a route still uses, but NO ACTION is checked at the
-- end of the statement, so deleting a whole subsystem (which cascades to its
-- routes and groups in unspecified order) still works.
CREATE TABLE route (
  id                  INTEGER PRIMARY KEY,
  subsystem_id        INTEGER NOT NULL REFERENCES subsystem(id) ON DELETE CASCADE,
  prefix              TEXT NOT NULL,
  upstream_group_id   INTEGER NOT NULL REFERENCES upstream_group(id),
  strip_prefix        INTEGER NOT NULL DEFAULT 0,
  upstream_prefix     TEXT NOT NULL DEFAULT '',
  preserve_host       INTEGER NOT NULL DEFAULT 1,
  connect_timeout_ms  INTEGER NOT NULL DEFAULT 5000,
  response_timeout_ms INTEGER NOT NULL DEFAULT 30000,
  enabled             INTEGER NOT NULL DEFAULT 1,
  UNIQUE (subsystem_id, prefix)
);

CREATE TABLE revision (
  version    INTEGER PRIMARY KEY,
  created_at INTEGER NOT NULL,
  summary    TEXT NOT NULL,
  config     TEXT NOT NULL
);
