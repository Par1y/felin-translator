//! Read/write the ocr-router `config.yaml` **in place** — the file referenced by
//! felin.toml `[sidecar] config` / `FELIN_SIDECAR_CONFIG`. There is no derived
//! config: the app is a GUI editor for that file.
//!
//! The managed surface is deliberately narrow (user directive): each provider's
//! `enabled` / `endpoint` / `model` / `api_key`, the provider **call order**
//! (`fallback.providers` priority), and the evaluator's
//! `enabled` / `endpoint` / `model` / `api_key`. Everything else in the file
//! (`server`/`storage`/`pdf`/`logging`/`task`, prompts, `max_tokens`, `${ENV}`
//! placeholders) round-trips untouched.
//!
//! Round-tripping through a [`serde_yml::Value`] tree means hand-written
//! comments and layout are dropped on write; callers surface that to the user
//! and never log API keys.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_yml::{Mapping, Value};
use std::path::Path;

/// The three providers ocr-router ships with, in a fixed display order. The UI
/// reorders them via [`OcrConfig::order`], never by reshuffling this list.
pub const PROVIDER_NAMES: &[&str] = &["nvidia", "llm_vision", "browser_sse"];

/// One OCR provider's editable basics. `endpoint` maps to the provider's
/// `endpoint` key, except for `browser_sse` which uses `base_url`; providers
/// without a `model`/`api_key` carry an empty string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrProviderConfig {
    pub name: String,
    pub enabled: bool,
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
}

/// The evaluator (quality-scoring stage) editable basics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrEvaluatorConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
}

/// The app-editable slice of `config.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrConfig {
    /// Always [`PROVIDER_NAMES`] order; the UI reorders via [`Self::order`].
    pub providers: Vec<OcrProviderConfig>,
    /// Provider names in call order, highest priority first (from
    /// `fallback.providers` sorted by ascending `priority`).
    pub order: Vec<String>,
    pub evaluator: OcrEvaluatorConfig,
}

/// Parse `config.yaml` into the managed slice. Missing providers/keys become
/// safe defaults (disabled, empty), so a partial file still reads.
pub fn read_config_file(path: &Path) -> Result<OcrConfig> {
    let text = read_file(path)?;
    let root = parse_root(&text, path)?;
    let root_m = root
        .as_mapping()
        .ok_or_else(|| Error::ocr_config("config.yaml must be a top-level YAML mapping"))?;
    let providers = read_providers(root_m);
    let order = call_order(root_m, &providers);
    let evaluator = read_evaluator(root_m);
    Ok(OcrConfig { providers, order, evaluator })
}

/// Apply `cfg` to `config.yaml` and write it back to the **same file**
/// (atomic: temp + rename, original permissions preserved). Managed keys are
/// overwritten; every unmanaged section and `${ENV}` placeholder survives.
pub fn apply_and_write(path: &Path, cfg: &OcrConfig) -> Result<()> {
    let text = read_file(path)?;
    let mut root = parse_root(&text, path)?;
    let root_m = root
        .as_mapping_mut()
        .ok_or_else(|| Error::ocr_config("config.yaml must be a top-level YAML mapping"))?;

    // Providers: only touch the managed keys, keep prompts/max_tokens/headers.
    for p in &cfg.providers {
        let providers = root_m
            .entry("providers")
            .or_insert(Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| Error::ocr_config("`providers` must be a mapping"))?;
        let entry = providers
            .entry(p.name.clone())
            .or_insert(Value::Mapping(Mapping::new()));
        let m = entry
            .as_mapping_mut()
            .ok_or_else(|| Error::ocr_config(format!("provider `{}` must be a mapping", p.name)))?;
        m.insert("enabled", Value::Bool(p.enabled));
        m.insert("api_key", Value::String(p.api_key.clone()));
        if p.name == "browser_sse" {
            m.insert("base_url", Value::String(p.endpoint.clone()));
        } else {
            m.insert("endpoint", Value::String(p.endpoint.clone()));
            if !p.model.is_empty() {
                m.insert("model", Value::String(p.model.clone()));
            }
        }
    }

    // Call order → `fallback.providers` with priority = position (1 = first).
    let fallback = root_m
        .entry("fallback")
        .or_insert(Value::Mapping(Mapping::new()));
    let fm = fallback
        .as_mapping_mut()
        .ok_or_else(|| Error::ocr_config("`fallback` must be a mapping"))?;
    let list: Vec<Value> = cfg
        .order
        .iter()
        .filter_map(|name| {
            cfg.providers.iter().find(|p| &p.name == name).map(|p| (name, p))
        })
        .enumerate()
        .map(|(idx, (name, p))| {
            let mut item = Mapping::new();
            item.insert("name", Value::String(name.clone()));
            item.insert("priority", Value::from((idx as i64) + 1));
            item.insert("enabled", Value::Bool(p.enabled));
            Value::Mapping(item)
        })
        .collect();
    fm.insert("providers", Value::from(list));

    // Evaluator.
    let ev = root_m
        .entry("evaluator")
        .or_insert(Value::Mapping(Mapping::new()));
    let em = ev
        .as_mapping_mut()
        .ok_or_else(|| Error::ocr_config("`evaluator` must be a mapping"))?;
    em.insert("enabled", Value::Bool(cfg.evaluator.enabled));
    em.insert("endpoint", Value::String(cfg.evaluator.endpoint.clone()));
    em.insert("model", Value::String(cfg.evaluator.model.clone()));
    em.insert("api_key", Value::String(cfg.evaluator.api_key.clone()));

    let out = serde_yml::to_string(&root)
        .map_err(|e| Error::ocr_config(format!("failed to serialize config.yaml: {e}")))?;
    atomic_write(path, &out)?;
    Ok(())
}

