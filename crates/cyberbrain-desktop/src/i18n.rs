//! The launcher's own words, in German and English.
//!
//! The web page has spoken German to a German system since the first release; the tray menu,
//! the dialogs and the installer did not, so somebody who set the product up in German got
//! two products. The language is the Windows display language, the same signal the page
//! reads from the browser: German for any German locale, English for everything else.
//!
//! What the `cyberbrain` command-line tool itself prints (a refused invitation, the report of
//! the client set-up) stays in its own words and is quoted as it is. A paraphrase here would
//! be one more thing to keep in step.
//!
//! Kept free of Win32 so a Linux machine can test that no key is missing in either language
//! and that both languages carry the same placeholders.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

/// The primary language of a Windows language id (`GetUserDefaultUILanguage`): the low ten
/// bits, where 0x07 is German whatever the region. Anything else is English.
pub fn lang_from_langid(id: u16) -> Lang {
    if id & 0x3ff == 0x07 {
        Lang::De
    } else {
        Lang::En
    }
}

static LANG: OnceLock<Lang> = OnceLock::new();

/// Fixed once, at startup. A language that could change under a dialog that is open would
/// be a dialog whose two halves disagree.
pub fn set_language(lang: Lang) {
    let _ = LANG.set(lang);
}

pub fn language() -> Lang {
    LANG.get().copied().unwrap_or(Lang::En)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    ServerNotFound,
    TrayMenuFailed,
    TrayIconFailed,
    ProjectsLost,
    AlreadyRunning,
    HubOffer,
    ServeStopped,
    TrayExplained,
    MenuRebuildFailed,
    MenuOpen,
    MenuFolder,
    MenuClients,
    MenuEnrol,
    MenuClose,
    MenuAdd,
    MenuPanes,
    MenuOpenIn,
    MenuWindow,
    MenuBrowser,
    MenuAutostart,
    MenuQuit,
    TooltipNone,
    TooltipOne,
    TooltipMany,
    PanesTitle,
    PanesFailed,
    PageInBrowser,
    ClientsDone,
    ClientsFailed,
    InvitationTitle,
    InvitationFilter,
    EnrolDone,
    EnrolFailed,
    HubConnectFailed,
    ChooseProject,
    AlreadyOpen,
    NoStoreAsk,
    StoreFailed,
    StartFailed,
    CannotOpen,
    PrintedHeader,
    Starting,
    AutostartFailed,
    WebviewMissing,
}

