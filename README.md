# CoAI Code

> DeepSeek-first AI coding agent for your terminal. Also works with OpenAI, Claude, and Ollama.

> 中文版: [README.zh-CN.md](./README.zh-CN.md)

A terminal-based autonomous coding agent written in Rust. It runs an
inspect → plan → edit → verify loop over your codebase with an inline TUI, and
works with multiple LLM providers (DeepSeek, OpenAI-compatible,
Anthropic/Claude, Ollama).

> Status: early/active development (`0.1.x`). Interfaces may change.

## Features

- **Inline TUI** — finished output flows into the terminal's native scrollback
  (mouse wheel, scrollbar, and text selection all work); a live input box stays
  at the bottom.
- **Tool-using agent loop** — file read/write/edit, shell exec/build/test,
  search, git, project memory, skills, history, and workspace cleanup.
- **Headless one-shot** — pipe a task on stdin for scripting and benchmarks.
- **Model-aware** — context window, output limits, and reasoning effort adapt to
  the configured model. DeepSeek‑V4 is tuned out of the box; other providers
  work via the same loop.
- **Safety** — path containment for file/exec tools, confirmation prompts in
  interactive mode, and bounded autonomy in headless mode.

## Install

From source (requires a recent stable Rust toolchain):

```bash
git clone https://github.com/coai-labs/coai-code.git
cd coai-code
cargo install --path .
```

This installs the `coai` binary into `~/.cargo/bin` (make sure it is on your
`PATH`). To just build without installing, use `cargo build --release`; the
binary is then at `target/release/coai`.

## Configuration

CoAI reads `~/.coai/coai.toml` (or `~/.config/coai/coai.toml`). See
[`coai.toml.example`](coai.toml.example) for all options. A minimal config:

```toml
[llm]
default_provider = "deepseek"

[llm.providers.deepseek]
provider = "anthropic"               # transport: anthropic | openai | ollama
model = "deepseek-v4-pro"
flash_model = "deepseek-v4-flash"    # optional: route simple subtasks here
base_url = "https://api.deepseek.com/anthropic"
api_key = "${DEEPSEEK_API_KEY}"      # env vars are supported
temperature = 0.2
max_tokens = 64000

[agent]
max_tool_iterations = 80
tool_timeout_seconds = 300
context_window = 1000000
```

API keys can be provided via the config file or environment variables
(`DEEPSEEK_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`).

You can also override the provider per invocation without editing the config:

```bash
coai --provider deepseek --model deepseek-v4-pro "Summarize this repo"
coai --provider openai   --model gpt-4o          # uses OPENAI_API_KEY
coai --provider ollama   --model qwen2.5-coder:32b --base-url http://localhost:11434
```

Run `coai doctor` to verify your environment and that the active provider,
model, and API key are picked up correctly.

## Usage

Interactive TUI:

```bash
coai
```

Headless one-shot (reads the full task from stdin):

```bash
echo "Fix the failing test in src/parser.rs and run cargo test" | coai
cat task.md | coai
```

In headless mode, mutations are confined to the working directory and remote
git operations (`push`/`pull`) are refused unless `COAI_AUTONOMOUS=1` is set.

Other subcommands:

```bash
coai doctor            # environment / config check
coai history list      # task history
coai tool list         # available tools
coai config ...        # manage configuration
```

Run `coai --help` for the full list.

## Supported models

| Provider transport | Examples |
|---|---|
| `anthropic` | Claude, DeepSeek (`/anthropic` endpoint) |
| `openai` (and OpenAI-compatible) | GPT, Gemini (OpenAI-compatible), Qwen, DeepSeek (`/v1`), local servers |
| `ollama` | local models |

DeepSeek‑V4 (Pro/Flash) is the tuned default. Other models run through the same
agent loop with model-appropriate defaults.

## Development

```bash
cargo build
cargo test
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
