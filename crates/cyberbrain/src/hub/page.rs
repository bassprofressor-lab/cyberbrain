//! The hub's own page: what state is this collection in, and is it licensed.
//!
//! # Why there is a page at all
//!
//! Everything here was already answerable — `hub licence show`, `hub fleet` — and that was
//! the problem. The person who installs a hub for a company of eleven people is not going to
//! open a prompt, and when they did open one they still could not tell from the dashboard
//! they were looking at whether this machine was a hub, a client, or licensed. A product
//! whose state can only be read by typing is a product with no state as far as its owner is
//! concerned.
//!
//! # Why it is server-rendered and has no JavaScript
//!
//! The store's web UI is a built React bundle behind a feature flag, and building it needs
//! node. None of that may be the price of finding out whether a licence was accepted. This
//! is one function that returns a string, it works with the page source visible, and it
//! cannot fail to load.
//!
//! # Why it is loopback only
//!
//! It shows who is on the network and it can install a licence, and the hub deliberately
//! binds an address the whole network can reach. Rather than invent a sign-in for this
//! slice, the rule is that you have to be at the machine. That is a rule with an obvious
//! shape, it cannot be misconfigured, and a networked view — with the admin role that
//! already exists behind it — can come later without taking anything back.

use super::report::{self, FleetRow};
use super::store::HubStore;
use super::{LicenceState, service};

/// Everything the page shows, gathered before any of it is written out.
pub struct View {
    pub version: String,
    pub record: std::path::PathBuf,
    pub licence: LicenceState,
    pub seats: Option<(usize, usize)>,
    pub fleet: Vec<FleetRow>,
    /// A licence file lying in the data directory, for the one-click install.
    pub found_file: Option<std::path::PathBuf>,
    /// What to put in an invitation as the address clients deliver to. A guess from the
    /// machine's own name, because the page is reached over loopback and "localhost" is the
    /// one address that is certainly wrong for everybody else.
    pub suggested_url: String,
    /// What just happened, if the page was reached by installing something.
    pub flash: Option<Result<String, String>>,
}

impl View {
    pub fn gather(
        hub: &HubStore,
        record: &std::path::Path,
        port: u16,
        now: jiff::Timestamp,
        flash: Option<Result<String, String>>,
    ) -> Self {
        let version = env!("CARGO_PKG_VERSION").to_string();
        let licence = LicenceState::read(hub, now);
        let seats = licence
            .seats()
            .and_then(|total| hub.active_device_count().ok().map(|used| (used, total)));
        let dir = record.parent().unwrap_or(std::path::Path::new("."));
        // A file that is already installed is not something to offer a button for; it is
        // the ordinary state of a hub that was licensed last year and never tidied up.
        let installed = hub.licence_text().ok().flatten();
        let found_file = service::find_licence_file(dir)
            .filter(|p| std::fs::read_to_string(p).ok().as_deref() != installed.as_deref());
        View {
            fleet: report::fleet(hub, now, &version).unwrap_or_default(),
            version,
            record: record.to_path_buf(),
            licence,
            seats,
            found_file,
            suggested_url: format!("http://{}:{port}", hostname()),
            flash,
        }
    }
}

/// The machine's own name, for the address clients should deliver to.
///
/// Its name rather than an IP: a hub that moves to another address keeps its name, and the
/// person filling this in can correct it in the field anyway.
fn hostname() -> String {
    #[cfg(windows)]
    let var = "COMPUTERNAME";
    #[cfg(not(windows))]
    let var = "HOSTNAME";
    std::env::var(var)
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .map(|h| h.trim().to_string())
                .ok()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| "this-machine".into())
        })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The screen shown before anything else, when nobody has claimed this hub yet.
