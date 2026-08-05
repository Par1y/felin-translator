//! Integration tests for the storage layer: the two typed DBs, the migration
//! runner (idempotency, pre-upgrade backup, forward-version refusal), and the
//! single-open project lock.

use felin_core::storage::{Db, DbTuning, GlobalDb, Migration, ProjectDb, ProjectLock};
use felin_core::types::{ExtractedNameStatus, IngestedParagraph, NameStatus, OcrParagraphStatus};

#[test]
fn global_db_upsert_and_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let g = GlobalDb::open(&dir.path().join("glossary.db")).unwrap();
    assert_eq!(g.count_names().unwrap(), 0);

    let id1 = g.upsert_name("田中", Some("田中"), "imported", NameStatus::Imported).unwrap();
    // Upserting the same japanese updates chinese, keeps one row and the same id.
    let id2 = g.upsert_name("田中", Some("Tanaka"), "draft", NameStatus::Draft).unwrap();
    assert_eq!(id1, id2);
    assert_eq!(g.count_names().unwrap(), 1);
    assert_eq!(g.chinese_for("田中").unwrap().as_deref(), Some("Tanaka"));
    assert_eq!(g.chinese_for("不存在").unwrap(), None);
}

#[test]
fn project_db_chapters_paragraphs_settings() {
    let dir = tempfile::tempdir().unwrap();
    let p = ProjectDb::open(&dir.path().join("project.db")).unwrap();

    let ch = p.get_or_create_chapter("Chapter 1").unwrap();
    assert_eq!(p.get_or_create_chapter("Chapter 1").unwrap(), ch, "idempotent by title");

    // Each ingest mints fresh paragraph UUIDs, so build a new batch per insert.
    let mk_paras = || {
        vec![
            IngestedParagraph::new(
                "第一段。".into(),
                Some(1),
                "book.pdf".into(),
                Some(0.9),
                OcrParagraphStatus::Ok,
                serde_json::json!({"best_score": 0.9}),
            ),
            IngestedParagraph::new(
                "第二段。".into(),
                Some(1),
                "book.pdf".into(),
                None,
                OcrParagraphStatus::LowScore,
                serde_json::Value::Null,
            ),
        ]
    };
    assert_eq!(p.insert_paragraphs(ch, &mk_paras()).unwrap(), 2);
    assert_eq!(p.count_paragraphs().unwrap(), 2);

    let listed = p.list_paragraphs(ch).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!((listed[0].ord, listed[1].ord), (0, 1));
    assert_eq!(listed[0].page_score, Some(0.9));
    assert_eq!(listed[1].page_score, None); // nullable score round-trips as NULL
    assert_eq!(listed[0].ocr_status, OcrParagraphStatus::Ok);
    assert_eq!(listed[1].ocr_status, OcrParagraphStatus::LowScore);
    assert!(listed[0].ocr_meta.is_some());
    assert!(listed[1].ocr_meta.is_none());

    // A second insert appends, continuing the ordinal.
    p.insert_paragraphs(ch, &mk_paras()).unwrap();
    let listed2 = p.list_paragraphs(ch).unwrap();
    assert_eq!(listed2.len(), 4);
    assert_eq!(listed2[3].ord, 3);

    p.set_setting("budget", "3000").unwrap();
    assert_eq!(p.get_setting("budget").unwrap().as_deref(), Some("3000"));
    p.set_setting("budget", "4000").unwrap(); // upsert overwrites
    assert_eq!(p.get_setting("budget").unwrap().as_deref(), Some("4000"));
    assert_eq!(p.get_setting("missing").unwrap(), None);
}

#[test]
fn reopening_runs_no_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.db");
    let ch = {
        let p = ProjectDb::open(&path).unwrap();
        p.create_chapter("c", 0).unwrap()
    };
    // Reopen: migrations already applied, data persists, no error.
    let p = ProjectDb::open(&path).unwrap();
    assert_eq!(p.list_chapters().unwrap().len(), 1);
    assert_eq!(p.list_chapters().unwrap()[0].id, ch);
}

#[test]
fn migration_backs_up_before_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    let v1: &[Migration] = &[Migration { version: 1, sql: "CREATE TABLE a(x);" }];
    let v2: &[Migration] =
        &[Migration { version: 1, sql: "CREATE TABLE a(x);" }, Migration { version: 2, sql: "CREATE TABLE b(y);" }];

    {
        let db = Db::open(&path, v1, true, DbTuning::default()).unwrap();
        db.write(|c| {
            c.execute("INSERT INTO a(x) VALUES (1)", [])?;
            Ok(())
        })
        .unwrap();
    }
    {
        // Upgrading to v2 must back up the existing DB to `t.db.bak-1`.
        let db = Db::open(&path, v2, true, DbTuning::default()).unwrap();
        let n: i64 = db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM b", [], |r| r.get(0))?)).unwrap();
        assert_eq!(n, 0);
    }
    assert!(dir.path().join("t.db.bak-1").exists(), "expected pre-upgrade backup");
}

