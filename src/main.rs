use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "coai",
    version = "0.1.0",
    about = "CoAI Code - terminal AI coding agent optimized for DeepSeek"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(help = "Task description")]
    task: Option<String>,

    #[arg(short, long, help = "Working directory")]
    workspace: Option<PathBuf>,

    #[arg(long, help = "State persistence directory")]
    state_dir: Option<PathBuf>,

    #[arg(long, help = "LLM provider (deepseek/openai/anthropic/ollama)")]
    provider: Option<String>,

    #[arg(long, help = "Model name")]
    model: Option<String>,

    #[arg(long, help = "API key")]
    api_key: Option<String>,

    #[arg(long, help = "API base URL")]
    base_url: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    History {
        #[command(subcommand)]
        subcommand: HistoryCommands,
    },
    Tool {
        #[command(subcommand)]
        subcommand: ToolCommands,
    },
    Git {
        #[command(subcommand)]
        subcommand: GitCommands,
    },
    Run {
        #[command(subcommand)]
        subcommand: RunCommands,
    },
    Doctor,
    Memory {
        #[command(subcommand)]
        subcommand: MemoryCommands,
    },
    Skills {
        #[command(subcommand)]
        subcommand: SkillsCommands,
    },
    Context {
        #[command(subcommand)]
        subcommand: ContextCommands,
    },
    Config {
        #[command(subcommand)]
        subcommand: ConfigCommands,
    },
    Session {
        #[command(subcommand)]
        subcommand: SessionCommands,
    },
}

