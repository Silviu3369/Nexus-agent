//! Folders and documents through the real host (`PLAN_NEXUS_RO.md` §3a, step
//! 2), on real files: only an ordinary local folder can be shared; the agent
//! lists it and reads Word, PDF, Excel and text documents through the
//! contained reader, and never outside the folder — not by `..`, not by an
//! absolute path, not through a link; a broken or oversized file fails on
//! its own; a document is written only after the user approves the exact
//! file, and never over an existing one; a register goes to Excel and an
//! existing list comes in, with what cannot be used kept or listed.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_host::documents;
use nexus_host::tools::document_file::{self, DocFormat};
use nexus_protocol::{
    AgentStepDetail, AgentStepStatus, ApprovalDecision, ChatStatus, ConversationId,
    DecideApprovalRequest, ErrorCode, FolderView, ImportRequest, RecordQuery, RunId, RunKind,
    RunStatus, RunView, SaveFolderRequest, SaveRecordRequest,
};
use serde_json::{Value, json};
use support::TestHost;
use support::chat::{gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "documents-token";
const KEY: &str = "SENTINEL-gemini-key-documents-55e1";
const CV_LINE: &str = "Service desk analyst with five years of Microsoft 365 support.";

fn extractor() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_nexus-host"))
}

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(30),
        retry_waits: Vec::new(),
        extractor_exe: Some(extractor()),
        ..RuntimeConfig::default()
    }
}

/// A folder outside app data (which cannot be shared), removed afterwards.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let path =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Dir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn text(&self) -> String {
        self.0.display().to_string()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn junction(link: &Path, target: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
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

fn workbook(path: &Path) {
    use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    sheet.write_string(0, 0, "Company").unwrap();
    sheet.write_string(0, 1, "Applied").unwrap();
    sheet.write_string(1, 0, "Acme").unwrap();
    let date = ExcelDateTime::parse_from_str("2026-10-18").unwrap();
    sheet
        .write_datetime_with_format(1, 1, &date, &Format::new().set_num_format("dd.mm.yyyy"))
        .unwrap();
    book.save(path).unwrap();
}

async fn connect(client: &NexusClient) {
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
}

async fn share(client: &NexusClient, label: &str, dir: &Dir, can_write: bool) -> FolderView {
    client
        .save_folder(SaveFolderRequest {
            folder_id: None,
            revision: 0,
            path: dir.text(),
            label: label.to_string(),
            can_write,
        })
        .await
        .unwrap()
}

fn answers<'a>(body: &'a Value, name: &str) -> Vec<&'a Value> {
    body["contents"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["parts"].as_array().unwrap())
        .filter(|p| p["functionResponse"]["name"] == name)
        .map(|p| &p["functionResponse"]["response"])
        .collect()
}

