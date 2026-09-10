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
//! # Who may look at it
//!
//! It shows who is on the network and it can install a licence, and the hub deliberately
//! binds an address the whole network can reach. Setting the first password is still only
//! possible at the machine; after that the page is reachable from a desk, and on a hub that
//! is not encrypted signing in is again only possible at the machine, because the password
//! would otherwise cross the network in the clear. The page says so where it applies, rather
//! than leaving the operator to work out why sign-in works in one place and not another.

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
    /// Who may share which bereich. On the administrator's page because a grant is state,
    /// not content: it names a device, a bereich and a reason, never a word of a note.
    pub grants: Vec<super::sync_access::BereichGrant>,
    /// A licence file lying in the data directory, for the one-click install.
    pub found_file: Option<std::path::PathBuf>,
    /// What to put in an invitation as the address clients deliver to. A guess from the
    /// machine's own name, because the page is reached over loopback and "localhost" is the
    /// one address that is certainly wrong for everybody else.
    pub suggested_url: String,
    /// What just happened, if the page was reached by installing something.
    pub flash: Option<Result<String, String>>,
    /// Whether the surface this was served over is encrypted. Shown, because the operator
    /// looking at the fleet is the one person who can do something about it.
    pub encrypted: bool,
}

impl View {
    pub fn gather(
        hub: &HubStore,
        record: &std::path::Path,
        port: u16,
        encrypted: bool,
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
        // Every device's grants, in the order the devices appear, so the table reads the way
        // the one above it does.
        let grants = report::fleet(hub, now, &version)
            .unwrap_or_default()
            .iter()
            .filter_map(|r| hub.grants_for_device(&r.device.id).ok())
            .flatten()
            .collect();
        View {
            grants,
            fleet: report::fleet(hub, now, &version).unwrap_or_default(),
            version,
            record: record.to_path_buf(),
            licence,
            seats,
            found_file,
            // The scheme this hub is actually being served over. An invitation that says
            // http to a hub that only answers https sends a client at a door that is shut.
            suggested_url: format!(
                "{}://{}:{port}",
                if encrypted { "https" } else { "http" },
                hostname()
            ),
            flash,
            encrypted,
        }
    }
}

