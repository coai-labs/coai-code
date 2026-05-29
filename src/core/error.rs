use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoAIError {
    #[error("File operation error: {0}")]
    File(String),

    #[error("Command execution error: {0}")]
    Command(String),

    #[error("Context error: {0}")]
    Context(String),

    #[error("History error: {0}")]
    History(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Security error: {0}")]
    Security(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("Config error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("Checkpoint error: {0}")]
    Checkpoint(String),

    #[error("Command parse error: {0}")]
    CommandParse(String),

    #[error("Command execution error: {0}")]
    CommandExecution(String),

    #[error("Atomic task error: {0}")]
    AtomicTask(String),

    #[error("TUI error: {0}")]
    Tui(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CoAIError>;
