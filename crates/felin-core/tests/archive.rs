//! Archive export/import integration tests: zip round-trip + SHA-256
//! verification + tamper detection.
//!
//! Moved here from the crate's inline `#[cfg(test)]` module (project policy: no
//! test code alongside application code).

use felin_core::archive::{export_project, import_project, ExportOutcome};
use std::io::{Read, Write};

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

    let archive = dir.path().join("my-book.felinproj");
    let out: ExportOutcome = export_project(&proj, "my-book", &archive).unwrap();
    assert!(archive.exists());
    assert!(dir.path().join("my-book.felinproj.sha256").exists());
    assert_eq!(out.files, 3); // db, json, page — NOT the lock

    // Import into a fresh projects dir.
    let dest_projects = dir.path().join("restored");
    let slug = import_project(&archive, &dest_projects).unwrap();
    assert_eq!(slug, "my-book");
    assert_eq!(std::fs::read(dest_projects.join("my-book").join("project.db")).unwrap(), b"fake-db-bytes");
    assert!(dest_projects.join("my-book").join("ocr").join("t1").join("page-0001.json").exists());
    assert!(!dest_projects.join("my-book").join("project.lock").exists());

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
    export_project(&proj, "p", &archive).unwrap();

    // Corrupt one stored file inside the zip by rebuilding it with altered bytes.
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
            f.read_to_end(&mut buf).unwrap();
            if name.ends_with("project.db") {
                buf = b"HACKED!!".to_vec();
            }
            zout.start_file(name, opts).unwrap();
            zout.write_all(&buf).unwrap();
        }
        zout.finish().unwrap();
    }
    let dest = dir.path().join("restored");
    assert!(import_project(&tampered, &dest).is_err(), "tampered archive must fail verification");
}
