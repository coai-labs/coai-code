use crate::core::{Result, ToolResult};
use crate::llm::client::{
    FinishReason, LLMClient, LLMResponse, PendingToolCall, StreamEvent, UsageStats,
};
use crate::llm::config::{Content, Message, Role, ToolDefinition};
use crate::llm::model_caps::{get_model_capabilities, tool_loop_system_prompt};
use crate::tools::{ToolProgressEvent, ToolRegistry};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use std::sync::{Arc, Mutex};

/// Trait for confirming tool execution (file read/write, etc.)
#[async_trait::async_trait]
pub trait ToolConfirmer: Send + Sync {
    async fn confirm(&self, tool_name: &str, path: &str, description: &str) -> bool;
}

pub struct ToolCallLoop {
    client: Box<dyn LLMClient>,
    tools: ToolRegistry,
    max_iterations: usize,
    pub system_prompt: String,
    model_name: String,
    confirmer: Option<Arc<dyn ToolConfirmer>>,
    live_context: Option<Arc<Mutex<Vec<String>>>>,
    last_live_context_len: usize,
}

#[derive(Debug, Clone)]
pub enum LoopEvent {
    /// Model's internal reasoning/thinking process
    Reasoning(String),
    /// Model's visible text output (not yet final — may be followed by tool calls)
    TextOutput(String),
    ToolStart {
        name: String,
        id: String,
        detail: String,
    },
    ToolOutput {
        name: String,
        result: ToolResult,
    },
    /// User input submitted while the task was running has been appended to the next model turn.
    LiveContextApplied {
        count: usize,
    },
    /// Conversation messages reached a recoverable checkpoint.
    MessagesCheckpoint(Vec<Message>),
    /// Final response when the model stops (no more tool calls)
    Response(String),
    Error(String),
}

