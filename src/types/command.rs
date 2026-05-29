//! Command type for /xx command system
//!
//! Supports Slack-style command parsing and execution for TUI interface.
//! Commands are entered as /command [args...] in the input box.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Parsed command with arguments
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Command {
    /// Command name (without leading slash)
    pub name: String,

    /// Command arguments
    pub args: Vec<String>,

    /// Named arguments (key-value pairs)
    pub named_args: HashMap<String, String>,

    /// Flags (arguments starting with -- or -)
    pub flags: HashMap<String, Option<String>>,

    /// Raw command string
    pub raw: String,

    /// Command source (user, system, etc.)
    pub source: CommandSource,
}

/// Source of the command
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandSource {
    /// Command from user input
    User,
    /// Command from system (auto-generated)
    System,
    /// Command from script/automation
    Script,
    /// Command from external integration
    External,
    /// Unknown source
    Unknown,
}

/// Command parsing result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandParseResult {
    /// Successfully parsed command
    Success(Command),
    /// Input is not a command (doesn't start with /)
    NotACommand(String),
    /// Invalid command format
    InvalidFormat(String),
    /// Unknown command
    UnknownCommand(String),
    /// Command requires arguments but none provided
    MissingArguments(String),
    /// Command has too many arguments
    TooManyArguments(String),
    /// Command parsing error
    Error(String),
}

/// Command execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandExecutionResult {
    /// Whether command execution was successful
    pub success: bool,

    /// Output message
    pub message: String,

    /// Data returned by command (if any)
    pub data: Option<serde_json::Value>,

    /// Next action to take
    pub next_action: CommandNextAction,

    /// Error details if execution failed
    pub error: Option<CommandError>,
}

/// Next action after command execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandNextAction {
    /// Continue normally (stay in input box)
    Continue,
    /// Switch to a different view/interface
    SwitchTo(String),
    /// Exit current context
    Exit,
    /// Restart command interface
    Restart,
    /// Show help
    ShowHelp,
    /// No action (handled by caller)
    None,
}

/// Command error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandError {
    /// Error type
    pub error_type: CommandErrorType,

    /// Error message
    pub message: String,

    /// Suggested fix (if any)
    pub suggestion: Option<String>,

    /// Whether error is recoverable
    pub recoverable: bool,

    /// Error details
    pub details: Option<serde_json::Value>,
}

/// Command error types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandErrorType {
    /// Command not found
    CommandNotFound,
    /// Invalid arguments
    InvalidArguments,
    /// Missing required arguments
    MissingArguments,
    /// Permission denied
    PermissionDenied,
    /// Resource not found
    ResourceNotFound,
    /// Resource already exists
    ResourceExists,
    /// Invalid state
    InvalidState,
    /// External service error
    ExternalServiceError,
    /// Internal error
    InternalError,
    /// Timeout
    Timeout,
    /// Unknown error
    Unknown,
}

impl Default for CommandSource {
    fn default() -> Self {
        CommandSource::User
    }
}

impl Command {
    /// Parse a command string
    pub fn parse(input: &str) -> CommandParseResult {
        let trimmed = input.trim();

        if !trimmed.starts_with('/') {
            return CommandParseResult::NotACommand(trimmed.to_string());
        }

        let command_part = trimmed[1..].trim();
        if command_part.is_empty() {
            return CommandParseResult::InvalidFormat("Empty command".to_string());
        }

        let parts: Vec<&str> = command_part.split_whitespace().collect();
        if parts.is_empty() {
            return CommandParseResult::InvalidFormat("No command name".to_string());
        }

        let name = parts[0].to_string();
        let mut args = Vec::new();
        let mut named_args = HashMap::new();
        let mut flags = HashMap::new();

        let mut i = 1;
        while i < parts.len() {
            let part = parts[i];

            if part.starts_with("--") {
                let arg_name = part[2..].to_string();

                if i + 1 < parts.len() && !parts[i + 1].starts_with('-') {
                    flags.insert(arg_name, Some(parts[i + 1].to_string()));
                    i += 2;
                } else {
                    flags.insert(arg_name, None);
                    i += 1;
                }
            } else if part.starts_with('-') && part.len() > 1 {
                let arg_name = part[1..].to_string();

                if i + 1 < parts.len() && !parts[i + 1].starts_with('-') {
                    flags.insert(arg_name, Some(parts[i + 1].to_string()));
                    i += 2;
                } else {
                    flags.insert(arg_name, None);
                    i += 1;
                }
            } else if part.contains('=') {
                let mut split = part.splitn(2, '=');
                if let (Some(key), Some(value)) = (split.next(), split.next()) {
                    named_args.insert(key.to_string(), value.to_string());
                }
                i += 1;
            } else {
                args.push(part.to_string());
                i += 1;
            }
        }

        CommandParseResult::Success(Command {
            name,
            args,
            named_args,
            flags,
            raw: trimmed.to_string(),
            source: CommandSource::User,
        })
    }

