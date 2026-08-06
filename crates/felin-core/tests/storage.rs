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

#[test]
fn small_glossary_crud_and_matcher_filters_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let p = ProjectDb::open(&dir.path().join("project.db")).unwrap();

    let id = p
        .insert_glossary_entry(None, "田中", Some("田中"), None, None, &["人名".into()], &["たなか".into()], None)
        .unwrap();
    let entries = p.list_glossary_entries(None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].chinese.as_deref(), Some("田中"));
    assert_eq!(entries[0].tags, vec!["人名"]);
    assert_eq!(entries[0].aliases, vec!["たなか"]);
    assert!(entries[0].enabled);

    // Upsert by japanese refreshes the value fields wholesale (tags/aliases
    // replaced with what the caller passes), keeps the row and id.
    let again = p
        .insert_glossary_entry(
            Some(7),
            "田中",
            Some("Tanaka"),
            None,
            None,
            &["人名".into(), "专名".into()],
            &["たなか".into(), "中".into()],
            Some("注"),
        )
        .unwrap();
    assert_eq!(id, again);
    let entries = p.list_glossary_entries(None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].chinese.as_deref(), Some("Tanaka"));
    assert_eq!(entries[0].name_global_id, Some(7));
    assert_eq!(entries[0].tags, vec!["人名", "专名"]);

    // Aliases + japanese feed the matcher; disabled entries are excluded.
    assert_eq!(p.matcher_entries().unwrap().len(), 1);
    p.set_entry_enabled(id, false).unwrap();
    assert!(p.matcher_entries().unwrap().is_empty(), "disabled entries never reach the matcher");
    p.set_entry_enabled(id, true).unwrap();

    // Tag set + free-text search.
    p.set_entry_tags(id, &["地名".into(), "历史".into()]).unwrap();
    assert_eq!(p.list_glossary_entries(Some("历史")).unwrap().len(), 1);
    assert_eq!(p.list_glossary_entries(Some("不存在")).unwrap().len(), 0);
    // Search hits aliases too.
    p.update_glossary_entry(id, "田中", Some("田中"), None, None, &["人名".into()], &["たなか".into(), "中".into()], None)
        .unwrap();
    assert_eq!(p.list_glossary_entries(Some("たなか")).unwrap().len(), 1);
    assert_eq!(p.list_glossary_entries(Some("中")).unwrap().len(), 1);

    p.delete_glossary_entry(id).unwrap();
    assert!(p.list_glossary_entries(None).unwrap().is_empty());
}

#[test]
fn source_override_and_editable_translation() {
    let dir = tempfile::tempdir().unwrap();
    let p = ProjectDb::open(&dir.path().join("project.db")).unwrap();
    let ch = p.get_or_create_chapter("c").unwrap();
    let para = IngestedParagraph::new(
        "原文段落。".into(),
        None,
        "b.pdf".into(),
        None,
        OcrParagraphStatus::Ok,
        serde_json::Value::Null,
    );
    p.insert_paragraphs(ch, &[para.clone()]).unwrap();
    let tu = p
        .db()
        .write(|c| {
            c.execute(
                "INSERT INTO tus (chapter_id, paragraph_ids, ord, budget, status)
                 VALUES (?1, ?2, 0, NULL, 'translated')",
                rusqlite::params![ch, serde_json::to_string(&vec![para.id.to_string()]).unwrap()],
            )?;
            Ok(c.last_insert_rowid())
        })
        .unwrap();

    // Concatenated paragraphs are the default source; the override wins verbatim.
    assert_eq!(p.tu_source(tu).unwrap(), "原文段落。");
    p.set_tu_source(tu, "用户改写的原文").unwrap();
    assert_eq!(p.tu_source(tu).unwrap(), "用户改写的原文");
    // list_tus_with_translations resolves the same effective source.
    let listed = p.list_tus_with_translations(ch).unwrap();
    assert_eq!(listed[0].source, "用户改写的原文");
    // Clearing the override falls back to the paragraphs.
    p.set_tu_source(tu, "   ").unwrap();
    assert_eq!(p.tu_source(tu).unwrap(), "原文段落。");

    // Editing the translation text demotes an approved TU to reviewing.
    p.approve_tu(tu).unwrap();
    let demoted = p.set_translation_text(tu, "手工译文").unwrap();
    assert!(demoted, "approve → reviewing on edit");
    assert_eq!(p.get_tu(tu).unwrap().unwrap().status, felin_core::types::TuStatus::Reviewing);
    let t = p.get_translation(tu).unwrap().unwrap();
    assert_eq!(t.final_text.as_deref(), Some("手工译文"));
    assert_eq!(t.status, felin_core::types::TranslationStatus::Draft);

    // Batch retranslate stamps instructions and requeues.
    let n = p.retranslate_tus(&[tu], Some("重新翻译，注意敬语")).unwrap();
    assert_eq!(n, 1);
    assert_eq!(p.get_tu(tu).unwrap().unwrap().status, felin_core::types::TuStatus::Queued);
    assert_eq!(
        p.get_translation(tu).unwrap().unwrap().instruction.as_deref(),
        Some("重新翻译，注意敬语")
    );
}