impl ToolCallLoop {
    pub fn new(client: Box<dyn LLMClient>, tools: ToolRegistry) -> Self {
        let system_prompt = tools.augment_system_prompt(tool_loop_system_prompt(128_000));
        Self {
            client,
            tools,
            max_iterations: 50,
            system_prompt, // Default, override with with_model/with_system_prompt
            model_name: String::new(),
            confirmer: None,
            live_context: None,
            last_live_context_len: 0,
        }
    }

    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = self.tools.augment_system_prompt(prompt.into());
        self
    }

    /// Set the model name to auto-generate a context-appropriate system prompt.
    /// If with_system_prompt is also called, that takes precedence.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let model_str = model.into();
        self.model_name = model_str.clone();
        // Only auto-generate if no explicit prompt was set
        if self.system_prompt.is_empty() {
            let caps = get_model_capabilities(&model_str);
            self.system_prompt = self
                .tools
                .augment_system_prompt(tool_loop_system_prompt(caps.context_length));
        }
        self
    }

    pub fn with_confirmer(mut self, confirmer: Arc<dyn ToolConfirmer>) -> Self {
        self.confirmer = Some(confirmer);
        self
    }

    pub fn with_live_context(mut self, live_context: Arc<Mutex<Vec<String>>>) -> Self {
        self.live_context = Some(live_context);
        self.last_live_context_len = 0;
        self
    }

    /// Check live_context for new user messages since last check.
    fn drain_new_live_context(&mut self) -> Option<(usize, String)> {
        let live_context = self.live_context.as_ref()?;
        let additions = live_context.lock().unwrap();
        if additions.len() <= self.last_live_context_len {
            return None;
        }
        let new_items: Vec<&str> = additions[self.last_live_context_len..]
            .iter()
            .map(|s| s.as_str())
            .collect();
        self.last_live_context_len = additions.len();
        if new_items.is_empty() {
            return None;
        }
        let count = new_items.len();
        let text = format!(
            "Information appended by the user during execution:\n{}",
            new_items
                .iter()
                .map(|s| format!("  • {}", s))
                .collect::<Vec<_>>()
                .join("\n")
        );
        Some((count, text))
    }

    /// Run with existing messages (for session resume) and return final messages.
    /// On error, returns messages so the caller can preserve conversation context.
    pub async fn run_with_messages(
        &mut self,
        mut messages: Vec<Message>,
        mut on_event: impl FnMut(LoopEvent) + Send + Sync,
    ) -> std::result::Result<(String, Vec<Message>), (crate::core::CoAIError, Vec<Message>)> {
        if let Some(question) = clarification_question(&messages) {
            messages.push(Message::assistant(&question));
            on_event(LoopEvent::MessagesCheckpoint(messages.clone()));
            on_event(LoopEvent::Response(question.clone()));
            return Ok((question, messages));
        }

        let tool_defs = self.build_tool_definitions();
        let mut length_continuations = 0usize;

        for _iteration in 0..self.max_iterations {
            let response = match self
                .chat_with_stream(&messages, &tool_defs, &mut on_event)
                .await
            {
                Ok(r) => r,
                Err(e) => return Err((e, messages)),
            };

            match response.finish_reason {
                crate::llm::client::FinishReason::Stop => {
                    let content = response.content.unwrap_or_default();
                    messages.push(Message::assistant(&content));
                    on_event(LoopEvent::MessagesCheckpoint(messages.clone()));
                    on_event(LoopEvent::Response(content.clone()));
                    return Ok((content, messages));
                }
                crate::llm::client::FinishReason::ToolCalls => {
                    // Build assistant message with tool_calls and reasoning_content
                    let tool_calls: Vec<crate::llm::config::ToolCall> = response
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            let args_str = serde_json::to_string(&tc.arguments).unwrap_or_default();
                            crate::llm::config::ToolCall {
                                id: tc.id.clone(),
                                call_type: "function".to_string(),
                                function: crate::llm::config::FunctionCall {
                                    name: tc.name.clone(),
                                    arguments: args_str,
                                },
                            }
                        })
                        .collect();

                    let assistant_content = response.content.clone().unwrap_or_default();
                    let reasoning = response.reasoning_content.clone().unwrap_or_default();

                    if reasoning.is_empty() {
                        messages.push(Message::assistant_with_tools(
                            &assistant_content,
                            tool_calls,
                        ));
                    } else {
                        messages.push(Message::assistant_with_reasoning(
                            &assistant_content,
                            reasoning,
                            Some(tool_calls),
                        ));
                    }
                    on_event(LoopEvent::MessagesCheckpoint(messages.clone()));

                    let tool_results = match self
                        .execute_tools(&response.tool_calls, &mut on_event)
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return Err((e, messages)),
                    };

                    for result in tool_results {
                        messages.push(Message::tool_result(&result.0, &result.1));
                        on_event(LoopEvent::MessagesCheckpoint(messages.clone()));
                    }

                    // Inject live context (user messages submitted during execution)
                    if let Some((count, user_input)) = self.drain_new_live_context() {
                        messages.push(Message::user(&user_input));
                        on_event(LoopEvent::MessagesCheckpoint(messages.clone()));
                        on_event(LoopEvent::LiveContextApplied { count });
                    }
                }
                crate::llm::client::FinishReason::Length => {
                    let content = response.content.unwrap_or_default();
                    if !content.trim().is_empty() {
                        messages.push(Message::assistant(&content));
                        on_event(LoopEvent::MessagesCheckpoint(messages.clone()));
                    }

                    if length_continuations < 2 {
                        length_continuations += 1;
                        messages.push(Message::user(
                            "Continue your previous response without repeating what was already output.".to_string(),
                        ));
                        on_event(LoopEvent::MessagesCheckpoint(messages.clone()));
                        continue;
                    }

                    on_event(LoopEvent::Error(
                        "Output repeatedly hit the max token limit; auto-continuation stopped".into(),
                    ));
                    return Err((
                        crate::core::CoAIError::Other(
                            "Output repeatedly hit the max token limit; auto-continuation stopped".into(),
                        ),
                        messages,
                    ));
                }
                crate::llm::client::FinishReason::ContentFilter => {
                    on_event(LoopEvent::Error("Content was filtered by the provider".into()));
                    return Err((crate::core::CoAIError::Other("Content was filtered by the provider".into()), messages));
                }
            }
        }

        // Iteration budget exhausted. Rather than discard all progress, ask for a
        // final handoff with no tools available so the caller still gets a usable
        // summary of what was done, what remains, and any verification results.
        messages.push(Message::user(
            "Tool call budget exhausted. Do not call any more tools. Based on progress so far, provide a final summary: what was completed, what remains, how to continue, and any verification results."
                .to_string(),
        ));
        on_event(LoopEvent::MessagesCheckpoint(messages.clone()));
        match self.chat_with_stream(&messages, &[], &mut on_event).await {
            Ok(response) => {
                let content = response.content.unwrap_or_default();
                messages.push(Message::assistant(&content));
                on_event(LoopEvent::MessagesCheckpoint(messages.clone()));
                on_event(LoopEvent::Response(content.clone()));
                Ok((content, messages))
            }
            Err(e) => Err((e, messages)),
        }
    }

    /// Original run method for backward compatibility
    pub async fn run(
        &mut self,
        user_message: &str,
        on_event: impl FnMut(LoopEvent) + Send + Sync,
    ) -> Result<String> {
        let messages = vec![
            Message::system(&self.system_prompt),
            Message::user(user_message),
        ];
        self.run_with_messages(messages, on_event)
            .await
            .map(|(r, _)| r)
            .map_err(|(e, _)| e)
    }

    async fn chat_with_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        on_event: &mut (impl FnMut(LoopEvent) + Send + Sync),
    ) -> Result<LLMResponse> {
        let mut stream = self.client.chat_stream(messages, tools).await?;

        let mut text_buffer = String::new();
        let mut reasoning_buffer = String::new();
        let mut tool_calls: Vec<PendingToolCall> = Vec::new();
        let mut current_tool_args = String::new();
        let mut stop_reason: Option<String> = None;
        let mut usage: Option<UsageStats> = None;

        while let Some(event) = stream.next().await {
            match event {
                StreamEvent::TextDelta(delta) => {
                    text_buffer.push_str(&delta);
                    on_event(LoopEvent::TextOutput(delta));
                }
                StreamEvent::ReasoningDelta(delta) => {
                    reasoning_buffer.push_str(&delta);
                    on_event(LoopEvent::Reasoning(delta));
                }
                StreamEvent::ToolCallStart { id, name } => {
                    // Flush previous tool's args before starting a new one
                    if !current_tool_args.is_empty() {
                        if let Some(last) = tool_calls.last_mut() {
                            if let Ok(v) = serde_json::from_str(&current_tool_args) {
                                last.arguments = v;
                            }
                        }
                    }
                    tool_calls.push(PendingToolCall {
                        id,
                        name,
                        arguments: serde_json::Value::Null,
                    });
                    current_tool_args.clear();
                }
                StreamEvent::ToolCallDelta { id: _, delta } => {
                    // Always accept delta regardless of id (DeepSeek omits ids)
                    current_tool_args.push_str(&delta);
                }
                StreamEvent::MessageDelta {
                    stop_reason: reason,
                    usage: u,
                } => {
                    stop_reason = reason;
                    usage = u;
                }
                StreamEvent::Done => {
                    if !current_tool_args.is_empty() {
                        if let Some(last) = tool_calls.last_mut() {
                            if let Ok(v) = serde_json::from_str(&current_tool_args) {
                                last.arguments = v;
                            }
                        }
                    }
                    break;
                }
                StreamEvent::Error(e) => {
                    on_event(LoopEvent::Error(e.clone()));
                    return Err(crate::core::CoAIError::Other(e));
                }
            }
        }

        let has_tool_calls = !tool_calls.is_empty();

        // Build reasoning_content as JSON array of thinking blocks (matching parse_response format)
        let reasoning_content = if reasoning_buffer.is_empty() {
            None
        } else {
            let blocks = vec![serde_json::json!({
                "type": "thinking",
                "thinking": reasoning_buffer
            })];
            Some(serde_json::to_string(&blocks).unwrap_or_default())
        };

        let finish_reason = match stop_reason.as_deref() {
            Some("tool_use") => FinishReason::ToolCalls,
            Some("max_tokens") => FinishReason::Length,
            Some("end_turn") => FinishReason::Stop,
            Some("stop_sequence") => FinishReason::Stop,
            _ if has_tool_calls => FinishReason::ToolCalls,
            _ => FinishReason::Stop,
        };

        Ok(LLMResponse {
            content: if text_buffer.is_empty() {
                None
            } else {
                Some(text_buffer)
            },
            reasoning_content,
            tool_calls,
            finish_reason,
            usage,
        })
    }

    async fn execute_tools(
        &self,
        calls: &[PendingToolCall],
        on_event: &mut (impl FnMut(LoopEvent) + Send + Sync),
    ) -> Result<Vec<(String, String)>> {
        // Emit ToolStart for all calls first (sequential, fast)
        for call in calls {
            let detail = tool_detail(&call.name, &call.arguments);
            on_event(LoopEvent::ToolStart {
                name: call.name.clone(),
                id: call.id.clone(),
                detail: detail.clone(),
            });
        }

        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let tools_with_progress =
            self.tools
                .clone()
                .with_progress_callback(Arc::new(move |event| {
                    let _ = progress_tx.send(event);
                }));

        if calls
            .iter()
            .any(|call| requires_serial_execution(&call.name))
        {
            let mut output: Vec<Option<(String, String)>> = vec![None; calls.len()];
            for (idx, call) in calls.iter().cloned().enumerate() {
                let tool_future = execute_one_tool_call(
                    idx,
                    call,
                    tools_with_progress.clone(),
                    self.confirmer.clone(),
                );
                tokio::pin!(tool_future);

                let (idx, id, name, result) = loop {
                    tokio::select! {
                        Some(progress) = progress_rx.recv() => {
                            emit_tool_progress(progress, on_event);
                        }
                        tool_result = &mut tool_future => {
                            break tool_result;
                        }
                    }
                };

                while let Ok(progress) = progress_rx.try_recv() {
                    emit_tool_progress(progress, on_event);
                }

                record_tool_result(idx, id, name, result, &mut output, on_event);
            }

            while let Ok(progress) = progress_rx.try_recv() {
                emit_tool_progress(progress, on_event);
            }

            return Ok(output.into_iter().flatten().collect());
        }

        // Execute read-only tool calls in parallel
        let mut futures = calls
            .iter()
            .enumerate()
            .map(|(idx, call)| {
                let call = call.clone();
                let tools = tools_with_progress.clone();
                let confirmer = self.confirmer.clone();
                execute_one_tool_call(idx, call, tools, confirmer)
            })
            .collect::<FuturesUnordered<_>>();

        let mut output: Vec<Option<(String, String)>> = vec![None; calls.len()];
        let mut remaining = calls.len();

        while remaining > 0 {
            tokio::select! {
                Some(progress) = progress_rx.recv() => {
                    emit_tool_progress(progress, on_event);
                }
                next_result = futures.next() => {
                    let Some((idx, id, name, result)) = next_result else {
                        break;
                    };
                    remaining -= 1;
                    record_tool_result(idx, id, name, result, &mut output, on_event);
                }
            }
        }

        while let Ok(progress) = progress_rx.try_recv() {
            emit_tool_progress(progress, on_event);
        }

        Ok(output.into_iter().flatten().collect())
    }

    fn build_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .list_tools()
            .into_iter()
            .map(|t| self.client.convert_tool(&t))
            .collect()
    }

    pub fn build_tool_definitions_for_budget(&self) -> Vec<ToolDefinition> {
        self.build_tool_definitions()
    }
}

