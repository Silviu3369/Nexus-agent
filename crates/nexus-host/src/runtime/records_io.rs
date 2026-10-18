//! Excel for registers (`PLAN_NEXUS_RO.md` §3a, step 2): export a register to
//! a new .xlsx file in a folder shared for writing, and import the user's
//! existing list from an Excel or CSV file in a shared folder. Import shows
//! first how each column would be used; the user confirms the mapping. A
//! value that cannot be used as is (an unreadable date, an unknown status)
//! is kept in the notes rather than guessed; a row without what the
//! register requires, or one already there, is listed and skipped.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use nexus_protocol::{
    ErrorCode, ExportView, FieldKind, FolderId, ImportPreviewView, ImportRequest, ImportResultView,
    ImportSkipView, NexusError, RegisterId,
};
use nexus_storage::{RecordFilter, Values, now_ms};

use super::records::{Register, today};
use super::{Runtime, to_nexus_error};
use crate::documents::{self, MAX_TABLE_ROWS, Table};
use crate::tools::document_file::{self, Destination, DocumentContent};

const SAMPLE_ROWS: usize = 5;
const MAX_EXPORT_ROWS: u32 = 500;

fn invalid(message: impl Into<String>) -> NexusError {
    NexusError::new(ErrorCode::InvalidRequest, message)
}

/// Letters and digits only, lower case, for comparing headers.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'ă' | 'â' | 'á' | 'à' | 'ä' => 'a',
            'î' | 'í' | 'ï' => 'i',
            'ș' | 'ş' => 's',
            'ț' | 'ţ' => 't',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'ó' | 'ö' | 'ô' => 'o',
            'ú' | 'ü' => 'u',
            other => other,
        })
        .collect()
}

/// Other names a column may carry for a field (English, Romanian, Dutch,
/// French), compared squashed.
fn synonyms(key: &str) -> &'static [&'static str] {
    match key {
        "reference" => &[
            "reference",
            "referenceno",
            "referentie",
            "referentienummer",
            "vacaturenummer",
            "vacancynumber",
            "jobid",
            "id",
            "referinta",
            "nrreferinta",
            "numar",
            "ref",
        ],
        "ad_file" => &["adfile", "file", "fisier", "bestand"],
        "company" => &[
            "company",
            "companie",
            "compania",
            "firma",
            "employer",
            "werkgever",
            "bedrijf",
            "entreprise",
            "societate",
            "angajator",
        ],
        "role" => &[
            "role", "post", "postul", "position", "job", "jobtitle", "functie", "functia",
            "functie", "poste", "rol", "titlu",
        ],
        "status" => &["status", "stare", "statut", "stadiu"],
        "applied_on" => &[
            "appliedon",
            "applied",
            "dateapplied",
            "dataaplicarii",
            "aplicat",
            "aplicatla",
            "datum",
            "date",
            "data",
            "sollicitatiedatum",
        ],
        "follow_up_on" => &[
            "followupon",
            "followup",
            "revenire",
            "reminder",
            "opvolging",
            "relance",
        ],
        "next_step" => &["nextstep", "pasurmator", "urmatorulpas", "volgendestap"],
        "link" => &["link", "url", "adresa", "anunt", "vacature", "annonce"],
        "source" => &[
            "source",
            "sursa",
            "platform",
            "platforma",
            "site",
            "bron",
            "via",
        ],
        "location" => &[
            "location", "locatie", "oras", "city", "stad", "ville", "plaats",
        ],
        "work_mode" => &["workmode", "modlucru", "remote", "telewerk"],
        "salary" => &["salary", "salariu", "loon", "salaire", "pay"],
        "contact" => &[
            "contact",
            "recruiter",
            "recrutor",
            "persoanacontact",
            "contactpersoon",
        ],
        "deadline" => &["deadline", "applyby", "termen", "termenlimita", "uiterlijk"],
        "documents" => &[
            "documents",
            "documente",
            "documentstrimise",
            "cv",
            "attachments",
        ],
        "requirements" => &["requirements", "cerinte", "vereisten", "exigences"],
        "notes" => &[
            "notes",
            "note",
            "notite",
            "observatii",
            "comments",
            "comentarii",
            "opmerkingen",
            "remarques",
        ],
        _ => &[],
    }
}

/// The field a header most likely names, if any.
pub(super) fn guess_field(register: &Register, header: &str) -> Option<String> {
    let h = squash(header);
    if h.is_empty() {
        return None;
    }
    register
        .schema
        .fields
        .iter()
        .find(|f| {
            squash(&f.key) == h || squash(&f.label) == h || synonyms(&f.key).contains(&h.as_str())
        })
        .map(|f| f.key.clone())
}