/// The template for a key. `{0}`, `{1}` stand for the arguments in order.
pub fn template(lang: Lang, key: Key) -> &'static str {
    use Key::*;
    match (lang, key) {
        (Lang::En, ServerNotFound) => {
            "cyberbrain.exe was not found next to this program or on the PATH.\n\n\
             The installer puts both in the same folder; if you moved one of them, put it \
             back or reinstall."
        }
        (Lang::De, ServerNotFound) => {
            "cyberbrain.exe wurde weder neben diesem Programm noch im PATH gefunden.\n\n\
             Der Installer legt beide in denselben Ordner; wenn Sie eines verschoben haben, \
             legen Sie es zurück oder installieren Sie neu."
        }
        (Lang::En, TrayMenuFailed) => "the tray menu could not be built",
        (Lang::De, TrayMenuFailed) => "Das Menü im Infobereich konnte nicht aufgebaut werden.",
        (Lang::En, TrayIconFailed) => "the tray icon could not be created: {0}",
        (Lang::De, TrayIconFailed) => "Das Symbol im Infobereich konnte nicht angelegt werden: {0}",
        (Lang::En, ProjectsLost) => {
            "These projects could not be opened and have been forgotten:\n\n{0}\n\nUse \
             Open another project… to point at them again once they are back."
        }
        (Lang::De, ProjectsLost) => {
            "Diese Projekte ließen sich nicht öffnen und wurden vergessen:\n\n{0}\n\nWählen \
             Sie „Weiteres Projekt öffnen…“, um sie wieder anzugeben, sobald sie erreichbar \
             sind."
        }
        (Lang::En, AlreadyRunning) => {
            "Cyberbrain is already running.\n\nIf no window opened, it is still starting up — \
             give it a moment, then use the Cyberbrain icon in the notification area."
        }
        (Lang::De, AlreadyRunning) => {
            "Cyberbrain läuft bereits.\n\nWenn sich kein Fenster geöffnet hat, startet es \
             noch. Warten Sie einen Moment und nutzen Sie dann das Cyberbrain-Symbol im \
             Infobereich."
        }
        (Lang::En, HubOffer) => {
            "Your company has set this computer up to connect projects to its hub.\n\n\
             Connect \"{0}\"? It will deliver its audit trail: what happened, not what a \
             note says. Sharing notes is a separate setting, and it stays off.\n\n\
             No is remembered for this project. You can still connect it later from its menu."
        }
        (Lang::De, HubOffer) => {
            "Ihr Unternehmen hat diesen Rechner so eingerichtet, dass Projekte mit seinem \
             Hub verbunden werden können.\n\n\
             „{0}“ verbinden? Es liefert dann sein Prüfprotokoll: was passiert ist, nicht, \
             was in einer Notiz steht. Das Teilen von Notizen ist eine eigene Einstellung \
             und bleibt aus.\n\n\
             Ein Nein gilt für dieses Projekt dauerhaft. Verbinden können Sie es später \
             weiterhin über sein Menü."
        }
        (Lang::En, ServeStopped) => {
            "cyberbrain serve stopped on its own ({0}) for\n{1}\n\nThat project has been \
             closed. The others are still open."
        }
        (Lang::De, ServeStopped) => {
            "cyberbrain serve wurde unerwartet beendet ({0}) für\n{1}\n\nDieses Projekt \
             wurde geschlossen. Die anderen sind weiter geöffnet."
        }
        (Lang::En, TrayExplained) => {
            "Cyberbrain is still running.\n\n\
             Closing a window closes the page, not the program: the servers stay \
             up, so your memory is there the moment you want it back. Its icon \
             lives in the notification area, next to the clock — behind the \
             arrow, if Windows has tidied it away. Quit in that icon's menu is \
             what closes Cyberbrain.\n\nSaid once."
        }
        (Lang::De, TrayExplained) => {
            "Cyberbrain läuft weiter.\n\n\
             Das Schließen eines Fensters schließt die Seite, nicht das Programm: Die \
             Server bleiben an, damit Ihr Gedächtnis sofort wieder da ist. Das Symbol \
             sitzt im Infobereich neben der Uhr, bei Bedarf hinter dem Pfeil, falls \
             Windows es dort versteckt hat. „Beenden“ im Menü dieses Symbols schließt \
             Cyberbrain.\n\nDieser Hinweis kommt nur einmal."
        }
        (Lang::En, MenuRebuildFailed) => {
            "The tray menu could not be rebuilt, so the project entries in it \
             have stopped working. Quit and start Cyberbrain again."
        }
        (Lang::De, MenuRebuildFailed) => {
            "Das Menü im Infobereich ließ sich nicht neu aufbauen, deshalb funktionieren \
             seine Projekteinträge nicht mehr. Beenden Sie Cyberbrain und starten Sie es \
             neu."
        }
        (Lang::En, MenuOpen) => "Open",
        (Lang::De, MenuOpen) => "Öffnen",
        (Lang::En, MenuFolder) => "Open project folder",
        (Lang::De, MenuFolder) => "Projektordner öffnen",
        (Lang::En, MenuClients) => "Set up Claude on this computer…",
        (Lang::De, MenuClients) => "Claude auf diesem Rechner einrichten…",
        (Lang::En, MenuEnrol) => "Connect to the company hub…",
        (Lang::De, MenuEnrol) => "Mit dem Firmen-Hub verbinden…",
        (Lang::En, MenuClose) => "Close this project",
        (Lang::De, MenuClose) => "Dieses Projekt schließen",
        (Lang::En, MenuAdd) => "Open another project…",
        (Lang::De, MenuAdd) => "Weiteres Projekt öffnen…",
        (Lang::En, MenuPanes) => "All command lines side by side",
        (Lang::De, MenuPanes) => "Alle Kommandozeilen nebeneinander",
        (Lang::En, MenuOpenIn) => "Open in",
        (Lang::De, MenuOpenIn) => "Öffnen in",
        (Lang::En, MenuWindow) => "A window of its own",
        (Lang::De, MenuWindow) => "Eigenem Fenster",
        (Lang::En, MenuBrowser) => "The web browser",
        (Lang::De, MenuBrowser) => "Dem Webbrowser",
        (Lang::En, MenuAutostart) => "Start with Windows",
        (Lang::De, MenuAutostart) => "Mit Windows starten",
        (Lang::En, MenuQuit) => "Quit",
        (Lang::De, MenuQuit) => "Beenden",
        (Lang::En, TooltipNone) => "Cyberbrain — nothing open",
        (Lang::De, TooltipNone) => "Cyberbrain — nichts geöffnet",
        (Lang::En, TooltipOne) => "Cyberbrain — {0}",
        (Lang::De, TooltipOne) => "Cyberbrain — {0}",
        (Lang::En, TooltipMany) => "Cyberbrain — {0} projects open",
        (Lang::De, TooltipMany) => "Cyberbrain — {0} Projekte geöffnet",
        (Lang::En, PanesTitle) => "Cyberbrain — command lines",
        (Lang::De, PanesTitle) => "Cyberbrain — Kommandozeilen",
        (Lang::En, PanesFailed) => "{0}\n\nThe command line is also in each project's own page.",
        (Lang::De, PanesFailed) => {
            "{0}\n\nDie Kommandozeile gibt es auch in der Seite jedes Projekts."
        }
        (Lang::En, PageInBrowser) => {
            "{0}\n\nThis page has been opened in the browser instead, and so will \
             the others until Cyberbrain is started again. Open in › The web browser \
             makes that the setting."
        }
        (Lang::De, PageInBrowser) => {
            "{0}\n\nDie Seite wurde stattdessen im Browser geöffnet, und die anderen \
             folgen, bis Cyberbrain neu gestartet wird. Mit „Öffnen in › Dem Webbrowser“ \
             wird das zur Einstellung."
        }
        (Lang::En, ClientsDone) => "{0}\n\nRestart the client for it to read this.",
        (Lang::De, ClientsDone) => "{0}\n\nStarten Sie den Client neu, damit er das übernimmt.",
        (Lang::En, ClientsFailed) => "Nothing was set up.\n\n{0}",
        (Lang::De, ClientsFailed) => "Es wurde nichts eingerichtet.\n\n{0}",
        (Lang::En, InvitationTitle) => "Open the invitation file you were sent",
        (Lang::De, InvitationTitle) => "Die zugesandte Einladungsdatei öffnen",
        (Lang::En, InvitationFilter) => "Invitation",
        (Lang::De, InvitationFilter) => "Einladung",
        (Lang::En, EnrolDone) => {
            "{0}\n\nThis machine will now deliver its audit trail to that hub: what \
             happened, not what a note says. Sharing notes is a separate setting, \
             and it is off unless somebody turns it on."
        }
        (Lang::De, EnrolDone) => {
            "{0}\n\nDieser Rechner liefert sein Prüfprotokoll jetzt an diesen Hub: was \
             passiert ist, nicht, was in einer Notiz steht. Das Teilen von Notizen ist \
             eine eigene Einstellung und bleibt aus, bis es jemand einschaltet."
        }
        (Lang::En, EnrolFailed) => "The invitation was not accepted.\n\n{0}",
        (Lang::De, EnrolFailed) => "Die Einladung wurde nicht angenommen.\n\n{0}",
        (Lang::En, HubConnectFailed) => "The project was not connected.\n\n{0}",
        (Lang::De, HubConnectFailed) => "Das Projekt wurde nicht verbunden.\n\n{0}",
        (Lang::En, ChooseProject) => "Choose the project whose memory to open",
        (Lang::De, ChooseProject) => "Projekt wählen, dessen Gedächtnis geöffnet werden soll",
        (Lang::En, AlreadyOpen) => "{0}\n\nis already open. It is in the menu.",
        (Lang::De, AlreadyOpen) => "{0}\n\nist bereits geöffnet. Es steht im Menü.",
        (Lang::En, NoStoreAsk) => "There is no Cyberbrain store in\n{0}\n\nCreate one there now?",
        (Lang::De, NoStoreAsk) => {
            "In\n{0}\n\ngibt es noch keinen Cyberbrain-Speicher. Jetzt dort einen anlegen?"
        }
        (Lang::En, StoreFailed) => "The store could not be created:\n\n{0}",
        (Lang::De, StoreFailed) => "Der Speicher konnte nicht angelegt werden:\n\n{0}",
        (Lang::En, StartFailed) => "Cyberbrain could not start:\n\n{0}",
        (Lang::De, StartFailed) => "Cyberbrain konnte nicht starten:\n\n{0}",
        (Lang::En, CannotOpen) => {
            "Cyberbrain cannot open:\n\n{0}.\n\nInstall the release build from \
             the Cyberbrain website, which has the page built in. The command-line \
             tool beside it works either way."
        }
        (Lang::De, CannotOpen) => {
            "Cyberbrain kann sich nicht öffnen:\n\n{0}.\n\nInstallieren Sie die \
             Release-Fassung von der Cyberbrain-Website, die die Seite eingebaut hat. \
             Das Kommandozeilenwerkzeug daneben funktioniert in beiden Fällen."
        }
        (Lang::En, PrintedHeader) => "\n\nWhat it printed:\n{0}",
        (Lang::De, PrintedHeader) => "\n\nAusgabe des Programms:\n{0}",
        (Lang::En, Starting) => "Cyberbrain — starting {0}…",
        (Lang::De, Starting) => "Cyberbrain — {0} wird gestartet…",
        (Lang::En, AutostartFailed) => "Start with Windows could not be changed:\n\n{0}",
        (Lang::De, AutostartFailed) => "„Mit Windows starten“ ließ sich nicht ändern:\n\n{0}",
        (Lang::En, WebviewMissing) => {
            "The WebView2 runtime could not be started. It is part of Windows 11 and of \
             current Windows 10, and can be installed from Microsoft's Evergreen download \
             page."
        }
        (Lang::De, WebviewMissing) => {
            "Die WebView2-Laufzeit konnte nicht gestartet werden. Sie gehört zu Windows 11 \
             und zum aktuellen Windows 10 und lässt sich von Microsofts Evergreen-Downloadseite \
             installieren."
        }
    }
}

