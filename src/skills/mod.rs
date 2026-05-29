//! Claude/Codex-compatible skill discovery.
//!
//! A skill is a directory containing `SKILL.md`. CoAI only discovers and reads
//! skill instructions; the LLM decides when a skill is relevant.

use crate::core::{CoAIError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_FILE_NAMES: [&str; 2] = ["SKILL.md", "skill.md"];
const PROMPT_SKILL_LIMIT: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub path: String,
    pub source: String,
}

#[derive(Debug, Clone)]
struct SkillEntry {
    summary: SkillSummary,
    skill_file: PathBuf,
}

#[derive(Debug, Clone)]
struct SkillRoot {
    path: PathBuf,
    source: String,
}

#[derive(Debug, Clone)]
pub struct SkillRegistry {
    workspace: PathBuf,
}

impl SkillRegistry {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    pub fn list(&self) -> Result<Vec<SkillSummary>> {
        Ok(self
            .discover_entries()?
            .into_iter()
            .map(|entry| entry.summary)
            .collect())
    }

    pub fn search(&self, query: &str) -> Result<Vec<SkillSummary>> {
        let query = query.to_lowercase();
        let mut matches = Vec::new();
        for entry in self.discover_entries()? {
            let content = fs::read_to_string(&entry.skill_file).unwrap_or_default();
            if entry.summary.name.to_lowercase().contains(&query)
                || entry.summary.description.to_lowercase().contains(&query)
                || content.to_lowercase().contains(&query)
            {
                matches.push(entry.summary);
            }
        }
        Ok(matches)
    }

    pub fn read(&self, name_or_path: &str) -> Result<String> {
        let entries = self.discover_entries()?;
        let mut matches = Vec::new();
        let needle = name_or_path.trim();
        let needle_lower = needle.to_lowercase();

        for entry in entries {
            let dir_name = entry
                .skill_file
                .parent()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if entry.summary.name.eq_ignore_ascii_case(needle)
                || dir_name.eq_ignore_ascii_case(needle)
                || entry.summary.path == needle
                || entry.skill_file.to_string_lossy() == needle
            {
                matches.push(entry);
                continue;
            }

            if let Ok(path) = resolve_requested_path(&self.workspace, needle) {
                if normalize_path(&entry.skill_file) == normalize_path(&path)
                    || entry
                        .skill_file
                        .parent()
                        .map(|parent| normalize_path(parent) == normalize_path(&path))
                        .unwrap_or(false)
                {
                    matches.push(entry);
                    continue;
                }
            }

            if entry.summary.name.to_lowercase() == needle_lower {
                matches.push(entry);
            }
        }

        match matches.len() {
            0 => Err(CoAIError::Other(format!("Skill not found: {}", name_or_path))),
            1 => fs::read_to_string(&matches[0].skill_file).map_err(|e| {
                CoAIError::File(format!(
                    "Failed to read skill {}: {}",
                    matches[0].skill_file.display(),
                    e
                ))
            }),
            _ => {
                let candidates = matches
                    .into_iter()
                    .map(|entry| format!("{} ({})", entry.summary.name, entry.summary.path))
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(CoAIError::Other(format!(
                    "Skill name is ambiguous; use the path instead: {}",
                    candidates
                )))
            }
        }
    }

    pub fn prompt_context(&self) -> String {
        let Ok(skills) = self.list() else {
            return String::new();
        };
        if skills.is_empty() {
            return String::new();
        }

        let mut lines = vec![
            "## Skills".to_string(),
            "Claude/Codex-compatible skills detected in the workspace. A skill is user-provided specialized instructions; decide yourself whether a task requires one. When relevant, first read the full SKILL.md with skills.read, then follow its instructions — do not act on the summary alone.".to_string(),
            "Available skills:".to_string(),
        ];
        for skill in skills.iter().take(PROMPT_SKILL_LIMIT) {
            lines.push(format!(
                "- {} [{}]: {} ({})",
                skill.name,
                skill.source,
                truncate_for_prompt(&skill.description, 180),
                skill.path
            ));
        }
        if skills.len() > PROMPT_SKILL_LIMIT {
            lines.push(format!(
                "- {} more skills available; use skills.list to see the full list.",
                skills.len() - PROMPT_SKILL_LIMIT
            ));
        }
        lines.join("\n")
    }

    fn discover_entries(&self) -> Result<Vec<SkillEntry>> {
        let roots = self.skill_roots();
        let mut seen = HashSet::new();
        let mut entries = Vec::new();

        for root in roots {
            if !root.path.exists() {
                continue;
            }
            for skill_dir in skill_dirs_under(&root.path)? {
                let Some(skill_file) = find_skill_file(&skill_dir) else {
                    continue;
                };
                let key = normalize_path(&skill_file).to_string_lossy().to_string();
                if !seen.insert(key) {
                    continue;
                }
                let content = fs::read_to_string(&skill_file).map_err(|e| {
                    CoAIError::File(format!("Failed to read skill {}: {}", skill_file.display(), e))
                })?;
                let metadata = parse_skill_metadata(&content, &skill_dir);
                entries.push(SkillEntry {
                    summary: SkillSummary {
                        name: metadata.0,
                        description: metadata.1,
                        path: display_path(&self.workspace, &skill_file),
                        source: root.source.clone(),
                    },
                    skill_file,
                });
            }
        }

        entries.sort_by(|a, b| {
            a.summary
                .source
                .cmp(&b.summary.source)
                .then(a.summary.name.cmp(&b.summary.name))
        });
        Ok(entries)
    }

