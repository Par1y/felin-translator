//! Small shared helpers.

/// Current UTC time as an RFC 3339 / ISO 8601 string — the canonical timestamp
/// format stored in `created_at` / `updated_at` columns and OCR manifests.
pub fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
