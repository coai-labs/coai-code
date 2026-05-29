//! Model capability registry — maps model names to context lengths and features.

use crate::llm::config::{LLMConfig, LLMProvider};

const DEEPSEEK_V4_CONTEXT: usize = 1_000_000;
const DEEPSEEK_V4_MAX_OUTPUT: usize = 384_000;
const DEEPSEEK_AGENT_OUTPUT: usize = 64_000;

/// Capabilities of a specific model variant.
#[derive(Debug, Clone)]
pub struct ModelCapabilities {
    /// Maximum context window in tokens.
    pub context_length: usize,
    /// Maximum output tokens.
    pub max_output: usize,
    /// Whether the model supports extended thinking / reasoning mode.
    pub supports_thinking: bool,
    /// Whether the model is cost-efficient (used for routing simple vs complex tasks).
    pub is_flash: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        // Conservative defaults: 128K context, 4K output, no thinking
        Self {
            context_length: 128_000,
            max_output: 4_096,
            supports_thinking: false,
            is_flash: false,
        }
    }
}

/// Look up model capabilities by model name string.
/// Matches by prefix so "deepseek-v4-pro-0324" still matches "deepseek-v4-pro".
pub fn get_model_capabilities(model: &str) -> ModelCapabilities {
    let lower = model.to_ascii_lowercase();

    // DeepSeek V4
    if is_deepseek_v4_pro(&lower) {
        return ModelCapabilities {
            context_length: DEEPSEEK_V4_CONTEXT,
            max_output: DEEPSEEK_V4_MAX_OUTPUT,
            supports_thinking: true,
            is_flash: false,
        };
    }
    if is_deepseek_v4_flash(&lower) {
        return ModelCapabilities {
            context_length: DEEPSEEK_V4_CONTEXT,
            max_output: DEEPSEEK_V4_MAX_OUTPUT,
            supports_thinking: true,
            is_flash: true,
        };
    }
    // DeepSeek V3 (prev gen)
    if lower.contains("deepseek-v3") || lower.contains("deepseek-chat") {
        return ModelCapabilities {
            context_length: 128_000,
            max_output: 8_192,
            supports_thinking: false,
            is_flash: false,
        };
    }
    if lower.contains("deepseek-reasoner") || lower.contains("deepseek-r1") {
        return ModelCapabilities {
            context_length: 128_000,
            max_output: 8_192,
            supports_thinking: true,
            is_flash: false,
        };
    }

    // Claude
    if lower.contains("claude-opus-4") {
        return ModelCapabilities {
            context_length: 200_000,
            max_output: 32_000,
            supports_thinking: true,
            is_flash: false,
        };
    }
    if lower.contains("claude-sonnet-4") {
        return ModelCapabilities {
            context_length: 200_000,
            max_output: 16_000,
            supports_thinking: true,
            is_flash: false,
        };
    }
    if lower.contains("claude-haiku") {
        return ModelCapabilities {
            context_length: 200_000,
            max_output: 8_192,
            supports_thinking: false,
            is_flash: true,
        };
    }

    // GPT
    if lower.contains("gpt-4o") {
        return ModelCapabilities {
            context_length: 128_000,
            max_output: 16_384,
            supports_thinking: false,
            is_flash: false,
        };
    }
    if lower.contains("gpt-4.1") {
        return ModelCapabilities {
            context_length: 1_000_000,
            max_output: 32_768,
            supports_thinking: false,
            is_flash: false,
        };
    }
    if lower.contains("o3") || lower.contains("o4-mini") {
        return ModelCapabilities {
            context_length: 200_000,
            max_output: 100_000,
            supports_thinking: true,
            is_flash: lower.contains("mini"),
        };
    }

    // Gemini
    if lower.contains("gemini-2.5-pro") {
        return ModelCapabilities {
            context_length: 1_000_000,
            max_output: 64_000,
            supports_thinking: true,
            is_flash: false,
        };
    }
    if lower.contains("gemini-2.5-flash") {
        return ModelCapabilities {
            context_length: 1_000_000,
            max_output: 64_000,
            supports_thinking: true,
            is_flash: true,
        };
    }

    // Qwen
    if lower.contains("qwen3") || lower.contains("qwen-3") {
        return ModelCapabilities {
            context_length: 128_000,
            max_output: 8_192,
            supports_thinking: true,
            is_flash: lower.contains("mini") || lower.contains("flash"),
        };
    }

    ModelCapabilities::default()
}

