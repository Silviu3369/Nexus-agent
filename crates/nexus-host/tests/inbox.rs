//! The job ads inbox through the real host (`PLAN_NEXUS_RO.md` §3a): a PDF
//! put in the inbox folder becomes an application on its own — with its
//! reference number, platform and file — through a task the agent runs as
//! if the user had asked; the same content is never read twice (a copy, a
//! rename); temporary and other files are ignored; a letter Nexus wrote
//! into the folder is not taken for an ad; without a model a file fails and
//! can be retried; turned off, the inbox reads nothing.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{
    ApprovalDecision, DecideApprovalRequest, InboxFileStatus, InboxView, RecordQuery, RecordView,
    RegisterView, RunStatus, SaveFolderRequest,
};
use serde_json::json;
use support::TestHost;
use support::chat::{gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "inbox-token";
const KEY: &str = "SENTINEL-gemini-key-inbox-91c3";

fn config(fake: Option<&FakeGemini>) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.map(|f| f.base_url.clone()).unwrap_or_default(),
        reply_deadline: Duration::from_secs(20),
        retry_waits: Vec::new(),
        extractor_exe: Some(PathBuf::from(env!("CARGO_BIN_EXE_nexus-host"))),
        inbox_interval: Duration::from_millis(150),
        ..RuntimeConfig::default()
    }
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

