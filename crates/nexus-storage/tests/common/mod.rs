//! Support shared by the storage tests.

/// Takes a database from the current schema back to version 14 (version
/// 16 only added the interventions table, version 17 columns of actions),
/// so an older
/// migration test can go on undoing to the version it starts from. Version
/// 15 (ADR-TASK-001) only added columns, to tables other tables refer to, so
/// those tables are rebuilt with their exact version-14 definition, read
/// from a database created at version 14. The agent's tasks and runs, which
/// only version 15 has, are dropped; M1's stay.
#[allow(dead_code)]
pub fn undo_v15(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let v14 = v14_tables();
    conn.execute_batch(
        "DROP TABLE IF EXISTS interventions;
         DELETE FROM actions WHERE compensates_action_id IS NOT NULL
             OR run_id IN (SELECT id FROM runs WHERE kind = 'Agent');
         DROP INDEX IF EXISTS actions_by_run; DROP INDEX IF EXISTS actions_by_compensated;
         ALTER TABLE actions DROP COLUMN compensates_action_id;
         ALTER TABLE actions DROP COLUMN effect_class;
         ALTER TABLE actions DROP COLUMN evidence;",
    )?;
    conn.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;")?;
    conn.execute_batch(
        "DROP INDEX IF EXISTS tasks_by_conversation; DROP INDEX IF EXISTS runs_by_task;
         DROP INDEX IF EXISTS runs_by_parent; DROP INDEX IF EXISTS messages_by_run;
         DELETE FROM runs WHERE kind = 'Agent';
         DELETE FROM tasks WHERE conversation_id IS NOT NULL;",
    )?;
    for table in &v14 {
        conn.execute_batch(&format!(
            "ALTER TABLE {name} RENAME TO {name}_v15;
             {ddl};
             INSERT INTO {name} ({columns}) SELECT {columns} FROM {name}_v15;
             DROP TABLE {name}_v15;",
            name = table.name,
            ddl = table.ddl,
            columns = table.columns,
        ))?;
        for index in &table.indexes {
            conn.execute_batch(index)?;
        }
    }
    conn.execute_batch("PRAGMA legacy_alter_table = OFF; PRAGMA foreign_keys = ON;")?;
    Ok(())
}

struct Table {
    name: &'static str,
    ddl: String,
    columns: String,
    indexes: Vec<String>,
}

/// How a database at version 14 defines the tables version 15 changed.
fn v14_tables() -> Vec<Table> {
    let dir = std::env::temp_dir().join(format!("nexus-v14-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("nexus.db");
    nexus_storage::create_at_version(&path, 14).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let tables = ["tasks", "runs", "messages"]
        .into_iter()
        .map(|name| {
            let ddl: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .unwrap();
            let columns: Vec<String> = conn
                .prepare(&format!("PRAGMA table_info({name})"))
                .unwrap()
                .query_map([], |r| r.get(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            let indexes: Vec<String> = conn
                .prepare(
                    "SELECT sql FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL",
                )
                .unwrap()
                .query_map([name], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            Table {
                name,
                ddl,
                columns: columns.join(", "),
                indexes,
            }
        })
        .collect();
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
    tables
}
