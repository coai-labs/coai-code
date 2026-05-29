# 变更日志

> English: [CHANGELOG.md](./CHANGELOG.md)

本文件记录项目所有重要变更。

格式基于 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)，
版本号遵循 [Semantic Versioning](https://semver.org/spec/v2.0.0.html)。

## [Unreleased]

## [0.1.0] - 2026-05-29

### Added

- **交互式 TUI 模式** — 内嵌终端 UI，支持原生滚动回看、鼠标滚轮、滚动条及文本选中；底部保留持久输入框。
- **Headless 单次执行模式** — 通过 stdin 传入任务，适合脚本和 CI 基准测试；文件操作限制在工作目录内。
- **多 provider LLM 支持** — 以 DeepSeek 为首选（V4 Pro/Flash 开箱调优），同时支持 OpenAI-compatible、Anthropic/Claude 和 Ollama transport。
- **Tool 调用 agent 循环** — 覆盖文件读写编辑、shell 执行/构建/测试、搜索、git 操作、项目记忆、skill、历史记录及工作区清理。
- **Slash 命令** — 会话内命令，用于控制 agent 行为、列出 tool、管理历史记录等。
- **对话历史与 session 持久化** — 多轮上下文跨 tool 调用保留；session 状态在中断后可恢复。
- **TOML 配置** — `~/.coai/coai.toml`，支持按 provider 配置、API key 环境变量插值，以及 CLI 临时覆盖（`--provider`、`--model`、`--base-url`）。
- **模型感知上下文管理** — context window 大小、输出 token 限制及推理 effort 随所配置的模型自动适配。
- **安全特性** — 文件与 exec tool 的路径限制、交互模式下的确认提示，以及 headless 模式的自主权边界（远程 git 操作需设置 `COAI_AUTONOMOUS=1`）。
- **`coai doctor` 子命令** — 一键验证环境、当前 provider、模型及 API key 是否配置正确。

[Unreleased]: https://github.com/coai-labs/coai-code/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/coai-labs/coai-code/releases/tag/v0.1.0