///
/// Reachable only from the machine itself, which is what makes it safe to have no password
/// in front of it: whoever is at the console could read the record with any SQLite tool
/// anyway. Everything after this is reachable from a desk.
pub fn claim_page(problem: Option<&str>) -> String {
    let mut h = String::from(HEAD);
    h.push_str("<header><h1>Cyberbrain Hub</h1><p class=sub>first run</p></header>");
    if let Some(p) = problem {
        h.push_str(&format!("<p class=\"flash bad\">{}</p>", esc(p)));
    }
    h.push_str(&format!(
        "<section class=card><h2>Set the administrator password</h2>\
         <p>The account is <code>{}</code>. There is no default password to change later: a \
         default on something that listens to the network is the thing that gets found.</p>\
         <form method=post action=\"/claim\">\
         <label>Password <input type=password name=password minlength={} required autofocus></label>\
         <label>Again <input type=password name=again minlength={} required></label>\
         <button type=submit>Set it</button></form>\
         <p class=note>At least {} characters. Until this is set, the page is shown only on \
         this machine — the hub itself collects normally either way, because evidence must \
         not wait for an administrator.</p></section></main>",
        super::admin::USER,
        super::admin::MIN_PASSWORD,
        super::admin::MIN_PASSWORD,
        super::admin::MIN_PASSWORD
    ));
    h
}

/// The sign-in screen.
pub fn login_page(problem: Option<&str>) -> String {
    let mut h = String::from(HEAD);
    h.push_str("<header><h1>Cyberbrain Hub</h1></header>");
    if let Some(p) = problem {
        h.push_str(&format!("<p class=\"flash bad\">{}</p>", esc(p)));
    }
    h.push_str(&format!(
        "<section class=card><h2>Sign in</h2>\
         <form method=post action=\"/login\">\
         <label>User <input name=user value=\"{}\" readonly></label>\
         <label>Password <input type=password name=password required autofocus></label>\
         <button type=submit>Sign in</button></form>\
         <p class=note>Forgotten it? On the machine the hub runs on, \
         <code>cyberbrain hub admin reset</code> clears it and the next visit from that \
         machine sets a new one.</p></section></main>",
        super::admin::USER
    ));
    h
}

/// The whole page, as one string.
pub fn render(v: &View) -> String {
    let mut h = String::with_capacity(8192);
    h.push_str(HEAD);

    h.push_str(&format!(
        "<header><h1>Cyberbrain Hub</h1><p class=sub>version {} · record <code>{}</code> \
         <a href=\"/logout\">sign out</a></p></header>",
        esc(&v.version),
        esc(&v.record.display().to_string())
    ));

    if let Some(flash) = &v.flash {
        let (kind, text) = match flash {
            Ok(t) => ("ok", t),
            Err(t) => ("bad", t),
        };
        h.push_str(&format!("<p class=\"flash {kind}\">{}</p>", esc(text)));
    }

    h.push_str(&licence_card(v));
    h.push_str(&fleet_card(v));

    h.push_str(&format!(
        "<section class=card><h2>Administration</h2>\
         <details><summary>Change the password</summary>\
         <form method=post action=\"/password\">\
         <label>Current <input type=password name=current required></label>\
         <label>New <input type=password name=password minlength={} required></label>\
         <label>Again <input type=password name=again minlength={} required></label>\
         <button type=submit>Change it</button></form></details></section>",
        super::admin::MIN_PASSWORD,
        super::admin::MIN_PASSWORD
    ));

    h.push_str(&format!(
        "<footer><p>Signed in as <code>{}</code>. \
         The log is <code>{}</code>.</p></footer>",
        super::admin::USER,
        esc(&v
            .record
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("hub-service.log")
            .display()
            .to_string())
    ));
    h.push_str("</main>");
    h
}

