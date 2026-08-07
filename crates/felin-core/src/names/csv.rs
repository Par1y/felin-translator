//! CSV glossary import: read headers for column mapping, then parse rows.

use crate::error::{Error, Result};

/// Which CSV column holds each field. `japanese`/`chinese` are required; the
/// rest are optional.
#[derive(Debug, Clone)]
pub struct ColumnMapping {
    pub japanese: usize,
    pub chinese: usize,
    pub english: Option<usize>,
    pub category: Option<usize>,
    pub notes: Option<usize>,
    pub has_header: bool,
}

/// One parsed glossary row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NameRow {
    pub japanese: String,
    pub chinese: String,
    pub english: Option<String>,
    pub category: Option<String>,
    pub notes: Option<String>,
}

/// The first row (for a column-mapping UI).
pub fn headers(data: &[u8]) -> Result<Vec<String>> {
    let mut rdr = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(data);
    match rdr.records().next() {
        Some(rec) => {
            let rec = rec.map_err(|e| Error::InvalidInput { detail: format!("bad CSV: {e}") })?;
            Ok(rec.iter().map(|s| s.to_string()).collect())
        }
        None => Ok(Vec::new()),
    }
}

/// Parse rows using `mapping`. Rows with an empty japanese or chinese are skipped.
pub fn parse(data: &[u8], mapping: &ColumnMapping) -> Result<Vec<NameRow>> {
    let mut rdr =
        csv::ReaderBuilder::new().has_headers(mapping.has_header).flexible(true).from_reader(data);
    let mut out = Vec::new();
    for rec in rdr.records() {
        let rec = rec.map_err(|e| Error::InvalidInput { detail: format!("bad CSV: {e}") })?;
        let get = |i: usize| rec.get(i).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        let (Some(japanese), Some(chinese)) = (get(mapping.japanese), get(mapping.chinese)) else {
            continue;
        };
        out.push(NameRow {
            japanese,
            chinese,
            english: mapping.english.and_then(|i| get(i)),
            category: mapping.category.and_then(|i| get(i)),
            notes: mapping.notes.and_then(|i| get(i)),
        });
    }
    Ok(out)
}

