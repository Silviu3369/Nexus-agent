//! DATA-01 for the provider key with the real `nexus-host` binary: the key
//! reaches the provider only in its header, and never the log, the
//! database, the events, a response or a backup.
//!
//! A test binary of its own: a second process writing to Credential Manager
//! at the same time as the in-process chat tests can bring deleted test
//! entries back (see `vault.rs`), so this one never runs alongside them.

mod support;

use std::path::Path;

use support::chat::{files_under, gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};
use support::{GEMINI_BASE_URL_ENV, TempProfile, connect, spawn_host_env, wait_exit};

const KEY: &str = "SENTINEL-gemini-key-binary-91d4";

#[tokio::test]
async fn data01_provider_key_never_leaks() {
    let fake = FakeGemini::start(KEY).await;
    let profile = TempProfile::new("provider-secret");
    let mut host = spawn_host_env(
        profile.path(),
        None,
        &[(GEMINI_BASE_URL_ENV, fake.base_url.as_str())],
    );
    let client = connect(profile.path()).await;
    let mut seen: Vec<String> = Vec::new();

    let wrong = format!("WRONG-{}", uuid::Uuid::new_v4());
    let refused = client
        .save_gemini_connection(gemini(&wrong))
        .await
        .unwrap_err();
    seen.push(format!("{refused} {refused:?}"));

    let saved = client.save_gemini_connection(gemini(KEY)).await.unwrap();
    seen.push(serde_json::to_string(&saved).unwrap());
    let models = client.list_models(saved.connection_id).await.unwrap();
    seen.push(serde_json::to_string(&models).unwrap());
    let selected = client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
    seen.push(serde_json::to_string(&selected).unwrap());

    fake.push(Reply::text(&["Bună."]));
    fake.push(Reply::status(
        401,
        &format!(r#"{{"error":{{"message":"key {KEY} revoked"}}}}"#),
    ));
    let ok = send(&client, None, "k1", "Salut").await;
    seen.push(serde_json::to_string(&settled(&client, &ok).await).unwrap());
    let failed = send(&client, Some(ok.conversation_id), "k2", "Din nou").await;
    seen.push(serde_json::to_string(&settled(&client, &failed).await).unwrap());
    let conversation = client.conversation(ok.conversation_id).await.unwrap();
    seen.push(serde_json::to_string(&conversation).unwrap());
    seen.push(serde_json::to_string(&client.list_connections().await.unwrap()).unwrap());
    seen.push(serde_json::to_string(&client.events(0, 500).await.unwrap()).unwrap());
    seen.push(serde_json::to_string(&client.snapshot(50).await.unwrap()).unwrap());
    let backup = client.create_backup().await.unwrap();

    client.stop_host().await.unwrap();
    drop(client);
    assert!(wait_exit(&mut host).success());

    let log = std::fs::read_to_string(profile.path().join("logs").join("nexus-host.log")).unwrap();
    assert!(
        log.contains("provider connection saved"),
        "the log records the activity"
    );
    seen.push(log);
    for text in &seen {
        assert!(!text.contains(KEY), "the provider key leaked into: {text}");
        assert!(
            !text.contains(&wrong),
            "the refused key leaked into: {text}"
        );
    }

    // Every file the host wrote: database (and WAL), log, artifacts, and the
    // backup (kept inside the profile).
    assert!(Path::new(&backup.path).starts_with(profile.path()));
    for file in files_under(profile.path()) {
        let bytes = std::fs::read(&file).unwrap();
        for secret in [KEY, wrong.as_str()] {
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "a key is inside {}",
                file.display()
            );
        }
    }

    // What the provider received: the key only in its header.
    for request in fake.received() {
        assert!(!request.target.contains(KEY) && !request.body.contains(KEY));
    }
    assert_eq!(fake.generations().len(), 2);
    assert!(
        fake.generations()
            .iter()
            .all(|r| r.header("x-goog-api-key") == Some(KEY))
    );
}
