//! Portable project archives.
//!
//! Packs a project's app-side data (project.db, OCR products, metadata) into a
//! single zstd-compressed zip with a SHA-256 per file plus an overall archive
//! digest, so a user can move / rename / back up a project even though the
//! internal data normally lives next to the software. The user's *source
//! inputs* are never included — they stay in place.

use crate::error::{Error, Result};
use crate::util::now_iso8601;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

const ARCHIVE_SCHEMA: i64 = 1;
const MANIFEST_NAME: &str = "felin-archive.json";
const LOCK_NAME: &str = "project.lock";

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

/// Result of exporting a project.
#[derive(Debug, Clone)]
pub struct ExportOutcome {
    /// SHA-256 of the whole archive file (also written to `<archive>.sha256`).
    pub sha256: String,
    pub bytes: u64,
    pub files: usize,
}

/// Pack `project_root` into a zip at `dest`, embedding a per-file SHA-256
/// manifest and writing a `<dest>.sha256` sidecar with the archive digest.
pub fn export_project(project_root: &Path, slug: &str, dest: &Path) -> Result<ExportOutcome> {
    let mut rels = Vec::new();
    collect_files(project_root, project_root, &mut rels)?;
    rels.sort();
    rels.retain(|r| r != Path::new(LOCK_NAME) && r != Path::new(MANIFEST_NAME));

    let file = std::fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(file);
    // zip container (widely inspectable) with zstd entries for a better ratio.
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Zstd);

    let mut entries = Vec::new();
    for rel in &rels {
        let bytes = std::fs::read(project_root.join(rel))?;
        let rel_str = rel_to_unix(rel);
        zip.start_file(format!("{slug}/{rel_str}"), opts).map_err(zip_err)?;
        zip.write_all(&bytes)?;
        entries.push(ArchiveFile { path: rel_str, sha256: sha256_hex(&bytes), size: bytes.len() as u64 });
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
    zip.finish().map_err(zip_err)?;

    // Digest of the whole archive, for external integrity checks.
    let archive_bytes = std::fs::read(dest)?;
    let sha256 = sha256_hex(&archive_bytes);
    let sidecar = append_ext(dest, "sha256");
    let name = dest.file_name().and_then(|s| s.to_str()).unwrap_or("archive");
    std::fs::write(sidecar, format!("{sha256}  {name}\n"))?;

    tracing::debug!(dest = %dest.display(), files, bytes = archive_bytes.len(), "project archive exported");

    Ok(ExportOutcome { sha256, bytes: archive_bytes.len() as u64, files })
}

/// Restore a project from `archive` into `projects_dir/<slug>`, verifying every
/// file's SHA-256 against the embedded manifest. Fails if a checksum mismatches
/// or a project with the same slug already exists. Returns the slug.
pub fn import_project(archive: &Path, projects_dir: &Path) -> Result<String> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(zip_err)?;

    std::fs::create_dir_all(projects_dir)?;
    let tmp = projects_dir.join(format!(".import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)?;
    let _guard = TmpGuard(tmp.clone());

    let mut manifest_bytes: Option<Vec<u8>> = None;
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
        if rel.file_name() == Some(std::ffi::OsStr::new(MANIFEST_NAME)) {
            manifest_bytes = Some(buf.clone());
        }
        std::fs::write(&out_path, &buf)?;
    }

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

    for entry in &manifest.files {
        let bytes = std::fs::read(tmp.join(&slug).join(&entry.path))
            .map_err(|e| Error::archive(format!("archived file '{}' is missing: {e}", entry.path)))?;
        if sha256_hex(&bytes) != entry.sha256 {
            return Err(Error::archive(format!("checksum mismatch for '{}'", entry.path)));
        }
    }

    let final_dir = projects_dir.join(&slug);
    if final_dir.exists() {
        return Err(Error::archive(format!("a project '{slug}' already exists; remove it first")));
    }
    // Drop the in-archive manifest from the restored tree (it's metadata).
    let _ = std::fs::remove_file(tmp.join(&slug).join(MANIFEST_NAME));
    std::fs::rename(tmp.join(&slug), &final_dir)?;
    Ok(slug)
}

// PH_HELPERS

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
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

fn append_ext(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
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

