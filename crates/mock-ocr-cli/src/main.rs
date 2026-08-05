//! `mock-ocr-cli` — a contract-faithful stand-in for `ocr-cli extract`.
//!
//! Implements enough of the plan's OCR backend contract (§2 per-page JSON +
//! incremental manifest, §3 `--progress json` JSONL, §4 exit codes 0/10/20/130,
//! §5 SIGTERM/SIGINT graceful cancel) to develop and test the translation app's
//! OCR layer before the real `ocr-router` implementation exists. It also ships
//! as the initial sidecar so the app runs end-to-end from day one.
//!
//! Behavior is scenario-driven via environment variables so tests can exercise
//! failures, blank pages, cancellation, and resume:
//!
//! | env | meaning | default |
//! |---|---|---|
//! | `MOCK_OCR_PAGES` | total pages (when `--pages` is absent) | 3 |
//! | `MOCK_OCR_FAIL_PAGES` | comma list of pages to mark `failed` | none |
//! | `MOCK_OCR_BLANK_PAGES` | comma list of pages to mark `blank` | none |
//! | `MOCK_OCR_FATAL` | if set, exit 20 immediately (no output) | unset |
//! | `MOCK_OCR_EVALUATOR` | if set, emit `score_present:true` + a score | unset |
//! | `MOCK_OCR_PAGE_DELAY_MS` | sleep per page (lets tests cancel mid-run) | 0 |
//! | `MOCK_OCR_TEXT` | per-page text; `{page}` is substituted | 「これは…{page}です。」 |

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match Args::parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("mock-ocr-cli: {e}");
            return 20;
        }
    };

    // §4: a fatal startup condition produces no output and exits 20.
    if std::env::var_os("MOCK_OCR_FATAL").is_some() {
        eprintln!("mock-ocr-cli: fatal: simulated startup failure (MOCK_OCR_FATAL)");
        return 20;
    }

    let scenario = Scenario::from_env();
    let cancel = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&cancel));
        let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&cancel));
    }

    if let Err(e) = std::fs::create_dir_all(&args.out) {
        eprintln!("mock-ocr-cli: cannot create out dir {}: {e}", args.out.display());
        return 20;
    }

    let base = args
        .input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page")
        .to_string();

    let pages = match args.pages_list(&scenario) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("mock-ocr-cli: bad --pages: {e}");
            return 20;
        }
    };
    let total = pages.len() as i64;

    if args.progress_json {
        emit(&json_start(&args.input, total));
    }

    let mut manifest_pages: Vec<serde_json::Value> = Vec::new();
    let mut pages_ok = 0i64;
    let mut pages_failed = 0i64;
    let mut done = 0i64;
    let mut cancelled = false;

    for page in pages {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }

        let page_file = format!("{base}-{page:04}.json");
        let page_path = args.out.join(&page_file);

        // §7 resume: with --skip-existing, an already-OK page is not reprocessed.
        // Reuse its stored score so the manifest matches the on-disk page JSON.
        if args.skip_existing {
            if let Some(existing) = read_existing_ok(&page_path) {
                let score = existing.get("score").and_then(|v| v.as_f64());
                pages_ok += 1;
                done += 1;
                manifest_pages.push(manifest_entry(page, "ok", score, &page_file, None));
                if args.progress_json {
                    emit(&json_page(page, "ok", score, None, done, total));
                }
                continue;
            }
        }

        let (status, blank, error) = scenario.outcome_for(page);
        let score = if status == "ok" { scenario.score_for(page) } else { None };
        let text = if status == "ok" && !blank { scenario.text_for(page, &base) } else { String::new() };

        let page_json = build_page_json(page, status, &text, score, scenario.evaluator, blank, error, &args.input, &base);
        if let Err(e) = write_atomic(&page_path, page_json.to_string().as_bytes()) {
            eprintln!("mock-ocr-cli: cannot write {}: {e}", page_path.display());
            return 20;
        }

        match status {
            "ok" => pages_ok += 1,
            _ => pages_failed += 1,
        }
        done += 1;
        manifest_pages.push(manifest_entry(page, status, score, &page_file, error));

        // §2: manifest is rewritten atomically after every page (resumable).
        let manifest = build_manifest(&args, &base, total, pages_ok, pages_failed, &manifest_pages, scenario.evaluator, None);
        let _ = write_atomic(&args.manifest, manifest.to_string().as_bytes());

        if args.progress_json {
            emit(&json_page(page, status, score, error, done, total));
        }

        if scenario.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(scenario.delay_ms));
        }
    }

    // Final manifest: mark "cancelled" if we were interrupted (§5).
    let final_status = if cancelled { Some("cancelled") } else { None };
    let manifest = build_manifest(&args, &base, total, pages_ok, pages_failed, &manifest_pages, scenario.evaluator, final_status);
    let _ = write_atomic(&args.manifest, manifest.to_string().as_bytes());

    if args.progress_json {
        emit(&json_done(pages_ok, pages_failed, &args.manifest));
    }

    if cancelled {
        130
    } else if pages_failed > 0 {
        10
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

struct Args {
    input: PathBuf,
    out: PathBuf,
    manifest: PathBuf,
    pages: Option<String>,
    skip_existing: bool,
    progress_json: bool,
}

impl Args {
    fn parse(argv: &[String]) -> Result<Self, String> {
        let mut it = argv.iter();
        match it.next().map(String::as_str) {
            Some("extract") => {}
            other => return Err(format!("expected subcommand `extract`, got {other:?}")),
        }

        let mut input: Option<PathBuf> = None;
        let mut out: Option<PathBuf> = None;
        let mut manifest: Option<PathBuf> = None;
        let mut pages: Option<String> = None;
        let mut skip_existing = false;
        let mut progress_json = false;

        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--out" => out = Some(it.next().ok_or("--out needs a value")?.into()),
                "--manifest" => manifest = Some(it.next().ok_or("--manifest needs a value")?.into()),
                "--pages" => pages = Some(it.next().ok_or("--pages needs a value")?.clone()),
                "-c" | "--config" => {
                    let _ = it.next(); // accepted and ignored by the mock
                }
                "--page-workers" | "--window" => {
                    let _ = it.next(); // accepted and ignored
                }
                "--skip-existing" => skip_existing = true,
                "--progress" => {
                    progress_json = it.next().map(String::as_str) == Some("json");
                }
                "--overlap-render" => {}
                s if s.starts_with('-') => { /* tolerate unknown flags */ }
                _ => {
                    if input.is_none() {
                        input = Some(arg.into());
                    }
                }
            }
        }

        Ok(Self {
            input: input.ok_or("missing input <path>")?,
            out: out.ok_or("missing --out")?,
            manifest: manifest.ok_or("missing --manifest")?,
            pages,
            skip_existing,
            progress_json,
        })
    }

    fn pages_list(&self, scenario: &Scenario) -> Result<Vec<i64>, String> {
        match &self.pages {
            Some(spec) => parse_pages(spec),
            None => Ok((1..=scenario.total_pages).collect()),
        }
    }
}