pub fn is_deepseek_v4_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    is_deepseek_v4_pro(&lower) || is_deepseek_v4_flash(&lower)
}

pub fn is_deepseek_v4_pro(model: &str) -> bool {
    model.to_ascii_lowercase().contains("deepseek-v4-pro")
}

pub fn is_deepseek_v4_flash(model: &str) -> bool {
    model.to_ascii_lowercase().contains("deepseek-v4-flash")
}

pub fn deepseek_default_flash_model(model: &str) -> Option<String> {
    if is_deepseek_v4_pro(model) {
        Some("deepseek-v4-flash".to_string())
    } else {
        None
    }
}

/// Apply DeepSeek-V4-specific defaults for CoAI's dedicated agent mode.
///
/// This enforces the agent-oriented defaults that make V4-Pro/Flash behave
/// well out of the box.
pub fn apply_deepseek_v4_profile(config: &mut LLMConfig) {
    if !is_deepseek_v4_model(&config.model) {
        return;
    }

    if config
        .base_url
        .as_ref()
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
    {
        match config.provider {
            LLMProvider::Anthropic => {
                config.base_url = Some("https://api.deepseek.com/anthropic".to_string());
            }
            LLMProvider::OpenAICompatible | LLMProvider::Custom => {
                config.base_url = Some("https://api.deepseek.com/v1".to_string());
            }
            _ => {}
        }
    }

    config.max_tokens = config.max_tokens.max(DEEPSEEK_AGENT_OUTPUT);

    let effort = if is_deepseek_v4_flash(&config.model) {
        "high"
    } else {
        "max"
    };
    match config.provider {
        LLMProvider::Anthropic => {
            config.extra_params.remove("reasoning_effort");
            config.extra_params.insert(
                "output_config".to_string(),
                serde_json::json!({ "effort": effort }),
            );
        }
        LLMProvider::OpenAICompatible | LLMProvider::Custom | LLMProvider::OpenAI => {
            config.extra_params.remove("output_config");
            config
                .extra_params
                .insert("reasoning_effort".to_string(), serde_json::json!(effort));
        }
        LLMProvider::Ollama => {}
    }

    if config.flash_model.is_none() {
        config.flash_model = deepseek_default_flash_model(&config.model);
    }
}

/// Truncation limits derived from model context length.
/// These scale proportionally so that context injection doesn't overwhelm smaller windows.
#[derive(Debug, Clone)]
pub struct TruncationLimits {
    /// Max chars for dependency task output before truncation.
    pub dep_output_chars: usize,
    /// Max chars for injected file content before truncation.
    pub context_file_chars: usize,
    /// Max chars for task description before truncation.
    pub description_chars: usize,
    /// Max chars for live context additions before truncation.
    pub live_context_chars: usize,
}

impl TruncationLimits {
    pub fn from_context_length(context_length: usize) -> Self {
        // Approximate: 1 token ≈ 2 chars (mixed CJK/Latin)
        // Use ~1.5% of context for dep output, ~2% for files, ~3% for description
        let scale = context_length as f64 / 1_000_000.0;
        Self {
            dep_output_chars: (8_000.0 * scale).max(1_500.0).min(20_000.0) as usize,
            context_file_chars: (10_000.0 * scale).max(2_000.0).min(30_000.0) as usize,
            description_chars: (15_000.0 * scale).max(3_000.0).min(30_000.0) as usize,
            live_context_chars: (1_000.0 * scale).max(300.0).min(2_000.0) as usize,
        }
    }
}

