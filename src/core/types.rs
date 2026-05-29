use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: Uuid,
    pub description: String,
    pub status: TaskStatus,
    pub result: Option<String>,
    pub steps: Vec<Step>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Paused,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub description: String,
    pub tool_calls: Vec<ToolCall>,
    pub result: Option<String>,
    pub status: StepStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Success,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub context_impact: Option<ContextImpact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextImpact {
    pub added_tokens: usize,
    pub current_usage: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub file: String,
    pub line: usize,
    pub content: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub passed: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Default for TaskRecord {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            description: String::new(),
            status: TaskStatus::Pending,
            result: None,
            steps: Vec::new(),
            created_at: Utc::now(),
            completed_at: None,
            tags: Vec::new(),
        }
    }
}
