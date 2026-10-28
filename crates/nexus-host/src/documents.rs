//! The user's documents (`PLAN_NEXUS_RO.md` §3a, step 2): which folders may
//! be shared, how a path inside one is resolved (never outside it, never
//! through a link), and how a document's text is read. Parsing happens in a
//! separate process (`nexus-host extract`) inside a Job Object with a memory
//! cap and no children, under a time limit: a malformed or hostile file can
//! only fail its own read, never stall or crash the Host. That process is
//! containment, not a sandbox: it reads only the one file it is given.

use std::fs;
use std::io::{Cursor, Read};
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

/// Largest document read.
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
/// Text given to the model from one document in one read (about 10,000
/// tokens). A longer document is read on in further parts, from where the
/// last one ended, only when the task needs it.
pub const MAX_TEXT_CHARS: usize = 40_000;
/// Rows read from a spreadsheet or CSV for an import.
pub const MAX_TABLE_ROWS: usize = 2_000;
const MAX_COLUMNS: usize = 60;
const MAX_CELL_CHARS: usize = 4_000;
const MAX_SHEETS: usize = 10;
/// Largest part unpacked from a Word file (guards against zip bombs).
const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(20);
const EXTRACT_MEMORY_BYTES: usize = 512 * 1024 * 1024;
/// What the parent reads back from the child at most.
const MAX_CHILD_OUTPUT: usize = 8 * 1024 * 1024;
/// Entries listed from one folder (a few levels deep).
pub const MAX_ENTRIES: usize = 300;
const MAX_DEPTH: usize = 3;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Text,
    Word,
    Pdf,
    Spreadsheet,
}

impl Format {
    pub fn of(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "log" | "text" => Format::Text,
            "docx" => Format::Word,
            "pdf" => Format::Pdf,
            "xlsx" | "xlsm" | "xls" | "ods" => Format::Spreadsheet,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Text => "Text",
            Format::Word => "Word",
            Format::Pdf => "PDF",
            Format::Spreadsheet => "Spreadsheet",
        }
    }
}

/// A part of a document's text, as the model receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extracted {
    pub format: Format,
    pub text: String,
    /// More text follows this part.
    pub truncated: bool,
    /// Where this part starts, in characters.
    #[serde(default)]
    pub offset: usize,
    /// Where the next part starts, when more follows.
    #[serde(default)]
    pub next_offset: Option<usize>,
}

/// A table's rows (the first sheet of a workbook, or a CSV).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub sheet: String,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}

// ---------- which folders may be shared, and paths inside them ----------

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

fn lower(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase()
}

/// `a` is `b` or inside it.
fn within(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}\\"))
}

/// The display form of a canonical path (`\\?\C:\x` → `C:\x`).
fn display(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .filter(|rest| !rest.starts_with("UNC\\"))
        .unwrap_or(&s)
        .to_string()
}

/// A folder the user may share: an existing local folder that is not a
/// link, not a whole drive, not their whole profile, and not where Windows,
/// programs or app data (Nexus's own included) live. Returns its canonical
/// display path.
pub fn check_shareable(path: &str) -> Result<String, String> {
    let trimmed = path.trim().trim_matches('"');
    let given = Path::new(trimmed);
    if trimmed.starts_with(r"\\") {
        return Err("network and device paths cannot be shared; use a folder on this PC".into());
    }
    if !given.is_absolute() {
        return Err("give the folder's full path, like C:\\Users\\you\\Documents\\Jobs".into());
    }
    let meta = fs::symlink_metadata(given).map_err(|_| "that folder does not exist".to_string())?;
    if meta.file_type().is_symlink() {
        return Err("that path is a link to another place; share the real folder".into());
    }
    if !meta.is_dir() {
        return Err("that path is a file, not a folder".into());
    }
    let canonical =
        fs::canonicalize(given).map_err(|e| format!("that folder cannot be opened: {e}"))?;
    let shown = display(&canonical);
    let low = lower(Path::new(&shown));
    if Path::new(&shown).parent().is_none() || low.len() <= 3 {
        return Err("a whole drive cannot be shared; choose a folder on it".into());
    }
    let mut forbidden: Vec<(String, &str)> = Vec::new();
    for (var, why) in [
        ("SystemRoot", "Windows"),
        ("ProgramFiles", "installed programs"),
        ("ProgramFiles(x86)", "installed programs"),
        ("ProgramData", "programs' data"),
        ("LOCALAPPDATA", "app data"),
        ("APPDATA", "app data"),
    ] {
        if let Some(p) = env_path(var) {
            forbidden.push((lower(&p), why));
        }
    }
    for (root, why) in &forbidden {
        if within(&low, root) {
            return Err(format!("folders of {why} cannot be shared"));
        }
    }
    if let Some(profile) = env_path("USERPROFILE")
        && low == lower(&profile)
    {
        return Err("your whole user folder cannot be shared; choose a folder inside it".into());
    }
    if let Some(profile) =
        env_path("NEXUS_PROFILE_DIR").or_else(|| Some(nexus_protocol::pairing::profile_dir()))
        && within(&low, &lower(&profile))
    {
        return Err("Nexus's own data folder cannot be shared".into());
    }
    Ok(shown)
}

