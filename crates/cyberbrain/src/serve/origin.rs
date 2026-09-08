//! Who is allowed to change something: the request has to come from this page.
//!
//! Every route here binds to loopback and asks for no credentials, and §8.1 defended that by
//! saying there is nothing remote to authenticate. That argument covers *reading* by a
//! program on this machine. It does not cover a **web page the user happens to have open**,
//! because a browser will send a cross-site request to `127.0.0.1` on that page's say-so.
//! Nothing is read back — the same-origin rule sees to that — but a request that only needs
//! to *arrive* is enough when it deletes something.
//!
//! It was not theoretical. `POST /api/v1/policy/retention/apply?dry_run=false` erases every
//! note past its retention, and it took a self-submitting form on any website: a form post
//! is a "simple request", so there is no preflight to refuse it. `POST /scan?full=true` the
//! same. The rest of the writing routes were protected only by accident — they take
//! `axum::Json`, which insists on `application/json`, which forces a preflight. An accident
//! is not a policy, and the accident had two holes in it.
//!
//! So every state-changing request is checked, and the rule is the terminal's rule (§8.3)
//! applied to the rest of the surface:
//!
//! - `Origin` present and not ours → refused. That is a page somewhere else.
//! - `Sec-Fetch-Site` present and not `same-origin` or `none` → refused. Browsers send this
//!   on every request and scripts cannot forge it, so it catches the cases where `Origin` is
//!   absent by design.
//! - Neither present → allowed. That is a program on this machine, which needs no browser to
//!   read this store and gains nothing from being refused here.
//!
//! Reads are untouched. A cross-site `GET` cannot see its own answer, and refusing them
//! would break nothing an attacker relies on while breaking every link into the page.

use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Whether this request may change something. `Err` carries what to say.
pub fn allowed(
    method: &Method,
    origin: Option<&str>,
    site: Option<&str>,
    ours: &[String],
) -> Result<(), String> {
    if !matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return Ok(());
    }
    if let Some(origin) = origin
        && !ours.iter().any(|o| o == origin)
    {
        return Err(format!(
            "a request from {origin} may not change this store. This server answers the page \
             it serves and programs on this machine, not other websites."
        ));
    }
    // `none` is a user typing an address or opening a bookmark; `same-origin` is our page.
    // `cross-site` and `same-site` are somebody else's.
    if let Some(site) = site
        && site != "same-origin"
        && site != "none"
    {
        return Err(format!(
            "a {site} request may not change this store. This server answers the page it \
             serves and programs on this machine, not other websites."
        ));
    }
    Ok(())
}

pub async fn guard(ours: std::sync::Arc<Vec<String>>, req: Request, next: Next) -> Response {
    // Owned before the await: a borrow of the request that outlived the decision would make
    // this future non-Send, and the runtime needs it to be.
    let (origin, site, method) = {
        let headers = req.headers();
        let owned = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        (
            owned(header::ORIGIN.as_str()),
            owned("sec-fetch-site"),
            req.method().clone(),
        )
    };
    match allowed(&method, origin.as_deref(), site.as_deref(), &ours) {
        Ok(()) => next.run(req).await,
        // The taxonomy's policy refusal (SPEC §8.1): understood and declined.
        Err(why) => (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": { "code": "policy-refusal", "message": why, "exit_code": 3 }
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours() -> Vec<String> {
        vec![
            "http://127.0.0.1:7777".to_string(),
            "http://localhost:7777".to_string(),
        ]
    }

    #[test]
    fn reading_is_never_refused_whoever_asks() {
        for site in [None, Some("cross-site"), Some("same-site")] {
            assert!(
                allowed(&Method::GET, Some("https://evil.example"), site, &ours()).is_ok(),
                "a cross-site read cannot see its own answer; refusing it protects nothing"
            );
        }
    }

    #[test]
    fn our_own_page_may_change_things() {
        assert!(
            allowed(
                &Method::POST,
                Some("http://127.0.0.1:7777"),
                Some("same-origin"),
                &ours()
            )
            .is_ok()
        );
        assert!(allowed(&Method::POST, Some("http://localhost:7777"), None, &ours()).is_ok());
    }

    /// The failure this exists for: a form on any website, posted to loopback. A form post
    /// is a simple request, so no preflight stands in its way — only this.
    #[test]
    fn a_form_on_another_website_may_not() {
        let why = allowed(
            &Method::POST,
            Some("https://evil.example"),
            Some("cross-site"),
            &ours(),
        )
        .unwrap_err();
        assert!(why.contains("evil.example"), "{why}");
    }

    /// A form post sends `Origin` in current browsers, but not every request does. This is
    /// the header that catches the rest, and a script cannot set it.
    #[test]
    fn a_cross_site_request_without_an_origin_is_still_refused() {
        let why = allowed(&Method::POST, None, Some("cross-site"), &ours()).unwrap_err();
        assert!(why.contains("cross-site"), "{why}");
        assert!(allowed(&Method::POST, None, Some("same-site"), &ours()).is_err());
    }

    /// curl, a script, our own tests. It has every ability this would deny it, so denying it
    /// protects nothing and breaks everything that is not a browser.
    #[test]
    fn a_program_on_this_machine_is_admitted() {
        assert!(allowed(&Method::POST, None, None, &ours()).is_ok());
        assert!(allowed(&Method::DELETE, None, Some("none"), &ours()).is_ok());
    }

    #[test]
    fn every_method_that_changes_something_is_checked() {
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(
                allowed(&method, Some("https://evil.example"), None, &ours()).is_err(),
                "{method} was not checked"
            );
        }
    }
}