    fn skill_roots(&self) -> Vec<SkillRoot> {
        let workspace = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| normalize_path(&self.workspace));
        let mut roots = vec![
            SkillRoot {
                path: workspace.join(".codex/skills"),
                source: "project-codex".to_string(),
            },
            SkillRoot {
                path: workspace.join(".claude/skills"),
                source: "project-claude".to_string(),
            },
            SkillRoot {
                path: workspace.join("skills"),
                source: "project".to_string(),
            },
        ];

        if let Ok(value) = std::env::var("COAI_SKILL_PATHS") {
            for path in std::env::split_paths(&value) {
                roots.push(SkillRoot {
                    path,
                    source: "env".to_string(),
                });
            }
        }

        if let Ok(codex_home) = std::env::var("CODEX_HOME") {
            roots.push(SkillRoot {
                path: PathBuf::from(codex_home).join("skills"),
                source: "codex".to_string(),
            });
        } else if let Some(home) = dirs::home_dir() {
            roots.push(SkillRoot {
                path: home.join(".codex/skills"),
                source: "codex".to_string(),
            });
        }

        if let Ok(claude_home) = std::env::var("CLAUDE_HOME") {
            roots.push(SkillRoot {
                path: PathBuf::from(claude_home).join("skills"),
                source: "claude".to_string(),
            });
        } else if let Some(home) = dirs::home_dir() {
            roots.push(SkillRoot {
                path: home.join(".claude/skills"),
                source: "claude".to_string(),
            });
        }

        roots
    }
}

fn skill_dirs_under(root: &Path) -> Result<Vec<PathBuf>> {
    if find_skill_file(root).is_some() {
        return Ok(vec![root.to_path_buf()]);
    }

    let mut dirs = Vec::new();
    let entries = fs::read_dir(root)
        .map_err(|e| CoAIError::File(format!("Failed to read skill directory {}: {}", root.display(), e)))?;
    for entry in entries {
        let entry = entry.map_err(|e| CoAIError::File(format!("Failed to read skill directory entry: {}", e)))?;
        let path = entry.path();
        if path.is_dir() && find_skill_file(&path).is_some() {
            dirs.push(path);
        }
    }
    Ok(dirs)
}

fn find_skill_file(dir: &Path) -> Option<PathBuf> {
    SKILL_FILE_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

fn parse_skill_metadata(content: &str, dir: &Path) -> (String, String) {
    let mut name = None;
    let mut description = None;
    let body_start = if content.lines().next().map(|line| line.trim()) == Some("---") {
        let mut offset = 0usize;
        let mut in_header = false;
        for line in content.split_inclusive('\n') {
            let trimmed = line.trim();
            offset += line.len();
            if !in_header {
                in_header = true;
                continue;
            }
            if trimmed == "---" {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                let value = clean_frontmatter_value(value);
                match key.trim() {
                    "name" | "title" => name = Some(value),
                    "description" | "summary" => description = Some(value),
                    _ => {}
                }
            }
        }
        offset.min(content.len())
    } else {
        0
    };

    let body = &content[body_start..];
    if name.as_deref().unwrap_or("").trim().is_empty() {
        name = first_heading(body).or_else(|| {
            dir.file_name()
                .and_then(|value| value.to_str())
                .map(|value| value.to_string())
        });
    }
    if description.as_deref().unwrap_or("").trim().is_empty() {
        description = first_body_paragraph(body);
    }

    (
        name.unwrap_or_else(|| "unnamed-skill".to_string()),
        description.unwrap_or_default(),
    )
}

fn clean_frontmatter_value(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn first_heading(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('#'))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .next()
        .filter(|value| !value.is_empty())
}

fn first_body_paragraph(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("---"))
        .map(|line| truncate_for_prompt(line, 240))
}

fn display_path(workspace: &Path, path: &Path) -> String {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(workspace));
    let normalized = path.canonicalize().unwrap_or_else(|_| normalize_path(path));
    normalized
        .strip_prefix(&workspace)
        .unwrap_or(&normalized)
        .to_string_lossy()
        .to_string()
}

fn resolve_requested_path(workspace: &Path, value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(workspace.join(path))
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn truncate_for_prompt(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_metadata() {
        let content = "---\nname: docx\nsummary: Create Word documents\n---\n# ignored\nbody";
        let (name, description) = parse_skill_metadata(content, Path::new("docx"));
        assert_eq!(name, "docx");
        assert_eq!(description, "Create Word documents");
    }

    #[test]
    fn falls_back_to_heading_and_paragraph() {
        let content = "# imagegen\n\nGenerate raster images when needed.";
        let (name, description) = parse_skill_metadata(content, Path::new("imagegen"));
        assert_eq!(name, "imagegen");
        assert_eq!(description, "Generate raster images when needed.");
    }
}
