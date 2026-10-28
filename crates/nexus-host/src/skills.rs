//! Skills in the open Agent Skills format (ADR-PACK-001; the specification
//! at agentskills.io, checked 2026-10-28): reading one from a folder or a
//! `.zip`, checking its `SKILL.md` header, and what Nexus keeps of it. A
//! skill is text: its scripts are never run (nor kept), and its
//! `allowed-tools` approves nothing.
//!
//! The header is YAML, read here by a small parser for the fields the
//! specification defines (plain, quoted and block values; `metadata` as one
//! level of names and values). What it cannot read is refused with the
//! reason, never guessed at.

use std::fs;
use std::io::{Cursor, Read};
use std::os::windows::fs::MetadataExt;
use std::path::Path;

use sha2::{Digest, Sha256};

pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_COMPATIBILITY_CHARS: usize = 500;
const MAX_LICENSE_CHARS: usize = 500;
const MAX_ALLOWED_TOOLS_CHARS: usize = 1000;
/// The instructions after the header (the specification recommends under
/// 5,000 tokens).
pub const MAX_INSTRUCTIONS_CHARS: usize = 20_000;
/// Text files kept from one skill, and their size in all.
pub const MAX_FILES: usize = 50;
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024;
/// Skills on at the same time.
pub const MAX_ENABLED: usize = 20;
/// A `.zip` larger than this is not opened.
const MAX_ZIP_BYTES: u64 = 10 * 1024 * 1024;
/// Entries looked at in a `.zip` or a folder (kept or not).
const MAX_ENTRIES: usize = 500;
const MAX_DEPTH: usize = 4;
/// Skipped files listed by name; more are counted.
const MAX_SKIPPED_SHOWN: usize = 20;
const TEXT_EXTENSIONS: [&str; 4] = ["md", "txt", "csv", "json"];
pub const INSTRUCTIONS_FILE: &str = "SKILL.md";

/// The fields of a `SKILL.md` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: Vec<(String, String)>,
    pub allowed_tools: Option<String>,
}

/// A skill read and checked, ready to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub header: Header,
    /// `SKILL.md` first, then the other text files, by path.
    pub files: Vec<(String, String)>,
    pub has_scripts: bool,
    pub skipped: Vec<String>,
    pub sha256: String,
}

/// The specification's rule for a name: 1–64 characters, lowercase letters,
/// digits and single hyphens, not at either end.
pub fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--");
    if ok {
        Ok(())
    } else {
        Err(format!(
            "the skill's name {name:?} is not valid: 1 to {MAX_NAME_CHARS} lowercase letters, digits and \
             single hyphens, not at the start or the end"
        ))
    }
}

/// The header and the instructions of a `SKILL.md`.
pub fn parse_skill_md(text: &str) -> Result<(Header, String), String> {
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let mut lines = text.split('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return Err("SKILL.md must start with a header between two \"---\" lines".into());
    }
    let mut header_lines = Vec::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        header_lines.push(line);
    }
    if !closed {
        return Err("SKILL.md's header has no closing \"---\" line".into());
    }
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let header = parse_header(&header_lines)?;
    if body.is_empty() {
        return Err("SKILL.md has no instructions after its header".into());
    }
    if body.chars().count() > MAX_INSTRUCTIONS_CHARS {
        return Err(format!(
            "the instructions are longer than {MAX_INSTRUCTIONS_CHARS} characters; move details into \
             files under references/"
        ));
    }
    Ok((header, body))
}

