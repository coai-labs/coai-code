use crate::core::{CoAIError, Result, ValidationResult};
use std::path::PathBuf;
use std::process::Command;

pub struct ValidationTools {
    workspace: PathBuf,
}

impl ValidationTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    pub async fn compile(&self) -> Result<ValidationResult> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        if self.workspace.join("Cargo.toml").exists() {
            let output = Command::new("cargo")
                .args(["check", "--all-targets"])
                .current_dir(&self.workspace)
                .output()
                .map_err(|e| CoAIError::Validation(format!("cargo check failed: {}", e)))?;

            let stderr = String::from_utf8_lossy(&output.stderr);

            for line in stderr.lines() {
                if line.contains("error[") {
                    errors.push(line.to_string());
                } else if line.contains("warning[") {
                    warnings.push(line.to_string());
                }
            }

            Ok(ValidationResult {
                passed: output.status.success(),
                errors,
                warnings,
            })
        } else if self.workspace.join("package.json").exists() {
            let output = Command::new("npx")
                .args(["tsc", "--noEmit"])
                .current_dir(&self.workspace)
                .output()
                .map_err(|e| CoAIError::Validation(format!("tsc failed: {}", e)))?;

            let stdout = String::from_utf8_lossy(&output.stdout);

            for line in stdout.lines() {
                if line.contains("error TS") {
                    errors.push(line.to_string());
                }
            }

            Ok(ValidationResult {
                passed: output.status.success(),
                errors,
                warnings,
            })
        } else {
            Ok(ValidationResult {
                passed: true,
                errors,
                warnings,
            })
        }
    }

    pub async fn lint(&self) -> Result<ValidationResult> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        if self.workspace.join("Cargo.toml").exists() {
            let output = Command::new("cargo")
                .args(["clippy", "--all-targets", "--", "-W", "clippy::all"])
                .current_dir(&self.workspace)
                .output()
                .map_err(|e| CoAIError::Validation(format!("cargo clippy failed: {}", e)))?;

            let stderr = String::from_utf8_lossy(&output.stderr);

            for line in stderr.lines() {
                if line.contains("error:") {
                    errors.push(line.to_string());
                } else if line.contains("warning:") {
                    warnings.push(line.to_string());
                }
            }

            Ok(ValidationResult {
                passed: errors.is_empty(),
                errors,
                warnings,
            })
        } else {
            Ok(ValidationResult {
                passed: true,
                errors,
                warnings,
            })
        }
    }

    pub async fn format_check(&self) -> Result<ValidationResult> {
        let mut errors = Vec::new();
        let warnings = Vec::new();

        if self.workspace.join("Cargo.toml").exists() {
            let output = Command::new("cargo")
                .args(["fmt", "--check"])
                .current_dir(&self.workspace)
                .output()
                .map_err(|e| CoAIError::Validation(format!("cargo fmt failed: {}", e)))?;

            if !output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if !line.is_empty() {
                        errors.push(format!("Format violation: {}", line));
                    }
                }
            }

            Ok(ValidationResult {
                passed: errors.is_empty(),
                errors,
                warnings,
            })
        } else {
            Ok(ValidationResult {
                passed: true,
                errors,
                warnings,
            })
        }
    }

    pub async fn test(&self, filter: Option<&str>) -> Result<ValidationResult> {
        let mut errors = Vec::new();
        let warnings = Vec::new();

        if self.workspace.join("Cargo.toml").exists() {
            let mut args = vec!["test", "--no-fail-fast"];
            if let Some(f) = filter {
                args.push(f);
            }

            let output = Command::new("cargo")
                .args(&args)
                .current_dir(&self.workspace)
                .output()
                .map_err(|e| CoAIError::Validation(format!("cargo test failed: {}", e)))?;

            let stdout = String::from_utf8_lossy(&output.stdout);

            for line in stdout.lines() {
                if line.contains("FAILED") || line.contains("test result: FAILED") {
                    errors.push(line.to_string());
                }
            }

            Ok(ValidationResult {
                passed: output.status.success() && errors.is_empty(),
                errors,
                warnings,
            })
        } else {
            Ok(ValidationResult {
                passed: true,
                errors,
                warnings,
            })
        }
    }
}
