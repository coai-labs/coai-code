use crate::context::ContextManager;
use crate::core::types::{Step, StepStatus, TaskRecord, TaskStatus, ToolCall};
use crate::core::Result;
use crate::history::HistoryStore;
use crate::llm::{create_client, LLMConfig, ToolCallLoop};
use crate::tools::ToolRegistry;
use chrono::Utc;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Configuration for the execution engine
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Maximum execution time in seconds
    pub timeout_secs: Option<u64>,
    /// Maximum number of retries for failed tasks
    pub max_retries: u32,
    /// Enable verbose logging
    pub verbose: bool,
    /// Custom system prompt for LLM
    pub system_prompt: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            timeout_secs: Some(300),
            max_retries: 3,
            verbose: true,
            system_prompt: None,
        }
    }
}

/// Result of a task execution
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Whether the execution was successful
    pub success: bool,
    /// Output from the execution
    pub output: String,
    /// Number of tool calls made during execution
    pub tool_calls_made: usize,
    /// Duration of the execution
    pub duration: Duration,
    /// Token usage statistics if available
    pub token_usage: Option<TokenUsage>,
}

/// Token usage statistics
#[derive(Debug, Clone)]
pub struct TokenUsage {
    /// Input tokens used
    pub input_tokens: u64,
    /// Output tokens used
    pub output_tokens: u64,
    /// Total tokens used
    pub total_tokens: u64,
}

/// Execution engine for orchestrating task execution with LLM integration
pub struct ExecutionEngine {
    /// LLM configuration
    llm_config: LLMConfig,
    /// Tool registry for executing tools
    tools: ToolRegistry,
    /// Context manager for maintaining execution context
    #[allow(dead_code)]
    context: Arc<RwLock<ContextManager>>,
    /// History store for recording task executions
    history: Arc<RwLock<HistoryStore>>,
    /// Workspace directory
    #[allow(dead_code)]
    workspace: PathBuf,
    /// Engine configuration
    config: EngineConfig,
}