    /// Check if command has a specific flag
    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.contains_key(flag)
    }

    /// Get flag value
    pub fn get_flag(&self, flag: &str) -> Option<&str> {
        self.flags.get(flag).and_then(|v| v.as_deref())
    }

    /// Get named argument value
    pub fn get_named_arg(&self, name: &str) -> Option<&str> {
        self.named_args.get(name).map(|s| s.as_str())
    }

    /// Get positional argument by index
    pub fn get_arg(&self, index: usize) -> Option<&str> {
        self.args.get(index).map(|s| s.as_str())
    }

    /// Get all positional arguments
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Get number of positional arguments
    pub fn arg_count(&self) -> usize {
        self.args.len()
    }

    /// Create a new command
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: Vec::new(),
            named_args: HashMap::new(),
            flags: HashMap::new(),
            raw: String::new(),
            source: CommandSource::User,
        }
    }

    /// Add a positional argument
    pub fn add_arg(&mut self, arg: impl Into<String>) {
        self.args.push(arg.into());
    }

    /// Add a named argument
    pub fn add_named_arg(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.named_args.insert(name.into(), value.into());
    }

    /// Add a flag
    pub fn add_flag(&mut self, flag: impl Into<String>, value: Option<impl Into<String>>) {
        self.flags.insert(flag.into(), value.map(|v| v.into()));
    }

    /// Set command source
    pub fn with_source(mut self, source: CommandSource) -> Self {
        self.source = source;
        self
    }

    /// Build the raw command string
    pub fn build(&self) -> String {
        let mut parts = vec![format!("/{}", self.name)];

        for arg in &self.args {
            parts.push(arg.clone());
        }

        for (key, value) in &self.named_args {
            parts.push(format!("{}={}", key, value));
        }

        for (flag, value) in &self.flags {
            if let Some(val) = value {
                parts.push(format!("--{} {}", flag, val));
            } else {
                parts.push(format!("--{}", flag));
            }
        }

        parts.join(" ")
    }
}

impl Default for CommandExecutionResult {
    fn default() -> Self {
        Self {
            success: true,
            message: String::new(),
            data: None,
            next_action: CommandNextAction::Continue,
            error: None,
        }
    }
}

impl CommandExecutionResult {
    /// Create a successful result
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            ..Default::default()
        }
    }

    /// Create a successful result with data
    pub fn success_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            success: true,
            message: message.into(),
            data: Some(data),
            ..Default::default()
        }
    }

    /// Create a failure result
    pub fn failure(message: impl Into<String>, error_type: CommandErrorType) -> Self {
        let msg = message.into();
        Self {
            success: false,
            message: msg.clone(),
            error: Some(CommandError {
                error_type,
                message: msg,
                suggestion: None,
                recoverable: true,
                details: None,
            }),
            ..Default::default()
        }
    }

    /// Create a failure result with suggestion
    pub fn failure_with_suggestion(
        message: impl Into<String>,
        error_type: CommandErrorType,
        suggestion: impl Into<String>,
    ) -> Self {
        let msg = message.into();
        Self {
            success: false,
            message: msg.clone(),
            error: Some(CommandError {
                error_type,
                message: msg,
                suggestion: Some(suggestion.into()),
                recoverable: true,
                details: None,
            }),
            ..Default::default()
        }
    }

    /// Set next action
    pub fn with_next_action(mut self, action: CommandNextAction) -> Self {
        self.next_action = action;
        self
    }
}

impl CommandError {
    /// Create a new command error
    pub fn new(error_type: CommandErrorType, message: impl Into<String>) -> Self {
        Self {
            error_type,
            message: message.into(),
            suggestion: None,
            recoverable: true,
            details: None,
        }
    }

    /// Create an unrecoverable error
    pub fn unrecoverable(error_type: CommandErrorType, message: impl Into<String>) -> Self {
        Self {
            error_type,
            message: message.into(),
            suggestion: None,
            recoverable: false,
            details: None,
        }
    }

    /// Add a suggestion
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Add details
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

/// Command handler trait
pub trait CommandHandler {
    /// Check if handler can handle this command
    fn can_handle(&self, command: &Command) -> bool;

    /// Handle the command
    fn handle(&self, command: &Command) -> CommandExecutionResult;

