//! The Windows half: a tray icon, a job object, and the dialogs a first run needs.
//!
//! Structured so that the unsafe calls are four small wrappers at the bottom of `sys`
//! rather than sprinkled through the logic. Everything here is ordinary Rust, and
//! everything worth testing lives in `launch` where a Linux machine can reach it.
//!
//! One launcher, several projects. The single-instance rule stays — two tray icons with no
//! way to tell which is which was never the thing anybody wanted — and what changed is that
//! the one icon now holds a list. Each project is a submenu carrying that project's own
//! actions, because every action here writes to one particular store, and a menu where that
//! is left implied is a menu that sets up the wrong project.

use crate::launch::{self, Server, StartError};
use crate::settings::{self, OpenIn, Settings};
use std::path::{Path, PathBuf};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIconBuilder};

mod sys;
mod window;

const APP: &str = "Cyberbrain";

/// One open project: its server, and its own delivery timer.
///
/// The timer is per project because enrolment is: one store on this machine can belong to a
/// company hub while the one beside it does not.
struct Running {
    server: Server,
    delivery: launch::Delivery,
    /// The window showing this project, once somebody has asked for it. `None` until then,
    /// and stale after the person closes it — closing a window closes a window, not a
    /// project, so the next Open makes a new one.
    window: Option<window::ProjectWindow>,
}

impl Running {
    fn new(server: Server) -> Running {
        Running {
            server,
            delivery: launch::Delivery::default(),
            window: None,
        }
    }

    /// Close this project for good: its window, then its server.
    fn shut(mut self) {
        if let Some(w) = &self.window {
            w.close();
        }
        self.server.stop();
    }
}

/// The menu ids of one project's submenu. Rebuilt with the menu, so they are held apart
/// from [`Running`] and matched to it by position.
#[derive(Default)]
struct ProjectIds {
    open: MenuId,
    folder: MenuId,
    clients: MenuId,
    enrol: MenuId,
    close: MenuId,
}

/// A built menu and the ids to recognise its items by.
struct Ui {
    menu: Menu,
    per_project: Vec<ProjectIds>,
    add: MenuId,
    panes: MenuId,
    open_window: MenuId,
    open_browser: MenuId,
    quit: MenuId,
}

