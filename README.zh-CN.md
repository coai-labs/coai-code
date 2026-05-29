# CoAI Code

> 为 DeepSeek 优化的终端 AI 编程 agent，同样支持 OpenAI、Claude 和 Ollama。

> English: [README.md](./README.md)

一款用 Rust 编写的终端自主编程 agent。它在你的代码库上执行「检查 → 规划 → 编辑 → 验证」循环，配备内联 TUI，并支持多种 LLM provider（DeepSeek、OpenAI 兼容接口、Anthropic/Claude、Ollama）。

> 状态：早期/活跃开发中（`0.1.x`），接口可能变更。

## 功能特性

- **内联 TUI** — 完成的输出流入终端原生滚动缓冲区（鼠标滚轮、滚动条、文本选择均可用）；底部保持实时输入框。
- **工具调用 agent 循环** — 支持文件读写/编辑、Shell 执行/构建/测试、搜索、git、项目记忆、skills、历史记录和工作区清理。
- **无头单次执行** — 通过 stdin 传入任务，用于脚本和基准测试。
- **模型感知** — 上下文窗口、输出限制和推理力度会根据配置的模型自动调整。DeepSeek-V4 开箱即用经过专项调优；其他 provider 通过相同循环运行。
- **安全性** — 文件/执行工具的路径隔离、交互模式下的确认提示，以及无头模式下的有界自主性。

## 安装

### 一键安装（Linux / macOS）

```sh
curl -fsSL https://raw.githubusercontent.com/coai-labs/coai-code/main/install.sh | sh
```

自动检测平台并下载最新 Release 二进制，安装到 `~/.local/bin/coai`。
Windows 用户请前往 [Release 页面](https://github.com/coai-labs/coai-code/releases) 下载预编译包。

### 从源码构建

从源码构建（需要较新的稳定版 Rust 工具链）：

```bash
git clone https://github.com/coai-labs/coai-code.git
cd coai-code
cargo install --path .
```

这会将 `coai` 二进制文件安装到 `~/.cargo/bin`（确保该目录在你的 `PATH` 中）。若只需构建而不安装，使用 `cargo build --release`，二进制文件位于 `target/release/coai`。

## 配置

CoAI Code 读取 `~/.coai/coai.toml`（或 `~/.config/coai/coai.toml`）。完整选项参见 [`coai.toml.example`](coai.toml.example)。最简配置示例：

```toml
[llm]
default_provider = "deepseek"

[llm.providers.deepseek]
provider = "anthropic"               # transport: anthropic | openai | ollama
model = "deepseek-v4-pro"
flash_model = "deepseek-v4-flash"    # 可选：将简单子任务路由到此模型
base_url = "https://api.deepseek.com/anthropic"
api_key = "${DEEPSEEK_API_KEY}"      # 支持环境变量
temperature = 0.2
max_tokens = 64000

[agent]
max_tool_iterations = 80
tool_timeout_seconds = 300
context_window = 1000000
```

API key 可通过配置文件或环境变量提供（`DEEPSEEK_API_KEY`、`OPENAI_API_KEY`、`ANTHROPIC_API_KEY`）。

也可以在每次调用时覆盖 provider，无需修改配置文件：

```bash
coai --provider deepseek --model deepseek-v4-pro "总结这个仓库"
coai --provider openai   --model gpt-4o          # 使用 OPENAI_API_KEY
coai --provider ollama   --model qwen2.5-coder:32b --base-url http://localhost:11434
```

运行 `coai doctor` 可验证环境配置，确认当前 provider、model 和 API key 是否正确加载。

## 使用方法

交互式 TUI：

```bash
coai
```

无头单次执行（从 stdin 读取完整任务）：

```bash
echo "Fix the failing test in src/parser.rs and run cargo test" | coai
cat task.md | coai
```

无头模式下，文件变更限制在当前工作目录内，除非设置 `COAI_AUTONOMOUS=1`，否则拒绝远程 git 操作（`push`/`pull`）。

其他子命令：

```bash
coai doctor            # 环境 / 配置检查
coai history list      # 任务历史
coai tool list         # 可用工具列表
coai config ...        # 管理配置
```

运行 `coai --help` 查看完整命令列表。

## 支持的模型

| Provider transport | 示例 |
|---|---|
| `anthropic` | Claude、DeepSeek（`/anthropic` 端点） |
| `openai`（及 OpenAI 兼容接口） | GPT、Gemini（OpenAI 兼容）、Qwen、DeepSeek（`/v1`）、本地服务器 |
| `ollama` | 本地模型 |

DeepSeek-V4（Pro/Flash）是经过专项调优的默认选择。其他模型通过相同 agent 循环运行，使用对应模型的默认参数。

## 开发

```bash
cargo build
cargo test
```

## 许可证

采用以下许可证之一（你可自由选择）：

- Apache License, Version 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT license（[LICENSE-MIT](LICENSE-MIT)）
