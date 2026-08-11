//! Archive export/import integration tests: zip round-trip + SHA-256
//! verification + tamper detection + OCR staging exclusion.
//!
//! Moved here from the crate's inline `#[cfg(test)]` module (project policy: no
//! test code alongside application code).

use felin_core::archive::{export_project, import_project, ArchiveProgress, ExportOutcome};

#[test]
fn export_then_import_roundtrips_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    // Fake a project tree.
    let proj = dir.path().join("projects").join("my-book");
    std::fs::create_dir_all(proj.join("ocr").join("t1")).unwrap();
    std::fs::write(proj.join("project.db"), b"fake-db-bytes").unwrap();
    std::fs::write(proj.join("project.json"), br#"{"slug":"my-book","name":"My Book","created_at":"t"}"#).unwrap();
    std::fs::write(proj.join("ocr").join("t1").join("page-0001.json"), br#"{"page":1}"#).unwrap();
    std::fs::write(proj.join("project.lock"), b"pid").unwrap(); // must be excluded

    // OCR batch staging inputs (symlink/copy of user sources) must be excluded.
    std::fs::create_dir_all(proj.join("ocr").join("t2").join("inputs")).unwrap();
    std::fs::write(proj.join("ocr").join("t2").join("inputs").join("img-001.png"), b"PNG-data").unwrap();

    let archive = dir.path().join("my-book.felinproj");
    let mut seen: Vec<ArchiveProgress> = Vec::new();
    let out: ExportOutcome = {
        let mut cb = |p: ArchiveProgress| seen.push(p);
        export_project(&proj, "my-book", &archive, Some(&mut cb)).unwrap()
    };
    assert!(archive.exists());
    // The whole-archive digest is a file *inside* the archive — no sidecar.
    assert!(!dir.path().join("my-book.felinproj.sha256").exists());
    assert_eq!(out.files, 3); // db, json, page — NOT the lock, NOT inputs/
    assert_eq!(seen.len(), out.files + 1); // Start + one Progress per file
    assert!(matches!(seen.first(), Some(ArchiveProgress::Start { total_files: 3 })));

    // Progress counters advance 1..=N.
    let progresses: Vec<usize> = seen
        .iter()
        .filter_map(|p| match p {
            ArchiveProgress::Progress { done, .. } => Some(*done),
            _ => None,
        })
        .collect();
    assert_eq!(progresses, vec![1, 2, 3]);

    // Import into a fresh projects dir and verify content restored.
    let dest_projects = dir.path().join("restored");
    let slug = import_project(&archive, &dest_projects).unwrap();
    assert_eq!(slug, "my-book");
    assert_eq!(std::fs::read(dest_projects.join("my-book").join("project.db")).unwrap(), b"fake-db-bytes");
    assert!(dest_projects.join("my-book").join("ocr").join("t1").join("page-0001.json").exists());
    assert!(!dest_projects.join("my-book").join("project.lock").exists());
    assert!(!dest_projects.join("my-book").join("ocr").join("t2").join("inputs").exists());
    // Metadata entries (manifest + digest) don't leak into the restored tree.
    assert!(!dest_projects.join("my-book").join("felin-archive.json").exists());
    assert!(!dest_projects.join("my-book").join("felin-archive.sha256").exists());

    // Re-importing the same slug must fail (no silent overwrite).
    assert!(import_project(&archive, &dest_projects).is_err());
}

#[test]
fn detects_tampering() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("p");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.db"), b"original").unwrap();
    let archive = dir.path().join("p.felinproj");
    export_project(&proj, "p", &archive, None::<fn(ArchiveProgress)>).unwrap();

    // (a) An intact archive imports fine — the embedded digest verifies.
    let dest = dir.path().join("restored");
    assert_eq!(import_project(&archive, &dest).unwrap(), "p");

    // (b) Flip a bit in a *file body*: the per-file manifest SHA catches it.
    let tampered = dir.path().join("tampered.felinproj");
    {
        let src = std::fs::File::open(&archive).unwrap();
        let mut zin = zip::ZipArchive::new(src).unwrap();
        let out = std::fs::File::create(&tampered).unwrap();
        let mut zout = zip::ZipWriter::new(out);
        let opts = zip::write::SimpleFileOptions::default();
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).unwrap();
            let name = f.name().to_string();
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf).unwrap();
            if name.ends_with("project.db") {
                buf = b"HACKED!!".to_vec();
            }
            zout.start_file(name, opts).unwrap();
            std::io::Write::write_all(&mut zout, &buf).unwrap();
        }
        zout.finish().unwrap();
    }
    let dest2 = dir.path().join("restored2");
    assert!(import_project(&tampered, &dest2).is_err(), "per-file manifest must detect tampering");

    // (c) Flip a bit in the *embedded digest entry*: whole-archive digest check
    // must fail (digest no longer matches the recomputed value).
    let tampered_digest = dir.path().join("tampered-digest.felinproj");
    {
        let src = std::fs::File::open(&archive).unwrap();
        let mut zin = zip::ZipArchive::new(src).unwrap();
        let out = std::fs::File::create(&tampered_digest).unwrap();
        let mut zout = zip::ZipWriter::new(out);
        let opts = zip::write::SimpleFileOptions::default();
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).unwrap();
            let name = f.name().to_string();
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf).unwrap();
            if name.ends_with("felin-archive.sha256") {
                let s = String::from_utf8_lossy(&buf);
                let flipped = flip_hex_char(&s);
                buf = flipped.into_bytes();
            }
            zout.start_file(name, opts).unwrap();
            std::io::Write::write_all(&mut zout, &buf).unwrap();
        }
        zout.finish().unwrap();
    }
    assert!(
        import_project(&tampered_digest, &dest2).is_err(),
        "corrupted embedded digest must be rejected"
    );
}

/// Flip one hex digit in a 64-char hex string.
fn flip_hex_char(s: &str) -> String {
    let mut bytes = s.as_bytes().to_vec();
    let idx = bytes.len() - 1;
    bytes[idx] = match bytes[idx] {
        b'0'..=b'8' => bytes[idx] + 1,
        _ => b'0',
    };
    String::from_utf8(bytes).unwrap()
}
