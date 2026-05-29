use crate::core::{CoAIError, FileInfo, Result};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const DEFAULT_FILE_READ_MAX_BYTES: usize = 512 * 1024;

/// File change info, including diff output
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: String,
    pub operation: &'static str, // "created", "modified"
    pub diff: String,
    pub old_size: usize,
    pub new_size: usize,
}

impl FileChange {
    /// Format as a human-readable summary
    pub fn summary(&self) -> String {
        format!(
            "{} ({}, {} → {})\n{}",
            self.path, self.operation, self.old_size, self.new_size, self.diff
        )
    }
}

pub struct FileTools {
    workspace: PathBuf,
    allow_external_paths: bool,
}

impl FileTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            allow_external_paths: false,
        }
    }

    pub fn with_external_paths(mut self, allow: bool) -> Self {
        self.allow_external_paths = allow;
        self
    }

    pub async fn read(&self, path: &str) -> Result<String> {
        let full_path = self.resolve_path(path)?;
        let max_bytes = file_read_max_bytes();
        let metadata = fs::metadata(&full_path)
            .map_err(|e| CoAIError::File(format!("Failed to read file {}: {}", path, e)))?;

        if metadata.len() as usize <= max_bytes {
            return fs::read_to_string(&full_path)
                .map_err(|e| CoAIError::File(format!("Failed to read file {}: {}", path, e)));
        }

        let mut file = fs::File::open(&full_path)
            .map_err(|e| CoAIError::File(format!("Failed to read file {}: {}", path, e)))?;
        let mut buf = vec![0; max_bytes];
        let n = file
            .read(&mut buf)
            .map_err(|e| CoAIError::File(format!("Failed to read file {}: {}", path, e)))?;
        buf.truncate(n);
        let mut content = String::from_utf8_lossy(&buf).to_string();
        content.push_str(&format!(
            "\n\n[File truncated: total size {}KB, read first {}KB. Use search.grep or a narrower scope.]",
            metadata.len() / 1024,
            max_bytes / 1024
        ));
        Ok(content)
    }

    /// Write a file and return change info (including diff)
    pub async fn write(&self, path: &str, content: &str) -> Result<FileChange> {
        let full_path = self.resolve_path(path)?;
        let path_str = full_path
            .strip_prefix(&self.workspace)
            .unwrap_or(&full_path)
            .to_string_lossy()
            .to_string();

        // Read existing content if the file already exists
        let old_content = if full_path.exists() {
            fs::read_to_string(&full_path).unwrap_or_default()
        } else {
            String::new()
        };

        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CoAIError::File(format!("Failed to create directory: {}", e)))?;
        }

        // Determine the operation type
        let operation = if old_content.is_empty() && !full_path.exists() {
            "created"
        } else {
            "modified"
        };

        fs::write(&full_path, content)
            .map_err(|e| CoAIError::File(format!("Failed to write file {}: {}", path, e)))?;

        let diff = compute_diff(&old_content, content, &path_str);
        let old_size = old_content.len();
        let new_size = content.len();

        Ok(FileChange {
            path: path_str,
            operation,
            diff,
            old_size,
            new_size,
        })
    }

    /// Edit a file (exact string replacement) and return change info (including diff)
    pub async fn edit(&self, path: &str, old: &str, new: &str) -> Result<FileChange> {
        let content = self.read(path).await?;

        if !content.contains(old) {
            return Err(CoAIError::File(format!(
                "Replacement string not found: {}",
                if old.len() > 50 { &old[..50] } else { old }
            )));
        }

        let new_content = content.replace(old, new);
        let change = self.write(path, &new_content).await?;
        Ok(change)
    }

    pub async fn list(&self, dir: &str) -> Result<Vec<FileInfo>> {
        let full_path = self.resolve_path(dir)?;
        let mut results = Vec::new();

        let entries = fs::read_dir(&full_path)
            .map_err(|e| CoAIError::File(format!("Failed to read directory {}: {}", dir, e)))?;

        for entry in entries {
            let entry = entry
                .map_err(|e| CoAIError::File(format!("Failed to read directory entry: {}", e)))?;
            let path = entry.path();
            let metadata = entry.metadata().ok();

            results.push(FileInfo {
                path: path
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string(),
                is_dir: path.is_dir(),
                size: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                modified: metadata
                    .and_then(|m| m.modified().ok())
                    .map(chrono::DateTime::from),
            });
        }

        Ok(results)
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let full_path = self.resolve_path(path)?;

        if full_path.is_dir() {
            fs::remove_dir_all(&full_path).map_err(|e| {
                CoAIError::File(format!("Failed to delete directory {}: {}", path, e))
            })?;
        } else {
            fs::remove_file(&full_path)
                .map_err(|e| CoAIError::File(format!("Failed to delete file {}: {}", path, e)))?;
        }

        Ok(())
    }

    pub async fn copy(&self, from: &str, to: &str) -> Result<()> {
        let from_path = self.resolve_path(from)?;
        let to_path = self.resolve_path(to)?;

        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CoAIError::File(format!("Failed to create directory: {}", e)))?;
        }

        fs::copy(&from_path, &to_path).map_err(|e| {
            CoAIError::File(format!("Failed to copy file {} -> {}: {}", from, to, e))
        })?;

        Ok(())
    }

    pub async fn r#move(&self, from: &str, to: &str) -> Result<()> {
        let from_path = self.resolve_path(from)?;
        let to_path = self.resolve_path(to)?;

        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CoAIError::File(format!("Failed to create directory: {}", e)))?;
        }

        fs::rename(&from_path, &to_path).map_err(|e| {
            CoAIError::File(format!("Failed to move file {} -> {}: {}", from, to, e))
        })?;

        Ok(())
    }

    pub async fn exists(&self, path: &str) -> Result<bool> {
        let full_path = self.resolve_path(path)?;
        Ok(full_path.exists())
    }

    pub async fn diff(&self, path: &str) -> Result<String> {
        let full_path = self.resolve_path(path)?;
        let path_str = full_path
            .strip_prefix(&self.workspace)
            .unwrap_or(&full_path)
            .to_string_lossy()
            .to_string();

        if !full_path.exists() {
            return Err(CoAIError::File(format!("File not found: {}", path)));
        }

        // Read current file content (to confirm it is readable)
        let _content = fs::read_to_string(&full_path)
            .map_err(|e| CoAIError::File(format!("Failed to read file {}: {}", path, e)))?;

        // Return an empty diff
        Ok(format!(
            "--- a/{}\n+++ b/{}\n@@ -1 +1 @@\n file unchanged",
            path_str, path_str
        ))
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);
        let workspace = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| normalize_path(&self.workspace));

        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            workspace.join(path)
        };

        if self.allow_external_paths {
            return Ok(full_path);
        }

        let normalized = normalize_path(&full_path);
        if !normalized.starts_with(&workspace) && normalized != workspace {
            return Err(CoAIError::Security(format!(
                "Path is outside the working directory: {}. Use a relative path inside the workspace; if an external path is truly needed, ask the user to confirm the target path and permissions.",
                path.display()
            )));
        }

        if full_path.exists() {
            let canonical = full_path.canonicalize().map_err(|e| {
                CoAIError::File(format!("Failed to resolve path {}: {}", path.display(), e))
            })?;
            if !canonical.starts_with(&workspace) && canonical != workspace {
                return Err(CoAIError::Security(format!(
                    "Path is outside the working directory: {}. Use a relative path inside the workspace; if an external path is truly needed, ask the user to confirm the target path and permissions.",
                    path.display()
                )));
            }
        } else if let Some(parent) = full_path.parent() {
            if let Some(existing_parent) = nearest_existing_parent(parent) {
                let canonical_parent = existing_parent.canonicalize().map_err(|e| {
                    CoAIError::File(format!("Failed to resolve path {}: {}", path.display(), e))
                })?;
                if !canonical_parent.starts_with(&workspace) && canonical_parent != workspace {
                    return Err(CoAIError::Security(format!(
                        "Path is outside the working directory: {}. Use a relative path inside the workspace; if an external path is truly needed, ask the user to confirm the target path and permissions.",
                        path.display()
                    )));
                }
            }
        }

        Ok(normalized)
    }
}