/// Statuses written in other words.
fn status_synonym(value: &str) -> Option<&'static str> {
    let v = squash(value);
    let pairs: [(&[&str], &str); 6] = [
        (
            &[
                "deaplicat",
                "toapply",
                "saved",
                "salvat",
                "teverzenden",
                "aplica",
            ],
            "To apply",
        ),
        (
            &[
                "aplicat",
                "applied",
                "trimis",
                "sent",
                "gesolliciteerd",
                "postule",
                "candidatura",
                "candidat",
            ],
            "Applied",
        ),
        (
            &[
                "interview",
                "interviu",
                "interviewing",
                "gesprek",
                "entretien",
                "sollicitatiegesprek",
            ],
            "Interviewing",
        ),
        (
            &["oferta", "offer", "aanbod", "offre", "acceptat", "accepted"],
            "Offer",
        ),
        (
            &[
                "refuzat",
                "rejected",
                "respins",
                "afgewezen",
                "refus",
                "refuse",
                "negatief",
                "nu",
            ],
            "Rejected",
        ),
        (
            &["retras", "withdrawn", "ingetrokken", "retire", "anulat"],
            "Withdrawn",
        ),
    ];
    pairs
        .iter()
        .find(|(words, _)| words.contains(&v.as_str()))
        .map(|(_, status)| *status)
}

/// A date as registers store it: ISO, or day-first with `.`, `/` or `-`
/// (as written in much of Europe); a time after it is dropped.
pub(super) fn normalize_date(value: &str) -> Option<String> {
    let v = value.trim().split([' ', 'T']).next().unwrap_or("");
    let parts: Vec<&str> = v.split(['-', '.', '/']).collect();
    let [a, b, c] = parts.as_slice() else {
        return None;
    };
    let (y, m, d) = if a.len() == 4 {
        (*a, *b, *c)
    } else {
        (*c, *b, *a)
    };
    let (y, m, d) = (
        y.parse::<u32>().ok()?,
        m.parse::<u32>().ok()?,
        d.parse::<u32>().ok()?,
    );
    let y = if y < 100 { 2000 + y } else { y };
    let iso = format!("{y:04}-{m:02}-{d:02}");
    super::records::valid_date(&iso).then_some(iso)
}

impl Runtime {
    async fn shared(
        &self,
        id: FolderId,
    ) -> Result<(nexus_storage::FolderRow, PathBuf), NexusError> {
        let folder = self.store.folder(id).await.map_err(to_nexus_error)?;
        if let Some(problem) = documents::folder_problem(Path::new(&folder.path)) {
            return Err(invalid(problem));
        }
        let root = PathBuf::from(&folder.path);
        Ok((folder, root))
    }

    async fn table(&self, root: &Path, relative: &str) -> Result<(String, Table), NexusError> {
        let (root, rel) = (root.to_path_buf(), relative.to_string());
        let path = tokio::task::spawn_blocking(move || documents::resolve_in(&root, &rel))
            .await
            .map_err(|e| NexusError::new(ErrorCode::InternalFailure, e.to_string()))?
            .map_err(invalid)?;
        let value = self
            .read_isolated("table", &path, MAX_TABLE_ROWS + 1)
            .await
            .map_err(invalid)?;
        let table: Table = serde_json::from_value(value)
            .map_err(|_| invalid("the file reader gave an unreadable answer"))?;
        if table.rows.len() < 2 {
            return Err(invalid(
                "the file needs a header row and at least one row of data",
            ));
        }
        let name = Path::new(relative)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok((name, table))
    }

    pub async fn export_register(
        &self,
        register_id: RegisterId,
        folder_id: FolderId,
    ) -> Result<ExportView, NexusError> {
        let register = self.register_by_id(register_id).await?;
        let (folder, root) = self.shared(folder_id).await?;
        if !folder.can_write {
            return Err(invalid(format!(
                "\"{}\" is shared for reading only; allow writing there to export into it",
                folder.label
            )));
        }
        let (rows, _) = self
            .store
            .records(
                register_id,
                RecordFilter {
                    limit: MAX_EXPORT_ROWS,
                    ..RecordFilter::default()
                },
            )
            .await
            .map_err(to_nexus_error)?;
        let count = rows.len() as u32;
        let bytes = workbook(
            &register,
            &rows.into_iter().map(|r| r.values).collect::<Vec<_>>(),
        )
        .map_err(|e| {
            NexusError::new(
                ErrorCode::InternalFailure,
                format!("the Excel file could not be built: {e}"),
            )
        })?;
        let stem = format!("{} {}", register.row.name, today());
        let written = tokio::task::spawn_blocking(move || -> Result<PathBuf, String> {
            let content = DocumentContent {
                bytes,
                preview: String::new(),
            };
            for n in 1..=50 {
                let name = if n == 1 {
                    format!("{stem}.xlsx")
                } else {
                    format!("{stem} ({n}).xlsx")
                };
                let dest = Destination::new(&root, &name, &uuid::Uuid::new_v4().to_string());
                if dest.target().exists() {
                    continue;
                }
                return document_file::create(&dest, &content)
                    .map(|()| dest.target())
                    .map_err(|e| e.to_string());
            }
            Err("too many exports with that name today".into())
        })
        .await
        .map_err(|e| NexusError::new(ErrorCode::InternalFailure, e.to_string()))?
        .map_err(invalid)?;
        tracing::info!(register = %register.row.key, rows = count, "register exported");
        Ok(ExportView {
            path: written.display().to_string(),
            rows: count,
        })
    }

