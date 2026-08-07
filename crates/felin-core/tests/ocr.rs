//! OCR integration tests: txt import + encoding detection, image selection,
//! paragraph ingestion, batch parsing/ingest, and in-place config.yaml editing.
//!
//! Moved here from the crate's inline `#[cfg(test)]` modules (project policy:
//! no test code alongside application code).

use felin_core::config::DEFAULT_SENTENCE_ENDERS;
use felin_core::ocr::{
    config::{self, OcrConfig},
    ingest::{self, PageForIngest},
    select::{self, ImageMatchRule, ImagePreset},
    txt,
};
use felin_core::types::OcrParagraphStatus;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

/// Default sentence enders for tests.
fn e() -> Vec<char> {
    DEFAULT_SENTENCE_ENDERS.chars().collect()
}

// ----- ocr/txt ---------------------------------------------------------------

#[test]
fn utf8_import_splits_paragraphs() {
    let paras = txt::import_txt("第一段。\n\n第二段。".as_bytes(), "a.txt").unwrap();
    assert_eq!(paras.len(), 2);
    assert_eq!(paras[0].text, "第一段。");
    assert_eq!(paras[0].page_num, None); // txt has no pages
    assert_eq!(paras[0].source_file, "a.txt");
}

#[test]
fn strips_utf8_bom() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice("あ".as_bytes());
    let (s, enc) = txt::decode_bytes(&bytes);
    assert_eq!(s, "あ");
    assert_eq!(enc, encoding_rs::UTF_8);
}

// A sentence long enough to give chardetng a strong signal.
const JA: &str = "吾輩は猫である。名前はまだ無い。どこで生れたか頓と見当がつかぬ。\
                  何でも薄暗いじめじめした所でニャーニャー泣いていた事だけは記憶している。";

#[test]
fn detects_and_decodes_shift_jis() {
    let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(JA);
    let (decoded, enc) = txt::decode_bytes(&bytes);
    // Round-trip is the real correctness check: exact decode ⇒ correct detection.
    assert_eq!(decoded, JA, "detected {} instead", enc.name());
    assert_ne!(enc, encoding_rs::UTF_8);
}

#[test]
fn detects_and_decodes_euc_jp() {
    let (bytes, _, _) = encoding_rs::EUC_JP.encode(JA);
    let (decoded, enc) = txt::decode_bytes(&bytes);
    assert_eq!(decoded, JA, "detected {} instead", enc.name());
    assert_ne!(enc, encoding_rs::UTF_8);
}

#[test]
fn decodes_utf16le_with_bom() {
    let mut bytes = vec![0xFF, 0xFE];
    for u in JA.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    let (decoded, enc) = txt::decode_bytes(&bytes);
    assert_eq!(decoded, JA);
    assert_eq!(enc, encoding_rs::UTF_16LE);
}

// ----- ocr/select -------------------------------------------------------------

fn write(dir: &Path, names: &[&str]) {
    for n in names {
        std::fs::write(dir.join(n), "x").unwrap();
    }
}

fn rule(preset: ImagePreset) -> ImageMatchRule {
    ImageMatchRule { preset, custom_glob: None, custom_regex: None, range: None }
}

fn file_names(sel: &[PathBuf]) -> Vec<String> {
    sel.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
}

#[test]
fn natural_sort_puts_number_prefixes_first() {
    let dir = tempdir().unwrap();
    write(dir.path(), &["155a.jpg", "003.png", "cover.png", "001.png", "10.png"]);
    let sel = select::select_images(dir.path(), &rule(ImagePreset::All)).unwrap();
    assert_eq!(file_names(&sel), vec!["001.png", "003.png", "10.png", "155a.jpg", "cover.png"]);
}

#[test]
fn pdfs_are_never_selected() {
    let dir = tempdir().unwrap();
    write(dir.path(), &["a.png", "b.pdf", "c.jpg"]);
    let sel = select::select_images(dir.path(), &rule(ImagePreset::All)).unwrap();
    assert_eq!(file_names(&sel), vec!["a.png", "c.jpg"]);
}