fn nearest_existing_parent(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }

    normalized
}

fn file_read_max_bytes() -> usize {
    std::env::var("COAI_FILE_READ_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_FILE_READ_MAX_BYTES)
}

// ─── Diff computation ─────────────────────────────────────────────

/// Compute a line-level unified diff of two strings
fn compute_diff(old_text: &str, new_text: &str, file_path: &str) -> String {
    if old_text == new_text {
        return String::new();
    }

    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();

    // Compute edit operations using LCS
    let ops = compute_edit_ops(&old_lines, &new_lines);

    // Generate unified diff format
    format_diff(&old_lines, &new_lines, &ops, file_path)
}

/// Edit operation type
#[derive(Debug, Clone, PartialEq)]
enum EditOp {
    Equal(usize),  // Index of equal line (same in old and new)
    Delete(usize), // Index of line in old
    Insert(usize), // Index of line in new
}

/// Compute the edit operation sequence using LCS
fn compute_edit_ops(old: &[&str], new: &[&str]) -> Vec<EditOp> {
    let o = old.len();
    let n = new.len();

    // DP table: dp[i][j] = LCS length
    let mut dp = vec![vec![0usize; n + 1]; o + 1];

    for i in 1..=o {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to build edit operations
    let mut ops = Vec::new();
    let mut i = o;
    let mut j = n;

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            ops.push(EditOp::Equal(i - 1));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops.push(EditOp::Insert(j - 1));
            j -= 1;
        } else {
            ops.push(EditOp::Delete(i - 1));
            i -= 1;
        }
    }

    ops.reverse();
    ops
}

