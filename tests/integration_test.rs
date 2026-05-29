use coai_code::command::CommandParser;
use coai_code::context::ContextManager;
use coai_code::core::types::{CommandOutput, TaskRecord, TaskStatus, ToolCall};
use coai_code::history::HistoryStore;
use coai_code::llm::LLMConfig;
use coai_code::run_log::{list_run_logs, read_run_log, RunLogger};
use coai_code::session::{new_session, SessionStore};
use coai_code::tools::ToolRegistry;

#[test]
fn test_command_parser_help() {
    let parser = CommandParser::new();
    let result = parser.execute("/help");
    assert!(result.success);
    assert!(result.message.contains("task"));
}

#[test]
fn test_command_parser_task_list() {
    let parser = CommandParser::new();
    let result = parser.execute("/task list --limit 5");
    assert!(result.success);
}

#[test]
fn test_command_parser_task_create() {
    let parser = CommandParser::new();
    let result = parser.execute("/task create \"test task\" --priority high");
    assert!(result.success);
}

#[test]
fn test_command_parser_config_show() {
    let parser = CommandParser::new();
    let result = parser.execute("/config show");
    assert!(result.success);
}

#[test]
fn test_command_parser_tools_list() {
    let parser = CommandParser::new();
    let result = parser.execute("/tools list");
    assert!(result.success);
}

#[test]
fn test_command_parser_tools_search() {
    let parser = CommandParser::new();
    let result = parser.execute("/tools search file");
    assert!(result.success);
}

#[test]
fn test_command_parser_memory_read() {
    let parser = CommandParser::new();
    let result = parser.execute("/memory read");
    assert!(result.success);
}

#[test]
fn test_command_parser_memory_delete() {
    let parser = CommandParser::new();
    let result = parser.execute("/memory delete --line 7");
    assert!(result.success);
}

#[test]
fn test_command_parser_history_list() {
    let parser = CommandParser::new();
    let result = parser.execute("/history list");
    assert!(result.success);
}

#[test]
fn test_command_parser_history_search() {
    let parser = CommandParser::new();
    let result = parser.execute("/history search auth --limit 5");
    assert!(result.success);
}

#[test]
fn test_command_parser_alias() {
    let parser = CommandParser::new();
    let result = parser.execute("/t list");
    assert!(result.success);
}

#[test]
fn test_command_parser_unknown() {
    let parser = CommandParser::new();
    let result = parser.execute("/nonexistent");
    assert!(!result.success);
}

#[test]
fn test_task_record_default() {
    let record = TaskRecord::default();
    assert_eq!(record.status, TaskStatus::Pending);
    assert!(record.description.is_empty());
    assert!(record.result.is_none());
    assert!(record.steps.is_empty());
    assert!(record.tags.is_empty());
}

#[test]
fn test_session_store_save_load_delete() {
    let temp = tempfile::TempDir::new().unwrap();
    let store = SessionStore::with_dir(temp.path());
    let mut session = new_session("resume test");
    session
        .messages
        .push(coai_code::session::SerializableMessage {
            role: "user".to_string(),
            content: "continue previous task".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        });

    store.save(&session);
    let loaded = store.load(&session.id).unwrap();
    assert_eq!(loaded.description, "resume test");
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(store.list().len(), 1);
    assert!(store.delete(&session.id));
    assert!(store.load(&session.id).is_none());
}

#[test]
fn test_context_manager_new() {
    let ctx = ContextManager::new(200_000, "/tmp/coai-test");
    let status = ctx.status();
    assert_eq!(status.total_tokens, 0);
    assert_eq!(status.available_tokens, 200_000);
}

#[test]
fn test_tool_registry_new() {
    let registry = ToolRegistry::new(".");
    let tools = registry.list_tools();
    assert!(!tools.is_empty());
    assert!(tools.iter().any(|t| t.name == "file.read"));
    assert!(tools.iter().any(|t| t.name == "search.grep"));
    assert!(tools.iter().any(|t| t.name == "search.semantic"));
    assert!(tools.iter().any(|t| t.name == "search.index"));
    assert!(tools.iter().any(|t| t.name == "exec.run"));
    assert!(tools.iter().any(|t| t.name == "history.search"));
    assert!(tools.iter().any(|t| t.name == "memory.read"));
    assert!(tools.iter().any(|t| t.name == "skills.list"));
    assert!(tools.iter().any(|t| t.name == "skills.read"));
    assert!(tools.iter().any(|t| t.name == "tools.info"));
    assert!(tools.iter().any(|t| t.name == "git.status"));
    assert!(!tools.iter().any(|t| t.name == "agent.spawn"));
}