/// Whether a shared folder can be read now (it may have been moved or
/// replaced by a link since it was shared).
pub fn folder_problem(root: &Path) -> Option<String> {
    match fs::symlink_metadata(root) {
        Err(_) => Some("the folder is missing".to_string()),
        Ok(m) if m.file_type().is_symlink() => {
            Some("the folder is now a link to another place".to_string())
        }
        Ok(m) if !m.is_dir() => Some("the path is no longer a folder".to_string()),
        Ok(_) => None,
    }
}

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// One plain name: no separators, no stream (`:`), no reserved device name.
pub fn plain_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 200 {
        return Err("a name must have 1 to 200 characters".into());
    }
    if name
        .chars()
        .any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c))
    {
        return Err(format!(
            "{name:?} contains characters not allowed in a file name"
        ));
    }
    if name.ends_with(['.', ' ']) || name == "." || name == ".." {
        return Err(format!("{name:?} is not a valid name"));
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!("{name:?} is a reserved Windows name"));
    }
    Ok(())
}

/// A path relative to a shared folder, resolved inside it: plain names only
/// (no `..`, no drive, no stream), and no link anywhere on the way.
pub fn resolve_in(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim().trim_matches(['/', '\\']);
    if relative.is_empty() || relative.len() > 400 {
        return Err("give the path of a file inside the folder".into());
    }
    let mut path = root.to_path_buf();
    for part in relative.split(['/', '\\']) {
        plain_name(part)?;
        let parsed = Path::new(part).components().collect::<Vec<_>>();
        if !matches!(parsed.as_slice(), [Component::Normal(_)]) {
            return Err(format!("{part:?} is not a plain name"));
        }
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("that path goes through a link; links are not followed".into());
            }
            Ok(_) => {}
            Err(_) => return Err(format!("{relative:?} was not found in the folder")),
        }
    }
    // Defence in depth: the real path must still be inside the real folder.
    let real_root =
        fs::canonicalize(root).map_err(|e| format!("the folder cannot be opened: {e}"))?;
    let real = fs::canonicalize(&path).map_err(|e| format!("the file cannot be opened: {e}"))?;
    if !real.starts_with(&real_root) {
        return Err("that path leads outside the folder".into());
    }
    Ok(path)
}

fn is_hidden(meta: &fs::Metadata) -> bool {
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    meta.file_attributes() & (HIDDEN | SYSTEM) != 0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the folder, `/` between parts.
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
    pub modified_ms: i64,
    pub readable: bool,
}