fn licence_card(v: &View) -> String {
    let (state, detail) = match &v.licence {
        // The one line somebody installing this wants, in the place they are looking.
        LicenceState::Missing => (
            ("bad", "not licensed"),
            "This hub is running and accepts nothing. Install the licence you were sent."
                .to_string(),
        ),
        LicenceState::Invalid(why) => (("bad", "licence not usable"), why.clone()),
        LicenceState::Expired {
            customer,
            valid_until,
        } => (
            ("bad", "licence expired"),
            format!(
                "{customer}, ended {valid_until}. The record is intact and can still be \
                 exported; new rows are refused until it is renewed."
            ),
        ),
        LicenceState::Valid {
            customer,
            valid_until,
            warning,
            ..
        } => (
            ("ok", "collecting"),
            match warning {
                Some(w) => w.clone(),
                None => format!("{customer}, until {valid_until}."),
            },
        ),
    };

    let seats = match v.seats {
        Some((used, total)) => format!(
            "<p class=seats><strong>{used}</strong> of <strong>{total}</strong> seat(s) in use \
             <span class=note>a seat is a device, counted when it is registered</span></p>"
        ),
        None => String::new(),
    };

    // The form is offered whenever the hub is not collecting, and also when it is, because
    // renewing is the same act and nobody should have to find a prompt for it either.
    let found = match &v.found_file {
        Some(p) => format!(
            "<form method=post action=\"/licence\" class=oneclick>\
             <input type=hidden name=use_found value=1>\
             <p>A licence file is lying next to the record: <code>{}</code></p>\
             <button type=submit>Install this file</button></form>",
            esc(&p.display().to_string())
        ),
        None => String::new(),
    };

    format!(
        "<section class=card><h2>Licence <span class=\"pill {}\">{}</span></h2>\
         <p>{}</p>{seats}{found}\
         <details{}><summary>Paste a licence instead</summary>\
         <form method=post action=\"/licence\">\
         <textarea name=text rows=4 spellcheck=false placeholder=\"Open the licence file, \
         select everything, paste it here\"></textarea>\
         <button type=submit>Install</button></form>\
         <p class=note>Two lines: the licence itself and its signature. It is checked here, \
         against a key built into this program — nothing is sent anywhere.</p>\
         </details></section>",
        state.0,
        state.1,
        esc(&detail),
        // Open by default when there is nothing else to click.
        if v.found_file.is_none() && !matches!(v.licence, LicenceState::Valid { .. }) {
            " open"
        } else {
            ""
        }
    )
}

fn fleet_card(v: &View) -> String {
    let form = format!(
        "<details class=add><summary>Register a machine</summary>\
         <form method=post action=\"/devices\">\
         <label>Name <input name=name placeholder=\"laptop-anna\" required></label>\
         <label>Address the machine delivers to \
         <input name=hub_url value=\"{}\"></label>\
         <button type=submit>Register and write the invitation</button></form>\
         <p class=note>An invitation file is written next to the record. It carries the \
         token, so hand it over the way you would a password and delete it once the machine \
         is set up. On that machine: the tray menu, <em>Connect to the company hub…</em></p>\
         </details>",
        esc(&v.suggested_url)
    );

    if v.fleet.is_empty() {
        return format!(
            "<section class=card><h2>Devices</h2><p>None registered yet.</p>{form}</section>"
        );
    }

    let mut rows = String::new();
    for r in &v.fleet {
        let concerns = if r.device.is_active() {
            r.concerns
                .iter()
                .map(|c| c.line())
                .collect::<Vec<_>>()
                .join("; ")
        } else {
            "revoked".to_string()
        };
        // Trouble is what this table is for, so it is the column that carries the colour.
        let cls = if !r.device.is_active() {
            "off"
        } else if r.concerns.is_empty() {
            "ok"
        } else {
            "bad"
        };
        rows.push_str(&format!(
            "<tr class={cls}><td>{}</td><td class=num>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&r.device.name),
            r.device.rows,
            esc(r.device.last_seen.as_deref().unwrap_or("never")),
            esc(if concerns.is_empty() {
                "—"
            } else {
                &concerns
            })
        ));
    }
    let attention = v
        .fleet
        .iter()
        .filter(|r| r.device.is_active() && !r.concerns.is_empty())
        .count();
    format!(
        "<section class=card><h2>Devices <span class=\"pill {}\">{}</span></h2>\
         <table><thead><tr><th>Name<th>Rows<th>Last heard from<th>State</tr></thead>\
         <tbody>{rows}</tbody></table>\
         <p class=note>State, not activity: what a note says never leaves the machine that \
         holds it.</p>{form}</section>",
        if attention == 0 { "ok" } else { "bad" },
        if attention == 0 {
            "all reporting".to_string()
        } else {
            format!("{attention} need attention")
        }
    )
}