#[derive(Subcommand)]
enum HistoryCommands {
    List {
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
    },
    Search {
        query: String,
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
    },
    Show {
        id: uuid::Uuid,
    },
    Stats,
    Delete {
        id: uuid::Uuid,
    },
    Export {
        #[arg(short, long, default_value = "json")]
        format: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ToolCommands {
    List {
        #[arg(short, long)]
        category: Option<String>,
    },
    Search {
        query: String,
        #[arg(short, long)]
        category: Option<String>,
    },
    Info {
        tool: String,
    },
    Exec {
        tool: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum GitCommands {
    Status,
    Diff {
        #[arg(long)]
        staged: bool,
        path: Option<String>,
    },
    Add {
        #[arg(required = true)]
        files: Vec<String>,
    },
    Commit {
        message: String,
    },
    Log {
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        path: Option<String>,
    },
    Branch,
    Show {
        #[arg(default_value = "HEAD")]
        rev: String,
    },
    Pull {
        remote: Option<String>,
        branch: Option<String>,
    },
    Push {
        remote: Option<String>,
        branch: Option<String>,
    },
}

#[derive(Subcommand)]
enum RunCommands {
    List {
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
    },
    Show {
        id: String,
        #[arg(long)]
        raw: bool,
    },
}

#[derive(Subcommand)]
enum MemoryCommands {
    Read,
    Search {
        query: String,
    },
    Sections,
    Append {
        content: String,
        #[arg(short, long)]
        section: Option<String>,
    },
    Delete {
        #[arg(long)]
        line: Option<usize>,
        #[arg(long)]
        section: Option<String>,
    },
    Edit,
    Write {
        content: String,
    },
    Clear,
}

#[derive(Subcommand)]
enum SkillsCommands {
    List,
    Search { query: String },
    Read { name: String },
}

#[derive(Subcommand)]
enum ContextCommands {
    Status,
    Save,
}

#[derive(Subcommand)]
enum ConfigCommands {
    Show,
    Set { key: String, value: String },
}

#[derive(Subcommand)]
enum SessionCommands {
    List {
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
    },
    Show {
        id: String,
    },
    Resume {
        id: String,
        prompt: Option<String>,
    },
    Delete {
        id: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let builder = coai_code::CoAIAgent::builder();

    let builder = if let Some(ws) = &cli.workspace {
        builder.workspace(ws)
    } else {
        builder
    };

    let builder = if let Some(state) = &cli.state_dir {
        builder.persistence_path(state)
    } else {
        builder
    };

    // Try CLI args first, then config file, then env vars
    let builder = match (&cli.provider, &cli.model, &cli.api_key, &cli.base_url) {
        (Some(provider), Some(model), api_key, base_url) => match provider.as_str() {
            "openai" => {
                let key = api_key
                    .clone()
                    .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                    .unwrap_or_default();
                builder.openai(model, key)
            }
            "anthropic" => {
                let key = api_key
                    .clone()
                    .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
                    .unwrap_or_default();
                builder.anthropic(model, key)
            }
            "ollama" => {
                let url = base_url
                    .clone()
                    .unwrap_or_else(|| "http://localhost:11434".to_string());
                builder.ollama(model, url)
            }
            "deepseek" => {
                let key = api_key
                    .clone()
                    .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
                    .unwrap_or_default();
                let url = base_url
                    .clone()
                    .unwrap_or_else(|| "https://api.deepseek.com/anthropic".to_string());
                let mut cfg = coai_code::llm::LLMConfig::anthropic(model, key);
                cfg.base_url = Some(url);
                coai_code::llm::model_caps::apply_deepseek_v4_profile(&mut cfg);
                builder.llm_config(cfg)
            }
            _ => {
                eprintln!(
                    "Unknown provider: {}; falling back to config file",
                    provider
                );
                builder
            }
        },
        _ => {
            // Try loading from config file
            match coai_code::config::load() {
                Ok(config) => {
                    let provider_name = &config.llm.default_provider;
                    if let Some(provider_cfg) = config.llm.providers.get(provider_name) {
                        let Some(llm_config) =
                            coai_code::config::llm_config_from_provider(provider_cfg)
                        else {
                            eprintln!("[CLI] Unknown provider: {}", provider_cfg.provider);
                            return Err(
                                format!("Unknown provider: {}", provider_cfg.provider).into()
                            );
                        };
                        builder.llm_config(llm_config)
                    } else {
                        eprintln!("[CLI] Provider not found in config: {}", provider_name);
                        builder
                    }
                }
                Err(e) => {
                    eprintln!("[CLI] Config load failed: {:?}", e);
                    // Fallback to env vars
                    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
                        let mut cfg = coai_code::llm::LLMConfig::anthropic("deepseek-v4-pro", key);
                        cfg.base_url = Some("https://api.deepseek.com/anthropic".to_string());
                        coai_code::llm::model_caps::apply_deepseek_v4_profile(&mut cfg);
                        builder.llm_config(cfg)
                    } else if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                        builder.openai("gpt-4o", key)
                    } else if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                        builder.anthropic("claude-sonnet-4-20250514", key)
                    } else {
                        eprintln!("[CLI] No LLM configuration found; set DEEPSEEK_API_KEY, OPENAI_API_KEY, or ANTHROPIC_API_KEY");
                        builder
                    }
                }
            }
        }
    };

    let agent = builder.build();

    match (&cli.command, &cli.task) {
        (Some(Commands::History { subcommand }), _) => {
            handle_history(subcommand, &agent).await?;
        }
        (Some(Commands::Tool { subcommand }), _) => {
            handle_tool(subcommand, &agent).await?;
        }
        (Some(Commands::Git { subcommand }), _) => {
            handle_git(subcommand, &agent).await?;
        }
        (Some(Commands::Run { subcommand }), _) => {
            handle_run(subcommand)?;
        }
        (Some(Commands::Doctor), _) => {
            handle_doctor()?;
        }
        (Some(Commands::Memory { subcommand }), _) => {
            handle_memory(subcommand, &agent).await?;
        }
        (Some(Commands::Skills { subcommand }), _) => {
            handle_skills(subcommand, &agent).await?;
        }
        (Some(Commands::Context { subcommand }), _) => {
            handle_context(subcommand, &agent).await?;
        }
        (Some(Commands::Config { subcommand }), _) => {
            handle_config(subcommand, &agent).await?;
        }
        (Some(Commands::Session { subcommand }), _) => {
            handle_session(subcommand, &agent).await?;
        }
        (None, Some(task)) => {
            println!("Running task: {}", task);
            let result = agent.execute_task(task).await?;
            println!("\nTask ID: {}", result.id);
            println!("Status: {:?}", result.status);
            if let Some(output) = result.result {
                println!("Result: {}", output);
            }
        }
        (None, None) => {
            // Launch interactive TUI mode
            coai_code::tui::run_interactive_mode().await?;
        }
    }

    Ok(())
}

async fn handle_history(
    cmd: &HistoryCommands,
    agent: &coai_code::CoAIAgent,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        HistoryCommands::List { limit } => {
            let records = agent.list_history(Some(*limit)).await;
            println!("History (last {} records):", limit);
            for r in records {
                println!("  {} - {:?}", r.id, r.status);
                println!("    {}", r.description);
            }
        }
        HistoryCommands::Search { query, limit } => {
            let records = agent
                .query_history(coai_code::history::QueryCondition {
                    time_range: None,
                    status: None,
                    tags: Vec::new(),
                    keyword: Some(query.clone()),
                })
                .await;
            println!("History search {:?} (up to {} records):", query, limit);
            for r in records.into_iter().take(*limit) {
                println!("  {} - {:?}", r.id, r.status);
                println!("    {}", r.description);
                if let Some(result) = &r.result {
                    let preview = result.chars().take(120).collect::<String>();
                    println!("    Result: {}", preview);
                }
            }
        }
        HistoryCommands::Show { id } => {
            let records = agent
                .query_history(coai_code::history::QueryCondition {
                    time_range: None,
                    status: None,
                    tags: Vec::new(),
                    keyword: None,
                })
                .await;
            if let Some(record) = records.into_iter().find(|record| record.id == *id) {
                println!("{}", serde_json::to_string_pretty(&record)?);
            } else {
                println!("History record not found: {}", id);
            }
        }
        HistoryCommands::Stats => {
            let records = agent.list_history(None).await;
            let mut by_status = std::collections::BTreeMap::new();
            let mut by_tag = std::collections::BTreeMap::new();
            for record in &records {
                *by_status
                    .entry(format!("{:?}", record.status))
                    .or_insert(0usize) += 1;
                for tag in &record.tags {
                    *by_tag.entry(tag.clone()).or_insert(0usize) += 1;
                }
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "total": records.len(),
                    "by_status": by_status,
                    "by_tag": by_tag,
                }))?
            );
        }
        HistoryCommands::Delete { id } => {
            let call = coai_code::ToolCall {
                tool: "history.delete".to_string(),
                params: serde_json::json!({ "id": id.to_string() }),
            };
            let result = agent.execute_tool(&call).await?;
            println!("Success: {}", result.success);
            if let Some(output) = result.output {
                println!("{}", output.as_str().unwrap_or(""));
            }
            if let Some(error) = result.error {
                println!("Error: {}", error);
            }
        }
        HistoryCommands::Export { format, output } => {
            let fmt = match format.as_str() {
                "json" => coai_code::history::ExportFormat::Json,
                "md" | "markdown" => coai_code::history::ExportFormat::Markdown,
                "csv" => coai_code::history::ExportFormat::Csv,
                _ => coai_code::history::ExportFormat::Json,
            };
            let content = agent.export_history(fmt).await?;
            if let Some(path) = output {
                std::fs::write(path, content)?;
                println!("History exported to: {}", path.display());
            } else {
                println!("{}", content);
            }
        }
    }
    Ok(())
}