/// What a folder (or a folder inside it) holds, a few levels deep, without
/// hidden or system items and without following links.
pub fn list(root: &Path, under: Option<&str>) -> Result<(Vec<Entry>, bool), String> {
    let start = match under
        .map(str::trim)
        .filter(|u| !u.is_empty() && *u != "/" && *u != ".")
    {
        Some(u) => resolve_in(root, u)?,
        None => root.to_path_buf(),
    };
    let mut out = Vec::new();
    let mut truncated = false;
    let mut stack = vec![(start, 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        let mut items: Vec<_> = read.filter_map(Result::ok).collect();
        items.sort_by_key(|e| e.file_name());
        for item in items {
            let Ok(meta) = fs::symlink_metadata(item.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() || is_hidden(&meta) {
                continue;
            }
            if out.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let path = item.path();
            let relative = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            out.push(Entry {
                path: relative,
                is_dir: meta.is_dir(),
                size_bytes: if meta.is_dir() { 0 } else { meta.len() },
                modified_ms,
                readable: !meta.is_dir() && Format::of(&path).is_some(),
            });
            if meta.is_dir() && depth + 1 < MAX_DEPTH {
                stack.push((path, depth + 1));
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((out, truncated))
}

// ---------- reading (in the extractor process) ----------

/// Up to `max_chars` characters of `text` from `offset`, and where the next
/// part starts if more follows. A part ends after a line break when one falls
/// in its second half, so a row of a sheet or a paragraph is not split.
pub(crate) fn part(
    text: &str,
    offset: usize,
    max_chars: usize,
) -> Result<(String, Option<usize>), String> {
    let start = match text.char_indices().nth(offset) {
        Some((i, _)) => i,
        None if offset == 0 => return Ok((String::new(), None)),
        None => {
            return Err(format!(
                "the document has {} characters; there is no text from offset {offset} on",
                text.chars().count()
            ));
        }
    };
    let rest = &text[start..];
    let Some((end, _)) = rest.char_indices().nth(max_chars) else {
        return Ok((rest.to_string(), None));
    };
    let (end, taken) = match rest[..end].rfind('\n') {
        Some(line_end) => {
            let chars = rest[..=line_end].chars().count();
            if chars > max_chars / 2 {
                (line_end + 1, chars)
            } else {
                (end, max_chars)
            }
        }
        None => (end, max_chars),
    };
    Ok((rest[..end].to_string(), Some(offset + taken)))
}

fn cut(text: &str, max_chars: usize) -> (String, bool) {
    match text.char_indices().nth(max_chars) {
        Some((i, _)) => (text[..i].to_string(), true),
        None => (text.to_string(), false),
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let meta = fs::metadata(path).map_err(|e| format!("cannot read the file: {e}"))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "the file is {} MB; at most {} MB is read",
            meta.len() / (1024 * 1024),
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("cannot read the file: {e}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("the file grew past the size limit while being read".into());
    }
    Ok(bytes)
}

fn zip_part(bytes: &[u8], name: &str) -> Result<String, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| "not a valid Word file".to_string())?;
    let file = archive
        .by_name(name)
        .map_err(|_| "not a valid Word file (no document part)".to_string())?;
    if file.size() > MAX_PART_BYTES {
        return Err("the Word file unpacks to too much data".into());
    }
    let mut xml = String::new();
    file.take(MAX_PART_BYTES)
        .read_to_string(&mut xml)
        .map_err(|_| "the Word file's text is unreadable".to_string())?;
    Ok(xml)
}

/// The text of a Word document: paragraphs on their own lines, tabs and
/// line breaks kept.
pub fn docx_text(bytes: &[u8]) -> Result<String, String> {
    use quick_xml::Reader;
    use quick_xml::events::Event;
    let xml = zip_part(bytes, "word/document.xml")?;
    let mut reader = Reader::from_str(&xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == "w:t" => in_text = true,
            Ok(Event::End(e)) => match e.name().as_ref() {
                "w:t" => in_text = false,
                "w:p" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "w:tab" => out.push('\t'),
                "w:br" | "w:cr" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => out.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if in_text => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.push(c);
                } else {
                    match r.xml10_content().as_ref() {
                        "amp" => out.push('&'),
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "quot" => out.push('"'),
                        "apos" => out.push('\''),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err("the Word file's text is malformed".into()),
            _ => {}
        }
    }
    Ok(out)
}

fn workbook(bytes: Vec<u8>) -> Result<calamine::Sheets<Cursor<Vec<u8>>>, String> {
    calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|e| format!("not a readable spreadsheet: {e}"))
}

/// Days since 1970-01-01 → (year, month, day) (proleptic Gregorian).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// An Excel date serial (1900 system) as `YYYY-MM-DD`, with `HH:MM` when
/// it carries a time of day.
pub fn excel_date(serial: f64) -> String {
    if !(61.0..=2_958_465.0).contains(&serial) {
        return serial.to_string();
    }
    let days = serial.floor() as i64;
    let (y, m, d) = civil_from_days(days - 25_569);
    let minutes = ((serial - serial.floor()) * 1440.0).round() as i64;
    if minutes == 0 || minutes >= 1440 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!(
            "{y:04}-{m:02}-{d:02} {:02}:{:02}",
            minutes / 60,
            minutes % 60
        )
    }
}

fn cell_text(cell: &calamine::Data) -> String {
    let raw = match cell {
        calamine::Data::DateTime(dt) => excel_date(dt.as_f64()),
        other => other.to_string(),
    };
    let (text, _) = cut(&raw, MAX_CELL_CHARS);
    text.replace(['\t', '\r', '\n'], " ")
}

fn spreadsheet_text(bytes: Vec<u8>, max_chars: usize) -> Result<String, String> {
    use calamine::Reader;
    let mut book = workbook(bytes)?;
    let mut out = String::new();
    for name in book.sheet_names().into_iter().take(MAX_SHEETS) {
        let Ok(range) = book.worksheet_range(&name) else {
            continue;
        };
        out.push_str(&format!("# Sheet: {name}\n"));
        for row in range.rows() {
            let cells: Vec<String> = row.iter().take(MAX_COLUMNS).map(cell_text).collect();
            if cells.iter().all(String::is_empty) {
                continue;
            }
            out.push_str(&cells.join("\t"));
            out.push('\n');
            if out.len() > max_chars.saturating_mul(4) {
                break;
            }
        }
        out.push('\n');
    }
    Ok(out)
}

/// The rows of a CSV (comma, semicolon or tab, guessed from the first line;
/// quoted fields may hold separators, quotes and line breaks).
pub fn csv_rows(text: &str, max_rows: usize) -> (Vec<Vec<String>>, bool) {
    let first = text.lines().next().unwrap_or("");
    let sep = [';', '\t', ',']
        .into_iter()
        .max_by_key(|s| first.matches(*s).count())
        .unwrap_or(',');
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.is_empty() {
            quoted = true;
        } else if c == sep {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            if !row.iter().all(|f| f.trim().is_empty()) {
                rows.push(std::mem::take(&mut row));
                if rows.len() >= max_rows {
                    return (rows, chars.peek().is_some());
                }
            } else {
                row.clear();
            }
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        if !row.iter().all(|f| f.trim().is_empty()) {
            rows.push(row);
        }
    }
    (rows, false)
}

/// A part of a document's text, from `offset` (run inside the extractor
/// process).
pub fn extract(path: &Path, offset: usize, max_chars: usize) -> Result<Extracted, String> {
    let format =
        Format::of(path).ok_or("Nexus reads text, Word (.docx), PDF and Excel files only")?;
    let bytes = read_bounded(path)?;
    let text = match format {
        Format::Text => String::from_utf8_lossy(&bytes).into_owned(),
        Format::Word => docx_text(&bytes)?,
        Format::Pdf => {
            let text = pdf_extract::extract_text_from_mem(&bytes)
                .map_err(|e| format!("the PDF's text could not be read: {e}"))?;
            if text.trim().is_empty() {
                return Err("the PDF has no text (it may be a scan)".into());
            }
            text
        }
        // A sheet is read only as far as this part needs.
        Format::Spreadsheet => spreadsheet_text(bytes, offset.saturating_add(max_chars))?,
    };
    let (text, next_offset) = part(&text, offset, max_chars)?;
    Ok(Extracted {
        format,
        text,
        truncated: next_offset.is_some(),
        offset,
        next_offset,
    })
}

/// A table's rows (run inside the extractor process): the first sheet of
/// a workbook, or a CSV file.
pub fn extract_table(path: &Path, max_rows: usize) -> Result<Table, String> {
    let format =
        Format::of(path).ok_or("only Excel (.xlsx, .xls, .ods) and CSV files can be imported")?;
    let bytes = read_bounded(path)?;
    match format {
        Format::Spreadsheet => {
            use calamine::Reader;
            let mut book = workbook(bytes)?;
            let sheet = book
                .sheet_names()
                .into_iter()
                .next()
                .ok_or("the workbook has no sheet")?;
            let range = book
                .worksheet_range(&sheet)
                .map_err(|e| format!("the sheet is unreadable: {e}"))?;
            let mut rows = Vec::new();
            let mut truncated = false;
            for row in range.rows() {
                let cells: Vec<String> = row.iter().take(MAX_COLUMNS).map(cell_text).collect();
                if cells.iter().all(|c| c.trim().is_empty()) {
                    continue;
                }
                if rows.len() >= max_rows {
                    truncated = true;
                    break;
                }
                rows.push(cells);
            }
            Ok(Table {
                sheet,
                rows,
                truncated,
            })
        }
        Format::Text
            if path.extension().is_some_and(|e| {
                e.eq_ignore_ascii_case("csv") || e.eq_ignore_ascii_case("tsv")
            }) =>
        {
            let (rows, truncated) = csv_rows(&String::from_utf8_lossy(&bytes), max_rows);
            let rows = rows
                .into_iter()
                .map(|r| {
                    r.into_iter()
                        .take(MAX_COLUMNS)
                        .map(|c| cut(&c, MAX_CELL_CHARS).0)
                        .collect()
                })
                .collect();
            Ok(Table {
                sheet: "CSV".to_string(),
                rows,
                truncated,
            })
        }
        _ => Err("only Excel (.xlsx, .xls, .ods) and CSV files can be imported".into()),
    }
}

/// `nexus-host extract text|table <path> <limit> [offset]`: prints the
/// result as one JSON line (`{"ok": …}` or `{"error": "…"}`).
pub fn run_extract_command(args: &[String]) -> i32 {
    let outcome = match args {
        [mode, path, limit, rest @ ..] if rest.len() <= 1 => {
            let limit = limit.parse::<usize>().unwrap_or(MAX_TEXT_CHARS);
            let offset = rest.first().and_then(|o| o.parse::<usize>().ok());
            let path = Path::new(path);
            match mode.as_str() {
                "text" => extract(path, offset.unwrap_or(0), limit).map(|x| serde_json::json!(x)),
                "table" => extract_table(path, limit).map(|x| serde_json::json!(x)),
                _ => Err("unknown extract mode".to_string()),
            }
        }
        _ => Err("usage: nexus-host extract text|table <path> <limit> [offset]".to_string()),
    };
    let line = match outcome {
        Ok(value) => serde_json::json!({ "ok": value }),
        Err(error) => serde_json::json!({ "error": error }),
    };
    println!("{line}");
    0
}

// ---------- running the extractor (in the Host) ----------

/// A Job Object that caps the extractor's memory, allows it no child
/// processes and kills it when dropped.
struct Job(windows_sys::Win32::Foundation::HANDLE);

impl Job {
    fn new() -> Result<Self, String> {
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: no security attributes, no name.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err("could not create a job object".into());
        }
        let job = Job(handle);
        // SAFETY: zeroed POD is a valid starting value.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        limits.ProcessMemoryLimit = EXTRACT_MEMORY_BYTES;
        // SAFETY: a valid job handle and a structure of the size given.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err("could not limit the job object".into());
        }
        Ok(job)
    }

    fn assign(&self, process: std::os::windows::io::RawHandle) -> Result<(), String> {
        // SAFETY: both handles are valid for the duration of the call.
        let ok = unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(self.0, process as _)
        };
        if ok == 0 {
            return Err("could not contain the reader process".into());
        }
        Ok(())
    }
}