async fn wait_run(client: &NexusClient, run: RunId, wanted: RunStatus) -> RunView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let view = client.run(run).await.unwrap();
        if view.status == wanted {
            return view;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run stayed {:?}, expected {wanted:?}",
            view.status
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn texts(client: &NexusClient, id: ConversationId, count: usize) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let view = client.conversation(id).await.unwrap();
        if view.messages.len() == count
            && view
                .messages
                .iter()
                .all(|m| m.status != ChatStatus::Streaming)
        {
            return view.messages.into_iter().map(|m| m.text).collect();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} messages",
            view.messages.len()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn only_an_ordinary_local_folder_can_be_shared_and_links_are_not_listed() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let inside = Dir::new("share-ok");
    let outside = Dir::new("share-outside");
    let file = inside.path().join("a.txt");
    std::fs::write(&file, "hello").unwrap();
    let link = Dir::new("share-link-parent");
    let link_path = link.path().join("link");
    junction(&link_path, outside.path());

    let refused = [
        "C:\\".to_string(),
        std::env::var("SystemRoot").unwrap(),
        std::env::temp_dir().display().to_string(),
        host.profile.path().display().to_string(),
        r"\\server\share\docs".to_string(),
        "Documents".to_string(),
        inside.path().join("missing").display().to_string(),
        file.display().to_string(),
        link_path.display().to_string(),
        std::env::var("USERPROFILE").unwrap(),
    ];
    for path in refused {
        let err = client
            .save_folder(SaveFolderRequest {
                folder_id: None,
                revision: 0,
                path: path.clone(),
                label: "X".into(),
                can_write: false,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::InvalidRequest), "{path}: {err}");
    }

    let shared = share(&client, "Jobs", &inside, false).await;
    assert!(shared.available && !shared.can_write);
    for (label, path) in [("jobs", inside.text()), ("Other", inside.text())] {
        let err = client
            .save_folder(SaveFolderRequest {
                folder_id: None,
                revision: 0,
                path,
                label: label.into(),
                can_write: false,
            })
            .await
            .unwrap_err();
        assert_eq!(
            err.code(),
            Some(ErrorCode::InvalidRequest),
            "a second share of the same name or place"
        );
    }

    std::fs::create_dir(inside.path().join("sub")).unwrap();
    std::fs::write(inside.path().join("sub").join("b.docx"), b"x").unwrap();
    std::fs::write(inside.path().join("hidden.txt"), b"x").unwrap();
    std::process::Command::new("attrib")
        .args(["+h"])
        .arg(inside.path().join("hidden.txt"))
        .status()
        .unwrap();
    junction(&inside.path().join("escape"), outside.path());
    let entries: Vec<String> = client
        .folder_entries(shared.folder_id)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert_eq!(
        entries,
        ["a.txt", "sub", "sub/b.docx"],
        "no hidden file, no link"
    );

    client.remove_folder(shared.folder_id).await.unwrap();
    assert!(client.folders().await.unwrap().is_empty());
    assert!(file.exists(), "unsharing touches nothing in the folder");
    host.shutdown().await;
}

