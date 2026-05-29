//! Workspace cleanliness tools.
//!
//! These tools help an agent review and remove its own leftover artifacts
//! without ever guessing what counts as "junk". `report` only surfaces what
//! git already reports (untracked / ignored entries) so the agent can decide;
//! `remove` deletes exactly the paths the caller names, behind strict safety
//! checks. There is no built-in extension or pattern blacklist anywhere here —
//! that would over-fit a particular language or toolchain.

use crate::core::{CoAIError, Result};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// Result of listing untracked / ignored entries for review.
#[derive(Debug, Serialize)]
pub struct CleanupReport {
    /// Paths git reports as untracked (porcelain `??`).
    pub untracked: Vec<String>,
    /// Paths git reports as ignored.
    pub ignored: Vec<String>,
}

/// Result of removing caller-specified paths.
#[derive(Debug, Serialize)]
pub struct CleanupRemoveResult {
    /// Paths that were successfully removed.
    pub removed: Vec<String>,
    /// Paths that were skipped, each with a reason.
    pub skipped: Vec<SkippedPath>,
}

#[derive(Debug, Serialize)]
pub struct SkippedPath {
    pub path: String,
    pub reason: String,
}

pub struct CleanupTools {
    workspace: PathBuf,
}

impl CleanupTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    /// List untracked and ignored entries so the agent can review which of them
    /// are its own leftover artifacts. Decision and removal stay with the caller.
    pub async fn report(&self) -> Result<CleanupReport> {
        // `--porcelain` keeps output stable; `--ignored` adds `!!` ignored lines.
        let output = self
            .run_git(&[
                "status",
                "--porcelain",
                "--ignored",
                "--untracked-files=all",
            ])
            .await?;

        let mut untracked = Vec::new();
        let mut ignored = Vec::new();
        for line in output.lines() {
            if let Some(path) = line.strip_prefix("?? ") {
                let path = path.trim();
                if !path.is_empty() {
                    untracked.push(path.to_string());
                }
            } else if let Some(path) = line.strip_prefix("!! ") {
                let path = path.trim();
                if !path.is_empty() {
                    ignored.push(path.to_string());
                }
            }
        }

        Ok(CleanupReport { untracked, ignored })
    }

    /// Remove exactly the caller-specified paths. Each path is validated:
    /// it must resolve inside the workspace, must not be (or live under) `.git`,
    /// and must not be tracked by git. Anything failing a check is skipped with
    /// a reason rather than removed.
    pub async fn remove(&self, paths: &[String]) -> Result<CleanupRemoveResult> {
        let mut removed = Vec::new();
        let mut skipped = Vec::new();

        for raw in paths {
            let path = raw.trim();
            if path.is_empty() {
                continue;
            }

            let full_path = match self.resolve_path(path) {
                Ok(p) => p,
                Err(e) => {
                    skipped.push(SkippedPath {
                        path: path.to_string(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };

            if !full_path.exists() {
                skipped.push(SkippedPath {
                    path: path.to_string(),
                    reason: "Path does not exist".to_string(),
                });
                continue;
            }

            if self.is_tracked(path).await? {
                skipped.push(SkippedPath {
                    path: path.to_string(),
                    reason: "Path is tracked by git; deletion refused".to_string(),
                });
                continue;
            }

            let outcome = if full_path.is_dir() {
                std::fs::remove_dir_all(&full_path)
            } else {
                std::fs::remove_file(&full_path)
            };

            match outcome {
                Ok(()) => removed.push(path.to_string()),
                Err(e) => skipped.push(SkippedPath {
                    path: path.to_string(),
                    reason: format!("Deletion failed: {}", e),
                }),
            }
        }

        Ok(CleanupRemoveResult { removed, skipped })
    }

    /// Resolve a caller path to an absolute path that must stay inside the
    /// workspace and must never be `.git` or anything beneath it.
    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let candidate = Path::new(path);
        let workspace = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| normalize_path(&self.workspace));

        let full_path = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            workspace.join(candidate)
        };

        let normalized = normalize_path(&full_path);
        if !normalized.starts_with(&workspace) || normalized == workspace {
            return Err(CoAIError::Security(format!(
                "Path is outside the working directory or points to the workspace root: {}",
                path
            )));
        }

        // Confirm the real on-disk path (resolving symlinks) is still inside the
        // workspace, so a symlink can't be used to escape.
        let canonical = full_path
            .canonicalize()
            .map_err(|e| CoAIError::File(format!("Failed to resolve path {}: {}", path, e)))?;
        if !canonical.starts_with(&workspace) || canonical == workspace {
            return Err(CoAIError::Security(format!(
                "Path is outside the working directory or points to the workspace root: {}",
                path
            )));
        }

        if is_git_internal(&workspace, &canonical) {
            return Err(CoAIError::Security(format!(
                "Refusing to operate inside .git directory: {}",
                path
            )));
        }

        Ok(canonical)
    }

    /// Whether git currently tracks the given path.
    async fn is_tracked(&self, path: &str) -> Result<bool> {
        let output = tokio::time::timeout(
            Duration::from_secs(120),
            Command::new("git")
                .args(["ls-files", "--error-unmatch", "--", path])
                .current_dir(&self.workspace)
                .output(),
        )
        .await
        .map_err(|_| CoAIError::Command("git ls-files timed out".to_string()))?
        .map_err(|e| CoAIError::Command(format!("git ls-files failed: {}", e)))?;

        Ok(output.status.success())
    }

    async fn run_git(&self, args: &[&str]) -> Result<String> {
        let output = tokio::time::timeout(
            Duration::from_secs(120),
            Command::new("git")
                .args(args)
                .current_dir(&self.workspace)
                .output(),
        )
        .await
        .map_err(|_| CoAIError::Command(format!("git {} timed out", args.join(" "))))?
        .map_err(|e| CoAIError::Command(format!("git {} failed: {}", args.join(" "), e)))?;

        if !output.status.success() {
            return Err(CoAIError::Command(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Whether `candidate` is the workspace `.git` directory or lives inside it.
fn is_git_internal(workspace: &Path, candidate: &Path) -> bool {
    let git_dir = workspace.join(".git");
    candidate == git_dir || candidate.starts_with(&git_dir)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn removes_untracked_file_inside_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join("scratch.tmp");
        std::fs::write(&target, "tmp").unwrap();

        let result = CleanupTools::new(workspace.path())
            .remove(&["scratch.tmp".to_string()])
            .await
            .unwrap();

        assert_eq!(result.removed, vec!["scratch.tmp".to_string()]);
        assert!(result.skipped.is_empty());
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn rejects_path_outside_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let external_file = external.path().join("keep.txt");
        std::fs::write(&external_file, "keep").unwrap();

        let result = CleanupTools::new(workspace.path())
            .remove(&[external_file.to_string_lossy().to_string()])
            .await
            .unwrap();

        assert!(result.removed.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert!(external_file.exists());
    }

    #[tokio::test]
    async fn rejects_relative_escape() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let external_file = external.path().join("keep.txt");
        std::fs::write(&external_file, "keep").unwrap();
        // ../<external dir name>/keep.txt relative to the workspace.
        let escape = format!(
            "../{}/keep.txt",
            external.path().file_name().unwrap().to_string_lossy()
        );

        // Place workspace and external as siblings is not guaranteed by tempdir,
        // so resolve via the canonical parent relationship instead.
        let result = CleanupTools::new(workspace.path())
            .remove(&[escape])
            .await
            .unwrap();

        assert!(result.removed.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert!(external_file.exists());
    }

    #[tokio::test]
    async fn refuses_to_remove_git_directory() {
        let workspace = tempfile::tempdir().unwrap();
        let git_dir = workspace.path().join(".git");
        std::fs::create_dir_all(git_dir.join("hooks")).unwrap();
        let internal = git_dir.join("config");
        std::fs::write(&internal, "[core]\n").unwrap();

        let result = CleanupTools::new(workspace.path())
            .remove(&[".git/config".to_string(), ".git".to_string()])
            .await
            .unwrap();

        assert!(result.removed.is_empty());
        assert_eq!(result.skipped.len(), 2);
        assert!(internal.exists());
        assert!(git_dir.exists());
    }

    #[tokio::test]
    async fn skips_tracked_file() {
        let workspace = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        run_git(ws, &["init"]).await;
        run_git(ws, &["config", "user.email", "t@example.com"]).await;
        run_git(ws, &["config", "user.name", "test"]).await;
        let tracked = ws.join("tracked.txt");
        std::fs::write(&tracked, "content").unwrap();
        run_git(ws, &["add", "tracked.txt"]).await;
        run_git(ws, &["commit", "-m", "init"]).await;

        let result = CleanupTools::new(ws)
            .remove(&["tracked.txt".to_string()])
            .await
            .unwrap();

        assert!(result.removed.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert!(tracked.exists());
    }

    #[tokio::test]
    async fn report_lists_untracked_entry() {
        let workspace = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        run_git(ws, &["init"]).await;
        std::fs::write(ws.join("leftover.tmp"), "x").unwrap();

        let report = CleanupTools::new(ws).report().await.unwrap();
        assert!(report.untracked.iter().any(|p| p.contains("leftover.tmp")));
    }

    async fn run_git(dir: &Path, args: &[&str]) {
        let status = tokio::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .await
            .unwrap();
        assert!(
            status.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&status.stderr)
        );
    }
}
