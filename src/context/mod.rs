use crate::core::{CoAIError, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct ContextManager {
    loaded: HashMap<PathBuf, LoadedContext>,
    window_size: usize,
    current_usage: usize,
    persistence_path: PathBuf,
    conversation_history: Vec<ConversationTurn>,
}

struct LoadedContext {
    content: String,
    token_count: usize,
    loaded_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct ContextStatus {
    pub loaded_files: Vec<String>,
    pub total_tokens: usize,
    pub available_tokens: usize,
    pub usage_percentage: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    pub loaded_files: Vec<String>,
    pub conversation_history: Vec<ConversationTurn>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConversationTurn {
    pub role: String,
    pub content: String,
    pub token_count: usize,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ContextManager {
    pub fn new(window_size: usize, persistence_path: impl Into<PathBuf>) -> Self {
        Self {
            loaded: HashMap::new(),
            window_size,
            current_usage: 0,
            persistence_path: persistence_path.into(),
            conversation_history: Vec::new(),
        }
    }

    pub fn load(&mut self, path: &Path) -> Result<&str> {
        let path_buf = path.to_path_buf();

        // Check if already loaded - return early without holding a reference
        if self.loaded.contains_key(&path_buf) {
            // Use separate statement to avoid borrow conflict
            let content = self.loaded.get(&path_buf).unwrap().content.clone();
            self.loaded.get_mut(&path_buf).unwrap().content = content;
            return Ok(&self.loaded.get(&path_buf).unwrap().content);
        }

        let content = fs::read_to_string(&path_buf)
            .map_err(|e| CoAIError::Context(format!("failed to load context: {}", e)))?;

        let token_count = estimate_tokens(&content);

        if self.current_usage + token_count > self.window_size {
            return Err(CoAIError::Context(format!(
                "context window too small: need {} tokens, {} available",
                token_count,
                self.window_size - self.current_usage
            )));
        }

        self.current_usage += token_count;

        self.loaded.insert(
            path_buf.clone(),
            LoadedContext {
                content,
                token_count,
                loaded_at: chrono::Utc::now(),
            },
        );

        Ok(&self.loaded.get(&path_buf).unwrap().content)
    }

    pub fn release(&mut self, path: &Path) {
        if let Some(ctx) = self.loaded.remove(path) {
            self.current_usage = self.current_usage.saturating_sub(ctx.token_count);
        }
    }

    pub fn release_all(&mut self) {
        let total_file_tokens: usize = self.loaded.values().map(|ctx| ctx.token_count).sum();
        self.current_usage = self.current_usage.saturating_sub(total_file_tokens);
        self.loaded.clear();
    }

    pub fn save(&self) -> Result<StateSnapshot> {
        let snapshot = StateSnapshot {
            loaded_files: self
                .loaded
                .keys()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            conversation_history: self.conversation_history.clone(),
            timestamp: chrono::Utc::now(),
        };

        let snapshot_path = self.persistence_path.join("context_snapshot.json");

        if let Some(parent) = snapshot_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CoAIError::Context(format!("failed to create directory: {}", e)))?;
        }

        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| CoAIError::Context(format!("serialization failed: {}", e)))?;

        fs::write(&snapshot_path, json)
            .map_err(|e| CoAIError::Context(format!("failed to save snapshot: {}", e)))?;

        Ok(snapshot)
    }

    pub fn restore(&mut self, snapshot: &StateSnapshot) -> Result<()> {
        self.release_all();
        self.clear_conversation_history();

        self.conversation_history = snapshot.conversation_history.clone();
        let conversation_tokens: usize = self
            .conversation_history
            .iter()
            .map(|t| t.token_count)
            .sum();
        self.current_usage = conversation_tokens;

        for file in &snapshot.loaded_files {
            let path = PathBuf::from(file);
            self.load(&path)?;
        }

        Ok(())
    }

    pub fn status(&self) -> ContextStatus {
        ContextStatus {
            loaded_files: self
                .loaded
                .keys()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            total_tokens: self.current_usage,
            available_tokens: self.window_size - self.current_usage,
            usage_percentage: self.current_usage as f64 / self.window_size as f64 * 100.0,
        }
    }

    pub fn is_loaded(&self, path: &Path) -> bool {
        self.loaded.contains_key(path)
    }

    pub fn get_loaded_content(&self, path: &Path) -> Option<&str> {
        self.loaded.get(path).map(|ctx| ctx.content.as_str())
    }

    pub fn add_conversation_turn(
        &mut self,
        role: impl Into<String>,
        content: impl Into<String>,
    ) -> Result<()> {
        let role = role.into();
        let content = content.into();
        let token_count = estimate_tokens(&content);

        if self.current_usage + token_count > self.window_size {
            return Err(CoAIError::Context(format!(
                "context window too small: need {} tokens, {} available",
                token_count,
                self.window_size - self.current_usage
            )));
        }

        self.current_usage += token_count;

        let turn = ConversationTurn {
            role,
            content,
            token_count,
            timestamp: chrono::Utc::now(),
        };

        self.conversation_history.push(turn);
        Ok(())
    }

    pub fn get_conversation_history(&self) -> &[ConversationTurn] {
        &self.conversation_history
    }

    pub fn clear_conversation_history(&mut self) {
        let total_conversation_tokens: usize = self
            .conversation_history
            .iter()
            .map(|t| t.token_count)
            .sum();
        self.current_usage = self.current_usage.saturating_sub(total_conversation_tokens);
        self.conversation_history.clear();
    }

    pub fn evict_oldest(&mut self) -> Option<PathBuf> {
        let oldest = self
            .loaded
            .iter()
            .min_by_key(|(_, ctx)| ctx.loaded_at)
            .map(|(path, _)| path.clone());

        if let Some(path) = oldest {
            self.release(&path);
            Some(path)
        } else {
            None
        }
    }

    pub fn evict_low_priority(&mut self) -> Option<PathBuf> {
        self.evict_oldest()
    }

    pub fn context_priority(&self, path: &Path) -> Option<usize> {
        self.loaded.get(path).map(|ctx| {
            let now = chrono::Utc::now();
            let age_seconds = (now - ctx.loaded_at).num_seconds() as usize;

            let recency_score = 1000 / (age_seconds + 1);
            let size_score = 100 / (ctx.token_count + 1);

            recency_score + size_score
        })
    }

}


fn estimate_tokens(text: &str) -> usize {
    let char_count = text.chars().count();
    let byte_count = text.len();

    (char_count + byte_count) / 4
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new(200_000, ".coai/state")
    }
}
