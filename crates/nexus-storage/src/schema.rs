//! Versioned schema. `PRAGMA user_version` records the applied version; each
//! migration runs in its own transaction. A database newer than this build
//! is refused, never "interpreted approximately" (`CONTRACTE_NEXUS_RO.md` §13).

use rusqlite::Connection;

use crate::StorageError;

pub const SUPPORTED_VERSION: i64 = 19;

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

/// Version 11: registers (`PLAN_NEXUS_RO.md` §3a): tables Nexus keeps for
/// the user (job applications first), their records and the history of
/// every change, which names who made it and, for the agent, the task and
/// the web hosts read before. A register's fields live in its schema; the
/// store only knows which field is the status and which the follow-up date.
/// Agent steps gain RecordsFound and RecordChanged (a closed CHECK list,
/// so that table is rebuilt; no other table refers to it).
const V11: &str = r#"
CREATE TABLE registers (
    id              TEXT    PRIMARY KEY,
    key             TEXT    NOT NULL UNIQUE CHECK (length(key) BETWEEN 1 AND 64),
    name            TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    schema_json     TEXT    NOT NULL CHECK (json_valid(schema_json)),
    status_field    TEXT    NOT NULL,
    follow_up_field TEXT,
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE records (
    id            TEXT    PRIMARY KEY,
    register_id   TEXT    NOT NULL REFERENCES registers (id) ON DELETE CASCADE,
    values_json   TEXT    NOT NULL CHECK (json_valid(values_json) AND json_type(values_json) = 'object'),
    -- Copies of the status and follow-up date (YYYY-MM-DD), for filtering.
    status        TEXT,
    follow_up_on  TEXT,
    revision      INTEGER NOT NULL CHECK (revision >= 1),
    created_by    TEXT    NOT NULL CHECK (created_by IN ('User', 'Agent')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX records_by_register ON records (register_id, updated_at_ms);
CREATE INDEX records_by_follow_up ON records (register_id, follow_up_on);

CREATE TABLE record_changes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    record_id       TEXT    NOT NULL REFERENCES records (id) ON DELETE CASCADE,
    kind            TEXT    NOT NULL CHECK (kind IN ('Created', 'Updated', 'Undo')),
    actor           TEXT    NOT NULL CHECK (actor IN ('User', 'Agent')),
    before_json     TEXT    CHECK (before_json IS NULL OR json_valid(before_json)),
    after_json      TEXT    NOT NULL CHECK (json_valid(after_json)),
    revision_after  INTEGER NOT NULL CHECK (revision_after >= 1),
    conversation_id TEXT    REFERENCES conversations (id) ON DELETE SET NULL,
    message_id      TEXT    REFERENCES messages (id) ON DELETE SET NULL,
    read_hosts_json TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(read_hosts_json)),
    undone_at_ms    INTEGER,
    at_ms           INTEGER NOT NULL,
    CHECK ((kind = 'Created') = (before_json IS NULL)),
    CHECK (actor = 'Agent' OR (conversation_id IS NULL AND message_id IS NULL))
) STRICT;

CREATE INDEX record_changes_by_record ON record_changes (record_id, id);
CREATE INDEX record_changes_by_message ON record_changes (message_id);

CREATE TABLE agent_steps_v11 (
    id             TEXT    PRIMARY KEY,
    message_id     TEXT    NOT NULL REFERENCES messages (id),
    seq            INTEGER NOT NULL CHECK (seq >= 1),
    kind           TEXT    NOT NULL CHECK (kind IN ('ModelCall', 'Fetch', 'NoteProposed', 'MemoryProposed', 'McpCall',
                                                    'RecordsFound', 'RecordChanged')),
    status         TEXT    NOT NULL CHECK (status IN ('Running', 'Done', 'Failed')),
    detail_json    TEXT    NOT NULL CHECK (json_valid(detail_json)),
    started_at_ms  INTEGER NOT NULL,
    finished_at_ms INTEGER,
    CHECK ((status = 'Running') = (finished_at_ms IS NULL)),
    UNIQUE (message_id, seq)
) STRICT;
INSERT INTO agent_steps_v11
    (id, message_id, seq, kind, status, detail_json, started_at_ms, finished_at_ms)
SELECT id, message_id, seq, kind, status, detail_json, started_at_ms, finished_at_ms
FROM agent_steps;
DROP TABLE agent_steps;
ALTER TABLE agent_steps_v11 RENAME TO agent_steps;
CREATE INDEX agent_steps_by_start ON agent_steps (started_at_ms);
CREATE INDEX agent_steps_by_status ON agent_steps (status);
"#;

/// Version 12: folders the user shares with Nexus (`PLAN_NEXUS_RO.md` §3a,
/// step 2). A row is only the user's permission: documents are read from
/// the folder when needed, never copied here. Agent steps gain
/// FilesListed, DocumentRead and DocumentProposed (the CHECK list is closed,
/// so that table is rebuilt; no other table refers to it).
const V12: &str = r#"
CREATE TABLE folders (
    id            TEXT    PRIMARY KEY,
    label         TEXT    NOT NULL UNIQUE CHECK (length(label) BETWEEN 1 AND 60),
    path          TEXT    NOT NULL UNIQUE CHECK (length(path) BETWEEN 3 AND 1024),
    can_write     INTEGER NOT NULL CHECK (can_write IN (0, 1)),
    revision      INTEGER NOT NULL CHECK (revision >= 1),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE agent_steps_v12 (
    id             TEXT    PRIMARY KEY,
    message_id     TEXT    NOT NULL REFERENCES messages (id),
    seq            INTEGER NOT NULL CHECK (seq >= 1),
    kind           TEXT    NOT NULL CHECK (kind IN ('ModelCall', 'Fetch', 'NoteProposed', 'MemoryProposed', 'McpCall',
                                                    'RecordsFound', 'RecordChanged', 'FilesListed', 'DocumentRead',
                                                    'DocumentProposed')),
    status         TEXT    NOT NULL CHECK (status IN ('Running', 'Done', 'Failed')),
    detail_json    TEXT    NOT NULL CHECK (json_valid(detail_json)),
    started_at_ms  INTEGER NOT NULL,
    finished_at_ms INTEGER,
    CHECK ((status = 'Running') = (finished_at_ms IS NULL)),
    UNIQUE (message_id, seq)
) STRICT;
INSERT INTO agent_steps_v12
    (id, message_id, seq, kind, status, detail_json, started_at_ms, finished_at_ms)
SELECT id, message_id, seq, kind, status, detail_json, started_at_ms, finished_at_ms
FROM agent_steps;
DROP TABLE agent_steps;
ALTER TABLE agent_steps_v12 RENAME TO agent_steps;
CREATE INDEX agent_steps_by_start ON agent_steps (started_at_ms);
CREATE INDEX agent_steps_by_status ON agent_steps (status);
"#;

/// Version 13: a register's inbox (`PLAN_NEXUS_RO.md` §3a): a shared folder
/// whose new documents the agent reads and records on its own, and one row
/// per file content seen there (its SHA-256), so a file is handled once.
/// Unsharing the folder removes the inbox; deleting a task keeps the row.
const V13: &str = r#"
CREATE TABLE register_inboxes (
    register_id   TEXT    PRIMARY KEY REFERENCES registers (id) ON DELETE CASCADE,
    folder_id     TEXT    REFERENCES folders (id) ON DELETE SET NULL,
    enabled       INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    revision      INTEGER NOT NULL CHECK (revision >= 1),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE inbox_files (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    register_id     TEXT    NOT NULL REFERENCES registers (id) ON DELETE CASCADE,
    path            TEXT    NOT NULL CHECK (length(path) BETWEEN 1 AND 400),
    sha256          TEXT    NOT NULL CHECK (length(sha256) = 64),
    size_bytes      INTEGER NOT NULL CHECK (size_bytes >= 0),
    status          TEXT    NOT NULL CHECK (status IN ('Waiting', 'Started', 'Failed', 'Skipped')),
    reason          TEXT    CHECK (reason IS NULL OR length(reason) <= 300),
    conversation_id TEXT    REFERENCES conversations (id) ON DELETE SET NULL,
    seen_at_ms      INTEGER NOT NULL,
    started_at_ms   INTEGER,
    UNIQUE (register_id, sha256)
) STRICT;

CREATE INDEX inbox_files_waiting ON inbox_files (register_id, status, id);
CREATE INDEX inbox_files_by_conversation ON inbox_files (conversation_id);
"#;

/// Version 14: an inbox file's task is followed to its end (Done), a task
/// stopped by a temporary provider error is tried again later on its own
/// (attempts, retry_at_ms), and the inbox names the user's CV that each new
/// ad is compared with. The status CHECK list is closed, so inbox_files is
/// rebuilt (no other table refers to it).
const V14: &str = r#"
ALTER TABLE register_inboxes ADD COLUMN cv_folder_id TEXT REFERENCES folders (id) ON DELETE SET NULL;
ALTER TABLE register_inboxes ADD COLUMN cv_path TEXT CHECK (cv_path IS NULL OR length(cv_path) BETWEEN 1 AND 400);

CREATE TABLE inbox_files_v14 (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    register_id     TEXT    NOT NULL REFERENCES registers (id) ON DELETE CASCADE,
    path            TEXT    NOT NULL CHECK (length(path) BETWEEN 1 AND 400),
    sha256          TEXT    NOT NULL CHECK (length(sha256) = 64),
    size_bytes      INTEGER NOT NULL CHECK (size_bytes >= 0),
    status          TEXT    NOT NULL CHECK (status IN ('Waiting', 'Started', 'Done', 'Failed', 'Skipped')),
    reason          TEXT    CHECK (reason IS NULL OR length(reason) <= 300),
    conversation_id TEXT    REFERENCES conversations (id) ON DELETE SET NULL,
    seen_at_ms      INTEGER NOT NULL,
    started_at_ms   INTEGER,
    attempts        INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    retry_at_ms     INTEGER,
    UNIQUE (register_id, sha256)
) STRICT;
INSERT INTO inbox_files_v14
    (id, register_id, path, sha256, size_bytes, status, reason, conversation_id, seen_at_ms, started_at_ms, attempts)
SELECT id, register_id, path, sha256, size_bytes, status, reason, conversation_id, seen_at_ms, started_at_ms,
       CASE WHEN started_at_ms IS NULL THEN 0 ELSE 1 END
FROM inbox_files;
DROP TABLE inbox_files;
ALTER TABLE inbox_files_v14 RENAME TO inbox_files;
CREATE INDEX inbox_files_waiting ON inbox_files (register_id, status, id);
CREATE INDEX inbox_files_by_conversation ON inbox_files (conversation_id);
"#;

/// Version 15 (ADR-TASK-001): one model of the agent's work. A task is a
/// conversation (or a note sent directly, M1, or an effect the agent
/// proposed), with its origin; each request and every reply to it is a run
/// of type Agent, whose state follows its messages; an effect the agent
/// proposed is a child run of that turn. Existing conversations become
/// tasks and their turns become runs, with the state and cause their last
/// reply had; nothing is deleted. Columns are only added (no rebuild).
const V15: &str = r#"
ALTER TABLE tasks ADD COLUMN origin TEXT NOT NULL DEFAULT 'Direct'
    CHECK (origin IN ('Direct', 'User', 'Automation', 'Intake', 'Agent'));
ALTER TABLE tasks ADD COLUMN conversation_id TEXT REFERENCES conversations (id) ON DELETE SET NULL;
ALTER TABLE tasks ADD COLUMN pack TEXT CHECK (pack IS NULL OR length(pack) BETWEEN 1 AND 64);
CREATE UNIQUE INDEX tasks_by_conversation ON tasks (conversation_id) WHERE conversation_id IS NOT NULL;

ALTER TABLE runs ADD COLUMN kind TEXT NOT NULL DEFAULT 'Effect' CHECK (kind IN ('Effect', 'Agent'));
ALTER TABLE runs ADD COLUMN parent_run_id TEXT REFERENCES runs (id) ON DELETE SET NULL;
ALTER TABLE runs ADD COLUMN failure_cause TEXT CHECK (failure_cause IS NULL OR failure_cause IN (
    'ProviderUnavailable', 'QuotaExceeded', 'BudgetExceeded', 'InvalidModelOutput', 'ValidationRefused', 'VerificationMismatch', 'PermissionDenied', 'UserRejected', 'Timeout', 'HostInterrupted', 'OutcomeUnknown', 'Internal'));
CREATE INDEX runs_by_task ON runs (task_id);
CREATE INDEX runs_by_parent ON runs (parent_run_id);

ALTER TABLE messages ADD COLUMN run_id TEXT REFERENCES runs (id);
CREATE INDEX messages_by_run ON messages (run_id);

-- A task for every conversation. Random ids are drawn once per row into a
-- table first, then formatted as UUIDs.
CREATE TEMP TABLE v15_tasks AS
SELECT c.id AS conversation_id,
       lower(hex(randomblob(16))) AS h,
       coalesce(c.project_id, (SELECT id FROM projects ORDER BY created_at_ms, id LIMIT 1)) AS project_id,
       c.created_at_ms AS created_at_ms,
       CASE WHEN EXISTS (SELECT 1 FROM automation_runs a WHERE a.conversation_id = c.id) THEN 'Automation'
            WHEN EXISTS (SELECT 1 FROM inbox_files f WHERE f.conversation_id = c.id) THEN 'Intake'
            ELSE 'User' END AS origin,
       (SELECT r.key FROM inbox_files f JOIN registers r ON r.id = f.register_id
         WHERE f.conversation_id = c.id LIMIT 1) AS pack
FROM conversations c;
INSERT INTO tasks (id, project_id, idempotency_key, request_json, request_sha256, created_at_ms, origin,
                   conversation_id, pack)
SELECT substr(h, 1, 8) || '-' || substr(h, 9, 4) || '-4' || substr(h, 14, 3) || '-a' || substr(h, 18, 3) || '-' || substr(h, 21, 12), project_id, 'agent:' || conversation_id,
       json_object('conversation_id', conversation_id), '', created_at_ms, origin, conversation_id, pack
FROM v15_tasks;
DROP TABLE v15_tasks;

-- A run for every request: its state is the one its last reply had.
CREATE TEMP TABLE v15_turns AS
SELECT u.id AS request_id,
       u.conversation_id AS conversation_id,
       u.created_at_ms AS created_at_ms,
       lower(hex(randomblob(16))) AS h,
       (SELECT m.id FROM messages m
         WHERE m.conversation_id = u.conversation_id AND m.role = 'Model' AND m.seq > u.seq
           AND NOT EXISTS (SELECT 1 FROM messages n WHERE n.conversation_id = u.conversation_id
                             AND n.role = 'User' AND n.seq > u.seq AND n.seq < m.seq)
         ORDER BY m.seq DESC LIMIT 1) AS last_reply
FROM messages u WHERE u.role = 'User';
ALTER TABLE v15_turns ADD COLUMN run_id TEXT;
UPDATE v15_turns SET run_id = substr(h, 1, 8) || '-' || substr(h, 9, 4) || '-4' || substr(h, 14, 3) || '-a' || substr(h, 18, 3) || '-' || substr(h, 21, 12);
INSERT INTO runs (id, task_id, state, revision, error_code, error_message, created_at_ms, updated_at_ms, kind,
                  failure_cause)
SELECT t.run_id, k.id,
       CASE WHEN m.id IS NULL THEN 'Failed'
            WHEN m.status = 'Streaming' THEN 'Running'
            WHEN m.status = 'Complete'
                 AND EXISTS (SELECT 1 FROM paused_turns p WHERE p.message_id = m.id) THEN 'AwaitingApproval'
            WHEN m.status = 'Complete' THEN 'Succeeded'
            WHEN m.status = 'Cancelled' THEN 'Cancelled'
            ELSE 'Failed' END,
       1, m.error_code, m.error_message, t.created_at_ms, coalesce(m.updated_at_ms, t.created_at_ms), 'Agent',
       CASE WHEN m.id IS NULL THEN 'Internal'
            WHEN m.status = 'Failed' THEN CASE m.error_code
             WHEN 'QuotaExceeded' THEN 'QuotaExceeded'
             WHEN 'ProviderUnavailable' THEN 'ProviderUnavailable'
             WHEN 'Busy' THEN 'ProviderUnavailable'
             WHEN 'DeadlineExceeded' THEN 'Timeout'
             WHEN 'BudgetExceeded' THEN 'BudgetExceeded'
             WHEN 'Unauthenticated' THEN 'PermissionDenied'
             WHEN 'Forbidden' THEN 'PermissionDenied'
             WHEN 'InvalidRequest' THEN 'ValidationRefused'
             WHEN 'Interrupted' THEN 'HostInterrupted'
             WHEN 'OutcomeUnknown' THEN 'OutcomeUnknown'
             ELSE 'Internal' END
            ELSE NULL END
FROM v15_turns t
JOIN tasks k ON k.conversation_id = t.conversation_id
LEFT JOIN messages m ON m.id = t.last_reply;
UPDATE messages SET run_id = (SELECT t.run_id FROM v15_turns t WHERE t.request_id = messages.id)
WHERE role = 'User';
UPDATE messages SET run_id = (
    SELECT u.run_id FROM messages u
    WHERE u.conversation_id = messages.conversation_id AND u.role = 'User' AND u.seq < messages.seq
    ORDER BY u.seq DESC LIMIT 1)
WHERE role = 'Model';
DROP TABLE v15_turns;

-- Effects the agent proposed become child runs of their turn.
UPDATE runs SET parent_run_id = (
    SELECT m.run_id FROM messages m WHERE m.proposed_run_id = runs.id AND m.run_id IS NOT NULL LIMIT 1)
WHERE kind = 'Effect'
  AND EXISTS (SELECT 1 FROM messages m WHERE m.proposed_run_id = runs.id AND m.run_id IS NOT NULL);
UPDATE runs SET parent_run_id = (
    SELECT m.run_id FROM paused_turns p JOIN messages m ON m.id = p.message_id
    WHERE p.run_id = runs.id AND m.run_id IS NOT NULL LIMIT 1)
WHERE kind = 'Effect' AND parent_run_id IS NULL
  AND EXISTS (SELECT 1 FROM paused_turns p JOIN messages m ON m.id = p.message_id
              WHERE p.run_id = runs.id AND m.run_id IS NOT NULL);
UPDATE tasks SET origin = 'Agent'
WHERE id IN (SELECT task_id FROM runs WHERE kind = 'Effect' AND parent_run_id IS NOT NULL);
"#;

/// Version 16 (ADR-TASK-001): what the user did about the agent's work,
/// kept as durable facts, each written in the same transaction as the act
/// itself: a proposal approved or rejected, a change undone or corrected, a
/// retry, a stop. `run_id` is the turn it concerns (or the effect run of a
/// note sent directly); `effect_run_id` the effect it was about, if any.
/// From earlier versions, the decisions already recorded are carried over:
/// approvals, undone agent changes and decided memory proposals.
const V16: &str = r#"
CREATE TABLE interventions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id       TEXT    NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    run_id        TEXT    REFERENCES runs (id) ON DELETE SET NULL,
    effect_run_id TEXT    REFERENCES runs (id) ON DELETE SET NULL,
    kind          TEXT    NOT NULL CHECK (kind IN ('Approved', 'Rejected', 'Undo', 'Correction', 'Retry', 'Stopped')),
    detail_json   TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(detail_json)),
    at_ms         INTEGER NOT NULL
) STRICT;
CREATE INDEX interventions_by_run ON interventions (run_id, at_ms);
CREATE INDEX interventions_by_task ON interventions (task_id, at_ms);

INSERT INTO interventions (task_id, run_id, effect_run_id, kind, detail_json, at_ms)
SELECT coalesce(parent.task_id, effect.task_id), coalesce(effect.parent_run_id, effect.id), effect.id,
       CASE a.decision WHEN 'Approved' THEN 'Approved' ELSE 'Rejected' END,
       json_object('from_version', 15), a.decided_at_ms
FROM approvals a
JOIN actions ac ON ac.id = a.action_id
JOIN runs effect ON effect.id = ac.run_id
LEFT JOIN runs parent ON parent.id = effect.parent_run_id
WHERE a.decision IS NOT NULL;

INSERT INTO interventions (task_id, run_id, kind, detail_json, at_ms)
SELECT k.id, m.run_id, 'Undo', json_object('record_change', rc.id, 'from_version', 15), rc.undone_at_ms
FROM record_changes rc
JOIN tasks k ON k.conversation_id = rc.conversation_id
LEFT JOIN messages m ON m.id = rc.message_id
WHERE rc.actor = 'Agent' AND rc.undone_at_ms IS NOT NULL;

INSERT INTO interventions (task_id, run_id, kind, detail_json, at_ms)
SELECT k.id, m.run_id, CASE mi.status WHEN 'Rejected' THEN 'Rejected' ELSE 'Approved' END,
       json_object('memory', mi.id, 'from_version', 15), mi.decided_at_ms
FROM memory_items mi
JOIN tasks k ON k.conversation_id = mi.conversation_id
LEFT JOIN messages m ON m.id = mi.message_id
WHERE mi.source = 'Agent' AND mi.decided_at_ms IS NOT NULL;
"#;

/// Version 17 (ADR-ACTION-001): every action names its effect class and
/// the evidence its success has, and a compensation (an undo) names the
/// action it reverses. Existing actions are classed by their tool. The
/// agent's register writes and proposed facts become actions of their turn
/// from now on (earlier ones stay in their own history).
const V17: &str = r#"
ALTER TABLE actions ADD COLUMN effect_class TEXT CHECK (effect_class IS NULL OR effect_class IN (
    'Read', 'InternalReversible', 'ExternalReversible', 'ExternalIrreversible'));
ALTER TABLE actions ADD COLUMN evidence TEXT CHECK (evidence IS NULL OR evidence IN (
    'Verified', 'Reported', 'Unverifiable'));
ALTER TABLE actions ADD COLUMN compensates_action_id TEXT REFERENCES actions (id);
CREATE INDEX actions_by_run ON actions (run_id);
CREATE INDEX actions_by_compensated ON actions (compensates_action_id);
UPDATE actions SET
    effect_class = CASE tool_id
        WHEN 'CreateMarkdownArtifact' THEN 'InternalReversible'
        WHEN 'CreateDocument' THEN 'ExternalReversible'
        WHEN 'McpToolCall' THEN 'ExternalIrreversible' END,
    evidence = CASE tool_id
        WHEN 'McpToolCall' THEN 'Reported'
        ELSE 'Verified' END;
"#;

/// Version 18 (ADR-IDENTITY-001): who does what, by a stable id. The actors
/// are the local user, the agent, the Host, the inbox and every automation
/// (a channel gets one when it is bound, M4). An actor is never deleted,
/// only disabled, so old entries still name it. A task names who asked for
/// it, an action who did it, an approval and an intervention who decided.
/// The `User`/`Agent` words already in records and memory stay: they name
/// the user or the agent, and who asked for the agent's work is its turn's
/// task. Existing rows are filled in from what they already say; a task
/// started by an automation since deleted keeps no requester.
const V18: &str = r#"
CREATE TABLE actors (
    id             TEXT    PRIMARY KEY,
    kind           TEXT    NOT NULL CHECK (kind IN ('User', 'Agent', 'Automation', 'Intake', 'Channel', 'System')),
    -- What the actor is, for the Host to find it: `user:local`, `agent`,
    -- `host`, `intake:inbox`, `automation:<id>`.
    ref            TEXT    NOT NULL UNIQUE CHECK (length(ref) BETWEEN 1 AND 128),
    name           TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    created_at_ms  INTEGER NOT NULL,
    disabled_at_ms INTEGER
) STRICT;

CREATE TEMP TABLE v18_actors AS
SELECT kind, ref, name, created_at_ms, lower(hex(randomblob(16))) AS h FROM (
    SELECT 'User' AS kind, 'user:local' AS ref, 'Local user' AS name,
           CAST(strftime('%s', 'now') AS INTEGER) * 1000 AS created_at_ms
    UNION ALL SELECT 'Agent', 'agent', 'Nexus', CAST(strftime('%s', 'now') AS INTEGER) * 1000
    UNION ALL SELECT 'System', 'host', 'Nexus Host', CAST(strftime('%s', 'now') AS INTEGER) * 1000
    UNION ALL SELECT 'Intake', 'intake:inbox', 'Inbox', CAST(strftime('%s', 'now') AS INTEGER) * 1000
    UNION ALL SELECT 'Automation', 'automation:' || id, name, created_at_ms FROM automations);
INSERT INTO actors (id, kind, ref, name, created_at_ms)
SELECT substr(h, 1, 8) || '-' || substr(h, 9, 4) || '-4' || substr(h, 14, 3) || '-a' || substr(h, 18, 3) || '-' || substr(h, 21, 12),
       kind, ref, name, created_at_ms
FROM v18_actors;
DROP TABLE v18_actors;

ALTER TABLE tasks ADD COLUMN requested_by TEXT REFERENCES actors (id);
ALTER TABLE actions ADD COLUMN actor_id TEXT REFERENCES actors (id);
ALTER TABLE approvals ADD COLUMN decided_by TEXT REFERENCES actors (id);
ALTER TABLE interventions ADD COLUMN actor_id TEXT REFERENCES actors (id);

UPDATE tasks SET requested_by = CASE origin
    WHEN 'Agent' THEN (SELECT id FROM actors WHERE ref = 'agent')
    WHEN 'Intake' THEN (SELECT id FROM actors WHERE ref = 'intake:inbox')
    WHEN 'Automation' THEN (
        SELECT a.id FROM automation_runs ar JOIN actors a ON a.ref = 'automation:' || ar.automation_id
        WHERE ar.conversation_id = tasks.conversation_id LIMIT 1)
    ELSE (SELECT id FROM actors WHERE ref = 'user:local') END;
-- The user undoes, and sends a note directly; everything else the agent did.
UPDATE actions SET actor_id = CASE
    WHEN compensates_action_id IS NOT NULL
         OR (SELECT t.origin FROM runs r JOIN tasks t ON t.id = r.task_id WHERE r.id = actions.run_id) = 'Direct'
        THEN (SELECT id FROM actors WHERE ref = 'user:local')
    ELSE (SELECT id FROM actors WHERE ref = 'agent') END;
UPDATE approvals SET decided_by = (SELECT id FROM actors WHERE ref = 'user:local')
WHERE decision IS NOT NULL;
UPDATE interventions SET actor_id = (SELECT id FROM actors WHERE ref = 'user:local');
"#;

/// Version 19 (ADR-POLICY-001): an action records what the approval policy
/// decided for it (`Auto`, `Ask` or `Deny`) and under which revision; the
/// user's own undo is their act, not the policy's, and records none. A
/// request for approval waits for the decision (`expires_at_ms`) longer than
/// an approval given may wait to be used: `use_by_ms`, set at the decision
/// from `grant_ms`. Existing actions get the decision their path implied
/// (asked when they had an approval or were a proposed fact, carried out
/// for a register write); approvals already decided keep their old limit.
const V19: &str = r#"
ALTER TABLE actions ADD COLUMN policy_decision TEXT CHECK (policy_decision IS NULL OR policy_decision IN ('Auto', 'Ask', 'Deny'));
ALTER TABLE actions ADD COLUMN policy_revision INTEGER CHECK (policy_revision IS NULL OR policy_revision >= 1);
ALTER TABLE approvals ADD COLUMN grant_ms INTEGER CHECK (grant_ms IS NULL OR grant_ms > 0);
ALTER TABLE approvals ADD COLUMN use_by_ms INTEGER;

UPDATE actions SET policy_decision = 'Ask'
WHERE compensates_action_id IS NULL
  AND (tool_id = 'MemoryFact' OR EXISTS (SELECT 1 FROM approvals a WHERE a.action_id = actions.id));
UPDATE actions SET policy_decision = 'Auto'
WHERE compensates_action_id IS NULL AND tool_id = 'RecordWrite';
UPDATE actions SET policy_revision = 1 WHERE policy_decision IS NOT NULL;
UPDATE approvals SET use_by_ms = expires_at_ms WHERE decision IS NOT NULL;
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
    Migration {
        sql: V11,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V12,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V13,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V14,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V15,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V16,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V17,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V18,
        rebuilds_parent_table: false,
    },
    Migration {
        sql: V19,
        rebuilds_parent_table: false,
    },
];

pub fn migrate(conn: &mut Connection) -> Result<(), StorageError> {
    migrate_until(conn, SUPPORTED_VERSION)
}

/// Applies the migrations up to `target` (tests build an older schema with
/// it, to prove a migration on the shape it really starts from).
pub(crate) fn migrate_until(conn: &mut Connection, target: i64) -> Result<(), StorageError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > SUPPORTED_VERSION {
        return Err(StorageError::SchemaTooNew {
            found: current,
            supported: SUPPORTED_VERSION,
        });
    }
    for (index, migration) in MIGRATIONS
        .iter()
        .enumerate()
        .skip(current as usize)
        .take(target.saturating_sub(current).max(0) as usize)
    {
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
