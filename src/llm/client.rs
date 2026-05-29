use crate::core::Result;
use crate::llm::config::*;
use async_trait::async_trait;
use futures::Stream;

#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    ReasoningDelta(String), // For DeepSeek thinking mode
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallDelta {
        id: String,
        delta: String,
    },
    MessageDelta {
        stop_reason: Option<String>,
        usage: Option<UsageStats>,
    },
    Done,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct LLMResponse {
    pub content: Option<String>,
    pub reasoning_content: Option<String>, // For DeepSeek thinking mode
    pub tool_calls: Vec<PendingToolCall>,
    pub finish_reason: FinishReason,
    pub usage: Option<UsageStats>,
}

#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum FinishReason {
    #[default]
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
}

#[derive(Debug, Clone, Default)]
pub struct UsageStats {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

#[async_trait]
pub trait LLMClient: Send + Sync {
    async fn chat(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<LLMResponse>;

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Box<dyn Stream<Item = StreamEvent> + Send + Unpin>>;

    fn convert_tool(&self, tool: &crate::tools::ToolInfo) -> ToolDefinition;

    fn provider_name(&self) -> &'static str;
}

pub fn create_client(config: LLMConfig) -> Result<Box<dyn LLMClient>> {
    create_client_with_http(config, None)
}

/// Create an LLM client with an optional shared HTTP client (connection pool reuse).
pub fn create_client_with_http(
    config: LLMConfig,
    shared_client: Option<reqwest::Client>,
) -> Result<Box<dyn LLMClient>> {
    match config.provider {
        LLMProvider::OpenAI | LLMProvider::OpenAICompatible | LLMProvider::Custom => Ok(Box::new(
            super::providers::OpenAIClient::new_with_client(config, shared_client)?,
        )),
        LLMProvider::Anthropic => Ok(Box::new(
            super::providers::AnthropicClient::new_with_client(config, shared_client)?,
        )),
        LLMProvider::Ollama => Ok(Box::new(super::providers::OllamaClient::new_with_client(
            config,
            shared_client,
        )?)),
    }
}

impl Default for LLMResponse {
    fn default() -> Self {
        Self {
            content: None,
            reasoning_content: None,
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: None,
        }
    }
}
