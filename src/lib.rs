pub mod command;
pub mod config;
pub mod context;
pub mod core;
pub mod engine;
pub mod history;
pub mod llm;
pub mod run_log;
pub mod session;
pub mod skills;
pub mod tools;
pub mod tui;
pub mod types;

#[allow(ambiguous_glob_reexports)]
pub mod prelude {
    pub use crate::command::*;
    pub use crate::config::*;
    pub use crate::context::*;
    pub use crate::core::*;
    pub use crate::engine::*;
    pub use crate::history::*;
    pub use crate::llm::*;
    pub use crate::tools::*;
    pub use crate::types::*;
}

pub use crate::config::*;
pub use crate::core::error::{CoAIError, Result};
pub use crate::core::types::*;
pub use crate::types::*;

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct CoAIAgent {
    workspace: PathBuf,
    tools: tools::ToolRegistry,
    context: Arc<RwLock<context::ContextManager>>,
    history: Arc<RwLock<history::HistoryStore>>,
    llm_config: Option<llm::LLMConfig>,
    #[allow(dead_code)]
    tui_enabled: bool,
}

impl CoAIAgent {
    pub fn new() -> Self {
        Self::builder().build()
    }

    pub fn builder() -> CoAIAgentBuilder {
        CoAIAgentBuilder::default()
    }

    /// Execute a task via the LLM tool loop
    pub async fn execute_task(&self, description: &str) -> Result<TaskRecord> {
        let mut record = TaskRecord {
            description: description.to_string(),
            status: TaskStatus::InProgress,
            created_at: chrono::Utc::now(),
            ..TaskRecord::default()
        };

        self.history.write().await.store(record.clone())?;

        if let Some(_config) = &self.llm_config {
            match self.execute_with_llm(description).await {
                Ok(output) => {
                    record.status = TaskStatus::Completed;
                    record.result = Some(output);
                }
                Err(e) => {
                    record.status = TaskStatus::Failed;
                    record.result = Some(format!("Execution failed: {}", e));
                }
            }
        } else {
            record.status = TaskStatus::Completed;
            record.result = Some(format!("Task accepted: {}", description));
        }

        record.completed_at = Some(chrono::Utc::now());
        self.history.write().await.store(record.clone())?;

        Ok(record)
    }

    async fn execute_with_llm(&self, description: &str) -> Result<String> {
        let config = self
            .llm_config
            .as_ref()
            .ok_or_else(|| CoAIError::Other("LLM not configured".into()))?;

        let client = llm::create_client(config.clone())?;
        let mut tool_loop =
            llm::ToolCallLoop::new(client, self.tools.clone()).with_model(&config.model);

        let mut thinking_started = false;
        let mut current_tool: Option<String> = None;

        let result = tool_loop
            .run(description, |event| {
                use std::io::Write;
                match event {
                    llm::tool_loop::LoopEvent::Reasoning(text)
                    | llm::tool_loop::LoopEvent::TextOutput(text) => {
                        if !thinking_started {
                            println!("\n▸ Analyzing...\n");
                            thinking_started = true;
                        }
                        print!("{}", text);
                        std::io::stdout().flush().ok();
                    }
                    llm::tool_loop::LoopEvent::ToolStart { name, .. } => {
                        if thinking_started {
                            println!();
                            thinking_started = false;
                        }
                        current_tool = Some(name.clone());
                        println!("\n▸ Calling tool: {}", name);
                    }
                    llm::tool_loop::LoopEvent::ToolOutput { name: _, result } => {
                        let status = if result.success { "done" } else { "failed" };
                        let preview = match &result.output {
                            Some(v) => {
                                let s = serde_json::to_string(v).unwrap_or_default();
                                if s.len() > 120 {
                                    let end = s
                                        .char_indices()
                                        .take(120)
                                        .last()
                                        .map(|(i, c)| i + c.len_utf8())
                                        .unwrap_or(120.min(s.len()));
                                    format!("{}...", &s[..end])
                                } else {
                                    s
                                }
                            }
                            None => result.error.clone().unwrap_or_default(),
                        };
                        println!("  Result ({}): {}", status, preview);
                        current_tool = None;
                    }
                    llm::tool_loop::LoopEvent::LiveContextApplied { .. }
                    | llm::tool_loop::LoopEvent::MessagesCheckpoint(_) => {}
                    llm::tool_loop::LoopEvent::Response(text) => {
                        if thinking_started {
                            println!();
                            thinking_started = false;
                        }
                        println!("\n▸ Completed:");
                        println!("{}", text);
                    }
                    llm::tool_loop::LoopEvent::Error(e) => {
                        if thinking_started {
                            println!();
                            thinking_started = false;
                        }
                        eprintln!("\n▸ Error: {}", e);
                    }
                }
            })
            .await?;

        Ok(result)
    }

    pub async fn execute_tool(&self, call: &ToolCall) -> Result<ToolResult> {
        self.tools.execute(call).await
    }

