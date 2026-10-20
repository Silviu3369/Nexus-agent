//! A register's inbox (`schema::V13`, `V14`) on a real SQLite file: one row
//! per file content, a stale change refused, files handled in the order
//! seen, only a failed file goes back in line, a file's task is settled and
//! a retry waits for its time, unsharing the folder removes the inbox, and
//! the migrations add the tables and keep every existing row.

mod common;

use std::path::PathBuf;

use nexus_storage::{InboxSettle, SUPPORTED_VERSION, StorageError, Store};

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
        .save_inbox(register, 0, Some(folder.id), true, None, None, 2)
        .await
        .unwrap();
    assert_eq!((on.revision, on.enabled), (1, true));
    assert!(matches!(
        s.save_inbox(register, 0, Some(folder.id), false, None, None, 3)
            .await,
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

    let first = s.next_inbox_file(register, 100).await.unwrap().unwrap();
    assert_eq!(first.path, "a.pdf");
    s.inbox_outcome(first.id, None, Some("no model".into()), 8)
        .await
        .unwrap();
    let second = s.next_inbox_file(register, 100).await.unwrap().unwrap();
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
        s.next_inbox_file(register, 100)
            .await
            .unwrap()
            .unwrap()
            .path,
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
        common::undo_v15(c)?;
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

#[tokio::test]
async fn a_task_that_ended_is_settled_and_a_retry_waits_for_its_time() {
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
    let cv = s
        .add_folder("Jobs".into(), r"C:\Jobs".into(), false, 1)
        .await
        .unwrap();
    let ads = s
        .add_folder("Ads".into(), r"C:\Ads".into(), false, 1)
        .await
        .unwrap();
    let inbox = s
        .save_inbox(
            register,
            0,
            Some(ads.id),
            true,
            Some(cv.id),
            Some("CV.pdf".into()),
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        (inbox.cv_folder_id, inbox.cv_path.as_deref()),
        (Some(cv.id), Some("CV.pdf"))
    );

    s.inbox_seen(register, "a.pdf".into(), sha(1), 1, None, 3)
        .await
        .unwrap();
    let file = s.next_inbox_file(register, 3).await.unwrap().unwrap();
    assert_eq!(file.attempts, 0);
    // Started (without a task row here), then tried again later.
    s.inbox_outcome(file.id, None, Some("x".into()), 4)
        .await
        .unwrap();
    s.retry_inbox_file(register, file.id, 5).await.unwrap();
    s.settle_inbox_file(
        file.id,
        InboxSettle::RetryAt {
            at: 1_000,
            reason: "tried again at 10:00".into(),
        },
        6,
    )
    .await
    .unwrap();
    assert!(
        s.next_inbox_file(register, 999).await.unwrap().is_none(),
        "not before its time"
    );
    let due = s.next_inbox_file(register, 1_000).await.unwrap().unwrap();
    assert_eq!(due.reason.as_deref(), Some("tried again at 10:00"));
    s.settle_inbox_file(due.id, InboxSettle::Failed("gave up".into()), 7)
        .await
        .unwrap();
    assert_eq!(
        s.inbox_files(register, 5).await.unwrap()[0].status,
        "Failed"
    );
    assert!(
        s.settle_inbox_file(due.id, InboxSettle::Done, 8)
            .await
            .is_err(),
        "only a started or waiting file settles"
    );

    s.remove_folder(cv.id, 9).await.unwrap();
    let after = s.inbox(register).await.unwrap();
    assert_eq!(
        after.cv_folder_id, None,
        "unsharing the CV's folder forgets the CV's place"
    );
}

#[tokio::test]
async fn migrating_to_v14_keeps_every_inbox_file_and_counts_the_started_ones() {
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
    let ads = s
        .add_folder("Ads".into(), r"C:\Ads".into(), false, 1)
        .await
        .unwrap();
    // The tables as version 13 had them, with the rows a v13 inbox leaves.
    let (reg, folder) = (register.to_string(), ads.id.to_string());
    s.call(move |c| {
        common::undo_v15(c)?;
        c.execute_batch(
            "DROP TABLE inbox_files; DROP TABLE register_inboxes;
             CREATE TABLE register_inboxes (
                 register_id TEXT PRIMARY KEY REFERENCES registers (id) ON DELETE CASCADE,
                 folder_id TEXT REFERENCES folders (id) ON DELETE SET NULL,
                 enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
                 revision INTEGER NOT NULL CHECK (revision >= 1),
                 created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL) STRICT;
             CREATE TABLE inbox_files (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 register_id TEXT NOT NULL REFERENCES registers (id) ON DELETE CASCADE,
                 path TEXT NOT NULL CHECK (length(path) BETWEEN 1 AND 400),
                 sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
                 size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
                 status TEXT NOT NULL CHECK (status IN ('Waiting', 'Started', 'Failed', 'Skipped')),
                 reason TEXT CHECK (reason IS NULL OR length(reason) <= 300),
                 conversation_id TEXT REFERENCES conversations (id) ON DELETE SET NULL,
                 seen_at_ms INTEGER NOT NULL, started_at_ms INTEGER,
                 UNIQUE (register_id, sha256)) STRICT;
             CREATE INDEX inbox_files_waiting ON inbox_files (register_id, status, id);
             CREATE INDEX inbox_files_by_conversation ON inbox_files (conversation_id);",
        )?;
        c.execute(
            "INSERT INTO register_inboxes VALUES (?1, ?2, 1, 1, 1, 1)",
            [&reg, &folder],
        )?;
        for (path, n, status, reason, started) in [
            ("a.pdf", 1, "Started", None, Some(5)),
            ("b.txt", 2, "Failed", Some("the reply failed"), Some(6)),
            ("c.pdf", 3, "Waiting", None, None),
            ("d.docx", 4, "Skipped", Some("a file Nexus wrote itself"), None),
        ] {
            c.execute(
                "INSERT INTO inbox_files (register_id, path, sha256, size_bytes, status, reason, seen_at_ms, started_at_ms)
                 VALUES (?1, ?2, ?3, 10, ?4, ?5, 2, ?6)",
                rusqlite::params![reg, path, sha(n), status, reason, started],
            )?;
        }
        c.execute_batch("PRAGMA user_version = 13;")?;
        Ok(())
    })
    .await
    .unwrap();

    let reopened = Store::open(&env.dir.join("nexus.db")).unwrap();
    assert_eq!(reopened.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let inbox = reopened.inbox(register).await.unwrap();
    assert_eq!(
        (
            inbox.folder_id,
            inbox.enabled,
            inbox.revision,
            inbox.cv_folder_id,
            inbox.cv_path
        ),
        (Some(ads.id), true, 1, None, None)
    );
    let mut files: Vec<_> = reopened
        .inbox_files(register, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|f| (f.path, f.status, f.reason, f.attempts, f.retry_at_ms))
        .collect();
    files.sort();
    assert_eq!(
        files,
        vec![
            ("a.pdf".into(), "Started".into(), None, 1, None),
            (
                "b.txt".into(),
                "Failed".into(),
                Some("the reply failed".into()),
                1,
                None
            ),
            ("c.pdf".into(), "Waiting".into(), None, 0, None),
            (
                "d.docx".into(),
                "Skipped".into(),
                Some("a file Nexus wrote itself".into()),
                0,
                None
            ),
        ]
    );
    assert_eq!(
        reopened
            .next_inbox_file(register, 3)
            .await
            .unwrap()
            .unwrap()
            .path,
        "c.pdf"
    );
}
