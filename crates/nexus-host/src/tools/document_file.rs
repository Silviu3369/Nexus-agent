//! `CreateDocument`: create one new file — Word (.docx), Markdown or text —
//! in a folder the user shared and allowed writing to. The bytes are built
//! deterministically from the title and body the user approves, so the
//! approval covers exactly what is written. Like `CreateMarkdownArtifact`:
//! stage next to the target, flush, publish without replacing (a hard link;
//! on a drive without hard links, a create-new write), then re-read and
//! check size and SHA-256. Nothing is ever overwritten.

use std::fs::{self, OpenOptions};
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};

use crate::documents;
use crate::tools::markdown_artifact::sha256_hex;

pub const TOOL_ID: &str = "CreateDocument";
pub const TOOL_VERSION: u32 = 1;
pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_NAME_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocFormat {
    Word,
    Markdown,
    Text,
}

impl DocFormat {
    pub fn parse(s: &str) -> Option<DocFormat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "docx" | "word" | "" => Some(DocFormat::Word),
            "md" | "markdown" => Some(DocFormat::Markdown),
            "txt" | "text" => Some(DocFormat::Text),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            DocFormat::Word => "docx",
            DocFormat::Markdown => "md",
            DocFormat::Text => "txt",
        }
    }
}

/// The exact file the user approves: its bytes, and the text shown for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentContent {
    pub bytes: Vec<u8>,
    pub preview: String,
}

impl DocumentContent {
    pub fn sha256_hex(&self) -> String {
        sha256_hex(&self.bytes)
    }

    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// `name` plus the format's extension, checked as one plain file name.
pub fn file_name(name: &str, format: DocFormat) -> Result<String, String> {
    let stem = name.trim();
    let ext = format!(".{}", format.extension());
    let stem = if stem.to_ascii_lowercase().ends_with(&ext) {
        &stem[..stem.len() - ext.len()]
    } else {
        stem
    };
    let stem = stem.trim();
    if stem.is_empty() || stem.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "a file name must have 1..={MAX_NAME_CHARS} characters"
        ));
    }
    let full = format!("{stem}{ext}");
    documents::plain_name(&full)?;
    Ok(full)
}

fn clean(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

pub fn build(format: DocFormat, title: &str, body: &str) -> Result<DocumentContent, String> {
    let title = clean(title).trim().to_string();
    if title.chars().count() > MAX_TITLE_CHARS || title.contains('\n') {
        return Err(format!(
            "a title must be one line of at most {MAX_TITLE_CHARS} characters"
        ));
    }
    let body = clean(body).trim_end().to_string();
    if body.trim().is_empty() {
        return Err("the document has no text".into());
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(format!(
            "the text is {} bytes; at most {MAX_BODY_BYTES}",
            body.len()
        ));
    }
    let preview = if title.is_empty() {
        format!("{body}\n")
    } else {
        format!("{title}\n\n{body}\n")
    };
    let bytes = match format {
        DocFormat::Text => preview.clone().into_bytes(),
        DocFormat::Markdown if title.is_empty() => preview.clone().into_bytes(),
        DocFormat::Markdown => format!("# {title}\n\n{body}\n").into_bytes(),
        DocFormat::Word => {
            docx(&title, &body).map_err(|e| format!("the Word file could not be built: {e}"))?
        }
    };
    Ok(DocumentContent { bytes, preview })
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// One line as Word runs: text, with tabs as `<w:tab/>`.
fn runs_of(line: &str) -> String {
    line.split('\t')
        .map(|part| format!("<w:t xml:space=\"preserve\">{}</w:t>", xml_escape(part)))
        .collect::<Vec<_>>()
        .join("<w:tab/>")
}

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#;
const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
const DOCUMENT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;

fn styles() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Calibri"/><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults></w:styles>"#
    )
}

fn document_xml(title: &str, body: &str) -> String {
    let mut paragraphs = String::new();
    if !title.is_empty() {
        paragraphs.push_str(&format!(
            "<w:p><w:pPr><w:spacing w:after=\"240\"/></w:pPr><w:r><w:rPr><w:b/><w:sz w:val=\"32\"/><w:szCs w:val=\"32\"/></w:rPr>{}</w:r></w:p>",
            runs_of(title)
        ));
    }
    // A blank line separates paragraphs; a single line break stays a break.
    for paragraph in body.split("\n\n") {
        let lines: Vec<String> = paragraph
            .trim_matches('\n')
            .split('\n')
            .map(runs_of)
            .collect();
        paragraphs.push_str(&format!("<w:p><w:r>{}</w:r></w:p>", lines.join("<w:br/>")));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="{W}"><w:body>{paragraphs}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    )
}