#[test]
fn export_translations_is_deterministic_and_records_paths() {
    let dir = tempfile::tempdir().unwrap();
    let p = ProjectDb::open(&dir.path().join("project.db")).unwrap();
    let ch_a = p.get_or_create_chapter("第一章 出会い").unwrap();
    let ch_b = p.get_or_create_chapter("第二章 別れ").unwrap();

    // Two translated TUs in chapter A, one empty (excluded), one in chapter B.
    let mk_tu = |ch: i64, ord: i64, final_text: Option<&str>| {
        let para = IngestedParagraph::new(
            format!("原文{ord}。"),
            None,
            "b.pdf".into(),
            None,
            OcrParagraphStatus::Ok,
            serde_json::Value::Null,
        );
        p.insert_paragraphs(ch, &[para.clone()]).unwrap();
        let tu = p
            .db()
            .write(|c| {
                c.execute(
                    "INSERT INTO tus (chapter_id, paragraph_ids, ord, budget, status)
                     VALUES (?1, ?2, ?3, NULL, 'approved')",
                    rusqlite::params![ch, serde_json::to_string(&vec![para.id.to_string()]).unwrap(), ord],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        if let Some(text) = final_text {
            p.set_translation_text(tu, text).unwrap();
        }
    };
    mk_tu(ch_a, 0, Some("第一句译文。"));
    mk_tu(ch_a, 1, Some("第二句译文。"));
    mk_tu(ch_a, 2, None); // empty final_text → excluded
    mk_tu(ch_b, 0, Some("第二章译文。"));

    let dest = dir.path().join("export");
    let out = p.export_translations(&dest).unwrap();
    assert_eq!(out.tus, 3);

    let txt = std::fs::read_to_string(&out.txt_path).unwrap();
    assert_eq!(
        txt,
        "# 第一章 出会い\n第一句译文。\n第二句译文。\n\n# 第二章 別れ\n第二章译文。\n\n"
    );
    let csv = std::fs::read_to_string(&out.csv_path).unwrap();
    let rows: Vec<&str> = csv.lines().collect();
    assert_eq!(rows[0], "章号,章节标题,序号,原文,译文,状态");
    assert_eq!(rows[1], "0,第一章 出会い,0,原文0。,第一句译文。,approved");
    assert_eq!(rows[2], "0,第一章 出会い,1,原文1。,第二句译文。,approved");
    assert_eq!(rows[3], "1,第二章 別れ,0,原文0。,第二章译文。,approved");
    // Both paths were recorded in the exports table.
    let n: i64 = p
        .db()
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM exports", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(n, 2);
}

#[test]
fn ocr_settings_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let p = ProjectDb::open(&dir.path().join("project.db")).unwrap();
    let d = p.get_ocr_settings().unwrap();
    assert_eq!(d.batch_workers, 4);
    assert!(!d.batch_recursive);
    p.set_ocr_settings(&felin_core::types::OcrSettings { batch_workers: 2, batch_recursive: true }).unwrap();
    let back = p.get_ocr_settings().unwrap();
    assert_eq!(back.batch_workers, 2);
    assert!(back.batch_recursive);
}
