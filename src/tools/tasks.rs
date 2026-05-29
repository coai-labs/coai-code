//! Task list (todo) tool.
//!
//! Lets the agent maintain a visible checklist for multi-step work. Each
//! `tasks.write` call replaces the whole list (like a todo board); the list is
//! persisted so it survives across tool calls and can be shown in the UI.

use crate::core::{CoAIError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[serde(alias = "todo", alias = "open", alias = "not_started")]
    Pending,
    #[serde(
        alias = "in-progress",
        alias = "inprogress",
        alias = "active",
        alias = "doing",
        alias = "running",
        alias = "started"
    )]
    InProgress,
    #[serde(alias = "done", alias = "complete", alias = "finished")]
    Completed,
}

impl TaskStatus {
    pub fn marker(&self) -> &'static str {
        match self {
            TaskStatus::Completed => "☑",
            TaskStatus::InProgress => "▶",
            TaskStatus::Pending => "☐",
        }
    }
}

fn default_status() -> TaskStatus {
    TaskStatus::Pending
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskItem {
    pub content: String,
    #[serde(default = "default_status")]
    pub status: TaskStatus,
}

pub struct TaskTools {
    path: PathBuf,
}

impl TaskTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            path: workspace.into().join(".coai/state/tasks.json"),
        }
    }

    /// Replace the whole task list and return the rendered checklist.
    pub fn write(&self, tasks: Vec<TaskItem>) -> Result<String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoAIError::File(format!("failed to create task directory: {}", e)))?;
        }
        let json = serde_json::to_string_pretty(&tasks)
            .map_err(|e| CoAIError::Other(format!("failed to serialize tasks: {}", e)))?;
        std::fs::write(&self.path, json)
            .map_err(|e| CoAIError::File(format!("failed to write tasks: {}", e)))?;
        Ok(render_checklist(&tasks))
    }

    pub fn read(&self) -> Result<String> {
        Ok(render_checklist(&self.load()))
    }

    pub fn load(&self) -> Vec<TaskItem> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
}

/// Render a task list as a checkbox checklist with a completed/total header.
pub fn render_checklist(tasks: &[TaskItem]) -> String {
    if tasks.is_empty() {
        return "Task list is empty".to_string();
    }
    let done = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Completed)
        .count();
    let mut out = format!("Tasks ({}/{} done)\n", done, tasks.len());
    for task in tasks {
        out.push_str(&format!(
            "{} {}\n",
            task.status.marker(),
            task.content.trim()
        ));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_checklist_with_markers_and_progress() {
        let tasks = vec![
            TaskItem {
                content: "analyze".into(),
                status: TaskStatus::Completed,
            },
            TaskItem {
                content: "implement".into(),
                status: TaskStatus::InProgress,
            },
            TaskItem {
                content: "verify".into(),
                status: TaskStatus::Pending,
            },
        ];
        let rendered = render_checklist(&tasks);
        assert!(rendered.contains("1/3 done"));
        assert!(rendered.contains("☑ analyze"));
        assert!(rendered.contains("▶ implement"));
        assert!(rendered.contains("☐ verify"));
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let tools = TaskTools::new(dir.path());
        tools
            .write(vec![TaskItem {
                content: "step one".into(),
                status: TaskStatus::InProgress,
            }])
            .unwrap();
        let read = tools.read().unwrap();
        assert!(read.contains("▶ step one"));
        assert!(read.contains("0/1 done"));
    }
}
