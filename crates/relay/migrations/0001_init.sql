-- Pocket Agent relay schema v1.
-- Apply with: wrangler d1 migrations apply DB --remote   (or `cargo xtask relay-setup`)

-- Paired devices. Only the SHA-256 hash of each device token is stored.
CREATE TABLE devices (
    id          TEXT PRIMARY KEY,
    token_hash  TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,          -- unix seconds
    last_seen   INTEGER,                   -- unix seconds
    revoked     INTEGER NOT NULL DEFAULT 0
);

-- Token usage ledger, one row per UTC day / device / model.
CREATE TABLE usage (
    day           TEXT NOT NULL,           -- YYYY-MM-DD (UTC)
    device_id     TEXT NOT NULL,
    model         TEXT NOT NULL,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read    INTEGER NOT NULL DEFAULT 0,
    cache_write   INTEGER NOT NULL DEFAULT 0,
    usd_micros    INTEGER NOT NULL DEFAULT 0,
    requests      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, device_id, model)
);

-- Preferences memory synced by the app (opaque JSON object).
CREATE TABLE prefs (
    scope      TEXT PRIMARY KEY,           -- 'global'
    json       TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Relay bookkeeping: schema_version, pairing lock state.
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO meta (key, value) VALUES ('schema_version', '1');
