use crate::core::{CoAIError, Result, TaskRecord, ToolCall};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicTask {
    pub id: Uuid,
    pub parent_task_id: Option<Uuid>,
    pub description: String,
    pub status: AtomicTaskStatus,
    pub result: Option<String>,
    pub steps: Vec<AtomicStep>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub priority: u8,
    pub dependencies: Vec<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AtomicTaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Blocked,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicStep {
    pub description: String,
    pub tool_calls: Vec<ToolCall>,
    pub result: Option<String>,
    pub status: AtomicStepStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AtomicStepStatus {
    Pending,
    Running,
    Success,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
struct HistoryStorage {
    tasks: HashMap<Uuid, TaskRecord>,
    atomic_tasks: HashMap<Uuid, AtomicTask>,
}

impl Default for HistoryStorage {
    fn default() -> Self {
        Self {
            tasks: HashMap::new(),
            atomic_tasks: HashMap::new(),
        }
    }
}

pub struct HistoryStore {
    tasks: HashMap<Uuid, TaskRecord>,
    atomic_tasks: HashMap<Uuid, AtomicTask>,
    storage_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct QueryCondition {
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub status: Option<String>,
    pub tags: Vec<String>,
    pub keyword: Option<String>,
}

pub enum ExportFormat {
    Json,
    Markdown,
    Csv,
}

impl HistoryStore {
    pub fn new(storage_path: impl Into<PathBuf>) -> Self {
        let storage_path = storage_path.into();
        let mut store = Self {
            tasks: HashMap::new(),
            atomic_tasks: HashMap::new(),
            storage_path,
        };

        let _ = store.load_from_disk();
        store
    }

    pub fn store(&mut self, record: TaskRecord) -> Result<()> {
        let id = record.id;
        self.tasks.insert(id, record);
        self.save_to_disk()
    }

    pub fn get(&self, id: &Uuid) -> Option<&TaskRecord> {
        self.tasks.get(id)
    }

    pub fn query(&self, condition: QueryCondition) -> Vec<&TaskRecord> {
        let mut records: Vec<_> = self
            .tasks
            .values()
            .filter(|task| {
                if let Some((start, end)) = &condition.time_range {
                    if task.created_at < *start || task.created_at > *end {
                        return false;
                    }
                }

                if let Some(status) = &condition.status {
                    let task_status = format!("{:?}", task.status);
                    if &task_status != status {
                        return false;
                    }
                }

                if !condition.tags.is_empty()
                    && !condition.tags.iter().all(|t| task.tags.contains(t))
                {
                    return false;
                }

                if let Some(keyword) = &condition.keyword {
                    if !task_matches_keyword(task, keyword) {
                        return false;
                    }
                }

                true
            })
            .collect();

        records.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        records
    }

    pub fn list(&self, limit: Option<usize>) -> Vec<&TaskRecord> {
        let mut records: Vec<_> = self.tasks.values().collect();
        records.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        if let Some(limit) = limit {
            records.truncate(limit);
        }

        records
    }

    pub fn search(&self, keyword: &str, limit: Option<usize>) -> Vec<&TaskRecord> {
        let condition = QueryCondition {
            time_range: None,
            status: None,
            tags: Vec::new(),
            keyword: Some(keyword.to_string()),
        };
        let mut records = self.query(condition);
        if let Some(limit) = limit {
            records.truncate(limit);
        }
        records
    }

    pub fn delete(&mut self, id: &Uuid) -> Result<bool> {
        let existed = self.tasks.remove(id).is_some();
        if existed {
            self.save_to_disk()?;
        }
        Ok(existed)
    }

    pub fn export(&self, format: ExportFormat) -> Result<String> {
        let records: Vec<_> = self.tasks.values().collect();

        match format {
            ExportFormat::Json => serde_json::to_string_pretty(&records)
                .map_err(|e| CoAIError::History(format!("JSON export failed: {}", e))),
            ExportFormat::Markdown => {
                let mut md = String::from("# Task History\n\n");

                for record in records {
                    md.push_str(&format!(
                        "## {} - {:?}\n\n**Description**: {}\n\n**Created**: {}\n\n",
                        record.id,
                        record.status,
                        record.description,
                        record.created_at.format("%Y-%m-%d %H:%M:%S")
                    ));

                    if let Some(result) = &record.result {
                        md.push_str(&format!("**Result**: {}\n\n", result));
                    }

                    md.push_str("---\n\n");
                }

                Ok(md)
            }
            ExportFormat::Csv => {
                let mut csv = String::from("id,description,status,created_at,result\n");

                for record in records {
                    csv.push_str(&format!(
                        "{},{},{:?},{},{}\n",
                        record.id,
                        csv_escape(&record.description),
                        record.status,
                        record.created_at.format("%Y-%m-%d %H:%M:%S"),
                        csv_escape(record.result.as_deref().unwrap_or(""))
                    ));
                }

                Ok(csv)
            }
        }
    }

    pub fn count(&self) -> usize {
        self.tasks.len()
    }

    // Atomic Task methods
    pub fn store_atomic_task(&mut self, atomic_task: AtomicTask) -> Result<()> {
        let id = atomic_task.id;
        self.atomic_tasks.insert(id, atomic_task);
        self.save_to_disk()
    }

    pub fn get_atomic_task(&self, id: &Uuid) -> Option<&AtomicTask> {
        self.atomic_tasks.get(id)
    }

    pub fn query_atomic_tasks(&self, parent_task_id: Option<&Uuid>) -> Vec<&AtomicTask> {
        self.atomic_tasks
            .values()
            .filter(|task| match parent_task_id {
                Some(id) => task.parent_task_id.as_ref() == Some(id),
                None => true,
            })
            .collect()
    }

    pub fn update_atomic_task_status(
        &mut self,
        id: &Uuid,
        status: AtomicTaskStatus,
    ) -> Result<bool> {
        if let Some(task) = self.atomic_tasks.get_mut(id) {
            let is_done = matches!(
                status,
                AtomicTaskStatus::Completed | AtomicTaskStatus::Failed
            );
            task.status = status;
            if is_done {
                task.completed_at = Some(Utc::now());
            }
            self.save_to_disk()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn add_atomic_task_result(&mut self, id: &Uuid, result: String) -> Result<bool> {
        if let Some(task) = self.atomic_tasks.get_mut(id) {
            task.result = Some(result);
            self.save_to_disk()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn delete_atomic_task(&mut self, id: &Uuid) -> Result<bool> {
        let existed = self.atomic_tasks.remove(id).is_some();
        if existed {
            self.save_to_disk()?;
        }
        Ok(existed)
    }

    fn save_to_disk(&self) -> Result<()> {
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CoAIError::History(format!("failed to create directory: {}", e)))?;
        }

        let storage = HistoryStorage {
            tasks: self.tasks.clone(),
            atomic_tasks: self.atomic_tasks.clone(),
        };

        let json = serde_json::to_string_pretty(&storage)
            .map_err(|e| CoAIError::History(format!("serialization failed: {}", e)))?;

        fs::write(&self.storage_path, json)
            .map_err(|e| CoAIError::History(format!("failed to write file: {}", e)))?;

        Ok(())
    }

    fn load_from_disk(&mut self) -> Result<()> {
        if !self.storage_path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&self.storage_path)
            .map_err(|e| CoAIError::History(format!("failed to read file: {}", e)))?;

        // Try to load as new format first
        match serde_json::from_str::<HistoryStorage>(&content) {
            Ok(storage) => {
                self.tasks = storage.tasks;
                self.atomic_tasks = storage.atomic_tasks;
                Ok(())
            }
            Err(_) => {
                // Fallback to old format for backward compatibility
                let records: Vec<TaskRecord> = serde_json::from_str(&content).unwrap_or_default();

                for record in records {
                    self.tasks.insert(record.id, record);
                }
                Ok(())
            }
        }
    }
}

impl Default for HistoryStore {
    fn default() -> Self {
        Self::new("./.coai/state/history.json")
    }
}

fn task_matches_keyword(task: &TaskRecord, keyword: &str) -> bool {
    let keyword = keyword.to_lowercase();
    if task.description.to_lowercase().contains(&keyword) {
        return true;
    }
    if task
        .tags
        .iter()
        .any(|tag| tag.to_lowercase().contains(&keyword))
    {
        return true;
    }
    if task
        .result
        .as_deref()
        .map(|result| result.to_lowercase().contains(&keyword))
        .unwrap_or(false)
    {
        return true;
    }
    task.steps.iter().any(|step| {
        step.description.to_lowercase().contains(&keyword)
            || step
                .result
                .as_deref()
                .map(|result| result.to_lowercase().contains(&keyword))
                .unwrap_or(false)
            || step
                .tool_calls
                .iter()
                .any(|call| call.tool.to_lowercase().contains(&keyword))
    })
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::TaskStatus;
    use tempfile::NamedTempFile;

    #[test]
    fn test_atomic_task_storage() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        let mut store = HistoryStore::new(path);

        let atomic_task = AtomicTask {
            id: Uuid::new_v4(),
            parent_task_id: Some(Uuid::new_v4()),
            description: "Test atomic task".to_string(),
            status: AtomicTaskStatus::Pending,
            result: None,
            steps: vec![],
            created_at: Utc::now(),
            completed_at: None,
            priority: 1,
            dependencies: vec![],
            tags: vec!["test".to_string()],
        };

        assert!(store.store_atomic_task(atomic_task.clone()).is_ok());

        let loaded = store.get_atomic_task(&atomic_task.id);
        assert!(loaded.is_some());
        let loaded_task = loaded.unwrap();
        assert_eq!(loaded_task.description, atomic_task.description);
        assert!(matches!(loaded_task.status, AtomicTaskStatus::Pending));

        assert!(store
            .update_atomic_task_status(&atomic_task.id, AtomicTaskStatus::InProgress)
            .unwrap());
        let updated = store.get_atomic_task(&atomic_task.id).unwrap();
        assert!(matches!(updated.status, AtomicTaskStatus::InProgress));

        assert!(store
            .add_atomic_task_result(&atomic_task.id, "Test result".to_string())
            .unwrap());
        let with_result = store.get_atomic_task(&atomic_task.id).unwrap();
        assert_eq!(with_result.result.as_ref().unwrap(), "Test result");

        let tasks = store.query_atomic_tasks(atomic_task.parent_task_id.as_ref());
        assert_eq!(tasks.len(), 1);

        assert!(store.delete_atomic_task(&atomic_task.id).unwrap());
        assert!(store.get_atomic_task(&atomic_task.id).is_none());
    }

    #[test]
    fn test_backward_compatibility() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        let old_data = vec![
            TaskRecord {
                id: Uuid::new_v4(),
                description: "Old task 1".to_string(),
                status: TaskStatus::Completed,
                result: Some("Success".to_string()),
                steps: vec![],
                created_at: Utc::now(),
                completed_at: Some(Utc::now()),
                tags: vec!["old".to_string()],
            },
            TaskRecord {
                id: Uuid::new_v4(),
                description: "Old task 2".to_string(),
                status: TaskStatus::Failed,
                result: Some("Error".to_string()),
                steps: vec![],
                created_at: Utc::now(),
                completed_at: Some(Utc::now()),
                tags: vec!["old".to_string()],
            },
        ];

        let json = serde_json::to_string_pretty(&old_data).unwrap();
        std::fs::write(&path, json).unwrap();

        let store = HistoryStore::new(path.clone());

        assert_eq!(store.tasks.len(), 2);
        assert!(store.atomic_tasks.is_empty());
    }
}