#[test]
fn preset_filters_by_extension_and_shape() {
    let dir = tempdir().unwrap();
    write(dir.path(), &["a.png", "b.jpg", "155.png", "155a.png", "155a.jpg", "c.jpeg"]);

    let png = select::select_images(dir.path(), &rule(ImagePreset::Png)).unwrap();
    assert_eq!(file_names(&png), vec!["155.png", "155a.png", "a.png"]);

    let jpg = select::select_images(dir.path(), &rule(ImagePreset::Jpg)).unwrap();
    assert_eq!(file_names(&jpg), vec!["155a.jpg", "b.jpg", "c.jpeg"]);

    let num = select::select_images(dir.path(), &rule(ImagePreset::Numbered)).unwrap();
    assert_eq!(file_names(&num), vec!["155.png"]);

    let numpref = select::select_images(dir.path(), &rule(ImagePreset::NumberedPrefix)).unwrap();
    assert_eq!(file_names(&numpref), vec!["155.png", "155a.jpg", "155a.png"]);
}

#[test]
fn custom_glob_and_regex_override_preset() {
    let dir = tempdir().unwrap();
    write(dir.path(), &["a.png", "b.jpg", "keep1.png", "keep2.png"]);

    let mut r = rule(ImagePreset::Png);
    r.custom_glob = Some("keep*.png".to_string());
    let sel = select::select_images(dir.path(), &r).unwrap();
    assert_eq!(file_names(&sel), vec!["keep1.png", "keep2.png"]);

    let mut r2 = rule(ImagePreset::All);
    r2.custom_regex = Some(r"^(a|b)\.".to_string());
    let sel = select::select_images(dir.path(), &r2).unwrap();
    assert_eq!(file_names(&sel), vec!["a.png", "b.jpg"]);
}

#[test]
fn range_cuts_by_natural_order() {
    let dir = tempdir().unwrap();
    write(dir.path(), &["001.png", "002.png", "003.png", "004.png"]);
    let mut r = rule(ImagePreset::All);
    r.range = Some((2, 3));
    let sel = select::select_images(dir.path(), &r).unwrap();
    assert_eq!(file_names(&sel), vec!["002.png", "003.png"]);
}

#[test]
fn missing_dir_is_an_error() {
    let err = select::select_images(Path::new("/definitely/not/here"), &rule(ImagePreset::All)).unwrap_err();
    assert!(err.to_string().contains("cannot read directory"));
}

// ----- ocr/ingest --------------------------------------------------------------

fn page(page: i64, text: &str) -> PageForIngest {
    PageForIngest {
        page,
        text: text.to_string(),
        score: None,
        quality_warning: false,
        blank: false,
        best_score: None,
        fallback: false,
        source_file: "book.pdf".into(),
        recovered: false,
    }
}

#[test]
fn split_blocks_handles_blanks_and_internal_newlines() {
    assert_eq!(ingest::split_blocks("a\n\nb"), vec!["a", "b"]);
    // internal single newlines within a block are preserved (reversible)
    assert_eq!(ingest::split_blocks("a\nb\n\nc"), vec!["a\nb", "c"]);
    // leading/trailing/whitespace-only lines are dropped
    assert_eq!(ingest::split_blocks("\n\n  \n a \n\n"), vec!["a"]);
    assert!(ingest::split_blocks("   \n  \t ").is_empty());
}

#[test]
fn sentence_enders_recognized() {
    for s in ["これは。", "終わり！", "何？", "「セリフ」", "『本』", "end.", "done。  "] {
        assert!(ingest::ends_with_sentence_punct(s, &e()), "should end a sentence: {s:?}");
    }
    for s in ["未完", "続く ", "no punct", "途中で"] {
        assert!(!ingest::ends_with_sentence_punct(s, &e()), "should NOT end a sentence: {s:?}");
    }
}

#[test]
fn no_merge_when_tail_ends_in_punctuation() {
    let out = ingest::build_paragraphs(&[page(1, "第一段。"), page(2, "第二段。")], 0.6, &e());
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].text, "第一段。");
    assert_eq!(out[0].page_num, Some(1));
    assert_eq!(out[1].page_num, Some(2));
}

