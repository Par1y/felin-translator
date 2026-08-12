# Felin Translator

跨平台（Linux / Windows / macOS）的半自动化 **日文 → 中文** 翻译校对桌面应用。
核心流程：**OCR 提取原文 → LLM 翻译 → 人工校对 → 导出**。

技术栈：Tauri v2 + React 19 + Vite 8 + Ant Design 6；Rust workspace。

## 功能

- **导入**：txt / markdown 直接导入；使用 [ocr-router](https://github.com/Par1y/ocr-router) 对 PDF、图片批量导入。
- **专名抽取与校对**：LLM 抽取候选、自动打标签，加入项目小词库或全局大词库。
- **分段**：按目标块字符数自动分段，防止暴力截断；校对时也可自由再分段。
- **翻译**：词库 + 规则并行翻译。
- **校对**：原文-译文对应，随时可添加要求重译，针对校对工作流优化。
- **导出**：译文汉化 `.txt` + 译文 `.csv`；项目整体归档备份。

## 使用

在 releases 下载对应操作系统解压打开即可。

## 配置

GUI 内配置，或编辑 `felin.toml` 修改进阶参数。

## 开发

### 环境要求

- Rust 稳定版（`dtolnay/rust-toolchain@stable` 对应版本）
- Node.js LTS + pnpm
- Linux 需 Tauri v2 系统依赖（WebKitGTK 等，见 CI 的 `apt-get` 列表）

### 启动

```bash
pnpm --dir ui install        # 安装前端依赖
./ui/node_modules/.bin/tauri dev  # 启动程序
```

### 测试

```bash
cargo test --workspace --locked --no-fail-fast   # Rust 全部测试
pnpm --dir ui run typecheck                        # 前端类型检查
pnpm --dir ui run build                            # 前端构建
```

> 测试全部位于各 crate 的 `tests/`。
> wiremock 实现 `mock-ocr-cli` 自动化测试。

### 构建 / 打包

Github Action 自动打包多端可执行文件。

## 目录结构

```
crates/
  felin-core/       # Tauri-无关，程序功能（存储、OCR、分段、LLM、专名、管线）
  mock-ocr-cli/     # mock sidecar（自动化测试用）
src-tauri/          # Tauri 壳（命令面、状态、配置加载）
ui/                 # React 前端
```

## 许可证

本项目采用 **GNU Affero General Public License v3 (AGPLv3)**，见 [LICENSE](LICENSE)。

本项目通过进程调用 [ocr-router](https://github.com/Par1y/ocr-router)
（AGPLv3）。