/// Read the file, mapping I/O errors onto `OcrConfig`.
fn read_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| Error::ocr_config(format!("cannot read {}: {e}", path.display())))
}

/// Parse the file text into an owned YAML value tree.
fn parse_root(text: &str, path: &Path) -> Result<Value> {
    let root: Value = serde_yml::from_str(text)
        .map_err(|e| Error::ocr_config(format!("invalid YAML in {}: {e}", path.display())))?;
    if !root.is_mapping() {
        return Err(Error::ocr_config("config.yaml must be a top-level YAML mapping"));
    }
    Ok(root)
}

/// Extract the three known providers in [`PROVIDER_NAMES`] order.
fn read_providers(root: &Mapping) -> Vec<OcrProviderConfig> {
    let prov_map = root.get("providers").and_then(|v| v.as_mapping());
    PROVIDER_NAMES
        .iter()
        .map(|name| {
            let entry = prov_map.and_then(|m| m.get(*name)).and_then(|v| v.as_mapping());
            let (endpoint, model, api_key) = if *name == "browser_sse" {
                (
                    str_field(entry, "base_url").unwrap_or_default(),
                    String::new(),
                    String::new(),
                )
            } else {
                (
                    str_field(entry, "endpoint").unwrap_or_default(),
                    str_field(entry, "model").unwrap_or_default(),
                    str_field(entry, "api_key").unwrap_or_default(),
                )
            };
            OcrProviderConfig {
                name: (*name).to_string(),
                enabled: bool_field(entry, "enabled").unwrap_or(false),
                endpoint,
                model,
                api_key,
            }
        })
        .collect()
}

/// Provider call order: `fallback.providers` sorted by ascending `priority`
/// (stable, so equal priorities keep file order). Providers missing from the
/// list but enabled are appended, so the UI can still reorder them; a missing
/// `fallback.providers` degrades to the declared provider order.
fn call_order(root: &Mapping, providers: &[OcrProviderConfig]) -> Vec<String> {
    let mut names: Vec<String> = root
        .get("fallback")
        .and_then(|v| v.as_mapping())
        .and_then(|m| m.get("providers"))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            let mut items: Vec<(String, i64)> = seq
                .iter()
                .filter_map(|v| {
                    let m = v.as_mapping()?;
                    let name = m.get("name")?.as_str()?.to_string();
                    let prio = m.get("priority").and_then(|p| p.as_i64()).unwrap_or(i64::MAX);
                    Some((name, prio))
                })
                .collect();
            items.sort_by_key(|(_, prio)| *prio);
            // Dedup by first occurrence (stable sort keeps the first).
            let mut seen = std::collections::HashSet::new();
            items.retain(|(n, _)| seen.insert(n.clone()));
            items.into_iter().map(|(n, _)| n).collect()
        })
        .unwrap_or_default();

    for p in providers {
        if p.enabled && !names.contains(&p.name) {
            names.push(p.name.clone());
        }
    }
    names
}