async fn execute_one_tool_call(
    idx: usize,
    call: PendingToolCall,
    tools: ToolRegistry,
    confirmer: Option<Arc<dyn ToolConfirmer>>,
) -> (
    usize,
    String,
    String,
    Result<(crate::core::ToolResult, String)>,
) {
    if needs_confirmation(&call.name) {
        if let Some(ref confirmer) = confirmer {
            // For file mutations show the actual change (content / old→new) so the
            // user can review before approving; otherwise the short tool detail.
            let detail = change_preview(&call.name, &call.arguments)
                .unwrap_or_else(|| tool_detail(&call.name, &call.arguments));
            let target = confirmation_target(&call.name, &call.arguments).unwrap_or("?");
            if !confirmer.confirm(&call.name, target, &detail).await {
                return (
                    idx,
                    call.id.clone(),
                    call.name.clone(),
                    Err(crate::core::CoAIError::Other(
                        "User cancelled this operation".to_string(),
                    )),
                );
            }
        }
    }

    let tool_call = crate::core::ToolCall {
        tool: call.name.clone(),
        params: call.arguments.clone(),
    };

    let result = tools
        .execute(&tool_call)
        .await
        .unwrap_or_else(|e| crate::core::ToolResult {
            success: false,
            output: None,
            error: Some(format!("{}", e)),
            context_impact: None,
        });

    let raw_output = match &result.output {
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
        None => result.error.clone().unwrap_or_default(),
    };
    let model_output = compact_tool_output_for_model(&call.name, &raw_output);

    (
        idx,
        call.id.clone(),
        call.name.clone(),
        Ok((result, model_output)),
    )
}