const HEAD: &str = r#"<title>Cyberbrain Hub</title>
<style>
:root{--bg:#f7f7f5;--card:#fff;--ink:#1a1a18;--dim:#6b6b64;--line:#e2e2dc;
      --ok:#1c6b3f;--okbg:#e3f2e8;--bad:#8a3312;--badbg:#fbe9e1;--off:#8a8a80}
@media (prefers-color-scheme:dark){:root{--bg:#141414;--card:#1c1c1b;--ink:#eceae4;
      --dim:#9a9a92;--line:#2e2e2b;--ok:#7fd4a2;--okbg:#17301f;--bad:#f0a184;--badbg:#3a1c11}}
*{box-sizing:border-box}
body{background:var(--bg);color:var(--ink);font:15px/1.55 system-ui,-apple-system,Segoe UI,sans-serif;
     margin:0;padding:2.2rem 1.2rem}
main,header,footer{max-width:52rem;margin:0 auto}
h1{font-size:1.5rem;margin:0}
h2{font-size:1.05rem;margin:0 0 .6rem;display:flex;align-items:center;gap:.6rem}
.sub{color:var(--dim);margin:.25rem 0 1.4rem}
code{font:13px ui-monospace,Consolas,monospace;background:var(--bg);padding:.1em .35em;
     border-radius:4px;border:1px solid var(--line)}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;
      padding:1.1rem 1.2rem;margin-bottom:1rem}
.pill{font-size:.72rem;font-weight:600;letter-spacing:.03em;text-transform:uppercase;
      padding:.18em .55em;border-radius:99px}
.pill.ok{color:var(--ok);background:var(--okbg)} .pill.bad{color:var(--bad);background:var(--badbg)}
.seats{margin:.7rem 0 0} .note{color:var(--dim);font-size:.85rem}
.seats .note{display:block;font-weight:400}
table{width:100%;border-collapse:collapse;margin-top:.4rem;font-size:.92rem}
th{text-align:left;font-weight:600;color:var(--dim);font-size:.78rem;text-transform:uppercase;
   letter-spacing:.04em;padding:.3rem .5rem .3rem 0;border-bottom:1px solid var(--line)}
td{padding:.42rem .5rem .42rem 0;border-bottom:1px solid var(--line);vertical-align:top}
.num{font-variant-numeric:tabular-nums;text-align:right;padding-right:1.2rem}
tr.bad td:last-child{color:var(--bad)} tr.off td{color:var(--off)}
textarea{width:100%;font:13px ui-monospace,Consolas,monospace;padding:.5rem;
         border:1px solid var(--line);border-radius:6px;background:var(--bg);color:var(--ink)}
button{margin-top:.6rem;font:inherit;font-weight:600;padding:.45rem 1rem;border-radius:6px;
       border:1px solid var(--line);background:var(--ink);color:var(--card);cursor:pointer}
.oneclick{margin:.9rem 0 .2rem;padding:.8rem;border:1px dashed var(--line);border-radius:8px}
.oneclick p{margin:0}
details{margin-top:.9rem} summary{cursor:pointer;color:var(--dim);font-size:.9rem}
label{display:block;margin:.7rem 0 0;font-size:.85rem;color:var(--dim)}
input{display:block;width:100%;margin-top:.2rem;font:inherit;padding:.4rem .5rem;
      border:1px solid var(--line);border-radius:6px;background:var(--bg);color:var(--ink)}
.invite{white-space:pre-wrap;font:13px ui-monospace,Consolas,monospace}
.flash{padding:.7rem 1rem;border-radius:8px;margin:0 0 1rem}
.flash.ok{color:var(--ok);background:var(--okbg)} .flash.bad{color:var(--bad);background:var(--badbg)}
footer{color:var(--dim);font-size:.85rem;margin-top:1.4rem}
.sub a{color:var(--dim);margin-left:.5rem}
</style>
<main>"#;
