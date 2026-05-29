use crate::core::{CoAIError, Result};
use crate::skills::SkillRegistry;
use std::path::PathBuf;

pub struct SkillTools {
    registry: SkillRegistry,
}

impl SkillTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            registry: SkillRegistry::new(workspace),
        }
    }

    pub async fn list(&self) -> Result<String> {
        let skills = self.registry.list()?;
        serde_json::to_string_pretty(&skills)
            .map_err(|e| CoAIError::Other(format!("Failed to serialize skill list: {}", e)))
    }

    pub async fn search(&self, query: &str) -> Result<String> {
        let skills = self.registry.search(query)?;
        serde_json::to_string_pretty(&skills)
            .map_err(|e| CoAIError::Other(format!("Failed to serialize skill search results: {}", e)))
    }

    pub async fn read(&self, name_or_path: &str) -> Result<String> {
        self.registry.read(name_or_path)
    }
}