fn record_tool_result(
    idx: usize,
    id: String,
    name: String,
    result: Result<(crate::core::ToolResult, String)>,
    output: &mut [Option<(String, String)>],
    on_event: &mut (impl FnMut(LoopEvent) + Send + Sync),
) {
    match result {
        Ok((tool_result, output_str)) => {
            on_event(LoopEvent::ToolOutput {
                name,
                result: tool_result,
            });
            output[idx] = Some((id, output_str));
        }
        Err(e) => {
            let cancelled = crate::core::ToolResult {
                success: false,
                output: None,
                error: Some(e.to_string()),
                context_impact: None,
            };
            on_event(LoopEvent::ToolOutput {
                name,
                result: cancelled,
            });
            output[idx] = Some((id, e.to_string()));
        }
    }
}

fn emit_tool_progress(
    progress: ToolProgressEvent,
    on_event: &mut (impl FnMut(LoopEvent) + Send + Sync),
) {
    match progress {
        ToolProgressEvent::ToolStart { name, detail } => {
            on_event(LoopEvent::ToolStart {
                name,
                id: String::new(),
                detail,
            });
        }
        ToolProgressEvent::ToolOutput {
            name,
            success,
            preview,
        } => {
            on_event(LoopEvent::ToolOutput {
                name,
                result: crate::core::ToolResult {
                    success,
                    output: if preview.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::String(preview))
                    },
                    error: None,
                    context_impact: None,
                },
            });
        }
    }
}