pub fn run() {
    // Not a mode of the launcher but a message to the one already running. It comes from an
    // installer that cannot replace a file Windows has locked, and it exits with 1 if the
    // launcher is still there afterwards so the caller knows to reach for force.
    if std::env::args().skip(1).any(|a| a == "--quit") {
        if !quit_the_one_that_is_running() {
            std::process::exit(1);
        }
        return;
    }

    // Before anything else, and held for the whole run: two launchers would mean two tray
    // icons, two sets of servers and no way to tell which icon holds which project.
    let instance_path = settings::instance_path();
    let Some(_single) = sys::SingleInstance::acquire() else {
        show_the_one_that_is_running(instance_path.as_deref());
        return;
    };

    // Created straight after the mutex, so the answer to "is it running" and the way to ask
    // it to stop appear at the same moment.
    let quit_request = sys::QuitRequest::create();

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));

    let Some(server_exe) = launch::locate_server(exe_dir.as_deref()) else {
        sys::error_box(
            APP,
            "cyberbrain.exe was not found next to this program or on the PATH.\n\n\
             The installer puts both in the same folder; if you moved one of them, put it \
             back or reinstall.",
        );
        return;
    };

    let settings_path = settings::path();
    let mut settings = settings_path
        .as_deref()
        .map(settings::load)
        .unwrap_or_default();

    // Everything the launcher starts belongs to this job. Closing the last handle to it
    // kills the members, which is what makes "the servers die with the tray icon" true
    // even when the tray icon is killed from the task manager rather than asked to quit.
    let job = sys::JobObject::new();

    let mut projects = reopen(&server_exe, &settings, &job);
    // Written before anything else can be asked, so that what the message from `reopen` says
    // about forgetting a project is true even if the person cancels the next dialog.
    remember(&mut settings, settings_path.as_deref(), &projects);
    if projects.is_empty() {
        // Either a first run or a machine where everything remembered has gone. Both end in
        // the same question, and it is the only one worth asking on startup.
        let Some(first) = choose_project(&server_exe, &[], &job) else {
            return; // The user cancelled the folder dialog. Nothing to run, nothing to say.
        };
        projects.push(first);
        remember(&mut settings, settings_path.as_deref(), &projects);
    }
    publish_instance(instance_path.as_deref(), &projects);
    // The session's memory of a WebView2 that would not start; see `open_page`.
    let mut browser_only = false;
    // The side-by-side window, once somebody has asked for it. One at a time: a second
    // would be the same panes again, and closing one of them would leave the other stale.
    let mut panes: Option<window::ProjectWindow> = None;
    // One page, not one per project. Somebody with four projects open wants their memory,
    // not four windows every time they log in; the rest are a click away in the menu.
    if let Some(title) = titles(&projects).pop()
        && let Some(last) = projects.last_mut()
    {
        open_page(last, &title, settings.open_in, &mut browser_only);
    }

    let Some(mut ui) = menu_for(&dirs(&projects), settings.open_in) else {
        sys::error_box(APP, "the tray menu could not be built");
        return;
    };

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(clone_menu(&ui)))
        .with_tooltip(tooltip(&projects))
        .with_icon(tray_icon())
        .with_menu_on_left_click(true)
        .build();
    let tray = match tray {
        Ok(t) => t,
        Err(e) => {
            sys::error_box(APP, &format!("the tray icon could not be created: {e}"));
            return;
        }
    };

    // A message loop, because that is what a tray icon needs to exist at all. Menu clicks
    // arrive as window messages first and reach us on the channel after dispatch.
    sys::pump_messages(|| {
        let mut changed = false;
        // `changed` means "rebuild the menu", and a setting change does that too. This one
        // means the list of projects is different, which is a smaller and more useful fact.
        let mut list_changed = false;
        let mut stop = false;
        // Collected rather than acted on inside the loop: removing a project while walking
        // the events would renumber the ids the next event is about to be matched against.
        let mut closing: Vec<usize> = Vec::new();
        // Worked out before the events are walked: naming a project needs the whole list,
        // and `projects` is borrowed one entry at a time inside the loop.
        let titles_by_index = titles(&projects);

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == ui.add {
                // Against what is running, not against what was last saved. Two of these
                // events can arrive in one tick, and the settings are written once at the
                // end of it — checking those would let the same store be opened twice, which
                // is two servers writing one index.
                let open = dirs(&projects);
                if let Some(next) = choose_project(&server_exe, &open, &job) {
                    projects.push(next);
                    list_changed = true;
                    // The title comes from the list it has just joined: a name that has to
                    // grow to stay unique can only be worked out once the others are known.
                    if let Some(title) = titles(&projects).pop()
                        && let Some(last) = projects.last_mut()
                    {
                        open_page(last, &title, settings.open_in, &mut browser_only);
                    }
                    changed = true;
                }
                continue;
            }
            if event.id == ui.quit {
                stop = true;
                continue;
            }
            if event.id == ui.panes {
                show_all_command_lines(&mut panes, &projects, &mut browser_only);
                continue;
            }
            if event.id == ui.open_window || event.id == ui.open_browser {
                settings.open_in = if event.id == ui.open_window {
                    // Asked for again, so try again: the runtime may have been installed
                    // since the failure that turned this off.
                    browser_only = false;
                    OpenIn::Window
                } else {
                    OpenIn::Browser
                };
                if let Some(path) = settings_path.as_deref() {
                    let _ = settings::save(path, &settings);
                }
                // Rebuilt for the ticks: muda lets a checked item be clicked again, and a
                // menu that does not show which one is on is the reason for having ticks.
                changed = true;
                continue;
            }
            for (i, ids) in ui.per_project.iter().enumerate() {
                let Some(p) = projects.get_mut(i) else {
                    continue;
                };
                if event.id == ids.open {
                    let title = titles_by_index.get(i).cloned().unwrap_or_default();
                    open_page(p, &title, settings.open_in, &mut browser_only);
                } else if event.id == ids.folder {
                    sys::open_in_explorer(&p.server.project_dir);
                } else if event.id == ids.clients {
                    set_up_clients(&server_exe, &p.server.project_dir);
                } else if event.id == ids.enrol {
                    if connect_to_hub(&server_exe, &p.server.project_dir) {
                        // Deliver on the next tick rather than in a quarter of an hour: the
                        // person who just enrolled is the one who wants to see it arrive.
                        p.delivery = launch::Delivery::now();
                    }
                } else if event.id == ids.close {
                    closing.push(i);
                }
            }
        }

        // Sorted and deduplicated before anything is removed: two Close events for the same
        // project in one tick would otherwise take the index twice, which closes a project
        // nobody asked about or runs off the end of the list.
        closing.sort_unstable();
        closing.dedup();
        for i in closing.into_iter().rev() {
            projects.remove(i).shut();
            changed = true;
        }
        for p in projects.iter_mut() {
            p.delivery.tick(&server_exe, &p.server.project_dir);
        }

        // A window that has gone was closed by the person: the ones we close go with their
        // entry. Forgetting it here is what makes the next Open build a new one instead of
        // focusing a handle that is no longer a window.
        let mut left_a_window = false;
        for p in projects.iter_mut() {
            if p.window.as_ref().is_some_and(|w| !w.is_open()) {
                p.window = None;
                left_a_window = true;
            }
        }
        if panes.as_ref().is_some_and(|w| !w.is_open()) {
            panes = None;
            left_a_window = true;
        }
        // Nothing on screen any more, and the program still running. Said once per machine,
        // because the honest reading of an empty screen is that Cyberbrain has quit — and
        // Windows 11 hides new notification-area icons behind an arrow, so there is nothing
        // to correct that reading. Somebody who believes it has quit reinstalls over a
        // running program, and the installer is the one that finds out.
        if left_a_window
            && !settings.told_about_tray
            && panes.is_none()
            && projects.iter().all(|p| p.window.is_none())
        {
            settings.told_about_tray = true;
            if let Some(path) = settings_path.as_deref() {
                let _ = settings::save(path, &settings);
            }
            sys::info_box(
                APP,
                concat!(
                    "Cyberbrain is still running.\n\n",
                    "Closing a window closes the page, not the program: the servers stay ",
                    "up, so your memory is there the moment you want it back. Its icon ",
                    "lives in the notification area, next to the clock — behind the ",
                    "arrow, if Windows has tidied it away. Quit in that icon's menu is ",
                    "what closes Cyberbrain.\n\nSaid once.",
                ),
            );
        }

        // A server going away on its own is not something to hide: without it that entry in
        // the menu is a button that does nothing. The others are unaffected, so only the one
        // that died goes.
        let mut died = Vec::new();
        for (i, p) in projects.iter_mut().enumerate() {
            if let Some(status) = p.server.exit_status() {
                died.push((i, p.server.project_dir.clone(), status));
            }
        }
        for (i, dir, status) in died.into_iter().rev() {
            // `shut`, not `remove` alone: the window is the project's too, and dropping the
            // entry does not close it. It used to be left standing on screen showing a dead
            // address, with the launcher no longer holding its handle — closable only by
            // hand.
            projects.remove(i).shut();
            changed = true;
            list_changed = true;
            sys::error_box(
                APP,
                &format!(
                    "cyberbrain serve stopped on its own ({status}) for\n{}\n\nThat project \
                     has been closed. The others are still open.",
                    dir.display()
                ),
            );
        }

        // Its panes are one per project, so a project leaving makes it wrong. Closed rather
        // than rebuilt: reopening is one click, and a window that silently loses a pane is
        // harder to trust than one that went away when the list changed.
        //
        // Keyed on the list, not on `changed`: switching Open in › The web browser also sets
        // `changed`, and that has nothing to do with the panes. It used to take the window
        // with it.
        if list_changed && let Some(w) = panes.take() {
            w.close();
        }

        if changed {
            remember(&mut settings, settings_path.as_deref(), &projects);
            publish_instance(instance_path.as_deref(), &projects);
            match menu_for(&dirs(&projects), settings.open_in) {
                Some(next) => {
                    tray.set_menu(Some(Box::new(clone_menu(&next))));
                    ui = next;
                }
                // The list changed and the menu could not be rebuilt to match. The entries
                // on screen are now one project out of step with the list behind them, and
                // acting on them would open, enrol or set up a store nobody pointed at. So
                // they are disowned: the menu still draws, and nothing in it fires.
                None => {
                    ui.per_project.clear();
                    sys::error_box(
                        APP,
                        "The tray menu could not be rebuilt, so the project entries in it \
                         have stopped working. Quit and start Cyberbrain again.",
                    );
                }
            }
            let _ = tray.set_tooltip(Some(tooltip(&projects)));
        }

        // Somebody outside asked — the installer, or `cyberbrain-desktop.exe --quit`. The
        // same way out as the menu item, so the servers are stopped, the last delivery goes
        // and the instance file is cleared, rather than a `taskkill` that does none of it.
        if quit_request.asked() {
            stop = true;
        }

        if stop {
            sys::Pump::Stop
        } else {
            sys::Pump::Continue
        }
    });

    // One last delivery on the way out, so a day's rows do not wait for tomorrow's login.
    for p in projects.iter_mut() {
        p.delivery.final_push(&server_exe, &p.server.project_dir);
    }
    if let Some(w) = panes.take() {
        w.close();
    }
    for p in projects.drain(..) {
        p.shut();
    }
    if let Some(path) = instance_path.as_deref() {
        settings::clear_instance(path);
    }
    drop(job); // Belt as well as braces: the job takes anything the children left behind.
}