async fn handle_tool(
    cmd: &ToolCommands,
    agent: &coai_code::CoAIAgent,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        ToolCommands::List { category } => {
            let tools = agent.list_tools().await;
            println!("Available tools:");
            for t in tools.into_iter().filter(|tool| {
                category
                    .as_ref()
                    .map(|category| tool.name.starts_with(&format!("{category}.")))
                    .unwrap_or(true)
            }) {
                println!("  {} [{}] - {}", t.name, t.category(), t.description);
                if !t.params.is_empty() {
                    println!("    Params: {}", t.params.join(", "));
                }
                if !t.examples().is_empty() {
                    println!("    Example: {}", serde_json::to_string(&t.examples()[0])?);
                }
            }
        }
        ToolCommands::Search { query, category } => {
            let tools = agent.list_tools().await;
            let query = query.to_lowercase();
            println!("Matching tools:");
            for t in tools.into_iter().filter(|tool| {
                let category_match = category
                    .as_ref()
                    .map(|category| tool.name.starts_with(&format!("{category}.")))
                    .unwrap_or(true);
                category_match
                    && (tool.name.to_lowercase().contains(&query)
                        || tool.description.to_lowercase().contains(&query)
                        || tool
                            .params
                            .iter()
                            .any(|param| param.to_lowercase().contains(&query)))
            }) {
                println!("  {} [{}] - {}", t.name, t.category(), t.description);
                if !t.params.is_empty() {
                    println!("    Params: {}", t.params.join(", "));
                }
            }
        }
        ToolCommands::Info { tool } => {
            let normalized = tool.replace('_', ".");
            let tools = agent.list_tools().await;
            if let Some(t) = tools
                .into_iter()
                .find(|t| t.name == *tool || t.name == normalized)
            {
                println!("{}", serde_json::to_string_pretty(&t.reference())?);
            } else {
                println!("Unknown tool: {}", tool);
            }
        }
        ToolCommands::Exec { tool, args } => {
            let params = parse_tool_args(args);
            let call = coai_code::ToolCall {
                tool: tool.clone(),
                params: serde_json::to_value(params)?,
            };
            let result = agent.execute_tool(&call).await?;
            println!("Tool: {}", tool);
            println!("Success: {}", result.success);
            if let Some(output) = result.output {
                println!("Output: {}", serde_json::to_string_pretty(&output)?);
            }
            if let Some(error) = result.error {
                println!("Error: {}", error);
            }
        }
    }
    Ok(())
}

