# 为 CoAI Code 贡献代码

> English: [CONTRIBUTING.md](./CONTRIBUTING.md)

感谢你的贡献兴趣！本文档介绍如何构建、测试并向 CoAI Code 提交变更。

## 开始之前

你需要一个较新的稳定版 Rust 工具链（通过 [rustup](https://rustup.rs/) 安装）。项目采用 **2021 edition**，在稳定版 Rust 上构建。

```bash
git clone https://github.com/coai-labs/coai-code.git
cd coai-code
cargo build
```

将 `coai.toml.example` 复制到 `~/.coai/coai.toml` 并填入你的 provider/API key，即可在本地运行 agent。**绝不要提交 API key、provider 密钥或本地状态**（例如 `.coai/` 或 `coai-state/` 目录下的任何内容）。

## 构建与测试

| 命令 | 用途 |
|---|---|
| `cargo build` | 编译 debug 二进制文件和库 |
| `cargo build --release` | 构建优化后的 `target/release/coai` |
| `cargo run -- --help` | 检查 CLI 入口点 |
| `cargo test` | 运行单元测试和集成测试 |
| `cargo fmt` | 应用 rustfmt 格式化 |
| `cargo fmt --all -- --check` | 验证格式（CI 执行此命令） |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint 检查（CI 执行此命令） |

提交 PR 前请先运行 `cargo fmt --all` 和 `cargo clippy --all-targets --all-features -- -D warnings`——CI 会强制执行这两项检查，且警告视为错误。

对于 TUI 相关变更，还需在本地运行二进制文件，手动验证布局、换行、滚动、中断处理和确认提示，因为这些内容难以通过自动化测试覆盖。

## 代码风格

- 标准 Rust 风格：4 空格缩进，函数和模块使用 `snake_case`，类型使用 `PascalCase`，常量使用 `SCREAMING_SNAKE_CASE`。
- 优先使用模块内的小型辅助函数，再考虑引入共享抽象。
- 保持 TUI 代码可读，状态转换清晰明确。
- 为命令解析、tool-registry 行为、序列化以及公共库 API 添加测试。集成测试位于 `tests/` 目录。

## Commit 消息

我们使用 [Conventional Commits](https://www.conventionalcommits.org/) 风格前缀，示例：

```
feat(tui): collapse long tool output
fix(llm): preserve message context on tool error
refactor(tui): simplify state transitions
docs: clarify provider override flags
```

保持 commit 范围精简，描述用户可感知的行为变化。

## Pull Request

1. Fork 仓库并创建主题分支。
2. 进行变更，在合理的情况下添加测试。
3. 确保 `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings` 和 `cargo test` 均通过。
4. 提交 PR，附上清晰的摘要、测试结果以及关联的 issue 或背景说明。TUI 相关变更请附上终端录屏或截图。

提交贡献即表示你同意将你的贡献以 MIT 和 Apache-2.0 双重许可证发布，与项目许可证保持一致。