/// Format edit operations as unified diff
fn format_diff(old: &[&str], new: &[&str], ops: &[EditOp], file_path: &str) -> String {
    if ops.is_empty() {
        return String::new();
    }

    // All-insert or all-delete case
    let all_inserts = ops.iter().all(|op| matches!(op, EditOp::Insert(_)));
    let all_deletes = ops.iter().all(|op| matches!(op, EditOp::Delete(_)));

    if all_inserts {
        let count = ops.len();
        let mut result = format!(
            "--- a/{}\n+++ b/{}\n@@ -1,0 +1,{} @@\n",
            file_path, file_path, count
        );
        for op in ops {
            if let EditOp::Insert(idx) = op {
                for line in new[*idx].split_inclusive('\n') {
                    // Add newline if the line lacks one
                    if line.ends_with('\n') {
                        result.push_str(&format!("+{}", line));
                    } else {
                        result.push_str(&format!("+{}\n", line));
                    }
                }
            }
        }
        return result;
    }

    if all_deletes {
        let count = ops.len();
        let mut result = format!(
            "--- a/{}\n+++ b/{}\n@@ -1,{} +1,0 @@\n",
            file_path, file_path, count
        );
        for op in ops {
            if let EditOp::Delete(idx) = op {
                for line in old[*idx].split_inclusive('\n') {
                    if line.ends_with('\n') {
                        result.push_str(&format!("-{}", line));
                    } else {
                        result.push_str(&format!("-{}\n", line));
                    }
                }
            }
        }
        return result;
    }

    // Group ops into hunks (contiguous change blocks)
    let mut hunks: Vec<Vec<(usize, &EditOp)>> = Vec::new();
    let mut current_hunk = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        match op {
            EditOp::Equal(_) => {
                if !current_hunk.is_empty() {
                    // When equal follows current hunk, flush it
                    // Simple approach: each contiguous changed block is one hunk
                    hunks.push(std::mem::take(&mut current_hunk));
                }
            }
            _ => {
                current_hunk.push((idx, op));
            }
        }
    }
    if !current_hunk.is_empty() {
        hunks.push(current_hunk);
    }

    if hunks.is_empty() {
        return String::new();
    }

    let mut result = String::new();
    result.push_str(&format!("--- a/{}\n+++ b/{}\n", file_path, file_path));

    for hunk in &hunks {
        if hunk.is_empty() {
            continue;
        }

        // Compute hunk start positions in old and new
        let (old_start, new_start) = compute_hunk_position(ops, hunk);

        // Count old and new lines in the hunk
        let mut old_count = 0usize;
        let mut new_count = 0usize;
        for (_, op) in hunk {
            match op {
                EditOp::Equal(_) => {
                    old_count += 1;
                    new_count += 1;
                }
                EditOp::Delete(_) => old_count += 1,
                EditOp::Insert(_) => new_count += 1,
            }
        }

        result.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start, old_count, new_start, new_count
        ));

        for (_, op) in hunk {
            match op {
                EditOp::Equal(idx) => {
                    for line in old[*idx].split_inclusive('\n') {
                        if line.ends_with('\n') {
                            result.push_str(&format!(" {}", line));
                        } else {
                            result.push_str(&format!(" {}\n", line));
                        }
                    }
                }
                EditOp::Delete(idx) => {
                    for line in old[*idx].split_inclusive('\n') {
                        if line.ends_with('\n') {
                            result.push_str(&format!("-{}", line));
                        } else {
                            result.push_str(&format!("-{}\n", line));
                        }
                    }
                }
                EditOp::Insert(idx) => {
                    for line in new[*idx].split_inclusive('\n') {
                        if line.ends_with('\n') {
                            result.push_str(&format!("+{}", line));
                        } else {
                            result.push_str(&format!("+{}\n", line));
                        }
                    }
                }
            }
        }
    }

    result
}