    /// Get command help
    fn help(&self) -> CommandHelp;
}

/// Command help information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandHelp {
    /// Command name
    pub name: String,

    /// Command description
    pub description: String,

    /// Usage examples
    pub usage: Vec<String>,

    /// Available arguments
    pub arguments: Vec<CommandArgument>,

    /// Available flags
    pub flags: Vec<CommandFlag>,

    /// Related commands
    pub related: Vec<String>,
}

/// Command argument definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandArgument {
    /// Argument name
    pub name: String,

    /// Argument description
    pub description: String,

    /// Whether argument is required
    pub required: bool,

    /// Default value (if optional)
    pub default: Option<String>,

    /// Value type
    pub value_type: ArgumentValueType,
}

/// Argument value type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArgumentValueType {
    /// String value
    String,
    /// Integer value
    Integer,
    /// Boolean value
    Boolean,
    /// File path
    FilePath,
    /// Directory path
    DirectoryPath,
    /// URL
    Url,
    /// JSON
    Json,
    /// Custom type
    Custom(String),
}

/// Command flag definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandFlag {
    /// Flag name (without --)
    pub name: String,

    /// Short flag (without -)
    pub short: Option<char>,

    /// Flag description
    pub description: String,

    /// Whether flag takes a value
    pub takes_value: bool,

    /// Default value (if takes_value is true)
    pub default_value: Option<String>,

    /// Value type (if takes_value is true)
    pub value_type: Option<ArgumentValueType>,
}

/// Command registry for managing available commands
#[derive(Default)]
pub struct CommandRegistry {
    /// Registered command handlers
    handlers: HashMap<String, Box<dyn CommandHandler>>,

    /// Command aliases
    aliases: HashMap<String, String>,
}

impl CommandRegistry {
    /// Create a new command registry
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    /// Register a command handler
    pub fn register(&mut self, handler: Box<dyn CommandHandler>) {
        let help = handler.help();
        self.handlers.insert(help.name.clone(), handler);
    }

    /// Add a command alias
    pub fn add_alias(&mut self, alias: impl Into<String>, command: impl Into<String>) {
        self.aliases.insert(alias.into(), command.into());
    }

    /// Handle a command string
    pub fn handle_command(&self, input: &str) -> CommandExecutionResult {
        match Command::parse(input) {
            CommandParseResult::Success(cmd) => self.handle_parsed_command(&cmd),
            CommandParseResult::NotACommand(_) => CommandExecutionResult::failure(
                "Input is not a command",
                CommandErrorType::InvalidArguments,
            ),
            CommandParseResult::InvalidFormat(msg) => CommandExecutionResult::failure(
                format!("Invalid command format: {}", msg),
                CommandErrorType::InvalidArguments,
            ),
            CommandParseResult::UnknownCommand(cmd) => CommandExecutionResult::failure(
                format!("Unknown command: {}", cmd),
                CommandErrorType::CommandNotFound,
            ),
            CommandParseResult::MissingArguments(cmd) => {
                CommandExecutionResult::failure_with_suggestion(
                    format!("Missing arguments for command: {}", cmd),
                    CommandErrorType::MissingArguments,
                    format!("Use /help {} for usage information", cmd),
                )
            }
            CommandParseResult::TooManyArguments(cmd) => CommandExecutionResult::failure(
                format!("Too many arguments for command: {}", cmd),
                CommandErrorType::InvalidArguments,
            ),
            CommandParseResult::Error(msg) => CommandExecutionResult::failure(
                format!("Command parsing error: {}", msg),
                CommandErrorType::InternalError,
            ),
        }
    }

    /// Handle a parsed command
    fn handle_parsed_command(&self, command: &Command) -> CommandExecutionResult {
        let command_name = if let Some(actual_name) = self.aliases.get(&command.name) {
            actual_name
        } else {
            &command.name
        };

        if let Some(handler) = self.handlers.get(command_name) {
            handler.handle(command)
        } else {
            CommandExecutionResult::failure(
                format!("Unknown command: {}", command.name),
                CommandErrorType::CommandNotFound,
            )
        }
    }

    /// Get all available commands
    pub fn available_commands(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Get command help
    pub fn get_help(&self, command_name: &str) -> Option<CommandHelp> {
        let command_name = self
            .aliases
            .get(command_name)
            .map(|s| s.as_str())
            .unwrap_or(command_name);
        self.handlers.get(command_name).map(|h| h.help())
    }

    /// List all command help
    pub fn list_all_help(&self) -> Vec<CommandHelp> {
        self.handlers.values().map(|h| h.help()).collect()
    }
}
