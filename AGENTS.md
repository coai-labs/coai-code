# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust 2021 CLI/TUI agent. `src/main.rs` defines the `coai` binary and `src/lib.rs` exposes the `coai_code` library. Core modules live under `src/`: `tui/` for the inline terminal interface, `command/` for slash commands, `llm/` for providers, `tools/` for agent tools, and `config/` for settings. Integration tests are in `tests/`.

## TUI Product Direction

The TUI should feel like a modern inline terminal agent: direct task input, compact status lines, visible tool calls, collapsible long output, interrupt/resume hints, and confirmation prompts for risky commands. Prefer terminal-native behavior over custom widgets when possible; for example, keep scrolling compatible with the terminal instead of inventing a separate scroll layer.

## Build, Test, and Development Commands

- `cargo build` compiles the debug binary and library.
- `cargo run -- --help` checks the CLI entry point.
- `cargo test` runs unit and integration tests.
- `cargo fmt` applies rustfmt formatting.
- `cargo clippy --all-targets --all-features` runs Rust lint checks.
- `cargo build --release` builds `target/release/coai`.

Use `coai.toml.example` for local configuration. Never commit API keys, provider secrets, or local state from `coai-state/`.

## Coding Style & Naming Conventions

Use standard Rust style: 4-space indentation, `snake_case` for functions/modules, `PascalCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Keep user-facing prompts and interaction copy in Chinese unless the surrounding code is already English. Prefer small module-local helpers before adding shared abstractions. Keep TUI code readable and state transitions explicit.

## Testing Guidelines

Add tests for command parsing, tool registry behavior, serialization, and public library APIs. Put integration tests in `tests/` with names such as `test_command_parser_history_list`. Run `cargo test` before handoff. For TUI changes, also run the binary locally and verify layout, wrapping, scrolling, interrupt handling, and confirmation prompts manually.

## Commit & Pull Request Guidelines

Recent commits use Conventional Commit-style prefixes such as `feat:` and `refactor(tui):`. Keep commit scopes narrow and describe user-visible behavior. Pull requests should include a summary, test results, linked issue or task context, and screenshots or terminal captures for TUI changes.

## Agent-Specific Instructions

Preserve the core principle: the system provides tools and state, while the LLM makes decisions.