/// Compute hunk start line numbers in old and new (1-based)
fn compute_hunk_position(ops: &[EditOp], hunk: &[(usize, &EditOp)]) -> (usize, usize) {
    let first_idx = hunk[0].0;

    let mut old_line = 1usize;
    let mut new_line = 1usize;

    for op in ops.iter().take(first_idx) {
        match op {
            EditOp::Equal(_) => {
                old_line += 1;
                new_line += 1;
            }
            EditOp::Delete(_) => old_line += 1,
            EditOp::Insert(_) => new_line += 1,
        }
    }

    (old_line, new_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_diff_add_lines() {
        let old = "line1\nline2\n";
        let new = "line1\nline2\nline3\nline4\n";
        let diff = compute_diff(old, new, "test.txt");
        assert!(diff.contains("+line3"));
        assert!(diff.contains("+line4"));
        assert!(!diff.contains("-line1"));
        assert!(diff.contains("@@"));
    }

    #[test]
    fn test_compute_diff_delete_lines() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nline3\n";
        let diff = compute_diff(old, new, "test.txt");
        assert!(diff.contains("-line2"));
    }

    #[test]
    fn test_compute_diff_modify() {
        let old = "line1\nold line\nline3\n";
        let new = "line1\nnew line\nline3\n";
        let diff = compute_diff(old, new, "test.txt");
        assert!(diff.contains("-old line"));
        assert!(diff.contains("+new line"));
    }

    #[test]
    fn test_compute_diff_no_change() {
        let old = "same content\n";
        let new = "same content\n";
        let diff = compute_diff(old, new, "test.txt");
        assert!(diff.is_empty());
    }

    #[test]
    fn test_compute_diff_empty_new() {
        let old = "line1\nline2\n";
        let new = "";
        let diff = compute_diff(old, new, "test.txt");
        assert!(diff.contains("-line1"));
        assert!(diff.contains("-line2"));
    }

    #[test]
    fn test_compute_diff_empty_old() {
        let old = "";
        let new = "line1\nline2\n";
        let diff = compute_diff(old, new, "test.txt");
        assert!(diff.contains("+line1"));
        assert!(diff.contains("+line2"));
    }

    #[tokio::test]
    async fn rejects_external_write_by_default() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let external_file = external.path().join("config.toml");

        let result = FileTools::new(workspace.path())
            .write(external_file.to_str().unwrap(), "value = true\n")
            .await;

        assert!(result.is_err());
        assert!(!external_file.exists());
    }

    #[tokio::test]
    async fn rejects_relative_escape_for_new_file() {
        let workspace = tempfile::tempdir().unwrap();
        let external_file = workspace.path().join("..").join("coai-path-escape.txt");
        let _ = std::fs::remove_file(&external_file);

        let result = FileTools::new(workspace.path())
            .write("../coai-path-escape.txt", "value = true\n")
            .await;

        assert!(result.is_err());
        assert!(!external_file.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_parent_escape() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let link = workspace.path().join("linked");
        std::os::unix::fs::symlink(external.path(), &link).unwrap();

        let result = FileTools::new(workspace.path())
            .write("linked/config.toml", "value = true\n")
            .await;

        assert!(result.is_err());
        assert!(!external.path().join("config.toml").exists());
    }

    #[tokio::test]
    async fn allows_external_write_when_enabled() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let external_file = external.path().join("config.toml");

        FileTools::new(workspace.path())
            .with_external_paths(true)
            .write(external_file.to_str().unwrap(), "value = true\n")
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(external_file).unwrap(),
            "value = true\n"
        );
    }
}