fn parse_header(lines: &[&str]) -> Result<Header, String> {
    let indented = |l: &str| l.starts_with(' ') || l.starts_with('\t');
    let mut fields: Vec<(String, Field)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let at = i + 2; // the header starts on the file's second line
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            i += 1;
            continue;
        }
        if indented(line) {
            return Err(format!("SKILL.md line {at}: unexpected indentation"));
        }
        let (key, rest) = line
            .split_once(':')
            .ok_or_else(|| format!("SKILL.md line {at}: expected \"name: value\""))?;
        let key = key.trim().to_string();
        let rest = rest.trim();
        let mut end = i + 1;
        while end < lines.len() && (indented(lines[end]) || lines[end].trim().is_empty()) {
            end += 1;
        }
        let mut block: Vec<&str> = lines[i + 1..end].to_vec();
        while block.last().is_some_and(|l| l.trim().is_empty()) {
            block.pop();
        }
        let field = if key == "metadata" && rest.is_empty() {
            Field::Map(parse_map(&block, at)?)
        } else if ["|", "|-", "|+", ">", ">-", ">+"].contains(&rest) {
            Field::Text(block_scalar(rest.starts_with('|'), &block))
        } else if rest.is_empty() {
            Field::Text(fold(&block))
        } else if !block.is_empty() && (rest.starts_with('"') || rest.starts_with('\'')) {
            return Err(format!(
                "SKILL.md line {at}: a quoted value must end on its own line"
            ));
        } else {
            let first = scalar(rest).map_err(|e| format!("SKILL.md line {at}: {e}"))?;
            if block.is_empty() {
                Field::Text(first)
            } else {
                Field::Text(format!("{first} {}", fold(&block)))
            }
        };
        if fields.iter().any(|(k, _)| *k == key) {
            return Err(format!("SKILL.md line {at}: {key} is given twice"));
        }
        fields.push((key, field));
        i = end;
    }

    let mut text = |key: &str| -> Result<Option<String>, String> {
        match fields.iter().position(|(k, _)| k == key) {
            None => Ok(None),
            Some(p) => match fields.remove(p).1 {
                Field::Text(t) => Ok(Some(t.trim().to_string()).filter(|t| !t.is_empty())),
                Field::Map(_) => Err(format!("SKILL.md: {key} must be text")),
            },
        }
    };
    let name = text("name")?.ok_or("SKILL.md's header has no name")?;
    let description = text("description")?.ok_or("SKILL.md's header has no description")?;
    let license = text("license")?;
    let compatibility = text("compatibility")?;
    let allowed_tools = text("allowed-tools")?;
    let metadata = match fields.iter().position(|(k, _)| k == "metadata") {
        None => Vec::new(),
        Some(p) => match fields.remove(p).1 {
            Field::Map(m) => m,
            Field::Text(_) => return Err("SKILL.md: metadata must map names to values".into()),
        },
    };
    check_name(&name)?;
    let longer = |what: &str, value: &Option<String>, max: usize| match value {
        Some(v) if v.chars().count() > max => Err(format!(
            "the skill's {what} is longer than {max} characters"
        )),
        _ => Ok(()),
    };
    longer(
        "description",
        &Some(description.clone()),
        MAX_DESCRIPTION_CHARS,
    )?;
    longer("license", &license, MAX_LICENSE_CHARS)?;
    longer("compatibility", &compatibility, MAX_COMPATIBILITY_CHARS)?;
    longer("allowed-tools", &allowed_tools, MAX_ALLOWED_TOOLS_CHARS)?;
    // Other fields have no meaning for Nexus and are left out.
    Ok(Header {
        name,
        description,
        license,
        compatibility,
        metadata,
        allowed_tools,
    })
}

enum Field {
    Text(String),
    Map(Vec<(String, String)>),
}

/// A plain or quoted value on one line.
fn scalar(raw: &str) -> Result<String, String> {
    if let Some(inner) = raw.strip_prefix('"') {
        let inner = inner
            .strip_suffix('"')
            .ok_or("a value opened with \" is not closed")?;
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(c @ ('"' | '\\' | '/')) => out.push(c),
                _ => return Err("a value has an escape Nexus does not read".into()),
            }
        }
        Ok(out)
    } else if let Some(inner) = raw.strip_prefix('\'') {
        let inner = inner
            .strip_suffix('\'')
            .ok_or("a value opened with ' is not closed")?;
        Ok(inner.replace("''", "'"))
    } else if raw.starts_with(['[', '{', '&', '*', '!']) {
        Err("lists, maps, anchors and tags are not read in a value".into())
    } else {
        // A comment starts at " #".
        Ok(raw.split(" #").next().unwrap_or("").trim().to_string())
    }
}

/// `|` keeps the lines, `>` folds them into one paragraph (a blank line
/// starts a new one).
fn block_scalar(literal: bool, block: &[&str]) -> String {
    let indent = block
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let lines: Vec<&str> = block
        .iter()
        .map(|l| if l.len() >= indent { &l[indent..] } else { "" })
        .collect();
    if literal {
        lines.join("\n")
    } else {
        fold(&lines)
    }
}