fn clarification_question(messages: &[Message]) -> Option<String> {
    let latest = latest_user_text(messages)?;
    let text = latest.trim();

    if text.is_empty() || explicitly_allows_investigation(text) {
        return None;
    }

    if is_short_acknowledgement(text) && has_prior_assistant_message(messages) {
        return None;
    }

    if !needs_goal_clarification(text) {
        return None;
    }

    Some(build_clarification_question(text))
}

fn latest_user_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if !matches!(&message.role, Role::User) {
            return None;
        }

        match &message.content {
            Content::Text(text) => Some(text.clone()),
            Content::Parts(parts) => {
                let text = parts
                    .iter()
                    .filter_map(|part| part.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.trim().is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
        }
    })
}

fn explicitly_allows_investigation(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.contains("investigate")
        || lowered.contains("look through")
        || lowered.contains("explore")
        || lowered.contains("check yourself")
        || lowered.contains("figure it out")
}

fn is_short_acknowledgement(text: &str) -> bool {
    let compact = text
        .trim_matches(|c: char| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’')
        })
        .to_ascii_lowercase();
    matches!(compact.as_str(), "ok" | "okay" | "sure" | "yes" | "yep")
}

fn has_prior_assistant_message(messages: &[Message]) -> bool {
    messages
        .iter()
        .rev()
        .skip(1)
        .any(|message| matches!(&message.role, Role::Assistant))
}

fn needs_goal_clarification(text: &str) -> bool {
    let compact = text
        .trim_matches(|c: char| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’')
        })
        .to_ascii_lowercase();

    let short_ambiguous = matches!(
        compact.as_str(),
        "ok" | "okay" | "take a look" | "handle it" | "deal with it"
    );

    if short_ambiguous {
        return true;
    }

    let asks_to_confirm_scope = (compact.contains("confirm") || compact.contains("clarify"))
        && (compact.contains("goal")
            || compact.contains("scope")
            || compact.contains("objective")
            || compact.contains("acceptance"));

    let ambiguous_ok_task = compact.contains("ok")
        && (compact.contains("goal")
            || compact.contains("scope")
            || compact.contains("task")
            || compact.contains("confirm"));

    asks_to_confirm_scope || ambiguous_ok_task
}