/// Start the projects that were open last time.
///
/// Anything that no longer starts is dropped and named in one message at the end. One
/// dialog per broken project, before the tray icon has even appeared, is how a launcher
/// with four projects becomes a launcher nobody opens.
fn reopen(server_exe: &Path, settings: &Settings, job: &sys::JobObject) -> Vec<Running> {
    let mut running = Vec::new();
    let mut lost: Vec<String> = Vec::new();
    for dir in &settings.projects {
        match launch::start(server_exe, dir) {
            Ok(server) => {
                job.adopt(&server);
                running.push(Running::new(server));
            }
            Err(e) => lost.push(format!("{}\n    {e}", dir.display())),
        }
    }
    if !lost.is_empty() {
        sys::error_box(
            APP,
            &format!(
                "These projects could not be opened and have been forgotten:\n\n{}\n\nUse \
                 Open another project… to point at them again once they are back.",
                lost.join("\n\n")
            ),
        );
    }
    running
}

/// A second launcher's whole job: open the browser at what is already running.
///
/// It deliberately does not ask the first instance for anything: the addresses it left
/// behind are enough. Liveness needs no check here — we only got this far because the mutex
/// is held, and a launcher that died is not holding it, so its leftover file is never read.
/// Ask a running launcher to close, and wait until it has. `false` if it is still there.
///
/// Silent on purpose: the caller is an installer, and a dialog behind the installer's own
/// window reads as a program that has hung. Ten seconds, which is twice the bound on the
/// last delivery to a hub that is not answering.
fn quit_the_one_that_is_running() -> bool {
    if !sys::launcher_running() {
        return true;
    }
    sys::ask_running_to_quit();
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if !sys::launcher_running() {
            return true;
        }
    }
    false
}

