//! Command parser and handlers for CoAI Code's /xx command system
//!
//! Provides Slack-style command parsing and execution for TUI interface.
//! Commands are entered as /command [args...] in the input box.

use crate::types::command::{
    ArgumentValueType, Command, CommandArgument, CommandErrorType, CommandExecutionResult,
    CommandFlag, CommandHandler, CommandHelp, CommandNextAction, CommandParseResult,
    CommandRegistry,
};

/// Command parser for CoAI Code
pub struct CommandParser {
    registry: CommandRegistry,
}

impl CommandParser {
    pub fn new() -> Self {
        let mut parser = Self {
            registry: CommandRegistry::new(),
        };
        parser.register_default_commands();
        parser
    }

    fn register_default_commands(&mut self) {
        self.registry.register(Box::new(TaskCommandHandler));
        self.registry.register(Box::new(ConfigCommandHandler));
        self.registry.register(Box::new(ToolsCommandHandler));
        self.registry.register(Box::new(HistoryCommandHandler));
        self.registry.register(Box::new(MemoryCommandHandler));
        self.registry.register(Box::new(SkillsCommandHandler));
        self.registry.register(Box::new(HelpCommandHandler));

        self.registry.add_alias("t", "task");
        self.registry.add_alias("c", "config");
        self.registry.add_alias("m", "memory");
        self.registry.add_alias("sk", "skills");
        self.registry.add_alias("h", "help");
    }

    pub fn execute(&self, input: &str) -> CommandExecutionResult {
        self.registry.handle_command(input)
    }

    pub fn parse(&self, input: &str) -> CommandParseResult {
        Command::parse(input)
    }

    pub fn get_help(&self, command: &str) -> Option<CommandHelp> {
        self.registry.get_help(command)
    }

    pub fn available_commands(&self) -> Vec<String> {
        self.registry.available_commands()
    }

    pub fn list_all_help(&self) -> Vec<CommandHelp> {
        self.registry.list_all_help()
    }
}

impl Default for CommandParser {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// /task handler
// ---------------------------------------------------------------------------

struct TaskCommandHandler;

impl CommandHandler for TaskCommandHandler {
    fn can_handle(&self, command: &Command) -> bool {
        command.name == "task"
    }

