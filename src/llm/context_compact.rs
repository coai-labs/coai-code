use crate::llm::config::{Content, Message, Role, ToolDefinition};

const MESSAGE_OVERHEAD_TOKENS: usize = 8;
const TOOL_SCHEMA_FLOOR_TOKENS: usize = 2_000;

#[derive(Debug, Clone)]
pub struct CompactReport {
    pub before_messages: usize,
    pub after_messages: usize,
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub budget_tokens: usize,
    pub reason: CompactReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactReason {
    PreflightBudget,
    ProviderContextLimit,
    Manual,
}

impl CompactReason {
    pub fn label(self) -> &'static str {
        match self {
            CompactReason::PreflightBudget => "preflight budget check",
            CompactReason::ProviderContextLimit => "provider context limit exceeded",
            CompactReason::Manual => "manual compaction",
        }
    }
}

pub fn compact_messages_for_request(
    messages: Vec<Message>,
    tools: &[ToolDefinition],
    context_window: usize,
    max_output_tokens: usize,
    reason: CompactReason,
) -> (Vec<Message>, Option<CompactReport>) {
    if messages.len() <= 2 {
        return (messages, None);
    }

    let mut budget = request_message_budget(context_window, max_output_tokens, tools);
    if matches!(reason, CompactReason::ProviderContextLimit) {
        budget = (budget * 7 / 10).max(context_window / 6);
    }
    let before_tokens = estimate_messages_tokens(&messages);
    if before_tokens <= budget
        && !matches!(
            reason,
            CompactReason::ProviderContextLimit | CompactReason::Manual
        )
    {
        return (messages, None);
    }

    let before_messages = messages.len();
    let compacted = compact_to_budget(&messages, budget);
    let after_tokens = estimate_messages_tokens(&compacted);

    let report = CompactReport {
        before_messages,
        after_messages: compacted.len(),
        before_tokens,
        after_tokens,
        budget_tokens: budget,
        reason,
    };

    (compacted, Some(report))
}

pub fn sanitize_conversation_messages(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|message| match message.role {
            Role::System | Role::Tool => None,
            Role::User => {
                let text = message_text(message);
                if text.trim().is_empty() {
                    None
                } else {
                    Some(Message::user(text))
                }
            }
            Role::Assistant => {
                let text = message_text(message);
                if text.trim().is_empty() {
                    None
                } else {
                    Some(Message::assistant(text))
                }
            }
        })
        .collect()
}

pub fn is_context_limit_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "context length",
        "maximum context",
        "max context",
        "context_length",
        "context window",
        "too many tokens",
        "input tokens",
        "prompt is too long",
        "prompt too long",
        "request too large",
        "tokens exceed",
        "exceeds the context",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub fn estimate_messages_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| estimate_message_tokens(message))
        .sum()
}

fn compact_to_budget(messages: &[Message], budget: usize) -> Vec<Message> {
    let system_messages: Vec<Message> = messages
        .iter()
        .filter(|message| matches!(message.role, Role::System))
        .cloned()
        .collect();

    let non_system = sanitize_conversation_messages(messages);
    if non_system.len() <= 2 {
        let mut out = system_messages;
        out.extend(non_system);
        return out;
    }

    let system_tokens = estimate_messages_tokens(&system_messages);
    let summary_budget = (budget / 5).clamp(700, 6_000);
    let mut recent_budget = budget.saturating_sub(system_tokens + summary_budget);
    recent_budget = recent_budget.max(budget / 3);

    let mut recent_reversed = Vec::new();
    let mut used_recent = 0usize;
    for message in non_system.iter().rev() {
        let cost = estimate_message_tokens(message);
        if !recent_reversed.is_empty() && used_recent + cost > recent_budget {
            break;
        }
        used_recent += cost;
        recent_reversed.push(message.clone());
    }
    recent_reversed.reverse();

    let older_len = non_system.len().saturating_sub(recent_reversed.len());
    let older = &non_system[..older_len];
    let mut out = system_messages;

    if !older.is_empty() {
        out.push(Message::user(build_history_summary(older, summary_budget)));
    }
    out.extend(recent_reversed);

    while estimate_messages_tokens(&out) > budget && out.len() > 2 {
        let summary_idx = out.iter().position(|m| {
            matches!(m.role, Role::User)
                && message_text(m).starts_with("The following is a compacted summary")
        });
        if let Some(remove_idx) = out
            .iter()
            .enumerate()
            .find(|(idx, m)| !matches!(m.role, Role::System) && Some(*idx) != summary_idx)
            .map(|(idx, _)| idx)
        {
            out.remove(remove_idx);
        } else if let Some(summary_idx) = summary_idx {
            out.remove(summary_idx);
        } else {
            break;
        }
    }

    out
}

