//! Type definitions for CoAI Code
//!
//! This module contains all core type definitions following the "trust LLM" principle.
//! Types are simple and non-prescriptive, allowing LLM to make decisions.

pub mod atomic_task;
pub mod command;

// Re-export commonly used types
pub use atomic_task::{AtomicTask, AtomicTaskList, AtomicTaskStatus};
pub use command::{
    ArgumentValueType, Command, CommandArgument, CommandError, CommandErrorType,
    CommandExecutionResult, CommandFlag, CommandHandler, CommandHelp, CommandNextAction,
    CommandParseResult, CommandRegistry, CommandSource,
};