/// A minimal Word document; the same text always gives the same bytes.
fn docx(title: &str, body: &str) -> zip::result::ZipResult<Vec<u8>> {
    use zip::write::SimpleFileOptions;
    let time = zip::DateTime::default();
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(time);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let parts = [
        ("[Content_Types].xml", CONTENT_TYPES.to_string()),
        ("_rels/.rels", ROOT_RELS.to_string()),
        ("word/_rels/document.xml.rels", DOCUMENT_RELS.to_string()),
        ("word/styles.xml", styles()),
        ("word/document.xml", document_xml(title, body)),
    ];
    for (name, content) in parts {
        writer.start_file(name, options)?;
        writer.write_all(content.as_bytes())?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Where one operation writes, inside the shared folder.
#[derive(Debug, Clone)]
pub struct Destination {
    pub root: PathBuf,
    pub file_name: String,
    /// Next to the target, so publishing is a link on the same volume.
    pub staging_name: String,
}

impl Destination {
    pub fn new(root: &Path, file_name: &str, operation: &str) -> Self {
        Self {
            root: root.to_path_buf(),
            file_name: file_name.to_string(),
            staging_name: format!("~nexus-{operation}.tmp"),
        }
    }

    pub fn target(&self) -> PathBuf {
        self.root.join(&self.file_name)
    }

    fn staging(&self) -> PathBuf {
        self.root.join(&self.staging_name)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Observed {
    NoEffect,
    Complete,
    Uncertain(String),
}

fn root_ok(root: &Path) -> io::Result<()> {
    match documents::folder_problem(root) {
        None => Ok(()),
        Some(problem) => Err(io::Error::other(problem)),
    }
}

pub fn create(dest: &Destination, content: &DocumentContent) -> io::Result<()> {
    root_ok(&dest.root)?;
    let target = dest.target();
    if fs::symlink_metadata(&target).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the file already exists; nothing is overwritten",
        ));
    }
    let staging = dest.staging();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    file.write_all(&content.bytes)?;
    file.sync_all()?;
    drop(file);
    root_ok(&dest.root)?;
    match fs::hard_link(&staging, &target) {
        Ok(()) => {
            fs::remove_file(&staging)?;
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&staging);
            return Err(err);
        }
        Err(_) => {
            // No hard links on this drive (some cloud drives): create the
            // target new — still never replacing an existing file.
            fs::remove_file(&staging)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            file.write_all(&content.bytes)?;
            file.sync_all()?;
        }
    }
    match observe(dest, content) {
        Observed::Complete => Ok(()),
        Observed::NoEffect => Err(io::Error::other("the file is missing right after writing")),
        Observed::Uncertain(reason) => Err(io::Error::other(reason)),
    }
}

pub fn observe(dest: &Destination, expected: &DocumentContent) -> Observed {
    let target = dest.target();
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Observed::Uncertain("the target is a link".into())
        }
        Ok(meta) if meta.len() != expected.size() => Observed::Uncertain(format!(
            "the file is {} bytes, expected {}",
            meta.len(),
            expected.size()
        )),
        Ok(_) => match fs::read(&target) {
            Ok(bytes) if sha256_hex(&bytes) == expected.sha256_hex() => Observed::Complete,
            Ok(_) => Observed::Uncertain("the file differs from the approved one".into()),
            Err(err) => Observed::Uncertain(format!("the file is unreadable: {err}")),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if dest.staging().exists() {
                Observed::Uncertain("a staging file exists but was never published".into())
            } else {
                Observed::NoEffect
            }
        }
        Err(err) => Observed::Uncertain(format!("the file's state is unknown: {err}")),
    }
}

pub fn discard_staging(dest: &Destination) {
    let _ = fs::remove_file(dest.staging());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_text_always_gives_the_same_word_file_and_it_reads_back() {
        let a = build(
            DocFormat::Word,
            "Cover letter",
            "Dear team,\n\nI apply for\tthe role & more.",
        )
        .unwrap();
        let b = build(
            DocFormat::Word,
            "Cover letter",
            "Dear team,\n\nI apply for\tthe role & more.",
        )
        .unwrap();
        assert_eq!(a.bytes, b.bytes, "deterministic");
        let text = documents::docx_text(&a.bytes).unwrap();
        assert_eq!(
            text,
            "Cover letter\nDear team,\nI apply for\tthe role & more.\n"
        );
        assert_eq!(
            a.preview,
            "Cover letter\n\nDear team,\n\nI apply for\tthe role & more.\n"
        );
        let md = build(DocFormat::Markdown, "Plan", "Step 1").unwrap();
        assert_eq!(md.bytes, b"# Plan\n\nStep 1\n");
        assert!(build(DocFormat::Text, "", "  ").is_err());
        assert!(build(DocFormat::Text, "two\nlines", "x").is_err());
    }

    #[test]
    fn names_get_their_extension_and_unsafe_ones_are_refused() {
        assert_eq!(
            file_name("Cover letter Acme", DocFormat::Word).unwrap(),
            "Cover letter Acme.docx"
        );
        assert_eq!(
            file_name("notes.md", DocFormat::Markdown).unwrap(),
            "notes.md"
        );
        for bad in ["../x", "a/b", "con", "x:y", " "] {
            assert!(file_name(bad, DocFormat::Word).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_document_is_created_once_and_never_replaces_a_file() {
        let root = std::env::temp_dir().join(format!("nexus-doc-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let content = build(DocFormat::Word, "Letter", "Hello").unwrap();
        let dest = Destination::new(&root, "Letter.docx", "op1");
        assert_eq!(observe(&dest, &content), Observed::NoEffect);
        create(&dest, &content).unwrap();
        assert_eq!(observe(&dest, &content), Observed::Complete);
        assert!(!root.join("~nexus-op1.tmp").exists());
        let again = create(&Destination::new(&root, "Letter.docx", "op2"), &content).unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        fs::write(root.join("Letter.docx"), b"changed").unwrap();
        assert!(matches!(observe(&dest, &content), Observed::Uncertain(_)));
        let _ = fs::remove_dir_all(&root);
    }
}