async fn handle_git(
    cmd: &GitCommands,
    agent: &coai_code::CoAIAgent,
) -> Result<(), Box<dyn std::error::Error>> {
    let (tool, params) = match cmd {
        GitCommands::Status => ("git.status", serde_json::json!({})),
        GitCommands::Diff { staged, path } => (
            "git.diff",
            serde_json::json!({
                "staged": staged,
                "path": path,
            }),
        ),
        GitCommands::Add { files } => (
            "git.add",
            serde_json::json!({
                "files": files.join(" "),
            }),
        ),
        GitCommands::Commit { message } => (
            "git.commit",
            serde_json::json!({
                "message": message,
            }),
        ),
        GitCommands::Log { limit, path } => (
            "git.log",
            serde_json::json!({
                "limit": limit,
                "path": path,
            }),
        ),
        GitCommands::Branch => ("git.branch", serde_json::json!({})),
        GitCommands::Show { rev } => (
            "git.show",
            serde_json::json!({
                "rev": rev,
            }),
        ),
        GitCommands::Pull { remote, branch } => (
            "git.pull",
            serde_json::json!({
                "remote": remote,
                "branch": branch,
            }),
        ),
        GitCommands::Push { remote, branch } => (
            "git.push",
            serde_json::json!({
                "remote": remote,
                "branch": branch,
            }),
        ),
    };
    let result = agent
        .execute_tool(&coai_code::ToolCall {
            tool: tool.to_string(),
            params,
        })
        .await?;
    if let Some(output) = result.output {
        println!("{}", serde_json::to_string_pretty(&output)?);
    }
    if let Some(error) = result.error {
        println!("Error: {}", error);
    }
    Ok(())
}

