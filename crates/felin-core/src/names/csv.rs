//! CSV glossary import: read headers for column mapping, then parse rows.

use crate::error::{Error, Result};

/// Which CSV column holds each field. `japanese`/`chinese` are required; the
/// rest are optional. `aliases` is a single column of `|`-separated forms.
#[derive(Debug, Clone)]
pub struct ColumnMapping {
    pub japanese: usize,
    pub chinese: usize,
    pub english: Option<usize>,
    pub category: Option<usize>,
    pub notes: Option<usize>,
    pub aliases: Option<usize>,
    pub has_header: bool,
}

/// One parsed glossary row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRow {
    pub japanese: String,
    pub chinese: String,
    pub english: Option<String>,
    pub category: Option<String>,
    pub notes: Option<String>,
    pub aliases: Vec<String>,
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
        let aliases = mapping
            .aliases
            .and_then(|i| get(i))
            .map(|s| s.split('|').map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect())
            .unwrap_or_default();
        out.push(NameRow {
            japanese,
            chinese,
            english: mapping.english.and_then(|i| get(i)),
            category: mapping.category.and_then(|i| get(i)),
            notes: mapping.notes.and_then(|i| get(i)),
            aliases,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> ColumnMapping {
        ColumnMapping {
            japanese: 0,
            chinese: 1,
            english: Some(2),
            category: None,
            notes: None,
            aliases: Some(3),
            has_header: true,
        }
    }

    #[test]
    fn parses_rows_with_aliases_and_skips_incomplete() {
        let data = "jp,zh,en,aliases\n田中,田中,Tanaka,たなか|タナカ\n,空,,\n猫,猫,cat,\n";
        let rows = parse(data.as_bytes(), &mapping()).unwrap();
        assert_eq!(rows.len(), 2); // the empty-japanese row is skipped
        assert_eq!(rows[0].japanese, "田中");
        assert_eq!(rows[0].english.as_deref(), Some("Tanaka"));
        assert_eq!(rows[0].aliases, vec!["たなか".to_string(), "タナカ".to_string()]);
        assert_eq!(rows[1].japanese, "猫");
        assert!(rows[1].aliases.is_empty());
    }

    #[test]
    fn reads_headers() {
        let data = "jp,zh,en,aliases\n田中,田中,,\n";
        assert_eq!(headers(data.as_bytes()).unwrap(), vec!["jp", "zh", "en", "aliases"]);
    }
}