    fn handle(&self, command: &Command) -> CommandExecutionResult {
        let sub = command.get_arg(0).unwrap_or("list");
        match sub {
            "create" => {
                let desc = match command.get_arg(1) {
                    Some(d) => d,
                    None => {
                        return CommandExecutionResult::failure(
                            "Task description required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                let priority = command.get_flag("priority").unwrap_or("medium");
                CommandExecutionResult::success_with_data(
                    format!("Task created: {}", desc),
                    serde_json::json!({
                        "id": uuid::Uuid::new_v4().to_string(),
                        "description": desc,
                        "priority": priority,
                        "status": "pending"
                    }),
                )
            }
            "list" => {
                let limit = command
                    .get_flag("limit")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(10);
                CommandExecutionResult::success_with_data(
                    format!("Listing up to {} tasks", limit),
                    serde_json::json!({
                        "tasks": [],
                        "total": 0,
                        "limit": limit,
                        "status_filter": command.get_flag("status")
                    }),
                )
            }
            "show" => {
                let id = match command.get_arg(1) {
                    Some(i) => i,
                    None => {
                        return CommandExecutionResult::failure(
                            "Task ID required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Task: {}", id),
                    serde_json::json!({ "id": id, "status": "pending" }),
                )
            }
            "cancel" => {
                let id = match command.get_arg(1) {
                    Some(i) => i,
                    None => {
                        return CommandExecutionResult::failure(
                            "Task ID required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success(format!("Task cancelled: {}", id))
            }
            "status" => {
                let id = match command.get_arg(1) {
                    Some(i) => i,
                    None => {
                        return CommandExecutionResult::failure(
                            "Task ID required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Status: {}", id),
                    serde_json::json!({
                        "id": id,
                        "status": "in_progress",
                        "progress": 0,
                    }),
                )
            }
            _ => CommandExecutionResult::failure(
                format!("Unknown subcommand: {}", sub),
                CommandErrorType::InvalidArguments,
            )
            .with_next_action(CommandNextAction::ShowHelp),
        }
    }

    fn help(&self) -> CommandHelp {
        CommandHelp {
            name: "task".into(),
            description: "Manage tasks".into(),
            usage: vec![
                "/task create \"description\" --priority high".into(),
                "/task list --status pending".into(),
                "/task show <id>".into(),
                "/task cancel <id>".into(),
            ],
            arguments: vec![CommandArgument {
                name: "subcommand".into(),
                description: "create, list, show, cancel, status".into(),
                required: true,
                default: None,
                value_type: ArgumentValueType::String,
            }],
            flags: vec![
                CommandFlag {
                    name: "priority".into(),
                    short: Some('p'),
                    description: "Task priority (low/medium/high)".into(),
                    takes_value: true,
                    default_value: Some("medium".into()),
                    value_type: Some(ArgumentValueType::String),
                },
                CommandFlag {
                    name: "status".into(),
                    short: Some('s'),
                    description: "Filter by status".into(),
                    takes_value: true,
                    default_value: None,
                    value_type: Some(ArgumentValueType::String),
                },
                CommandFlag {
                    name: "limit".into(),
                    short: Some('l'),
                    description: "Max results".into(),
                    takes_value: true,
                    default_value: Some("10".into()),
                    value_type: Some(ArgumentValueType::Integer),
                },
            ],
            related: vec!["history".into()],
        }
    }
}

// ---------------------------------------------------------------------------
// /config handler
// ---------------------------------------------------------------------------

struct ConfigCommandHandler;

impl CommandHandler for ConfigCommandHandler {
    fn can_handle(&self, command: &Command) -> bool {
        command.name == "config"
    }

    fn handle(&self, command: &Command) -> CommandExecutionResult {
        let sub = command.get_arg(0).unwrap_or("show");
        match sub {
            "show" => CommandExecutionResult::success_with_data(
                "Current config",
                serde_json::json!({
                    "llm.provider": "openai",
                    "llm.model": "gpt-4",
                    "tui.theme": "dark"
                }),
            ),
            "set" => {
                let key = command.get_arg(1);
                let val = command.get_arg(2);
                match (key, val) {
                    (Some(k), Some(v)) => {
                        CommandExecutionResult::success(format!("Set {} = {}", k, v))
                    }
                    _ => CommandExecutionResult::failure(
                        "Usage: /config set <key> <value>",
                        CommandErrorType::MissingArguments,
                    )
                    .with_next_action(CommandNextAction::ShowHelp),
                }
            }
            "get" => {
                let key = match command.get_arg(1) {
                    Some(k) => k,
                    None => {
                        return CommandExecutionResult::failure(
                            "Key required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Config: {}", key),
                    serde_json::json!({ "key": key, "value": null }),
                )
            }
            "list" => CommandExecutionResult::success_with_data(
                "Config keys",
                serde_json::json!({
                    "keys": [
                        "llm.provider", "llm.model", "llm.temperature",
                        "agent.max_tokens", "agent.timeout",
                        "tui.theme", "tui.keybindings"
                    ]
                }),
            ),
            _ => CommandExecutionResult::failure(
                format!("Unknown subcommand: {}", sub),
                CommandErrorType::InvalidArguments,
            )
            .with_next_action(CommandNextAction::ShowHelp),
        }
    }

    fn help(&self) -> CommandHelp {
        CommandHelp {
            name: "config".into(),
            description: "Manage configuration".into(),
            usage: vec![
                "/config show".into(),
                "/config set key value".into(),
                "/config get key".into(),
                "/config list".into(),
            ],
            arguments: vec![CommandArgument {
                name: "subcommand".into(),
                description: "show, set, get, list".into(),
                required: true,
                default: None,
                value_type: ArgumentValueType::String,
            }],
            flags: vec![
                CommandFlag {
                    name: "global".into(),
                    short: Some('g'),
                    description: "Global config".into(),
                    takes_value: false,
                    default_value: None,
                    value_type: None,
                },
                CommandFlag {
                    name: "local".into(),
                    short: Some('l'),
                    description: "Local config".into(),
                    takes_value: false,
                    default_value: None,
                    value_type: None,
                },
            ],
            related: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// /tools handler
// ---------------------------------------------------------------------------

struct ToolsCommandHandler;

impl CommandHandler for ToolsCommandHandler {
    fn can_handle(&self, command: &Command) -> bool {
        command.name == "tools"
    }

    fn handle(&self, command: &Command) -> CommandExecutionResult {
        let sub = command.get_arg(0).unwrap_or("list");
        match sub {
            "list" => CommandExecutionResult::success_with_data(
                "Available tools",
                serde_json::json!({
                    "tools": [
                        {"name": "file_read", "description": "Read file contents"},
                        {"name": "file_write", "description": "Write file contents"},
                        {"name": "file_list", "description": "List directory"},
                        {"name": "search", "description": "Search files"},
                        {"name": "exec", "description": "Execute command"},
                        {"name": "validate", "description": "Validate output"}
                    ]
                }),
            ),
            "exec" => {
                let tool = match command.get_arg(1) {
                    Some(t) => t,
                    None => {
                        return CommandExecutionResult::failure(
                            "Tool name required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Execute tool: {}", tool),
                    serde_json::json!({ "tool": tool, "status": "executed" }),
                )
            }
            "info" => {
                let tool = match command.get_arg(1) {
                    Some(t) => t,
                    None => {
                        return CommandExecutionResult::failure(
                            "Tool name required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Tool info: {}", tool),
                    serde_json::json!({ "tool": tool, "params": [] }),
                )
            }
            "search" => {
                let query = match command.get_arg(1) {
                    Some(t) => t,
                    None => {
                        return CommandExecutionResult::failure(
                            "Search query required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Tool search: {}", query),
                    serde_json::json!({ "query": query, "tools": [] }),
                )
            }
            _ => CommandExecutionResult::failure(
                format!("Unknown subcommand: {}", sub),
                CommandErrorType::InvalidArguments,
            )
            .with_next_action(CommandNextAction::ShowHelp),
        }
    }

    fn help(&self) -> CommandHelp {
        CommandHelp {
            name: "tools".into(),
            description: "Manage and execute tools".into(),
            usage: vec![
                "/tools list".into(),
                "/tools search <query>".into(),
                "/tools exec <name> [args...]".into(),
                "/tools info <name>".into(),
            ],
            arguments: vec![CommandArgument {
                name: "subcommand".into(),
                description: "list, search, exec, info".into(),
                required: true,
                default: None,
                value_type: ArgumentValueType::String,
            }],
            flags: vec![],
            related: vec!["task".into()],
        }
    }
}

// ---------------------------------------------------------------------------
// /history handler
// ---------------------------------------------------------------------------

struct HistoryCommandHandler;

impl CommandHandler for HistoryCommandHandler {
    fn can_handle(&self, command: &Command) -> bool {
        command.name == "history"
    }

    fn handle(&self, command: &Command) -> CommandExecutionResult {
        let sub = command.get_arg(0).unwrap_or("list");
        match sub {
            "list" => {
                let limit = command
                    .get_flag("limit")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(10);
                CommandExecutionResult::success_with_data(
                    format!("History ({} most recent)", limit),
                    serde_json::json!({ "records": [], "limit": limit }),
                )
            }
            "search" | "query" => {
                let keyword = match command.get_arg(1) {
                    Some(k) => k,
                    None => {
                        return CommandExecutionResult::failure(
                            "Keyword required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                let limit = command
                    .get_flag("limit")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(10);
                CommandExecutionResult::success_with_data(
                    format!("Search history: {}", keyword),
                    serde_json::json!({ "records": [], "query": keyword, "limit": limit }),
                )
            }
            "show" => {
                let id = match command.get_arg(1) {
                    Some(i) => i,
                    None => {
                        return CommandExecutionResult::failure(
                            "Record ID required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Record: {}", id),
                    serde_json::json!({ "id": id }),
                )
            }
            "export" => {
                let fmt = command.get_flag("format").unwrap_or("json");
                CommandExecutionResult::success_with_data(
                    format!("Exporting as {}", fmt),
                    serde_json::json!({ "format": fmt, "data": "" }),
                )
            }
            "clear" => CommandExecutionResult::success("History cleared"),
            _ => CommandExecutionResult::failure(
                format!("Unknown subcommand: {}", sub),
                CommandErrorType::InvalidArguments,
            )
            .with_next_action(CommandNextAction::ShowHelp),
        }
    }

    fn help(&self) -> CommandHelp {
        CommandHelp {
            name: "history".into(),
            description: "View and manage task history".into(),
            usage: vec![
                "/history list --limit 20".into(),
                "/history search <keyword> --limit 20".into(),
                "/history show <id>".into(),
                "/history export --format json".into(),
                "/history clear".into(),
            ],
            arguments: vec![CommandArgument {
                name: "subcommand".into(),
                description: "list, search, show, export, clear".into(),
                required: true,
                default: None,
                value_type: ArgumentValueType::String,
            }],
            flags: vec![
                CommandFlag {
                    name: "limit".into(),
                    short: Some('l'),
                    description: "Max records".into(),
                    takes_value: true,
                    default_value: Some("10".into()),
                    value_type: Some(ArgumentValueType::Integer),
                },
                CommandFlag {
                    name: "format".into(),
                    short: Some('f'),
                    description: "Export format (json/md/csv)".into(),
                    takes_value: true,
                    default_value: Some("json".into()),
                    value_type: Some(ArgumentValueType::String),
                },
            ],
            related: vec!["task".into()],
        }
    }
}

// ---------------------------------------------------------------------------
// /memory handler
// ---------------------------------------------------------------------------

struct MemoryCommandHandler;

impl CommandHandler for MemoryCommandHandler {
    fn can_handle(&self, command: &Command) -> bool {
        command.name == "memory"
    }

    fn handle(&self, command: &Command) -> CommandExecutionResult {
        let sub = command.get_arg(0).unwrap_or("read");
        match sub {
            "read" => CommandExecutionResult::success_with_data(
                "Project memory",
                serde_json::json!({ "path": ".coai/memory.md" }),
            ),
            "search" => {
                let query = match command.get_arg(1) {
                    Some(q) => q,
                    None => {
                        return CommandExecutionResult::failure(
                            "Query required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Search memory: {}", query),
                    serde_json::json!({ "query": query, "matches": [] }),
                )
            }
            "sections" => CommandExecutionResult::success_with_data(
                "Memory sections",
                serde_json::json!({ "sections": [] }),
            ),
            "append" => {
                let content = match command.get_arg(1) {
                    Some(content) => content,
                    None => {
                        return CommandExecutionResult::failure(
                            "Content required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    "Memory append",
                    serde_json::json!({
                        "path": ".coai/memory.md",
                        "content": content,
                        "section": command.get_flag("section")
                    }),
                )
            }
            "delete" => CommandExecutionResult::success_with_data(
                "Memory delete",
                serde_json::json!({
                    "path": ".coai/memory.md",
                    "line": command.get_flag("line"),
                    "section": command.get_flag("section")
                }),
            ),
            "edit" => CommandExecutionResult::success_with_data(
                "Memory edit",
                serde_json::json!({ "path": ".coai/memory.md" }),
            ),
            "write" => {
                let content = match command.get_arg(1) {
                    Some(content) => content,
                    None => {
                        return CommandExecutionResult::failure(
                            "Content required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    "Memory write",
                    serde_json::json!({ "path": ".coai/memory.md", "content": content }),
                )
            }
            "clear" => CommandExecutionResult::success("Memory cleared"),
            _ => CommandExecutionResult::failure(
                format!("Unknown subcommand: {}", sub),
                CommandErrorType::InvalidArguments,
            )
            .with_next_action(CommandNextAction::ShowHelp),
        }
    }

    fn help(&self) -> CommandHelp {
        CommandHelp {
            name: "memory".into(),
            description: "Manage explicit project memory".into(),
            usage: vec![
                "/memory read".into(),
                "/memory search <query>".into(),
                "/memory sections".into(),
                "/memory append \"content\" --section Notes".into(),
                "/memory delete --line 8".into(),
                "/memory delete --section Notes".into(),
                "/memory edit".into(),
                "/memory write \"full content\"".into(),
                "/memory clear".into(),
            ],
            arguments: vec![CommandArgument {
                name: "subcommand".into(),
                description: "read, search, sections, append, delete, edit, write, clear".into(),
                required: true,
                default: None,
                value_type: ArgumentValueType::String,
            }],
            flags: vec![
                CommandFlag {
                    name: "section".into(),
                    short: Some('s'),
                    description: "Memory section for append/delete".into(),
                    takes_value: true,
                    default_value: Some("Notes".into()),
                    value_type: Some(ArgumentValueType::String),
                },
                CommandFlag {
                    name: "line".into(),
                    short: Some('l'),
                    description: "Memory line number for delete".into(),
                    takes_value: true,
                    default_value: None,
                    value_type: Some(ArgumentValueType::Integer),
                },
            ],
            related: vec!["history".into(), "tools".into()],
        }
    }
}

// ---------------------------------------------------------------------------
// /skills handler
// ---------------------------------------------------------------------------

struct SkillsCommandHandler;

impl CommandHandler for SkillsCommandHandler {
    fn can_handle(&self, command: &Command) -> bool {
        command.name == "skills"
    }

    fn handle(&self, command: &Command) -> CommandExecutionResult {
        let sub = command.get_arg(0).unwrap_or("list");
        match sub {
            "list" => CommandExecutionResult::success_with_data(
                "Available skills",
                serde_json::json!({ "skills": [] }),
            ),
            "search" => {
                let query = match command.get_arg(1) {
                    Some(q) => q,
                    None => {
                        return CommandExecutionResult::failure(
                            "Query required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Search skills: {}", query),
                    serde_json::json!({ "query": query, "skills": [] }),
                )
            }
            "read" | "show" => {
                let name = match command.get_arg(1) {
                    Some(name) => name,
                    None => {
                        return CommandExecutionResult::failure(
                            "Skill name required",
                            CommandErrorType::MissingArguments,
                        )
                        .with_next_action(CommandNextAction::ShowHelp);
                    }
                };
                CommandExecutionResult::success_with_data(
                    format!("Read skill: {}", name),
                    serde_json::json!({ "name": name }),
                )
            }
            _ => CommandExecutionResult::failure(
                format!("Unknown subcommand: {}", sub),
                CommandErrorType::InvalidArguments,
            )
            .with_next_action(CommandNextAction::ShowHelp),
        }
    }

    fn help(&self) -> CommandHelp {
        CommandHelp {
            name: "skills".into(),
            description: "List and read Claude/Codex compatible skills".into(),
            usage: vec![
                "/skills list".into(),
                "/skills search <query>".into(),
                "/skills read <name-or-path>".into(),
            ],
            arguments: vec![CommandArgument {
                name: "subcommand".into(),
                description: "list, search, read".into(),
                required: true,
                default: None,
                value_type: ArgumentValueType::String,
            }],
            flags: vec![],
            related: vec!["tools".into(), "memory".into()],
        }
    }
}

// ---------------------------------------------------------------------------
// /help handler
// ---------------------------------------------------------------------------

struct HelpCommandHandler;

impl CommandHandler for HelpCommandHandler {
    fn can_handle(&self, command: &Command) -> bool {
        command.name == "help"
    }

    fn handle(&self, command: &Command) -> CommandExecutionResult {
        let target = command.get_arg(0);
        match target {
            Some(cmd_name) => {
                let parser = CommandParser::new();
                match parser.get_help(cmd_name) {
                    Some(help) => {
                        let mut msg = format!("**/{}** - {}\n\n", help.name, help.description);
                        if !help.usage.is_empty() {
                            msg.push_str("Usage:\n");
                            for u in &help.usage {
                                msg.push_str(&format!("  {}\n", u));
                            }
                            msg.push('\n');
                        }
                        if !help.flags.is_empty() {
                            msg.push_str("Flags:\n");
                            for f in &help.flags {
                                let short =
                                    f.short.map(|c| format!("-{}, ", c)).unwrap_or_default();
                                msg.push_str(&format!(
                                    "  --{}{}\t{}\n",
                                    f.name, short, f.description
                                ));
                            }
                        }
                        if !help.related.is_empty() {
                            msg.push_str(&format!("\nRelated: {}\n", help.related.join(", ")));
                        }
                        CommandExecutionResult::success(msg)
                    }
                    None => CommandExecutionResult::failure(
                        format!("Unknown command: {}", cmd_name),
                        CommandErrorType::CommandNotFound,
                    ),
                }
            }
            None => {
                let parser = CommandParser::new();
                let cmds = parser.available_commands();
                let mut msg = String::from("Available commands:\n\n");
                for cmd in &cmds {
                    msg.push_str(&format!("  /{}\n", cmd));
                }
                msg.push_str("\nType /help <command> for details.\n");
                CommandExecutionResult::success(msg)
            }
        }
    }

    fn help(&self) -> CommandHelp {
        CommandHelp {
            name: "help".into(),
            description: "Show help".into(),
            usage: vec!["/help".into(), "/help task".into()],
            arguments: vec![CommandArgument {
                name: "command".into(),
                description: "Command to get help for".into(),
                required: false,
                default: None,
                value_type: ArgumentValueType::String,
            }],
            flags: vec![],
            related: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_task_command() {
        let parser = CommandParser::new();
        let result = parser.execute("/task list --limit 5");
        assert!(result.success);
    }

    #[test]
    fn test_not_a_command() {
        let parser = CommandParser::new();
        let result = parser.execute("hello world");
        assert!(!result.success);
    }

    #[test]
    fn test_unknown_command() {
        let parser = CommandParser::new();
        let result = parser.execute("/unknown");
        assert!(!result.success);
    }

    #[test]
    fn test_alias() {
        let parser = CommandParser::new();
        let result = parser.execute("/t list");
        assert!(result.success);
    }

    #[test]
    fn test_help_all() {
        let parser = CommandParser::new();
        let result = parser.execute("/help");
        assert!(result.success);
        assert!(result.message.contains("task"));
    }

    #[test]
    fn test_help_specific() {
        let parser = CommandParser::new();
        let result = parser.execute("/help task");
        assert!(result.success);
        assert!(result.message.contains("task"));
    }

    #[test]
    fn test_config_set() {
        let parser = CommandParser::new();
        let result = parser.execute("/config set llm.model gpt-4");
        assert!(result.success);
    }

    #[test]
    fn test_history_list() {
        let parser = CommandParser::new();
        let result = parser.execute("/history list --limit 5");
        assert!(result.success);
    }

    #[test]
    fn test_tools_list() {
        let parser = CommandParser::new();
        let result = parser.execute("/tools list");
        assert!(result.success);
    }
}