/// Generate the decomposition system prompt dynamically based on model capabilities.
/// If has_flash is true, include [L] marker instructions for routing simple tasks to a lighter model.
pub fn decompose_system_prompt(context_length: usize, has_flash: bool) -> String {
    let context_desc = if context_length >= 1_000_000 {
        "1M tokens"
    } else if context_length >= 200_000 {
        "200K tokens"
    } else {
        "128K tokens"
    };

    let split_guidance = if context_length >= 1_000_000 {
        "The vast majority of tasks will not hit the context limit in a single run. \
The only reason to split is: the task has multiple independent branches that can run in parallel, \
or a single run would produce output too long to avoid truncation."
    } else if context_length >= 200_000 {
        "Most standard tasks can be completed in a single run. \
Splitting is appropriate when: the task has multiple independent parallel branches, \
heavy exploration would cause significant context growth, or the output would be too long."
    } else {
        "Context space is limited; split carefully to avoid truncation. \
Split principle: each subtask must be completable within one context window; \
merge related operations and separate independent branches."
    };

    let granularity = if context_length >= 1_000_000 {
        "Granularity guideline — ask yourself: can this be done in a 1M context window?\n\
- Yes → do not split; keep as one item\n\
- Output would be truncated → split by output phase\n\
- Multiple independent parallel branches exist → split by branch and mark with [P]"
    } else if context_length >= 200_000 {
        "Granularity guideline — can this be completed in a single context without overflowing from exploration?\n\
- Yes → do not split\n\
- Heavy exploration required (may consume a lot of context) → split out a dedicated exploration step\n\
- Multiple independent parallel branches exist → split by branch and mark with [P]"
    } else {
        "Granularity guideline — context space is limited; plan carefully:\n\
- Will the exploration required exceed the context? → Yes → split out an exploration step\n\
- Related operations (e.g. read + edit the same area) → merge into one item to avoid redundant reads\n\
- Multiple independent parallel branches exist → split by branch and mark with [P]\n\
- Keep each item's output within a reasonable size to avoid truncation"
    };

    let flash_rule = if has_flash {
        r#"
12. Mark simple tasks with [L] (execution-only tasks that need no complex reasoning or heavy exploration)
   - [L] is appropriate for: directly executing a clear operation, simple format conversion, small single-file edits
   - Do NOT mark [L]: tasks that need analysis and judgment, cross-file exploration, or a design decision"#
    } else {
        ""
    };

    let flash_example = if has_flash {
        r#"
- ✅ [L] "Replace v1.0.0 with v2.0.0 in README.md" → simple substitution, mark [L]
- ✅ "Analyze project dependencies and determine upgrade order" → requires analysis, do not mark [L]"#
    } else {
        ""
    };

    format!(
        r#"You are a task planning expert. Generate a TODO list for the given task.

Available context window: {context_desc}. {split_guidance}

Rules:
1. Each item is a complete functional unit that can be completed independently in a single context run.
2. Related operations must be merged into one item (read + edit + verify belong together); include verification, test updates, and doc updates in the appropriate work unit — do not leave them out of the plan.
3. Only split when:
   - Independent parallel branches exist → split into multiple items marked with [P]
   - A single run's output would exceed the output limit → split by output phase
4. Order items by dependency (later items may depend on the output of earlier ones).
5. Mark parallelizable items with [P].
6. Output a numbered list only, one item per line — no JSON, no explanations.
7. If a task needs specific files as context, append |files:path1,path2 after the description.
8. If a task needs prerequisite information, append |note:info after the description.
9. Mark items that change behavior or add/modify public interfaces with [QA] (signals that tests need to be added or updated).
10. Mark items whose changes affect user-visible behavior or public interfaces with [DOC] (signals that docs need updating).
11. When the same operation repeats in multiple places, first schedule an item to extract a reusable function/abstraction; subsequent items reuse it to avoid duplicated logic.{flash_rule}

{granularity}

Examples:
- ✅ "Find references and compile a summary" → 1 item (context is large enough for exploration + writing)
- ❌ "Find references" + "Compile summary" → do not split (the second item loses the exploration context)
- ✅ "Search source A" + [P] "Search source B" + "Merge results from A and B" → 3 items (independent branches parallelized; merge depends on both)
- ❌ "Read config" + "Edit config" → do not split (read and edit are two steps of the same operation)
- ✅ "Analyze existing structure and draft a migration plan" + "Implement the plan" → 2 items (exploration and implementation are distinct concerns; exploration output feeds implementation)
- ✅ [QA] "Implement and add unit tests covering edge cases and empty input for the new parser function" → behavior change, mark [QA]
- ✅ [DOC] "Update docs and examples after adjusting the public API" → affects public interface, mark [DOC]{flash_example}"#,
        context_desc = context_desc,
        split_guidance = split_guidance,
        flash_rule = flash_rule,
        granularity = granularity,
        flash_example = flash_example,
    )
}