fn show_the_one_that_is_running(instance_path: Option<&Path>) {
    if let Some(instance) = instance_path.and_then(settings::read_instance)
        && let Some(url) = instance.latest().and_then(launch::loopback_url)
    {
        sys::open_in_browser(&url);
        return;
    }
    // The mutex says a launcher exists, but it has left no usable address: it is still
    // starting up, or something rewrote the file. Saying so beats a second icon appearing
    // for no visible reason.
    sys::error_box(
        APP,
        "Cyberbrain is already running.\n\nIf no window opened, it is still starting up — \
         give it a moment, then use the Cyberbrain icon in the notification area.",
    );
}

fn dirs(projects: &[Running]) -> Vec<PathBuf> {
    projects
        .iter()
        .map(|p| p.server.project_dir.clone())
        .collect()
}

/// What to reopen next time. Written on every change rather than on the way out, because
/// the way out is also the path where the machine was shut down under us.
fn remember(settings: &mut Settings, path: Option<&Path>, projects: &[Running]) {
    settings.projects = dirs(projects);
    if let Some(path) = path {
        // Not fatal: it runs, it just will not remember next time. Not worth a dialog
        // either — the person is in the middle of opening a project, not of saving one.
        let _ = settings::save(path, settings);
    }
}

fn publish_instance(path: Option<&Path>, projects: &[Running]) {
    // Not worth a dialog if it fails. The cost is that a second launch says "already
    // running" instead of opening a page, and the first one keeps working either way.
    if let Some(path) = path {
        let _ = settings::write_instance(
            path,
            &settings::Instance::new(
                projects
                    .iter()
                    .map(|p| settings::Open {
                        url: p.server.url.clone(),
                        project_dir: p.server.project_dir.clone(),
                    })
                    .collect(),
            ),
        );
    }
}