#[test]
fn merge_sentence_split_across_page_break() {
    let out = ingest::build_paragraphs(&[page(1, "これは途中で"), page(2, "切れた文。")], 0.6, &e());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "これは途中で切れた文。");
    // merged paragraph records its STARTING page
    assert_eq!(out[0].page_num, Some(1));
}

#[test]
fn merge_can_span_three_pages() {
    let out = ingest::build_paragraphs(&[page(1, "あ"), page(2, "い"), page(3, "う。")], 0.6, &e());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "あいう。");
    assert_eq!(out[0].page_num, Some(1));
}

#[test]
fn no_merge_across_a_missing_or_failed_page() {
    // page 2 absent (e.g. it failed OCR) → pages 1 and 3 are not adjacent.
    let out = ingest::build_paragraphs(&[page(1, "未完の文"), page(3, "別の文。")], 0.6, &e());
    assert_eq!(out.len(), 2);
}

#[test]
fn only_the_first_block_of_the_next_page_merges() {
    let out = ingest::build_paragraphs(&[page(1, "続く文"), page(2, "の残り。\n\n新しい段。")], 0.6, &e());
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].text, "続く文の残り。");
    assert_eq!(out[0].page_num, Some(1));
    assert_eq!(out[1].text, "新しい段。");
    assert_eq!(out[1].page_num, Some(2));
}

#[test]
fn blank_pages_skipped_low_score_and_warnings_flagged() {
    let mut low = page(1, "低品質の頁。");
    low.score = Some(0.3);
    let mut blank = page(2, "");
    blank.blank = true;
    let mut warned = page(3, "警告あり。");
    warned.quality_warning = true;
    let out = ingest::build_paragraphs(&[low, blank, warned], 0.6, &e());
    assert_eq!(out.len(), 2, "blank page contributes nothing");
    assert_eq!(out[0].ocr_status, OcrParagraphStatus::LowScore); // score < threshold
    assert_eq!(out[1].ocr_status, OcrParagraphStatus::LowScore); // quality_warning
}

#[test]
fn recovered_pages_marked() {
    let mut p = page(5, "抢救回来的页。");
    p.recovered = true;
    let out = ingest::build_paragraphs(&[p], 0.6, &e());
    assert_eq!(out[0].ocr_status, OcrParagraphStatus::PageFailedRecovered);
    assert_eq!(out[0].page_num, Some(5));
}

// ----- ocr/batch ----------------------------------------------------------------

use felin_core::ocr::batch::{ingest_batch_txts, parse_batch_line, strip_batch_score_marker, BatchArgs, BatchEvent};

#[test]
fn parses_image_success_and_failure_lines() {
    assert_eq!(
        parse_batch_line("✓ 002.png -> /tmp/out/002.txt"),
        Some(BatchEvent::Done { label: "002.png".into(), out: "/tmp/out/002.txt".into() })
    );
    assert_eq!(
        parse_batch_line("✗ 155a.jpg: recognition failed: provider down"),
        Some(BatchEvent::Failed { label: "155a.jpg".into(), error: "recognition failed: provider down".into() })
    );
    assert_eq!(parse_batch_line("Completed: 2 success, 1 failed, 3 total"), None);
    assert_eq!(parse_batch_line("No files found"), None);
    assert_eq!(parse_batch_line(""), None);
}

#[test]
fn parses_pdf_page_lines_on_either_glyph_stream() {
    // PDF success is `✓ name page N -> path`, failure `✗ name page N: err`.
    assert_eq!(
        parse_batch_line("✓ book.pdf page 3 -> /tmp/out/book_003.txt"),
        Some(BatchEvent::Done { label: "book.pdf page 3".into(), out: "/tmp/out/book_003.txt".into() })
    );
    assert_eq!(
        parse_batch_line("✗ book.pdf page 3: timeout"),
        Some(BatchEvent::Failed { label: "book.pdf page 3".into(), error: "timeout".into() })
    );
}

