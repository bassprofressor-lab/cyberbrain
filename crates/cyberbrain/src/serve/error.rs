//! The error contract (SPEC §8.1): `{ error: { code, message, exit_code } }`, where
//! `exit_code` is exactly what `cyberbrain_core::Error::exit_code()` gives the CLI. One
//! failure taxonomy, two front ends. The HTTP status is *derived* from that taxonomy:
//! 400 for a user error (1), 500 for an internal one (2), 403 for a policy refusal (3),
//! plus 404 for a missing note or citation and 409 for the two typed write outcomes.
//!
//! `code` is the closed set `ApiErrorCode` in `ui/src/api/types.ts`; the finer CLI variant
//! name travels alongside as `variant` so the two front ends stay traceable to each other.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cyberbrain_core::Error;
use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub exit_code: i32,
    /// Extra members of the `error` object: `hold`, `cap`, `current_updated`, `variant`.
    pub extra: Map<String, Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            exit_code: exit_code_for(status),
            extra: Map::new(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad-request", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not-found", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    /// A 409 with the given code; `exit_code` follows the *taxonomy*, not the status:
    /// a PII hold is a policy decision (3), a write conflict a user-side retry (1).
    pub fn conflict(code: &'static str, message: impl Into<String>, exit_code: i32) -> Self {
        let mut e = Self::new(StatusCode::CONFLICT, code, message);
        e.exit_code = exit_code;
        e
    }

    pub fn with(mut self, key: &str, value: impl Serialize) -> Self {
        self.extra.insert(
            key.to_string(),
            serde_json::to_value(value).unwrap_or(Value::Null),
        );
        self
    }

    pub fn body(&self) -> Value {
        let mut error = Map::new();
        error.insert("code".into(), json!(self.code));
        error.insert("message".into(), json!(self.message));
        error.insert("exit_code".into(), json!(self.exit_code));
        for (k, v) in &self.extra {
            error.insert(k.clone(), v.clone());
        }
        json!({ "error": Value::Object(error) })
    }
}

/// The CLI's exit code for an HTTP status that did not come from a core error.
fn exit_code_for(status: StatusCode) -> i32 {
    if status == StatusCode::FORBIDDEN {
        3
    } else if status.is_server_error() {
        2
    } else {
        1
    }
}

/// The CLI's variant name (`main.rs::error_code`), reproduced here because the binary's
/// dispatch is not a library. Kept in the same order as the enum so a new variant is
/// noticed in both places.
pub fn variant_name(e: &Error) -> &'static str {
    match e {
        Error::Io { .. } => "io",
        Error::Frontmatter { .. } => "frontmatter",
        Error::BadCitation(_) => "bad-citation",
        Error::NoSuchNote(_) => "no-such-note",
        Error::BadRing(_) => "bad-ring",
        Error::RingCapExceeded { .. } => "ring-cap-exceeded",
        Error::StoreIntegrity(_) => "store-integrity",
        Error::Index(_) => "index",
        Error::Embed(_) => "embed",
        Error::EmbeddingProfileMismatch { .. } => "embedding-profile-mismatch",
        Error::Llm(_) => "llm",
        Error::PolicyRefusal { .. } => "policy-refusal",
        Error::Config(_) => "config",
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        let exit_code = e.exit_code();
        let (status, code) = match &e {
            Error::NoSuchNote(_) => (StatusCode::NOT_FOUND, "not-found"),
            Error::RingCapExceeded { .. } => (StatusCode::BAD_REQUEST, "ring-cap-exceeded"),
            Error::Frontmatter { .. } => (StatusCode::BAD_REQUEST, "bad-frontmatter"),
            Error::PolicyRefusal { .. } => (StatusCode::FORBIDDEN, "policy-refusal"),
            Error::EmbeddingProfileMismatch { .. } => {
                (StatusCode::INTERNAL_SERVER_ERROR, "profile-mismatch")
            }
            _ if exit_code == 1 => (StatusCode::BAD_REQUEST, "bad-request"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        debug_assert_eq!(
            exit_code_for(status),
            exit_code,
            "status and exit code must come from the same taxonomy"
        );
        let mut err = ApiError {
            status,
            code,
            message: e.to_string(),
            exit_code,
            extra: Map::new(),
        };
        if let Error::RingCapExceeded { actual, cap, .. } = &e {
            // `used` (the rings without this note) is filled in by the write handler, which
            // has the store at hand; here it is the best this error alone can say.
            err = err.with(
                "cap",
                json!({ "tokens": cap, "used": actual, "would_be": actual }),
            );
        }
        err.with("variant", variant_name(&e))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut resp = (self.status, axum::Json(self.body())).into_response();
        resp.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
        resp
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;
