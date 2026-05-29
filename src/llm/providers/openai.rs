use crate::core::{CoAIError, Result};
use crate::llm::client::{
    FinishReason, LLMClient, LLMResponse, PendingToolCall, StreamEvent, UsageStats,
};
use crate::llm::config::{LLMConfig, Message, ToolDefinition};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;

pub struct OpenAIClient {
    config: LLMConfig,
    client: Client,
}

impl OpenAIClient {
    pub fn new(config: LLMConfig) -> Result<Self> {
        Self::new_with_client(config, None)
    }

    pub fn new_with_client(config: LLMConfig, shared_client: Option<Client>) -> Result<Self> {
        let client = if let Some(c) = shared_client {
            c
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
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "role".to_string(),
                    serde_json::json!(match m.role {
                        crate::llm::config::Role::System => "system",
                        crate::llm::config::Role::User => "user",
                        crate::llm::config::Role::Assistant => "assistant",
                        crate::llm::config::Role::Tool => "tool",
                    }),
                );

                match &m.content {
                    crate::llm::config::Content::Text(text) => {
                        obj.insert("content".to_string(), serde_json::json!(text));
                    }
                    crate::llm::config::Content::Parts(parts) => {
                        obj.insert("content".to_string(), serde_json::json!(parts));
                    }
                }

                if let Some(calls) = &m.tool_calls {
                    obj.insert("tool_calls".to_string(), serde_json::json!(calls));
                }

                if let Some(tool_call_id) = &m.tool_call_id {
                    obj.insert("tool_call_id".to_string(), serde_json::json!(tool_call_id));
                }

                if let Some(reasoning) = &m.reasoning_content {
                    obj.insert(
                        "reasoning_content".to_string(),
                        serde_json::json!(reasoning),
                    );
                }

                serde_json::Value::Object(obj)
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": messages_json,
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
            "stream": stream,
        });

        // DeepSeek V4 thinking mode — reasoning_effort parameter
        if let Some(effort) = self.config.extra_params.get("reasoning_effort") {
            body["reasoning_effort"] = effort.clone();
        }

        // Forward other extra params (excluding reserved keys)
        for (key, value) in &self.config.extra_params {
            if key != "reasoning_effort" && key != "timeout_seconds" {
                body[key] = value.clone();
            }
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        body
    }

    fn parse_response(&self, data: &serde_json::Value) -> Result<LLMResponse> {
        let choices = data["choices"]
            .as_array()
            .ok_or_else(|| CoAIError::Other("malformed response: missing choices".into()))?;

        if choices.is_empty() {
            return Ok(LLMResponse::default());
        }

        let choice = &choices[0];
        let message = &choice["message"];

        let content = message["content"].as_str().map(|s| s.to_string());

        let tool_calls: Vec<PendingToolCall> = message["tool_calls"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| {
                        let id = tc["id"].as_str()?.to_string();
                        let name = tc["function"]["name"].as_str()?.to_string();
                        let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                        let arguments =
                            serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));

                        Some(PendingToolCall {
                            id,
                            name,
                            arguments,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let finish_reason = match choice["finish_reason"].as_str() {
            Some("stop") => FinishReason::Stop,
            Some("tool_calls") => FinishReason::ToolCalls,
            Some("length") => FinishReason::Length,
            Some("content_filter") => FinishReason::ContentFilter,
            _ => FinishReason::Stop,
        };

        let usage = data["usage"].as_object().map(|u| UsageStats {
            prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as usize,
            completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as usize,
            total_tokens: u["total_tokens"].as_u64().unwrap_or(0) as usize,
        });

        let reasoning_content = message["reasoning_content"].as_str().map(|s| s.to_string());

        Ok(LLMResponse {
            content,
            reasoning_content,
            tool_calls,
            finish_reason,
            usage,
        })
    }

    fn get_api_key(&self) -> Result<String> {
        self.config
            .api_key
            .clone()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .ok_or_else(|| CoAIError::Other("API key not configured".into()))
    }

    fn get_base_url(&self) -> String {
        self.config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
    }
}

#[async_trait]
impl LLMClient for OpenAIClient {
    async fn chat(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<LLMResponse> {
        let url = format!("{}/chat/completions", self.get_base_url());
        let body = self.build_request_body(messages, tools, false);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.get_api_key()?))
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
        let url = format!("{}/chat/completions", self.get_base_url());
        let body = self.build_request_body(messages, tools, true);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.get_api_key()?))
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

        let stream = response
            .bytes_stream()
            .map(move |result| {
                match result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        let mut events = Vec::new();

                        for line in text.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    events.push(StreamEvent::Done);
                                    continue;
                                }

                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                    if let Some(delta) =
                                        json["choices"][0]["delta"]["content"].as_str()
                                    {
                                        events.push(StreamEvent::TextDelta(delta.to_string()));
                                    }

                                    if let Some(reasoning) =
                                        json["choices"][0]["delta"]["reasoning_content"].as_str()
                                    {
                                        events.push(StreamEvent::ReasoningDelta(
                                            reasoning.to_string(),
                                        ));
                                    }

                                    if let Some(tc) =
                                        json["choices"][0]["delta"]["tool_calls"].as_array()
                                    {
                                        for tool in tc {
                                            let id = tool["id"].as_str().unwrap_or("").to_string();
                                            let name = tool["function"]["name"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string();
                                            let delta = tool["function"]["arguments"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string();

                                            if !name.is_empty() {
                                                events.push(StreamEvent::ToolCallStart {
                                                    id: id.clone(),
                                                    name,
                                                });
                                            }
                                            if !delta.is_empty() {
                                                events
                                                    .push(StreamEvent::ToolCallDelta { id, delta });
                                            }
                                        }
                                    }

                                    // Capture finish_reason and usage from final chunk
                                    let finish_reason =
                                        json["choices"][0]["finish_reason"].as_str();
                                    let usage = json["usage"].as_object().map(|u| UsageStats {
                                        prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0)
                                            as usize,
                                        completion_tokens: u["completion_tokens"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as usize,
                                        total_tokens: u["total_tokens"].as_u64().unwrap_or(0)
                                            as usize,
                                    });
                                    if finish_reason.is_some() || usage.is_some() {
                                        events.push(StreamEvent::MessageDelta {
                                            stop_reason: finish_reason.map(|s| s.to_string()),
                                            usage,
                                        });
                                    }
                                }
                            }
                        }
                        events
                    }
                    Err(e) => vec![StreamEvent::Error(e.to_string())],
                }
            })
            .flat_map(futures::stream::iter);

        Ok(Box::new(stream))
    }

    fn convert_tool(&self, tool: &crate::tools::ToolInfo) -> ToolDefinition {
        // Convert tool name: "file.read" -> "file_read" (for API compatibility)
        let api_name = tool.name.replace('.', "_");
        ToolDefinition::new(&api_name, &tool.description, tool.schema())
    }

    fn provider_name(&self) -> &'static str {
        "openai"
    }
}
