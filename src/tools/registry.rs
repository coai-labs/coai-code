use crate::core::{CoAIError, Result, ToolCall, ToolResult};
use crate::history::ExportFormat;
use crate::llm::LLMConfig;
use crate::skills::SkillRegistry;
use crate::tools::{
    AgentTools, CleanupTools, ExecTools, FileTools, GitTools, HistoryTools, MemoryTools, NetTools,
    SearchTools, SkillTools, TaskItem, TaskTools, ValidationTools,
};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum ToolProgressEvent {
    ToolStart {
        name: String,
        detail: String,
    },
    ToolOutput {
        name: String,
        success: bool,
        preview: String,
    },
}

pub type ToolProgressCallback = Arc<dyn Fn(ToolProgressEvent) + Send + Sync>;

#[derive(Clone)]
pub struct ToolRegistry {
    workspace: PathBuf,
    allow_external_mutations: bool,
    llm_config: Option<LLMConfig>,
    agent_tools_enabled: bool,
    progress_callback: Option<ToolProgressCallback>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub params: Vec<String>,
}

impl ToolInfo {
    pub fn category(&self) -> &str {
        self.name.split('.').next().unwrap_or("tool")
    }

    pub fn schema(&self) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for param in &self.params {
            let name = param.trim_end_matches('?');
            properties.insert(
                name.to_string(),
                serde_json::json!({
                    "type": parameter_type(name),
                    "description": parameter_description(&self.name, name),
                }),
            );
            if !param.ends_with('?') {
                required.push(name.to_string());
            }
        }
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": properties,
        });
        if !required.is_empty() {
            schema["required"] = serde_json::json!(required);
        }
        schema
    }

    pub fn examples(&self) -> Vec<serde_json::Value> {
        tool_examples(&self.name)
    }

    pub fn reference(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "category": self.category(),
            "description": self.description,
            "params": self.params,
            "schema": self.schema(),
            "examples": self.examples(),
        })
    }
}

