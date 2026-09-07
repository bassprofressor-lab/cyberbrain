//! Who may look at the hub's page.
//!
//! # Why there is a password rather than only the loopback rule
//!
//! The first version showed the page only to the machine the hub runs on. That is a rule
//! with an obvious shape and it cannot be misconfigured, but it puts the one screen that
//! says whether a company's evidence is being collected on a server in a cupboard. Nobody
//! walks to a cupboard, so nobody looks, so the screen may as well not exist.
//!
//! # Why there is no default password
//!
//! A fixed default on a service that listens to the whole network is the thing that gets
//! found, and "you must change it afterwards" is a sentence people read after the change was
//! needed. So: the account is `admin`, and the password is set on the first visit **from the
//! machine itself**. Same number of clicks as typing a default, nothing to look up, and
//! nothing written anywhere that could leak it. Until it is set the hub collects normally —
//! evidence must not wait for an administrator.
//!
//! # What this is not
//!
//! One account for the machine's administration, which is what the fleet view is. It is not
//! the roles model: an auditor reading activity still needs a countersignature, and that
//! lives in `access.rs` where it belongs. Nothing here can read a row.

use super::store::HubStore;
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use std::collections::HashMap;
use std::sync::Mutex;

/// The only account. Named rather than chosen, so there is one less thing to forget.
pub const USER: &str = "admin";

/// Short enough that people will accept it, long enough to be worth the argon2 in front.
pub const MIN_PASSWORD: usize = 10;

/// How long a session lasts without being used again.
const SESSION_HOURS: i64 = 12;

/// What a failed attempt costs. Not a lockout — locking out the administrator is a way to
/// take a hub's own operator off it — but enough that guessing over a network is hopeless.
pub const FAILURE_DELAY_MS: u64 = 400;

pub fn is_claimed(hub: &HubStore) -> bool {
    hub.setting("admin_password")
        .ok()
        .flatten()
        .is_some_and(|v| !v.is_empty())
}

/// Set or replace the password. The hash carries its own salt and parameters.
pub fn set_password(hub: &HubStore, password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(format!(
            "The password needs at least {MIN_PASSWORD} characters. Length is the only part \
             of this a person controls; everything else is done for them."
        ));
    }
    // The salt comes from the operating system, inside `hash_password`, and travels in the
    // stored string along with the parameters — so a later change of cost does not invalidate
    // what is already stored.
    let hash = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| format!("the password could not be hashed: {e}"))?
        .to_string();
    hub.set_setting("admin_password", &hash)
        .map_err(|e| e.to_string())
}

pub fn verify(hub: &HubStore, password: &str) -> bool {
    let Some(stored) = hub.setting("admin_password").ok().flatten() else {
        return false;
    };
    let Ok(parsed) = PasswordHash::new(&stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Sessions live in memory, so a restart signs everybody out.
///
/// Deliberate: a hub restarts when it is upgraded or the machine reboots, both moments when
/// asking again costs one login and removes every stale cookie in the building.
#[derive(Default)]
pub struct Sessions {
    open: Mutex<HashMap<String, jiff::Timestamp>>,
}

impl Sessions {
    pub fn open(&self, now: jiff::Timestamp) -> String {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("the operating system has randomness");
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        if let Ok(mut open) = self.open.lock() {
            open.retain(|_, until| *until > now);
            open.insert(token.clone(), now + jiff::Span::new().hours(SESSION_HOURS));
        }
        token
    }

    /// Is this cookie still good — and if so, push its expiry out again.
    pub fn holds(&self, token: &str, now: jiff::Timestamp) -> bool {
        let Ok(mut open) = self.open.lock() else {
            return false;
        };
        match open.get(token) {
            Some(until) if *until > now => {
                open.insert(
                    token.to_string(),
                    now + jiff::Span::new().hours(SESSION_HOURS),
                );
                true
            }
            // Expired rather than unknown, but the answer to the person is the same, and
            // saying which would tell an unknown caller that a token nearly worked.
            _ => false,
        }
    }

    pub fn close(&self, token: &str) {
        if let Ok(mut open) = self.open.lock() {
            open.remove(token);
        }
    }
}

/// The cookie the browser carries. `HttpOnly` because no script on the page reads it, and
/// there is no script on the page.
pub const COOKIE: &str = "cyberbrain_hub";

pub fn set_cookie(token: &str) -> String {
    format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict")
}

pub fn clear_cookie() -> String {
    format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

/// Pull our cookie out of a Cookie header, ignoring whatever else is in it.
pub fn cookie_from(header: Option<&str>) -> Option<String> {
    header?.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == COOKIE).then(|| v.to_string())
    })
}
