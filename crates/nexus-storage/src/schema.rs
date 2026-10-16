//! Versioned schema. `PRAGMA user_version` records the applied version; each
//! migration runs in its own transaction. A database newer than this build
//! is refused, never "interpreted approximately" (`CONTRACTE_NEXUS_RO.md` §13).

use rusqlite::Connection;

use crate::StorageError;

pub const SUPPORTED_VERSION: i64 = 4;

/// Version 1: everything M1 persists. State columns are closed lists that a
/// test keeps identical to the `nexus-core` enums. Times are UTC unix ms.
const V1: &str = r#"
CREATE TABLE projects (
    id            TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    revision      INTEGER NOT NULL CHECK (revision >= 1),
    created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE tasks (
    id              TEXT    PRIMARY KEY,
    project_id      TEXT    NOT NULL REFERENCES projects (id),
    idempotency_key TEXT    NOT NULL UNIQUE,
    request_json    TEXT    NOT NULL CHECK (json_valid(request_json)),
    request_sha256  TEXT    NOT NULL,
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE runs (
    id            TEXT    PRIMARY KEY,
    task_id       TEXT    NOT NULL REFERENCES tasks (id),
    state         TEXT    NOT NULL CHECK (state IN (
                      'Queued', 'Running', 'AwaitingApproval', 'Verifying',
                      'CancellationRequested', 'NeedsReconciliation', 'NeedsReview',
                      'Succeeded', 'Failed', 'Cancelled', 'Denied')),
    revision      INTEGER NOT NULL CHECK (revision >= 1),
    error_code    TEXT,
    error_message TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX runs_by_state ON runs (state);

CREATE TABLE actions (
    id              TEXT    PRIMARY KEY,
    run_id          TEXT    NOT NULL REFERENCES runs (id),
    operation_id    TEXT    NOT NULL UNIQUE,
    tool_id         TEXT    NOT NULL,
    tool_version    INTEGER NOT NULL,
    state           TEXT    NOT NULL CHECK (state IN (
                        'Prepared', 'AwaitingApproval', 'Authorized', 'Executing',
                        'Verified', 'Failed', 'NeedsReconciliation', 'Denied',
                        'Expired', 'Invalidated')),
    manifest_json   TEXT    NOT NULL CHECK (json_valid(manifest_json)),
    manifest_sha256 TEXT    NOT NULL,
    revision        INTEGER NOT NULL CHECK (revision >= 1),
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE approvals (
    id              TEXT    PRIMARY KEY,
    action_id       TEXT    NOT NULL UNIQUE REFERENCES actions (id),
    manifest_sha256 TEXT    NOT NULL,
    expires_at_ms   INTEGER NOT NULL,
    decision        TEXT    CHECK (decision IN ('Approved', 'Denied')),
    decided_at_ms   INTEGER,
    consumed_at_ms  INTEGER,
    revoked_at_ms   INTEGER,
    -- Only an approved decision can be consumed, and only once.
    CHECK (consumed_at_ms IS NULL OR decision = 'Approved'),
    CHECK ((decision IS NULL) = (decided_at_ms IS NULL))
) STRICT;

CREATE TABLE operations (
    id                TEXT    PRIMARY KEY,
    action_id         TEXT    NOT NULL UNIQUE REFERENCES actions (id),
    state             TEXT    NOT NULL CHECK (state IN (
                          'Intended', 'Verified', 'Failed', 'NeedsReview')),
    attempt           INTEGER NOT NULL CHECK (attempt >= 1),
    target_relpath    TEXT    NOT NULL UNIQUE,
    staging_relpath   TEXT    NOT NULL UNIQUE,
    expected_sha256   TEXT    NOT NULL,
    expected_size     INTEGER NOT NULL CHECK (expected_size >= 0),
    algorithm_version INTEGER NOT NULL,
    created_at_ms     INTEGER NOT NULL,
    updated_at_ms     INTEGER NOT NULL
) STRICT;

CREATE TABLE artifacts (
    id            TEXT    PRIMARY KEY,
    operation_id  TEXT    NOT NULL UNIQUE REFERENCES operations (id),
    project_id    TEXT    NOT NULL REFERENCES projects (id),
    relpath       TEXT    NOT NULL UNIQUE,
    sha256        TEXT    NOT NULL,
    size_bytes    INTEGER NOT NULL CHECK (size_bytes >= 0),
    created_at_ms INTEGER NOT NULL
) STRICT;

-- Written in the same transaction as the state change it describes.
CREATE TABLE events (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    id              TEXT    NOT NULL UNIQUE,
    kind            TEXT    NOT NULL,
    entity_id       TEXT    NOT NULL,
    entity_revision INTEGER NOT NULL,
    payload_json    TEXT    NOT NULL CHECK (json_valid(payload_json)),
    created_at_ms   INTEGER NOT NULL
) STRICT;
"#;

/// Version 2 (M2): AI provider connections, a versioned persona and
/// conversations. API keys are never stored here: a connection holds only an
/// opaque name of its entry in the Windows vault.
const V2: &str = r#"
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value_json TEXT NOT NULL CHECK (json_valid(value_json))
) STRICT;

CREATE TABLE provider_connections (
    id            TEXT    PRIMARY KEY,
    provider      TEXT    NOT NULL CHECK (provider IN ('Gemini')),
    display_name  TEXT    NOT NULL CHECK (length(display_name) BETWEEN 1 AND 100),
    model         TEXT,
    free_tier     INTEGER NOT NULL CHECK (free_tier IN (0, 1)),
    secret_name   TEXT    NOT NULL UNIQUE,
    revision      INTEGER NOT NULL CHECK (revision >= 1),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE personas (
    version       INTEGER PRIMARY KEY CHECK (version >= 1),
    name          TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 60),
    tone          TEXT    NOT NULL CHECK (length(tone) <= 500),
    style         TEXT    NOT NULL CHECK (length(style) <= 1000),
    created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE conversations (
    id            TEXT    PRIMARY KEY,
    title         TEXT    NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE messages (
    id              TEXT    PRIMARY KEY,
    conversation_id TEXT    NOT NULL REFERENCES conversations (id),
    seq             INTEGER NOT NULL CHECK (seq >= 1),
    role            TEXT    NOT NULL CHECK (role IN ('User', 'Model')),
    text            TEXT    NOT NULL,
    status          TEXT    NOT NULL CHECK (status IN ('Streaming', 'Complete', 'Failed', 'Cancelled')),
    idempotency_key TEXT    UNIQUE,
    connection_id   TEXT    REFERENCES provider_connections (id) ON DELETE SET NULL,
    model           TEXT,
    persona_version INTEGER REFERENCES personas (version),
    error_code      TEXT,
    error_message   TEXT,
    prompt_tokens   INTEGER,
    output_tokens   INTEGER,
    proposed_run_id TEXT    REFERENCES runs (id),
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    UNIQUE (conversation_id, seq)
) STRICT;

CREATE INDEX messages_by_status ON messages (status);
"#;

/// Version 3: the user's speed/quality choice for models that support it
/// (Gemini 3.x; `nexus_providers::ThinkingLevel`). Existing rows default to
/// `Low`, matching the fixed behaviour this replaces, so nothing changes
/// for a connection until the user picks something else.
const V3: &str = r#"
ALTER TABLE provider_connections
    ADD COLUMN thinking_level TEXT NOT NULL DEFAULT 'Low'
    CHECK (thinking_level IN ('Low', 'Medium', 'High'));
"#;

/// Version 4: what the agent did while producing a reply (model calls, web
/// reads, proposed notes), so the user can watch it work and look back.
/// Only metadata — a fetched page's URL, status and size, never its text.
const V4: &str = r#"
CREATE TABLE agent_steps (
    id             TEXT    PRIMARY KEY,
    message_id     TEXT    NOT NULL REFERENCES messages (id),
    seq            INTEGER NOT NULL CHECK (seq >= 1),
    kind           TEXT    NOT NULL CHECK (kind IN ('ModelCall', 'Fetch', 'NoteProposed')),
    status         TEXT    NOT NULL CHECK (status IN ('Running', 'Done', 'Failed')),
    detail_json    TEXT    NOT NULL CHECK (json_valid(detail_json)),
    started_at_ms  INTEGER NOT NULL,
    finished_at_ms INTEGER,
    -- A step is finished exactly when it is no longer running.
    CHECK ((status = 'Running') = (finished_at_ms IS NULL)),
    UNIQUE (message_id, seq)
) STRICT;

CREATE INDEX agent_steps_by_start ON agent_steps (started_at_ms);
CREATE INDEX agent_steps_by_status ON agent_steps (status);
"#;

const MIGRATIONS: &[&str] = &[V1, V2, V3, V4];

pub fn migrate(conn: &mut Connection) -> Result<(), StorageError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > SUPPORTED_VERSION {
        return Err(StorageError::SchemaTooNew {
            found: current,
            supported: SUPPORTED_VERSION,
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = index as i64 + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}
