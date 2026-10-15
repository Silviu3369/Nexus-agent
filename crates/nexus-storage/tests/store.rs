//! Real SQLite files in a temp directory — no mocked database, since a mock
//! cannot show durability or constraint behaviour (`SECURITATE_SI_VALIDARE_RO.md` §5).

use std::path::PathBuf;

use nexus_core::{ActionState, OperationState, RunState};
use nexus_storage::{SUPPORTED_VERSION, StorageError, Store};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("nexus-storage-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn db(&self) -> PathBuf {
        self.0.join("nexus.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn seed_task(store: &Store) {
    store
        .call(|conn| {
            conn.execute_batch(
                "INSERT INTO projects VALUES ('p1', 'Nexus', 1, 0);
                 INSERT INTO tasks VALUES ('t1', 'p1', 'key-1', '{}', 'h', 0);",
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn fresh_database_is_migrated_with_wal_and_foreign_keys() {
    let dir = TempDir::new();
    let store = Store::open(&dir.db()).unwrap();

    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let (mode, fk) = store
        .call(|conn| {
            let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
            let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
            Ok((mode, fk))
        })
        .await
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
    assert_eq!(fk, 1);
}

#[tokio::test]
async fn data_survives_closing_and_reopening() {
    let dir = TempDir::new();
    {
        let store = Store::open(&dir.db()).unwrap();
        seed_task(&store).await;
    } // dropped: storage thread joined, connection closed

    let store = Store::open(&dir.db()).unwrap();
    let count: i64 = store
        .call(|conn| Ok(conn.query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
}

#[tokio::test]
async fn newer_schema_is_refused_and_left_untouched() {
    let dir = TempDir::new();
    {
        let conn = rusqlite::Connection::open(dir.db()).unwrap();
        conn.execute_batch("CREATE TABLE future (x); PRAGMA user_version = 99;")
            .unwrap();
    }

    match Store::open(&dir.db()) {
        Err(StorageError::SchemaTooNew { found, supported }) => {
            assert_eq!(found, 99);
            assert_eq!(supported, SUPPORTED_VERSION);
        }
        Err(other) => panic!("expected SchemaTooNew, got {other}"),
        Ok(_) => panic!("an older host must not open a newer schema"),
    }

    let conn = rusqlite::Connection::open(dir.db()).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let future_table: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'future'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        (version, future_table),
        (99, 1),
        "refused database must not be modified"
    );
}

#[tokio::test]
async fn foreign_keys_are_enforced() {
    let dir = TempDir::new();
    let store = Store::open(&dir.db()).unwrap();

    let result = store
        .call(|conn| {
            conn.execute(
                "INSERT INTO tasks VALUES ('t1', 'no-such-project', 'k', '{}', 'h', 0)",
                [],
            )?;
            Ok(())
        })
        .await;
    assert!(
        result.is_err(),
        "a task must not reference a missing project"
    );
}

#[tokio::test]
async fn state_columns_accept_exactly_the_domain_states() {
    let dir = TempDir::new();
    let store = Store::open(&dir.db()).unwrap();
    seed_task(&store).await;

    let rejected = store
        .call(|conn| {
            let mut rejected = Vec::new();

            for (i, state) in RunState::ALL.iter().enumerate() {
                conn.execute(
                    "INSERT INTO runs VALUES (?1, 't1', ?2, 1, NULL, NULL, 0, 0)",
                    (format!("r{i}"), state.as_str()),
                )?;
            }
            if conn
                .execute("INSERT INTO runs VALUES ('rx', 't1', 'Bogus', 1, NULL, NULL, 0, 0)", [])
                .is_err()
            {
                rejected.push("run");
            }

            for (i, state) in ActionState::ALL.iter().enumerate() {
                conn.execute(
                    "INSERT INTO actions VALUES (?1, 'r0', ?2, 'tool', 1, ?3, '{}', 'h', 1, 0, 0)",
                    (format!("a{i}"), format!("op{i}"), state.as_str()),
                )?;
            }
            if conn
                .execute(
                    "INSERT INTO actions VALUES ('ax', 'r0', 'opx', 'tool', 1, 'Approved=true', '{}', 'h', 1, 0, 0)",
                    [],
                )
                .is_err()
            {
                rejected.push("action");
            }

            for (i, state) in OperationState::ALL.iter().enumerate() {
                conn.execute(
                    "INSERT INTO operations VALUES (?1, ?2, ?3, 1, ?4, ?5, 'h', 0, 1, 0, 0)",
                    (
                        format!("op{i}"),
                        format!("a{i}"),
                        state.as_str(),
                        format!("t{i}.md"),
                        format!("s{i}.tmp"),
                    ),
                )?;
            }
            if conn
                .execute(
                    "INSERT INTO operations VALUES ('opx', 'a9', 'Done', 1, 'tx.md', 'sx.tmp', 'h', 0, 1, 0, 0)",
                    [],
                )
                .is_err()
            {
                rejected.push("operation");
            }
            Ok(rejected)
        })
        .await
        .expect("every domain state must be accepted by the schema");

    assert_eq!(
        rejected,
        ["run", "action", "operation"],
        "unknown states must be refused"
    );
}

#[tokio::test]
async fn failed_event_write_rolls_back_the_state_change() {
    let dir = TempDir::new();
    let store = Store::open(&dir.db()).unwrap();

    let result = store
        .call(|conn| {
            let tx = conn.transaction()?;
            tx.execute("INSERT INTO projects VALUES ('p1', 'Nexus', 1, 0)", [])?;
            // Invalid JSON violates the events CHECK, so the transaction
            // must fail as a whole.
            tx.execute(
                "INSERT INTO events (id, kind, entity_id, entity_revision, payload_json, created_at_ms)
                 VALUES ('e1', 'ProjectCreated', 'p1', 1, 'not json', 0)",
                [],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await;
    assert!(result.is_err());

    let projects: i64 = store
        .call(|conn| Ok(conn.query_row("SELECT count(*) FROM projects", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(projects, 0, "state must not persist without its event");
}

#[tokio::test]
async fn approval_cannot_be_consumed_without_an_approved_decision() {
    let dir = TempDir::new();
    let store = Store::open(&dir.db()).unwrap();
    seed_task(&store).await;

    let result = store
        .call(|conn| {
            conn.execute_batch(
                "INSERT INTO runs VALUES ('r1', 't1', 'AwaitingApproval', 1, NULL, NULL, 0, 0);
                 INSERT INTO actions VALUES ('a1', 'r1', 'op1', 'tool', 1, 'AwaitingApproval', '{}', 'h', 1, 0, 0);",
            )?;
            conn.execute(
                "INSERT INTO approvals VALUES ('ap1', 'a1', 'h', 0, 'Denied', 5, 6, NULL)",
                [],
            )?;
            Ok(())
        })
        .await;
    assert!(
        result.is_err(),
        "a denied approval must not be marked consumed"
    );
}

#[test]
fn network_share_path_is_refused() {
    match Store::open(std::path::Path::new(r"\\fileserver\share\nexus.db")) {
        Err(StorageError::NetworkPath(_)) => {}
        Err(other) => panic!("expected NetworkPath, got {other}"),
        Ok(_) => panic!("WAL must not be used on a network share"),
    }
}
