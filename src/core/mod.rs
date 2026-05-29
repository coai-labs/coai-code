//! # Core module
//!
//! Core types, error handling, and foundational trait definitions.

pub mod error;
pub mod types;

pub use error::{CoAIError, Result};
pub use types::*;