#[test]
fn strips_evaluator_score_marker() {
    assert_eq!(strip_batch_score_marker("本文。\n\n[Score: 0.92]"), "本文。");
    // no marker → unchanged
    assert_eq!(strip_batch_score_marker("本文。"), "本文。");
    // interior [Score:...] line is not stripped
    assert_eq!(strip_batch_score_marker("本文。[Score: 0.5]\n次段。"), "本文。[Score: 0.5]\n次段。");
}

#[test]
fn argv_is_deterministic() {
    let args = BatchArgs {
        sidecar: PathBuf::from("/bin/ocr-cli"),
        input_dir: PathBuf::from("/tmp/inputs"),
        config: Some(PathBuf::from("/tmp/config.yaml")),
        out_dir: PathBuf::from("/tmp/out"),
        workers: Some(4),
        recursive: false,
        skip_existing: false,
        save_json: true,
        envs: vec![],
    };
    let a: Vec<String> = args.to_argv().into_iter().map(|s| s.into_string().unwrap()).collect();
    assert_eq!(
        a,
        vec!["batch", "/tmp/inputs", "-c", "/tmp/config.yaml", "-o", "/tmp/out", "-w", "4", "--save-json"]
    );
}

#[test]
fn ingest_treats_each_txt_as_a_page_and_reads_json_metadata() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    // Two selected images → two txt outputs (+ one json with evaluator data).
    let files = vec![dir.path().join("001.png"), dir.path().join("002.png")];
    std::fs::write(out.join("001.txt"), "第一页の文。\n\n[Score: 0.91]").unwrap();
    std::fs::write(
        out.join("001.json"),
        r#"{"text":"第一页の文。","quality_warning":false,"best_score":0.91,"fallback":false,"evaluation":{"score":0.91,"reason":"ok","pass":true}}"#,
    )
    .unwrap();
    std::fs::write(out.join("002.txt"), "第二页の文。").unwrap();

    let paras = ingest_batch_txts(&files, &out, 0.6, &['。', '！', '？']).unwrap();
    assert_eq!(paras.len(), 2);
    assert_eq!(paras[0].text, "第一页の文。");
    assert_eq!(paras[0].page_num, Some(1));
    assert_eq!(paras[0].page_score, Some(0.91));
    assert!(paras[0].source_file.ends_with("001.png"));
    assert_eq!(paras[1].page_num, Some(2));
    assert!(paras[1].source_file.ends_with("002.png"));
}

#[test]
fn ingest_skips_txts_the_sidecar_failed_to_write() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let files = vec![dir.path().join("001.png"), dir.path().join("002.png")];
    // 002.txt never written (failed image).
    std::fs::write(out.join("001.txt"), "第一页の文。").unwrap();
    let paras = ingest_batch_txts(&files, &out, 0.6, &['。']).unwrap();
    assert_eq!(paras.len(), 1);
    assert!(paras[0].source_file.ends_with("001.png"));
}

// ----- ocr/config (in-place config.yaml editing) -------------------------------

/// A realistic ocr-router config.yaml exercising every section, an `${ENV}`
/// placeholder, and a block-scalar prompt.
const SAMPLE: &str = r#"server:
  addr: "127.0.0.1:9088"
storage:
  format: json
pdf:
  enabled: true
  dpi: 200
  format: png
  max_pages: 100
providers:
  nvidia:
    type: nvidia
    enabled: true
    api_key: "${NVIDIA_API_KEY}"
    endpoint: "https://ai.api.nvidia.com/v1"
  llm_vision:
    type: llm_vision
    enabled: true
    api_key: "${STEP_API_KEY}"
    endpoint: "https://api.stepfun.com/step_plan/v1/chat/completions"
    model: "step-3.7-flash"
    prompt: "请识别图中文字"
    max_tokens: 4000
    max_b64_len: 180000
  browser_sse:
    type: browser_sse
    enabled: true
    base_url: "http://localhost:9222"
