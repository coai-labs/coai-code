use crate::core::{CoAIError, Result};
use crate::llm::client::{
    FinishReason, LLMClient, LLMResponse, PendingToolCall, StreamEvent, UsageStats,
};
use crate::llm::config::{Content, LLMConfig, Message, Role, ToolDefinition};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;

pub struct OllamaClient {
    config: LLMConfig,
    client: Client,
}

impl OllamaClient {
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
        let messages_json: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                let role_str = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool",
                };

                let content = match &m.content {
                    Content::Text(t) => t.clone(),
                    Content::Parts(parts) => parts
                        .iter()
                        .filter_map(|p| p.text.clone())
                        .collect::<Vec<_>>()
                        .join("\n"),
                };

                let mut msg = serde_json::json!({
                    "role": role_str,
                    "content": content
                });

                if let Some(calls) = &m.tool_calls {
                    msg["tool_calls"] = serde_json::json!(calls);
                }
                if let Some(tool_call_id) = &m.tool_call_id {
                    msg["tool_call_id"] = serde_json::json!(tool_call_id);
                }

                msg
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": messages_json,
            "stream": stream,
            "options": {
                "temperature": self.config.temperature,
                "num_predict": self.config.max_tokens,
            }
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.function.name,
                            "description": t.function.description,
                            "parameters": t.function.parameters
                        }
                    })
                })
                .collect::<Vec<_>>());
        }

        body
    }

    fn parse_response(&self, data: &serde_json::Value) -> Result<LLMResponse> {
        let content = data["message"]["content"].as_str().map(|s| s.to_string());

        let tool_calls: Vec<PendingToolCall> = data["message"]["tool_calls"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| {
                        let id = tc["id"].as_str().unwrap_or("").to_string();
                        let name = tc["function"]["name"].as_str()?.to_string();
                        let arguments = tc["function"]["arguments"].clone();

                        Some(PendingToolCall {
                            id,
                            name,
                            arguments,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let finish_reason = if !tool_calls.is_empty() {
            FinishReason::ToolCalls
        } else if data["done"].as_bool().unwrap_or(true) {
            FinishReason::Stop
        } else {
            FinishReason::Length
        };

        let usage = UsageStats {
            prompt_tokens: data["prompt_eval_count"].as_u64().unwrap_or(0) as usize,
            completion_tokens: data["eval_count"].as_u64().unwrap_or(0) as usize,
            total_tokens: 0,
        };

        let reasoning_content = data["message"]["reasoning_content"]
            .as_str()
            .map(|s| s.to_string());

        Ok(LLMResponse {
            content,
            reasoning_content,
            tool_calls,
            finish_reason,
            usage: Some(usage),
        })
    }

    fn get_base_url(&self) -> String {
        self.config
            .base_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434".to_string())
    }
}

#[async_trait]
impl LLMClient for OllamaClient {
    async fn chat(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<LLMResponse> {
        let url = format!("{}/api/chat", self.get_base_url());
        let body = self.build_request_body(messages, tools, false);

        let response = self
            .client
            .post(&url)
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
        let url = format!("{}/api/chat", self.get_base_url());
        let body = self.build_request_body(messages, tools, true);

        let response = self
            .client
            .post(&url)
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
                        if line.is_empty() {
                            continue;
                        }

                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                            // Text content delta
                            if let Some(content) = json["message"]["content"].as_str() {
                                if !content.is_empty() {
                                    events.push(StreamEvent::TextDelta(content.to_string()));
                                }
                            }

                            // Done flag — extract tool_calls and usage before emitting Done
                            if json["done"].as_bool().unwrap_or(false) {
                                // Tool calls (sent in final message for Ollama)
                                if let Some(tool_calls) = json["message"]["tool_calls"].as_array() {
                                    for tc in tool_calls {
                                        let name = tc["function"]["name"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_string();
                                        let args = &tc["function"]["arguments"];
                                        // Arguments may be object or string (model-dependent)
                                        let args_str = match args {
                                            serde_json::Value::Object(_) => {
                                                serde_json::to_string(args).unwrap_or_default()
                                            }
                                            serde_json::Value::String(s) => s.clone(),
                                            _ => String::new(),
                                        };
                                        events.push(StreamEvent::ToolCallStart {
                                            id: String::new(),
                                            name,
                                        });
                                        if !args_str.is_empty() {
                                            events.push(StreamEvent::ToolCallDelta {
                                                id: String::new(),
                                                delta: args_str,
                                            });
                                        }
                                    }
                                }

                                // Usage stats
                                let usage = Some(UsageStats {
                                    prompt_tokens: json["prompt_eval_count"].as_u64().unwrap_or(0)
                                        as usize,
                                    completion_tokens: json["eval_count"].as_u64().unwrap_or(0)
                                        as usize,
                                    total_tokens: 0,
                                });
                                events.push(StreamEvent::MessageDelta {
                                    stop_reason: Some(
                                        if json["message"]["tool_calls"].as_array().is_some() {
                                            "tool_use".into()
                                        } else {
                                            "end_turn".into()
                                        },
                                    ),
                                    usage,
                                });

                                events.push(StreamEvent::Done);
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
        ToolDefinition::new(&tool.name, &tool.description, tool.schema())
    }

    fn provider_name(&self) -> &'static str {
        "ollama"
    }
}
