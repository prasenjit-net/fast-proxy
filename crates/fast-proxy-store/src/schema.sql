-- Schema v7: what a new database is created with. A released schema is never
-- edited in place: a change is additive and comes with a step in `migrations/`
-- that upgrades the previous version, and SCHEMA_VERSION in database.rs is bumped
-- with it. A column added by a migration goes at the end of its table here too,
-- so a new database and an upgraded one have the same shape (a test compares them).

-- Certificate material. Private keys are stored only as AES-256-GCM ciphertext
-- (sealed by the tls crate); this crate never decrypts. The SQL stays portable
-- (BLOB to BYTEA, millisecond BIGINT times, JSON as text) so a PostgreSQL
-- implementation needs type changes only.
CREATE TABLE certificate (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  source      TEXT NOT NULL,
  chain_pem   TEXT NOT NULL,
  key_enc     BLOB NOT NULL,
  key_nonce   BLOB NOT NULL,
  names       TEXT NOT NULL,
  subject     TEXT NOT NULL,
  issuer      TEXT NOT NULL,
  not_before  INTEGER NOT NULL,
  not_after   INTEGER NOT NULL,
  fingerprint TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE ca_bundle (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  pem         TEXT NOT NULL,
  cert_count  INTEGER NOT NULL,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

-- One row proving the master key is the right one.
CREATE TABLE vault (
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  check_nonce BLOB NOT NULL,
  check_blob  BLOB NOT NULL
);

CREATE TABLE subsystem (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  enabled     INTEGER NOT NULL DEFAULT 1,
  description TEXT NOT NULL DEFAULT '',
  https_enabled           INTEGER NOT NULL DEFAULT 0,
  -- NO ACTION: a certificate a subsystem uses cannot be deleted.
  https_certificate_id    INTEGER REFERENCES certificate(id),
  https_redirect          TEXT    NOT NULL DEFAULT 'off',
  hsts_enabled            INTEGER NOT NULL DEFAULT 0,
  hsts_max_age_secs       INTEGER NOT NULL DEFAULT 31536000,
  hsts_include_subdomains INTEGER NOT NULL DEFAULT 0,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  https_acme_enabled      INTEGER NOT NULL DEFAULT 0  -- schema 6
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
  lb_algorithm         TEXT    NOT NULL DEFAULT 'round_robin',
  lb_hash_key          TEXT    NOT NULL DEFAULT 'client_ip',
  failure_policy       TEXT    NOT NULL DEFAULT 'fail_open',
  health_check         TEXT    NOT NULL DEFAULT 'none',
  health_path          TEXT    NOT NULL DEFAULT '/',
  health_status_min    INTEGER NOT NULL DEFAULT 200,
  health_status_max    INTEGER NOT NULL DEFAULT 399,
  health_interval_ms   INTEGER NOT NULL DEFAULT 5000,
  health_timeout_ms    INTEGER NOT NULL DEFAULT 2000,
  health_rise          INTEGER NOT NULL DEFAULT 2,
  health_fall          INTEGER NOT NULL DEFAULT 3,
  passive_failures     INTEGER NOT NULL DEFAULT 3,
  passive_base_ms      INTEGER NOT NULL DEFAULT 10000,
  passive_max_ms       INTEGER NOT NULL DEFAULT 300000,
  passive_max_percent  INTEGER NOT NULL DEFAULT 50,
  retry_budget_percent INTEGER NOT NULL DEFAULT 20,
  retry_budget_min_per_sec INTEGER NOT NULL DEFAULT 10,
  tls_enabled          INTEGER NOT NULL DEFAULT 0,
  tls_verify           INTEGER NOT NULL DEFAULT 1,
  tls_server_name      TEXT    NOT NULL DEFAULT '',
  tls_ca               TEXT    NOT NULL DEFAULT '',
  max_requests         INTEGER,  -- schema 7: per-endpoint in-flight cap; NULL is unlimited
  UNIQUE (subsystem_id, name)
);

CREATE TABLE upstream_endpoint (
  id       INTEGER PRIMARY KEY,
  group_id INTEGER NOT NULL REFERENCES upstream_group(id) ON DELETE CASCADE,
  addr     TEXT NOT NULL,
  weight   INTEGER NOT NULL DEFAULT 1,
  state    TEXT NOT NULL DEFAULT 'enabled',
  UNIQUE (group_id, addr)
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
  retry_max_attempts  INTEGER NOT NULL DEFAULT 2,
  retry_idempotent_on_5xx INTEGER NOT NULL DEFAULT 1,
  retry_on_timeout    INTEGER NOT NULL DEFAULT 0,
  enabled             INTEGER NOT NULL DEFAULT 1,
  max_body_bytes      INTEGER,  -- schema 7: NULL uses the process default
  UNIQUE (subsystem_id, prefix)
);

CREATE TABLE revision (
  version    INTEGER PRIMARY KEY,
  created_at INTEGER NOT NULL,
  summary    TEXT NOT NULL,
  config     TEXT NOT NULL,
  -- Who made the change: a token name, 'system' or 'cli'.
  actor      TEXT NOT NULL DEFAULT 'system'
);

-- Admin tokens. The secret is never stored, only its SHA-256; this crate never
-- hashes or verifies. Tokens, audit entries and maintenance are not part of
-- Config and a rollback never touches them.
CREATE TABLE admin_token (
  id           INTEGER PRIMARY KEY,
  name         TEXT NOT NULL UNIQUE,
  role         TEXT NOT NULL,
  secret_hash  BLOB NOT NULL,
  created_at   INTEGER NOT NULL,
  created_by   TEXT NOT NULL,
  expires_at   INTEGER,
  revoked_at   INTEGER,
  last_used_at INTEGER
);

-- Append-only from the application; pruned by age and count.
CREATE TABLE audit_log (
  id         INTEGER PRIMARY KEY,
  at         INTEGER NOT NULL,
  instance   TEXT NOT NULL,
  actor      TEXT NOT NULL,
  role       TEXT NOT NULL,
  source     TEXT NOT NULL,
  request_id TEXT NOT NULL,
  action     TEXT NOT NULL,
  target     TEXT NOT NULL,
  outcome    TEXT NOT NULL,
  detail     TEXT NOT NULL,
  revision   INTEGER
);
CREATE INDEX audit_log_at ON audit_log (at);

-- One row per active scope; removed with its subsystem or route.
CREATE TABLE maintenance (
  id                 INTEGER PRIMARY KEY,
  scope              TEXT NOT NULL,
  subsystem_id       INTEGER REFERENCES subsystem(id) ON DELETE CASCADE,
  route_id           INTEGER REFERENCES route(id) ON DELETE CASCADE,
  message            TEXT NOT NULL DEFAULT '',
  body               TEXT NOT NULL DEFAULT '',
  content_type       TEXT NOT NULL DEFAULT '',
  retry_after_secs   INTEGER,
  bypass_cidrs       TEXT NOT NULL DEFAULT '[]',
  bypass_header      TEXT NOT NULL DEFAULT '',
  bypass_secret_hash BLOB,
  expires_at         INTEGER,
  set_by             TEXT NOT NULL,
  set_at             INTEGER NOT NULL
);
CREATE UNIQUE INDEX maintenance_target
  ON maintenance (scope, COALESCE(subsystem_id, 0), COALESCE(route_id, 0));

-- Sampling settings: one global row (absent until first written; the reader
-- supplies the defaults) and overrides per subsystem or route. Like tokens,
-- audit and maintenance they are not Config and a rollback never changes them.
CREATE TABLE sampling_settings (
  id                    INTEGER PRIMARY KEY CHECK (id = 1),
  enabled               INTEGER NOT NULL,
  triggers              INTEGER NOT NULL,     -- bit set over the fixed Trigger enum
  slow_threshold_ms     INTEGER NOT NULL,
  slow_p99_factor_x10   INTEGER NOT NULL,
  slow_p99_floor_ms     INTEGER NOT NULL,
  client_errors_per_min INTEGER NOT NULL,
  oversized_request     INTEGER NOT NULL,
  oversized_response    INTEGER NOT NULL,
  baseline_per_mille    INTEGER NOT NULL,
  rate_per_sec          INTEGER NOT NULL,
  rate_burst            INTEGER NOT NULL,
  storm_burst           INTEGER NOT NULL,
  storm_interval_secs   INTEGER NOT NULL,
  retention_days        INTEGER NOT NULL,
  max_rows              INTEGER NOT NULL,
  max_bytes             INTEGER NOT NULL,
  body_request_cap      INTEGER NOT NULL,
  body_response_cap     INTEGER NOT NULL,
  extra_headers         TEXT NOT NULL,        -- JSON lists of names; redaction lists are additions only
  redact_headers        TEXT NOT NULL,
  redact_query_params   TEXT NOT NULL,
  redact_body_fields    TEXT NOT NULL,
  updated_by            TEXT NOT NULL,
  updated_at            INTEGER NOT NULL
);

-- One row per overridden target, removed with its subsystem or route.
-- NULL inherits. Redaction cannot be overridden.
CREATE TABLE sampling_override (
  id                  INTEGER PRIMARY KEY,
  scope               TEXT NOT NULL,
  subsystem_id        INTEGER REFERENCES subsystem(id) ON DELETE CASCADE,
  route_id            INTEGER REFERENCES route(id) ON DELETE CASCADE,
  enabled             INTEGER,
  capture_bodies      INTEGER,
  baseline_per_mille  INTEGER,
  slow_threshold_ms   INTEGER,
  slow_p99_factor_x10 INTEGER,
  set_by              TEXT NOT NULL,
  set_at              INTEGER NOT NULL
);
CREATE UNIQUE INDEX sampling_override_target
  ON sampling_override (scope, COALESCE(subsystem_id, 0), COALESCE(route_id, 0));

-- One row per kept request, written by every instance sharing the database.
-- Not Config: no revision, and no foreign keys (a sample outlives its subsystem
-- and route, and names are what an operator remembers). Samples are already
-- redacted when they get here. Portable SQL (plain integers and text).
CREATE TABLE sample (
  id              INTEGER PRIMARY KEY,   -- assigned by the database, one sequence for every writer
  at              INTEGER NOT NULL,      -- ms since the epoch, when the response finished
  instance        TEXT    NOT NULL,
  request_id      TEXT    NOT NULL,
  triggers        INTEGER NOT NULL,      -- bit set: filter with (triggers & mask) <> 0
  primary_trigger TEXT    NOT NULL,
  subsystem       TEXT    NOT NULL,      -- a name, or _unmatched
  route           TEXT    NOT NULL,      -- a prefix, or _none
  client          TEXT    NOT NULL,
  method          TEXT    NOT NULL,
  status          INTEGER NOT NULL,
  total_ms        INTEGER NOT NULL,
  size_bytes      INTEGER NOT NULL,      -- stored length, for the byte limit
  detail          TEXT    NOT NULL       -- the redacted sample as JSON, last so that list and
                                         -- aggregate queries never read past it (large values overflow)
);
CREATE INDEX sample_at        ON sample (at);
CREATE INDEX sample_subsystem ON sample (subsystem, id);
CREATE INDEX sample_status    ON sample (status, id);
CREATE INDEX sample_instance  ON sample (instance, id);
-- Lets the table's size be summed from the index alone, without reading rows.
CREATE INDEX sample_size      ON sample (size_bytes);

-- ACME (M4). None of these tables is Config: a revision never holds them and a
-- rollback never changes them.

-- One account per directory. The credentials (account URL and private key) are
-- sealed by the tls crate's vault; this crate sees only ciphertext.
CREATE TABLE acme_account (
  id                INTEGER PRIMARY KEY,
  directory         TEXT NOT NULL UNIQUE,
  email             TEXT NOT NULL DEFAULT '',
  credentials_enc   BLOB NOT NULL,
  credentials_nonce BLOB NOT NULL,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
);

-- Pending HTTP-01 challenges, compiled into the snapshot so every instance can
-- answer them. Written with a summary-only revision.
CREATE TABLE acme_challenge (
  token        TEXT PRIMARY KEY,
  subsystem_id INTEGER NOT NULL REFERENCES subsystem(id) ON DELETE CASCADE,
  names        TEXT NOT NULL,      -- JSON array: the hosts this token answers for
  key_auth     TEXT NOT NULL,
  expires_at   INTEGER NOT NULL
);

-- One row: the instance that may talk to the CA. Taken by compare-and-swap and
-- written without a revision.
CREATE TABLE acme_lease (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  holder     TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);

-- Per-subsystem ACME status, written without a revision and read by every
-- instance's admin plane.
CREATE TABLE acme_status (
  subsystem_id         INTEGER PRIMARY KEY REFERENCES subsystem(id) ON DELETE CASCADE,
  state                TEXT NOT NULL,
  eligible_names       TEXT NOT NULL,   -- JSON array
  skipped_names        TEXT NOT NULL,   -- JSON array of {name, reason}
  certificate_id       INTEGER REFERENCES certificate(id) ON DELETE SET NULL,
  next_attempt_at      INTEGER,
  failures             INTEGER NOT NULL DEFAULT 0,  -- since the last success
  window_failures      INTEGER NOT NULL DEFAULT 0,  -- since failure_window_start
  failure_window_start INTEGER,
  last_error           TEXT NOT NULL DEFAULT '',
  last_attempt_at      INTEGER,
  last_success_at      INTEGER,
  instance             TEXT NOT NULL DEFAULT '',
  updated_at           INTEGER NOT NULL
);

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
