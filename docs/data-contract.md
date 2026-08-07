# Felin Translator — 数据类型与调用约定（Data Contract）

> 本文档汇总项目内部的数据类型、存储布局、Tauri 命令调用约定与「类别/标签」等
> 易混淆字段的统一语义。改动任一约定时，须同步更新本文档。
>
> 最近更新：2026-08-07

## 0. 分层

```
ui/ (React 19 + AntD 6)  ──invoke──▶  src-tauri/commands.rs (Tauri 命令)
                                        │
                                        ▼
                                   felin-core/ (纯领域逻辑, 无 Tauri 依赖)
                                        │
                                        ▼
                               storage/{db,global,project}  +  SQLite
```

- `ui/src/types.ts` 是 Rust 侧命令载荷的**手工镜像**（`GlossaryName`/`GlossaryEntry`/
  `ExtractedName`/`PromptConfig`/`MatchedName`…），字段名、可选性、串行名必须与
  `felin_core::types` / 命令签名逐字段一致。Tauri v2 自动做 camelCase⇄snake_case。
- 跨层的**枚举一律存 TEXT**（`sql_enum!` 宏：`NameStatus`/`TuStatus`/`ExtractedNameStatus`…），
  字符串常量即 wire 表示，勿改。

## 1. 配置（felin.toml，技术参数）

`felin_core::config::TechConfig`（`#[serde(default)]` 逐节默认）。非技术选项进 GUI/项目
设置（project.db `settings`），**不**进此文件。节：

| 节 | 承载 | 备注 |
|---|---|---|
| `[seg]` | 章节正则、句末符、默认块大小、回退标题、标题上限 | |
| `[ocr]` | low_score_threshold、max_page_json_bytes、max_manifest_bytes | |
| `[names]` | fuzzy_max_distance | |
| `[llm]` | timeout/retries/backoff/temperature/max_tokens | endpoint/model/key 在 GUI |
| `[sidecar]` | cancel_grace_secs、poll_ms、bin、config | bin/config 为用户管理路径，找不到即报错 |
| `[pipeline]` | queue_capacity、context_max_chars、guidelines_max_chars | |
| `[db]` | read_pool_size、busy_timeout_ms | |
| `[import]` | max_file_bytes | |
| `[prompt]` | `extract_system`、`extract_tags_system`、`translation_system`、`translation_user` | **prompt 唯一来源**；空字段 = 不发送该消息段 |
| `[debug]` | enabled（默认关） | 开则输出关键操作日志 |

**Prompt 铁律**：运行时无任何 prompt 常量；出厂文本只在首启模板
（`factory_prompt_config()`）。`load_from_disk` 对缺 `[prompt]` 的旧文件**追加**该节，
对 `[prompt]` 中**缺失** `extract_tags_system` 键的旧文件**就地补键**（只补缺失字段，
显式空串视为「关闭自动打标签」，不补）。`set_prompt_section` 只重写 `[prompt]` 节。

## 2. 存储（SQLite）

- **全局大词库** `glossary.db`（`GlobalDb`）：`names`（japanese UNIQUE, chinese,
  english, category, notes, source, status, tags JSON, enabled）+ `name_history`。
- **项目库** `project.db`（`ProjectDb`）：`chapters` / `paragraphs` / `tus` /
  `translations` / `extracted_names` / `glossary_entries` / `project_name_overrides` /
  `exports` / `settings`。
- 迁移：`PROJECT_MIGRATIONS` / `GLOBAL_MIGRATIONS` 有序追加；IMMEDIATE 事务 + 升级前备份；
  只允许**向后**（新版本号）迁移。

### 「类别」与「标签」的约定（重点）

| 字段 | 位置 | 语义 |
|---|---|---|
| `category`（类别） | 全局 `names.category`、项目 `glossary_entries.category` | **单一字符串**；= 该词条所属类别（人名/地名/…）。**= `tags[0]`** |
| `tags`（标签） | JSON 数组列 | 多标签（可含来源 `project:<slug>`、任意 tag）；`category` 是它的首个标签的冗余快照 |

**统一规则**：任何写入词库的路径（专名候选确认、CSV 导入、词条新建/编辑、自动打标签后
确认）都把「第一个标签」作为 category 冗余存一份，标签数组保留全部。手工新建/编辑词条时
**不再单独输入类别**（曾是多出来的「类别」输入框）——只填标签，`tags[0]` 即类别。

## 3. 专名候选（extracted_names）

`ExtractedName { id, japanese, matched_name_id, candidate_chinese, status, tags, notes }`

- `tags`（JSON 数组）: LLM 自动打标签 / 用户编辑 / 批量标记 的类别。
- `japanese` / `candidate_chinese` **都可改**（OCR 可能误识别）：`update_extracted_japanese`
  带同名去重护栏（撞名报错），`update_extracted` 改中文。
- `auto_tag_extracted` 只写**首个**分类（已有标签不覆盖）；`apply_extracted_tags` 批量写同一标签。

## 4. 词库（global ↔ project）