fn handle_run(cmd: &RunCommands) -> Result<(), Box<dyn std::error::Error>> {
    let workspace = std::env::current_dir()?;
    match cmd {
        RunCommands::List { limit } => {
            let logs = coai_code::run_log::list_run_logs(&workspace, *limit)?;
            if logs.is_empty() {
                println!("No run logs found");
            } else {
                println!("Run logs:");
                for log in logs {
                    println!(
                        "  {}  {} bytes  {}",
                        log.id,
                        log.bytes,
                        log.modified.unwrap_or_else(|| "-".into())
                    );
                }
            }
        }
        RunCommands::Show { id, raw } => {
            let content = coai_code::run_log::read_run_log(&workspace, id)?;
            if *raw {
                println!("{}", content);
            } else {
                println!("{}", coai_code::run_log::format_run_log_timeline(&content));
            }
        }
    }
    Ok(())
}

fn handle_doctor() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    println!("coai doctor");
    println!("Working directory: {}", cwd.display());
    println!("State directory: {}", cwd.join(".coai/state").display());
    println!(
        "State directory writable: {}",
        std::fs::create_dir_all(cwd.join(".coai/state")).is_ok()
    );

    match coai_code::config::load() {
        Ok(config) => {
            println!("Config file: OK");
            println!("Default provider: {}", config.llm.default_provider);
            if let Some(provider) = config.llm.providers.get(&config.llm.default_provider) {
                if let Some(llm) = coai_code::config::llm_config_from_provider(provider) {
                    let caps = coai_code::llm::model_caps::get_model_capabilities(&llm.model);
                    println!("Model: {}", llm.model);
                    println!("Context window: {}", caps.context_length);
                    println!("Max output tokens: {}", llm.max_tokens);
                    if let Some(flash) = llm.flash_model {
                        println!("Flash model: {}", flash);
                    }
                    println!("base_url: {}", llm.base_url.unwrap_or_else(|| "-".into()));
                    println!(
                        "api_key: {}",
                        if llm.api_key.is_some() {
                            "configured"
                        } else {
                            "not configured"
                        }
                    );
                }
            }
        }
        Err(e) => {
            println!("Config file: not loaded ({})", e);
            for key in ["DEEPSEEK_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"] {
                println!(
                    "Env var {}: {}",
                    key,
                    if std::env::var(key).is_ok() {
                        "configured"
                    } else {
                        "not configured"
                    }
                );
            }
        }
    }

    let tools = coai_code::tools::ToolRegistry::new(&cwd).list_tools();
    println!("Tool count: {}", tools.len());
    println!(
        "Run log directory: {}",
        cwd.join(".coai/state/runs").display()
    );
    println!(
        "Search index: {}",
        cwd.join(".coai/state/search-index.json").display()
    );
    for command in ["git", "grep", "find"] {
        println!(
            "System command {}: {}",
            command,
            if command_exists(command) {
                "available"
            } else {
                "not available"
            }
        );
    }
    if command_exists("git") {
        match std::process::Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(&cwd)
            .output()
        {
            Ok(output) => println!(
                "Git workspace: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            ),
            Err(e) => println!("Git workspace check failed: {}", e),
        }
    }
    Ok(())
}