/// The evaluator's editable basics.
fn read_evaluator(root: &Mapping) -> OcrEvaluatorConfig {
    let e = root.get("evaluator").and_then(|v| v.as_mapping());
    OcrEvaluatorConfig {
        enabled: bool_field(e, "enabled").unwrap_or(false),
        endpoint: str_field(e, "endpoint").unwrap_or_default(),
        model: str_field(e, "model").unwrap_or_default(),
        api_key: str_field(e, "api_key").unwrap_or_default(),
    }
}

fn str_field(m: Option<&Mapping>, key: &str) -> Option<String> {
    m.and_then(|m| m.get(key)).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn bool_field(m: Option<&Mapping>, key: &str) -> Option<bool> {
    m.and_then(|m| m.get(key)).and_then(|v| v.as_bool())
}

/// Write atomically (temp file in the same dir + rename) so a crash mid-write
/// can never corrupt the config, and best-effort preserve the original file
/// permissions (the file holds API keys).
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::ocr_config(format!("no parent directory for {}", path.display())))?;
    let perms = std::fs::metadata(path).ok().map(|m| m.permissions());
    let tmp = dir.join(format!(
        ".{}.felin-tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("config"),
        std::process::id()
    ));
    std::fs::write(&tmp, content)
        .map_err(|e| Error::ocr_config(format!("cannot write {}: {e}", tmp.display())))?;
    if let Some(p) = perms {
        let _ = std::fs::set_permissions(&tmp, p);
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| Error::ocr_config(format!("cannot replace {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

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

    fn write_sample(dir: &Path) -> std::path::PathBuf {
        let p = dir.join("config.yaml");
        std::fs::write(&p, SAMPLE).unwrap();
        p
    }

    /// Parse a written file back into a Value tree for round-trip assertions.
    fn parsed(p: &Path) -> Mapping {
        let text = std::fs::read_to_string(p).unwrap();
        let v: Value = serde_yml::from_str(&text).expect("written config re-parses");
        v.as_mapping().unwrap().clone()
    }

    #[test]
    fn read_extracts_providers_order_and_evaluator() {
        let dir = tempdir().unwrap();
        let p = write_sample(dir.path());
        let cfg = read_config_file(&p).unwrap();

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
        let cfg = read_config_file(&p).unwrap();
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

        let mut cfg = read_config_file(&p).unwrap();
        cfg.providers[1].endpoint = "https://new-host.example/v1".into();
        cfg.providers[1].model = "step-4".into();
        cfg.providers[1].api_key = "${NEW_KEY}".into();
        cfg.providers[0].enabled = false;
        // Flip the call order: nvidia first, browser_sse last.
        cfg.order = vec!["nvidia".into(), "llm_vision".into(), "browser_sse".into()];
        cfg.evaluator.enabled = false;

        apply_and_write(&p, &cfg).unwrap();

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
        let reread = read_config_file(&p).unwrap();
        assert_eq!(reread.order, vec!["nvidia", "llm_vision", "browser_sse"]);
        assert!(!reread.evaluator.enabled);
    }

    #[test]
    fn apply_keeps_disabled_provider_in_order_list() {
        let dir = tempdir().unwrap();
        let p = write_sample(dir.path());
        let mut cfg = read_config_file(&p).unwrap();
        cfg.providers[2].enabled = false; // browser_sse off
        apply_and_write(&p, &cfg).unwrap();
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
        let err = read_config_file(&p).unwrap_err();
        assert!(matches!(err, Error::OcrConfig { .. }), "got {err:?}");
    }

    #[test]
    fn non_mapping_root_is_a_clear_error() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.yaml");
        std::fs::write(&p, "- just\n- a\n- list\n").unwrap();
        assert!(matches!(read_config_file(&p).unwrap_err(), Error::OcrConfig { .. }));
    }

    #[test]
    fn missing_file_errors() {
        let dir = tempdir().unwrap();
        assert!(matches!(read_config_file(&dir.path().join("nope.yaml")).unwrap_err(), Error::OcrConfig { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn write_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let p = write_sample(dir.path());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();

        let cfg = read_config_file(&p).unwrap();
        apply_and_write(&p, &cfg).unwrap();

        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "API-key config keeps its restrictive permissions");
    }
}
