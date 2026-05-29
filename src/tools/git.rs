use crate::core::{CoAIError, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Serialize)]
pub struct GitCommandResult {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
}

pub struct GitTools {
    workspace: PathBuf,
}

impl GitTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    pub async fn status(&self) -> Result<GitCommandResult> {
        let mut result = self.run_git(&["status", "--short"]).await?;
        // Surface git-reported untracked entries more visibly so callers notice
        // their own leftover artifacts. We only summarize what git already
        // reported (porcelain `??` marker); no extension/pattern is hardcoded.
        if result.success {
            if let Some(summary) = untracked_summary(&result.stdout) {
                result.stdout = if result.stdout.is_empty() {
                    summary
                } else {
                    format!("{}\n\n{}", result.stdout.trim_end(), summary)
                };
            }
        }
        Ok(result)
    }

    pub async fn diff(&self, staged: bool, path: Option<&str>) -> Result<GitCommandResult> {
        let mut args = vec!["diff"];
        if staged {
            args.push("--staged");
        }
        if let Some(path) = path.filter(|path| !path.trim().is_empty()) {
            args.push("--");
            args.push(path);
        }
        self.run_git(&args).await
    }

    pub async fn add(&self, files: &str) -> Result<GitCommandResult> {
        let mut args = vec!["add"];
        for file in files.split_whitespace() {
            args.push(file);
        }
        if args.len() == 1 {
            return Err(CoAIError::Other("Missing required parameter: files".to_string()));
        }
        self.run_git(&args).await
    }

    pub async fn commit(&self, message: &str) -> Result<GitCommandResult> {
        if message.trim().is_empty() {
            return Err(CoAIError::Other("Missing required parameter: message".to_string()));
        }
        self.run_git(&["commit", "-m", message]).await
    }

    pub async fn log(&self, limit: usize, path: Option<&str>) -> Result<GitCommandResult> {
        let limit = limit.clamp(1, 100).to_string();
        let mut args = vec![
            "log",
            "--oneline",
            "--decorate",
            "--graph",
            "-n",
            limit.as_str(),
        ];
        if let Some(path) = path.filter(|path| !path.trim().is_empty()) {
            args.push("--");
            args.push(path);
        }
        self.run_git(&args).await
    }

    pub async fn branch(&self) -> Result<GitCommandResult> {
        self.run_git(&["branch", "--show-current"]).await
    }

    pub async fn show(&self, rev: &str) -> Result<GitCommandResult> {
        let rev = if rev.trim().is_empty() { "HEAD" } else { rev };
        self.run_git(&["show", "--stat", "--oneline", "--decorate", rev])
            .await
    }

    pub async fn pull(
        &self,
        remote: Option<&str>,
        branch: Option<&str>,
    ) -> Result<GitCommandResult> {
        let mut args = vec!["pull"];
        if let Some(remote) = remote.filter(|value| !value.trim().is_empty()) {
            args.push(remote);
        }
        if let Some(branch) = branch.filter(|value| !value.trim().is_empty()) {
            args.push(branch);
        }
        self.run_git(&args).await
    }

    pub async fn push(
        &self,
        remote: Option<&str>,
        branch: Option<&str>,
    ) -> Result<GitCommandResult> {
        let mut args = vec!["push"];
        if let Some(remote) = remote.filter(|value| !value.trim().is_empty()) {
            args.push(remote);
        }
        if let Some(branch) = branch.filter(|value| !value.trim().is_empty()) {
            args.push(branch);
        }
        self.run_git(&args).await
    }

    async fn run_git(&self, args: &[&str]) -> Result<GitCommandResult> {
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

        Ok(GitCommandResult {
            command: format!("git {}", args.join(" ")),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            success: output.status.success(),
        })
    }
}

/// Build a generic summary of untracked entries from `git status --short` output.
/// Untracked entries are the porcelain `??` lines; nothing about their names or
/// extensions is interpreted. Returns `None` when there are no untracked entries.
fn untracked_summary(short_status: &str) -> Option<String> {
    let untracked: Vec<&str> = short_status
        .lines()
        .filter_map(|line| line.strip_prefix("?? "))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .collect();

    if untracked.is_empty() {
        return None;
    }

    let mut summary = format!(
        "Untracked entries ({} item(s) — confirm whether these are artefacts from this session):",
        untracked.len()
    );
    for path in &untracked {
        summary.push_str("\n  ?? ");
        summary.push_str(path);
    }
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::untracked_summary;

    #[test]
    fn summary_is_none_without_untracked_entries() {
        let status = " M src/main.rs\nA  src/lib.rs\n";
        assert!(untracked_summary(status).is_none());
    }

    #[test]
    fn summary_lists_only_untracked_entries() {
        let status = " M src/main.rs\n?? scratch.tmp\n?? out/\n";
        let summary = untracked_summary(status).expect("expected untracked summary");
        assert!(summary.contains("2 item(s)"));
        assert!(summary.contains("?? scratch.tmp"));
        assert!(summary.contains("?? out/"));
        // Tracked modifications must not appear in the untracked summary.
        assert!(!summary.contains("src/main.rs"));
    }

    #[test]
    fn summary_ignores_blank_untracked_paths() {
        let status = "?? \n?? real-path\n";
        let summary = untracked_summary(status).expect("expected untracked summary");
        assert!(summary.contains("1 item(s)"));
        assert!(summary.contains("?? real-path"));
    }
}
