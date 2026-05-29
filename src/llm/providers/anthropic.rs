use crate::core::{CoAIError, Result};
use crate::llm::client::{
    FinishReason, LLMClient, LLMResponse, PendingToolCall, StreamEvent, UsageStats,
};
use crate::llm::config::{Content, LLMConfig, Message, Role, ToolDefinition};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;

pub struct AnthropicClient {
    config: LLMConfig,
    client: Client,
}

impl AnthropicClient {
    pub fn new(config: LLMConfig) -> Result<Self> {
        Self::new_with_client(config, None)
    }

    pub fn new_with_client(config: LLMConfig, shared_client: Option<Client>) -> Result<Self> {
        let client = if let Some(client) = shared_client {
            client
        } else {
            let mut builder = Client::builder().connect_timeout(std::time::Duration::from_secs(30));
            if let Some(timeout) = config
                .extra_params
                .get("timeout_seconds")
                .and_then(|v| v.as_u64())
            {
                builder = builder.timeout(std::time::Duration::from_secs(timeout));
            }
            builder
                .build()
                .map_err(|e| CoAIError::Other(format!("failed to create HTTP client: {}", e)))?
        };

        Ok(Self { config, client })
    }

    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        stream: bool,
    ) -> serde_json::Value {
        let system_message: Vec<_> = messages
            .iter()
            .filter(|m| matches!(m.role, Role::System))
            .collect();

        let other_messages: Vec<_> = messages
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .collect();

        let system_text = system_message
            .first()
            .and_then(|m| match &m.content {
                Content::Text(t) => Some(t.as_str()),
                Content::Parts(_) => None,
            })
            .unwrap_or("You are a helpful AI assistant.");

        let messages_json: Vec<serde_json::Value> = {
            let mut result: Vec<serde_json::Value> = Vec::new();
            let mut msgs = other_messages.iter().peekable();

            while let Some(m) = msgs.next() {
                let role_str = match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "user", // Anthropic uses "user" role for tool results
                    _ => "user",
                };

                let mut content_blocks: Vec<serde_json::Value> = Vec::new();

                // Tool result -> tool_result block
                if let Some(tool_use_id) = &m.tool_call_id {
                    let result_text = match &m.content {
                        Content::Text(t) => t.clone(),
                        _ => String::new(),
                    };
                    content_blocks.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": result_text
                    }));

                    // Merge consecutive tool_result messages into single user message
                    // (Anthropic requires all tool_results for one assistant turn in one user message)
                    while let Some(next) = msgs.peek() {
                        if let Some(tool_use_id) = next.tool_call_id.as_ref() {
                            let next_text = match &next.content {
                                Content::Text(t) => t.clone(),
                                _ => String::new(),
                            };
                            content_blocks.push(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": tool_use_id,
                                "content": next_text
                            }));
                            msgs.next(); // consume
                        } else {
                            break;
                        }
                    }
                } else {
                    // Thinking blocks must come before text content (Anthropic API requirement)
                    if let Some(ref thinking_json) = m.reasoning_content {
                        if let Ok(blocks) =
                            serde_json::from_str::<Vec<serde_json::Value>>(thinking_json)
                        {
                            for b in blocks {
                                content_blocks.push(b);
                            }
                        } else {
                            content_blocks.push(serde_json::json!({
                                "type": "thinking",
                                "thinking": thinking_json
                            }));
                        }
                    }

                    // Text content
                    match &m.content {
                        Content::Text(t) if !t.is_empty() => {
                            content_blocks.push(serde_json::json!({
                                "type": "text",
                                "text": t
                            }));
                        }
                        Content::Parts(parts) => {
                            for p in parts {
                                content_blocks.push(serde_json::json!({
                                    "type": p.content_type,
                                    "text": p.text
                                }));
                            }
                        }
                        _ => {}
                    }
                }

                // Tool calls -> tool_use blocks (assistant messages, must come after text)
                if let Some(tool_calls) = &m.tool_calls {
                    for tc in tool_calls {
                        let input: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(serde_json::json!({}));
                        content_blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": input
                        }));
                    }
                }

                // Anthropic API requires at least one content block per message
                if content_blocks.is_empty() {
                    content_blocks.push(serde_json::json!({
                        "type": "text",
                        "text": ""
                    }));
                }

                result.push(serde_json::json!({
                    "role": role_str,
                    "content": content_blocks
                }));
            }

            result
        };

        let mut body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "system": system_text,
            "messages": messages_json,
            "stream": stream,
            "temperature": self.config.temperature,
        });

        // DeepSeek extended thinking — pass through if configured
        if let Some(thinking) = self.config.extra_params.get("thinking") {
            body["thinking"] = thinking.clone();
        }

        // Forward any other Anthropic-specific extra params
        for (key, value) in &self.config.extra_params {
            if key != "thinking" && key != "timeout_seconds" && key != "reasoning_effort" {
                body[key] = value.clone();
            }
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        "input_schema": t.function.parameters
                    })
                })
                .collect::<Vec<_>>());
        }

        body
    }

    fn parse_response(&self, data: &serde_json::Value) -> Result<LLMResponse> {
        let content_blocks = data["content"]
            .as_array()
            .ok_or_else(|| CoAIError::Other("malformed response: missing content".into()))?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut thinking_blocks: Vec<serde_json::Value> = Vec::new();

        for block in content_blocks {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(text) = block["text"].as_str() {
                        content.push_str(text);
                    }
                }
                Some("thinking") => {
                    // Save thinking blocks to pass back (DeepSeek requirement)
                    thinking_blocks.push(block.clone());
                }
                Some("tool_use") => {
                    let id = block["id"].as_str().unwrap_or("").to_string();
                    let name = block["name"].as_str().unwrap_or("").to_string();
                    // input may be an object or a JSON string (provider-dependent)
                    let arguments = match &block["input"] {
                        serde_json::Value::Object(_) => block["input"].clone(),
                        serde_json::Value::String(s) => {
                            serde_json::from_str(s).unwrap_or(serde_json::json!({}))
                        }
                        _ => block["input"].clone(),
                    };

                    tool_calls.push(PendingToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                _ => {}
            }
        }

        let finish_reason = match data["stop_reason"].as_str() {
            Some("end_turn") => FinishReason::Stop,
            Some("tool_use") => FinishReason::ToolCalls,
            Some("max_tokens") => FinishReason::Length,
            _ => FinishReason::Stop,
        };

        let usage = data["usage"].as_object().map(|u| UsageStats {
            prompt_tokens: u["input_tokens"].as_u64().unwrap_or(0) as usize,
            completion_tokens: u["output_tokens"].as_u64().unwrap_or(0) as usize,
            total_tokens: 0,
        });

        let thinking_json = if !thinking_blocks.is_empty() {
            Some(serde_json::to_string(&thinking_blocks).unwrap_or_default())
        } else {
            None
        };

        Ok(LLMResponse {
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            reasoning_content: thinking_json,
            tool_calls,
            finish_reason,
            usage,
        })
    }

    fn get_api_key(&self) -> Result<String> {
        self.config
            .api_key
            .clone()
            .or_else(|| {
                self.config
                    .base_url
                    .as_deref()
                    .filter(|url| url.contains("api.deepseek.com"))
                    .and_then(|_| std::env::var("DEEPSEEK_API_KEY").ok())
            })
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .ok_or_else(|| {
                CoAIError::Other("DEEPSEEK_API_KEY or ANTHROPIC_API_KEY is not configured".into())
            })
    }
    fn get_base_url(&self) -> String {
        let base = self
            .config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        if base.ends_with("/v1/messages") || base.ends_with("/v1/messages/") {
            base
        } else {
            format!("{}/v1/messages", base.trim_end_matches('/'))
        }
    }
}

