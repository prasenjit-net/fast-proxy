-- Schema 5 (v0.1.1) to 6: ACME (M4). Additive only: one column with a default and
-- four new tables, so every existing row keeps its meaning.

ALTER TABLE subsystem ADD COLUMN https_acme_enabled INTEGER NOT NULL DEFAULT 0;

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
