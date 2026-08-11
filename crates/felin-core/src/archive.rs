//! Portable project archives.
//!
//! Packs a project's app-side data (project.db, OCR products, metadata) into a
//! single zstd-compressed zip with a per-file SHA-256 manifest plus a
//! **whole-archive digest stored as a file inside the archive**
//! (`felin-archive.sha256`), so a user can move / rename / back up a project
//! even though the internal data normally lives next to the software. The
//! digest travels with the archive — there is no `.sha256` sidecar to keep
//! alongside it.
//!
//! The user's *source inputs* are never included: OCR staging dirs named
//! `inputs/` (image symlinks created only for the `batch` sidecar pass) are
//! skipped, and source files stay in place.

use crate::error::{Error, Result};
use crate::util::now_iso8601;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

const ARCHIVE_SCHEMA: i64 = 1;
const MANIFEST_NAME: &str = "felin-archive.json";
const DIGEST_NAME: &str = "felin-archive.sha256";
const LOCK_NAME: &str = "project.lock";
/// OCR batch staging dir (symlinks to the user's source images). Never archived.
const INPUTS_DIR: &str = "inputs";

#[derive(Serialize, Deserialize)]
struct ArchiveManifest {
    schema: i64,
    slug: String,
    created_at: String,
    files: Vec<ArchiveFile>,
}

#[derive(Serialize, Deserialize)]
struct ArchiveFile {
    path: String,
    sha256: String,
    size: u64,
}

/// Progress of an archive export, reported between files.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum ArchiveProgress {
    /// Export started with the given number of files to pack.
    Start { total_files: usize },
    /// `done` of `total_files` files have been read/compressed so far.
    Progress { done: usize, total_files: usize },
}

/// Result of exporting a project.
#[derive(Debug, Clone)]
pub struct ExportOutcome {
    /// SHA-256 of the archive's file contents (embedded as
    /// `felin-archive.sha256` inside the archive).
    pub sha256: String,
    pub bytes: u64,
    pub files: usize,
}

/// Pack `project_root` into a zip at `dest`, embedding a per-file SHA-256
/// manifest and a whole-archive digest entry (`felin-archive.sha256`).
///
/// The digest is `sha256` over a canonical concatenation of every file's
/// `path:sha256` (paths sorted) — deterministic, content-based, and independent
/// of the zip container's framing bytes. On import the digest is recomputed and
/// compared, so any file tampering or reordering is caught.
///
/// `on_progress` (optional) is invoked between files so a UI can report how far
/// the packing got. It must not block for long — it's called from the calling
/// thread.
pub fn export_project(
    project_root: &Path,
    slug: &str,
    dest: &Path,
    on_progress: Option<impl FnMut(ArchiveProgress)>,
) -> Result<ExportOutcome> {
    let mut rels = Vec::new();
    collect_files(project_root, project_root, &mut rels)?;
    rels.sort();
    rels.retain(|r| {
        r != Path::new(LOCK_NAME)
            && r != Path::new(MANIFEST_NAME)
            && r != Path::new(DIGEST_NAME)
    });

    let total = rels.len();
    let mut on_progress = on_progress;
    if let Some(cb) = on_progress.as_mut() {
        cb(ArchiveProgress::Start { total_files: total });
    }

    let mut file = std::fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(&mut file);
    // zip container (widely inspectable) with zstd entries for a better ratio.
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Zstd);

    let mut entries = Vec::new();
    for (i, rel) in rels.iter().enumerate() {
        let bytes = std::fs::read(project_root.join(rel))?;
        let rel_str = rel_to_unix(rel);
        zip.start_file(format!("{slug}/{rel_str}"), opts).map_err(zip_err)?;
        zip.write_all(&bytes)?;
        entries.push(ArchiveFile {
            path: rel_str,
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
        });
        if let Some(cb) = on_progress.as_mut() {
            cb(ArchiveProgress::Progress { done: i + 1, total_files: total });
        }
    }

    let files = entries.len();
    let manifest = ArchiveManifest {
        schema: ARCHIVE_SCHEMA,
        slug: slug.to_string(),
        created_at: now_iso8601(),
        files: entries,
    };
    zip.start_file(format!("{slug}/{MANIFEST_NAME}"), opts).map_err(zip_err)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;

    // Whole-archive digest over the (sorted) per-file hashes — deterministic
    // and independent of the zip container. Stored as an entry inside the zip.
    let digest = archive_digest(&manifest);
    zip.start_file(format!("{slug}/{DIGEST_NAME}"), opts).map_err(zip_err)?;
    zip.write_all(digest.as_bytes())?;
    zip.finish().map_err(zip_err)?;
    drop(file);

    let bytes = std::fs::metadata(dest)?.len();
    tracing::debug!(dest = %dest.display(), files, bytes, digest, "project archive exported");

    Ok(ExportOutcome { sha256: digest, bytes, files })
}

