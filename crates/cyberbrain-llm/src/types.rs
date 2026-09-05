//! Public request and response types. These are this crate's vocabulary; the wire format
//! of the OpenAI-compatible API lives in `wire.rs` and never leaks out.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// A chat completion request. Everything not set falls back to `LlmConfig`.
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    /// Overrides `LlmConfig::max_tokens`.
    pub max_tokens: Option<u32>,
    /// Overrides `LlmConfig::temperature`.
    pub temperature: Option<f32>,
    /// Overrides `LlmConfig::model`.
    pub model: Option<String>,
    /// Name of the feature making the call, for the audit row.
    pub task: Option<&'static str>,
}

impl ChatRequest {
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    pub fn with_task(mut self, task: &'static str) -> Self {
        self.task = Some(task);
        self
    }
}

/// Token counts as reported by the endpoint. Absent fields stay absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatResponse {
    /// Assistant text of the first choice. Empty string if the model returned nothing.
    pub content: String,
    /// Model name as reported by the endpoint, which may differ from the one requested
    /// (PAIR and Ollama both resolve aliases).
    pub model: Option<String>,
    pub finish_reason: Option<String>,
    /// `None` when the endpoint did not report usage.
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub owned_by: Option<String>,
}
