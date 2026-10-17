//! Versioned schema. `PRAGMA user_version` records the applied version; each
//! migration runs in its own transaction. A database newer than this build
//! is refused, never "interpreted approximately" (`CONTRACTE_NEXUS_RO.md` §13).

use rusqlite::Connection;

use crate::StorageError;

pub const SUPPORTED_VERSION: i64 = 10;

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

/// Version 5: connections to more providers (OpenAI, Anthropic, DeepSeek).
/// SQLite cannot change a CHECK in place, so the table is rebuilt with the
/// procedure SQLite documents for this ("Making Other Kinds Of Table Schema
/// Changes"): foreign keys off around the transaction — otherwise dropping
/// the old table would clear `messages.connection_id` through its
/// `ON DELETE SET NULL` — then a foreign-key check before commit. Every
/// row, id and column value is copied unchanged.
const V5: &str = r#"
CREATE TABLE provider_connections_v5 (
    id             TEXT    PRIMARY KEY,
    provider       TEXT    NOT NULL CHECK (provider IN ('Gemini', 'OpenAI', 'Anthropic', 'DeepSeek')),
    display_name   TEXT    NOT NULL CHECK (length(display_name) BETWEEN 1 AND 100),
    model          TEXT,
    free_tier      INTEGER NOT NULL CHECK (free_tier IN (0, 1)),
    secret_name    TEXT    NOT NULL UNIQUE,
    revision       INTEGER NOT NULL CHECK (revision >= 1),
    created_at_ms  INTEGER NOT NULL,
    updated_at_ms  INTEGER NOT NULL,
    thinking_level TEXT    NOT NULL DEFAULT 'Low'
                   CHECK (thinking_level IN ('Low', 'Medium', 'High'))
) STRICT;

INSERT INTO provider_connections_v5
    (id, provider, display_name, model, free_tier, secret_name, revision,
     created_at_ms, updated_at_ms, thinking_level)
SELECT id, provider, display_name, model, free_tier, secret_name, revision,
       created_at_ms, updated_at_ms, thinking_level
FROM provider_connections;

DROP TABLE provider_connections;
ALTER TABLE provider_connections_v5 RENAME TO provider_connections;
"#;

/// Version 6: projects as the unit of work, and memory. Every existing
/// conversation joins the first (default) project. Memory facts carry their
/// scope (about the user, or one project), status and provenance; the
/// full-text index is derived from them (triggers keep it in step, and
/// `INSERT INTO memory_fts(memory_fts) VALUES ('rebuild')` recreates it) and
/// ignores diacritics, so "nota" finds "notă". Agent steps gain the
/// MemoryProposed kind: a closed CHECK list, so that table is rebuilt (no
/// other table refers to it).
const V6: &str = r#"
ALTER TABLE projects
    ADD COLUMN description TEXT NOT NULL DEFAULT '' CHECK (length(description) <= 1000);

ALTER TABLE conversations ADD COLUMN project_id TEXT REFERENCES projects (id);
UPDATE conversations
SET project_id = (SELECT id FROM projects ORDER BY created_at_ms, id LIMIT 1)
WHERE project_id IS NULL;
CREATE INDEX conversations_by_project ON conversations (project_id, updated_at_ms);

CREATE TABLE agent_steps_v6 (
    id             TEXT    PRIMARY KEY,
    message_id     TEXT    NOT NULL REFERENCES messages (id),
    seq            INTEGER NOT NULL CHECK (seq >= 1),
    kind           TEXT    NOT NULL CHECK (kind IN ('ModelCall', 'Fetch', 'NoteProposed', 'MemoryProposed')),
    status         TEXT    NOT NULL CHECK (status IN ('Running', 'Done', 'Failed')),
    detail_json    TEXT    NOT NULL CHECK (json_valid(detail_json)),
    started_at_ms  INTEGER NOT NULL,
    finished_at_ms INTEGER,
    CHECK ((status = 'Running') = (finished_at_ms IS NULL)),
    UNIQUE (message_id, seq)
) STRICT;
INSERT INTO agent_steps_v6
    (id, message_id, seq, kind, status, detail_json, started_at_ms, finished_at_ms)