fn build_clarification_question(text: &str) -> String {
    let target = extract_quoted_target(text).unwrap_or("this task");
    format!("Before I start on {target}, I need to confirm the goal and scope: what exactly needs to be done? What is the expected output? What are the acceptance criteria?")
}

fn extract_quoted_target(text: &str) -> Option<&str> {
    for (open, close) in [
        ('\"', '\"'),
        ('\'', '\''),
        ('`', '`'),
        ('“', '”'),
        ('‘', '’'),
    ] {
        let Some(start) = text.find(open) else {
            continue;
        };
        let rest = &text[start + open.len_utf8()..];
        let Some(end) = rest.find(close) else {
            continue;
        };
        let target = rest[..end].trim();
        if !target.is_empty() {
            return Some(target);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_before_investigating_scope_confirmation_tasks() {
        let prompt = tool_loop_system_prompt(1_000_000);
        let messages = vec![
            Message::system(&prompt),
            Message::user("confirm the goal and scope of the \"ok\" task"),
        ];

        let question = clarification_question(&messages).expect("should ask for clarification");
        assert!(question.contains("ok"));
        assert!(question.contains("needs to be done"));
        assert!(question.contains("acceptance criteria"));
    }

    #[test]
    fn asks_before_investigating_short_ambiguous_tasks() {
        let messages = vec![Message::user("ok")];

        let question = clarification_question(&messages).expect("should ask for clarification");
        assert!(question.contains("goal and scope"));
    }

    #[test]
    fn allows_explicit_investigation() {
        let messages = vec![Message::user(
            "confirm the goal and scope of the \"ok\" task, investigate yourself first",
        )];

        assert!(clarification_question(&messages).is_none());
    }

    #[test]
    fn does_not_block_clear_tasks() {
        let messages = vec![Message::user("update the help text in src/main.rs")];

        assert!(clarification_question(&messages).is_none());
    }

    #[test]
    fn does_not_treat_ok_reply_as_new_ambiguous_task() {
        let messages = vec![
            Message::user("Should I continue?"),
            Message::assistant("Can I proceed?"),
            Message::user("ok"),
        ];

        assert!(clarification_question(&messages).is_none());
    }

    #[test]
    fn compacts_exec_output_for_model_context() {
        let raw = serde_json::json!({
            "stdout": "a".repeat(30_000),
            "stderr": "b".repeat(12_000),
            "exit_code": 1,
            "success": false
        })
        .to_string();

        let compacted = compact_tool_output_for_model("exec_run", &raw);

        assert!(compacted.contains("exit_code: 1"));
        assert!(compacted.contains("stdout"));
        assert!(compacted.contains("stderr"));
        assert!(compacted.contains("output truncated"));
        assert!(compacted.chars().count() < raw.chars().count());
    }

    #[test]
    fn keeps_file_read_more_generous_than_generic_tools() {
        let raw = "x".repeat(80_000);

        let compacted = compact_tool_output_for_model("file_read", &raw);

        assert_eq!(compacted, raw);
    }

    #[test]
    fn browser_tool_requires_confirmation_with_url_target() {
        let args = serde_json::json!({ "url": "https://example.com" });

        assert!(needs_confirmation("net.browser"));
        assert!(needs_confirmation("net_browser"));
        assert_eq!(
            confirmation_target("net.browser", &args),
            Some("https://example.com")
        );
    }
}

/// Build a human-readable detail string from tool name and arguments
/// A human-readable preview of a file mutation for the permission prompt:
/// the content for writes, and old→new for edits. Returns None for non-file tools.
fn change_preview(name: &str, args: &serde_json::Value) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("file_write") || lower.contains("file.write") {
        let path = args["path"]
            .as_str()
            .or_else(|| args["filename"].as_str())
            .unwrap_or("?");
        let content = args["content"]
            .as_str()
            .or_else(|| args["data"].as_str())
            .unwrap_or("");
        Some(format!(
            "write {} ({} lines):\n{}",
            path,
            content.lines().count(),
            preview_block(content, 60, 4000)
        ))
    } else if lower.contains("file_edit") || lower.contains("file.edit") {
        let path = args["path"].as_str().unwrap_or("?");
        let old = args["old"].as_str().unwrap_or("");
        let new = args["new"].as_str().unwrap_or("");
        Some(format!(
            "edit {}:\n--- old\n{}\n+++ new\n{}",
            path,
            preview_block(old, 30, 2000),
            preview_block(new, 30, 2000)
        ))
    } else {
        None
    }
}

/// Truncate text to at most `max_lines` lines / `max_chars` chars, noting how
/// much was omitted.
fn preview_block(text: &str, max_lines: usize, max_chars: usize) -> String {
    let total = text.lines().count();
    let mut out = String::new();
    let mut shown = 0;
    for line in text.lines().take(max_lines) {
        if out.len() >= max_chars {
            break;
        }
        out.push_str(line);
        out.push('\n');
        shown += 1;
    }
    if shown < total {
        out.push_str(&format!("… (+{} more lines)", total - shown));
    }
    out.trim_end().to_string()
}

fn tool_detail(name: &str, args: &serde_json::Value) -> String {
    match name {
        n if n.contains("exec_run") || n.contains("exec.run") => {
            args["command"].as_str().unwrap_or("?").to_string()
        }
        n if n.contains("file_write") || n.contains("file.write") => {
            let path = args["path"]
                .as_str()
                .or_else(|| args["filename"].as_str())
                .unwrap_or("?");
            let content = args["content"]
                .as_str()
                .or_else(|| args["data"].as_str())
                .unwrap_or("");
            format!("{} ({}B)", path, content.len())
        }
        n if n.contains("file_read") || n.contains("file.read") => {
            args["path"].as_str().unwrap_or("?").to_string()
        }
        n if n.contains("file_list") || n.contains("file.list") => {
            args["dir"].as_str().unwrap_or(".").to_string()
        }
        n if n.contains("net_search") || n.contains("net.search") => {
            args["query"].as_str().unwrap_or("?").to_string()
        }
        n if n.contains("net_http_get") || n.contains("net.http_get") => {
            args["url"].as_str().unwrap_or("?").to_string()
        }
        n if n.contains("net_browser") || n.contains("net.browser") => {
            args["url"].as_str().unwrap_or("?").to_string()
        }
        n if n.contains("agent_spawn") || n.contains("agent.spawn") => {
            let role = args["role"].as_str().unwrap_or("explorer").trim();
            let task = args["task"].as_str().unwrap_or("?").trim();
            let task = truncate_chars(task, 120);
            let write_scope = args["write_scope"]
                .as_str()
                .map(str::trim)
                .filter(|scope| !scope.is_empty());
            if let Some(write_scope) = write_scope {
                format!(
                    "subagent {role}: {task} (scope: {})",
                    truncate_chars(write_scope, 60)
                )
            } else {
                format!("subagent {role}: {task}")
            }
        }
        n if n.contains("search_grep") || n.contains("search.grep") => {
            args["pattern"].as_str().unwrap_or("?").to_string()
        }
        _ => String::new(),
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect::<String>() + "..."
    }
}

fn compact_tool_output_for_model(name: &str, raw: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let raw = unwrap_json_string(raw);

    if lower.contains("file_read") || lower.contains("file.read") {
        return truncate_chars_with_notice(&raw, 220_000, "file.read");
    }

    if lower.contains("exec_run") || lower.contains("exec.run") {
        return compact_exec_output(&raw);
    }

    if lower.contains("net_http") || lower.contains("net.http") {
        return truncate_chars_with_notice(&raw, 80_000, "http");
    }

    if lower.contains("search_grep")
        || lower.contains("search.grep")
        || lower.contains("search_find")
        || lower.contains("search.find")
        || lower.contains("net_search")
        || lower.contains("net.search")
    {
        return truncate_chars_with_notice(&raw, 50_000, "search");
    }

    truncate_chars_with_notice(&raw, 40_000, "tool")
}

fn compact_exec_output(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return truncate_chars_with_notice(raw, 24_000, "exec");
    };

    let stdout = value.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    let stderr = value.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
    let exit_code = value
        .get("exit_code")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let success = value
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(exit_code == 0);

    let mut out = String::new();
    out.push_str(&format!("exit_code: {exit_code}\nsuccess: {success}\n"));
    if !stdout.trim().is_empty() {
        out.push_str("\nstdout:\n");
        out.push_str(&truncate_chars_with_notice(stdout.trim(), 16_000, "stdout"));
    }
    if !stderr.trim().is_empty() {
        out.push_str("\n\nstderr:\n");
        out.push_str(&truncate_chars_with_notice(stderr.trim(), 8_000, "stderr"));
    }
    if stdout.trim().is_empty() && stderr.trim().is_empty() {
        out.push_str("\n(no output)");
    }
    out
}

