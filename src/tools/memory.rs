use crate::core::{CoAIError, Result};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

const MEMORY_PATH: &str = ".coai/memory.md";
const DEFAULT_MEMORY: &str = "# CoAI Project Memory\n\nExplicit project memory. Records stable facts, user preferences, common commands, and known pitfalls. The system only stores and queries this content; it does not auto-recommend.\n\n";

pub struct MemoryTools {
    workspace: PathBuf,
}

impl MemoryTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    pub async fn read(&self) -> Result<String> {
        let path = self.memory_path();
        if !path.exists() {
            return Ok(DEFAULT_MEMORY.to_string());
        }
        fs::read_to_string(&path).map_err(|e| CoAIError::File(format!("Failed to read project memory: {}", e)))
    }

    pub async fn search(&self, query: &str) -> Result<String> {
        let content = self.read().await?;
        let query = query.to_lowercase();
        let matches: Vec<_> = content
            .lines()
            .enumerate()
            .filter(|(_, line)| line.to_lowercase().contains(&query))
            .map(|(idx, line)| {
                serde_json::json!({
                    "line": idx + 1,
                    "content": line,
                })
            })
            .collect();

        serde_json::to_string_pretty(&matches)
            .map_err(|e| CoAIError::Other(format!("Failed to serialize memory search results: {}", e)))
    }

    pub async fn append(&self, content: &str, section: Option<&str>) -> Result<String> {
        let path = self.memory_path();
        ensure_parent(&path)?;

        let mut existing = if path.exists() {
            fs::read_to_string(&path)
                .map_err(|e| CoAIError::File(format!("Failed to read project memory: {}", e)))?
        } else {
            DEFAULT_MEMORY.to_string()
        };

        if !existing.ends_with('\n') {
            existing.push('\n');
        }

        let section = section.unwrap_or("Notes").trim();
        if !section.is_empty() && !contains_heading(&existing, section) {
            existing.push_str(&format!("\n## {}\n", section));
        }

        let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        existing.push_str(&format!("- [{}] {}\n", timestamp, content.trim()));

        fs::write(&path, existing)
            .map_err(|e| CoAIError::File(format!("Failed to write project memory: {}", e)))?;

        Ok(format!("Appended to {}", MEMORY_PATH))
    }

    pub async fn sections(&self) -> Result<String> {
        let content = self.read().await?;
        let sections: Vec<_> = content
            .lines()
            .enumerate()
            .filter_map(|(idx, line)| {
                let trimmed = line.trim_start();
                if !trimmed.starts_with('#') {
                    return None;
                }
                let level = trimmed.chars().take_while(|c| *c == '#').count();
                let title = trimmed[level..].trim();
                if title.is_empty() {
                    return None;
                }
                Some(serde_json::json!({
                    "line": idx + 1,
                    "level": level,
                    "title": title,
                }))
            })
            .collect();

        serde_json::to_string_pretty(&sections)
            .map_err(|e| CoAIError::Other(format!("Failed to serialize memory sections: {}", e)))
    }

    pub async fn delete_line(&self, line: usize) -> Result<String> {
        if line == 0 {
            return Err(CoAIError::Other("line must be 1-indexed".to_string()));
        }

        let path = self.memory_path();
        let content = self.read().await?;
        let mut lines: Vec<&str> = content.lines().collect();
        if line > lines.len() {
            return Err(CoAIError::Other(format!(
                "line out of range: {}, file has {} lines",
                line,
                lines.len()
            )));
        }

        lines.remove(line - 1);
        ensure_parent(&path)?;
        fs::write(&path, format!("{}\n", lines.join("\n")))
            .map_err(|e| CoAIError::File(format!("Failed to write project memory: {}", e)))?;
        Ok(format!("Deleted line {} from {}", line, MEMORY_PATH))
    }

    pub async fn delete_section(&self, section: &str) -> Result<String> {
        let section = section.trim();
        if section.is_empty() {
            return Err(CoAIError::Other("section cannot be empty".to_string()));
        }

        let path = self.memory_path();
        let content = self.read().await?;
        let lines: Vec<&str> = content.lines().collect();
        let Some((start, level)) = lines.iter().enumerate().find_map(|(idx, line)| {
            let (heading_level, title) = parse_heading(line)?;
            if title.eq_ignore_ascii_case(section) {
                Some((idx, heading_level))
            } else {
                None
            }
        }) else {
            return Err(CoAIError::Other(format!("Memory section not found: {}", section)));
        };

        let end = lines
            .iter()
            .enumerate()
            .skip(start + 1)
            .find_map(|(idx, line)| {
                let (heading_level, _) = parse_heading(line)?;
                (heading_level <= level).then_some(idx)
            })
            .unwrap_or(lines.len());

        let mut updated = Vec::new();
        updated.extend_from_slice(&lines[..start]);
        updated.extend_from_slice(&lines[end..]);
        ensure_parent(&path)?;
        fs::write(&path, format!("{}\n", updated.join("\n")))
            .map_err(|e| CoAIError::File(format!("Failed to write project memory: {}", e)))?;
        Ok(format!("Deleted section {} from {}", section, MEMORY_PATH))
    }

    pub async fn edit_path(&self) -> Result<String> {
        let path = self.memory_path();
        ensure_parent(&path)?;
        if !path.exists() {
            fs::write(&path, DEFAULT_MEMORY)
                .map_err(|e| CoAIError::File(format!("Failed to create project memory file: {}", e)))?;
        }
        Ok(path.display().to_string())
    }

    pub async fn write(&self, content: &str) -> Result<String> {
        let path = self.memory_path();
        ensure_parent(&path)?;
        fs::write(&path, content)
            .map_err(|e| CoAIError::File(format!("Failed to write project memory: {}", e)))?;
        Ok(format!("Updated {}", MEMORY_PATH))
    }

    pub async fn clear(&self) -> Result<String> {
        let path = self.memory_path();
        ensure_parent(&path)?;
        fs::write(&path, DEFAULT_MEMORY)
            .map_err(|e| CoAIError::File(format!("Failed to clear project memory: {}", e)))?;
        Ok(format!("Reset {}", MEMORY_PATH))
    }

    fn memory_path(&self) -> PathBuf {
        self.workspace.join(MEMORY_PATH)
    }
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    let title = trimmed[level..].trim();
    if title.is_empty() {
        None
    } else {
        Some((level, title.to_string()))
    }
}

fn contains_heading(content: &str, section: &str) -> bool {
    let expected = section.trim().to_lowercase();
    content
        .lines()
        .any(|line| line.trim_start_matches('#').trim().to_lowercase() == expected)
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| CoAIError::File(format!("Failed to create project memory directory: {}", e)))?;
    }
    Ok(())
}
