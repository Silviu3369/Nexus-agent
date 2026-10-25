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
    RegisterView, RunStatus, SaveFolderRequest, SaveInboxRequest,
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
        .save_inbox(SaveInboxRequest {
            register_id: register.register_id,
            revision: 0,
            folder_id: Some(folder.folder_id),
            enabled: true,
            cv_folder_id: None,
            cv_path: None,
        })
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
        v.files
            .iter()
            .any(|f| matches!(f.status, InboxFileStatus::Started | InboxFileStatus::Done))
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
    assert_eq!(
        task.requested_by,
        Some(nexus_protocol::ActorView {
            kind: "Intake".into(),
            name: "Inbox".into(),
        }),
        "the inbox asked for the task (ADR-IDENTITY-001)"
    );
    let asked = &task.messages[0].text;
    assert!(
        asked.contains("\"Ads\" as \"TopJobs vacature.pdf\"")
            && asked.contains("TopJobs's vacature number"),
        "{asked}"
    );
    let read = fake.generations()[1].json().to_string();
    assert!(read.contains("70123456"), "the agent read the PDF's text");

    // Results count the ad by how its task ended (ADR-TASK-001).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let handled = loop {
        let handled = client.job_results(register.register_id).await.unwrap().ads;
        if handled.finished == 1 {
            break handled;
        }
        assert!(tokio::time::Instant::now() < deadline, "{handled:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        (
            handled.seen,
            handled.recorded,
            handled.nothing_recorded,
            handled.failed
        ),
        (1, 1, 0, 0)
    );
    assert!(
        handled.prompt_tokens > 0,
        "the ad's tokens are counted: {handled:?}"
    );
    assert!(handled.median_seconds.is_some());
    assert_eq!(
        handled.estimated_cost_usd, None,
        "the test connection is paid and no price was entered: no estimate"
    );

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
            via: nexus_protocol::ApprovalPath::TrustedDialog,
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
        .save_inbox(SaveInboxRequest {
            register_id: register.register_id,
            revision: view.revision,
            folder_id: Some(folder.folder_id),
            enabled: false,
            cv_folder_id: None,
            cv_path: None,
        })
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
        .save_inbox(SaveInboxRequest {
            register_id: register.register_id,
            revision: 0,
            folder_id: None,
            enabled: true,
            cv_folder_id: None,
            cv_path: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(nexus_protocol::ErrorCode::InvalidRequest));
    client
        .save_inbox(SaveInboxRequest {
            register_id: register.register_id,
            revision: 0,
            folder_id: Some(folder.folder_id),
            enabled: true,
            cv_folder_id: None,
            cv_path: None,
        })
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

#[tokio::test]
async fn a_cv_kept_in_the_inbox_folder_is_not_taken_for_an_ad() {
    let host = TestHost::start_with(TOKEN, config(None)).await;
    let client = host.client().await;
    let register = jobs(&client).await;
    let dir = Dir::new("inbox-cv");
    std::fs::write(dir.0.join("CV.txt"), "Jane Doe, service desk").unwrap();
    let folder = share(&client, &dir, "Jobs", false).await;
    client
        .save_inbox(SaveInboxRequest {
            register_id: register.register_id,
            revision: 0,
            folder_id: Some(folder.folder_id),
            enabled: true,
            cv_folder_id: Some(folder.folder_id),
            cv_path: Some("cv.TXT".into()),
        })
        .await
        .unwrap();
    let view = inbox_until(&client, &register, |v| !v.files.is_empty()).await;
    let cv = &view.files[0];
    assert_eq!(
        (cv.path.as_str(), cv.status),
        ("CV.txt", InboxFileStatus::Skipped)
    );
    assert_eq!(
        cv.reason.as_deref(),
        Some("your CV, used for the match score")
    );
    assert!(cv.conversation_id.is_none(), "no task was started for it");
    host.shutdown().await;
}

fn retrying(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        inbox_retry: Duration::from_millis(300),
        ..config(Some(fake))
    }
}