/// The machine's own name, for the address clients should deliver to — and for the names in
/// the hub's own certificate, which have to be the ones this page suggests or the browser
/// warns about a name mismatch on top of everything else.
///
/// Its name rather than an IP: a hub that moves to another address keeps its name, and the
/// person filling this in can correct it in the field anyway.
pub(crate) fn hostname() -> String {
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

    // Above the fleet, not below it: on an unencrypted hub every token in that table was
    // handed over in the clear, so it is the first thing about them that is true.
    if !v.encrypted {
        h.push_str(
            "<p class=\"flash bad\">This hub is not encrypted. Device tokens and anything \
             typed here cross the network in the clear, and signing in works only at this \
             machine. Start it with <code>--tls-cert</code> and <code>--tls-key</code> to \
             change that.</p>",
        );
    }

    h.push_str(&licence_card(v));
    h.push_str(&fleet_card(v));
    h.push_str(&grants_card(v));

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

/// Who may share which bereich, and the form to add one.
///
/// A grant is state: a device, a bereich, a direction and a reason. No note text passes
/// through here, which is why this belongs on the administrator's page while conflicts do
/// not. The reason field is required by the form as well as by the command, because the
/// place it is most likely to be skipped is the one where typing feels optional.
fn grants_card(v: &View) -> String {
    let options = v
        .fleet
        .iter()
        .filter(|r| r.device.is_active())
        .map(|r| {
            format!(
                "<option value=\"{}\">{}</option>",
                esc(&r.device.id),
                esc(&r.device.name)
            )
        })
        .collect::<String>();

    let form = format!(
        "<details><summary>Grant a bereich</summary>\
         <form method=post action=\"/grants\">\
         <label>Device <select name=device required>{options}</select></label>\
         <label>Bereich <input name=bereich required placeholder=\"disposition\"></label>\
         <label>Direction <select name=direction>\
           <option value=both>send and receive</option>\
           <option value=send>send only</option>\
           <option value=receive>receive only</option></select></label>\
         <label>Reason <input name=reason required \
           placeholder=\"why this department may share\"></label>\
         <button type=submit>Grant it</button></form>\
         <p class=note>Rings 0 and 1 never leave a machine, whatever is granted here.</p>\
         </details>"
    );

    if v.grants.is_empty() {
        return format!(
            "<section class=card><h2>Sharing</h2>\
             <p>No grants. Every device delivers audit rows and nothing else.</p>{form}\
             </section>"
        );
    }

    let mut rows = String::new();
    for g in &v.grants {
        let live = g.is_active();
        // Three states, not two. A grant waiting for a countersignature looked exactly like
        // one that was working, which sent the operator looking for the fault in the client
        // — the one place it was not.
        let state = if !live {
            "withdrawn"
        } else if g.is_effective() {
            "in force"
        } else {
            "waiting for a countersignature"
        };
        rows.push_str(&format!(
            "<tr class={cls}><td>{device}<td>{bereich}<td>{dir}<td>{reason}<td>{state}\
             <td>{action}</tr>",
            cls = if g.is_effective() {
                "ok"
            } else if live {
                "pending"
            } else {
                "off"
            },
            device = esc(&g.device),
            bereich = esc(&g.bereich),
            dir = esc(g.direction.as_str()),
            reason = esc(&g.reason),
            state = match &g.approved_by {
                Some(by) => format!("in force, countersigned by {}", esc(by)),
                None => state.to_string(),
            },
            action = if live {
                format!(
                    "<form method=post action=\"/grants/{}/revoke\">\
                     <button type=submit>Withdraw</button></form>",
                    esc(&g.id)
                )
            } else {
                "withdrawn".to_string()
            },
        ));
    }
    format!(
        "<section class=card><h2>Sharing</h2>\
         <table><thead><tr><th>Device<th>Bereich<th>Direction<th>Reason<th>State<th></tr></thead>\
         <tbody>{rows}</tbody></table>{form}</section>"
    )
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

const HEAD: &str = r#"<!doctype html>
<meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1">
<title>Cyberbrain Hub</title>
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
tr.pending td{color:var(--dim)} tr.pending td:nth-last-child(2){color:var(--bad)}
/* The conflicts page: two versions beside each other on a desk, stacked on a phone. Its
   markup used these three and the stylesheet had none of them, so the comparison the page
   exists for was drawn one under the other and the note texts ran off the card. */
.side-by-side{display:grid;grid-template-columns:1fr 1fr;gap:1rem}
@media (max-width:720px){.side-by-side{grid-template-columns:1fr}}
.muted{color:var(--dim);font-size:.85rem}
pre{white-space:pre-wrap;overflow-wrap:anywhere;background:var(--bg);border:1px solid var(--line);
    border-radius:6px;padding:.6rem;font:12.5px ui-monospace,Consolas,monospace;margin:.3rem 0}
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

/// The page an editor sees: the conflicts in their bereiche and nothing else.
///
/// Two versions side by side and two buttons. The decision is which text stands, so the
/// page shows the texts and not their metadata — an editor who has to reason about
/// timestamps to answer "which of these is right" has been handed the wrong question.
pub fn conflicts_page(name: &str, conflicts: &[(super::store::NoteConflict, String)]) -> String {
    let mut body = String::new();
    if conflicts.is_empty() {
        body.push_str(
            "<section class=card><h2>Nothing to decide</h2>\
             <p>No note in your bereiche was changed in two places at once.</p></section>",
        );
    }
    for (c, held_text) in conflicts {
        body.push_str(&format!(
            "<section class=card>\
               <h2>{name_}</h2>\
               <p class=muted>in {bereich} · two machines changed this without seeing each \
                  other. Nothing was overwritten.</p>\
               <div class=side-by-side>\
                 <div><h3>What stands now</h3><p class=muted>{held_from}, {held_when}</p>\
                   <pre>{held_body}</pre>\
                   <form method=post action=\"/conflicts/{id}\">\
                     <input type=hidden name=take value=held>\
                     <button type=submit>Keep this one</button></form></div>\
                 <div><h3>What was offered</h3><p class=muted>{off_from}, {off_when}</p>\
                   <pre>{off_body}</pre>\
                   <form method=post action=\"/conflicts/{id}\">\
                     <input type=hidden name=take value=offered>\
                     <button type=submit>Use this one instead</button></form></div>\
               </div>\
             </section>",
            name_ = esc(&c.name),
            bereich = esc(&c.bereich),
            id = esc(&c.id),
            held_from = esc(&c.held_from_device),
            held_when = esc(&c.held_updated),
            held_body = esc(held_text),
            off_from = esc(&c.offered_from_device),
            off_when = esc(&c.offered_updated),
            off_body = esc(&c.offered_body),
        ));
    }
    let mut h = String::from(HEAD);
    h.push_str(&format!(
        "<header><h1>Conflicts</h1></header>\
         <p class=note>Signed in as {}. You see the bereiche you are responsible for.\
         <form method=post action=\"/logout\" style=\"display:inline\">\
         <button type=submit>Sign out</button></form></p>{body}</main>",
        esc(name)
    ));
    h
}

/// What a countersigner or an auditor sees. Everything on it is about decisions, never rows.
///
/// This page exists because the two roles the hub's whole two-person rule rests on had no
/// way in at all: they could sign in, land on the operator's page, be told they were nobody,
/// and their actual work — approving a request, countersigning a bereich — was reachable
/// only from a shell on the hub's own machine. Which is the machine whose operator they are
/// there to check.
pub struct SigningView<'a> {
    pub name: &'a str,
    pub role: super::access::Role,
    /// Requests to see activity. A countersigner gets all of them; an auditor their own.
    pub requests: Vec<(super::access::AccessRequest, super::access::RequestState)>,
    /// Bereich grants written and waiting for a second signature. Countersigners only.
    pub grants: Vec<super::sync_access::BereichGrant>,
    /// The hub's own log. Countersigners only — it is the record of who looked at what.
    pub log: Vec<super::store::HubEvent>,
    pub devices: Vec<(String, String)>,
    pub flash: Option<Result<String, String>>,
}

pub fn signing_page(v: &SigningView<'_>) -> String {
    use super::access::{RequestState, Role};
    let signing = v.role == Role::Countersigner;
    let mut h = String::from(HEAD);
    h.push_str(&format!(
        "<header><h1>{title}</h1></header>\
         <p class=note>Signed in as {name}.\
         <form method=post action=\"/logout\" style=\"display:inline\">\
         <button type=submit>Sign out</button></form></p>",
        title = if signing {
            "To countersign"
        } else {
            "Access requests"
        },
        name = esc(v.name),
    ));
    match &v.flash {
        Some(Ok(m)) => h.push_str(&format!("<p class=\"flash ok\">{}</p>", esc(m))),
        Some(Err(m)) => h.push_str(&format!("<p class=\"flash bad\">{}</p>", esc(m))),
        None => {}
    }

    // --- bereich grants ---------------------------------------------------------------
    if signing {
        h.push_str("<section class=card><h2>Bereiche waiting for a second signature</h2>");
        if v.grants.is_empty() {
            h.push_str("<p class=muted>Nothing waiting.</p>");
        } else {
            h.push_str(
                "<p class=muted>Somebody has written these down. None of them moves a note \
                 until you sign it, and signing puts both names in the log.</p>\
                 <table><thead><tr><th>Device<th>Bereich<th>Direction<th>Reason<th>Written by\
                 <th></tr></thead><tbody>",
            );
            for g in &v.grants {
                h.push_str(&format!(
                    "<tr><td>{d}<td>{b}<td>{dir}<td>{why}<td>{by}\
                     <td><form method=post action=\"/grants/{id}/approve\">\
                     <button type=submit>Countersign</button></form></tr>",
                    d = esc(&g.device),
                    b = esc(&g.bereich),
                    dir = esc(g.direction.as_str()),
                    why = esc(&g.reason),
                    by = esc(&g.granted_by),
                    id = esc(&g.id),
                ));
            }
            h.push_str("</tbody></table>");
        }
        h.push_str("</section>");
    }

    // --- access requests --------------------------------------------------------------
    h.push_str(&format!(
        "<section class=card><h2>{}</h2>",
        if signing {
            "Requests to see activity"
        } else {
            "Your requests"
        }
    ));
    if v.requests.is_empty() {
        h.push_str("<p class=muted>None.</p>");
    } else {
        h.push_str(
            "<table><thead><tr><th>Asked by<th>What<th>Reason<th>State<th></tr></thead><tbody>",
        );
        for (r, state) in &v.requests {
            let what = match &r.device {
                Some(d) => esc(d),
                None => "every device".to_string(),
            };
            let state_text = match state {
                RequestState::Pending => "waiting".to_string(),
                RequestState::Open => {
                    format!("open until {}", esc(r.expires_at.as_deref().unwrap_or("?")))
                }
                RequestState::Closed => "window closed".to_string(),
            };
            let action = match (signing, state) {
                (true, RequestState::Pending) if r.requester != v.name => format!(
                    "<form method=post action=\"/requests/{}/approve\">\
                     <button type=submit>Approve for 7 days</button></form>",
                    esc(&r.id)
                ),
                // Named rather than hidden: a countersigner who asked for something has to
                // see why the button is not there, or the page looks broken to them.
                (true, RequestState::Pending) => "you asked for this one".to_string(),
                _ => String::new(),
            };
            h.push_str(&format!(
                "<tr><td>{who}<td>{what}<td>{why}<td>{state_text}<td>{action}</tr>",
                who = esc(&r.requester_name),
                why = esc(&r.reason),
            ));
        }
        h.push_str("</tbody></table>");
    }
    h.push_str("</section>");

    // --- ask for one ------------------------------------------------------------------
    if v.role == Role::Auditor {
        let mut options = String::from("<option value=\"\">every device</option>");
        for (id, name) in &v.devices {
            options.push_str(&format!(
                "<option value=\"{}\">{}</option>",
                esc(id),
                esc(name)
            ));
        }
        h.push_str(&format!(
            "<section class=card><h2>Ask to see activity</h2>\
             <form method=post action=\"/requests\">\
             <label>Which machine <select name=device>{options}</select></label>\
             <label>Why <textarea name=reason rows=3 required \
               placeholder=\"The countersigner reads this and nothing else decides for \
               them.\"></textarea></label>\
             <button type=submit>Ask</button></form>\
             <p class=note>Approving is somebody else's to do, and the rows come out with \
             <code>cyberbrain hub disclose</code> once the window is open.</p></section>"
        ));
    }

    // --- the log ----------------------------------------------------------------------
    if signing {
        h.push_str(
            "<section class=card><h2>The hub's own log</h2>\
             <p class=muted>Every role granted, every request, every approval, every \
             disclosure, in a chain that cannot be edited afterwards without it showing.</p>\
             <table><thead><tr><th>When<th>Who<th>What</tr></thead><tbody>",
        );
        for e in v.log.iter().rev() {
            h.push_str(&format!(
                "<tr><td>{}<td>{}<td>{}</tr>",
                esc(&e.ts),
                esc(&e.actor),
                esc(&e.action)
            ));
        }
        h.push_str("</tbody></table></section>");
    }
    h.push_str("</main>");
    h
}