fn fold(lines: &[&str]) -> String {
    lines
        .split(|l| l.trim().is_empty())
        .map(|p| p.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn parse_map(block: &[&str], at: usize) -> Result<Vec<(String, String)>, String> {
    let indent = block
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len());
    let mut out: Vec<(String, String)> = Vec::new();
    for line in block.iter().filter(|l| !l.trim().is_empty()) {
        if Some(line.len() - line.trim_start().len()) != indent {
            return Err(format!(
                "SKILL.md near line {at}: metadata must be one level of names and values"
            ));
        }
        let (key, value) = line.trim().split_once(':').ok_or_else(|| {
            format!("SKILL.md near line {at}: metadata must be one level of names and values")
        })?;
        let value = scalar(value.trim()).map_err(|e| format!("SKILL.md near line {at}: {e}"))?;
        out.push((key.trim().to_string(), value));
    }
    Ok(out)
}

/// One file found in a skill, before sorting what is kept.
struct Found {
    path: String,
    size: u64,
    read: Box<dyn FnOnce(u64) -> Result<Vec<u8>, String>>,
}

/// Reads the skill in `path`: the folder that holds `SKILL.md`, or a `.zip`
/// of it (the form claude.ai takes).
pub fn read_skill(path: &Path) -> Result<Imported, String> {
    let meta = fs::metadata(path).map_err(|_| {
        "nothing to import there: give the folder that holds SKILL.md, or a .zip of it".to_string()
    })?;
    let is_zip = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let (folder, found) = if meta.is_dir() {
        let folder = path.file_name().map(|n| n.to_string_lossy().into_owned());
        (folder, folder_files(path)?)
    } else if meta.is_file() && is_zip {
        zip_files(path, meta.len())?
    } else {
        return Err(
            "give the folder that holds SKILL.md, or a .zip of it (as uploaded to claude.ai)"
                .into(),
        );
    };
    collect(folder, found)
}

fn folder_files(root: &Path) -> Result<Vec<Found>, String> {
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const REPARSE_POINT: u32 = 0x400;
    let root_meta =
        fs::symlink_metadata(root).map_err(|e| format!("the folder cannot be read: {e}"))?;
    if root_meta.file_attributes() & REPARSE_POINT != 0 {
        return Err("the folder is a link; give the folder itself".into());
    }
    let mut found = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("the folder cannot be read: {e}"))?;
        for entry in entries.filter_map(Result::ok) {
            seen += 1;
            if seen > MAX_ENTRIES {
                return Err(format!(
                    "the skill's folder holds more than {MAX_ENTRIES} items"
                ));
            }
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let attributes = meta.file_attributes();
            if attributes & (HIDDEN | SYSTEM) != 0 {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if attributes & REPARSE_POINT != 0 || meta.file_type().is_symlink() {
                found.push(Found {
                    path: relative,
                    size: 0,
                    read: Box::new(|_| Err("a link, not followed".into())),
                });
            } else if meta.is_dir() {
                if depth + 1 < MAX_DEPTH {
                    stack.push((entry.path(), depth + 1));
                }
            } else {
                let file = entry.path();
                found.push(Found {
                    path: relative,
                    size: meta.len(),
                    read: Box::new(move |max| {
                        let handle = fs::File::open(&file).map_err(|e| e.to_string())?;
                        let mut bytes = Vec::new();
                        handle
                            .take(max + 1)
                            .read_to_end(&mut bytes)
                            .map_err(|e| e.to_string())?;
                        Ok(bytes)
                    }),
                });
            }
        }
    }
    Ok(found)
}