evaluator:
  enabled: true
  endpoint: "https://api.stepfun.com/step_plan/v1/chat/completions"
  api_key: "${STEP_API_KEY}"
  model: "step-3.7-flash"
  threshold: 0.7
  max_retries: 2
  retry_delay: "1s"
  timeout: 60s
  max_tokens: 1024
  reasoning_effort: "high"
  prompt: |
    你是质量评估器。根据原文与译文给出打分。
fallback:
  strategy: sequential
  max_retries: 3
  retry_delay: "2s"
  providers:
    - name: nvidia
      priority: 3
      enabled: true
    - name: llm_vision
      priority: 2
      enabled: true
    - name: browser_sse
      priority: 1
      enabled: true
task:
  workers: 5
logging:
  level: info
"#;

fn write_sample(dir: &Path) -> PathBuf {
    let p = dir.join("config.yaml");
    std::fs::write(&p, SAMPLE).unwrap();
    p
}

/// Parse a written file back into a Value tree for round-trip assertions.
fn parsed(p: &Path) -> serde_yml::Mapping {
    let text = std::fs::read_to_string(p).unwrap();
    let v: serde_yml::Value = serde_yml::from_str(&text).expect("written config re-parses");
    v.as_mapping().unwrap().clone()
}

#[test]
fn read_extracts_providers_order_and_evaluator() {
    let dir = tempdir().unwrap();
    let p = write_sample(dir.path());
    let cfg = config::read_config_file(&p).unwrap();

    assert_eq!(cfg.providers.len(), 3);
    assert_eq!(cfg.providers[0].name, "nvidia");
    assert!(cfg.providers[0].enabled);
    assert_eq!(cfg.providers[0].endpoint, "https://ai.api.nvidia.com/v1");
    assert_eq!(cfg.providers[0].api_key, "${NVIDIA_API_KEY}");
    assert!(cfg.providers[0].model.is_empty());

    assert_eq!(cfg.providers[1].name, "llm_vision");
    assert_eq!(cfg.providers[1].endpoint, "https://api.stepfun.com/step_plan/v1/chat/completions");
    assert_eq!(cfg.providers[1].model, "step-3.7-flash");
    assert_eq!(cfg.providers[1].api_key, "${STEP_API_KEY}");

    // browser_sse maps endpoint ← base_url, and has no model/api_key.
    assert_eq!(cfg.providers[2].name, "browser_sse");
    assert_eq!(cfg.providers[2].endpoint, "http://localhost:9222");
    assert!(cfg.providers[2].model.is_empty());
    assert!(cfg.providers[2].api_key.is_empty());

    // Call order = priority ascending: browser_sse(1), llm_vision(2), nvidia(3).
    assert_eq!(cfg.order, vec!["browser_sse", "llm_vision", "nvidia"]);

    assert!(cfg.evaluator.enabled);
    assert_eq!(cfg.evaluator.endpoint, "https://api.stepfun.com/step_plan/v1/chat/completions");
    assert_eq!(cfg.evaluator.model, "step-3.7-flash");
    assert_eq!(cfg.evaluator.api_key, "${STEP_API_KEY}");
}

#[test]
fn read_defaults_when_providers_missing() {
    let dir = tempdir().unwrap();
    // No providers/evaluator/fallback at all.
    let p = dir.path().join("config.yaml");
    std::fs::write(&p, "server:\n  addr: x\n").unwrap();
    let cfg = config::read_config_file(&p).unwrap();
    assert_eq!(cfg.providers.len(), 3);
    for pr in &cfg.providers {
        assert!(!pr.enabled, "missing provider must default to disabled");
        assert!(pr.endpoint.is_empty());
    }
    assert!(cfg.order.is_empty(), "no fallback list, no enabled provider → empty order");
    assert!(!cfg.evaluator.enabled);
}