- 全局大词库（`glossary.db names`）是共享池；项目小词库（`glossary_entries`）是自包含快照
  （japanese/chinese/english/category/tags 拷贝于添加时），`name_global_id` 记录来源（跨文件
  引用，非外键）。翻译 prompt **只注入项目小词库 enabled 词条**。
- 确认专名候选：**总是**先 upsert 进全局（带 `project:<slug>` source 标签），再按目标写入
  小词库；小词库 `category = 候选 tags[0]`，全局 `category` **仅在原本为空时**补写
  （不覆盖他项目已确立的类别）。

## 5. Tauri 命令调用约定

命令在 `src-tauri/src/commands.rs`，`ui/src/api.ts` 包装。全部 `Result<T, String>`
（Err 到前端）。当前命令（70）：

- 应用/项目：`app_info`, `create_project`/`rename_project`/`delete_project`/
  `open_project`/`close_project`/`current_project`/`list_projects`
- 读：`list_chapters`/`list_paragraphs`/`list_tus`
- 分段：`segment_project`；导入：`import_txt_file`/`import_ocr`/`cancel_import`/
  `scan_image_dir`/`import_images_batch`；归档：`export_project`/`import_project`
- 模型：`get_llm_config`/`set_llm_config`/`test_llm_connection`
- OCR 配置：`get_ocr_config`/`set_ocr_config`
- 专名：`csv_headers`/`csv_preview`/`import_glossary_csv`/`run_name_extraction`/
  `list_extracted`/`update_extracted`/`update_extracted_japanese`/`update_extracted_tags`/
  `auto_tag_extracted`/`apply_extracted_tags`/`reject_extracted`/`reject_extracted_batch`/
  `confirm_extracted`/`confirm_extracted_batch`
- 词库：`list_glossary`/`set_global_name_tags`/`set_global_name_enabled`/`delete_global_names`/
  `list_glossary_entries`/`add_glossary_entry`/`update_glossary_entry`/`set_entry_enabled`/
  `set_entry_tags`/`delete_glossary_entry`/`delete_glossary_entries`
- 翻译：`start_translation`/`stop_translation`/`translation_status`/`retry_translation`/
  `approve_tu`/`set_tu_instruction`/`retranslate_tu`/`retranslate_tus`/
  `get_translation_settings`/`set_translation_settings`/`get_guidelines`/`set_guidelines`/
  `list_tus_with_translations`/`set_tu_source`/`set_translation_text`/`delete_tus`
- 设置/导出：`get_ocr_settings`/`set_ocr_settings`/`export_translations`/
  `get_prompt_config`/`set_prompt_config`

**异步命令**（async）：`test_llm_connection`/`run_name_extraction`/`auto_tag_extracted`
跑在 Tauri 自己的 async runtime 上；OCR 导入与翻译跑在**独立专用线程 + tokio runtime**，
经事件（`ocr://progress`/`ocr://done`/`ocr://error`、`translation://progress`/
`translation://done`/`translation://error`）回报。见 §6 并发模型。

## 6. 并发模型（LLM / 外部工具统一约束）

- **LLM 并发**：全局仅一个共享 `LlmRateLimiter`（`tokio::sync::Semaphore`，容量 = `[llm]
  concurrency`，felin.toml，默认 2）。**所有** LLM 调用——翻译 worker、专名抽取、自动打标签、
  连接测试——都必须先 `acquire` 该信号量（按 LLM 速率上限排队），杜绝「N 翻译 worker + 抽取 +
  打标签 + 多次连接测试」同时打爆上游。permit **按次网络尝试获取**：请求返回后才释放，退避睡眠
  期间不占 permit，因此某个被上游卡住的调用不会长占一个额度而冻结其余功能。
- **翻译管线**：`RunConfig.workers`（GUI 1–8）是翻译并发，同时是**不越过限流器上限**的约束；
  管线在独立线程 + tokio runtime 上跑，事件经 `translation://` 回报。
- **外部进程（ocr-cli）**：每次导入一个进程组（`group_spawn`），取消 = SIGTERM 全组 + 宽限 +
  强杀；OCR 导入与翻译的专用 runtime 各自独立（互不阻塞 Tauri 主线程）。
- **阻塞 SQLite**：项目库读写在命令内联执行（`with_project`），不驻留 async 执行器——命令
  是非 async 的直接返回，或 async 命令中**不持有 guard 跨 await**（先取数据再 await，再取写）。
- **单飞**：翻译管线单飞（`translation_guard`）；OCR 导入可并发但任务按 task_id 隔离目录。

## 7. 关键字段速查（易混）

| 字段 | 位置 | 说明 |
|---|---|---|
| `TuStatus` | tus.status | pending/queued/translating/translated/reviewing/approved/exported/interrupted/failed_retryable/failed_permanent |
| `TranslationStatus` | translations.status | draft/memory_hit/failed |
| `ExtractedNameStatus` | extracted_names.status | new/matched/confirmed/rejected |
| `matched_names` | `TuWithTranslation`（查询时计算） | 该 TU 原文命中的小词库 enabled 词条，去重按出现序 |
| `source_override` | tus | 用户改过的原文（优先于段落拼接） |
| `final_text` | translations | 人工批准的译文（导出只用它） |
| `enabled` | 词库 | 翻译注入只认 enabled=1 |
