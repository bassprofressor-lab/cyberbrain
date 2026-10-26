//! Dictation: the browser records, this hands the recording to `App::transcribe` and gives
//! the text back. Nothing is written here; the text lands in the field the person was
//! typing into, and saving it is the ordinary write with its PII check.
//!
//! The origin rule in `origin.rs` covers this route like every other POST, so a page on
//! another site cannot send audio through the operator's whisper-server.

use super::ServeState;
use super::error::{ApiError, ApiResult};
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use cyberbrain_core::Error;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct TranscriptionStatus {
    /// `transcription.base_url` is set. Says nothing about whether the server answers:
    /// asking would cost a connection on every page load, and the first dictation says it.
    pub enabled: bool,
    pub language: String,
    pub model_label: String,
    pub max_upload_mb: u64,
}

#[derive(Serialize)]
pub struct Dictated {
    pub text: String,
    pub language: Option<String>,
    pub duration_s: Option<f64>,
    pub elapsed_ms: u64,
}

pub async fn status(State(st): State<Arc<ServeState>>) -> Json<TranscriptionStatus> {
    let t = &st.app.config().transcription;
    Json(TranscriptionStatus {
        enabled: t.base_url.as_deref().is_some_and(|u| !u.trim().is_empty()),
        language: t.language.clone(),
        model_label: t.model_label.clone(),
        max_upload_mb: t.max_upload_mb,
    })
}

pub async fn dictate(
    State(st): State<Arc<ServeState>>,
    headers: HeaderMap,
    audio: Bytes,
) -> ApiResult<Json<Dictated>> {
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let file_name = match mime.split(';').next().unwrap_or_default().trim() {
        "audio/webm" => "dictation.webm",
        "audio/ogg" => "dictation.ogg",
        "audio/mp4" => "dictation.m4a",
        "audio/wav" | "audio/x-wav" => "dictation.wav",
        _ => "dictation",
    };
    match st.app.transcribe(&audio, file_name, &mime).await {
        Ok(t) => Ok(Json(Dictated {
            text: t.text,
            language: t.language,
            duration_s: t.duration_s,
            elapsed_ms: t.elapsed_ms,
        })),
        // The store is fine and so is the request; the server beside it is not there or
        // not well. 503 tells the page to say so rather than to report a fault here.
        Err(Error::Llm(m)) => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "transcription-unavailable",
            m,
        )),
        Err(e) => Err(e.into()),
    }
}