#[test]
fn test_command_parser_skills_list() {
    let parser = CommandParser::new();
    let result = parser.execute("/skills list");
    assert!(result.success);
}

#[tokio::test]
async fn test_tool_registry_skills_list_search_read() {
    let temp = tempfile::TempDir::new().unwrap();
    let skill_dir = temp.path().join(".codex/skills/coai-test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: coai-test-skill\nsummary: Handles deterministic test fixtures\n---\n# Ignored\n\nUse this for fixture work.\n",
    )
    .unwrap();

    let registry = ToolRegistry::new(temp.path());
    let list = registry
        .execute(&ToolCall {
            tool: "skills.list".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();
    let list_output = list.output.unwrap().as_str().unwrap().to_string();
    assert!(list_output.contains("coai-test-skill"));
    assert!(list_output.contains("Handles deterministic test fixtures"));

    let search = registry
        .execute(&ToolCall {
            tool: "skills.search".to_string(),
            params: serde_json::json!({ "query": "fixture" }),
        })
        .await
        .unwrap();
    assert!(search
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .contains("coai-test-skill"));

    let read = registry
        .execute(&ToolCall {
            tool: "skills.read".to_string(),
            params: serde_json::json!({ "name": "coai-test-skill" }),
        })
        .await
        .unwrap();
    assert!(read
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .contains("Use this for fixture work."));
}

#[test]
fn test_skill_prompt_context_lists_detected_skills() {
    let temp = tempfile::TempDir::new().unwrap();
    let skill_dir = temp.path().join(".claude/skills/coai-prompt-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "# coai-prompt-skill\n\nPrompt context coverage skill.\n",
    )
    .unwrap();

    let registry = ToolRegistry::new(temp.path());
    let prompt = registry.skill_prompt_context();
    assert!(prompt.contains("## Skills"));
    assert!(prompt.contains("coai-prompt-skill"));
    assert!(prompt.contains("skills.read"));
}

#[test]
fn test_history_store_search_matches_result_and_tags() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    let mut store = HistoryStore::new(temp.path());
    let record = TaskRecord {
        description: "implement login endpoint".to_string(),
        status: TaskStatus::Completed,
        result: Some("JWT authentication finished".to_string()),
        tags: vec!["auth".to_string()],
        ..Default::default()
    };

    store.store(record).unwrap();

    assert_eq!(store.search("authentication", Some(10)).len(), 1);
    assert_eq!(store.search("auth", Some(10)).len(), 1);
}

#[tokio::test]
async fn test_tool_registry_semantic_search_matches_related_code() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(
        temp.path().join("src/recovery.rs"),
        "pub fn load_checkpoint_and_resume_session() { /* recover context state */ }\n",
    )
    .unwrap();

    let registry = ToolRegistry::new(temp.path());
    let result = registry
        .execute(&ToolCall {
            tool: "search.semantic".to_string(),
            params: serde_json::json!({
                "query": "long task interrupt resume",
                "path": "src",
                "k": 3
            }),
        })
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap().as_str().unwrap().to_string();
    assert!(output.contains("src/recovery.rs"));
}

#[tokio::test]
async fn test_tool_registry_search_index_builds_and_semantic_uses_it() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(
        temp.path().join("src/permission.rs"),
        "pub fn confirm_risky_action() { /* approval required */ }\n",
    )
    .unwrap();

    let registry = ToolRegistry::new(temp.path());
    let index = registry
        .execute(&ToolCall {
            tool: "search.index".to_string(),
            params: serde_json::json!({ "path": "src" }),
        })
        .await
        .unwrap();
    assert!(index
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .contains("index written to"));

    let result = registry
        .execute(&ToolCall {
            tool: "search.semantic".to_string(),
            params: serde_json::json!({
                "query": "permission confirm",
                "path": "src",
                "k": 3
            }),
        })
        .await
        .unwrap();
    assert!(result
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .contains("src/permission.rs"));
}