#[tokio::test]
async fn a_rate_limited_ad_is_tried_again_on_its_own_and_scored_against_the_cv() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, retrying(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let register = jobs(&client).await;
    let jobs_dir = Dir::new("inbox-cv");
    std::fs::write(
        jobs_dir.0.join("CV.txt"),
        "Five years of Microsoft 365 support",
    )
    .unwrap();
    let ads = Dir::new("inbox-retry");
    let cv_folder = share(&client, &jobs_dir, "Jobs", false).await;
    let ads_folder = share(&client, &ads, "Ads", false).await;

    for (folder, path) in [
        (Some(cv_folder.folder_id), "../CV.txt"),
        (Some(cv_folder.folder_id), "missing.pdf"),
    ] {
        let err = client
            .save_inbox(SaveInboxRequest {
                register_id: register.register_id,
                revision: 0,
                folder_id: Some(ads_folder.folder_id),
                enabled: true,
                cv_folder_id: folder,
                cv_path: Some(path.into()),
            })
            .await
            .unwrap_err();
        assert_eq!(
            err.code(),
            Some(nexus_protocol::ErrorCode::InvalidRequest),
            "{path}"
        );
    }
    let view = client
        .save_inbox(SaveInboxRequest {
            register_id: register.register_id,
            revision: 0,
            folder_id: Some(ads_folder.folder_id),
            enabled: true,
            cv_folder_id: Some(cv_folder.folder_id),
            cv_path: Some("CV.txt".into()),
        })
        .await
        .unwrap();
    assert_eq!(view.cv_folder_label.as_deref(), Some("Jobs"));
    assert_eq!(view.cv_path.as_deref(), Some("CV.txt"));

    // The first try is refused by the provider; the second one works.
    fake.push(Reply::status(
        429,
        r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED"}}"#,
    ));
    fake.push(Reply::calls(&[
        ("read_document", json!({"folder": "Ads", "path": "ad.txt"})),
        ("read_document", json!({"folder": "Jobs", "path": "CV.txt"})),
    ]));
    fake.push(Reply::calls(&[(
        "add_record",
        json!({"register": "job_applications", "values": {
            "company": "Acme", "role": "M365 Support", "status": "To apply", "reference": "4711",
            "match_score": 78, "match_notes": "Fits: Microsoft 365. Missing: Dutch C1.",
            "ad_file": "Ads/ad.txt"
        }}),
    )]));
    fake.push(Reply::text(&["Saved Acme, M365 Support, match 78."]));
    std::fs::write(
        ads.0.join("ad.txt"),
        "Vacature 4711: M365 Support at Acme. Dutch C1 required.",
    )
    .unwrap();

    let done = inbox_until(&client, &register, |v| {
        v.files
            .first()
            .is_some_and(|f| f.status == InboxFileStatus::Done)
    })
    .await;
    assert_eq!(done.files.len(), 1, "one file, tried twice");
    let record = records(&client, &register).await.remove(0);
    assert_eq!(record.values["match_score"], "78");
    assert!(record.values["match_notes"].contains("Missing: Dutch C1"));
    let titled: Vec<_> = client
        .conversations(20)
        .await
        .unwrap()
        .into_iter()
        .filter(|c| c.title == "Job ad · ad.txt")
        .collect();
    assert_eq!(
        titled.len(),
        2,
        "the retry is a new task, the first one stays as it ended"
    );
    let second = client
        .conversation(done.files[0].conversation_id.unwrap())
        .await
        .unwrap();
    assert!(
        second.messages[0].text.contains("path \"CV.txt\""),
        "{}",
        second.messages[0].text
    );
    // The ad counts once, by the task that finished it; the refused first
    // try does not make it a failure.
    let ads = client.job_results(register.register_id).await.unwrap().ads;
    assert_eq!(
        (ads.seen, ads.finished, ads.recorded, ads.failed),
        (1, 1, 1, 0),
        "{ads:?}"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn with_the_daily_budget_used_up_a_file_waits_for_tomorrow() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, retrying(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let register = jobs(&client).await;
    fake.push(Reply::text(&["Hello."]));
    let sent = send(&client, None, "hi", "Hello").await;
    settled(&client, &sent).await;
    client.set_daily_token_budget(Some(1)).await.unwrap();
    let ads = Dir::new("inbox-budget");
    let folder = share(&client, &ads, "Ads", false).await;
    client
        .save_inbox(SaveInboxRequest {
            register_id: register.register_id,
            revision: 0,
            folder_id: Some(folder.folder_id),
            enabled: true,
            cv_folder_id: None,
            cv_path: None,
        })
        .await
        .unwrap();
    std::fs::write(ads.0.join("ad.txt"), "Vacature at Acme").unwrap();
    let view = inbox_until(&client, &register, |v| {
        v.files.first().is_some_and(|f| f.reason.is_some())
    })
    .await;
    assert_eq!(view.files[0].status, InboxFileStatus::Waiting);
    assert!(
        view.files[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("tried again tomorrow")
    );
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(
        fake.generations().len(),
        1,
        "nothing is sent while the budget is used up"
    );
    host.shutdown().await;
}
