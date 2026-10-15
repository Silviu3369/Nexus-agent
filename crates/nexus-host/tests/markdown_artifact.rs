//! `CreateMarkdownArtifact` on a real NTFS directory: never overwrites,
//! verifies by hash, and refuses paths redirected by a real junction (PATH-01).

use std::path::{Path, PathBuf};
use std::process::Command;

use nexus_host::tools::markdown_artifact::{
    self as tool, ArtifactError, ArtifactPaths, NoteContent, Observed,
};
use nexus_protocol::{OperationId, ProjectId};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("nexus-artifact-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn store(dir: &TempDir) -> PathBuf {
    dir.path().join("artifacts")
}

fn paths(root: &Path) -> ArtifactPaths {
    ArtifactPaths::for_operation(root, ProjectId::new(), OperationId::new())
}

fn junction(link: &Path, target: &Path) {
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("cmd must run");
    assert!(
        status.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

#[test]
fn creates_exact_content_verified_by_hash_with_no_staging_left() {
    let dir = TempDir::new();
    let p = paths(&store(&dir));
    let note = NoteContent::new("Lista de cumpărături", "- lapte\n- pâine").unwrap();

    tool::create(&p, &note).unwrap();

    let written = std::fs::read_to_string(p.target()).unwrap();
    assert_eq!(written, "# Lista de cumpărături\n\n- lapte\n- pâine\n");
    assert_eq!(tool::sha256_hex(written.as_bytes()), note.sha256_hex());
    assert!(
        !p.staging().exists(),
        "staging name must be removed after publishing"
    );
    assert_eq!(tool::observe(&p, &note), Observed::Complete);
}

#[test]
fn never_overwrites_an_existing_target() {
    let dir = TempDir::new();
    let p = paths(&store(&dir));
    let first = NoteContent::new("Prima", "original").unwrap();
    tool::create(&p, &first).unwrap();

    let second = NoteContent::new("A doua", "would replace").unwrap();
    match tool::create(&p, &second) {
        Err(ArtifactError::AlreadyExists(_)) => {}
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(p.target()).unwrap(),
        first.text,
        "the existing file must be left exactly as it was"
    );
}

#[test]
fn user_text_never_becomes_part_of_the_path() {
    let dir = TempDir::new();
    let root = store(&dir);
    let project = ProjectId::new();
    let operation = OperationId::new();
    let p = ArtifactPaths::for_operation(&root, project, operation);

    // Traversal, absolute, device, UNC, ADS and reserved names — all inert text.
    let hostile =
        r"..\..\Windows\System32\evil.md C:\x \\?\GLOBALROOT \\server\share CON file.md:stream";
    let note = NoteContent::new(hostile, hostile).unwrap();
    tool::create(&p, &note).unwrap();

    assert_eq!(p.target_relpath, format!("{project}/{operation}.md"));
    assert!(p.target().starts_with(&root));
    let entries: Vec<_> = std::fs::read_dir(root.join(project.to_string()))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(entries.contains(&format!("{operation}.md")));
    assert!(
        std::fs::read_to_string(p.target())
            .unwrap()
            .contains(hostile)
    );
}

#[test]
fn junction_in_place_of_the_project_directory_is_refused() {
    let dir = TempDir::new();
    let root = store(&dir);
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    let project = ProjectId::new();
    junction(&root.join(project.to_string()), &outside);

    let p = ArtifactPaths::for_operation(&root, project, OperationId::new());
    let note = NoteContent::new("Notă", "conținut").unwrap();
    match tool::create(&p, &note) {
        Err(ArtifactError::ReparsePoint(_)) => {}
        other => panic!("expected ReparsePoint, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_dir(&outside).unwrap().count(),
        0,
        "nothing may be written through the junction"
    );
}

#[test]
fn junction_in_place_of_the_staging_directory_is_refused() {
    let dir = TempDir::new();
    let root = store(&dir);
    let outside = dir.path().join("outside");
    let project = ProjectId::new();
    std::fs::create_dir_all(root.join(project.to_string())).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    junction(&root.join(project.to_string()).join(".staging"), &outside);

    let p = ArtifactPaths::for_operation(&root, project, OperationId::new());
    let note = NoteContent::new("Notă", "conținut").unwrap();
    assert!(matches!(
        tool::create(&p, &note),
        Err(ArtifactError::ReparsePoint(_))
    ));
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn observe_distinguishes_no_effect_partial_and_foreign_content() {
    let dir = TempDir::new();
    let p = paths(&store(&dir));
    let note = NoteContent::new("Notă", "conținut complet").unwrap();

    assert_eq!(tool::observe(&p, &note), Observed::NoEffect);

    // Crash after staging, before publishing.
    std::fs::create_dir_all(p.staging().parent().unwrap()).unwrap();
    std::fs::write(p.staging(), &note.text[..5]).unwrap();
    assert!(matches!(tool::observe(&p, &note), Observed::Uncertain(_)));

    // A partial target is never treated as success.
    std::fs::write(p.target(), &note.text[..5]).unwrap();
    assert!(matches!(tool::observe(&p, &note), Observed::Uncertain(_)));

    // Same size, different bytes: not ours to claim.
    let forged = "x".repeat(note.text.len());
    std::fs::write(p.target(), forged).unwrap();
    assert!(matches!(tool::observe(&p, &note), Observed::Uncertain(_)));
}

#[test]
fn content_limits_are_enforced() {
    assert!(matches!(
        NoteContent::new("", "x"),
        Err(ArtifactError::InvalidTitle)
    ));
    assert!(matches!(
        NoteContent::new("line\nbreak", "x"),
        Err(ArtifactError::InvalidTitle)
    ));
    assert!(matches!(
        NoteContent::new(&"t".repeat(201), "x"),
        Err(ArtifactError::InvalidTitle)
    ));
    assert!(matches!(
        NoteContent::new("ok", &"x".repeat(tool::MAX_CONTENT_BYTES)),
        Err(ArtifactError::TooLarge(_))
    ));
}