SELECT id, message_id, seq, kind, status, detail_json, started_at_ms, finished_at_ms
FROM agent_steps;
DROP TABLE agent_steps;
ALTER TABLE agent_steps_v6 RENAME TO agent_steps;
CREATE INDEX agent_steps_by_start ON agent_steps (started_at_ms);
CREATE INDEX agent_steps_by_status ON agent_steps (status);

CREATE TABLE memory_items (
    id              TEXT    PRIMARY KEY,
    scope           TEXT    NOT NULL CHECK (scope IN ('Personal', 'Project')),
    project_id      TEXT    REFERENCES projects (id),
    content         TEXT    NOT NULL CHECK (length(content) BETWEEN 1 AND 1000),
    status          TEXT    NOT NULL CHECK (status IN ('Proposed', 'Active', 'Rejected')),
    source          TEXT    NOT NULL CHECK (source IN ('User', 'Agent')),
    edited_by_user  INTEGER NOT NULL DEFAULT 0 CHECK (edited_by_user IN (0, 1)),
    conversation_id TEXT    REFERENCES conversations (id),
    message_id      TEXT    REFERENCES messages (id),
    -- Hosts whose pages the agent had read in that reply before proposing.
    influences_json TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(influences_json)),
    revision        INTEGER NOT NULL CHECK (revision >= 1),
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    decided_at_ms   INTEGER,
    -- A project fact names its project; a personal one names none.
    CHECK ((scope = 'Project') = (project_id IS NOT NULL)),
    -- Only the agent proposes; the user's own facts start active.
    CHECK (source = 'Agent' OR status = 'Active')
) STRICT;
CREATE INDEX memory_items_by_scope ON memory_items (status, scope, project_id);