// SAFETY: a job object handle is a kernel handle, usable from any thread.
unsafe impl Send for Job {}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: a handle we own; closing it kills what is left in the job.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Runs `exe extract <mode> <path> <limit> <offset>` contained and
/// time-limited, and returns the JSON value it printed under `ok`.
pub async fn run_extractor(
    exe: &Path,
    mode: &str,
    path: &Path,
    limit: usize,
    offset: usize,
) -> Result<serde_json::Value, String> {
    let job = Job::new()?;
    let mut child = tokio::process::Command::new(exe)
        .arg("extract")
        .arg(mode)
        .arg(path)
        .arg(limit.to_string())
        .arg(offset.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start the document reader: {e}"))?;
    if let Some(handle) = child.raw_handle() {
        job.assign(handle)?;
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or("the document reader has no output")?;
    let read = async {
        let mut out = Vec::new();
        (&mut stdout)
            .take(MAX_CHILD_OUTPUT as u64)
            .read_to_end(&mut out)
            .await
            .map(|_| out)
    };
    let output = match tokio::time::timeout(EXTRACT_TIMEOUT, read).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(format!("the document reader failed: {e}")),
        Err(_) => {
            let _ = child.kill().await;
            return Err(format!(
                "reading the document took longer than {} seconds",
                EXTRACT_TIMEOUT.as_secs()
            ));
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    drop(job);
    let value: serde_json::Value = serde_json::from_slice(&output).map_err(|_| {
        "the document could not be read (the reader stopped; it may be too large or malformed)"
            .to_string()
    })?;
    match (
        value.get("ok"),
        value.get("error").and_then(serde_json::Value::as_str),
    ) {
        (Some(ok), _) => Ok(ok.clone()),
        (None, Some(error)) => Err(error.to_string()),
        _ => Err("the document reader gave no answer".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_text_is_read_in_parts_that_end_at_a_line_break() {
        let text = "row one\nrow two\nrow three\n";
        // 12 characters reach into "row two"; the part ends after "row one".
        assert_eq!(
            part(text, 0, 12).unwrap(),
            ("row one\n".to_string(), Some(8))
        );
        assert_eq!(
            part(text, 8, 12).unwrap(),
            ("row two\n".to_string(), Some(16))
        );
        assert_eq!(
            part(text, 16, 12).unwrap(),
            ("row three\n".to_string(), None)
        );
        // No line break in the second half: the part is cut where it fills.
        assert_eq!(
            part("abcdefghij", 0, 4).unwrap(),
            ("abcd".to_string(), Some(4))
        );
        // Offsets count characters, not bytes.
        assert_eq!(part("ăâîșț", 2, 2).unwrap(), ("îș".to_string(), Some(4)));
        assert_eq!(part("", 0, 4).unwrap(), (String::new(), None));
        let past = part("abc", 3, 4).unwrap_err();
        assert!(past.contains("3 characters"), "{past}");
    }

    #[test]
    fn csv_handles_separators_quotes_and_line_breaks() {
        let (rows, truncated) = csv_rows(
            "\u{feff}Company;Role;Notes\r\nAcme;\"Service Desk; L1\";\"said \"\"maybe\"\"\nlater\"\r\n\r\nBeta;IT;\n",
            100,
        );
        assert!(!truncated);
        assert_eq!(
            rows,
            [
                vec!["Company", "Role", "Notes"],
                vec!["Acme", "Service Desk; L1", "said \"maybe\"\nlater"],
                vec!["Beta", "IT", ""],
            ]
        );
        let (rows, truncated) = csv_rows("a,b\n1,2\n3,4\n", 2);
        assert_eq!(rows.len(), 2);
        assert!(truncated);
    }

    #[test]
    fn names_that_could_leave_the_folder_or_hide_a_stream_are_refused() {
        for bad in [
            "..",
            "a:b",
            "CON",
            "nul.txt",
            "x/y",
            r"x\y",
            "trailing.",
            "a*b",
            "",
        ] {
            assert!(plain_name(bad).is_err(), "{bad}");
        }
        for good in ["CV 2026.pdf", "Scrisoare Acme.docx", "notes.md"] {
            assert!(plain_name(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn excel_dates_become_calendar_days() {
        assert_eq!(excel_date(45_658.0), "2025-01-01");
        assert_eq!(excel_date(46_313.0), "2026-10-18");
        assert_eq!(excel_date(46_313.375), "2026-10-18 09:00");
        assert_eq!(
            excel_date(3.5),
            "3.5",
            "not a plausible date: left as a number"
        );
    }

    #[test]
    fn formats_are_known_by_extension() {
        assert_eq!(Format::of(Path::new("cv.PDF")), Some(Format::Pdf));
        assert_eq!(Format::of(Path::new("a.docx")), Some(Format::Word));
        assert_eq!(Format::of(Path::new("a.xlsx")), Some(Format::Spreadsheet));
        assert_eq!(Format::of(Path::new("a.csv")), Some(Format::Text));
        assert_eq!(Format::of(Path::new("a.exe")), None);
        assert_eq!(Format::of(Path::new("a.doc")), None);
    }
}
