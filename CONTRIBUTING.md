# Contributing to CoAI Code

> 中文版: [CONTRIBUTING.zh-CN.md](./CONTRIBUTING.zh-CN.md)

Thanks for your interest in contributing! This document describes how to build,
test, and submit changes to CoAI Code.

## Getting started

You'll need a recent stable Rust toolchain (install via [rustup](https://rustup.rs/)).
The project targets the **2021 edition** and builds on stable Rust.

```bash
git clone https://github.com/coai-labs/coai-code.git
cd coai-code
cargo build
```

Copy `coai.toml.example` to `~/.coai/coai.toml` and add your provider/API key to
run the agent locally. **Never commit API keys, provider secrets, or local
state** (e.g. anything under `.coai/` or `coai-state/`).

## Building and testing

| Command | Purpose |
|---|---|
| `cargo build` | Compile the debug binary and library |
| `cargo build --release` | Build the optimized `target/release/coai` |
| `cargo run -- --help` | Check the CLI entry point |
| `cargo test` | Run unit and integration tests |
| `cargo fmt` | Apply rustfmt formatting |
| `cargo fmt --all -- --check` | Verify formatting (what CI runs) |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint (what CI runs) |

Please run `cargo fmt --all` and `cargo clippy --all-targets --all-features -- -D warnings`
before opening a pull request — CI enforces both, and warnings are treated as
errors.

For TUI changes, also run the binary locally and verify layout, wrapping,
scrolling, interrupt handling, and confirmation prompts manually, since these
are hard to cover with automated tests.

## Coding style

- Standard Rust style: 4-space indentation, `snake_case` for functions and
  modules, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- Prefer small module-local helpers before introducing shared abstractions.
- Keep TUI code readable and state transitions explicit.
- Add tests for command parsing, tool-registry behavior, serialization, and
  public library APIs. Integration tests live in `tests/`.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/)-style
prefixes, for example:

```
feat(tui): collapse long tool output
fix(llm): preserve message context on tool error
refactor(tui): simplify state transitions
docs: clarify provider override flags
```

Keep commit scopes narrow and describe user-visible behavior.

## Pull requests

1. Fork the repo and create a topic branch.
2. Make your change, with tests where it makes sense.
3. Ensure `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
   and `cargo test` all pass.
4. Open a PR with a clear summary, test results, and any linked issue or context.
   For TUI changes, include a terminal capture or screenshot.

By contributing, you agree that your contributions will be dual-licensed under
the MIT and Apache-2.0 licenses, matching the project's licensing.