#[test]
fn apply_updates_managed_keys_and_keeps_the_rest() {
    let dir = tempdir().unwrap();
    let p = write_sample(dir.path());

    let mut cfg = config::read_config_file(&p).unwrap();
    cfg.providers[1].endpoint = "https://new-host.example/v1".into();
    cfg.providers[1].model = "step-4".into();
    cfg.providers[1].api_key = "${NEW_KEY}".into();
    cfg.providers[0].enabled = false;
    // Flip the call order: nvidia first, browser_sse last.
    cfg.order = vec!["nvidia".into(), "llm_vision".into(), "browser_sse".into()];
    cfg.evaluator.enabled = false;

    config::apply_and_write(&p, &cfg).unwrap();

    let root = parsed(&p);
    // Managed keys updated.
    let llm = root.get("providers").unwrap().as_mapping().unwrap()
        .get("llm_vision").unwrap().as_mapping().unwrap();
    assert_eq!(llm.get("endpoint").unwrap().as_str().unwrap(), "https://new-host.example/v1");
    assert_eq!(llm.get("model").unwrap().as_str().unwrap(), "step-4");
    assert_eq!(llm.get("api_key").unwrap().as_str().unwrap(), "${NEW_KEY}");
    let nv = root.get("providers").unwrap().as_mapping().unwrap()
        .get("nvidia").unwrap().as_mapping().unwrap();
    assert_eq!(nv.get("enabled").unwrap().as_bool(), Some(false));
    // browser_sse still written via base_url.
    let bs = root.get("providers").unwrap().as_mapping().unwrap()
        .get("browser_sse").unwrap().as_mapping().unwrap();
    assert_eq!(bs.get("base_url").unwrap().as_str().unwrap(), "http://localhost:9222");
    // Evaluator disabled.
    assert_eq!(root.get("evaluator").unwrap().as_mapping().unwrap()
        .get("enabled").unwrap().as_bool(), Some(false));

    // Call order re-prioritized.
    let fb = root.get("fallback").unwrap().as_mapping().unwrap()
        .get("providers").unwrap().as_sequence().unwrap();
    assert_eq!(fb[0].as_mapping().unwrap().get("name").unwrap().as_str().unwrap(), "nvidia");
    assert_eq!(fb[0].as_mapping().unwrap().get("priority").unwrap().as_i64(), Some(1));
    assert_eq!(fb[2].as_mapping().unwrap().get("name").unwrap().as_str().unwrap(), "browser_sse");
    assert_eq!(fb[2].as_mapping().unwrap().get("priority").unwrap().as_i64(), Some(3));

    // Unmanaged sections + ${ENV} placeholders + block-scalar prompt survive.
    assert_eq!(root.get("server").unwrap().as_mapping().unwrap()
        .get("addr").unwrap().as_str().unwrap(), "127.0.0.1:9088");
    assert_eq!(root.get("pdf").unwrap().as_mapping().unwrap()
        .get("dpi").unwrap().as_i64(), Some(200));
    assert_eq!(root.get("pdf").unwrap().as_mapping().unwrap()
        .get("format").unwrap().as_str().unwrap(), "png");
    assert_eq!(nv.get("api_key").unwrap().as_str().unwrap(), "${NVIDIA_API_KEY}");
    let prompt = root.get("evaluator").unwrap().as_mapping().unwrap()
        .get("prompt").unwrap().as_str().unwrap();
    assert!(prompt.contains("质量评估器"), "block scalar survives: {prompt}");
    assert_eq!(root.get("logging").unwrap().as_mapping().unwrap()
        .get("level").unwrap().as_str().unwrap(), "info");
    assert_eq!(root.get("task").unwrap().as_mapping().unwrap()
        .get("workers").unwrap().as_i64(), Some(5));

    // A subsequent read sees the new state.
    let reread = config::read_config_file(&p).unwrap();
    assert_eq!(reread.order, vec!["nvidia", "llm_vision", "browser_sse"]);
    assert!(!reread.evaluator.enabled);
}