fn build_history_summary(messages: &[Message], token_budget: usize) -> String {
    let char_budget = token_budget.saturating_mul(3).clamp(1_200, 18_000);
    let mut lines = Vec::new();
    lines.push("The following is a compacted summary of the conversation history for context continuity. Use this information to continue the session without asking the user to repeat constraints they have already provided.".to_string());
    lines.push(String::new());
    lines.push("Key history:".to_string());

    let mut used = lines.iter().map(|s| s.chars().count() + 1).sum::<usize>();
    let mut omitted = 0usize;

    for message in messages {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
        };
        let text = concise_text(&message_text(message), 900);
        if text.trim().is_empty() {
            continue;
        }
        let line = format!("- {role}: {text}");
        let line_len = line.chars().count() + 1;
        if used + line_len > char_budget {
            omitted += 1;
            continue;
        }
        used += line_len;
        lines.push(line);
    }

    if omitted > 0 {
        lines.push(format!(
            "- {omitted} earlier message(s) omitted; re-read files via tool or ask the user if details are needed."
        ));
    }

    lines.join("\n")
}

fn request_message_budget(
    context_window: usize,
    max_output_tokens: usize,
    tools: &[ToolDefinition],
) -> usize {
    let tool_tokens = estimate_tools_tokens(tools).max(TOOL_SCHEMA_FLOOR_TOKENS);
    let reserved_output = max_output_tokens.max(1_024);
    let safety_margin = (context_window / 20).clamp(1_000, 20_000);
    context_window
        .saturating_sub(tool_tokens + reserved_output + safety_margin)
        .max(context_window / 4)
}

fn estimate_tools_tokens(tools: &[ToolDefinition]) -> usize {
    serde_json::to_string(tools)
        .map(|text| estimate_text_tokens(&text))
        .unwrap_or(TOOL_SCHEMA_FLOOR_TOKENS)
}

fn estimate_message_tokens(message: &Message) -> usize {
    estimate_text_tokens(&message_text(message))
        + message
            .reasoning_content
            .as_ref()
            .map(|text| estimate_text_tokens(text))
            .unwrap_or(0)
        + message
            .tool_calls
            .as_ref()
            .and_then(|calls| serde_json::to_string(calls).ok())
            .map(|text| estimate_text_tokens(&text))
            .unwrap_or(0)
        + MESSAGE_OVERHEAD_TOKENS
}

fn estimate_text_tokens(text: &str) -> usize {
    let char_count = text.chars().count();
    let byte_count = text.len();
    ((char_count + byte_count) / 4).max(1)
}

fn message_text(message: &Message) -> String {
    match &message.content {
        Content::Text(text) => text.clone(),
        Content::Parts(parts) => parts
            .iter()
            .filter_map(|part| part.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn concise_text(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    normalized.chars().take(max_chars).collect::<String>() + "..."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_long_history_under_budget() {
        let mut messages = vec![Message::system("system")];
        for i in 0..40 {
            messages.push(Message::user(format!("user message {i} {}", "x".repeat(400))));
            messages.push(Message::assistant(format!(
                "assistant reply {i} {}",
                "y".repeat(400)
            )));
        }
        messages.push(Message::user("current task"));

        let (compacted, report) = compact_messages_for_request(
            messages,
            &[],
            8_000,
            1_000,
            CompactReason::PreflightBudget,
        );

        assert!(report.is_some());
        assert!(estimate_messages_tokens(&compacted) <= report.unwrap().budget_tokens);
        assert!(compacted
            .iter()
            .any(|m| message_text(m).contains("The following is a compacted summary")));
        assert!(matches!(compacted.last().unwrap().role, Role::User));
    }

    #[test]
    fn detects_provider_context_limit_errors() {
        assert!(is_context_limit_error(
            "This model's maximum context length is 128000 tokens"
        ));
        assert!(!is_context_limit_error("network timeout"));
    }
}