/// The text for a key in the language of this session, with `{0}`, `{1}` filled in.
pub fn tr(key: Key, args: &[&str]) -> String {
    fill(template(language(), key), args)
}

pub fn fill(template: &str, args: &[&str]) -> String {
    let mut out = template.to_string();
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), a);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Key] = &[
        Key::ServerNotFound,
        Key::TrayMenuFailed,
        Key::TrayIconFailed,
        Key::ProjectsLost,
        Key::AlreadyRunning,
        Key::HubOffer,
        Key::ServeStopped,
        Key::TrayExplained,
        Key::MenuRebuildFailed,
        Key::MenuOpen,
        Key::MenuFolder,
        Key::MenuClients,
        Key::MenuEnrol,
        Key::MenuClose,
        Key::MenuAdd,
        Key::MenuPanes,
        Key::MenuOpenIn,
        Key::MenuWindow,
        Key::MenuBrowser,
        Key::MenuAutostart,
        Key::MenuQuit,
        Key::TooltipNone,
        Key::TooltipOne,
        Key::TooltipMany,
        Key::PanesTitle,
        Key::PanesFailed,
        Key::PageInBrowser,
        Key::ClientsDone,
        Key::ClientsFailed,
        Key::InvitationTitle,
        Key::InvitationFilter,
        Key::EnrolDone,
        Key::EnrolFailed,
        Key::HubConnectFailed,
        Key::ChooseProject,
        Key::AlreadyOpen,
        Key::NoStoreAsk,
        Key::StoreFailed,
        Key::StartFailed,
        Key::CannotOpen,
        Key::PrintedHeader,
        Key::Starting,
        Key::AutostartFailed,
        Key::WebviewMissing,
    ];

    fn placeholders(t: &str) -> Vec<usize> {
        (0..6).filter(|i| t.contains(&format!("{{{i}}}"))).collect()
    }

    #[test]
    fn every_key_is_listed_and_spoken_in_both_languages() {
        // `ALL` is what the other tests walk, so a key left out of it would escape them:
        // the match above is exhaustive, which makes a missing translation a compile error,
        // and this makes a missing entry in the list a test failure.
        let keys =
            std::collections::HashSet::<String>::from_iter(ALL.iter().map(|k| format!("{k:?}")));
        assert_eq!(keys.len(), ALL.len(), "a key is listed twice");
        let src = include_str!("i18n.rs");
        let declared = src
            .split("pub enum Key {")
            .nth(1)
            .and_then(|r| r.split('}').next())
            .expect("the enum is in this file")
            .lines()
            .filter(|l| l.trim().ends_with(','))
            .count();
        assert_eq!(declared, ALL.len(), "ALL and the enum differ");
        for k in ALL {
            assert!(!template(Lang::De, *k).trim().is_empty(), "{k:?} de");
            assert!(!template(Lang::En, *k).trim().is_empty(), "{k:?} en");
        }
    }

    #[test]
    fn both_languages_take_the_same_arguments() {
        for k in ALL {
            assert_eq!(
                placeholders(template(Lang::De, *k)),
                placeholders(template(Lang::En, *k)),
                "{k:?}"
            );
        }
    }

    #[test]
    fn german_locales_are_german_and_the_rest_english() {
        assert_eq!(lang_from_langid(0x0407), Lang::De); // Germany
        assert_eq!(lang_from_langid(0x0c07), Lang::De); // Austria
        assert_eq!(lang_from_langid(0x0807), Lang::De); // Switzerland
        assert_eq!(lang_from_langid(0x0409), Lang::En);
        assert_eq!(lang_from_langid(0x040c), Lang::En); // French
        assert_eq!(lang_from_langid(0), Lang::En);
    }

    #[test]
    fn arguments_are_filled_in_order() {
        assert_eq!(fill("{1} before {0}", &["a", "b"]), "b before a");
    }
}
