//! Configuration module for CoAI Code
//!
//! Loads configuration from coai.toml files and provides a unified Config struct.
//! Follows the "trust LLM" principle - configuration is simple and non-prescriptive.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Main configuration structure for CoAI Code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// LLM configuration section
    #[serde(default)]
    pub llm: LlmConfig,

    /// Agent configuration section
    #[serde(default)]
    pub agent: AgentConfig,

    /// TUI configuration section
    #[serde(default)]
    pub tui: TuiConfig,

    /// Workspace configuration
    #[serde(default)]
    pub workspace: WorkspaceConfig,

    /// Additional custom configuration sections
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}

/// LLM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Default LLM provider to use
    #[serde(default = "default_llm_provider")]
    pub default_provider: String,

    /// Available LLM providers configuration
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Model parameters
    #[serde(default)]
    pub parameters: LlmParameters,
}

/// Individual LLM provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider type (deepseek, openai, anthropic, ollama, openai_compatible, custom)
    pub provider: String,

    /// Model name/identifier
    pub model: String,

    /// Lighter model for simple tasks (e.g. "deepseek-v4-flash" for Pro).
    #[serde(default)]
    pub flash_model: Option<String>,

    /// API key (can use environment variable syntax like ${VAR_NAME})
    #[serde(default)]
    pub api_key: Option<String>,

    /// Base URL for API (for custom or openai_compatible providers)
    #[serde(default)]
    pub base_url: Option<String>,

    /// Temperature for generation
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// Maximum tokens to generate
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,

    /// Additional provider-specific parameters
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// LLM generation parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmParameters {
    /// Default temperature
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// Default max tokens
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,

    /// Whether to use system prompt
    #[serde(default = "default_true")]
    pub use_system_prompt: bool,

    /// Default system prompt (if use_system_prompt is true)
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// Agent behavior configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Maximum number of tool iterations per task
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: usize,

    /// Timeout for tool execution in seconds
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_seconds: u64,

    /// Context window size in tokens
    #[serde(default = "default_context_window")]
    pub context_window: usize,

    /// Whether to enable automatic task decomposition
    /// Follows "trust LLM" principle - simple flag, LLM decides how to use it
    #[serde(default = "default_true")]
    pub enable_task_decomposition: bool,

    /// Whether to enable automatic validation
    #[serde(default = "default_true")]
    pub enable_validation: bool,
}

/// TUI (Terminal User Interface) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Whether TUI is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// UI theme (light, dark, auto)
    #[serde(default = "default_theme")]
    pub theme: String,

    /// Whether to show progress bars
    #[serde(default = "default_true")]
    pub show_progress: bool,

    /// Whether to show detailed logs
    #[serde(default = "default_false")]
    pub detailed_logs: bool,

    /// Refresh rate in milliseconds
    #[serde(default = "default_refresh_rate")]
    pub refresh_rate_ms: u64,

    /// Keybindings configuration
    #[serde(default)]
    pub keybindings: HashMap<String, String>,
}

/// Workspace configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Default workspace directory
    #[serde(default = "default_workspace_dir")]
    pub directory: PathBuf,

    /// Auto-save interval in seconds (0 = disabled)
    #[serde(default = "default_auto_save_interval")]
    pub auto_save_interval: u64,

    /// Maximum number of backup files to keep
    #[serde(default = "default_max_backups")]
    pub max_backups: usize,

    /// File patterns to ignore in workspace scans
    #[serde(default)]
    pub ignore_patterns: Vec<String>,
}

// Default values

fn default_llm_provider() -> String {
    "deepseek".to_string()
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tokens() -> usize {
    64000
}

fn default_max_tool_iterations() -> usize {
    // L/XL coding tasks routinely need: read several files, make multiple edits,
    // then run verification a few times. 50 was too tight for cross-file work.
    80
}

fn default_tool_timeout() -> u64 {
    300
}

fn default_context_window() -> usize {
    1_000_000
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_theme() -> String {
    "auto".to_string()
}

fn default_refresh_rate() -> u64 {
    100
}

fn default_workspace_dir() -> PathBuf {
    PathBuf::from(".")
}

fn default_auto_save_interval() -> u64 {
    60 // 1 minute
}

fn default_max_backups() -> usize {
    10
}

// Default implementations

impl Default for Config {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            agent: AgentConfig::default(),
            tui: TuiConfig::default(),
            workspace: WorkspaceConfig::default(),
            custom: HashMap::new(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            default_provider: default_llm_provider(),
            providers: HashMap::new(),
            parameters: LlmParameters::default(),
        }
    }
}

impl Default for LlmParameters {
    fn default() -> Self {
        Self {
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            use_system_prompt: default_true(),
            system_prompt: None,
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_tool_iterations: default_max_tool_iterations(),
            tool_timeout_seconds: default_tool_timeout(),
            context_window: default_context_window(),
            enable_task_decomposition: default_true(),
            enable_validation: default_true(),
        }
    }
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            theme: default_theme(),
            show_progress: default_true(),
            detailed_logs: default_false(),
            refresh_rate_ms: default_refresh_rate(),
            keybindings: HashMap::new(),
        }
    }
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            directory: default_workspace_dir(),
            auto_save_interval: default_auto_save_interval(),
            max_backups: default_max_backups(),
            ignore_patterns: Vec::new(),
        }
    }
}

