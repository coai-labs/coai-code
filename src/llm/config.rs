use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMConfig {
    pub provider: LLMProvider,
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub temperature: f32,
    pub max_tokens: usize,
    pub system_prompt: Option<String>,
    pub flash_model: Option<String>,
    pub extra_params: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMProvider {
    #[serde(rename = "openai")]
    OpenAI,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai_compatible")]
    OpenAICompatible,
    #[serde(rename = "ollama")]
    Ollama,
    #[serde(rename = "custom")]
    Custom,
}

impl Default for LLMConfig {
    fn default() -> Self {
        Self {
            provider: LLMProvider::Anthropic,
            model: "deepseek-v4-pro".to_string(),
            api_key: None,
            base_url: Some("https://api.deepseek.com/anthropic".to_string()),
            temperature: 0.2,
            max_tokens: 64_000,
            system_prompt: None,
            flash_model: Some("deepseek-v4-flash".to_string()),
            extra_params: HashMap::from([(
                "output_config".to_string(),
                serde_json::json!({ "effort": "max" }),
            )]),
        }
    }
}

impl LLMConfig {
    pub fn openai(model: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            provider: LLMProvider::OpenAI,
            model: model.into(),
            api_key: Some(api_key.into()),
            base_url: None,
            temperature: 0.7,
            max_tokens: 4096,
            system_prompt: None,
            flash_model: None,
            extra_params: HashMap::new(),
        }
    }

    pub fn anthropic(model: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            provider: LLMProvider::Anthropic,
            model: model.into(),
            api_key: Some(api_key.into()),
            base_url: None,
            temperature: 0.7,
            max_tokens: 4096,
            system_prompt: None,
            flash_model: None,
            extra_params: HashMap::new(),
        }
    }

    pub fn ollama(model: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            provider: LLMProvider::Ollama,
            model: model.into(),
            api_key: None,
            base_url: Some(base_url.into()),
            temperature: 0.7,
            max_tokens: 4096,
            system_prompt: None,
            flash_model: None,
            extra_params: HashMap::new(),
        }
    }

    pub fn openai_compatible(
        model: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            provider: LLMProvider::OpenAICompatible,
            model: model.into(),
            api_key,
            base_url: Some(base_url.into()),
            temperature: 0.7,
            max_tokens: 4096,
            system_prompt: None,
            flash_model: None,
            extra_params: HashMap::new(),
        }
    }

    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = temp;
        self
    }

    pub fn with_max_tokens(mut self, max: usize) -> Self {
        self.max_tokens = max;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn api_endpoint(&self) -> String {
        match &self.provider {
            LLMProvider::OpenAI => "https://api.openai.com/v1/chat/completions".to_string(),
            LLMProvider::Anthropic => "https://api.anthropic.com/v1/messages".to_string(),
            LLMProvider::OpenAICompatible | LLMProvider::Custom => self
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:8000/v1/chat/completions".to_string()),
            LLMProvider::Ollama => self
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434/api/chat".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: ToolChoice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: Option<String>,
    pub image_url: Option<ImageUrl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    #[serde(untagged)]
    Specific {
        name: String,
    },
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Content::Text(content.into()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Content::Text(content.into()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Content::Text(content.into()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: Content::Text(content.into()),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_reasoning(
        content: impl Into<String>,
        reasoning: impl Into<String>,
        tool_calls: Option<Vec<ToolCall>>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: Content::Text(content.into()),
            tool_calls,
            tool_call_id: None,
            reasoning_content: Some(reasoning.into()),
        }
    }

    pub fn tool_result(id: impl Into<String>, result: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Content::Text(result.into()),
            tool_calls: None,
            tool_call_id: Some(id.into()),
            reasoning_content: None,
        }
    }
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}