fn command_exists(command: &str) -> bool {
    std::process::Command::new("which")
        .arg(command)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

async fn handle_memory(
    cmd: &MemoryCommands,
    agent: &coai_code::CoAIAgent,
) -> Result<(), Box<dyn std::error::Error>> {
    let (tool, params) = match cmd {
        MemoryCommands::Read => ("memory.read", serde_json::json!({})),
        MemoryCommands::Search { query } => {
            ("memory.search", serde_json::json!({ "query": query }))
        }
        MemoryCommands::Sections => ("memory.sections", serde_json::json!({})),
        MemoryCommands::Append { content, section } => (
            "memory.append",
            serde_json::json!({ "content": content, "section": section }),
        ),
        MemoryCommands::Delete { line, section } => (
            "memory.delete",
            serde_json::json!({ "line": line, "section": section }),
        ),
        MemoryCommands::Edit => ("memory.edit", serde_json::json!({})),
        MemoryCommands::Write { content } => {
            ("memory.write", serde_json::json!({ "content": content }))
        }
        MemoryCommands::Clear => ("memory.clear", serde_json::json!({})),
    };

    let result = agent
        .execute_tool(&coai_code::ToolCall {
            tool: tool.to_string(),
            params,
        })
        .await?;

    if let Some(output) = result.output {
        let text = output.as_str().unwrap_or("");
        if matches!(cmd, MemoryCommands::Edit) {
            open_editor(text)?;
        } else {
            println!("{}", text);
        }
    }
    if let Some(error) = result.error {
        println!("Error: {}", error);
    }
    Ok(())
}

async fn handle_skills(
    cmd: &SkillsCommands,
    agent: &coai_code::CoAIAgent,
) -> Result<(), Box<dyn std::error::Error>> {
    let (tool, params) = match cmd {
        SkillsCommands::List => ("skills.list", serde_json::json!({})),
        SkillsCommands::Search { query } => {
            ("skills.search", serde_json::json!({ "query": query }))
        }
        SkillsCommands::Read { name } => ("skills.read", serde_json::json!({ "name": name })),
    };

    let result = agent
        .execute_tool(&coai_code::ToolCall {
            tool: tool.to_string(),
            params,
        })
        .await?;

    if let Some(output) = result.output {
        println!("{}", output.as_str().unwrap_or(""));
    }
    if let Some(error) = result.error {
        println!("Error: {}", error);
    }
    Ok(())
}

fn open_editor(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor).arg(path).status()?;
    if status.success() {
        println!("Edited {}", path);
    } else {
        println!("Editor exited with status: {}", status);
    }
    Ok(())
}

async fn handle_context(
    cmd: &ContextCommands,
    agent: &coai_code::CoAIAgent,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        ContextCommands::Status => {
            let status = agent.context_status().await;
            println!("Context status:");
            println!("  Loaded files: {:?}", status.loaded_files);
            println!(
                "  Token usage: {}/{} ({:.1}%)",
                status.total_tokens,
                status.total_tokens + status.available_tokens,
                status.usage_percentage
            );
        }
        ContextCommands::Save => {
            let snapshot = agent.save_state().await?;
            println!("State saved: {} files", snapshot.loaded_files.len());
        }
    }
    Ok(())
}

async fn handle_config(
    cmd: &ConfigCommands,
    agent: &coai_code::CoAIAgent,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        ConfigCommands::Show => {
            println!("Current config:");
            println!("  Workspace: {:?}", agent.workspace());
            if let Some(config) = agent.llm_config() {
                println!("  LLM provider: {:?}", config.provider);
                println!("  Model: {}", config.model);
                println!("  Temperature: {}", config.temperature);
                println!("  Max tokens: {}", config.max_tokens);
            } else {
                println!("  LLM: not configured");
            }
        }
        ConfigCommands::Set { key, value } => {
            println!(
                "Config {} = {} (restart required to take effect)",
                key, value
            );
        }
    }
    Ok(())
}