/// The menu, and the ids to recognise it by.
///
/// Rebuilt whole whenever the list of projects changes rather than patched: a submenu
/// removed from the middle of a patched menu leaves the ids after it pointing at the wrong
/// project, and that failure is silent and acts on somebody's store.
fn menu_for(dirs: &[PathBuf], open_in: OpenIn) -> Option<Ui> {
    let menu = Menu::new();
    let mut per_project = Vec::new();
    for label in launch::labels(dirs) {
        let open = MenuItem::new("Open", true, None);
        let folder = MenuItem::new("Open project folder", true, None);
        let clients = MenuItem::new("Set up Claude on this computer…", true, None);
        let enrol = MenuItem::new("Connect to the company hub…", true, None);
        let close = MenuItem::new("Close this project", true, None);
        let sub = Submenu::with_items(
            &label,
            true,
            &[
                &open,
                &folder,
                &PredefinedMenuItem::separator(),
                &clients,
                &enrol,
                &PredefinedMenuItem::separator(),
                &close,
            ],
        )
        .ok()?;
        menu.append(&sub).ok()?;
        per_project.push(ProjectIds {
            open: open.id().clone(),
            folder: folder.id().clone(),
            clients: clients.id().clone(),
            enrol: enrol.id().clone(),
            close: close.id().clone(),
        });
    }

    let add = MenuItem::new("Open another project…", true, None);
    // The command line of every open project, tiled in one window. Disabled with nothing
    // open, rather than absent: an entry that comes and goes is one people stop looking for.
    let panes = MenuItem::new("All command lines side by side", !dirs.is_empty(), None);
    // Ticks rather than a single toggle: a toggle labelled "Open in a window" leaves the
    // person to work out what is happening now, and this is a setting they may only ever
    // look at once.
    let in_window =
        CheckMenuItem::new("A window of its own", true, open_in == OpenIn::Window, None);
    let in_browser = CheckMenuItem::new("The web browser", true, open_in == OpenIn::Browser, None);
    let open_in_menu = Submenu::with_items("Open in", true, &[&in_window, &in_browser]).ok()?;
    let quit = MenuItem::new("Quit", true, None);

    let mut tail: Vec<&dyn tray_icon::menu::IsMenuItem> = Vec::new();
    let sep = PredefinedMenuItem::separator();
    if !dirs.is_empty() {
        tail.push(&sep);
    }
    let sep2 = PredefinedMenuItem::separator();
    tail.push(&add);
    tail.push(&panes);
    tail.push(&open_in_menu);
    tail.push(&sep2);
    tail.push(&quit);
    menu.append_items(&tail).ok()?;

    Some(Ui {
        menu,
        per_project,
        add: add.id().clone(),
        panes: panes.id().clone(),
        open_window: in_window.id().clone(),
        open_browser: in_browser.id().clone(),
        quit: quit.id().clone(),
    })
}

/// Every open project's command line, tiled in one window.
///
/// The panes are the projects' own pages at their own addresses, opened straight at the
/// command line screen. Nothing reaches across them — the browser's origin rule and the
/// page's own `connect-src 'self'` both see to that — so this is a layout and not a new
/// way for one project to touch another.
fn show_all_command_lines(
    panes: &mut Option<window::ProjectWindow>,
    projects: &[Running],
    browser_only: &mut bool,
) {
    if panes.as_ref().is_some_and(|w| w.is_open()) {
        if let Some(w) = panes.as_ref() {
            w.focus();
        }
        return;
    }
    *panes = None;
    if projects.is_empty() {
        return;
    }
    let urls: Vec<String> = projects
        .iter()
        .map(|p| format!("{}#/console", p.server.url))
        .collect();
    match window::open_panes("Cyberbrain — command lines", &urls) {
        Ok(w) => *panes = Some(w),
        Err(why) => {
            *browser_only = true;
            sys::error_box(
                APP,
                &format!("{why}\n\nThe command line is also in each project's own page."),
            );
        }
    }
}