impl ToolRegistry {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            allow_external_mutations: false,
            llm_config: None,
            agent_tools_enabled: true,
            progress_callback: None,
        }
    }

    pub fn with_external_mutations(mut self, allow: bool) -> Self {
        self.allow_external_mutations = allow;
        self
    }

    pub fn with_llm_config(mut self, config: LLMConfig) -> Self {
        self.llm_config = Some(config);
        self
    }

    pub fn with_agent_tools_enabled(mut self, enabled: bool) -> Self {
        self.agent_tools_enabled = enabled;
        self
    }

    pub fn with_progress_callback(mut self, callback: ToolProgressCallback) -> Self {
        self.progress_callback = Some(callback);
        self
    }

    pub async fn execute(&self, call: &ToolCall) -> Result<ToolResult> {
        // Convert API tool name back to internal format: "file_read" -> "file.read"
        // Only replace the first underscore, since method names might contain underscores
        let tool_name = if let Some(pos) = call.tool.find('_') {
            let mut s = call.tool.clone();
            s.replace_range(pos..pos + 1, ".");
            s
        } else {
            call.tool.clone()
        };

        let parts: Vec<&str> = tool_name.split('.').collect();

        if parts.len() != 2 {
            return Err(CoAIError::Other(format!(
                "Invalid tool name: {}",
                call.tool
            )));
        }

        let (category, method) = (parts[0], parts[1]);

        match category {
            "file" => self.execute_file(method, &call.params).await,
            "search" => self.execute_search(method, &call.params).await,
            "exec" => self.execute_exec(method, &call.params).await,
            "validate" => self.execute_validate(method, &call.params).await,
            "cleanup" => self.execute_cleanup(method, &call.params).await,
            "net" => self.execute_net(method, &call.params).await,
            "history" => self.execute_history(method, &call.params).await,
            "memory" => self.execute_memory(method, &call.params).await,
            "skills" => self.execute_skills(method, &call.params).await,
            "tools" => self.execute_tools_reference(method, &call.params).await,
            "git" => self.execute_git(method, &call.params).await,
            "tasks" => self.execute_tasks(method, &call.params).await,
            "agent" => Box::pin(self.execute_agent(method, &call.params)).await,
            _ => Err(CoAIError::Other(format!(
                "Unknown tool category: {}",
                category
            ))),
        }
    }

    pub fn list_tools(&self) -> Vec<ToolInfo> {
        let mut tools = vec![
            ToolInfo {
                name: "file.read".to_string(),
                description: "Read file contents".to_string(),
                params: vec!["path".to_string()],
            },
            ToolInfo {
                name: "file.write".to_string(),
                description: "Write file contents".to_string(),
                params: vec!["path".to_string(), "content".to_string()],
            },
            ToolInfo {
                name: "file.edit".to_string(),
                description: "Edit a file via exact string replacement".to_string(),
                params: vec!["path".to_string(), "old".to_string(), "new".to_string()],
            },
            ToolInfo {
                name: "file.list".to_string(),
                description: "List directory contents".to_string(),
                params: vec!["dir".to_string()],
            },
            ToolInfo {
                name: "file.delete".to_string(),
                description: "Delete a file or directory".to_string(),
                params: vec!["path".to_string()],
            },
            ToolInfo {
                name: "search.grep".to_string(),
                description: "grep-style text search".to_string(),
                params: vec!["pattern".to_string(), "path?".to_string()],
            },
            ToolInfo {
                name: "search.find".to_string(),
                description: "Search by filename".to_string(),
                params: vec!["name".to_string(), "path?".to_string()],
            },
            ToolInfo {
                name: "search.semantic".to_string(),
                description: "Lightweight local semantic search: matches code, docs, and paths by meaning — useful when you don't know the exact keyword".to_string(),
                params: vec!["query".to_string(), "k?".to_string(), "path?".to_string()],
            },
            ToolInfo {
                name: "search.index".to_string(),
                description: "Build a persistent index for local semantic search, written to .coai/state/search-index.json".to_string(),
                params: vec!["path?".to_string()],
            },
            ToolInfo {
                name: "exec.run".to_string(),
                description: "Run a shell command; use cwd to target a monorepo subdirectory".to_string(),
                params: vec!["command".to_string(), "cwd?".to_string()],
            },
            ToolInfo {
                name: "exec.build".to_string(),
                description: "Build the project; use cwd to target a monorepo subdirectory".to_string(),
                params: vec!["cwd?".to_string()],
            },
            ToolInfo {
                name: "exec.test".to_string(),
                description: "Run tests; use cwd to target a monorepo subdirectory".to_string(),
                params: vec!["filter?".to_string(), "cwd?".to_string()],
            },
            ToolInfo {
                name: "exec.install".to_string(),
                description: "Install or fetch project dependencies; use cwd to target a monorepo subdirectory".to_string(),
                params: vec!["cwd?".to_string()],
            },
            ToolInfo {
                name: "validate.compile".to_string(),
                description: "Compile check".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "validate.lint".to_string(),
                description: "Lint check".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "validate.test".to_string(),
                description: "Run tests for validation".to_string(),
                params: vec!["filter?".to_string()],
            },
            ToolInfo {
                name: "cleanup.report".to_string(),
                description: "List untracked and ignored entries via git so you can review which are artifacts from this task. Lists only — does not delete.".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "cleanup.remove".to_string(),
                description: "Delete a specific set of paths (space-separated). Only paths inside the working directory that are not tracked by git and are not under .git.".to_string(),
                params: vec!["paths".to_string()],
            },
            ToolInfo {
                name: "net.http_get".to_string(),
                description: "HTTP GET request to fetch web content".to_string(),
                params: vec!["url".to_string()],
            },
            ToolInfo {
                name: "net.http_post".to_string(),
                description: "HTTP POST request".to_string(),
                params: vec!["url".to_string(), "body".to_string()],
            },
            ToolInfo {
                name: "net.http_request".to_string(),
                description: "Generic HTTP request".to_string(),
                params: vec![
                    "method".to_string(),
                    "url".to_string(),
                    "headers?".to_string(),
                    "body?".to_string(),
                ],
            },
            ToolInfo {
                name: "net.search".to_string(),
                description: "Web search returning a result summary. Use for real-time information, news, documentation, etc."
                    .to_string(),
                params: vec!["query".to_string()],
            },
            ToolInfo {
                name: "net.browser".to_string(),
                description: "Open a URL in the default browser".to_string(),
                params: vec!["url".to_string()],
            },
            ToolInfo {
                name: "history.list".to_string(),
                description: "List recent task history records. Returns stored records only; the LLM decides whether to use them."
                    .to_string(),
                params: vec!["limit?".to_string()],
            },
            ToolInfo {
                name: "history.search".to_string(),
                description: "Search task history by keyword, status, or tag. Provides searchable memory; no automatic recommendations."
                    .to_string(),
                params: vec![
                    "query?".to_string(),
                    "limit?".to_string(),
                    "status?".to_string(),
                    "tag?".to_string(),
                ],
            },
            ToolInfo {
                name: "history.show".to_string(),
                description: "Show details of a specific history record".to_string(),
                params: vec!["id".to_string()],
            },
            ToolInfo {
                name: "history.export".to_string(),
                description: "Export task history; format supports json, md, csv".to_string(),
                params: vec!["format?".to_string()],
            },
            ToolInfo {
                name: "history.stats".to_string(),
                description: "Show history statistics including status and tag distribution".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "history.delete".to_string(),
                description: "Delete a specific history record".to_string(),
                params: vec!["id".to_string()],
            },
            ToolInfo {
                name: "memory.read".to_string(),
                description: "Read the explicit project memory file .coai/memory.md".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "memory.search".to_string(),
                description: "Search explicit project memory by keyword. Returns matching lines only; no automatic recommendations.".to_string(),
                params: vec!["query".to_string()],
            },
            ToolInfo {
                name: "memory.append".to_string(),
                description: "Append stable facts, user preferences, common commands, or known pitfalls to project memory".to_string(),
                params: vec!["content".to_string(), "section?".to_string()],
            },
            ToolInfo {
                name: "memory.sections".to_string(),
                description: "List section headings and line numbers in the project memory file".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "memory.delete".to_string(),
                description: "Delete content from project memory by line number or section name".to_string(),
                params: vec!["line?".to_string(), "section?".to_string()],
            },
            ToolInfo {
                name: "memory.edit".to_string(),
                description: "Ensure the project memory file exists and return its editable path".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "memory.write".to_string(),
                description: "Overwrite the project memory file .coai/memory.md".to_string(),
                params: vec!["content".to_string()],
            },
            ToolInfo {
                name: "memory.clear".to_string(),
                description: "Reset the project memory file".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "tools.list".to_string(),
                description: "List all available tools. Provides the tool catalog only; the LLM decides whether to use them.".to_string(),
                params: vec!["category?".to_string()],
            },
            ToolInfo {
                name: "tools.search".to_string(),
                description: "Search tool descriptions by keyword. Returns matching tool references; not a recommendation.".to_string(),
                params: vec!["query".to_string(), "category?".to_string()],
            },
            ToolInfo {
                name: "tools.info".to_string(),
                description: "View the parameters and description of a specific tool".to_string(),
                params: vec!["name".to_string()],
            },
            ToolInfo {
                name: "skills.list".to_string(),
                description:
                    "List the currently available Claude/Codex-compatible skill directory. Returns summaries only; the LLM decides whether to read them."
                        .to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "skills.search".to_string(),
                description: "Search skills by name, description, or SKILL.md content".to_string(),
                params: vec!["query".to_string()],
            },
            ToolInfo {
                name: "skills.read".to_string(),
                description: "Read the full SKILL.md for a specific skill; read it before following its instructions for related tasks.".to_string(),
                params: vec!["name".to_string()],
            },
            ToolInfo {
                name: "git.status".to_string(),
                description: "Show the short git working-tree status".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "git.diff".to_string(),
                description: "Show a git diff with optional staged and path filters".to_string(),
                params: vec!["staged?".to_string(), "path?".to_string()],
            },
            ToolInfo {
                name: "git.add".to_string(),
                description: "Stage specific files; files is a space-separated list".to_string(),
                params: vec!["files".to_string()],
            },
            ToolInfo {
                name: "git.commit".to_string(),
                description: "Commit staged changes".to_string(),
                params: vec!["message".to_string()],
            },
            ToolInfo {
                name: "git.log".to_string(),
                description: "View commit history with optional limit and path filter".to_string(),
                params: vec!["limit?".to_string(), "path?".to_string()],
            },
            ToolInfo {
                name: "git.branch".to_string(),
                description: "Show the current git branch".to_string(),
                params: vec![],
            },
            ToolInfo {
                name: "git.show".to_string(),
                description: "Show the summary and file statistics of a specific commit or HEAD".to_string(),
                params: vec!["rev?".to_string()],
            },
            ToolInfo {
                name: "git.pull".to_string(),
                description: "Pull changes from the remote branch — high-risk operation, requires user confirmation".to_string(),
                params: vec!["remote?".to_string(), "branch?".to_string()],
            },
            ToolInfo {
                name: "git.push".to_string(),
                description: "Push local commits to the remote — high-risk operation, requires user confirmation".to_string(),
                params: vec!["remote?".to_string(), "branch?".to_string()],
            },
            ToolInfo {
                name: "tasks.write".to_string(),
                description: "Maintain a task checklist (todo). Pass the full task list (each item with content and status: pending/in_progress/completed); each call replaces the entire list. Used for planning and progress display in multi-step tasks.".to_string(),
                params: vec!["tasks".to_string()],
            },
            ToolInfo {
                name: "tasks.read".to_string(),
                description: "Read the current task checklist.".to_string(),
                params: vec![],
            },
        ];

        if self.agent_tools_enabled && self.llm_config.is_some() {
            tools.push(ToolInfo {
                name: "agent.spawn".to_string(),
                description:
                    "Dispatch a clearly-scoped subagent subtask. Suitable for parallel exploration, narrow-scope implementation, or independent verification. Returns a summary of the subtask conclusion."
                        .to_string(),
                params: vec![
                    "task".to_string(),
                    "role?".to_string(),
                    "write_scope?".to_string(),
                ],
            });
        }

        tools
    }

    pub fn skill_prompt_context(&self) -> String {
        SkillRegistry::new(&self.workspace).prompt_context()
    }

    pub fn augment_system_prompt(&self, mut prompt: String) -> String {
        if prompt.contains("\n## Skills\n") || prompt.starts_with("## Skills\n") {
            return prompt;
        }
        let skill_context = self.skill_prompt_context();
        if !skill_context.trim().is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&skill_context);
        }
        prompt
    }

    async fn execute_file(&self, method: &str, params: &serde_json::Value) -> Result<ToolResult> {
        let allow_external = self.allow_external_mutations
            && matches!(method, "write" | "edit" | "delete" | "copy" | "move");
        let file = FileTools::new(&self.workspace).with_external_paths(allow_external);

        let result = match method {
            "read" => {
                let path = params["path"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: path".to_string())
                })?;
                file.read(path).await?
            }
            "write" => {
                let path = params["path"]
                    .as_str()
                    .or_else(|| params["filename"].as_str())
                    .or_else(|| params["file"].as_str());
                let content = params["content"]
                    .as_str()
                    .or_else(|| params["data"].as_str())
                    .or_else(|| params["text"].as_str())
                    .or_else(|| params["body"].as_str());
                match (path, content) {
                    (Some(p), Some(c)) => {
                        let change = file.write(p, c).await?;
                        change.summary()
                    }
                    _ => {
                        return Err(CoAIError::Other("path and content are both required. Example: file_write(path=\"x.html\", content=\"...\")".into()));
                    }
                }
            }
            "edit" => {
                let path = params["path"].as_str().or_else(|| params["file"].as_str());
                let old = params["old"]
                    .as_str()
                    .or_else(|| params["search"].as_str())
                    .or_else(|| params["from"].as_str());
                let new = params["new"]
                    .as_str()
                    .or_else(|| params["replace"].as_str())
                    .or_else(|| params["to"].as_str());
                match (path, old, new) {
                    (Some(p), Some(o), Some(n)) => {
                        let change = file.edit(p, o, n).await?;
                        change.summary()
                    }
                    _ => {
                        return Err(CoAIError::Other(
                            "path, old, and new are all required. Example: file_edit(path=\"x.html\", old=\"old text\", new=\"new text\")".into()
                        ));
                    }
                }
            }
            "list" => {
                let dir = params["dir"].as_str().unwrap_or(".");
                let files = file.list(dir).await?;
                serde_json::to_string(&files)?
            }
            "delete" => {
                let path = params["path"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: path".to_string())
                })?;
                file.delete(path).await?;
                String::new()
            }
            _ => return Err(CoAIError::Other(format!("Unknown file method: {}", method))),
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_search(&self, method: &str, params: &serde_json::Value) -> Result<ToolResult> {
        let search = SearchTools::new(&self.workspace);

        let result = match method {
            "grep" => {
                let pattern = params["pattern"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: pattern".to_string())
                })?;
                let path = params["path"].as_str();
                let results = search.grep(pattern, path).await?;
                serde_json::to_string(&results)?
            }
            "find" => {
                let name = params["name"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: name".to_string())
                })?;
                let path = params["path"].as_str();
                let results = search.find(name, path).await?;
                serde_json::to_string(&results)?
            }
            "regex" => {
                let pattern = params["pattern"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: pattern".to_string())
                })?;
                let path = params["path"].as_str();
                let results = search.regex(pattern, path).await?;
                serde_json::to_string(&results)?
            }
            "semantic" => {
                let query = params["query"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: query".to_string())
                })?;
                let k = optional_usize(params, "k")
                    .or_else(|| optional_usize(params, "limit"))
                    .unwrap_or(10);
                let path = params["path"].as_str();
                let results = search.semantic(query, k, path).await?;
                serde_json::to_string(&results)?
            }
            "index" => {
                let path = params["path"].as_str();
                search.index(path).await?
            }
            _ => {
                return Err(CoAIError::Other(format!(
                    "Unknown search method: {}",
                    method
                )))
            }
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_exec(&self, method: &str, params: &serde_json::Value) -> Result<ToolResult> {
        let exec = ExecTools::new(&self.workspace);

        let result = match method {
            "run" => {
                let command = params["command"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: command".to_string())
                })?;
                let cwd = params["cwd"].as_str();
                let output = exec.run(command, cwd).await?;
                serde_json::to_string(&output)?
            }
            "build" => {
                let cwd = params["cwd"].as_str();
                let output = exec.build(cwd).await?;
                serde_json::to_string(&output)?
            }
            "test" => {
                let filter = params["filter"].as_str();
                let cwd = params["cwd"].as_str();
                let output = exec.test(filter, cwd).await?;
                serde_json::to_string(&output)?
            }
            "install" => {
                let cwd = params["cwd"].as_str();
                let output = exec.install(cwd).await?;
                serde_json::to_string(&output)?
            }
            _ => return Err(CoAIError::Other(format!("Unknown exec method: {}", method))),
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_validate(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<ToolResult> {
        let validate = ValidationTools::new(&self.workspace);

        let result = match method {
            "compile" => {
                let result = validate.compile().await?;
                serde_json::to_string(&result)?
            }
            "lint" => {
                let result = validate.lint().await?;
                serde_json::to_string(&result)?
            }
            "format_check" => {
                let result = validate.format_check().await?;
                serde_json::to_string(&result)?
            }
            "test" => {
                let filter = params["filter"].as_str();
                let result = validate.test(filter).await?;
                serde_json::to_string(&result)?
            }
            _ => {
                return Err(CoAIError::Other(format!(
                    "Unknown validate method: {}",
                    method
                )))
            }
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_cleanup(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<ToolResult> {
        let cleanup = CleanupTools::new(&self.workspace);

        let result = match method {
            "report" => {
                let report = cleanup.report().await?;
                serde_json::to_string(&report)?
            }
            "remove" => {
                let paths = params["paths"]
                    .as_str()
                    .or_else(|| params["path"].as_str())
                    .ok_or_else(|| {
                        CoAIError::Other("Missing required parameter: paths".to_string())
                    })?;
                let paths: Vec<String> = paths.split_whitespace().map(|s| s.to_string()).collect();
                if paths.is_empty() {
                    return Err(CoAIError::Other("paths parameter is empty".to_string()));
                }
                let result = cleanup.remove(&paths).await?;
                serde_json::to_string(&result)?
            }
            _ => {
                return Err(CoAIError::Other(format!(
                    "Unknown cleanup method: {}",
                    method
                )))
            }
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_net(&self, method: &str, params: &serde_json::Value) -> Result<ToolResult> {
        let net = NetTools::new(&self.workspace);

        let result = match method {
            "http_get" => {
                let url = params["url"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: url".to_string())
                })?;
                net.http_get(url).await?
            }
            "http_post" => {
                let url = params["url"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: url".to_string())
                })?;
                let body = params["body"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: body".to_string())
                })?;
                net.http_post(url, body).await?
            }
            "http_request" => {
                let method_param = params["method"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: method".to_string())
                })?;
                let url = params["url"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: url".to_string())
                })?;

                let headers = if let Some(headers_value) = params.get("headers") {
                    if let Some(headers_obj) = headers_value.as_object() {
                        let mut headers_map = std::collections::HashMap::new();
                        for (key, value) in headers_obj {
                            if let Some(value_str) = value.as_str() {
                                headers_map.insert(key.clone(), value_str.to_string());
                            }
                        }
                        Some(headers_map)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let body = params["body"].as_str().map(|s| s.to_string());

                net.http_request(method_param, url, headers, body).await?
            }
            "search" => {
                let query = params["query"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: query".to_string())
                })?;
                net.web_search(query).await?
            }
            "browser" => {
                let url = params["url"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: url".to_string())
                })?;
                net.open_browser(url)?
            }
            _ => return Err(CoAIError::Other(format!("Unknown net method: {}", method))),
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_history(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<ToolResult> {
        let history = HistoryTools::new(&self.workspace);

        let result = match method {
            "list" => {
                let limit = optional_usize(params, "limit");
                history.list(limit).await?
            }
            "search" | "query" => {
                let query = params["query"]
                    .as_str()
                    .or_else(|| params["keyword"].as_str())
                    .unwrap_or("");
                let limit = optional_usize(params, "limit");
                let status = params["status"].as_str();
                let tag = params["tag"].as_str().or_else(|| params["tags"].as_str());
                history.search(query, limit, status, tag).await?
            }
            "show" | "get" => {
                let id = params["id"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: id".to_string())
                })?;
                history.show(id).await?
            }
            "export" => {
                let format = parse_export_format(params["format"].as_str().unwrap_or("json"));
                history.export(format).await?
            }
            "stats" => history.stats().await?,
            "delete" => {
                let id = params["id"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: id".to_string())
                })?;
                history.delete(id).await?
            }
            _ => {
                return Err(CoAIError::Other(format!(
                    "Unknown history method: {}",
                    method
                )))
            }
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_memory(&self, method: &str, params: &serde_json::Value) -> Result<ToolResult> {
        let memory = MemoryTools::new(&self.workspace);

        let result = match method {
            "read" => memory.read().await?,
            "search" => {
                let query = params["query"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: query".to_string())
                })?;
                memory.search(query).await?
            }
            "append" => {
                let content = params["content"]
                    .as_str()
                    .or_else(|| params["text"].as_str())
                    .ok_or_else(|| {
                        CoAIError::Other("Missing required parameter: content".to_string())
                    })?;
                let section = params["section"].as_str();
                memory.append(content, section).await?
            }
            "sections" => memory.sections().await?,
            "delete" => {
                if let Some(line) = optional_usize(params, "line") {
                    memory.delete_line(line).await?
                } else if let Some(section) = params["section"].as_str() {
                    memory.delete_section(section).await?
                } else {
                    return Err(CoAIError::Other(
                        "Missing required parameter: line or section".to_string(),
                    ));
                }
            }
            "edit" => memory.edit_path().await?,
            "write" => {
                let content = params["content"]
                    .as_str()
                    .or_else(|| params["text"].as_str())
                    .ok_or_else(|| {
                        CoAIError::Other("Missing required parameter: content".to_string())
                    })?;
                memory.write(content).await?
            }
            "clear" => memory.clear().await?,
            _ => {
                return Err(CoAIError::Other(format!(
                    "Unknown memory method: {}",
                    method
                )))
            }
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_skills(&self, method: &str, params: &serde_json::Value) -> Result<ToolResult> {
        let skills = SkillTools::new(&self.workspace);

        let result = match method {
            "list" => skills.list().await?,
            "search" => {
                let query = params["query"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: query".to_string())
                })?;
                skills.search(query).await?
            }
            "read" => {
                let name = params["name"]
                    .as_str()
                    .or_else(|| params["path"].as_str())
                    .or_else(|| params["skill"].as_str())
                    .ok_or_else(|| {
                        CoAIError::Other("Missing required parameter: name or path".to_string())
                    })?;
                skills.read(name).await?
            }
            _ => {
                return Err(CoAIError::Other(format!(
                    "Unknown skills method: {}",
                    method
                )))
            }
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_tools_reference(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<ToolResult> {
        let result = match method {
            "list" => {
                let category = params["category"].as_str();
                let tools: Vec<serde_json::Value> = self
                    .list_tools()
                    .into_iter()
                    .filter(|tool| {
                        category
                            .map(|category| tool.name.starts_with(&format!("{category}.")))
                            .unwrap_or(true)
                    })
                    .map(|tool| tool.reference())
                    .collect();
                serde_json::to_string_pretty(&tools).map_err(|e| {
                    CoAIError::Other(format!("Failed to serialize tool list: {}", e))
                })?
            }
            "search" => {
                let query = params["query"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: query".to_string())
                })?;
                let query = query.to_lowercase();
                let category = params["category"].as_str();
                let tools: Vec<serde_json::Value> = self
                    .list_tools()
                    .into_iter()
                    .filter(|tool| {
                        category
                            .map(|category| tool.name.starts_with(&format!("{category}.")))
                            .unwrap_or(true)
                    })
                    .filter(|tool| {
                        tool.name.to_lowercase().contains(&query)
                            || tool.description.to_lowercase().contains(&query)
                            || tool
                                .params
                                .iter()
                                .any(|param| param.to_lowercase().contains(&query))
                    })
                    .map(|tool| tool.reference())
                    .collect();
                serde_json::to_string_pretty(&tools).map_err(|e| {
                    CoAIError::Other(format!("Failed to serialize tool search results: {}", e))
                })?
            }
            "info" => {
                let name = params["name"]
                    .as_str()
                    .or_else(|| params["tool"].as_str())
                    .ok_or_else(|| {
                        CoAIError::Other("Missing required parameter: name".to_string())
                    })?;
                let normalized = name.replace('_', ".");
                let tool = self
                    .list_tools()
                    .into_iter()
                    .find(|tool| tool.name == normalized || tool.name == name)
                    .ok_or_else(|| CoAIError::Other(format!("Unknown tool: {}", name)))?;
                serde_json::to_string_pretty(&tool.reference()).map_err(|e| {
                    CoAIError::Other(format!("Failed to serialize tool info: {}", e))
                })?
            }
            _ => {
                return Err(CoAIError::Other(format!(
                    "Unknown tools method: {}",
                    method
                )))
            }
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_git(&self, method: &str, params: &serde_json::Value) -> Result<ToolResult> {
        let git = GitTools::new(&self.workspace);
        let result = match method {
            "status" => git.status().await?,
            "diff" => {
                let staged = params["staged"]
                    .as_bool()
                    .or_else(|| params["staged"].as_str().map(|value| value == "true"))
                    .unwrap_or(false);
                let path = params["path"].as_str();
                git.diff(staged, path).await?
            }
            "add" => {
                let files = params["files"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: files".to_string())
                })?;
                git.add(files).await?
            }
            "commit" => {
                let message = params["message"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: message".to_string())
                })?;
                git.commit(message).await?
            }
            "log" => {
                let limit = optional_usize(params, "limit").unwrap_or(20);
                let path = params["path"].as_str();
                git.log(limit, path).await?
            }
            "branch" => git.branch().await?,
            "show" => {
                let rev = params["rev"].as_str().unwrap_or("HEAD");
                git.show(rev).await?
            }
            "pull" => {
                let remote = params["remote"].as_str();
                let branch = params["branch"].as_str();
                git.pull(remote, branch).await?
            }
            "push" => {
                let remote = params["remote"].as_str();
                let branch = params["branch"].as_str();
                git.push(remote, branch).await?
            }
            _ => return Err(CoAIError::Other(format!("Unknown git method: {}", method))),
        };

        Ok(ToolResult {
            success: result.success,
            output: Some(serde_json::to_value(result)?),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_tasks(&self, method: &str, params: &serde_json::Value) -> Result<ToolResult> {
        let tasks = TaskTools::new(&self.workspace);
        let result = match method {
            "write" => {
                let items: Vec<TaskItem> = serde_json::from_value(params["tasks"].clone())
                    .map_err(|e| CoAIError::Other(format!("Invalid tasks parameter: {}", e)))?;
                tasks.write(items)?
            }
            "read" => tasks.read()?,
            _ => {
                return Err(CoAIError::Other(format!(
                    "Unknown tasks method: {}",
                    method
                )))
            }
        };
        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }

    async fn execute_agent(&self, method: &str, params: &serde_json::Value) -> Result<ToolResult> {
        if !self.agent_tools_enabled {
            return Err(CoAIError::Other(
                "Subagent dispatch is not allowed in this context".to_string(),
            ));
        }

        let llm_config = self.llm_config.clone().ok_or_else(|| {
            CoAIError::Other("LLM not configured; cannot dispatch subagent".to_string())
        })?;

        let result = match method {
            "spawn" => {
                let task = params["task"].as_str().ok_or_else(|| {
                    CoAIError::Other("Missing required parameter: task".to_string())
                })?;
                let role = params["role"].as_str();
                let write_scope = params["write_scope"].as_str();
                let mut agent =
                    AgentTools::new(&self.workspace, llm_config, self.allow_external_mutations);
                if let Some(callback) = &self.progress_callback {
                    agent = agent.with_progress_callback(callback.clone());
                }
                let result = agent.spawn(task, role, write_scope).await?;
                serde_json::to_string(&result)?
            }
            _ => {
                return Err(CoAIError::Other(format!(
                    "Unknown agent method: {}",
                    method
                )))
            }
        };

        Ok(ToolResult {
            success: true,
            output: Some(serde_json::Value::String(result)),
            error: None,
            context_impact: None,
        })
    }
}

fn optional_usize(params: &serde_json::Value, key: &str) -> Option<usize> {
    params[key]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| params[key].as_str().and_then(|value| value.parse().ok()))
}

fn parse_export_format(format: &str) -> ExportFormat {
    match format {
        "md" | "markdown" => ExportFormat::Markdown,
        "csv" => ExportFormat::Csv,
        _ => ExportFormat::Json,
    }
}

fn parameter_type(name: &str) -> &'static str {
    match name {
        "limit" | "line" | "k" => "integer",
        "staged" => "boolean",
        _ => "string",
    }
}

fn parameter_description(tool: &str, name: &str) -> String {
    match (tool, name) {
        ("file.read", "path")
        | ("file.write", "path")
        | ("file.edit", "path")
        | ("file.delete", "path") => {
            "Path relative to the current working directory; only use an external absolute path when the user explicitly requires it".into()
        }
        ("file.write", "content") => "Complete content to write to the file".into(),
        ("file.edit", "old") => "Original text to replace; must match exactly".into(),
        ("file.edit", "new") => "Replacement text".into(),
        ("search.grep", "pattern") => "Text or regex pattern to search for".into(),
        ("search.find", "name") => "Filename keyword".into(),
        ("search.semantic", "query") => "Natural-language query describing what you are looking for".into(),
        ("search.semantic", "k") => "Maximum number of relevant snippets to return".into(),
        ("search.semantic", "path") => "Optional search scope, e.g. src or docs".into(),
        ("search.index", "path") => "Optional indexing scope, e.g. src or docs".into(),
        ("exec.run", "command") => "Shell command to execute".into(),
        ("exec.run", "cwd")
        | ("exec.build", "cwd")
        | ("exec.test", "cwd")
        | ("exec.install", "cwd") => {
            "Optional working directory; must be an existing directory inside the workspace — useful for monorepo sub-project validation, e.g. packages/api".into()
        }
        ("cleanup.remove", "paths") => {
            "Space-separated list of paths to delete; must be inside the working directory, not under .git, and not tracked by git".into()
        }
        ("net.search", "query") => "Search query".into(),
        ("net.http_get", "url") | ("net.browser", "url") => "Full URL".into(),
        ("history.search", "query") | ("memory.search", "query") => "Search keyword".into(),
        ("skills.search", "query") => "Skill search keyword".into(),
        ("skills.read", "name") => "Skill name, directory name, or path returned by skills.list".into(),
        ("memory.append", "section") => "Memory section name, e.g. Notes, Commands, Preferences".into(),
        ("git.diff", "staged") => "Whether to view staged changes".into(),
        ("git.diff", "path") => "Optional path filter".into(),
        ("git.add", "files") => "Space-separated list of files to stage".into(),
        ("git.commit", "message") => "Commit message".into(),
        ("git.log", "limit") => "Maximum number of commits to return".into(),
        ("git.log", "path") => "Optional path filter".into(),
        ("git.show", "rev") => "Commit reference, e.g. HEAD or a commit SHA".into(),
        ("git.pull", "remote") | ("git.push", "remote") => "Optional remote name, e.g. origin".into(),
        ("git.pull", "branch") | ("git.push", "branch") => "Optional branch name".into(),
        _ => name.to_string(),
    }
}

fn tool_examples(name: &str) -> Vec<serde_json::Value> {
    match name {
        "file.read" => vec![serde_json::json!({"path": "src/main.rs"})],
        "file.write" => vec![serde_json::json!({"path": "notes.txt", "content": "hello\n"})],
        "file.edit" => vec![serde_json::json!({"path": "src/main.rs", "old": "foo", "new": "bar"})],
        "search.grep" => vec![serde_json::json!({"pattern": "TODO", "path": "src"})],
        "search.semantic" => {
            vec![
                serde_json::json!({"query": "resume interrupted long task", "path": "src", "k": 5}),
            ]
        }
        "search.index" => vec![serde_json::json!({"path": "src"})],
        "exec.run" => vec![serde_json::json!({"command": "cargo test", "cwd": "."})],
        "exec.test" => vec![serde_json::json!({"filter": "tool_registry", "cwd": "."})],
        "cleanup.report" => vec![serde_json::json!({})],
        "cleanup.remove" => vec![serde_json::json!({"paths": "scratch.tmp build-output/"})],
        "net.search" => vec![serde_json::json!({"query": "Rust crossterm documentation"})],
        "history.search" => vec![serde_json::json!({"query": "auth", "limit": 5})],
        "memory.append" => vec![
            serde_json::json!({"content": "Run tests with: cargo test", "section": "Commands"}),
        ],
        "tools.search" => vec![serde_json::json!({"query": "git", "category": "git"})],
        "skills.list" => vec![serde_json::json!({})],
        "skills.search" => vec![serde_json::json!({"query": "docx"})],
        "skills.read" => vec![serde_json::json!({"name": "docx"})],
        "git.status" => vec![serde_json::json!({})],
        "git.diff" => vec![serde_json::json!({"staged": false, "path": "src/main.rs"})],
        "git.add" => vec![serde_json::json!({"files": "src/main.rs tests/integration_test.rs"})],
        "git.commit" => vec![serde_json::json!({"message": "feat: improve tui permissions"})],
        "git.log" => vec![serde_json::json!({"limit": 10, "path": "src"})],
        "git.branch" => vec![serde_json::json!({})],
        "git.show" => vec![serde_json::json!({"rev": "HEAD"})],
        "git.pull" => vec![serde_json::json!({"remote": "origin", "branch": "main"})],
        "git.push" => vec![serde_json::json!({"remote": "origin", "branch": "main"})],
        _ => Vec::new(),
    }
}
