//! Plain-text import with encoding detection (plan §1: UTF-8 / Shift-JIS /
//! EUC-JP via `encoding_rs` + `chardetng`). No page structure, so no scores and
//! no cross-page merge — just blank-line paragraph splitting.

use crate::error::Result;
use crate::ocr::ingest::split_blocks;
use crate::types::{IngestedParagraph, OcrParagraphStatus};
use encoding_rs::Encoding;

/// Detect the encoding of `bytes` and decode to a `String`.
///
/// Fast-paths valid UTF-8; otherwise uses `chardetng` to guess (covers
/// Shift-JIS / EUC-JP / etc.). A lossy decode is not treated as fatal — the best
/// guess is kept and the replacement is logged — because a human proofreads
/// every paragraph anyway and losing the whole import would be worse.
pub fn decode_bytes(bytes: &[u8]) -> (String, &'static Encoding) {
    // Unicode encodings chardetng cannot detect: sniff their BOMs first. Check
    // the 4-byte UTF-32 BOMs before the 2-byte UTF-16 ones (UTF-32LE's BOM
    // starts with the UTF-16LE BOM).
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE, 0x00, 0x00]) {
        return (decode_utf32(rest, Endian::Little), encoding_rs::UTF_8);
    }
    if let Some(rest) = bytes.strip_prefix(&[0x00, 0x00, 0xFE, 0xFF]) {
        return (decode_utf32(rest, Endian::Big), encoding_rs::UTF_8);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (cow, _, _) = encoding_rs::UTF_16LE.decode(rest);
        return (cow.into_owned(), encoding_rs::UTF_16LE);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (cow, _, _) = encoding_rs::UTF_16BE.decode(rest);
        return (cow.into_owned(), encoding_rs::UTF_16BE);
    }

    let bytes = strip_bom(bytes); // UTF-8 BOM
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_string(), encoding_rs::UTF_8);
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    det.feed(bytes, true);
    // We only reach here when the bytes are NOT valid UTF-8 (fast path above),
    // so there is no point allowing a UTF-8 guess.
    let enc = det.guess(None, chardetng::Utf8Detection::Deny);
    let (cow, _, had_errors) = enc.decode(bytes);
    if had_errors {
        tracing::warn!(encoding = enc.name(), "text decode produced replacement characters");
    }
    (cow.into_owned(), enc)
}

enum Endian {
    Little,
    Big,
}

/// Best-effort UTF-32 decode (rare; `encoding_rs` has no UTF-32 codec).
fn decode_utf32(bytes: &[u8], endian: Endian) -> String {
    tracing::warn!("decoding UTF-32 text (rare); best-effort");
    bytes
        .chunks_exact(4)
        .map(|c| {
            let v = match endian {
                Endian::Little => u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                Endian::Big => u32::from_be_bytes([c[0], c[1], c[2], c[3]]),
            };
            char::from_u32(v).unwrap_or('\u{FFFD}')
        })
        .collect()
}

/// Import a text file's raw bytes into paragraphs.
pub fn import_txt(bytes: &[u8], source_file: &str) -> Result<Vec<IngestedParagraph>> {
    let (text, enc) = decode_bytes(bytes);
    tracing::info!(source = source_file, encoding = enc.name(), "imported txt");
    let paras = split_blocks(&text)
        .into_iter()
        .map(|block| {
            IngestedParagraph::new(
                block,
                None,
                source_file.to_string(),
                None,
                OcrParagraphStatus::Ok,
                serde_json::Value::Null,
            )
        })
        .collect();
    Ok(paras)
}

/// Strip a leading UTF-8 BOM if present.
fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

