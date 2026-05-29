use crate::core::{CoAIError, Result, TaskRecord};
use crate::history::{ExportFormat, HistoryStore, QueryCondition};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub struct HistoryTools {
    storage_path: PathBuf,
}

impl HistoryTools {
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            storage_path: workspace.as_ref().join(".coai/state/history.json"),
        }
    }

    pub async fn list(&self, limit: Option<usize>) -> Result<String> {
        let store = HistoryStore::new(&self.storage_path);
        let records = store.list(limit);
        serialize_records(&records)
    }

    pub async fn search(
        &self,
        query: &str,
        limit: Option<usize>,
        status: Option<&str>,
        tag: Option<&str>,
    ) -> Result<String> {
        let store = HistoryStore::new(&self.storage_path);
        let records = store.query(QueryCondition {
            time_range: None,
            status: status.map(status_to_task_status_name),
            tags: tag
                .map(|value| {
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            keyword: if query.trim().is_empty() {
                None
            } else {
                Some(query.to_string())
            },
        });
        let records: Vec<&TaskRecord> = records.into_iter().take(limit.unwrap_or(20)).collect();
        serialize_records(&records)
    }

    pub async fn show(&self, id: &str) -> Result<String> {
        let id =
            Uuid::parse_str(id).map_err(|_| CoAIError::Other(format!("Invalid history record ID: {}", id)))?;
        let store = HistoryStore::new(&self.storage_path);
        let record = store
            .get(&id)
            .ok_or_else(|| CoAIError::Other(format!("History record not found: {}", id)))?;
        serde_json::to_string_pretty(record)
            .map_err(|e| CoAIError::History(format!("Failed to serialize history record: {}", e)))
    }

    pub async fn export(&self, format: ExportFormat) -> Result<String> {
        let store = HistoryStore::new(&self.storage_path);
        store.export(format)
    }

    pub async fn stats(&self) -> Result<String> {
        let store = HistoryStore::new(&self.storage_path);
        let records = store.list(None);
        let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_tag: BTreeMap<String, usize> = BTreeMap::new();

        for record in &records {
            *by_status.entry(format!("{:?}", record.status)).or_default() += 1;
            for tag in &record.tags {
                *by_tag.entry(tag.clone()).or_default() += 1;
            }
        }

        serde_json::to_string_pretty(&serde_json::json!({
            "total": records.len(),
            "by_status": by_status,
            "by_tag": by_tag,
        }))
        .map_err(|e| CoAIError::History(format!("Failed to serialize history statistics: {}", e)))
    }

    pub async fn delete(&self, id: &str) -> Result<String> {
        let id =
            Uuid::parse_str(id).map_err(|_| CoAIError::Other(format!("Invalid history record ID: {}", id)))?;
        let mut store = HistoryStore::new(&self.storage_path);
        let deleted = store.delete(&id)?;
        serde_json::to_string_pretty(&serde_json::json!({
            "id": id,
            "deleted": deleted,
        }))
        .map_err(|e| CoAIError::History(format!("Failed to serialize delete result: {}", e)))
    }
}

fn serialize_records(records: &[&TaskRecord]) -> Result<String> {
    serde_json::to_string_pretty(records)
        .map_err(|e| CoAIError::History(format!("Failed to serialize history record: {}", e)))
}

fn status_to_task_status_name(status: &str) -> String {
    let normalized = status.to_ascii_lowercase();
    match normalized.as_str() {
        "pending" => "Pending",
        "in_progress" | "inprogress" | "running" => "InProgress",
        "completed" | "success" | "done" => "Completed",
        "failed" | "failure" | "error" => "Failed",
        "paused" => "Paused",
        "cancelled" | "canceled" => "Cancelled",
        _ => status,
    }
    .to_string()
}
