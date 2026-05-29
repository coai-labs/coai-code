//! Session persistence — save/load/list/resume conversation history.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::llm::config::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub description: String,
    pub messages: Vec<SerializableMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<String>, // JSON string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

pub struct SessionStore {
    dir: PathBuf,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStore {
    pub fn new() -> Self {
        let dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("coai")
            .join("sessions");
        Self::with_dir(dir)
    }

    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = std::fs::create_dir_all(&dir);
        Self { dir }
    }

    /// List all sessions, newest first
    pub fn list(&self) -> Vec<Session> {
        let mut sessions: Vec<Session> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                if entry
                    .path()
                    .extension()
                    .map(|e| e == "json")
                    .unwrap_or(false)
                {
                    if let Ok(data) = std::fs::read_to_string(entry.path()) {
                        if let Ok(s) = serde_json::from_str::<Session>(&data) {
                            sessions.push(s);
                        }
                    }
                }
            }
        }
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions
    }

    /// Load a session by id
    pub fn load(&self, id: &str) -> Option<Session> {
        let path = self.dir.join(format!("{}.json", id));
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
    }

    /// Save a session
    pub fn save(&self, session: &Session) {
        let path = self.dir.join(format!("{}.json", session.id));
        if let Ok(json) = serde_json::to_string_pretty(session) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Delete a session
    pub fn delete(&self, id: &str) -> bool {
        let path = self.dir.join(format!("{}.json", id));
        std::fs::remove_file(path).is_ok()
    }
}

/// Convert internal Message to SerializableMessage for storage
pub fn message_to_serializable(m: &Message) -> SerializableMessage {
    SerializableMessage {
        role: format!("{:?}", m.role).to_lowercase(),
        content: match &m.content {
            crate::llm::config::Content::Text(t) => t.clone(),
            crate::llm::config::Content::Parts(parts) => {
                // Serialize parts as JSON so they survive save/load roundtrip
                serde_json::to_string(parts).unwrap_or_else(|_| "[serialize-error]".into())
            }
        },
        tool_calls: m
            .tool_calls
            .as_ref()
            .map(|tc| serde_json::to_string(tc).unwrap_or_default()),
        tool_call_id: m.tool_call_id.clone(),
        reasoning_content: m.reasoning_content.clone(),
    }
}

/// Convert SerializableMessage back to Message for resuming
pub fn serializable_to_message(m: &SerializableMessage) -> Message {
    use crate::llm::config::{Content, Message, Role, ToolCall};

    let role = match m.role.as_str() {
        "system" => Role::System,
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::User,
    };

    let tool_calls = m
        .tool_calls
        .as_ref()
        .and_then(|s| serde_json::from_str::<Vec<ToolCall>>(s).ok());

    let content = if let Ok(parts) =
        serde_json::from_str::<Vec<crate::llm::config::ContentPart>>(&m.content)
    {
        Content::Parts(parts)
    } else {
        Content::Text(m.content.clone())
    };

    Message {
        role,
        content,
        tool_calls,
        tool_call_id: m.tool_call_id.clone(),
        reasoning_content: m.reasoning_content.clone(),
    }
}

/// Create a new session from user input
pub fn new_session(description: &str) -> Session {
    let now = chrono::Utc::now();
    Session {
        id: uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("")
            .to_string(),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        description: if description.chars().count() > 80 {
            format!("{}...", description.chars().take(80).collect::<String>())
        } else {
            description.to_string()
        },
        messages: vec![],
    }
}