async fn handle_session(
    cmd: &SessionCommands,
    agent: &coai_code::CoAIAgent,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = coai_code::session::SessionStore::new();
    match cmd {
        SessionCommands::List { limit } => {
            let sessions = store.list();
            if sessions.is_empty() {
                println!("No sessions found");
            } else {
                println!("Sessions (last {} records):", limit);
                for session in sessions.into_iter().take(*limit) {
                    println!(
                        "  {}  {}  ({} messages) {}",
                        session.updated_at,
                        session.id,
                        session.messages.len(),
                        session.description
                    );
                }
            }
        }
        SessionCommands::Show { id } => {
            let Some(session) = store.load(id) else {
                println!("Session not found: {}", id);
                return Ok(());
            };
            println!("Session: {}", session.id);
            println!("Created: {}", session.created_at);
            println!("Updated: {}", session.updated_at);
            println!("Description: {}", session.description);
            println!("Messages:");
            for (idx, message) in session.messages.iter().enumerate() {
                let preview = compact_preview(&message.content, 160);
                println!("  {}. {}: {}", idx + 1, message.role, preview);
            }
        }
        SessionCommands::Resume { id, prompt } => {
            let Some(mut session) = store.load(id) else {
                println!("Session not found: {}", id);
                return Ok(());
            };
            let mut messages: Vec<coai_code::llm::Message> = session
                .messages
                .iter()
                .map(coai_code::session::serializable_to_message)
                .collect();

            if let Some(prompt) = prompt {
                messages.push(coai_code::llm::Message::user(prompt));
            } else if !matches!(
                messages.last().map(|message| &message.role),
                Some(coai_code::llm::Role::User)
            ) {
                messages.push(coai_code::llm::Message::user(
                    "Please continue from where you left off.",
                ));
            }

            let (_output, updated_messages) = agent
                .run_messages_with_tools(messages, |event| match event {
                    coai_code::llm::tool_loop::LoopEvent::Reasoning(text)
                    | coai_code::llm::tool_loop::LoopEvent::TextOutput(text) => {
                        use std::io::Write;
                        print!("{}", text);
                        std::io::stdout().flush().ok();
                    }
                    coai_code::llm::tool_loop::LoopEvent::ToolStart { name, detail, .. } => {
                        println!("\n⏺ {}", name);
                        if !detail.is_empty() {
                            println!("  ⎿ {}", detail);
                        }
                    }
                    coai_code::llm::tool_loop::LoopEvent::ToolOutput { result, .. } => {
                        println!("  ⎿ {}", if result.success { "done" } else { "failed" });
                    }
                    coai_code::llm::tool_loop::LoopEvent::LiveContextApplied { .. }
                    | coai_code::llm::tool_loop::LoopEvent::MessagesCheckpoint(_) => {}
                    coai_code::llm::tool_loop::LoopEvent::Response(text) => {
                        if !text.trim().is_empty() {
                            println!("\n{}", text);
                        }
                    }
                    coai_code::llm::tool_loop::LoopEvent::Error(error) => {
                        eprintln!("\nError: {}", error);
                    }
                })
                .await?;

            session.updated_at = chrono::Utc::now().to_rfc3339();
            session.messages = updated_messages
                .iter()
                .filter(|message| !matches!(message.role, coai_code::llm::Role::System))
                .map(coai_code::session::message_to_serializable)
                .collect();
            store.save(&session);
            println!("\nSession saved: {}", session.id);
        }
        SessionCommands::Delete { id } => {
            if store.delete(id) {
                println!("Session deleted: {}", id);
            } else {
                println!("Delete failed or session not found: {}", id);
            }
        }
    }
    Ok(())
}

fn compact_preview(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    format!(
        "{}...",
        normalized.chars().take(max_chars).collect::<String>()
    )
}

fn parse_tool_args(args: &[String]) -> std::collections::HashMap<String, String> {
    let mut params = std::collections::HashMap::new();
    let mut i = 0;
    while i < args.len() {
        if args[i].starts_with("--") {
            let key = args[i][2..].to_string();
            if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                params.insert(key, args[i + 1].clone());
                i += 2;
            } else {
                params.insert(key, "true".to_string());
                i += 1;
            }
        } else {
            params.insert(format!("arg{}", i), args[i].clone());
            i += 1;
        }
    }
    params
}