#[tokio::test]
async fn the_agent_reads_word_pdf_and_excel_in_a_shared_folder_and_nothing_outside_it() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let docs = Dir::new("docs");
    let outside = Dir::new("docs-outside");
    std::fs::write(outside.path().join("secret.txt"), "SENTINEL-outside-secret").unwrap();
    let cv = document_file::build(DocFormat::Word, "CV", CV_LINE).unwrap();
    std::fs::write(docs.path().join("CV.docx"), &cv.bytes).unwrap();
    std::fs::write(
        docs.path().join("Ad.pdf"),
        tiny_pdf("Service Desk Analyst at Acme"),
    )
    .unwrap();
    workbook(&docs.path().join("Jobs.xlsx"));
    junction(&docs.path().join("escape"), outside.path());
    share(&client, "Jobs", &docs, false).await;

    fake.push(Reply::calls(&[("list_files", json!({"folder": "Jobs"}))]));
    fake.push(Reply::calls(&[
        (
            "read_document",
            json!({"folder": "Jobs", "path": "CV.docx"}),
        ),
        ("read_document", json!({"folder": "Jobs", "path": "Ad.pdf"})),
        (
            "read_document",
            json!({"folder": "jobs", "path": "Jobs.xlsx"}),
        ),
    ]));
    fake.push(Reply::calls(&[
        ("read_document", json!({"folder": "Jobs", "path": "../docs-outside/secret.txt"})),
        ("read_document", json!({"folder": "Jobs", "path": "escape/secret.txt"})),
        ("read_document", json!({"folder": "Jobs", "path": outside.path().join("secret.txt").display().to_string()})),
    ]));
    fake.push(Reply::text(&["Your CV matches the ad."]));
    let sent = send(
        &client,
        None,
        "d1",
        "Compare my CV with the ad in my Jobs folder",
    )
    .await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.text, "Your CV matches the ad.");

    let generations = fake.generations();
    let system = generations[0].json()["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(system.contains("\"Jobs\" (read only)"), "{system}");
    assert!(
        !system.contains(&docs.text()),
        "the folder's path is not sent"
    );
    let listing = answers(&generations[1].json(), "list_files")[0].clone();
    let paths: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        ["Ad.pdf", "CV.docx", "Jobs.xlsx"],
        "the link is not listed"
    );

    let reads = generations[2].json();
    let reads = answers(&reads, "read_document");
    assert!(reads[0]["text"].as_str().unwrap().contains(CV_LINE));
    assert_eq!(reads[0]["format"], "Word");
    assert!(
        reads[1]["text"]
            .as_str()
            .unwrap()
            .contains("Service Desk Analyst at Acme"),
        "{}",
        reads[1]
    );
    let sheet = reads[2]["text"].as_str().unwrap();
    assert!(
        sheet.contains("Acme\t2026-10-18"),
        "an Excel date reads as a date: {sheet}"
    );

    let escapes = generations[3].json();
    let escapes = answers(&escapes, "read_document");
    // The earlier three reads are in the history; the last three are new.
    assert_eq!(escapes.len(), 6);
    let escapes = &escapes[3..];
    for e in escapes {
        assert!(e["error"].is_string(), "{e}");
    }
    assert!(
        !generations[3]
            .json()
            .to_string()
            .contains("SENTINEL-outside-secret")
    );
    let read_steps: Vec<(AgentStepStatus, Option<&str>)> = reply
        .steps
        .iter()
        .filter_map(|s| match &s.detail {
            AgentStepDetail::DocumentRead { format, .. } => Some((s.status, format.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(
        read_steps,
        [
            (AgentStepStatus::Done, Some("Word")),
            (AgentStepStatus::Done, Some("PDF")),
            (AgentStepStatus::Done, Some("Spreadsheet")),
            (AgentStepStatus::Failed, None),
            (AgentStepStatus::Failed, None),
            (AgentStepStatus::Failed, None),
        ]
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_long_document_is_read_on_in_parts_when_the_model_asks() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let docs = Dir::new("long");
    // 1,000 rows of 60 characters: 60,000 in all, past one part of 40,000.
    let mut text: String = (0..999)
        .map(|i| format!("{:<59}\n", format!("row {i}: Service Desk, Springfield")))
        .collect();
    text.push_str(&format!("{:<59}\n", "LAST ROW"));
    std::fs::write(docs.path().join("long.txt"), &text).unwrap();
    share(&client, "Docs", &docs, false).await;

    // The first part ends after the last whole row that fits.
    let next = (40_000 / 60) * 60;
    fake.push(Reply::calls(&[(
        "read_document",
        json!({"folder": "Docs", "path": "long.txt"}),
    )]));
    fake.push(Reply::calls(&[
        // Sent as a float, as some models do.
        (
            "read_document",
            json!({"folder": "Docs", "path": "long.txt", "offset": next as f64}),
        ),
        (
            "read_document",
            json!({"folder": "Docs", "path": "long.txt", "offset": -5}),
        ),
        (
            "read_document",
            json!({"folder": "Docs", "path": "long.txt", "offset": 60_000}),
        ),
    ]));
    fake.push(Reply::text(&["The last row is LAST ROW."]));
    let sent = send(&client, None, "p1", "What is the last row of long.txt?").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.text, "The last row is LAST ROW.");

    let generations = fake.generations();
    let first = answers(&generations[1].json(), "read_document")[0].clone();
    let first_text = first["text"].as_str().unwrap();
    assert_eq!(first_text.chars().count(), next);
    assert!(first_text.ends_with('\n') && !first_text.contains("LAST ROW"));
    assert_eq!(first["offset"], 0);
    assert_eq!(first["next_offset"], next);
    assert!(
        first["note"]
            .as_str()
            .unwrap()
            .contains(&format!("offset {next}")),
        "{first}"
    );

    let later = generations[2].json();
    let later = answers(&later, "read_document");
    // The first part is in the history; the last three are new.
    assert_eq!(later.len(), 4);
    let rest = later[1];
    assert_eq!(rest["offset"], next);
    assert_eq!(rest["text"].as_str().unwrap(), &text[next..]);
    assert!(rest.get("next_offset").is_none(), "{rest}");
    assert!(
        later[2]["error"].as_str().unwrap().contains("whole number"),
        "{}",
        later[2]
    );
    assert!(
        later[3]["error"]
            .as_str()
            .unwrap()
            .contains("60000 characters"),
        "{}",
        later[3]
    );

    let read_steps: Vec<(AgentStepStatus, Option<u32>, Option<bool>)> = reply
        .steps
        .iter()
        .filter_map(|s| match &s.detail {
            AgentStepDetail::DocumentRead {
                offset, truncated, ..
            } => Some((s.status, *offset, *truncated)),
            _ => None,
        })
        .collect();
    assert_eq!(
        read_steps,
        [
            (AgentStepStatus::Done, Some(0), Some(true)),
            (AgentStepStatus::Done, Some(next as u32), Some(false)),
            (AgentStepStatus::Failed, None, None),
            (AgentStepStatus::Failed, Some(60_000), None),
        ]
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_broken_or_oversized_file_fails_on_its_own() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let docs = Dir::new("broken");
    std::fs::write(
        docs.path().join("broken.pdf"),
        b"%PDF-1.4\n not really a pdf \x00\x01\x02",
    )
    .unwrap();
    std::fs::write(docs.path().join("broken.docx"), b"PK\x03\x04 truncated").unwrap();
    std::fs::write(docs.path().join("big.txt"), vec![b'a'; 21 * 1024 * 1024]).unwrap();
    std::fs::write(docs.path().join("tool.exe"), b"MZ").unwrap();
    share(&client, "Docs", &docs, false).await;

    fake.push(Reply::calls(&[
        (
            "read_document",
            json!({"folder": "Docs", "path": "broken.pdf"}),
        ),
        (
            "read_document",
            json!({"folder": "Docs", "path": "broken.docx"}),
        ),
        (
            "read_document",
            json!({"folder": "Docs", "path": "big.txt"}),
        ),
        (
            "read_document",
            json!({"folder": "Docs", "path": "tool.exe"}),
        ),
    ]));
    fake.push(Reply::text(&["None of them could be read."]));
    let sent = send(&client, None, "b1", "Read my documents").await;
    settled(&client, &sent).await;
    let body = fake.generations()[1].json();
    let results = answers(&body, "read_document");
    let errors: Vec<&str> = results
        .iter()
        .map(|r| r["error"].as_str().unwrap())
        .collect();
    assert!(errors[0].contains("PDF"), "{errors:?}");
    assert!(errors[1].contains("Word"), "{errors:?}");
    assert!(errors[2].contains("MB"), "{errors:?}");
    assert!(errors[3].contains("only"), "{errors:?}");

    // The Host is unaffected.
    fake.push(Reply::text(&["Still here."]));
    let again = send(&client, None, "b2", "Hello").await;
    assert_eq!(settled(&client, &again).await.text, "Still here.");
    host.shutdown().await;
}

#[tokio::test]
async fn a_document_is_written_only_after_approval_and_never_over_a_file() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let out = Dir::new("letters");
    let read_only = Dir::new("readonly");
    share(&client, "Letters", &out, true).await;
    share(&client, "Archive", &read_only, false).await;
    std::fs::write(out.path().join("Taken.docx"), b"mine").unwrap();
    let body = "Dear Acme team,\n\nI am applying for the Service Desk Analyst role.";

    fake.push(Reply::calls(&[(
        "propose_document",
        json!({"folder": "Letters", "file_name": "Cover letter Acme", "format": "docx", "title": "", "body": body}),
    )]));
    fake.push(Reply::text(&["The letter is saved."]));
    let sent = send(&client, None, "w1", "Write my cover letter for Acme").await;
    let reply = settled(&client, &sent).await;
    let run = reply.proposed_run_id.expect("the document waits as a run");
    let target = out.path().join("Cover letter Acme.docx");
    let waiting = wait_run(&client, run, RunStatus::AwaitingApproval).await;
    assert_eq!(waiting.kind, RunKind::Document);
    assert_eq!(waiting.title, "Cover letter Acme.docx in Letters");
    let pending = waiting.pending_approval.unwrap();
    let preview = pending
        .preview
        .expect("the exact file is shown before deciding");
    assert_eq!(preview.destination, target.display().to_string());
    assert!(preview.content.contains("Service Desk Analyst role"));
    assert!(!target.exists(), "nothing is written before the approval");

    // APPROVAL-01: the window alone cannot approve a file outside Nexus.
    let from_window = client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256.clone(),
            decision: ApprovalDecision::Approve,
            via: nexus_protocol::ApprovalPath::Window,
        })
        .await;
    assert_eq!(
        from_window.unwrap_err().code(),
        Some(nexus_protocol::ErrorCode::Forbidden)
    );
    assert!(!target.exists(), "refused, nothing is written");
    assert_eq!(
        client.run(run).await.unwrap().status,
        RunStatus::AwaitingApproval,
        "the request still waits"
    );

    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
            via: nexus_protocol::ApprovalPath::TrustedDialog,
        })
        .await
        .unwrap();
    let done = wait_run(&client, run, RunStatus::Succeeded).await;
    assert_eq!(
        (
            done.effect_class.as_deref(),
            done.evidence.as_deref(),
            done.can_undo
        ),
        (Some("ExternalReversible"), Some("Verified"), true),
        "ADR-ACTION-001: the run says what kind of effect it is and how it is proven"
    );
    let written = std::fs::read(&target).unwrap();
    assert_eq!(
        documents::docx_text(&written).unwrap(),
        "Dear Acme team,\nI am applying for the Service Desk Analyst role.\n"
    );
    assert_eq!(
        texts(&client, sent.conversation_id, 3).await[2],
        "The letter is saved."
    );
    let continued = fake.generations().pop().unwrap().json();
    assert_eq!(
        answers(&continued, "propose_document")[0]["status"],
        "written"
    );
    assert!(
        std::fs::read_dir(out.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("~nexus")),
        "no staging file is left"
    );

    // Undo (ADR-ACTION-001): refused while the file differs from what
    // Nexus wrote; once it is the same again, deleted, and only once.
    std::fs::write(&target, b"changed by me").unwrap();
    let refused = client.undo_effect(run).await.unwrap_err();
    assert!(
        format!("{refused:?}").contains("changed since Nexus wrote it"),
        "{refused:?}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"changed by me");
    std::fs::write(&target, &written).unwrap();
    let undone = client.undo_effect(run).await.unwrap();
    assert!(!target.exists(), "the document Nexus created is gone");
    assert!(undone.undone_at_ms.is_some() && !undone.can_undo);
    assert!(client.undo_effect(run).await.is_err(), "undone once");
    let task = client.conversation(sent.conversation_id).await.unwrap();
    let turn = task
        .messages
        .iter()
        .rev()
        .find_map(|m| m.turn.clone())
        .unwrap();
    assert_eq!(
        turn.interventions,
        ["Approved", "Undo"],
        "the approval and the undo are the user's interventions on the turn"
    );
    assert_eq!(turn.acceptance, nexus_protocol::Acceptance::NotAccepted);

    // An existing name, or a folder shared for reading, is refused at once.
    fake.push(Reply::calls(&[(
        "propose_document",
        json!({"folder": "Letters", "file_name": "Taken", "format": "docx", "body": "x"}),
    )]));
    fake.push(Reply::text(&["That name is taken."]));
    let taken = send(&client, None, "w2", "Save it as Taken").await;
    let taken = settled(&client, &taken).await;
    assert!(taken.proposed_run_id.is_none());
    assert!(taken.steps.iter().any(|s| matches!(&s.detail, AgentStepDetail::DocumentProposed { error: Some(e), .. } if e.contains("already exists"))));
    assert_eq!(
        std::fs::read(out.path().join("Taken.docx")).unwrap(),
        b"mine"
    );
    let body = fake.generations().pop().unwrap().json();
    assert!(
        body["tools"][0]["functionDeclarations"]
            .to_string()
            .contains("\"enum\":[\"Letters\"]"),
        "only folders shared for writing are offered for new files"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_register_goes_to_excel_and_an_existing_list_comes_in() {
    let host = TestHost::start_with(
        TOKEN,
        RuntimeConfig {
            extractor_exe: Some(extractor()),
            ..RuntimeConfig::default()
        },
    )
    .await;
    let client = host.client().await;
    let out = Dir::new("export");
    let read_only = Dir::new("export-ro");
    let writable = share(&client, "Exports", &out, true).await;
    let ro = share(&client, "Old", &read_only, false).await;
    let register = client.registers().await.unwrap().remove(0);
    let values = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    client
        .save_record(SaveRecordRequest {
            register_id: register.register_id,
            record_id: None,
            revision: 0,
            values: values(&[
                ("company", "Acme"),
                ("role", "Service Desk"),
                ("status", "Applied"),
                ("applied_on", "2026-10-18"),
            ]),
        })
        .await
        .unwrap();

    let refused = client
        .export_register(register.register_id, ro.folder_id)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::InvalidRequest));
    let exported = client
        .export_register(register.register_id, writable.folder_id)
        .await
        .unwrap();
    assert_eq!(exported.rows, 1);
    let table = documents::extract_table(Path::new(&exported.path), 100).unwrap();
    assert_eq!(
        table.rows[0][..5],
        ["Company", "Role", "Reference no.", "Status", "Applied on"]
    );
    assert_eq!(
        table.rows[1][..5],
        ["Acme", "Service Desk", "", "Applied", "2026-10-18"]
    );
    let second = client
        .export_register(register.register_id, writable.folder_id)
        .await
        .unwrap();
    assert_ne!(
        second.path, exported.path,
        "a second export never replaces the first"
    );

    std::fs::write(
        read_only.path().join("candidaturi.csv"),
        "Firma;Post;Status;Data aplicarii;Notite\n\
         Beta;IT Support;Aplicat;18.10.2026;via Ana\n\
         Gamma;Sysadmin;in asteptare;ieri;\n\
         Delta;;Aplicat;;\n\
         Acme;Service Desk;;;again\n",
    )
    .unwrap();
    let preview = client
        .import_preview(register.register_id, ro.folder_id, "candidaturi.csv".into())
        .await
        .unwrap();
    assert_eq!(preview.rows, 4);
    let mapping: Vec<Option<&str>> = preview.mapping.iter().map(|m| m.as_deref()).collect();
    assert_eq!(
        mapping,
        [
            Some("company"),
            Some("role"),
            Some("status"),
            Some("applied_on"),
            Some("notes")
        ]
    );
    let result = client
        .import_records(ImportRequest {
            register_id: register.register_id,
            folder_id: ro.folder_id,
            path: "candidaturi.csv".into(),
            mapping: preview.mapping.clone(),
        })
        .await
        .unwrap();
    assert_eq!((result.added, result.adjusted), (2, 1));
    let skipped: Vec<(u32, &str)> = result
        .skipped
        .iter()
        .map(|s| (s.row, s.reason.as_str()))
        .collect();
    assert_eq!(skipped.len(), 2);
    assert_eq!(skipped[0].0, 4);
    assert!(skipped[0].1.contains("Role is required"));
    assert_eq!(skipped[1].0, 5);
    assert!(skipped[1].1.contains("already in Job applications"));

    let all = client
        .records(RecordQuery {
            register_id: register.register_id,
            statuses: Vec::new(),
            text: None,
            follow_up_due: false,
            limit: 0,
        })
        .await
        .unwrap();
    let beta = all.iter().find(|r| r.values["company"] == "Beta").unwrap();
    assert_eq!(beta.values["status"], "Applied");
    assert_eq!(beta.values["applied_on"], "2026-10-18");
    let gamma = all.iter().find(|r| r.values["company"] == "Gamma").unwrap();
    assert_eq!(gamma.values["status"], "To apply");
    assert!(
        gamma.values["notes"].contains("Status in the file: in asteptare"),
        "{:?}",
        gamma.values
    );
    assert!(gamma.values["notes"].contains("Applied on in the file: ieri"));
    host.shutdown().await;
}