#[test]
fn apply_keeps_disabled_provider_in_order_list() {
    let dir = tempdir().unwrap();
    let p = write_sample(dir.path());
    let mut cfg = config::read_config_file(&p).unwrap();
    cfg.providers[2].enabled = false; // browser_sse off
    config::apply_and_write(&p, &cfg).unwrap();
    let root = parsed(&p);
    let bs = root.get("providers").unwrap().as_mapping().unwrap()
        .get("browser_sse").unwrap().as_mapping().unwrap();
    assert_eq!(bs.get("enabled").unwrap().as_bool(), Some(false));
    let fb = root.get("fallback").unwrap().as_mapping().unwrap()
        .get("providers").unwrap().as_sequence().unwrap();
    // Still listed in the fallback, but flagged disabled.
    assert!(fb.iter().any(|v| v.as_mapping().unwrap().get("name").unwrap().as_str() == Some("browser_sse")));
    assert!(fb.iter().all(|v| v.as_mapping().unwrap().get("enabled").unwrap().as_bool() != Some(true)
        || v.as_mapping().unwrap().get("name").unwrap().as_str() != Some("browser_sse")));
}

#[test]
fn invalid_yaml_is_a_clear_error() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config.yaml");
    std::fs::write(&p, "providers: [unclosed\n").unwrap();
    let err = config::read_config_file(&p).unwrap_err();
    assert!(matches!(err, felin_core::Error::OcrConfig { .. }), "got {err:?}");
}

#[test]
fn non_mapping_root_is_a_clear_error() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config.yaml");
    std::fs::write(&p, "- just\n- a\n- list\n").unwrap();
    assert!(matches!(config::read_config_file(&p).unwrap_err(), felin_core::Error::OcrConfig { .. }));
}

#[test]
fn missing_file_errors() {
    let dir = tempdir().unwrap();
    assert!(matches!(
        config::read_config_file(&dir.path().join("nope.yaml")).unwrap_err(),
        felin_core::Error::OcrConfig { .. }
    ));
}

#[cfg(unix)]
#[test]
fn write_preserves_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let p = write_sample(dir.path());
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();

    let cfg = config::read_config_file(&p).unwrap();
    config::apply_and_write(&p, &cfg).unwrap();

    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "API-key config keeps its restrictive permissions");
}

/// The integration module also type-checks the serde round-trip surface used by
/// the Tauri IPC commands (`OcrConfig` and friends).
#[test]
fn ocr_config_serde_roundtrip() {
    let cfg = OcrConfig {
        providers: vec![
            config::OcrProviderConfig {
                name: "nvidia".into(),
                enabled: true,
                endpoint: "https://ai.api.nvidia.com/v1".into(),
                model: String::new(),
                api_key: "${NVIDIA_API_KEY}".into(),
            },
        ],
        order: vec!["nvidia".into()],
        evaluator: config::OcrEvaluatorConfig {
            enabled: false,
            endpoint: String::new(),
            model: String::new(),
            api_key: String::new(),
        },
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: OcrConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, cfg);
}

// ----- ocr/sidecar (fatal-startup stderr surfacing) ---------------------------

/// A fake sidecar that writes a diagnostic to stderr and exits 20. The
/// contract's generic "fatal startup error" message hides the real cause
/// (e.g. ocr-cli's "failed to load config: ..."); the captured stderr tail
/// must surface in the OcrFatal message.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_startup_surfaces_sidecar_stderr() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::sync::watch;

    let dir = tempdir().unwrap();
    let script = dir.path().join("fake-sidecar.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\necho 'failed to load config: open config.yaml: no such file or directory' >&2\nexit 20\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let args = felin_core::ocr::ExtractArgs {
        sidecar: script,
        input: dir.path().join("in.png"),
        config: None,
        out_dir: dir.path().join("out"),
        manifest: dir.path().join("out/m.manifest.json"),
        pages: None,
        page_workers: None,
        skip_existing: false,
        extra: vec![],
        envs: vec![],
    };
    let (_tx, rx) = watch::channel(false);
    let err = felin_core::ocr::sidecar::run_extract(&args, |_| {}, rx).await.unwrap_err();
    match err {
        felin_core::Error::OcrFatal { exit_code, message } => {
            assert_eq!(exit_code, 20);
            assert!(
                message.contains("failed to load config"),
                "message should carry the sidecar's own stderr, got: {message}"
            );
        }
        other => panic!("expected OcrFatal, got {other:?}"),
    }
}
