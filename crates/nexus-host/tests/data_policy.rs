//! What data may leave for which AI provider, through the real host
//! (ADR-PROVIDER-002): a free Gemini quota receives the user's own data only
//! once the user says they use Nexus from the EEA (said only through the
//! Windows dialog); a folder marked as other people's data is not read for a
//! connection that may not receive it; each connection says what it may
//! receive and why.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{
    ApprovalPath, ChatStatus, DataChange, DataPolicyView, ErrorCode, ProviderChoice,
    SaveConnectionRequest, SaveFolderRequest, SetDataPolicyRequest,
};
use serde_json::{Value, json};
use support::TestHost;
use support::chat::{send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "data-policy-token";
const KEY: &str = "SENTINEL-gemini-key-data-policy-2b77";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(30),
        retry_waits: Vec::new(),
        extractor_exe: Some(PathBuf::from(env!("CARGO_BIN_EXE_nexus-host"))),
        ..RuntimeConfig::default()
    }
}

async fn connect(client: &NexusClient, free_tier: bool) {
    let saved = client
        .save_connection(SaveConnectionRequest {
            connection_id: None,
            display_name: Some("Google AI Studio".into()),
            provider: Some(ProviderChoice::Gemini),
            free_tier,
            api_key: Some(KEY.into()),
        })
        .await
        .unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
}

fn change(change: DataChange, via: ApprovalPath) -> SetDataPolicyRequest {
    SetDataPolicyRequest { change, via }
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let path =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Dir(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn answer(body: &Value, name: &str) -> Value {
    body["contents"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["parts"].as_array().unwrap())
        .find(|p| p["functionResponse"]["name"] == name)
        .map(|p| p["functionResponse"]["response"].clone())
        .expect("the call is answered")
}

#[tokio::test]
async fn a_free_gemini_quota_gets_your_data_only_once_you_say_you_are_in_the_eea() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client, true).await;

    let policy = client.data_policy().await.unwrap();
    let connection = &policy.connections[0];
    assert_eq!(connection.can_receive, "Public");
    assert!(connection.reason.contains("EEA"), "{}", connection.reason);
    assert_eq!(
        connection.terms_url,
        "https://ai.google.dev/gemini-api/terms"
    );
    assert!(!policy.in_eea);

    // Nothing is sent: the conversation itself is the user's own data.
    let sent = send(&client, None, "d1", "Hello").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Failed);
    assert_eq!(fake.generations().len(), 0, "nothing reached the provider");

    // Saying so lets more data leave: only through the Windows dialog.
    let from_window = client
        .set_data_policy(change(
            DataChange::InEea { in_eea: true },
            ApprovalPath::Window,
        ))
        .await;
    assert_eq!(from_window.unwrap_err().code(), Some(ErrorCode::Forbidden));
    let policy: DataPolicyView = client
        .set_data_policy(change(
            DataChange::InEea { in_eea: true },
            ApprovalPath::TrustedDialog,
        ))
        .await
        .unwrap();
    assert!(policy.in_eea);
    assert_eq!(policy.connections[0].can_receive, "Personal");

    fake.push(Reply::text(&["Hi."]));
    let sent = send(&client, None, "d2", "Hello again").await;
    assert_eq!(settled(&client, &sent).await.status, ChatStatus::Complete);
    // Taking it back keeps more in: the window may do it.
    client
        .set_data_policy(change(
            DataChange::InEea { in_eea: false },
            ApprovalPath::Window,
        ))
        .await
        .unwrap();
    host.shutdown().await;
}

#[tokio::test]
async fn a_folder_of_other_peoples_data_is_not_read_for_a_connection_that_may_not_receive_it() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client, true).await;
    client
        .set_data_policy(change(
            DataChange::InEea { in_eea: true },
            ApprovalPath::TrustedDialog,
        ))
        .await
        .unwrap();
    let dir = Dir::new("clients");
    std::fs::write(dir.0.join("contract.txt"), "Client contract, confidential.").unwrap();
    let folder = client
        .save_folder(SaveFolderRequest {
            folder_id: None,
            revision: 0,
            path: dir.0.display().to_string(),
            label: "Clients".into(),
            can_write: false,
        })
        .await
        .unwrap();
    // Marking it as other people's data keeps more in: the window may.
    let policy = client
        .set_data_policy(change(
            DataChange::Source {
                kind: "Folder".into(),
                id: folder.folder_id.to_string(),
                data_class: "Business".into(),
            },
            ApprovalPath::Window,
        ))
        .await
        .unwrap();
    assert!(
        policy
            .sources
            .iter()
            .any(|s| s.name == "Clients" && s.data_class == "Business")
    );

    fake.push(Reply::calls(&[(
        "read_document",
        json!({"folder": "Clients", "path": "contract.txt"}),
    )]));
    fake.push(Reply::text(&["I may not read it."]));
    let sent = send(&client, None, "c1", "Summarise the client contract").await;
    settled(&client, &sent).await;
    let body = fake.generations()[1].json();
    let refused = answer(&body, "read_document");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("other people's data"),
        "{refused}"
    );
    assert!(
        !body.to_string().contains("confidential"),
        "the document never reached the provider"
    );

    // Back to the user's own data lets more leave: only through the dialog.
    let back = change(
        DataChange::Source {
            kind: "Folder".into(),
            id: folder.folder_id.to_string(),
            data_class: "Personal".into(),
        },
        ApprovalPath::Window,
    );
    assert_eq!(
        client.set_data_policy(back).await.unwrap_err().code(),
        Some(ErrorCode::Forbidden)
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_paid_connection_may_receive_other_peoples_data() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client, false).await;
    let policy = client.data_policy().await.unwrap();
    assert_eq!(policy.connections[0].can_receive, "Business");
    assert_eq!(policy.connections[0].dpa, "Included");
    host.shutdown().await;
}