/// Starts the real host binary set to abort at `fault`, has the agent
/// propose a letter, approves it and waits for the crash.
async fn propose_and_crash(profile: &Path, fault: &str, fake: &FakeGemini, out: &Dir) -> RunId {
    let envs = [(support::GEMINI_BASE_URL_ENV, fake.base_url.as_str())];
    let mut host = support::spawn_host_env(profile, Some(fault), &envs);
    let client = support::connect(profile).await;
    connect(&client).await;
    share(&client, "Letters", out, true).await;
    fake.push(Reply::calls(&[(
        "propose_document",
        json!({"folder": "Letters", "file_name": "Cover letter", "format": "docx", "body": "Dear team,\n\nHello."}),
    )]));
    let sent = send(&client, None, "crash", "Write the letter").await;
    let run = settled(&client, &sent).await.proposed_run_id.unwrap();
    let pending = wait_run(&client, run, RunStatus::AwaitingApproval)
        .await
        .pending_approval
        .unwrap();
    let _ = client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
            via: nexus_protocol::ApprovalPath::TrustedDialog,
        })
        .await;
    assert!(
        !support::wait_exit(&mut host).success(),
        "the host aborted at {fault}"
    );
    run
}

#[tokio::test]
async fn a_crash_while_writing_leaves_exactly_one_document_after_restart() {
    for fault in ["after_intent", "after_publish"] {
        let fake = FakeGemini::start(KEY).await;
        let profile = support::TempProfile::new("doc-recovery");
        let out = Dir::new("doc-recovery");
        let run = propose_and_crash(profile.path(), fault, &fake, &out).await;
        let written_before = out.path().join("Cover letter.docx").exists();
        assert_eq!(written_before, fault == "after_publish", "{fault}");

        // The task continues once the document is settled.
        fake.push(Reply::text(&["The letter is saved."]));
        let envs = [(support::GEMINI_BASE_URL_ENV, fake.base_url.as_str())];
        let mut host = support::spawn_host_env(profile.path(), None, &envs);
        let client = support::connect(profile.path()).await;
        wait_run(&client, run, RunStatus::Succeeded).await;
        let names: Vec<String> = std::fs::read_dir(out.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            names,
            ["Cover letter.docx"],
            "{fault}: one file, no staging, no second copy"
        );
        let text =
            documents::docx_text(&std::fs::read(out.path().join("Cover letter.docx")).unwrap())
                .unwrap();
        assert_eq!(text, "Dear team,\nHello.\n");
        client.stop_host().await.ok();
        support::wait_exit(&mut host);
    }
}