/// Show a project's page where the person said it should go.
///
/// `browser_only` is the session's memory of a WebView2 that would not start. Without it a
/// machine with no runtime asks the same impossible thing on every click and answers with
/// the same dialog; with it, the first failure is explained once and everything opens in
/// the browser until the launcher is restarted.
fn open_page(p: &mut Running, title: &str, open_in: OpenIn, browser_only: &mut bool) {
    if open_in == OpenIn::Browser || *browser_only {
        sys::open_in_browser(&p.server.open_url);
        return;
    }
    // Cleared rather than left standing: Windows reuses window handles, so a stale one can
    // start answering for a window somebody else opened — and then Open focuses the wrong
    // window, and closing this project destroys it.
    if p.window.as_ref().is_some_and(|w| !w.is_open()) {
        p.window = None;
    }
    if let Some(w) = &p.window {
        w.focus();
        return;
    }
    match window::open(title, &p.server.open_url) {
        Ok(w) => p.window = Some(w),
        Err(why) => {
            *browser_only = true;
            sys::error_box(
                APP,
                &format!(
                    "{why}\n\nThis page has been opened in the browser instead, and so will \
                     the others until Cyberbrain is started again. Open in › The web browser \
                     makes that the setting."
                ),
            );
            sys::open_in_browser(&p.server.open_url);
        }
    }
}

/// The title of a project's window: the same short name its menu entry carries, so the
/// taskbar button and the menu entry are recognisably the same thing.
fn titles(projects: &[Running]) -> Vec<String> {
    launch::labels(&dirs(projects))
}

/// The tray takes ownership of the menu it is given; the ids stay with us. `Menu` is a
/// handle, so this hands over the same menu rather than a copy of it.
fn clone_menu(ui: &Ui) -> Menu {
    ui.menu.clone()
}

fn tooltip(projects: &[Running]) -> String {
    match projects {
        [] => "Cyberbrain — nothing open".to_string(),
        [one] => format!("Cyberbrain — {}", one.server.project_dir.display()),
        many => format!("Cyberbrain — {} projects open", many.len()),
    }
}

/// Put this project's memory into the AI clients installed here, and say what happened.
///
/// The report is shown whole rather than boiled down to "done". Two things in it are worth
/// a person's eyes: which file was written when Windows has two Claude Desktop
/// installations, and the lines to paste for a client this does not write.
fn set_up_clients(server_exe: &Path, project_dir: &Path) {
    match launch::set_up_clients(server_exe, project_dir) {
        Ok(said) => sys::info_box(
            APP,
            &format!("{said}\n\nRestart the client for it to read this."),
        ),
        Err(why) => sys::error_box(APP, &format!("Nothing was set up.\n\n{why}")),
    }
}

/// Take the invitation file the company sent and enrol this store with their hub.
///
/// One dialog, one answer. The alternative is a paragraph of instructions ending in a
/// command, and the people this is for do not get to the end of that paragraph.
fn connect_to_hub(server_exe: &Path, project_dir: &Path) -> bool {
    let Some(file) = rfd::FileDialog::new()
        .set_title("Open the invitation file you were sent")
        .add_filter("Invitation", &["json"])
        .pick_file()
    else {
        return false; // Cancelled. Nothing happened, so nothing is said.
    };
    match launch::enrol(server_exe, project_dir, &file) {
        Ok(said) => {
            sys::info_box(
                APP,
                &format!(
                    "{said}\n\nThis machine will now deliver its audit trail to that hub: what \
                     happened, not what a note says. Sharing notes is a separate setting, \
                     and it is off unless somebody turns it on."
                ),
            );
            true
        }
        Err(why) => {
            sys::error_box(APP, &format!("The invitation was not accepted.\n\n{why}"));
            false
        }
    }
}