/// A one-page PDF whose text is `text`.
fn tiny_pdf(text: &str) -> Vec<u8> {
    let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

async fn connect(client: &NexusClient) {
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
}

async fn jobs(client: &NexusClient) -> RegisterView {
    client.registers().await.unwrap().remove(0)
}

async fn records(client: &NexusClient, register: &RegisterView) -> Vec<RecordView> {
    client
        .records(RecordQuery {
            register_id: register.register_id,
            statuses: Vec::new(),
            text: None,
            follow_up_due: false,
            limit: 0,
        })
        .await
        .unwrap()
}

/// Waits until the inbox satisfies `done`.
async fn inbox_until(
    client: &NexusClient,
    register: &RegisterView,
    done: impl Fn(&InboxView) -> bool,
) -> InboxView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let view = client.inbox(register.register_id).await.unwrap();
        if done(&view) {
            return view;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "inbox stayed {view:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn share(
    client: &NexusClient,
    dir: &Dir,
    label: &str,
    can_write: bool,
) -> nexus_protocol::FolderView {
    client
        .save_folder(SaveFolderRequest {
            folder_id: None,
            revision: 0,
            path: dir.0.display().to_string(),
            label: label.into(),
            can_write,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn a_job_ad_put_in_the_inbox_becomes_an_application_once() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(Some(&fake))).await;
    let client = host.client().await;
    connect(&client).await;
    let register = jobs(&client).await;
    assert!(register.fields.iter().any(|f| f.key == "reference"));
    let ads = Dir::new("inbox-ads");
    let folder = share(&client, &ads, "Ads", true).await;
    let inbox = client
        .save_inbox(register.register_id, 0, Some(folder.folder_id), true)
        .await
        .unwrap();
    assert!(inbox.enabled && inbox.files.is_empty());

    fake.push(Reply::calls(&[(
        "read_document",
        json!({"folder": "Ads", "path": "TopJobs vacature.pdf"}),
    )]));
    fake.push(Reply::calls(&[(
        "add_record",
        json!({"register": "job_applications", "values": {
            "company": "Acme", "role": "Service Desk Agent", "status": "To apply",
            "reference": "70123456", "source": "TopJobs", "location": "Springfield",
            "ad_file": "Ads/TopJobs vacature.pdf"
        }}),
    )]));
    fake.push(Reply::text(&[
        "Saved Acme, Service Desk Agent (TopJobs 70123456).",
    ]));
    std::fs::write(ads.0.join("~$draft.docx"), b"lock").unwrap();
    std::fs::write(ads.0.join("list.csv"), "a;b").unwrap();
    let pdf = tiny_pdf("Vacature 70123456 Service Desk Agent bij Acme in Springfield");
    std::fs::write(ads.0.join("TopJobs vacature.pdf"), &pdf).unwrap();

    let view = inbox_until(&client, &register, |v| {
        v.files.iter().any(|f| f.status == InboxFileStatus::Started)
    })
    .await;
    assert_eq!(
        view.files.len(),
        1,
        "temporary and list files are not ads: {:?}",
        view.files
    );
    let started = &view.files[0];
    assert_eq!(started.path, "TopJobs vacature.pdf");
    let conversation = started.conversation_id.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let record = loop {
        if let Some(r) = records(&client, &register).await.into_iter().next() {
            break r;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no application was added"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(record.values["reference"], "70123456");
    assert_eq!(record.values["ad_file"], "Ads/TopJobs vacature.pdf");
    assert_eq!(record.created_by, nexus_protocol::RecordActor::Agent);
    let task = client.conversation(conversation).await.unwrap();
    assert_eq!(task.title, "Job ad · TopJobs vacature.pdf");
    let asked = &task.messages[0].text;
    assert!(
        asked.contains("\"Ads\" as \"TopJobs vacature.pdf\"")
            && asked.contains("TopJobs's vacature number"),
        "{asked}"
    );
    let read = fake.generations()[1].json().to_string();
    assert!(read.contains("70123456"), "the agent read the PDF's text");

    // The same content again (a copy) is not read twice.
    std::fs::write(ads.0.join("copy.pdf"), &pdf).unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(
        client
            .inbox(register.register_id)
            .await
            .unwrap()
            .files
            .len(),
        1
    );
    let calls_so_far = fake.generations().len();

    // A letter Nexus writes into the folder is not taken for an ad.
    fake.push(Reply::calls(&[(
        "propose_document",
        json!({"folder": "Ads", "file_name": "Letter Acme", "format": "docx", "body": "Dear Acme team"}),
    )]));
    fake.push(Reply::text(&["Written."]));
    let sent = send(&client, None, "letter", "Write a letter to Acme").await;
    let run = settled(&client, &sent).await.proposed_run_id.unwrap();
    let pending = loop {
        let view = client.run(run).await.unwrap();
        if view.status == RunStatus::AwaitingApproval {
            break view.pending_approval.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
        })
        .await
        .unwrap();
    let view = inbox_until(&client, &register, |v| v.files.len() == 2).await;
    assert_eq!(view.files[0].path, "Letter Acme.docx");
    assert_eq!(view.files[0].status, InboxFileStatus::Skipped);
    assert_eq!(
        view.files[0].reason.as_deref(),
        Some("a file Nexus wrote itself")
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        fake.generations().len(),
        calls_so_far + 2,
        "only the letter's own task called the model"
    );

    // Turned off, the inbox reads nothing new.
    client
        .save_inbox(
            register.register_id,
            view.revision,
            Some(folder.folder_id),
            false,
        )
        .await
        .unwrap();
    std::fs::write(ads.0.join("Another ad.txt"), "Vacature 2 at Beta").unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(
        client
            .inbox(register.register_id)
            .await
            .unwrap()
            .files
            .len(),
        2
    );
    assert_eq!(records(&client, &register).await.len(), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn without_a_model_a_file_fails_and_can_be_retried() {
    let host = TestHost::start_with(TOKEN, config(None)).await;
    let client = host.client().await;
    let register = jobs(&client).await;
    let ads = Dir::new("inbox-nomodel");
    let folder = share(&client, &ads, "Ads", false).await;
    let err = client
        .save_inbox(register.register_id, 0, None, true)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(nexus_protocol::ErrorCode::InvalidRequest));
    client
        .save_inbox(register.register_id, 0, Some(folder.folder_id), true)
        .await
        .unwrap();
    std::fs::write(ads.0.join("ad.txt"), "Vacature at Acme").unwrap();
    let view = inbox_until(&client, &register, |v| {
        v.files.iter().any(|f| f.status == InboxFileStatus::Failed)
    })
    .await;
    let failed = &view.files[0];
    assert!(
        failed
            .reason
            .as_deref()
            .unwrap()
            .contains("choose an AI connection"),
        "{failed:?}"
    );
    let retried = client
        .retry_inbox_file(register.register_id, failed.file_id)
        .await
        .unwrap();
    assert!(matches!(
        retried.files[0].status,
        InboxFileStatus::Waiting | InboxFileStatus::Failed
    ));
    inbox_until(&client, &register, |v| {
        v.files[0].status == InboxFileStatus::Failed
    })
    .await;

    // Unsharing the folder removes the inbox.
    client.remove_folder(folder.folder_id).await.unwrap();
    let view = client.inbox(register.register_id).await.unwrap();
    assert!(!view.enabled && view.folder_id.is_none());
    host.shutdown().await;
}