/// Parse a page spec like `"3,7,10-12"` into a sorted, de-duplicated list.
fn parse_pages(spec: &str) -> Result<Vec<i64>, String> {
    // Cap total expansion so a typo like `1-99999999999` can't OOM/hang us.
    const MAX_PAGES: usize = 200_000;
    let mut set = std::collections::BTreeSet::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let a: i64 = a.trim().parse().map_err(|_| format!("bad range start {a:?}"))?;
            let b: i64 = b.trim().parse().map_err(|_| format!("bad range end {b:?}"))?;
            if a > b {
                return Err(format!("range {a}-{b} is inverted"));
            }
            let span = b.checked_sub(a).and_then(|d| d.checked_add(1)).unwrap_or(i64::MAX);
            if span as u64 > MAX_PAGES as u64 || set.len().saturating_add(span as usize) > MAX_PAGES {
                return Err(format!("page range {a}-{b} is too large (cap {MAX_PAGES})"));
            }
            for p in a..=b {
                set.insert(p);
            }
        } else {
            set.insert(part.parse().map_err(|_| format!("bad page {part:?}"))?);
        }
        if set.len() > MAX_PAGES {
            return Err(format!("too many pages requested (cap {MAX_PAGES})"));
        }
    }
    Ok(set.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Scenario (env-driven)
// ---------------------------------------------------------------------------

struct Scenario {
    total_pages: i64,
    fail: Vec<i64>,
    blank: Vec<i64>,
    evaluator: bool,
    delay_ms: u64,
    text_template: Option<String>,
}

impl Scenario {
    fn from_env() -> Self {
        Self {
            total_pages: env_i64("MOCK_OCR_PAGES").unwrap_or(3),
            fail: env_list("MOCK_OCR_FAIL_PAGES"),
            blank: env_list("MOCK_OCR_BLANK_PAGES"),
            evaluator: std::env::var_os("MOCK_OCR_EVALUATOR").is_some(),
            delay_ms: env_i64("MOCK_OCR_PAGE_DELAY_MS").unwrap_or(0).max(0) as u64,
            text_template: std::env::var("MOCK_OCR_TEXT").ok(),
        }
    }

    /// Returns (status, blank, error) for a page.
    fn outcome_for(&self, page: i64) -> (&'static str, bool, Option<&'static str>) {
        if self.fail.contains(&page) {
            ("failed", false, Some("simulated provider timeout"))
        } else if self.blank.contains(&page) {
            ("ok", true, None)
        } else {
            ("ok", false, None)
        }
    }

    fn score_for(&self, page: i64) -> Option<f64> {
        if self.evaluator {
            // A deterministic, page-varying score in [0.5, 0.95].
            Some(0.5 + ((page % 10) as f64) * 0.05)
        } else {
            None
        }
    }

    fn text_for(&self, page: i64, base: &str) -> String {
        match &self.text_template {
            Some(t) => t.replace("{page}", &page.to_string()),
            None => format!("これはモックOCRによる「{base}」のページ{page}の本文です。"),
        }
    }
}

// ---------------------------------------------------------------------------
// JSON builders
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn build_page_json(
    page: i64,
    status: &str,
    text: &str,
    score: Option<f64>,
    evaluator: bool,
    blank: bool,
    error: Option<&str>,
    input: &Path,
    base: &str,
) -> serde_json::Value {
    serde_json::json!({
        "page": page,
        "status": status,
        "text": text,
        "score": score,
        "score_present": evaluator && status == "ok",
        "provider": "mock",
        "fallback": false,
        "quality_warning": false,
        "blank": blank,
        "best_score": score,
        "error": error,
        "source_file": input.to_string_lossy(),
        "image": format!("{base}-{page:04}.png"),
    })
}

fn manifest_entry(page: i64, status: &str, score: Option<f64>, file: &str, error: Option<&str>) -> serde_json::Value {
    serde_json::json!({ "page": page, "status": status, "score": score, "file": file, "error": error })
}

#[allow(clippy::too_many_arguments)]
fn build_manifest(
    args: &Args,
    base: &str,
    total: i64,
    pages_ok: i64,
    pages_failed: i64,
    pages: &[serde_json::Value],
    evaluator: bool,
    status: Option<&str>,
) -> serde_json::Value {
    let _ = base;
    let mut m = serde_json::json!({
        "schema_version": 1,
        "source": args.input.to_string_lossy(),
        "type": source_type(&args.input),
        "pages_total": total,
        "pages_attempted": pages.len(),
        "pages_ok": pages_ok,
        "pages_failed": pages_failed,
        "evaluator_enabled": evaluator,
        "provider": "mock",
        "pages": pages,
    });
    if let Some(s) = status {
        m["status"] = serde_json::Value::String(s.to_string());
    }
    m
}

fn source_type(input: &Path) -> &'static str {
    match input.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("pdf") => "pdf",
        Some("png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "bmp") => "image",
        _ if input.is_dir() => "dir",
        _ => "image",
    }
}

