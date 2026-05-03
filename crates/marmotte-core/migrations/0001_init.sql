-- Marmotte initial schema (v1).

CREATE TABLE projects (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    quota_bytes INTEGER,
    ttl_seconds INTEGER,
    created_at  INTEGER NOT NULL
);

CREATE TABLE api_keys (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key_lookup  TEXT NOT NULL UNIQUE,
    key_hash    TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('read', 'write')),
    label       TEXT,
    created_at  INTEGER NOT NULL,
    revoked_at  INTEGER
);
CREATE INDEX idx_api_keys_project ON api_keys(project_id);

CREATE TABLE admin_tokens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    token_lookup TEXT NOT NULL UNIQUE,
    token_hash   TEXT NOT NULL,
    label        TEXT,
    created_at   INTEGER NOT NULL,
    revoked_at   INTEGER
);

CREATE TABLE blobs (
    hash        TEXT PRIMARY KEY,
    size_bytes  INTEGER NOT NULL,
    refcount    INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX idx_blobs_refcount ON blobs(refcount) WHERE refcount = 0;

CREATE TABLE entries (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL CHECK (kind IN ('sstate', 'downloads')),
    path            TEXT NOT NULL,
    blob_hash       TEXT NOT NULL REFERENCES blobs(hash),
    size_bytes      INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,
    last_accessed   INTEGER NOT NULL,
    pinned          INTEGER NOT NULL DEFAULT 0,
    UNIQUE(project_id, kind, path)
);

CREATE INDEX idx_entries_proj_lru      ON entries(project_id, pinned, last_accessed);
CREATE INDEX idx_entries_global_lru    ON entries(pinned, last_accessed);
CREATE INDEX idx_entries_proj_path     ON entries(project_id, path, id);
CREATE INDEX idx_entries_proj_size     ON entries(project_id, size_bytes DESC, id);
CREATE INDEX idx_entries_proj_lastacc  ON entries(project_id, last_accessed DESC, id);

CREATE TABLE stats_counters (
    project_id  INTEGER REFERENCES projects(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    bucket_day  INTEGER NOT NULL,
    count       INTEGER NOT NULL DEFAULT 0,
    bytes       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, kind, bucket_day)
);

CREATE TABLE audit_log (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp    INTEGER NOT NULL,
    actor_token  TEXT,
    action       TEXT NOT NULL,
    target       TEXT,
    detail_json  TEXT
);
CREATE INDEX idx_audit_log_timestamp ON audit_log(timestamp DESC);
