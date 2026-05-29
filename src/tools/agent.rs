use crate::core::{CoAIError, Result};
use crate::llm::{create_client, LLMConfig, ToolCallLoop};
use crate::tools::registry::{ToolProgressCallback, ToolProgressEvent};
use crate::tools::ToolRegistry;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    pub role: String,
    pub task: String,
    pub output: String,
}

pub struct AgentTools {
    workspace: PathBuf,
    llm_config: LLMConfig,
    allow_external_mutations: bool,
    progress_callback: Option<ToolProgressCallback>,
}

impl AgentTools {
    pub fn new(
        workspace: impl Into<PathBuf>,
        llm_config: LLMConfig,
        allow_external_mutations: bool,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            llm_config,
            allow_external_mutations,
            progress_callback: None,
        }
    }

    pub fn with_progress_callback(mut self, callback: ToolProgressCallback) -> Self {
        self.progress_callback = Some(callback);
        self
    }

    pub async fn spawn(
        &self,
        task: &str,
        role: Option<&str>,
        write_scope: Option<&str>,
    ) -> Result<SubagentResult> {
        let role = role.unwrap_or("explorer").trim();
        let mut config = self.llm_config.clone();
        if let Some(flash_model) = config.flash_model.clone() {
            config.model = flash_model;
        }

        let client = create_client(config.clone())?;
        let tools = ToolRegistry::new(&self.workspace)
            .with_external_mutations(self.allow_external_mutations)
            .with_agent_tools_enabled(false);

        let system_prompt = subagent_system_prompt(role, write_scope);
        let mut tool_loop = ToolCallLoop::new(client, tools)
            .with_model(&config.model)
            .with_system_prompt(system_prompt)
            .with_max_iterations(20);

        let progress_callback = self.progress_callback.clone();
        let role_label = role.to_string();
        let output = tool_loop
            .run(task, move |event| {
                let Some(callback) = progress_callback.as_ref() else {
                    return;
                };

                match event {
                    crate::llm::tool_loop::LoopEvent::ToolStart { name, detail, .. } => {
                        callback(ToolProgressEvent::ToolStart {
                            name: format!("subagent {} {}", role_label, name),
                            detail,
                        });
                    }
                    crate::llm::tool_loop::LoopEvent::ToolOutput { name, result } => {
                        callback(ToolProgressEvent::ToolOutput {
                            name: format!("subagent {} {}", role_label, name),
                            success: result.success,
                            preview: subagent_tool_preview(&result),
                        });
                    }
                    crate::llm::tool_loop::LoopEvent::Error(message) => {
                        callback(ToolProgressEvent::ToolOutput {
                            name: format!("subagent {}", role_label),
                            success: false,
                            preview: message,
                        });
                    }
                    _ => {}
                }
            })
            .await
            .map_err(|e| CoAIError::Other(format!("subagent execution failed: {}", e)))?;

        Ok(SubagentResult {
            role: role.to_string(),
            task: task.to_string(),
            output,
        })
    }
}

fn subagent_tool_preview(result: &crate::core::ToolResult) -> String {
    if result.success {
        String::new()
    } else {
        result
            .error
            .clone()
            .or_else(|| result.output.as_ref().map(|v| v.to_string()))
            .unwrap_or_else(|| "tool execution failed".to_string())
    }
}

fn subagent_system_prompt(role: &str, write_scope: Option<&str>) -> String {
    let write_scope = write_scope
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("read-only exploration; do not modify files unless the task explicitly requires it and the scope is clear");

    format!(
        r#"You are a CoAI subagent responsible for completing a well-scoped subtask delegated by the primary agent.

Role: {role}
Write scope: {write_scope}

Working rules:
- Handle only the current subtask; do not expand the objective.
- Prioritise reading and verifying facts; conclusions must reference specific files, commands, or observations.
- If file modifications are needed, stay within the write scope and list every modified file in the final reply.
- Before finishing, run targeted verification; in a monorepo use the exec.* cwd parameter to run inside the real sub-project directory.
- Before finishing, inspect git.status/git.diff or equivalent workspace state and use cleanup.report to review untracked/ignored entries; only clean up temporary artefacts you introduced this session.
- When fixing a bug, first confirm the minimal input/output/error-handling contract — do not just make the current example pass.
- Do not spawn new subagents.
- Final reply must include: conclusion, files affected, verification commands, cleanup/workspace state, risks or unverified points."#
    )
}