    pub async fn import_preview(
        &self,
        register_id: RegisterId,
        folder_id: FolderId,
        path: String,
    ) -> Result<ImportPreviewView, NexusError> {
        let register = self.register_by_id(register_id).await?;
        let (_, root) = self.shared(folder_id).await?;
        let (file, table) = self.table(&root, &path).await?;
        let columns = table.rows[0].clone();
        let mut taken = HashSet::new();
        let mapping = columns
            .iter()
            .map(|h| guess_field(&register, h).filter(|k| taken.insert(k.clone())))
            .collect();
        Ok(ImportPreviewView {
            file,
            sheet: table.sheet,
            mapping,
            rows: (table.rows.len() - 1).min(MAX_TABLE_ROWS) as u32,
            sample: table
                .rows
                .iter()
                .skip(1)
                .take(SAMPLE_ROWS)
                .cloned()
                .collect(),
            truncated: table.truncated || table.rows.len() - 1 > MAX_TABLE_ROWS,
            columns,
        })
    }

    pub async fn import_records(
        &self,
        request: ImportRequest,
    ) -> Result<ImportResultView, NexusError> {
        let register = self.register_by_id(request.register_id).await?;
        let (_, root) = self.shared(request.folder_id).await?;
        let (_, table) = self.table(&root, &request.path).await?;
        let columns = &table.rows[0];
        if request.mapping.len() != columns.len() {
            return Err(invalid(
                "the column mapping does not match the file; preview it again",
            ));
        }
        let mut seen = HashSet::new();
        for key in request.mapping.iter().flatten() {
            if register.schema.fields.iter().all(|f| &f.key != key) {
                return Err(invalid(format!(
                    "{key:?} is not a field of {}",
                    register.row.name
                )));
            }
            if !seen.insert(key) {
                return Err(invalid(format!("two columns go to {key:?}; choose one")));
            }
        }
        let (existing, _) = self
            .store
            .records(
                register.row.id,
                RecordFilter {
                    limit: 500,
                    ..RecordFilter::default()
                },
            )
            .await
            .map_err(to_nexus_error)?;
        let mut titles: HashSet<String> = existing
            .iter()
            .map(|r| register.schema.title(&r.values).to_lowercase())
            .collect();
        let key = register.schema.key_field.clone();
        let mut references: HashSet<String> = existing
            .iter()
            .filter_map(|r| key.as_ref().and_then(|k| r.values.get(k)))
            .map(|v| v.to_lowercase())
            .collect();
        let mut adds = Vec::new();
        let mut skipped = Vec::new();
        let mut adjusted = 0u32;
        for (i, row) in table.rows.iter().enumerate().skip(1).take(MAX_TABLE_ROWS) {
            let number = i as u32 + 1;
            match import_row(&register, &request.mapping, row) {
                Err(reason) => skipped.push(ImportSkipView {
                    row: number,
                    reason,
                }),
                Ok((values, changed)) => {
                    if let Some(reference) = key.as_ref().and_then(|k| values.get(k))
                        && !references.insert(reference.to_lowercase())
                    {
                        skipped.push(ImportSkipView {
                            row: number,
                            reason: format!(
                                "reference {reference} is already in {}",
                                register.row.name
                            ),
                        });
                        continue;
                    }
                    let title = register.schema.title(&values).to_lowercase();
                    if !titles.insert(title) {
                        skipped.push(ImportSkipView {
                            row: number,
                            reason: format!(
                                "{} is already in {}",
                                register.schema.title(&values),
                                register.row.name
                            ),
                        });
                        continue;
                    }
                    if changed {
                        adjusted += 1;
                    }
                    adds.push(values);
                }
            }
        }
        let added = adds.len() as u32;
        if !adds.is_empty() {
            self.store
                .import_records(register.row.id, adds, now_ms())
                .await
                .map_err(to_nexus_error)?;
        }
        tracing::info!(register = %register.row.key, added, skipped = skipped.len(), "records imported");
        Ok(ImportResultView {
            added,
            adjusted,
            skipped,
        })
    }
}

