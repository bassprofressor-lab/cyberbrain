//! The embedded web UI (SPEC §13): `ui/dist`, compiled into the binary with `rust-embed`.
//! Nothing is fetched at runtime, which is what keeps the egress register at its two
//! entries (SPEC §12.1).
//!
//! The `Content-Security-Policy` is sent as a **header** (SPEC §8.1). The page carries the
//! same policy in a `<meta>`, but `frame-ancestors` is ignored there, so the header is the
//! only thing that actually stops the UI being framed. The header is read from the built
//! page rather than restated here, so the inline-script hash the bundler computed cannot
//! drift from the one the header allows; `frame-ancestors 'none'` is appended.

use super::error::ApiError;
#[cfg(feature = "ui")]
use axum::http::header;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[cfg(feature = "ui")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../ui/dist"]
pub struct Ui;

/// What the header falls back to when the built page carries no `<meta>` policy. The
/// inline theme bootstrap is then blocked (no hash to allow), which degrades the theme,
/// not the security; the startup message says so.
const FALLBACK_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data:; font-src 'self'; connect-src 'self'; manifest-src 'self'; \
     object-src 'none'; base-uri 'none'; form-action 'none'";

/// The policy the built page declares in its `<meta http-equiv="Content-Security-Policy">`.
#[cfg(not(feature = "ui"))]
pub fn meta_csp() -> Option<String> {
    None
}

/// The policy the built page declares in its `<meta http-equiv="Content-Security-Policy">`.
#[cfg(feature = "ui")]
pub fn meta_csp() -> Option<String> {
    let index = Ui::get("index.html")?;
    let html = String::from_utf8_lossy(&index.data);
    let at = html.find("Content-Security-Policy")?;
    let rest = &html[at..];
    let start = rest.find("content=\"")? + "content=\"".len();
    let end = rest[start..].find('"')?;
    Some(unescape_attr(&rest[start..start + end]))
}

/// The bundler writes the attribute HTML-escaped (`'` as `&#39;`); the header wants the
/// characters back.
#[cfg(feature = "ui")]
fn unescape_attr(s: &str) -> String {
    s.replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The header value: the page's own policy plus the directive only a header can carry.
pub fn csp_header() -> HeaderValue {
    let base = meta_csp().unwrap_or_else(|| FALLBACK_CSP.to_string());
    let base = base.trim().trim_end_matches(';').to_string();
    let value = if base.contains("frame-ancestors") {
        base
    } else {
        format!("{base}; frame-ancestors 'none'")
    };
    HeaderValue::from_str(&value)
        .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"))
}

#[cfg(feature = "ui")]
fn etag_of(file: &rust_embed::EmbeddedFile) -> String {
    let h = file.metadata.sha256_hash();
    let hex: String = h.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

fn clean_path(uri: &Uri) -> Option<String> {
    let raw = uri.path().trim_start_matches('/');
    let mut parts = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

#[cfg(feature = "ui")]
fn serve_file(path: &str, file: rust_embed::EmbeddedFile, headers: &HeaderMap) -> Response {
    let etag = etag_of(&file);
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") || path.starts_with("fonts/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"));
    let mut resp = if matches {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        file.data.into_owned().into_response()
    };
    let h = resp.headers_mut();
    if !matches {
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime.as_ref())
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        );
    }
    h.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).expect("hex etag"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    resp
}

/// Everything that is not `/api/v1/...`: the embedded page and its assets. An unknown
/// path without an extension gets `index.html`, so a deep link into the hash router
/// works; an unknown file with an extension is a 404, because serving HTML where a
/// stylesheet was expected only moves the error somewhere confusing.
pub async fn fallback(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let Some(path) = clean_path(&uri) else {
        return ApiError::bad_request("path escapes the site root").into_response();
    };
    if path == "api" || path.starts_with("api/") {
        return ApiError::not_found(format!("no such route: {} /{path}", method)).into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return ApiError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "bad-request",
            format!("{method} is not allowed on /{path}"),
        )
        .into_response();
    }
    let path = if path.is_empty() {
        "index.html".to_string()
    } else {
        path
    };
    #[cfg(feature = "ui")]
    {
        if let Some(f) = Ui::get(&path) {
            return serve_file(&path, f, &headers);
        }
        let last = path.rsplit('/').next().unwrap_or("");
        if !last.contains('.')
            && let Some(index) = Ui::get("index.html")
        {
            return serve_file("index.html", index, &headers);
        }
        ApiError::not_found(format!("/{path} is not part of the embedded UI")).into_response()
    }
    // Built without the `ui` feature: the API is fully there, the page is not. Say which,
    // because "404" alone would send someone looking for a routing bug.
    #[cfg(not(feature = "ui"))]
    {
        let _ = &headers;
        ApiError::not_found(format!(
            "/{path}: this binary was built without the `ui` feature, so no web page is \
             embedded. The HTTP API under /api/v1 is unaffected."
        ))
        .into_response()
    }
}

/// True when the bundle is actually embedded, for the startup message.
pub fn bundle_present() -> bool {
    #[cfg(feature = "ui")]
    {
        Ui::get("index.html").is_some()
    }
    #[cfg(not(feature = "ui"))]
    {
        false
    }
}
