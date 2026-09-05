//! Wire format of the OpenAI-compatible chat API, private to this crate. Lenient on
//! input: every field a server might omit is `Option`, and unknown fields are ignored,
//! because llama.cpp, vLLM, Ollama and LM Studio each add their own.

use serde::{Deserialize, Serialize};

use crate::types::{ChatMessage, Usage};

#[derive(Debug, Serialize)]
pub(crate) struct ChatCompletionRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Serialize)]
pub(crate) struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionResponse {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Choice {
    #[serde(default)]
    pub message: Option<WireMessage>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireMessage {
    #[serde(default)]
    pub content: Option<String>,
}

/// One SSE `data:` payload of a streamed completion.
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionChunk {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChunkChoice {
    #[serde(default)]
    pub delta: Option<WireMessage>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct WireUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u32>,
    /// OpenAI and Ollama both report prompt-cache hits here. Absent on servers that do not.
    #[serde(default)]
    pub prompt_tokens_details: Option<WirePromptTokensDetails>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct WirePromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

impl From<WireUsage> for Usage {
    fn from(w: WireUsage) -> Self {
        Usage {
            prompt_tokens: w.prompt_tokens,
            completion_tokens: w.completion_tokens,
            total_tokens: w.total_tokens,
            cached_prompt_tokens: w.prompt_tokens_details.and_then(|d| d.cached_tokens),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelsResponse {
    #[serde(default)]
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub owned_by: Option<String>,
}

/// The `{"error": {...}}` envelope most servers use on 4xx/5xx.
#[derive(Debug, Deserialize)]
pub(crate) struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum ErrorBody {
    Structured { message: String },
    Plain(String),
}

impl ErrorBody {
    pub fn message(&self) -> &str {
        match self {
            ErrorBody::Structured { message } => message,
            ErrorBody::Plain(s) => s,
        }
    }
}
