//! Atomic task type for CoAI Code
//!
//! Follows the "trust LLM" principle - simple metadata only, no complex decomposition logic.
//! Atomic tasks are the minimal execution units that can be independently completed.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Status of an atomic task
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AtomicTaskStatus {
    /// Task is pending execution
    Pending,
    /// Task is currently in progress
    InProgress,
    /// Task completed successfully
    Completed,
    /// Task failed
    Failed,
    /// Task was skipped (not executed)
    Skipped,
    /// Task is waiting for dependencies
    Blocked,
}

/// Atomic task - minimal execution unit
/// Follows "trust LLM" principle: simple metadata only, LLM decides decomposition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicTask {
    /// Unique identifier for the task
    pub id: Uuid,

    /// Human-readable description of the task
    pub description: String,

    /// Current status of the task
    pub status: AtomicTaskStatus,

    /// Result output from task execution
    #[serde(default)]
    pub result: Option<String>,

    /// Error message if task failed
    #[serde(default)]
    pub error: Option<String>,

    /// When the task was created
    pub created_at: DateTime<Utc>,

    /// When the task was started (if applicable)
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,

    /// When the task was completed (if applicable)
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,

    /// Estimated effort (in relative units, e.g., story points)
    #[serde(default)]
    pub estimated_effort: Option<u32>,

    /// Actual effort spent (in relative units)
    #[serde(default)]
    pub actual_effort: Option<u32>,

    /// Task tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,

    /// Arbitrary metadata (LLM can store anything here)
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl Default for AtomicTask {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            description: String::new(),
            status: AtomicTaskStatus::Pending,
            result: None,
            error: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            estimated_effort: None,
            actual_effort: None,
            tags: Vec::new(),
            metadata: serde_json::Value::Null,
        }
    }
}

impl AtomicTask {
    /// Create a new atomic task with description
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            ..Default::default()
        }
    }

    /// Start the task (set status to InProgress and started_at)
    pub fn start(&mut self) {
        self.status = AtomicTaskStatus::InProgress;
        self.started_at = Some(Utc::now());
    }

    /// Complete the task successfully with a result
    pub fn complete(&mut self, result: impl Into<String>) {
        self.status = AtomicTaskStatus::Completed;
        self.result = Some(result.into());
        self.completed_at = Some(Utc::now());
    }

    /// Fail the task with an error message
    pub fn fail(&mut self, error: impl Into<String>) {
        self.status = AtomicTaskStatus::Failed;
        self.error = Some(error.into());
        self.completed_at = Some(Utc::now());
    }

    /// Skip the task (mark as not executed)
    pub fn skip(&mut self) {
        self.status = AtomicTaskStatus::Skipped;
        self.completed_at = Some(Utc::now());
    }

    /// Check if task is completed (successfully or not)
    pub fn is_done(&self) -> bool {
        matches!(
            self.status,
            AtomicTaskStatus::Completed | AtomicTaskStatus::Failed | AtomicTaskStatus::Skipped
        )
    }

    /// Get task duration if completed
    pub fn duration(&self) -> Option<std::time::Duration> {
        match (self.started_at, self.completed_at) {
            (Some(start), Some(end)) => Some((end - start).to_std().unwrap_or_default()),
            _ => None,
        }
    }
}

/// Collection of atomic tasks
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AtomicTaskList {
    /// List of atomic tasks
    pub tasks: Vec<AtomicTask>,

    /// Total estimated effort
    #[serde(default)]
    pub total_estimated_effort: Option<u32>,

    /// Total actual effort spent
    #[serde(default)]
    pub total_actual_effort: Option<u32>,

    /// When this task list was created
    pub created_at: DateTime<Utc>,
}

impl AtomicTaskList {
    /// Create a new task list
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            total_estimated_effort: None,
            total_actual_effort: None,
            created_at: Utc::now(),
        }
    }

    /// Add a task to the list
    pub fn add(&mut self, task: AtomicTask) {
        self.tasks.push(task);
        self.update_totals();
    }

    /// Get task by ID
    pub fn get(&self, id: &Uuid) -> Option<&AtomicTask> {
        self.tasks.iter().find(|t| &t.id == id)
    }

    /// Get mutable task by ID
    pub fn get_mut(&mut self, id: &Uuid) -> Option<&mut AtomicTask> {
        self.tasks.iter_mut().find(|t| &t.id == id)
    }

    fn update_totals(&mut self) {
        let total_estimated: u32 = self.tasks.iter().filter_map(|t| t.estimated_effort).sum();

        let total_actual: u32 = self.tasks.iter().filter_map(|t| t.actual_effort).sum();

        self.total_estimated_effort = if total_estimated > 0 {
            Some(total_estimated)
        } else {
            None
        };

        self.total_actual_effort = if total_actual > 0 {
            Some(total_actual)
        } else {
            None
        };
    }

    /// Get completion percentage (0-100)
    pub fn completion_percentage(&self) -> f32 {
        if self.tasks.is_empty() {
            return 0.0;
        }

        let done_count = self.tasks.iter().filter(|t| t.is_done()).count();
        (done_count as f32 / self.tasks.len() as f32) * 100.0
    }

    /// Check if all tasks are done
    pub fn is_all_done(&self) -> bool {
        self.tasks.iter().all(|t| t.is_done())
    }
}