/// The files of a `.zip`: under its one top folder when it has one (that
/// folder's name must be the skill's), otherwise at its root.
fn zip_files(path: &Path, size: u64) -> Result<(Option<String>, Vec<Found>), String> {
    if size > MAX_ZIP_BYTES {
        return Err(format!(
            "the .zip is larger than {} MB",
            MAX_ZIP_BYTES / 1024 / 1024
        ));
    }
    let bytes = fs::read(path).map_err(|e| format!("the .zip cannot be read: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes.clone())).map_err(|_| "not a valid .zip")?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!("the .zip holds more than {MAX_ENTRIES} items"));
    }
    let mut names = Vec::new();
    for i in 0..archive.len() {
        let file = archive.by_index(i).map_err(|_| "not a valid .zip")?;
        if file.is_dir() {
            continue;
        }
        let name = file
            .enclosed_name()
            .ok_or("the .zip names a file outside itself")?
            .to_string_lossy()
            .replace('\\', "/");
        names.push((i, name, file.size()));
    }
    let tops: std::collections::BTreeSet<&str> = names
        .iter()
        .map(|(_, n, _)| n.split('/').next().unwrap_or(""))
        .collect();
    let folder = match tops.iter().next() {
        Some(top) if tops.len() == 1 && names.iter().all(|(_, n, _)| n.contains('/')) => {
            Some(top.to_string())
        }
        _ => None,
    };
    let shared = std::rc::Rc::new(std::cell::RefCell::new(archive));
    let found = names
        .into_iter()
        .map(|(i, name, size)| {
            let path = match &folder {
                Some(top) => name[top.len() + 1..].to_string(),
                None => name,
            };
            let archive = shared.clone();
            Found {
                path,
                size,
                read: Box::new(move |max| {
                    let mut archive = archive.borrow_mut();
                    let file = archive
                        .by_index(i)
                        .map_err(|_| "not a valid .zip".to_string())?;
                    let mut bytes = Vec::new();
                    file.take(max + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|_| "unreadable in the .zip".to_string())?;
                    Ok(bytes)
                }),
            }
        })
        .collect();
    Ok((folder, found))
}

fn collect(folder: Option<String>, mut found: Vec<Found>) -> Result<Imported, String> {
    found.sort_by(|a, b| a.path.cmp(&b.path));
    let instructions = found
        .iter()
        .position(|f| f.path.eq_ignore_ascii_case(INSTRUCTIONS_FILE))
        .ok_or("no SKILL.md at the top of the skill")?;
    let mut has_scripts = false;
    let mut skipped = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    let mut total = 0u64;
    let mut skip = |path: &str, why: &str| skipped.push(format!("{path}: {why}"));

    let main = found.remove(instructions);
    let text = read_text(main, MAX_TOTAL_BYTES)?;
    total += text.len() as u64;
    let (header, _) = parse_skill_md(&text)?;
    if let Some(folder) = &folder
        && *folder != header.name
    {
        return Err(format!(
            "the skill's folder is named {folder:?} but SKILL.md names it {:?}; they must match",
            header.name
        ));
    }
    files.push((INSTRUCTIONS_FILE.to_string(), text));

    for file in found {
        let lower = file.path.to_ascii_lowercase();
        if lower.starts_with("scripts/") {
            has_scripts = true;
            skip(&file.path, "scripts never run in Nexus");
            continue;
        }
        let text_kind = Path::new(&lower)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| TEXT_EXTENSIONS.contains(&e));
        if !text_kind {
            skip(
                &file.path,
                "not a text file Nexus reads (.md, .txt, .csv, .json)",
            );
            continue;
        }
        if files.len() >= MAX_FILES {
            return Err(format!("the skill has more than {MAX_FILES} text files"));
        }
        if total + file.size > MAX_TOTAL_BYTES {
            return Err(format!(
                "the skill's text files are larger than {} MiB in all",
                MAX_TOTAL_BYTES / 1024 / 1024
            ));
        }
        let path = file.path.clone();
        match read_text(file, MAX_TOTAL_BYTES - total) {
            Ok(text) => {
                total += text.len() as u64;
                files.push((path, text));
            }
            Err(why) => skip(&path, &why),
        }
    }
    if skipped.len() > MAX_SKIPPED_SHOWN {
        let more = skipped.len() - MAX_SKIPPED_SHOWN;
        skipped.truncate(MAX_SKIPPED_SHOWN);
        skipped.push(format!("and {more} more"));
    }
    let sha256 = content_sha256(&files);
    Ok(Imported {
        header,
        files,
        has_scripts,
        skipped,
        sha256,
    })
}