/// Generate the default system prompt for tool_loop based on model capabilities.
pub fn tool_loop_system_prompt(context_length: usize) -> String {
    let context_hint = if context_length >= 1_000_000 {
        "You have an extra-large context window (~1M tokens). Prioritize building complete understanding from the long context; do not ask the user to repeat information prematurely, and do not compress critical history too early."
    } else if context_length >= 200_000 {
        "You have a large context window (200K tokens). Most tasks can be executed directly; be mindful of context growth during heavy exploration."
    } else {
        "Your context window is limited (128K tokens). Merge related operations and avoid excessive exploration that consumes context."
    };
    let workspace_hint = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.display().to_string())
        .unwrap_or_else(|| "current working directory".to_string());

    format!(
        r#"You are an autonomous AI assistant. {context_hint}

## How to work

When you receive a task, plan in your thinking before acting:
1. Understand the goal — break the task into verifiable acceptance criteria (what deliverables, behaviors, and constraints define "done"?). If the goal is unclear, ask directly; do not guess.
2. Plan steps — outline the execution plan in your thinking. For multi-step tasks, use tasks.write to maintain a visible checklist (each item has status: pending/in_progress/completed); mark items in_progress before starting and completed when done, updating as you progress.
3. Announce before acting — before each significant tool call, write a brief visible sentence explaining what you are about to do.
4. Execute incrementally — use tools and verify each step. For code tasks, build a global understanding before editing. Always check your work against the goal from step 1; if you drift, correct immediately — do not expand or shrink scope opportunistically.
5. Confirm completion — before reporting, check each acceptance criterion from step 1: is each one genuinely satisfied? Is anything missing? Are there any changes beyond the stated goal? Fix any shortfall before reporting; only declare done once all changes are verified.

Merge related operations (read + edit + verify in one pass). Prefer file.edit (precise replacement) over file.write to avoid rewriting entire files. file.write requires both path and content.

## Working principles

- Thorough verification: do not only exercise the happy path; cover edge cases, empty inputs, invalid inputs, and error handling, and assess regression risk for existing functionality. Prefer running the project's own build/test/lint. In a monorepo, verify in the actual sub-project directory; exec.* supports a cwd parameter.
- Minimal contract fix: when fixing a bug, do not just make the current example pass; first identify the function/API's input-output contract, then add targeted tests that prove the contract holds.
- Keep the workspace clean: do not leave behind temporary files or build artifacts you created. Before finishing, inspect git.status/git.diff or equivalent workspace state and use cleanup.report to review untracked/ignored entries. Only clean up what you introduced this session; use workspace state to judge rather than assuming a file extension.
- Sound and testable abstractions: prefer injectable/replaceable abstractions for time, I/O, and external dependencies. Extract and reuse repeated logic rather than scattering hardcoded copies.
- Tests and docs in sync: when behavior or public interfaces change, add or update tests and relevant documentation accordingly.

## Information classification and list maintenance

- When maintaining a named list or table the user provided, first confirm the classification boundary from the title, context, and item semantics; only modify items that belong to that list.
- Do not default to inserting a new item into whatever table appeared on the previous screen. Items that clearly belong to a project, repository, open-source work, GitHub, code, or P0/P1 work should not be added to a personal/household list; place them in the matching project category, or ask first if the category is ambiguous.
- Before adding, moving, or deleting a table item, briefly state the classification rationale and target list. If an item was placed in the wrong category, revert it from the wrong category first, then write it to the correct one.

If available, you may use agent.spawn to dispatch clearly-scoped subtasks to subagents. This is well-suited for parallel exploration, independent verification, and narrow-scope implementation; do not delegate overall architecture decisions to subagents. When dispatching, always specify task clearly; provide role and write_scope when needed; you are responsible for synthesizing results and final quality.

## Working directory and path constraints

Current working directory: {workspace_hint}

By default, all file.* tool paths must point to files inside the current working directory. Prefer relative paths the user has mentioned, or choose paths based on files and directories that actually exist in the current working directory; do not invent absolute paths outside it. When the user has not specified an output directory, write new files to the current working directory or a relative subdirectory the user specified.

Only use an external path when the user explicitly asks you to modify a file outside the working directory (e.g., system config, home directory config, or a specified absolute path), and you must wait for the tool confirmation flow. When you encounter a path boundary or permission error, do not switch to an invented path; explain the limitation and ask the user to confirm the correct target path or authorization.

Make full use of the model's long-context capability: retain the task goal, user constraints, key file content, and verification results in context. Only summarize logs, repeated tool output, and low-value intermediate information. Apply thorough reasoning for complex agent scenarios; stay concise and efficient for simple mechanical tasks."#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_prompt_guards_named_list_classification() {
        let prompt = tool_loop_system_prompt(1_000_000);

        assert!(prompt.contains("Information classification and list maintenance"));
        assert!(prompt.contains("personal/household list"));
        assert!(prompt.contains("project, repository, open-source work, GitHub, code"));
        assert!(prompt.contains("ask first if the category is ambiguous"));
    }

    #[test]
    fn deepseek_v4_profile_sets_agent_defaults() {
        let mut cfg = LLMConfig::anthropic("deepseek-v4-pro", "test-key");

        apply_deepseek_v4_profile(&mut cfg);

        assert_eq!(
            cfg.base_url.as_deref(),
            Some("https://api.deepseek.com/anthropic")
        );
        assert_eq!(cfg.max_tokens, 64_000);
        assert_eq!(cfg.flash_model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(
            cfg.extra_params.get("output_config"),
            Some(&serde_json::json!({ "effort": "max" }))
        );
        assert!(cfg.extra_params.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_v4_openai_compatible_keeps_openai_endpoint() {
        let mut cfg =
            LLMConfig::openai_compatible("deepseek-v4-pro", "", Some("test-key".to_string()));

        apply_deepseek_v4_profile(&mut cfg);

        assert_eq!(cfg.base_url.as_deref(), Some("https://api.deepseek.com/v1"));
        assert_eq!(
            cfg.extra_params.get("reasoning_effort"),
            Some(&serde_json::json!("max"))
        );
        assert!(cfg.extra_params.get("output_config").is_none());
    }

    #[test]
    fn deepseek_v4_flash_uses_high_effort() {
        let mut cfg = LLMConfig::anthropic("deepseek-v4-flash", "test-key");

        apply_deepseek_v4_profile(&mut cfg);

        assert!(cfg.flash_model.is_none());
        assert_eq!(
            cfg.extra_params.get("output_config"),
            Some(&serde_json::json!({ "effort": "high" }))
        );
    }

    #[test]
    fn deepseek_v4_flash_overrides_pro_reasoning_after_routing() {
        let mut cfg = LLMConfig::anthropic("deepseek-v4-pro", "test-key");
        apply_deepseek_v4_profile(&mut cfg);

        cfg.model = "deepseek-v4-flash".to_string();
        apply_deepseek_v4_profile(&mut cfg);

        assert_eq!(
            cfg.extra_params.get("output_config"),
            Some(&serde_json::json!({ "effort": "high" }))
        );
    }
}
