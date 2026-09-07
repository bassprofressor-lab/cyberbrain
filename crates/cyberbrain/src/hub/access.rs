//! Who may see what, and the two-person rule for seeing activity.
//!
//! # The shape, and why
//!
//! The hub separates two questions that tend to land on one screen: *is the collection
//! working* and *what did this person do*. The first is daily administration. The second is
//! a procedure with a reason, and it is modelled on how works agreements handle access to
//! video recordings — not forbidden, but never alone and never unnoticed.
//!
//! - **Administration** sees state: devices, gaps, versions, seats, licence. Not activity.
//! - **Auditor**, named by management, sees activity rows — only inside an approved window.
//! - **Countersigner**, the works council or a named second person, approves each request
//!   and can read every request ever made.
//! - **Individuals** see their own rows, always: a person who cannot see what was recorded
//!   about them has no way to contest it.
//!
//! # What this does and does not enforce
//!
//! It enforces the *route*: through this program, activity rows are unreachable without a
//! request that somebody else approved, inside a window that closes itself, and every step
//! is a row in the hub's own hash chain — so the question "who looked, and why" has an
//! answer that cannot be quietly edited.
//!
//! It does **not** defend against someone with file access to the hub's database. They can
//! open it with any SQLite tool, and reading leaves no trace anywhere. That is why the hub
//! belongs on a machine with controlled access, and why this is a procedure supported by
//! software rather than a guarantee made by it. Saying otherwise in a works agreement would
//! be a promise the software cannot keep.

use cyberbrain_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// How long an approved window stays open unless a shorter one was asked for.
pub const DEFAULT_WINDOW_HOURS: i64 = 72;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Runs the hub. State only.
    Admin,
    /// May request and, once approved, read activity.
    Auditor,
    /// Approves requests; may read every request and every disclosure.
    Countersigner,
}

impl Role {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "admin" => Ok(Role::Admin),
            "auditor" => Ok(Role::Auditor),
            "countersigner" => Ok(Role::Countersigner),
            other => Err(Error::Config(format!(
                "unknown role {other:?}; one of admin, auditor, countersigner"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Auditor => "auditor",
            Role::Countersigner => "countersigner",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Principal {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

impl Principal {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// A request to look at activity, and what became of it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccessRequest {
    pub id: String,
    /// Who asked.
    pub requester: String,
    pub requester_name: String,
    /// Which device's rows, or `None` for all of them.
    pub device: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Why. Free text, and the field a countersigner actually reads.
    pub reason: String,
    pub created_at: String,
    pub approved_by: Option<String>,
    pub approved_by_name: Option<String>,
    pub approved_at: Option<String>,
    /// When the window closes. Set at approval.
    pub expires_at: Option<String>,
    /// How many times rows were handed out under this request.
    pub disclosures: i64,
}

/// What a request can be, at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequestState {
    /// Waiting for a countersignature.
    Pending,
    /// Approved and inside its window.
    Open,
    /// Approved, but the window has closed. Extending means a new request.
    Closed,
}

impl AccessRequest {
    pub fn state(&self, now: jiff::Timestamp) -> RequestState {
        match (&self.approved_at, &self.expires_at) {
            (None, _) => RequestState::Pending,
            (Some(_), Some(exp)) => match exp.parse::<jiff::Timestamp>() {
                Ok(t) if now <= t => RequestState::Open,
                // An expiry that cannot be read closes the window rather than opening it
                // forever: fail towards the side that needs a new decision.
                _ => RequestState::Closed,
            },
            (Some(_), None) => RequestState::Closed,
        }
    }

    pub fn line(&self, now: jiff::Timestamp) -> String {
        let what = match &self.device {
            Some(d) => d.clone(),
            None => "all devices".to_string(),
        };
        let period = match (&self.from, &self.to) {
            (Some(f), Some(t)) => format!("{f} to {t}"),
            (Some(f), None) => format!("from {f}"),
            (None, Some(t)) => format!("up to {t}"),
            (None, None) => "the whole record".to_string(),
        };
        let state = match self.state(now) {
            RequestState::Pending => "awaiting countersignature".to_string(),
            RequestState::Open => format!(
                "open until {}",
                self.expires_at.as_deref().unwrap_or("unknown")
            ),
            RequestState::Closed => "closed".to_string(),
        };
        format!(
            "{}  {}\n    {} · {} · asked by {} on {}\n    reason: {}\n    {} disclosure(s)\n",
            self.id,
            state,
            what,
            period,
            self.requester_name,
            self.created_at,
            self.reason,
            self.disclosures
        )
    }
}

/// Why an attempt to see activity was turned down.
#[derive(Debug, Clone, PartialEq)]
pub enum Denied {
    /// No credential, an unknown one, or a revoked principal.
    NotAuthorised(String),
    /// The right person, the wrong role for this action.
    WrongRole { need: Role, has: Role },
    /// The request exists but nobody has countersigned it.
    NotApproved(String),
    /// The window closed. A new request is the way, not an extension.
    WindowClosed(String),
    /// Approving one's own request. The whole point of the rule.
    SamePerson,
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denied::NotAuthorised(m) => write!(f, "{m}"),
            Denied::WrongRole { need, has } => write!(
                f,
                "this needs the {} role; that credential is {}",
                need.as_str(),
                has.as_str()
            ),
            Denied::NotApproved(id) => write!(
                f,
                "request {id} has not been countersigned; activity cannot be read until \
                 somebody else approves it"
            ),
            Denied::WindowClosed(id) => write!(
                f,
                "the window for request {id} has closed. Make a new request rather than \
                 extending this one, so the reason is stated again"
            ),
            Denied::SamePerson => write!(
                f,
                concat!(
                    "a request cannot be countersigned by the person who made it. ",
                    "That is the rule, not an obstacle to work around"
                )
            ),
        }
    }
}