fn read_text(file: Found, max: u64) -> Result<String, String> {
    let bytes = (file.read)(max)?;
    if bytes.len() as u64 > max {
        return Err("larger than the skill's limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())
}

/// What the user reads and turns on: every kept file, by path.
pub fn content_sha256(files: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (path, text) in sorted {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update((text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The instructions of a stored `SKILL.md` (after its header).
pub fn instructions(skill_md: &str) -> String {
    parse_skill_md(skill_md)
        .map(|(_, body)| body)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "---\nname: cv-letter\ndescription: Writes cover letters. Use when the user asks for one.\n---\n\n# Letter\nWrite it in Dutch.\n";

    #[test]
    fn names_follow_the_specification() {
        for good in ["pdf-processing", "a", "cover-letters", "x2"] {
            check_name(good).unwrap();
        }
        for bad in [
            "PDF",
            "-pdf",
            "pdf-",
            "pdf--x",
            "",
            "pdf_x",
            "pdf x",
            &"a".repeat(65),
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_header_reads_plain_quoted_block_and_metadata_values() {
        let (header, body) = parse_skill_md(SKILL).unwrap();
        assert_eq!(header.name, "cv-letter");
        assert_eq!(
            header.description,
            "Writes cover letters. Use when the user asks for one."
        );
        assert_eq!(body, "# Letter\nWrite it in Dutch.");

        let text = "\u{feff}---\r\nname: \"pdf-tools\"\r\ndescription: >\r\n  Extracts text\r\n  from PDFs.\r\n\r\n  Use for forms.\r\nlicense: 'Apache-2.0'\r\nmetadata:\r\n  author: example-org\r\n  version: \"1.0\"\r\nallowed-tools: Bash(git:*) Read\r\ncompatibility: |\r\n  Needs:\r\n  - internet\r\nunknown: ignored # a comment\r\n---\r\nDo it.\r\n";
        let (header, body) = parse_skill_md(text).unwrap();
        assert_eq!(header.name, "pdf-tools");
        assert_eq!(
            header.description,
            "Extracts text from PDFs.\nUse for forms."
        );
        assert_eq!(header.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(header.compatibility.as_deref(), Some("Needs:\n- internet"));
        assert_eq!(header.allowed_tools.as_deref(), Some("Bash(git:*) Read"));
        assert_eq!(
            header.metadata,
            [
                ("author".to_string(), "example-org".to_string()),
                ("version".to_string(), "1.0".to_string())
            ]
        );
        assert_eq!(body, "Do it.");
    }

    #[test]
    fn what_cannot_be_read_is_refused_with_the_reason() {
        let cases = [
            ("no header", "Just text"),
            ("no closing", "---\nname: a\ndescription: b\n"),
            ("no name", "---\ndescription: b\n---\nx"),
            ("no description", "---\nname: a\n---\nx"),
            ("bad name", "---\nname: Bad_Name\ndescription: b\n---\nx"),
            ("twice", "---\nname: a\nname: b\ndescription: c\n---\nx"),
            ("list", "---\nname: a\ndescription: [b]\n---\nx"),
            ("no body", "---\nname: a\ndescription: b\n---\n\n"),
            (
                "deep metadata",
                "---\nname: a\ndescription: b\nmetadata:\n  a:\n    b: c\n---\nx",
            ),
        ];
        for (what, text) in cases {
            assert!(parse_skill_md(text).is_err(), "{what}");
        }
        let long = format!("---\nname: a\ndescription: {}\n---\nx", "d".repeat(1025));
        assert!(parse_skill_md(&long).is_err());
        let big = format!("---\nname: a\ndescription: b\n---\n{}", "x".repeat(20_001));
        assert!(parse_skill_md(&big).unwrap_err().contains("references/"));
    }

    #[test]
    fn the_hash_covers_every_path_and_text_in_any_order() {
        let a = vec![
            ("SKILL.md".to_string(), "x".to_string()),
            ("references/a.md".to_string(), "y".to_string()),
        ];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(content_sha256(&a), content_sha256(&b));
        let mut c = a.clone();
        c[1].1 = "z".to_string();
        assert_ne!(content_sha256(&a), content_sha256(&c));
        let mut d = a.clone();
        d[1].0 = "references/b.md".to_string();
        assert_ne!(content_sha256(&a), content_sha256(&d));
    }
}
