use crate::core::{CoAIError, CommandOutput, Result};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub struct ExecTools {
    workspace: PathBuf,
    timeout_secs: u64,
}

impl ExecTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            timeout_secs: 300,
        }
    }

    pub async fn run(&self, command: &str, cwd: Option<&str>) -> Result<CommandOutput> {
        let cwd = self.resolve_cwd(cwd)?;
        let output = tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            self.run_inner(command, &cwd),
        )
        .await
        .map_err(|_| {
            CoAIError::Command(format!("Command timed out ({}s): {}", self.timeout_secs, command))
        })??;

        Ok(output)
    }

    async fn run_program(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput> {
        let display = display_command(program, args);
        let output = tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            self.run_program_inner(program, args, cwd),
        )
        .await
        .map_err(|_| {
            CoAIError::Command(format!("Command timed out ({}s): {}", self.timeout_secs, display))
        })??;

        Ok(output)
    }

    async fn run_inner(&self, command: &str, cwd: &Path) -> Result<CommandOutput> {
        // Detach stdin so child processes can't read from — or reconfigure
        // (e.g. disable raw mode on) — the TUI's controlling terminal.
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| CoAIError::Command(format!("Failed to execute command {}: {}", command, e)))?;

        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            success: output.status.success(),
        })
    }

    async fn run_program_inner(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
    ) -> Result<CommandOutput> {
        let output = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| {
                CoAIError::Command(format!(
                    "Failed to execute command {}: {}",
                    display_command(program, args),
                    e
                ))
            })?;

        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            success: output.status.success(),
        })
    }

    pub async fn build(&self, cwd: Option<&str>) -> Result<CommandOutput> {
        let cwd = self.resolve_cwd(cwd)?;
        if cwd.join("Cargo.toml").exists() {
            self.run_program("cargo", &["build"], &cwd).await
        } else if cwd.join("package.json").exists() {
            self.run_program("npm", &["run", "build"], &cwd).await
        } else if cwd.join("go.mod").exists() {
            self.run_program("go", &["build"], &cwd).await
        } else if cwd.join("Makefile").exists() {
            self.run_program("make", &[], &cwd).await
        } else {
            Err(CoAIError::Command("Unrecognized project type".to_string()))
        }
    }

    pub async fn test(&self, filter: Option<&str>, cwd: Option<&str>) -> Result<CommandOutput> {
        let cwd = self.resolve_cwd(cwd)?;
        if cwd.join("Cargo.toml").exists() {
            match filter {
                Some(f) => self.run_program("cargo", &["test", f], &cwd).await,
                None => self.run_program("cargo", &["test"], &cwd).await,
            }
        } else if cwd.join("package.json").exists() {
            match filter {
                Some(f) => self.run_program("npm", &["test", "--", f], &cwd).await,
                None => self.run_program("npm", &["test"], &cwd).await,
            }
        } else if cwd.join("go.mod").exists() {
            match filter {
                Some(f) => self.run_program("go", &["test", "-run", f], &cwd).await,
                None => self.run_program("go", &["test", "./..."], &cwd).await,
            }
        } else {
            Err(CoAIError::Command("Unrecognized project type".to_string()))
        }
    }

    pub async fn install(&self, cwd: Option<&str>) -> Result<CommandOutput> {
        let cwd = self.resolve_cwd(cwd)?;
        if cwd.join("Cargo.toml").exists() {
            self.run_program("cargo", &["fetch"], &cwd).await
        } else if cwd.join("package.json").exists() {
            self.run_program("npm", &["install"], &cwd).await
        } else if cwd.join("go.mod").exists() {
            self.run_program("go", &["mod", "download"], &cwd).await
        } else if cwd.join("requirements.txt").exists() {
            self.run_program("pip", &["install", "-r", "requirements.txt"], &cwd)
                .await
        } else {
            Err(CoAIError::Command("Unrecognized project type".to_string()))
        }
    }

    fn resolve_cwd(&self, cwd: Option<&str>) -> Result<PathBuf> {
        let workspace = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| normalize_path(&self.workspace));
        let Some(cwd) = cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) else {
            return Ok(workspace);
        };

        let requested = Path::new(cwd);
        let full_path = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            workspace.join(requested)
        };
        let normalized = normalize_path(&full_path);
        if !normalized.starts_with(&workspace) {
            return Err(CoAIError::Security(format!("cwd is outside the working directory: {}", cwd)));
        }

        let canonical = full_path
            .canonicalize()
            .map_err(|e| CoAIError::File(format!("Failed to resolve cwd {}: {}", cwd, e)))?;
        if !canonical.starts_with(&workspace) {
            return Err(CoAIError::Security(format!("cwd is outside the working directory: {}", cwd)));
        }
        if !canonical.is_dir() {
            return Err(CoAIError::Command(format!("cwd is not a directory: {}", cwd)));
        }

        Ok(canonical)
    }
}

fn display_command(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .map(shell_display_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_display_quote(arg: &str) -> String {
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_command_quotes_shell_metacharacters() {
        let rendered = display_command("cargo", &["test", "ok; touch /tmp/pwned"]);
        assert_eq!(rendered, "cargo test 'ok; touch /tmp/pwned'");
    }

    #[test]
    fn resolve_cwd_accepts_workspace_subdir() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("crate-a")).unwrap();
        let exec = ExecTools::new(workspace.path());

        let cwd = exec.resolve_cwd(Some("crate-a")).unwrap();

        assert_eq!(
            cwd,
            workspace.path().join("crate-a").canonicalize().unwrap()
        );
    }

    #[test]
    fn resolve_cwd_rejects_escape() {
        let workspace = tempfile::tempdir().unwrap();
        let exec = ExecTools::new(workspace.path());

        let err = exec.resolve_cwd(Some("..")).unwrap_err();

        assert!(matches!(err, CoAIError::Security(_)));
    }
}