/// Load configuration from a TOML file
pub fn load_from_file(path: &Path) -> Result<Config, config::ConfigError> {
    let config = config::Config::builder()
        .add_source(config::File::from(path))
        .add_source(config::Environment::with_prefix("COAI"))
        .build()?;

    config.try_deserialize()
}

/// Load configuration from default locations
/// Searches in: current directory, user config directory, system config directory
pub fn load() -> Result<Config, config::ConfigError> {
    // First, try to find a config file
    let mut config_paths: Vec<PathBuf> = vec![PathBuf::from("coai.toml")];

    if let Some(home) = dirs::home_dir() {
        config_paths.push(home.join(".coai").join("coai.toml"));
    }

    // macOS standard config directory
    if let Some(config_dir) = dirs::config_dir() {
        config_paths.push(config_dir.join("coai").join("coai.toml"));
    }

    // XDG config directory (common on Linux and some macOS setups)
    if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
        config_paths.push(PathBuf::from(xdg_config).join("coai").join("coai.toml"));
    } else if let Some(home) = dirs::home_dir() {
        // Fallback to ~/.config/coai/coai.toml
        config_paths.push(home.join(".config").join("coai").join("coai.toml"));
    }

    // System config directory
    config_paths.push(PathBuf::from("/etc/coai/coai.toml"));

    // Find the first existing config file
    let config_file = config_paths.into_iter().find(|p| p.exists() && p.is_file());

    let mut config = Config::default();

    if let Some(path) = config_file {
        // Parse TOML directly
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(parsed) = toml::from_str::<Config>(&content) {
                config = parsed;
            }
        }
    }

    // Override with environment variables
    if let Ok(provider) = std::env::var("COAI_LLM_DEFAULT_PROVIDER") {
        config.llm.default_provider = provider;
    }

    Ok(config)
}

pub fn llm_config_from_provider(provider_cfg: &ProviderConfig) -> Option<crate::llm::LLMConfig> {
    let api_key = provider_cfg.api_key.as_deref().map(resolve_env_value);
    let base_url = provider_cfg.base_url.as_deref().map(resolve_env_value);

    let mut cfg = match provider_cfg.provider.as_str() {
        "deepseek" => {
            let mut cfg =
                crate::llm::LLMConfig::anthropic(&provider_cfg.model, api_key.unwrap_or_default());
            cfg.base_url =
                Some(base_url.unwrap_or_else(|| "https://api.deepseek.com/anthropic".to_string()));
            cfg
        }
        "openai" => crate::llm::LLMConfig::openai(&provider_cfg.model, api_key.unwrap_or_default()),
        "anthropic" => {
            crate::llm::LLMConfig::anthropic(&provider_cfg.model, api_key.unwrap_or_default())
        }
        "openai_compatible" => crate::llm::LLMConfig::openai_compatible(
            &provider_cfg.model,
            base_url.unwrap_or_default(),
            api_key,
        ),
        "ollama" => crate::llm::LLMConfig::ollama(
            &provider_cfg.model,
            base_url.unwrap_or_else(|| "http://localhost:11434".to_string()),
        ),
        _ => return None,
    };

    cfg.temperature = provider_cfg.temperature;
    cfg.max_tokens = provider_cfg.max_tokens;
    cfg.flash_model = provider_cfg.flash_model.clone();
    cfg.extra_params = provider_cfg.extra.clone();
    crate::llm::model_caps::apply_deepseek_v4_profile(&mut cfg);
    Some(cfg)
}

fn resolve_env_value(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(name) = trimmed
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
    {
        std::env::var(name).unwrap_or_default()
    } else if let Some(name) = trimmed.strip_prefix('$') {
        std::env::var(name).unwrap_or_default()
    } else {
        value.to_string()
    }
}

/// Configuration error type
pub type ConfigError = config::ConfigError;