#[async_trait]
impl LLMClient for AnthropicClient {
    async fn chat(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<LLMResponse> {
        let url = self.get_base_url();
        let body = self.build_request_body(messages, tools, false);

        let response = self
            .client
            .post(url)
            .header("x-api-key", self.get_api_key()?)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CoAIError::Other(format!("request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(CoAIError::Other(format!("API error {}: {}", status, text)));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| CoAIError::Other(format!("failed to parse response: {}", e)))?;

        self.parse_response(&data)
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Box<dyn Stream<Item = StreamEvent> + Send + Unpin>> {
        let url = self.get_base_url();
        let body = self.build_request_body(messages, tools, true);

        let response = self
            .client
            .post(url)
            .header("x-api-key", self.get_api_key()?)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CoAIError::Other(format!("request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(CoAIError::Other(format!("API error {}: {}", status, text)));
        }

        let stream = response.bytes_stream().flat_map(|result| {
            match result {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    let mut events = Vec::new();

                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                match json["type"].as_str() {
                                    Some("message_start") => {
                                        // Stream initialization — no action needed
                                    }
                                    Some("content_block_start") => {
                                        if let Some(block) = json["content_block"].as_object() {
                                            if let Some("tool_use") = block["type"].as_str() {
                                                let id =
                                                    block["id"].as_str().unwrap_or("").to_string();
                                                let name = block["name"]
                                                    .as_str()
                                                    .unwrap_or("")
                                                    .to_string();
                                                events
                                                    .push(StreamEvent::ToolCallStart { id, name });
                                            }
                                        }
                                    }
                                    Some("content_block_delta") => {
                                        // Text delta
                                        if let Some(delta) = json["delta"]["text"].as_str() {
                                            events.push(StreamEvent::TextDelta(delta.to_string()));
                                        }
                                        // Thinking delta (DeepSeek/Anthropic extended thinking)
                                        if let Some(delta) = json["delta"]["thinking"].as_str() {
                                            events.push(StreamEvent::ReasoningDelta(
                                                delta.to_string(),
                                            ));
                                        }
                                        // Tool use argument delta (Anthropic: partial_json)
                                        if let Some(delta) = json["delta"]["partial_json"].as_str()
                                        {
                                            events.push(StreamEvent::ToolCallDelta {
                                                id: String::new(),
                                                delta: delta.to_string(),
                                            });
                                        }
                                    }
                                    Some("content_block_stop") => {
                                        // Block boundary — no action needed, deltas are already accumulated
                                    }
                                    Some("message_delta") => {
                                        let stop_reason = json["delta"]["stop_reason"]
                                            .as_str()
                                            .map(|s| s.to_string());
                                        let usage = json["usage"].as_object().map(|u| UsageStats {
                                            prompt_tokens: 0, // not provided in message_delta
                                            completion_tokens: u["output_tokens"]
                                                .as_u64()
                                                .unwrap_or(0)
                                                as usize,
                                            total_tokens: 0,
                                        });
                                        events
                                            .push(StreamEvent::MessageDelta { stop_reason, usage });
                                    }
                                    Some("message_stop") => {
                                        events.push(StreamEvent::Done);
                                    }
                                    Some("error") => {
                                        let msg = json["error"]["message"]
                                            .as_str()
                                            .unwrap_or("unknown stream error");
                                        events.push(StreamEvent::Error(msg.to_string()));
                                    }
                                    Some("ping") => {
                                        // Keep-alive, no action needed
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    futures::stream::iter(events)
                }
                Err(e) => futures::stream::iter(vec![StreamEvent::Error(e.to_string())]),
            }
        });

        Ok(Box::new(stream))
    }

    fn convert_tool(&self, tool: &crate::tools::ToolInfo) -> ToolDefinition {
        let api_name = tool.name.replace('.', "_");
        ToolDefinition::new(&api_name, &tool.description, tool.schema())
    }

    fn provider_name(&self) -> &'static str {
        "anthropic"
    }
}