#[test]
fn refuses_forward_versioned_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    let v1: &[Migration] = &[Migration { version: 1, sql: "CREATE TABLE a(x);" }];

    {
        let db = Db::open(&path, v1, true, DbTuning::default()).unwrap();
        // Simulate a DB created by a newer app build.
        db.write(|c| {
            c.execute("INSERT INTO _schema_version(version, applied_at) VALUES (999, 'future')", [])?;
            Ok(())
        })
        .unwrap();
    }
    let err = Db::open(&path, v1, true, DbTuning::default()).unwrap_err();
    assert!(
        matches!(err, felin_core::Error::SchemaTooNew { found: 999, supported_max: 1 }),
        "got {err:?}"
    );
}

#[test]
fn project_lock_is_exclusive_and_released_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let first = ProjectLock::acquire(dir.path()).unwrap();
    let second = ProjectLock::acquire(dir.path());
    assert!(matches!(second, Err(felin_core::Error::ProjectLocked { .. })));
    drop(first);
    // Once released, it can be acquired again.
    let _third = ProjectLock::acquire(dir.path()).unwrap();
}

#[test]
fn glossary_and_extracted_names_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let g = GlobalDb::open(&dir.path().join("glossary.db")).unwrap();
    let id = g
        .upsert_name_full("田中角栄", Some("田中角荣"), Some("Tanaka"), None, None, "imported", NameStatus::Imported)
        .unwrap();
    g.add_alias(id, "田中").unwrap();

    let forms = g.glossary_forms().unwrap();
    assert!(forms.iter().any(|(f, i)| f == "田中角栄" && *i == id));
    assert!(forms.iter().any(|(f, i)| f == "田中" && *i == id));
    assert_eq!(g.list_names(10).unwrap().len(), 1);

    let p = ProjectDb::open(&dir.path().join("project.db")).unwrap();
    assert!(p.insert_extracted("猫", Some("猫"), Some("主人公のペット")).unwrap().is_some());
    // Same japanese is deduped.
    assert!(p.insert_extracted("猫", Some("ねこ"), None).unwrap().is_none());

    let new = p.list_extracted_names(Some(ExtractedNameStatus::New)).unwrap();
    assert_eq!(new.len(), 1);
    p.set_extracted_status(new[0].id, ExtractedNameStatus::Confirmed, Some(id)).unwrap();
    assert!(p.list_extracted_names(Some(ExtractedNameStatus::New)).unwrap().is_empty());
    let confirmed = p.list_extracted_names(Some(ExtractedNameStatus::Confirmed)).unwrap();
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].matched_name_id, Some(id));
}

#[test]
fn segment_cleans_detects_chapters_and_builds_tus() {
    let dir = tempfile::tempdir().unwrap();
    let p = ProjectDb::open(&dir.path().join("project.db")).unwrap();
    let ch = p.get_or_create_chapter("import").unwrap();

    let mk = |t: &str| {
        IngestedParagraph::new(
            t.into(),
            Some(1),
            "b.pdf".into(),
            None,
            OcrParagraphStatus::Ok,
            serde_json::Value::Null,
        )
    };
    // A PDF-header artifact (dropped), two chapters, a score tag (cleaned).
    let paras = vec![
        mk("--- PDF (2 pages) ---"),
        mk("第一章 出会い"),
        mk("本文A。[Score: 0.90]"),
        mk("第二章 別れ"),
        mk("本文B。"),
    ];
    p.insert_paragraphs(ch, &paras).unwrap();

    let out = p.segment(3000, "正文", &felin_core::seg::ChapterRecognizer::default()).unwrap();
    assert_eq!(out.chapters, 2);

    let chapters = p.list_chapters().unwrap();
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[0].title, "第一章 出会い");
    assert_eq!(chapters[1].title, "第二章 別れ");

    // The artifact paragraph was dropped; the score tag was cleaned.
    let c1 = p.list_paragraphs(chapters[0].id).unwrap();
    assert_eq!(c1.len(), 2);
    assert_eq!(c1[1].text, "本文A。");

    // Small chapter → one TU covering both its paragraphs.
    assert!(p.count_tus().unwrap() >= 2);
    let tus1 = p.list_tus(chapters[0].id).unwrap();
    assert_eq!(tus1.len(), 1);
    assert_eq!(tus1[0].paragraph_ids.len(), 2);
    // The TU references the surviving paragraphs' UUIDs.
    assert_eq!(tus1[0].paragraph_ids[0], c1[0].id);
}