fn json_start(input: &Path, total: i64) -> serde_json::Value {
    serde_json::json!({ "event": "start", "source": input.to_string_lossy(), "pages_total": total })
}

fn json_page(page: i64, status: &str, score: Option<f64>, error: Option<&str>, done: i64, total: i64) -> serde_json::Value {
    serde_json::json!({ "event": "page", "page": page, "status": status, "score": score, "error": error, "done": done, "total": total })
}

fn json_done(pages_ok: i64, pages_failed: i64, manifest: &Path) -> serde_json::Value {
    serde_json::json!({ "event": "done", "pages_ok": pages_ok, "pages_failed": pages_failed, "manifest": manifest.to_string_lossy() })
}

// ---------------------------------------------------------------------------
// IO helpers
// ---------------------------------------------------------------------------

/// Print a JSONL event to stdout and flush (so the reader sees it promptly).
fn emit(value: &serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

/// Write `bytes` to `path` atomically (temp file + rename).
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// If `path` is an existing page JSON marked `status:"ok"`, return its parsed
/// contents (so callers can reuse the stored score on the skip fast-path).
fn read_existing_ok(path: &Path) -> Option<serde_json::Value> {
    let data = std::fs::read(path).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&data).ok()?;
    if v.get("status").and_then(|s| s.as_str()) == Some("ok") {
        Some(v)
    } else {
        None
    }
}

fn env_i64(key: &str) -> Option<i64> {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
}

fn env_list(key: &str) -> Vec<i64> {
    std::env::var(key)
        .ok()
        .map(|v| v.split(',').filter_map(|p| p.trim().parse().ok()).collect())
        .unwrap_or_default()
}