fn unwrap_json_string(raw: &str) -> String {
    serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.to_string())
}

fn truncate_chars_with_notice(text: &str, max_chars: usize, label: &str) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }

    let head = max_chars.saturating_mul(3) / 4;
    let tail = max_chars.saturating_sub(head);
    let start = text.chars().take(head).collect::<String>();
    let end = text
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();

    format!(
        "{start}\n\n[{label} output truncated: kept first {head} and last {tail} characters for model context, omitting ~{} characters. Full output is still visible in the UI.]\n\n{end}",
        char_count.saturating_sub(max_chars)
    )
}

fn needs_confirmation(tool_name: &str) -> bool {
    tool_name.contains("file_write")
        || tool_name.contains("file.write")
        || tool_name.contains("file_read")
        || tool_name.contains("file.read")
        || tool_name.contains("file_edit")
        || tool_name.contains("file.edit")
        || tool_name.contains("file_delete")
        || tool_name.contains("file.delete")
        || tool_name.contains("file_copy")
        || tool_name.contains("file.copy")
        || tool_name.contains("file_move")
        || tool_name.contains("file.move")
        || tool_name.contains("exec_run")
        || tool_name.contains("exec.run")
        || tool_name.contains("exec_build")
        || tool_name.contains("exec.build")
        || tool_name.contains("exec_test")
        || tool_name.contains("exec.test")
        || tool_name.contains("exec_install")
        || tool_name.contains("exec.install")
        || tool_name.contains("net_browser")
        || tool_name.contains("net.browser")
        || tool_name.contains("agent_spawn")
        || tool_name.contains("agent.spawn")
        || tool_name.contains("git_add")
        || tool_name.contains("git.add")
        || tool_name.contains("git_commit")
        || tool_name.contains("git.commit")
        || tool_name.contains("git_pull")
        || tool_name.contains("git.pull")
        || tool_name.contains("git_push")
        || tool_name.contains("git.push")
}

