use crate::core::{CoAIError, Result, SearchResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_SEMANTIC_FILES: usize = 2_000;
const MAX_SEMANTIC_FILE_BYTES: u64 = 512 * 1024;
const SEMANTIC_CHUNK_LINES: usize = 32;
const SEMANTIC_CHUNK_OVERLAP: usize = 8;

pub struct SearchTools {
    workspace: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchIndex {
    root: String,
    created_at: String,
    chunks: Vec<IndexedChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedChunk {
    file: String,
    line: usize,
    content: String,
    terms: Vec<String>,
    modified: Option<u64>,
    size: u64,
}

impl SearchTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    pub async fn grep(&self, pattern: &str, path: Option<&str>) -> Result<Vec<SearchResult>> {
        let search_path = path
            .map(|p| self.workspace.join(p))
            .unwrap_or_else(|| self.workspace.clone());

        let output = Command::new("grep")
            .arg("-rn")
            .arg("--color=never")
            .arg(pattern)
            .arg(&search_path)
            .output()
            .map_err(|e| CoAIError::Command(format!("grep failed: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut results = Vec::new();

        for line in stdout.lines() {
            if let Some((file_line, content)) = line.split_once(':') {
                if let Some((file, line_num)) = file_line.rsplit_once(':') {
                    if let Ok(line) = line_num.parse::<usize>() {
                        results.push(SearchResult {
                            file: file.to_string(),
                            line,
                            content: content.trim().to_string(),
                            score: 1.0,
                        });
                    }
                }
            }
        }

        Ok(results)
    }

    pub async fn find(&self, name: &str, path: Option<&str>) -> Result<Vec<String>> {
        let search_path = path
            .map(|p| self.workspace.join(p))
            .unwrap_or_else(|| self.workspace.clone());

        let output = Command::new("find")
            .arg(&search_path)
            .arg("-name")
            .arg(name)
            .arg("-type")
            .arg("f")
            .output()
            .map_err(|e| CoAIError::Command(format!("find failed: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let results: Vec<String> = stdout
            .lines()
            .filter_map(|line| {
                PathBuf::from(line)
                    .strip_prefix(&self.workspace)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            })
            .collect();

        Ok(results)
    }

    pub async fn regex(&self, pattern: &str, path: Option<&str>) -> Result<Vec<SearchResult>> {
        let search_path = path
            .map(|p| self.workspace.join(p))
            .unwrap_or_else(|| self.workspace.clone());

        let output = Command::new("grep")
            .arg("-rnP")
            .arg("--color=never")
            .arg(pattern)
            .arg(&search_path)
            .output()
            .map_err(|e| CoAIError::Command(format!("grep -P failed: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut results = Vec::new();

        for line in stdout.lines() {
            if let Some((file_line, content)) = line.split_once(':') {
                if let Some((file, line_num)) = file_line.rsplit_once(':') {
                    if let Ok(line) = line_num.parse::<usize>() {
                        results.push(SearchResult {
                            file: file.to_string(),
                            line,
                            content: content.trim().to_string(),
                            score: 1.0,
                        });
                    }
                }
            }
        }

        Ok(results)
    }

    pub async fn semantic(
        &self,
        query: &str,
        k: usize,
        path: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let query = query.trim();
        if query.is_empty() {
            return Err(CoAIError::Other("missing query parameter".to_string()));
        }

        let search_path = path
            .map(|p| self.workspace.join(p))
            .unwrap_or_else(|| self.workspace.clone());
        let query_terms = expand_query_terms(query);
        let query_vec = term_vector(&query_terms);
        let query_lower = query.to_lowercase();

        if let Some(index) = self.load_index(path)? {
            let mut results = index
                .chunks
                .into_iter()
                .filter(|chunk| indexed_chunk_is_fresh(&self.workspace, chunk))
                .filter(|chunk| {
                    path.map(|path| chunk.file.starts_with(path.trim_matches('/')))
                        .unwrap_or(true)
                })
                .filter_map(|chunk| {
                    let chunk_vec = term_vector(&chunk.terms);
                    let mut score = cosine_similarity(&query_vec, &chunk_vec);
                    if chunk.file.to_lowercase().contains(&query_lower) {
                        score += 0.25;
                    }
                    if chunk.content.to_lowercase().contains(&query_lower) {
                        score += 0.35;
                    }
                    (score > 0.0).then_some(SearchResult {
                        file: chunk.file,
                        line: chunk.line,
                        content: compact_chunk(&chunk.content),
                        score,
                    })
                })
                .collect::<Vec<_>>();
            results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            results.truncate(k.max(1));
            return Ok(results);
        }

        let mut results = Vec::new();
        let mut files_seen = 0usize;

        for file in collect_text_files(&search_path, &mut files_seen)? {
            let relative = file
                .strip_prefix(&self.workspace)
                .unwrap_or(&file)
                .to_string_lossy()
                .to_string();
            let Ok(content) = fs::read_to_string(&file) else {
                continue;
            };
            let path_terms = expand_query_terms(&relative);
            let path_vec = term_vector(&path_terms);

            for (line, chunk) in chunk_lines(&content, SEMANTIC_CHUNK_LINES, SEMANTIC_CHUNK_OVERLAP)
            {
                let mut terms = path_terms.clone();
                terms.extend(expand_query_terms(&chunk));
                let chunk_vec = term_vector(&terms);
                let mut score = cosine_similarity(&query_vec, &chunk_vec);
                score += 0.20 * cosine_similarity(&query_vec, &path_vec);
                if relative.to_lowercase().contains(&query_lower) {
                    score += 0.25;
                }
                if chunk.to_lowercase().contains(&query_lower) {
                    score += 0.35;
                }

                if score > 0.0 {
                    results.push(SearchResult {
                        file: relative.clone(),
                        line,
                        content: compact_chunk(&chunk),
                        score,
                    });
                }
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(k.max(1));
        Ok(results)
    }

    pub async fn index(&self, path: Option<&str>) -> Result<String> {
        let search_path = path
            .map(|p| self.workspace.join(p))
            .unwrap_or_else(|| self.workspace.clone());
        let mut files_seen = 0usize;
        let mut chunks = Vec::new();

        for file in collect_text_files(&search_path, &mut files_seen)? {
            let relative = file
                .strip_prefix(&self.workspace)
                .unwrap_or(&file)
                .to_string_lossy()
                .to_string();
            let Ok(content) = fs::read_to_string(&file) else {
                continue;
            };
            let metadata = fs::metadata(&file)?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs());
            let path_terms = expand_query_terms(&relative);

            for (line, chunk) in chunk_lines(&content, SEMANTIC_CHUNK_LINES, SEMANTIC_CHUNK_OVERLAP)
            {
                let mut terms = path_terms.clone();
                terms.extend(expand_query_terms(&chunk));
                terms.sort();
                terms.dedup();
                chunks.push(IndexedChunk {
                    file: relative.clone(),
                    line,
                    content: compact_chunk(&chunk),
                    terms,
                    modified,
                    size: metadata.len(),
                });
            }
        }

        let index = SearchIndex {
            root: path.unwrap_or(".").to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            chunks,
        };
        let index_path = self.index_path();
        if let Some(parent) = index_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&index_path, serde_json::to_string_pretty(&index)?)?;
        Ok(format!(
            "Semantic search index written to {}, {} file(s), {} chunk(s)",
            index_path.display(),
            files_seen,
            index.chunks.len()
        ))
    }

    fn index_path(&self) -> PathBuf {
        self.workspace.join(".coai/state/search-index.json")
    }

    fn load_index(&self, path: Option<&str>) -> Result<Option<SearchIndex>> {
        let index_path = self.index_path();
        if !index_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(index_path)?;
        let index: SearchIndex = serde_json::from_str(&content)?;
        if let Some(path) = path {
            let root = index.root.trim_matches('/');
            let path = path.trim_matches('/');
            if root != "." && !path.starts_with(root) && !root.starts_with(path) {
                return Ok(None);
            }
        }
        Ok(Some(index))
    }
}

fn indexed_chunk_is_fresh(workspace: &Path, chunk: &IndexedChunk) -> bool {
    let path = workspace.join(&chunk.file);
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if metadata.len() != chunk.size {
        return false;
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    modified == chunk.modified
}

fn collect_text_files(path: &PathBuf, files_seen: &mut usize) -> Result<Vec<PathBuf>> {
    if *files_seen >= MAX_SEMANTIC_FILES || should_skip_path(path) {
        return Ok(Vec::new());
    }
    if path.is_file() {
        let metadata = fs::metadata(path)?;
        if metadata.len() <= MAX_SEMANTIC_FILE_BYTES && looks_text_path(path) {
            *files_seen += 1;
            return Ok(vec![path.clone()]);
        }
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    if !path.is_dir() {
        return Ok(files);
    }

    let mut entries = fs::read_dir(path)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    entries.sort();

    for entry in entries {
        if *files_seen >= MAX_SEMANTIC_FILES {
            break;
        }
        files.extend(collect_text_files(&entry, files_seen)?);
    }
    Ok(files)
}

fn should_skip_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".next"
            | "dist"
            | "build"
            | "vendor"
            | ".coai"
            | ".DS_Store"
    )
}

fn looks_text_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return true;
    };
    !matches!(
        ext.to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "pdf"
            | "zip"
            | "gz"
            | "tar"
            | "7z"
            | "wasm"
            | "rlib"
            | "dylib"
            | "so"
            | "a"
            | "o"
            | "lock"
    )
}

fn chunk_lines(content: &str, chunk_size: usize, overlap: usize) -> Vec<(usize, String)> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < lines.len() {
        let end = (start + chunk_size).min(lines.len());
        chunks.push((start + 1, lines[start..end].join("\n")));
        if end == lines.len() {
            break;
        }
        start += step;
    }
    chunks
}

fn compact_chunk(chunk: &str) -> String {
    let mut out = chunk
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(10)
        .collect::<Vec<_>>()
        .join("\n");
    if out.chars().count() > 900 {
        out = out.chars().take(900).collect::<String>();
        out.push_str("...");
    }
    out
}

fn expand_query_terms(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut terms = tokenize(&lower);
    for (phrase, aliases) in semantic_aliases() {
        if lower.contains(phrase) {
            terms.extend(aliases.iter().map(|alias| alias.to_string()));
        }
    }
    terms
}

fn tokenize(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    for ch in split_camel_boundaries(text).chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            current.push(ch);
        } else {
            push_identifier_terms(&mut terms, &current);
            current.clear();
            if is_cjk(ch) {
                terms.push(ch.to_string());
            }
        }
    }
    push_identifier_terms(&mut terms, &current);

    let cjk_chars = text.chars().filter(|ch| is_cjk(*ch)).collect::<Vec<_>>();
    for pair in cjk_chars.windows(2) {
        terms.push(pair.iter().collect());
    }
    terms
        .into_iter()
        .filter(|term| term.len() > 1 || term.chars().any(is_cjk))
        .filter(|term| !STOP_WORDS.contains(&term.as_str()))
        .collect()
}

fn split_camel_boundaries(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    for ch in text.chars() {
        if let Some(prev) = prev {
            if prev.is_ascii_lowercase() && ch.is_ascii_uppercase() {
                out.push(' ');
            }
        }
        out.push(ch);
        prev = Some(ch);
    }
    out
}

fn push_identifier_terms(terms: &mut Vec<String>, value: &str) {
    if value.is_empty() {
        return;
    }
    for part in value.split(['_', '-', '/']) {
        let part = part.trim();
        if part.len() > 1 {
            terms.push(part.to_string());
        }
    }
}

fn term_vector(terms: &[String]) -> HashMap<String, f64> {
    let mut unique = HashSet::new();
    let mut vector = HashMap::new();
    for term in terms {
        if unique.insert(term.clone()) {
            let weight = if term.chars().any(is_cjk) { 1.2 } else { 1.0 };
            *vector.entry(term.clone()).or_insert(0.0) += weight;
        }
    }
    vector
}

fn cosine_similarity(a: &HashMap<String, f64>, b: &HashMap<String, f64>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let dot = a
        .iter()
        .filter_map(|(term, av)| b.get(term).map(|bv| av * bv))
        .sum::<f64>();
    if dot == 0.0 {
        return 0.0;
    }
    let an = a.values().map(|v| v * v).sum::<f64>().sqrt();
    let bn = b.values().map(|v| v * v).sum::<f64>().sqrt();
    if an == 0.0 || bn == 0.0 {
        0.0
    } else {
        dot / (an * bn)
    }
}

fn is_cjk(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
}

fn semantic_aliases() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        (
            "中断",
            vec!["resume", "retry", "checkpoint", "recover", "恢复"],
        ),
        ("恢复", vec!["resume", "checkpoint", "session", "recover"]),
        ("长任务", vec!["checkpoint", "atomic", "task"]),
        ("上下文", vec!["context", "compact", "budget", "tokens"]),
        ("超限", vec!["limit", "budget", "compact", "tokens"]),
        ("压缩", vec!["compact", "context", "tokens"]),
        ("记忆", vec!["memory", "history", "remember"]),
        ("历史", vec!["history", "session", "record"]),
        ("权限", vec!["permission", "confirm", "approval", "risk"]),
        ("确认", vec!["confirm", "permission", "approval"]),
        ("工具", vec!["tool", "registry", "schema"]),
        ("运行日志", vec!["run", "log", "jsonl", "trace"]),
        ("日志", vec!["log", "trace", "record"]),
        ("提交", vec!["git", "commit", "add", "diff"]),
        ("差异", vec!["git", "diff", "change"]),
        ("搜索", vec!["search", "grep", "find", "semantic"]),
        ("语义", vec!["semantic", "search", "meaning"]),
        ("配置", vec!["config", "settings", "provider"]),
        ("模型", vec!["model", "llm", "provider"]),
        ("tui", vec!["terminal", "ui", "render", "state"]),
        ("resume", vec!["恢复", "checkpoint", "session", "recover"]),
        ("checkpoint", vec!["恢复", "resume", "state"]),
        ("memory", vec!["记忆", "history", "store"]),
        ("permission", vec!["权限", "confirm", "approval", "risk"]),
        ("token", vec!["tokens", "context", "budget", "compact"]),
    ]
}

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "pub", "fn", "let", "mut", "use",
    "mod", "impl", "self", "true", "false", "none", "some",
];

#[cfg(test)]
mod tests {
    use super::SearchTools;

    #[tokio::test]
    async fn semantic_search_finds_related_terms_without_exact_query() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(
            temp.path().join("src/ulw.rs"),
            "pub async fn resume_from_checkpoint() { /* recover state */ }",
        )
        .unwrap();

        let search = SearchTools::new(temp.path());
        let results = search
            .semantic("长任务中断恢复", 5, Some("src"))
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].file, "src/ulw.rs");
    }
}
