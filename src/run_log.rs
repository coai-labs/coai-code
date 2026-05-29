//! Lightweight JSONL run logs for debugging long interactive turns.

use crate::core::{CoAIError, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLogSummary {
    pub id: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunLogger {
    id: String,
    path: Arc<PathBuf>,
}

impl RunLogger {
    pub fn new(workspace: impl AsRef<Path>, description: &str) -> Result<Self> {
        let dir = workspace.as_ref().join(".coai/state/runs");
        fs::create_dir_all(&dir)?;
        let id = format!(
            "{}-{}",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            Uuid::new_v4().simple()
        );
        let path = dir.join(format!("{id}.jsonl"));
        let logger = Self {
            id,
            path: Arc::new(path),
        };
        logger.log(
            "run_started",
            serde_json::json!({
                "description": description,
                "cwd": workspace.as_ref().display().to_string(),
            }),
        )?;
        Ok(logger)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn log<T: Serialize>(&self, event: &str, data: T) -> Result<()> {
        let record = serde_json::json!({
            "ts": Utc::now().to_rfc3339(),
            "run_id": self.id,
            "event": event,
            "data": data,
        });
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.as_ref())?;
        writeln!(file, "{}", serde_json::to_string(&record)?)?;
        Ok(())
    }
}

pub fn list_run_logs(workspace: impl AsRef<Path>, limit: usize) -> Result<Vec<RunLogSummary>> {
    let dir = workspace.as_ref().join(".coai/state/runs");
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut logs = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata
            .modified()
            .ok()
            .map(chrono::DateTime::<Utc>::from)
            .map(|dt| dt.to_rfc3339());
        let id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        logs.push(RunLogSummary {
            id,
            path,
            bytes: metadata.len(),
            modified,
        });
    }
    logs.sort_by(|a, b| b.modified.cmp(&a.modified));
    logs.truncate(limit);
    Ok(logs)
}

pub fn read_run_log(workspace: impl AsRef<Path>, id: &str) -> Result<String> {
    let path = if id.contains(std::path::MAIN_SEPARATOR) || id.ends_with(".jsonl") {
        PathBuf::from(id)
    } else {
        workspace
            .as_ref()
            .join(".coai/state/runs")
            .join(format!("{id}.jsonl"))
    };
    if !path.exists() {
        return Err(CoAIError::Other(format!("Run log not found: {}", id)));
    }
    Ok(fs::read_to_string(path)?)
}

pub fn format_run_log_timeline(raw: &str) -> String {
    let mut lines = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            lines.push(line.to_string());
            continue;
        };
        let ts = value["ts"].as_str().unwrap_or("-");
        let time = ts.get(11..19).unwrap_or(ts);
        let event = value["event"].as_str().unwrap_or("event");
        let data = &value["data"];
        let summary = match event {
            "run_started" => data["description"].as_str().unwrap_or("").to_string(),
            "context_budget" => format!(
                "{}: {} / {} tokens",
                data["label"].as_str().unwrap_or("context"),
                data["estimated_tokens"].as_u64().unwrap_or(0),
                data["context_length"].as_u64().unwrap_or(0)
            ),
            "context_compacted" => format!(
                "{} -> {} messages, {} -> {} tokens",
                data["before_messages"].as_u64().unwrap_or(0),
                data["after_messages"].as_u64().unwrap_or(0),
                data["before_tokens"].as_u64().unwrap_or(0),
                data["after_tokens"].as_u64().unwrap_or(0)
            ),
            "tool_start" => format!(
                "{} {}",
                data["name"].as_str().unwrap_or("tool"),
                data["detail"].as_str().unwrap_or("")
            ),
            "tool_result" => format!(
                "{} {}",
                if data["success"].as_bool().unwrap_or(false) {
                    "ok"
                } else {
                    "failed"
                },
                data["name"].as_str().unwrap_or("tool")
            ),
            "thinking" => compact(data["text"].as_str().unwrap_or(""), 90),
            "text_output" => compact(data["text"].as_str().unwrap_or(""), 90),
            "run_finished" => {
                if data["success"].as_bool().unwrap_or(false) {
                    "success".into()
                } else {
                    format!("failed: {}", data["error"].as_str().unwrap_or(""))
                }
            }
            _ => compact(&data.to_string(), 120),
        };
        lines.push(format!("{time}  {event:<20} {summary}"));
    }
    lines.join("\n")
}

fn compact(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        normalized
    } else {
        format!(
            "{}...",
            normalized.chars().take(max_chars).collect::<String>()
        )
    }
}