CREATE VIRTUAL TABLE memory_fts USING fts5(
    content,
    content = 'memory_items',
    content_rowid = 'rowid',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER memory_items_ai AFTER INSERT ON memory_items BEGIN
    INSERT INTO memory_fts (rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER memory_items_ad AFTER DELETE ON memory_items BEGIN
    INSERT INTO memory_fts (memory_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
END;
CREATE TRIGGER memory_items_au AFTER UPDATE OF content ON memory_items BEGIN
    INSERT INTO memory_fts (memory_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO memory_fts (rowid, content) VALUES (new.rowid, new.content);
END;
"#;

/// Version 7: remote MCP servers and their tools (`PLAN_NEXUS_RO.md`, step
/// 3), and the agent step that calls one. A tool is offered only while the
/// user has enabled exactly its current definition.
const V7: &str = r#"
CREATE TABLE agent_steps_v7 (
    id             TEXT    PRIMARY KEY,
    message_id     TEXT    NOT NULL REFERENCES messages (id),
    seq            INTEGER NOT NULL CHECK (seq >= 1),
    kind           TEXT    NOT NULL CHECK (kind IN ('ModelCall', 'Fetch', 'NoteProposed', 'MemoryProposed', 'McpCall')),
    status         TEXT    NOT NULL CHECK (status IN ('Running', 'Done', 'Failed')),
    detail_json    TEXT    NOT NULL CHECK (json_valid(detail_json)),
    started_at_ms  INTEGER NOT NULL,
    finished_at_ms INTEGER,
    CHECK ((status = 'Running') = (finished_at_ms IS NULL)),
    UNIQUE (message_id, seq)
) STRICT;
INSERT INTO agent_steps_v7
    (id, message_id, seq, kind, status, detail_json, started_at_ms, finished_at_ms)
SELECT id, message_id, seq, kind, status, detail_json, started_at_ms, finished_at_ms
FROM agent_steps;
DROP TABLE agent_steps;
ALTER TABLE agent_steps_v7 RENAME TO agent_steps;
CREATE INDEX agent_steps_by_start ON agent_steps (started_at_ms);
CREATE INDEX agent_steps_by_status ON agent_steps (status);

CREATE TABLE mcp_servers (
    id            TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    url           TEXT    NOT NULL UNIQUE CHECK (length(url) BETWEEN 1 AND 2000),
    -- The Windows vault entry holding its key; NULL when it needs none.
    secret_name   TEXT,
    revision      INTEGER NOT NULL CHECK (revision >= 1),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    checked_at_ms INTEGER,
    last_error    TEXT    CHECK (last_error IS NULL OR length(last_error) <= 500)
) STRICT;

CREATE TABLE mcp_tools (
    server_id         TEXT    NOT NULL REFERENCES mcp_servers (id) ON DELETE CASCADE,
    name              TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
    title             TEXT,
    description       TEXT    NOT NULL CHECK (length(description) <= 4000),
    input_schema_json TEXT    NOT NULL CHECK (json_valid(input_schema_json)),
    annotations_json  TEXT    NOT NULL CHECK (json_valid(annotations_json)),
    -- SHA-256 of the definition as last listed, and of the one the user
    -- enabled: a tool the server changed is off until enabled again.
    definition_sha256 TEXT    NOT NULL CHECK (length(definition_sha256) = 64),
    enabled_sha256    TEXT    CHECK (enabled_sha256 IS NULL OR length(enabled_sha256) = 64),
    available         INTEGER NOT NULL CHECK (available IN (0, 1)),
    position          INTEGER NOT NULL,
    first_seen_at_ms  INTEGER NOT NULL,
    updated_at_ms     INTEGER NOT NULL,
    PRIMARY KEY (server_id, name)
) STRICT;
"#;

/// Version 8: a reply that stopped on a proposal waiting for the user keeps
/// what the model needs to continue the task once the user decides (its
/// own calls, their signatures and any signed thinking; external data such
/// as page text is not kept). The row lives only until the task continues,
/// or goes with its message. Runs gain the outcome of an effect that leaves
/// no file (a call to an MCP tool): its size and duration, never content.
const V8: &str = r#"
ALTER TABLE runs ADD COLUMN outcome_json TEXT
    CHECK (outcome_json IS NULL OR json_valid(outcome_json));

CREATE TABLE paused_turns (
    message_id      TEXT    PRIMARY KEY REFERENCES messages (id) ON DELETE CASCADE,
    conversation_id TEXT    NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    run_id          TEXT    NOT NULL REFERENCES runs (id),
    state_json      TEXT    NOT NULL CHECK (json_valid(state_json)),
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE INDEX paused_turns_by_run ON paused_turns (run_id);
CREATE INDEX paused_turns_by_conversation ON paused_turns (conversation_id);
"#;

/// Version 9: what each model call cost in tokens, and the prices the user
/// enters per model. The ledger holds numbers only (no text), so it outlives
/// the tasks whose calls it counts: the daily budget is not reset by
/// deleting a task.
const V9: &str = r#"
CREATE TABLE model_usage (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms         INTEGER NOT NULL,
    provider      TEXT    NOT NULL,
    model         TEXT    NOT NULL,
    free_tier     INTEGER NOT NULL CHECK (free_tier IN (0, 1)),
    prompt_tokens INTEGER NOT NULL CHECK (prompt_tokens >= 0),
    output_tokens INTEGER NOT NULL CHECK (output_tokens >= 0)
) STRICT;

CREATE INDEX model_usage_by_time ON model_usage (at_ms);

CREATE TABLE model_prices (
    provider           TEXT    NOT NULL,
    model              TEXT    NOT NULL,
    input_per_million  REAL    NOT NULL CHECK (input_per_million >= 0),
    output_per_million REAL    NOT NULL CHECK (output_per_million >= 0),
    updated_at_ms      INTEGER NOT NULL,
    PRIMARY KEY (provider, model)
) STRICT;
"#;

/// Version 10: automations (`PLAN_NEXUS_RO.md` M4): an instruction run as a
/// new task on a schedule, and what happened at each of its times. A run
/// names the task it started; deleting that task keeps the entry without
/// it. Deleting an automation removes its history, never its tasks.
const V10: &str = r#"
CREATE TABLE automations (
    id             TEXT    PRIMARY KEY,
    name           TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    prompt         TEXT    NOT NULL CHECK (length(prompt) BETWEEN 1 AND 16384),
    project_id     TEXT    REFERENCES projects (id),
    schedule_json  TEXT    NOT NULL CHECK (json_valid(schedule_json)),
    enabled        INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    catch_up       INTEGER NOT NULL CHECK (catch_up IN (0, 1)),
    -- UTC unix ms of the next time due; NULL when it will not run again.
    next_run_at_ms INTEGER,
    revision       INTEGER NOT NULL CHECK (revision >= 1),
    created_at_ms  INTEGER NOT NULL,
    updated_at_ms  INTEGER NOT NULL,
    CHECK (enabled = 1 OR next_run_at_ms IS NULL)
) STRICT;

CREATE INDEX automations_by_next_run ON automations (next_run_at_ms) WHERE next_run_at_ms IS NOT NULL;

CREATE TABLE automation_runs (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    automation_id    TEXT    NOT NULL REFERENCES automations (id) ON DELETE CASCADE,
    scheduled_for_ms INTEGER NOT NULL,
    at_ms            INTEGER NOT NULL,
    status           TEXT    NOT NULL CHECK (status IN ('Started', 'Skipped', 'Missed', 'Failed')),
    reason           TEXT    CHECK (reason IS NULL OR length(reason) <= 500),
    conversation_id  TEXT    REFERENCES conversations (id) ON DELETE SET NULL,
    catch_up         INTEGER NOT NULL CHECK (catch_up IN (0, 1)),
    manual           INTEGER NOT NULL CHECK (manual IN (0, 1)),
    CHECK (status = 'Started' OR conversation_id IS NULL)
) STRICT;

CREATE INDEX automation_runs_by_automation ON automation_runs (automation_id, id);
CREATE INDEX automation_runs_by_conversation ON automation_runs (conversation_id);
"#;

struct Migration {
    sql: &'static str,
    /// Rebuilds a table other tables refer to (see [`V5`]).
    rebuilds_parent_table: bool,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        sql: V1,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V2,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V3,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V4,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V5,
        rebuilds_parent_table: true,
    },
    Migration {
        sql: V6,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V7,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V8,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V9,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V10,
        rebuilds_parent_table: false,
    },
];

pub fn migrate(conn: &mut Connection) -> Result<(), StorageError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > SUPPORTED_VERSION {
        return Err(StorageError::SchemaTooNew {
            found: current,
            supported: SUPPORTED_VERSION,
        });
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = index as i64 + 1;
        // Foreign-key enforcement can only be switched outside a transaction.
        if migration.rebuilds_parent_table {
            conn.pragma_update(None, "foreign_keys", false)?;
        }
        let result = (|| -> Result<(), StorageError> {
            let tx = conn.transaction()?;
            tx.execute_batch(migration.sql)?;
            if migration.rebuilds_parent_table {
                let broken: i64 =
                    tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                        r.get(0)
                    })?;
                if broken > 0 {
                    // Dropping the transaction rolls the rebuild back.
                    return Err(StorageError::Corrupt(format!(
                        "migration {version} would leave {broken} broken references"
                    )));
                }
            }
            tx.pragma_update(None, "user_version", version)?;
            tx.commit()?;
            Ok(())
        })();
        if migration.rebuilds_parent_table {
            conn.pragma_update(None, "foreign_keys", true)?;
        }
        result?;
    }
    Ok(())
}