#[tokio::test]
async fn an_undo_interrupted_before_deleting_leaves_the_file_to_be_undone_again() {
    let fake = FakeGemini::start(KEY).await;
    let profile = support::TempProfile::new("doc-undo-recovery");
    let out = Dir::new("doc-undo-recovery");
    let envs = [(support::GEMINI_BASE_URL_ENV, fake.base_url.as_str())];
    let mut host = support::spawn_host_env(profile.path(), Some("undo_after_intent"), &envs);
    let client = support::connect(profile.path()).await;
    connect(&client).await;
    share(&client, "Letters", &out, true).await;
    fake.push(Reply::calls(&[(
        "propose_document",
        json!({"folder": "Letters", "file_name": "Cover letter", "format": "docx", "body": "Dear team,

Hello."}),
    )]));
    fake.push(Reply::text(&["The letter is saved."]));
    let sent = send(&client, None, "undo", "Write the letter").await;
    let run = settled(&client, &sent).await.proposed_run_id.unwrap();
    let pending = wait_run(&client, run, RunStatus::AwaitingApproval)
        .await
        .pending_approval
        .unwrap();
    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
            via: nexus_protocol::ApprovalPath::TrustedDialog,
        })
        .await
        .unwrap();
    wait_run(&client, run, RunStatus::Succeeded).await;
    let target = out.path().join("Cover letter.docx");
    assert!(target.exists());
    let _ = client.undo_effect(run).await;
    assert!(
        !support::wait_exit(&mut host).success(),
        "the host aborted once the undo's intent was written"
    );
    assert!(target.exists(), "nothing was deleted before the crash");

    // After the restart the undo is decided from the file: still there and
    // unchanged, so it did not happen; the user can undo again.
    let mut host = support::spawn_host_env(profile.path(), None, &envs);
    let client = support::connect(profile.path()).await;
    let view = wait_run(&client, run, RunStatus::Succeeded).await;
    assert!(view.can_undo && view.undone_at_ms.is_none(), "{view:?}");
    let undone = client.undo_effect(run).await.unwrap();
    assert!(!target.exists());
    assert!(undone.undone_at_ms.is_some());
    client.stop_host().await.ok();
    support::wait_exit(&mut host);
}
