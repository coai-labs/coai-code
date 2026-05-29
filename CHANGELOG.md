# Changelog

> 中文版: [CHANGELOG.zh-CN.md](./CHANGELOG.zh-CN.md)

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-05-29

### Added

- Release workflow (`.github/workflows/release.yml`) — triggers on tag push (`v*`), cross-compiles and uploads binaries for Linux x86_64/aarch64, macOS x86_64/aarch64, and Windows x86_64.
- Installation script (`install.sh`) for one-line install on Linux/macOS; auto-detects platform, fetches the latest release, and installs to `~/.local/bin`.

## [0.1.0] - 2026-05-29

### Added

- **Interactive TUI mode** — inline terminal UI with native scrollback, mouse wheel, scrollbar, and text selection; a persistent input box stays at the bottom.
- **Headless one-shot mode** — pipe a task on stdin for scripting and CI benchmarks; mutations are confined to the working directory.
- **Multi-provider LLM support** — DeepSeek-first (V4 Pro/Flash tuned out of the box), plus OpenAI-compatible, Anthropic/Claude, and Ollama transports.
- **Tool-using agent loop** — file read/write/edit, shell exec/build/test, search, git operations, project memory, skills, history, and workspace cleanup.
- **Slash commands** — in-session commands for controlling agent behavior, listing tools, managing history, and more.
- **Conversation history and session persistence** — multi-turn context preserved across tool calls; session state survives interruptions.
- **TOML configuration** — `~/.coai/coai.toml` with per-provider settings, env-var interpolation for API keys, and per-invocation CLI overrides (`--provider`, `--model`, `--base-url`).
- **Model-aware context management** — context window size, output token limits, and reasoning effort adapt to the configured model.
- **Safety features** — path containment for file and exec tools, confirmation prompts in interactive mode, and bounded autonomy in headless mode (remote git ops require `COAI_AUTONOMOUS=1`).
- **`coai doctor` subcommand** — verifies environment, active provider, model, and API key at a glance.

[Unreleased]: https://github.com/coai-labs/coai-code/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/coai-labs/coai-code/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/coai-labs/coai-code/releases/tag/v0.1.0