impl ExecutionEngine {
    /// Create a new execution engine from LLM configuration
    pub fn new(llm_config: LLMConfig) -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            tools: ToolRegistry::new(&workspace).with_llm_config(llm_config.clone()),
            context: Arc::new(RwLock::new(ContextManager::new(
                200_000,
                workspace.join(".coai/state"),
            ))),
            history: Arc::new(RwLock::new(HistoryStore::new(
                workspace.join("./.coai/state/history.json"),
            ))),
            workspace,
            llm_config,
            config: EngineConfig::default(),
        }
    }

    /// Create a new execution engine with custom configuration
    pub fn with_config(llm_config: LLMConfig, config: EngineConfig) -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            tools: ToolRegistry::new(&workspace).with_llm_config(llm_config.clone()),
            context: Arc::new(RwLock::new(ContextManager::new(
                200_000,
                workspace.join(".coai/state"),
            ))),
            history: Arc::new(RwLock::new(HistoryStore::new(
                workspace.join("./.coai/state/history.json"),
            ))),
            workspace,
            llm_config,
            config,
        }
    }

    /// Execute a single task end-to-end
    pub async fn execute(&self, task_description: &str) -> Result<ExecutionResult> {
        let start_time = Instant::now();

        let mut task_record = TaskRecord {
            description: task_description.to_string(),
            status: TaskStatus::InProgress,
            created_at: Utc::now(),
            ..TaskRecord::default()
        };

        self.history.write().await.store(task_record.clone())?;

        if self.config.verbose {
            println!("[Engine] Starting task: {}", task_description);
        }

        let result = self
            .execute_with_llm(task_description, &mut task_record)
            .await;

        let _duration = start_time.elapsed();

        match &result {
            Ok(exec_result) => {
                task_record.status = TaskStatus::Completed;
                task_record.result = Some(exec_result.output.clone());
            }
            Err(e) => {
                task_record.status = TaskStatus::Failed;
                task_record.result = Some(format!("Execution failed: {}", e));
            }
        }
        task_record.completed_at = Some(Utc::now());

        self.history.write().await.store(task_record.clone())?;

        match result {
            Ok(exec_result) => Ok(exec_result),
            Err(e) => Err(e),
        }
    }

    /// Execute a single step within a task
    pub async fn execute_step(&self, step_description: &str) -> Result<ExecutionResult> {
        let start_time = Instant::now();

        if self.config.verbose {
            println!("[Engine] Executing step: {}", step_description);
        }

        let mut step_record = Step {
            description: step_description.to_string(),
            tool_calls: Vec::new(),
            result: None,
            status: StepStatus::Running,
        };

        let result = self
            .execute_single_step(step_description, &mut step_record)
            .await;

        let _duration = start_time.elapsed();

        match &result {
            Ok(exec_result) => {
                step_record.status = StepStatus::Success;
                step_record.result = Some(exec_result.output.clone());
            }
            Err(e) => {
                step_record.status = StepStatus::Failed;
                step_record.result = Some(format!("Step execution failed: {}", e));
            }
        }

        match result {
            Ok(exec_result) => Ok(exec_result),
            Err(e) => Err(e),
        }
    }

    /// Run validation tools on the result
    pub async fn validate(&self, result: &ExecutionResult) -> Result<bool> {
        if self.config.verbose {
            println!("[Engine] Validating execution result...");
        }

        let output_lower = result.output.to_lowercase();
        let has_errors = output_lower.contains("error")
            || output_lower.contains("failed")
            || output_lower.contains("exception");

        Ok(!has_errors)
    }

    /// Retry a task with error context
    pub async fn retry(
        &self,
        task_description: &str,
        previous_error: &str,
    ) -> Result<ExecutionResult> {
        let enhanced_description = format!(
            "{} (previous failure reason: {}) Please fix the issue and retry.",
            task_description, previous_error
        );

        if self.config.verbose {
            println!(
                "[Engine] Retrying task with error context: {}",
                previous_error
            );
        }

        self.execute(&enhanced_description).await
    }

    /// Internal method to execute a task with LLM
    async fn execute_with_llm(
        &self,
        description: &str,
        task_record: &mut TaskRecord,
    ) -> Result<ExecutionResult> {
        let start_time = Instant::now();
        let mut tool_calls_made = 0;

        let client = create_client(self.llm_config.clone())?;

        let mut tool_loop =
            ToolCallLoop::new(client, self.tools.clone()).with_model(&self.llm_config.model);

        if let Some(prompt) = &self.config.system_prompt {
            tool_loop = tool_loop.with_system_prompt(prompt);
        }

        let result = tool_loop
            .run(description, |event| match event {
                crate::llm::tool_loop::LoopEvent::Reasoning(text)
                | crate::llm::tool_loop::LoopEvent::TextOutput(text) => {
                    if self.config.verbose {
                        print!("{}", text);
                        use std::io::Write;
                        std::io::stdout().flush().ok();
                    }
                }
                crate::llm::tool_loop::LoopEvent::ToolStart { name, id, .. } => {
                    if self.config.verbose {
                        println!("\n[Engine] Executing tool: {} (id: {})", name, id);
                    }

                    let step = Step {
                        description: format!("Executing tool: {}", name),
                        tool_calls: vec![ToolCall {
                            tool: name.clone(),
                            params: serde_json::Value::Null,
                        }],
                        result: None,
                        status: StepStatus::Running,
                    };
                    task_record.steps.push(step);
                    tool_calls_made += 1;
                }
                crate::llm::tool_loop::LoopEvent::ToolOutput {
                    name,
                    result: tool_result,
                } => {
                    if self.config.verbose {
                        println!(
                            "[Engine] Tool result: {} - {}",
                            name,
                            if tool_result.success {
                                "success"
                            } else {
                                "failed"
                            }
                        );
                    }

                    if let Some(last_step) = task_record.steps.last_mut() {
                        last_step.result = Some(format!(
                            "Tool {} {}",
                            name,
                            if tool_result.success {
                                "succeeded"
                            } else {
                                "failed"
                            }
                        ));
                        last_step.status = if tool_result.success {
                            StepStatus::Success
                        } else {
                            StepStatus::Failed
                        };
                    }
                }
                crate::llm::tool_loop::LoopEvent::LiveContextApplied { .. }
                | crate::llm::tool_loop::LoopEvent::MessagesCheckpoint(_) => {}
                crate::llm::tool_loop::LoopEvent::Response(text) => {
                    if self.config.verbose {
                        println!("\n[Engine] Task completed:\n{}", text);
                    }
                }
                crate::llm::tool_loop::LoopEvent::Error(e) => {
                    if self.config.verbose {
                        eprintln!("\n[Engine] Error: {}", e);
                    }
                }
            })
            .await?;

        let duration = start_time.elapsed();

        Ok(ExecutionResult {
            success: true,
            output: result,
            tool_calls_made,
            duration,
            token_usage: None,
        })
    }

    /// Internal method to execute a single step
    async fn execute_single_step(
        &self,
        description: &str,
        step_record: &mut Step,
    ) -> Result<ExecutionResult> {
        let start_time = Instant::now();
        let mut tool_calls_made = 0;

        let client = create_client(self.llm_config.clone())?;

        let system_prompt = format!(
            "Execute only this single step: {}\n\nUse only the tools necessary to complete this step; do not perform any extra operations.",
            description
        );

        let mut tool_loop = ToolCallLoop::new(client, self.tools.clone())
            .with_model(&self.llm_config.model)
            .with_system_prompt(&system_prompt);

        let result = tool_loop
            .run(description, |event| match event {
                crate::llm::tool_loop::LoopEvent::Reasoning(text)
                | crate::llm::tool_loop::LoopEvent::TextOutput(text) => {
                    if self.config.verbose {
                        print!("{}", text);
                        use std::io::Write;
                        std::io::stdout().flush().ok();
                    }
                }
                crate::llm::tool_loop::LoopEvent::ToolStart { name, id, .. } => {
                    if self.config.verbose {
                        println!("\n[Engine] Executing step tool: {} (id: {})", name, id);
                    }

                    step_record.tool_calls.push(ToolCall {
                        tool: name.clone(),
                        params: serde_json::Value::Null,
                    });
                    tool_calls_made += 1;
                }
                crate::llm::tool_loop::LoopEvent::ToolOutput {
                    name,
                    result: tool_result,
                } => {
                    if self.config.verbose {
                        println!(
                            "[Engine] Step tool result: {} - {}",
                            name,
                            if tool_result.success {
                                "success"
                            } else {
                                "failed"
                            }
                        );
                    }
                }
                crate::llm::tool_loop::LoopEvent::LiveContextApplied { .. }
                | crate::llm::tool_loop::LoopEvent::MessagesCheckpoint(_) => {}
                crate::llm::tool_loop::LoopEvent::Response(text) => {
                    if self.config.verbose {
                        println!("\n[Engine] Step completed:\n{}", text);
                    }
                }
                crate::llm::tool_loop::LoopEvent::Error(e) => {
                    if self.config.verbose {
                        eprintln!("\n[Engine] Step error: {}", e);
                    }
                }
            })
            .await?;

        let duration = start_time.elapsed();

        Ok(ExecutionResult {
            success: true,
            output: result,
            tool_calls_made,
            duration,
            token_usage: None,
        })
    }
}
