//! A register's inbox (`schema::V13`) on a real SQLite file: one row per
//! file content, a stale change refused, files handled in the order seen,
//! only a failed file goes back in line, unsharing the folder removes the
//! inbox, and the migration adds the tables without touching anything else.

use std::path::PathBuf;

use nexus_storage::{SUPPORTED_VERSION, StorageError, Store};

struct Env {
    store: Store,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("nexus-inbox-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

fn sha(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

#[tokio::test]
async fn files_are_seen_once_handled_in_order_and_only_a_failure_is_retried() {
    let env = env();
    let s = &env.store;
    let register = s
        .ensure_register(
            "job_applications".into(),
            "Jobs".into(),
            "{}".into(),
            "status".into(),
            None,
            1,
        )
        .await
        .unwrap()
        .id;
    let folder = s
        .add_folder("Ads".into(), r"C:\Ads".into(), false, 1)
        .await
        .unwrap();
    let off = s.inbox(register).await.unwrap();
    assert_eq!((off.revision, off.enabled, off.folder_id), (0, false, None));
    let on = s
        .save_inbox(register, 0, Some(folder.id), true, 2)
        .await
        .unwrap();
    assert_eq!((on.revision, on.enabled), (1, true));
    assert!(matches!(
        s.save_inbox(register, 0, Some(folder.id), false, 3).await,
        Err(StorageError::RevisionConflict(_))
    ));
    assert_eq!(s.active_inboxes().await.unwrap().len(), 1);

    assert!(
        s.inbox_seen(register, "a.pdf".into(), sha(1), 10, None, 4)
            .await
            .unwrap()
    );
    assert!(
        !s.inbox_seen(register, "copy of a.pdf".into(), sha(1), 10, None, 5)
            .await
            .unwrap(),
        "same content"
    );
    assert!(
        s.inbox_seen(register, "b.pdf".into(), sha(2), 10, None, 6)
            .await
            .unwrap()
    );
    assert!(
        s.inbox_seen(
            register,
            "letter.docx".into(),
            sha(3),
            10,
            Some("a file Nexus wrote itself".into()),
            7
        )
        .await
        .unwrap()
    );
    assert_eq!(s.inbox_waiting(register).await.unwrap(), 2);

    let first = s.next_inbox_file(register).await.unwrap().unwrap();
    assert_eq!(first.path, "a.pdf");
    s.inbox_outcome(first.id, None, Some("no model".into()), 8)
        .await
        .unwrap();
    let second = s.next_inbox_file(register).await.unwrap().unwrap();
    assert_eq!(second.path, "b.pdf");
    assert!(
        matches!(
            s.retry_inbox_file(register, second.id, 9).await,
            Err(StorageError::NotFound(_))
        ),
        "only a failed file is put back in line"
    );
    s.retry_inbox_file(register, first.id, 10).await.unwrap();
    assert_eq!(
        s.next_inbox_file(register).await.unwrap().unwrap().path,
        "a.pdf"
    );
    let files = s.inbox_files(register, 10).await.unwrap();
    let statuses: Vec<(&str, &str)> = files
        .iter()
        .map(|f| (f.path.as_str(), f.status.as_str()))
        .collect();
    assert_eq!(
        statuses,
        [
            ("letter.docx", "Skipped"),
            ("b.pdf", "Waiting"),
            ("a.pdf", "Waiting")
        ]
    );
    assert_eq!(s.inbox_known(register).await.unwrap().len(), 3);

    s.remove_folder(folder.id, 11).await.unwrap();
    let after = s.inbox(register).await.unwrap();
    assert_eq!(
        after.folder_id, None,
        "unsharing the folder takes it out of the inbox"
    );
    assert!(s.active_inboxes().await.unwrap().is_empty());
}

#[tokio::test]
async fn migrating_to_v13_adds_the_inbox_and_keeps_the_rest() {
    let env = env();
    let s = &env.store;
    s.add_folder("Ads".into(), r"C:\Ads".into(), true, 1)
        .await
        .unwrap();
    s.call(|c| {
        c.execute_batch(
            "DROP TABLE inbox_files; DROP TABLE register_inboxes; PRAGMA user_version = 12;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let reopened = Store::open(&env.dir.join("nexus.db")).unwrap();
    assert_eq!(reopened.schema_version().await.unwrap(), SUPPORTED_VERSION);
    assert_eq!(reopened.folders().await.unwrap().len(), 1);
    assert!(reopened.active_inboxes().await.unwrap().is_empty());
}