#[tokio::test]
async fn test_tool_registry_history_search_reads_persisted_history() {
    let temp = tempfile::TempDir::new().unwrap();
    let history_path = temp.path().join(".coai/state/history.json");
    let mut store = HistoryStore::new(&history_path);
    let record = TaskRecord {
        description: "add authentication flow".to_string(),
        status: TaskStatus::Completed,
        result: Some("authentication flow completed".to_string()),
        ..Default::default()
    };
    store.store(record).unwrap();

    let registry = ToolRegistry::new(temp.path());
    let result = registry
        .execute(&ToolCall {
            tool: "history.search".to_string(),
            params: serde_json::json!({ "query": "authentication", "limit": 5 }),
        })
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap().as_str().unwrap().to_string();
    assert!(output.contains("add authentication flow"));
}

#[tokio::test]
async fn test_tool_registry_history_stats_and_delete() {
    let temp = tempfile::TempDir::new().unwrap();
    let history_path = temp.path().join(".coai/state/history.json");
    let mut store = HistoryStore::new(&history_path);
    let record = TaskRecord {
        description: "record history stats".to_string(),
        status: TaskStatus::Completed,
        tags: vec!["memory".to_string()],
        ..Default::default()
    };
    let id = record.id;
    store.store(record).unwrap();

    let registry = ToolRegistry::new(temp.path());
    let stats = registry
        .execute(&ToolCall {
            tool: "history.stats".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();
    assert!(stats.output.unwrap().as_str().unwrap().contains("memory"));

    let deleted = registry
        .execute(&ToolCall {
            tool: "history.delete".to_string(),
            params: serde_json::json!({ "id": id.to_string() }),
        })
        .await
        .unwrap();
    assert!(deleted
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .contains("\"deleted\": true"));
}

#[tokio::test]
async fn test_tool_registry_tools_info_and_search() {
    let registry = ToolRegistry::new(".");
    let info = registry
        .execute(&ToolCall {
            tool: "tools.info".to_string(),
            params: serde_json::json!({ "name": "file.read" }),
        })
        .await
        .unwrap();
    let info_output = info.output.unwrap().as_str().unwrap().to_string();
    assert!(info_output.contains("Read file contents"));
    assert!(info_output.contains("\"schema\""));
    assert!(info_output.contains("\"examples\""));

    let search = registry
        .execute(&ToolCall {
            tool: "tools.search".to_string(),
            params: serde_json::json!({ "query": "history" }),
        })
        .await
        .unwrap();
    assert!(search
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .contains("history.search"));

    let category_search = registry
        .execute(&ToolCall {
            tool: "tools.search".to_string(),
            params: serde_json::json!({ "query": "status", "category": "git" }),
        })
        .await
        .unwrap();
    let category_output = category_search
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    assert!(category_output.contains("git.status"));
    assert!(!category_output.contains("history."));
}

#[tokio::test]
async fn test_tool_registry_exec_run_supports_safe_cwd() {
    let temp = tempfile::TempDir::new().unwrap();
    let subdir = temp.path().join("crates/api");
    std::fs::create_dir_all(&subdir).unwrap();
    let registry = ToolRegistry::new(temp.path());

    let result = registry
        .execute(&ToolCall {
            tool: "exec.run".to_string(),
            params: serde_json::json!({
                "command": "pwd",
                "cwd": "crates/api"
            }),
        })
        .await
        .unwrap();

    let output: CommandOutput = serde_json::from_str(result.output.unwrap().as_str().unwrap())
        .expect("exec.run output should be CommandOutput json");
    assert!(output.success);
    assert_eq!(
        output.stdout.trim(),
        subdir.canonicalize().unwrap().to_string_lossy()
    );
}

#[tokio::test]
async fn test_tool_registry_exec_run_rejects_external_cwd() {
    let temp = tempfile::TempDir::new().unwrap();
    let external = tempfile::TempDir::new().unwrap();
    let registry = ToolRegistry::new(temp.path());

    let err = registry
        .execute(&ToolCall {
            tool: "exec.run".to_string(),
            params: serde_json::json!({
                "command": "pwd",
                "cwd": external.path().to_string_lossy()
            }),
        })
        .await
        .unwrap_err();

    assert!(err.to_string().contains("cwd is outside the working directory"));
}

#[tokio::test]
async fn test_tool_reference_describes_exec_cwd_and_cleanup_review() {
    let registry = ToolRegistry::new(".");
    let exec_info = registry
        .execute(&ToolCall {
            tool: "tools.info".to_string(),
            params: serde_json::json!({ "name": "exec.test" }),
        })
        .await
        .unwrap();
    let exec_output = exec_info.output.unwrap().as_str().unwrap().to_string();
    assert!(exec_output.contains("\"cwd\""));
    assert!(exec_output.contains("monorepo"));

    let cleanup_info = registry
        .execute(&ToolCall {
            tool: "tools.info".to_string(),
            params: serde_json::json!({ "name": "cleanup.report" }),
        })
        .await
        .unwrap();
    let cleanup_output = cleanup_info.output.unwrap().as_str().unwrap().to_string();
    assert!(cleanup_output.contains("untracked"));
    assert!(cleanup_output.contains("Lists only"));
}

#[tokio::test]
async fn test_tool_registry_git_status() {
    let registry = ToolRegistry::new(".");
    let result = registry
        .execute(&ToolCall {
            tool: "git.status".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();
    assert!(result.output.unwrap().get("command").is_some());
}

#[tokio::test]
async fn test_tool_registry_git_branch_and_log() {
    let registry = ToolRegistry::new(".");
    let branch = registry
        .execute(&ToolCall {
            tool: "git.branch".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();
    assert!(branch.output.unwrap().get("command").is_some());

    let log = registry
        .execute(&ToolCall {
            tool: "git.log".to_string(),
            params: serde_json::json!({ "limit": 1 }),
        })
        .await
        .unwrap();
    assert!(log.output.unwrap().get("command").is_some());
}

#[test]
fn test_run_logger_writes_and_lists_jsonl() {
    let temp = tempfile::TempDir::new().unwrap();
    let logger = RunLogger::new(temp.path(), "test run log").unwrap();
    logger
        .log("tool_start", serde_json::json!({ "name": "file.read" }))
        .unwrap();

    let logs = list_run_logs(temp.path(), 10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].id, logger.id());

    let content = read_run_log(temp.path(), logger.id()).unwrap();
    assert!(content.contains("run_started"));
    assert!(content.contains("tool_start"));
}

#[tokio::test]
async fn test_tool_registry_memory_read_append_search() {
    let temp = tempfile::TempDir::new().unwrap();
    let registry = ToolRegistry::new(temp.path());

    let append = registry
        .execute(&ToolCall {
            tool: "memory.append".to_string(),
            params: serde_json::json!({
                "content": "project test command is cargo test",
                "section": "Commands"
            }),
        })
        .await
        .unwrap();
    assert!(append.success);

    let read = registry
        .execute(&ToolCall {
            tool: "memory.read".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();
    assert!(read
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .contains("cargo test"));

    let search = registry
        .execute(&ToolCall {
            tool: "memory.search".to_string(),
            params: serde_json::json!({ "query": "cargo" }),
        })
        .await
        .unwrap();
    assert!(search
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .contains("cargo test"));

    let sections = registry
        .execute(&ToolCall {
            tool: "memory.sections".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();
    assert!(sections
        .output
        .unwrap()
        .as_str()
        .unwrap()
        .contains("Commands"));

    let deleted = registry
        .execute(&ToolCall {
            tool: "memory.delete".to_string(),
            params: serde_json::json!({ "section": "Commands" }),
        })
        .await
        .unwrap();
    assert!(deleted.success);

    let after_delete = registry
        .execute(&ToolCall {
            tool: "memory.search".to_string(),
            params: serde_json::json!({ "query": "cargo" }),
        })
        .await
        .unwrap();
    assert_eq!(after_delete.output.unwrap().as_str().unwrap(), "[]");
}

#[test]
fn test_tool_registry_agent_spawn_requires_llm_config() {
    let registry = ToolRegistry::new(".").with_llm_config(LLMConfig::default());
    let tools = registry.list_tools();
    assert!(tools.iter().any(|t| t.name == "agent.spawn"));
}

#[test]
fn test_tool_registry_can_disable_agent_tools() {
    let registry = ToolRegistry::new(".")
        .with_llm_config(LLMConfig::default())
        .with_agent_tools_enabled(false);
    let tools = registry.list_tools();
    assert!(!tools.iter().any(|t| t.name == "agent.spawn"));
}

#[test]
fn test_command_parser_available_commands() {
    let parser = CommandParser::new();
    let commands = parser.available_commands();
    assert!(commands.contains(&"task".to_string()));
    assert!(commands.contains(&"config".to_string()));
    assert!(commands.contains(&"tools".to_string()));
    assert!(commands.contains(&"history".to_string()));
    assert!(commands.contains(&"memory".to_string()));
    assert!(commands.contains(&"skills".to_string()));
    assert!(commands.contains(&"help".to_string()));
}