/// Restore a project from `archive` into `projects_dir/<slug>`, verifying every
/// file's SHA-256 against the embedded manifest and the whole-archive digest
/// entry. Fails if a checksum mismatches or a project with the same slug
/// already exists. Returns the slug.
pub fn import_project(archive: &Path, projects_dir: &Path) -> Result<String> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(zip_err)?;

    std::fs::create_dir_all(projects_dir)?;
    let tmp = projects_dir.join(format!(".import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)?;
    let _guard = TmpGuard(tmp.clone());

    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut digest_bytes: Option<Vec<u8>> = None;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(zip_err)?;
        let rel = sanitize_zip_name(f.name())?;
        let out_path = tmp.join(&rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        match rel.file_name().and_then(|n| n.to_str()) {
            Some(MANIFEST_NAME) => manifest_bytes = Some(buf),
            Some(DIGEST_NAME) => digest_bytes = Some(buf),
            _ => std::fs::write(&out_path, &buf)?,
        }
    }
    drop(zip);

    let manifest: ArchiveManifest = serde_json::from_slice(
        &manifest_bytes.ok_or_else(|| Error::archive("archive is missing its manifest"))?,
    )
    .map_err(|e| Error::archive(format!("bad archive manifest: {e}")))?;
    if manifest.schema != ARCHIVE_SCHEMA {
        return Err(Error::archive(format!("unsupported archive schema {}", manifest.schema)));
    }
    let slug = manifest.slug.clone();
    if !is_single_component(&slug) {
        return Err(Error::archive(format!("archive has an unsafe slug: {slug:?}")));
    }

    // Per-file checks: each stored file must match the manifest's SHA-256.
    for entry in &manifest.files {
        let bytes = std::fs::read(tmp.join(&slug).join(&entry.path))
            .map_err(|e| Error::archive(format!("archived file '{}' is missing: {e}", entry.path)))?;
        if sha256_hex(&bytes) != entry.sha256 {
            return Err(Error::archive(format!("checksum mismatch for '{}'", entry.path)));
        }
    }

    // Whole-archive check: the embedded digest must match a recomputation over
    // the manifest's per-file hashes.
    let digest_buf = digest_bytes.ok_or_else(|| Error::archive("archive is missing its digest"))?;
    let embedded = std::str::from_utf8(&digest_buf)
        .map_err(|_| Error::archive("archive digest is not valid UTF-8"))?
        .trim();
    let expected = archive_digest(&manifest);
    if embedded != expected {
        return Err(Error::archive(format!(
            "archive digest mismatch: {embedded} != {expected}"
        )));
    }

    let final_dir = projects_dir.join(&slug);
    if final_dir.exists() {
        return Err(Error::archive(format!("a project '{slug}' already exists; remove it first")));
    }
    // Drop the in-archive manifest + digest from the restored tree (metadata).
    let _ = std::fs::remove_file(tmp.join(&slug).join(MANIFEST_NAME));
    let _ = std::fs::remove_file(tmp.join(&slug).join(DIGEST_NAME));
    std::fs::rename(tmp.join(&slug), &final_dir)?;
    Ok(slug)
}

/// Deterministic whole-archive digest: `sha256` of `"<path>:<sha256>\n"` lines
/// for every file, paths sorted (paths are already unique inside the zip).
fn archive_digest(manifest: &ArchiveManifest) -> String {
    let mut lines: Vec<String> = manifest
        .files
        .iter()
        .map(|f| format!("{}:{}\n", f.path, f.sha256))
        .collect();
    lines.sort();
    let mut hasher = Sha256::new();
    for line in lines {
        hasher.update(line.as_bytes());
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

// PH_HELPERS

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            // OCR batch staging (image symlinks/copies of the user's source
            // files) is never part of the portable archive.
            if path.file_name().and_then(|n| n.to_str()) == Some(INPUTS_DIR) {
                continue;
            }
            collect_files(root, &path, out)?;
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
        }
    }
    Ok(())
}

fn rel_to_unix(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Turn a zip entry name into a safe relative path, rejecting absolute paths and
/// any `..` traversal (zip-slip guard).
fn sanitize_zip_name(name: &str) -> Result<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute()
        || p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(Error::archive(format!("unsafe path in archive: {name:?}")));
    }
    Ok(p.components().filter(|c| matches!(c, Component::Normal(_))).collect())
}

fn is_single_component(slug: &str) -> bool {
    let mut it = Path::new(slug).components();
    matches!(it.next(), Some(Component::Normal(_))) && it.next().is_none()
}

fn zip_err(e: zip::result::ZipError) -> Error {
    Error::archive(format!("zip error: {e}"))
}

/// Removes a temp directory on drop (used to clean up a partial import).
struct TmpGuard(PathBuf);
impl Drop for TmpGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