/// One row as the register's values, or why it cannot be imported. The
/// flag says some value was kept in the notes instead of its field.
fn import_row(
    register: &Register,
    mapping: &[Option<String>],
    row: &[String],
) -> Result<(Values, bool), String> {
    let schema = &register.schema;
    let mut changes: Vec<(String, Option<String>)> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    for (key, raw) in mapping.iter().zip(row) {
        let Some(key) = key else { continue };
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let field = schema.field(key).expect("checked against the fields");
        let value = match field.kind {
            FieldKind::Date => normalize_date(raw),
            FieldKind::Choice => field
                .choices
                .iter()
                .find(|c| c.eq_ignore_ascii_case(raw))
                .cloned()
                .or_else(|| {
                    (key == &schema.status_field)
                        .then(|| status_synonym(raw))
                        .flatten()
                        .filter(|s| field.choices.iter().any(|c| c == s))
                        .map(str::to_string)
                }),
            FieldKind::Url if !raw.contains("://") && raw.contains('.') && !raw.contains(' ') => {
                Some(format!("https://{raw}"))
            }
            _ => Some(raw.to_string()),
        };
        match value {
            Some(v) if v.chars().count() <= field.max_chars as usize => {
                changes.push((key.clone(), Some(v)))
            }
            Some(v) => {
                kept.push(format!("{} in the file: {v}", field.label));
            }
            None => kept.push(format!("{} in the file: {raw}", field.label)),
        }
    }
    // A row with an application date but no status was sent.
    let has_status = changes.iter().any(|(k, _)| k == &schema.status_field);
    if !has_status
        && changes.iter().any(|(k, _)| k == "applied_on")
        && let Some(field) = schema.field(&schema.status_field)
        && field.choices.iter().any(|c| c == "Applied")
    {
        changes.push((schema.status_field.clone(), Some("Applied".to_string())));
    }
    let changed = !kept.is_empty();
    if changed && schema.field("notes").is_some() {
        let existing = changes
            .iter()
            .position(|(k, _)| k == "notes")
            .map(|i| changes.remove(i).1.unwrap_or_default());
        let mut notes: Vec<String> = existing.into_iter().filter(|n| !n.is_empty()).collect();
        notes.extend(kept);
        changes.push(("notes".to_string(), Some(notes.join("\n"))));
    }
    schema
        .apply(Values::new(), changes)
        .map(|values| (values, changed))
}

/// The register as one sheet: a bold header row of the field labels,
/// dates as real Excel dates, the header frozen and columns fitted.
fn workbook(register: &Register, rows: &[Values]) -> Result<Vec<u8>, rust_xlsxwriter::XlsxError> {
    use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    let name: String = register
        .row
        .name
        .chars()
        .filter(|c| !"[]:*?/\\".contains(*c))
        .take(31)
        .collect();
    sheet.set_name(&name)?;
    let bold = Format::new().set_bold();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    for (col, field) in register.schema.fields.iter().enumerate() {
        sheet.write_string_with_format(0, col as u16, &field.label, &bold)?;
    }
    for (r, values) in rows.iter().enumerate() {
        let row = r as u32 + 1;
        for (col, field) in register.schema.fields.iter().enumerate() {
            let Some(value) = values.get(&field.key) else {
                continue;
            };
            let col = col as u16;
            match (field.kind, ExcelDateTime::parse_from_str(value)) {
                (FieldKind::Date, Ok(d)) => {
                    sheet.write_datetime_with_format(row, col, &d, &date)?;
                }
                _ => {
                    sheet.write_string(row, col, value)?;
                }
            }
        }
    }
    sheet.set_freeze_panes(1, 0)?;
    sheet.autofit();
    book.save_to_buffer()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_written_in_the_usual_ways_are_read() {
        assert_eq!(normalize_date("2026-10-18").as_deref(), Some("2026-10-18"));
        assert_eq!(normalize_date("18.10.2026").as_deref(), Some("2026-10-18"));
        assert_eq!(normalize_date("18/10/2026").as_deref(), Some("2026-10-18"));
        assert_eq!(normalize_date("8-1-26").as_deref(), Some("2026-01-08"));
        assert_eq!(
            normalize_date("2026-10-18 09:30").as_deref(),
            Some("2026-10-18")
        );
        assert_eq!(normalize_date("31.02.2026"), None);
        assert_eq!(normalize_date("next week"), None);
    }

    #[test]
    fn statuses_in_other_words_are_recognised() {
        assert_eq!(status_synonym("Aplicat"), Some("Applied"));
        assert_eq!(status_synonym("respins"), Some("Rejected"));
        assert_eq!(status_synonym("Interviu"), Some("Interviewing"));
        assert_eq!(status_synonym("gesolliciteerd"), Some("Applied"));
        assert_eq!(status_synonym("in progress"), None);
    }
}