/// Ask for a folder and start there, until it works or the user gives up.
///
/// A folder that is already open is not an error and not a second server: the answer to
/// "open this one too" when it is open is to say so.
fn choose_project(server_exe: &Path, open: &[PathBuf], job: &sys::JobObject) -> Option<Running> {
    loop {
        let chosen = rfd::FileDialog::new()
            .set_title("Choose the project whose memory to open")
            .set_directory(
                open.last()
                    .filter(|d| d.is_dir())
                    .cloned()
                    .unwrap_or_else(|| {
                        std::env::var_os("USERPROFILE")
                            .map(PathBuf::from)
                            .unwrap_or_default()
                    }),
            )
            .pick_folder()?;

        let dir = launch::normalise_project_dir(&chosen);
        // Two servers on one store would be two writers on one index. Compared by the store
        // each folder resolves to, not by the folder: the store is found by walking upwards,
        // so a subdirectory of an open project is a different folder and the same store, and
        // comparing folders let it be opened a second time.
        let already = launch::store_of(&dir).is_some_and(|store| {
            open.iter()
                .filter_map(|d| launch::store_of(d))
                .any(|other| other == store)
        });
        if already || open.contains(&dir) {
            sys::info_box(
                APP,
                &format!("{}\n\nis already open. It is in the menu.", dir.display()),
            );
            continue;
        }
        match try_start(server_exe, &dir, job) {
            Started::Ok(running) => return Some(running),
            Started::Rejected => continue,
            Started::Impossible => return None,
        }
    }
}

enum Started {
    Ok(Running),
    /// It did not start and the user has been told; ask again.
    Rejected,
    /// Nothing about this installation can work, so asking for another folder would only
    /// be the same dialog again with a different path in it.
    Impossible,
}

/// One attempt at a folder, including the offer to create a store where there is none.
fn try_start(server_exe: &Path, dir: &Path, job: &sys::JobObject) -> Started {
    match launch::start(server_exe, dir) {
        Ok(server) => {
            job.adopt(&server);
            Started::Ok(Running::new(server))
        }
        Err(StartError::NoStore { dir }) => {
            let question = format!(
                "There is no Cyberbrain store in\n{}\n\nCreate one there now?",
                dir.display()
            );
            if !sys::ask_yes_no(APP, &question) {
                return Started::Rejected;
            }
            if let Err(e) = launch::init_store(server_exe, &dir) {
                sys::error_box(APP, &format!("The store could not be created:\n\n{e}"));
                return Started::Rejected;
            }
            match launch::start(server_exe, &dir) {
                Ok(server) => {
                    job.adopt(&server);
                    Started::Ok(Running::new(server))
                }
                Err(e) => {
                    sys::error_box(APP, &format!("Cyberbrain could not start:\n\n{e}"));
                    Started::Rejected
                }
            }
        }
        // Not a folder problem, so there is no folder to try next: this copy of cyberbrain
        // has no page, and opening a page is all this program does.
        Err(e @ StartError::NoPage) => {
            sys::error_box(
                APP,
                &format!(
                    "Cyberbrain cannot open:\n\n{e}.\n\nInstall the release build from \
                     the Cyberbrain website, which has the page built in. The command-line \
                     tool beside it works either way."
                ),
            );
            Started::Impossible
        }
        Err(e @ StartError::Failed { .. }) => {
            let detail = match &e {
                StartError::Failed { output, .. } if !output.trim().is_empty() => {
                    format!("\n\nWhat it printed:\n{}", tail(output, 600))
                }
                _ => String::new(),
            };
            sys::error_box(APP, &format!("Cyberbrain could not start:\n\n{e}{detail}"));
            Started::Rejected
        }
    }
}

fn tail(text: &str, max: usize) -> String {
    let t = text.trim_end();
    if t.len() <= max {
        return t.to_string();
    }
    let start = t.len() - max;
    let start = t
        .char_indices()
        .map(|(i, _)| i)
        .find(|&i| i >= start)
        .unwrap_or(0);
    format!("…{}", &t[start..])
}

/// The mark from `ui/public/favicon.svg`, as the tray wants it: straight RGBA.
///
/// One PNG feeds the tray here and the shortcut through the installer, so the icon in the
/// notification area and the one on the Start menu cannot drift apart.
fn tray_icon() -> Icon {
    const PNG: &[u8] = include_bytes!("../../assets/tray.png");
    let decoder = png::Decoder::new(std::io::Cursor::new(PNG));
    let mut reader = decoder.read_info().expect("the icon is a valid png");
    let mut buf = vec![0; reader.output_buffer_size().expect("icon size is known")];
    let info = reader.next_frame(&mut buf).expect("the icon has one frame");
    buf.truncate(info.buffer_size());
    Icon::from_rgba(buf, info.width, info.height).expect("the icon is rgba")
}