    pub async fn run_messages_with_tools(
        &self,
        messages: Vec<llm::Message>,
        on_event: impl FnMut(llm::tool_loop::LoopEvent) + Send + Sync,
    ) -> Result<(String, Vec<llm::Message>)> {
        let config = self
            .llm_config
            .as_ref()
            .ok_or_else(|| CoAIError::Other("LLM not configured".into()))?;

        let client = llm::create_client(config.clone())?;
        let caps = llm::model_caps::get_model_capabilities(&config.model);
        let system_prompt =
            self.tools
                .augment_system_prompt(llm::model_caps::tool_loop_system_prompt(
                    caps.context_length,
                ));
        let mut request_messages = Vec::new();
        if !matches!(
            messages.first().map(|message| &message.role),
            Some(llm::Role::System)
        ) {
            request_messages.push(llm::Message::system(system_prompt));
        }
        request_messages.extend(messages);

        let mut tool_loop =
            llm::ToolCallLoop::new(client, self.tools.clone()).with_model(&config.model);
        tool_loop
            .run_with_messages(request_messages, on_event)
            .await
            .map_err(|(e, _)| e)
    }

    pub async fn load_context(&self, path: &str) -> Result<String> {
        let path = self.workspace.join(path);
        let mut ctx = self.context.write().await;
        ctx.load(&path).map(|s| s.to_string())
    }

    pub async fn release_context(&self, path: &str) {
        let path = self.workspace.join(path);
        let mut ctx = self.context.write().await;
        ctx.release(&path);
    }

    pub async fn context_status(&self) -> context::ContextStatus {
        self.context.read().await.status()
    }

    pub async fn save_state(&self) -> Result<context::StateSnapshot> {
        self.context.read().await.save()
    }

    pub async fn restore_state(&self, snapshot: &context::StateSnapshot) -> Result<()> {
        self.context.write().await.restore(snapshot)
    }

    pub async fn list_tools(&self) -> Vec<tools::ToolInfo> {
        self.tools.list_tools()
    }

    pub async fn query_history(&self, condition: history::QueryCondition) -> Vec<TaskRecord> {
        self.history
            .read()
            .await
            .query(condition)
            .into_iter()
            .cloned()
            .collect()
    }

    pub async fn list_history(&self, limit: Option<usize>) -> Vec<TaskRecord> {
        self.history
            .read()
            .await
            .list(limit)
            .into_iter()
            .cloned()
            .collect()
    }

    pub async fn export_history(&self, format: history::ExportFormat) -> Result<String> {
        self.history.read().await.export(format)
    }

    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    pub fn llm_config(&self) -> Option<&llm::LLMConfig> {
        self.llm_config.as_ref()
    }
}

pub struct CoAIAgentBuilder {
    workspace: PathBuf,
    tui_enabled: bool,
    persistence_path: PathBuf,
    context_window: usize,
    llm_config: Option<llm::LLMConfig>,
}

impl Default for CoAIAgentBuilder {
    fn default() -> Self {
        Self {
            workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            tui_enabled: true,
            persistence_path: PathBuf::from(".coai/state"),
            context_window: 200_000,
            llm_config: None,
        }
    }
}

impl CoAIAgentBuilder {
    pub fn workspace(mut self, path: impl Into<PathBuf>) -> Self {
        self.workspace = path.into();
        self
    }

    pub fn tui_enabled(mut self, enabled: bool) -> Self {
        self.tui_enabled = enabled;
        self
    }

    pub fn persistence_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.persistence_path = path.into();
        self
    }

    pub fn context_window(mut self, size: usize) -> Self {
        self.context_window = size;
        self
    }

    pub fn llm_config(mut self, config: llm::LLMConfig) -> Self {
        self.llm_config = Some(config);
        self
    }

    pub fn openai(mut self, model: impl Into<String>, api_key: impl Into<String>) -> Self {
        self.llm_config = Some(llm::LLMConfig::openai(model, api_key));
        self
    }

    pub fn anthropic(mut self, model: impl Into<String>, api_key: impl Into<String>) -> Self {
        self.llm_config = Some(llm::LLMConfig::anthropic(model, api_key));
        self
    }

    pub fn ollama(mut self, model: impl Into<String>, base_url: impl Into<String>) -> Self {
        self.llm_config = Some(llm::LLMConfig::ollama(model, base_url));
        self
    }

    pub fn build(self) -> CoAIAgent {
        CoAIAgent {
            tools: self
                .llm_config
                .clone()
                .map(|config| tools::ToolRegistry::new(&self.workspace).with_llm_config(config))
                .unwrap_or_else(|| tools::ToolRegistry::new(&self.workspace)),
            context: Arc::new(RwLock::new(context::ContextManager::new(
                self.context_window,
                &self.persistence_path,
            ))),
            history: Arc::new(RwLock::new(history::HistoryStore::new(
                self.persistence_path.join("history.json"),
            ))),
            workspace: self.workspace,
            llm_config: self.llm_config,
            tui_enabled: self.tui_enabled,
        }
    }
}

impl Default for CoAIAgent {
    fn default() -> Self {
        Self::new()
    }
}
