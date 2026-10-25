//! What data may leave for which provider (`schema::V22`, ADR-PROVIDER-002)
//! on a real SQLite file: every source is the user's own data until marked
//! as someone else's; what the user says about a connection and where they
//! use Nexus from are kept and read back; the migration starts from the
//! defaults.

mod common;

use std::path::PathBuf;

use nexus_core::DataClass;
use nexus_storage::{ConnectionData, SUPPORTED_VERSION, Source, Store};

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
    let dir = std::env::temp_dir().join(format!("nexus-data-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

#[tokio::test]
async fn sources_are_the_users_own_until_marked_and_settings_are_kept() {
    let env = env();
    let s = &env.store;
    let folder = s
        .add_folder("Clients".into(), "C:\\Clients".into(), false, 1)
        .await
        .unwrap();
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
        .unwrap();
    let connection = s
        .save_connection(None, "OpenAI", "o".into(), false, "openai-d".into(), 1)
        .await
        .unwrap();

    let defaults = s.data_settings().await.unwrap();
    assert_eq!(
        defaults.class_of(Source::Folder(folder.id)),
        DataClass::Personal
    );
    assert_eq!(
        defaults.class_of(Source::Register(register.id)),
        DataClass::Personal
    );
    assert_eq!(
        defaults.connection(connection.id),
        ConnectionData::default()
    );
    assert!(!defaults.in_eea, "not assumed: the user says it");

    s.set_source_class(Source::Folder(folder.id), DataClass::Business, 2)
        .await
        .unwrap();
    s.set_connection_data(
        connection.id,
        ConnectionData {
            dpa_signed: true,
            allows_training: false,
        },
        3,
    )
    .await
    .unwrap();
    s.set_in_eea(true, 4).await.unwrap();
    let set = s.data_settings().await.unwrap();
    assert_eq!(set.class_of(Source::Folder(folder.id)), DataClass::Business);
    assert_eq!(
        set.class_of(Source::Register(register.id)),
        DataClass::Personal
    );
    assert!(set.connection(connection.id).dpa_signed);
    assert!(set.in_eea);
    s.set_in_eea(false, 5).await.unwrap();
    assert!(!s.data_settings().await.unwrap().in_eea);

    assert!(
        s.set_source_class(
            Source::Folder(nexus_protocol::FolderId::new()),
            DataClass::Business,
            6
        )
        .await
        .is_err(),
        "an unknown source is refused"
    );
}

#[tokio::test]
async fn migrating_to_v22_starts_from_the_defaults() {
    let env = env();
    let path = env.dir.join("nexus.db");
    let folder = env
        .store
        .add_folder("Mine".into(), "C:\\Mine".into(), false, 1)
        .await
        .unwrap();
    env.store
        .call(|c| {
            common::undo_v22(c)?;
            Ok(())
        })
        .await
        .unwrap();
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let settings = s.data_settings().await.unwrap();
    assert_eq!(
        settings.class_of(Source::Folder(folder.id)),
        DataClass::Personal
    );
    assert!(!settings.in_eea);
}