fn requires_serial_execution(tool_name: &str) -> bool {
    needs_confirmation(tool_name)
}

fn confirmation_target<'a>(tool_name: &str, args: &'a serde_json::Value) -> Option<&'a str> {
    if tool_name.contains("exec_run") || tool_name.contains("exec.run") {
        return args.get("command").and_then(|v| v.as_str());
    }
    if tool_name.contains("net_browser") || tool_name.contains("net.browser") {
        return args.get("url").and_then(|v| v.as_str());
    }
    if tool_name.contains("git_add") || tool_name.contains("git.add") {
        return args.get("files").and_then(|v| v.as_str());
    }
    if tool_name.contains("git_commit") || tool_name.contains("git.commit") {
        return args.get("message").and_then(|v| v.as_str());
    }
    if tool_name.contains("git_pull")
        || tool_name.contains("git.pull")
        || tool_name.contains("git_push")
        || tool_name.contains("git.push")
    {
        return args
            .get("branch")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("remote").and_then(|v| v.as_str()));
    }

    args.get("path")
        .and_then(|v| v.as_str())
        .or_else(|| args.get("filename").and_then(|v| v.as_str()))
        .or_else(|| args.get("file_path").and_then(|v| v.as_str()))
        .or_else(|| args.get("file").and_then(|v| v.as_str()))
}
