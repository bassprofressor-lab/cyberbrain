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

use crate::i18n::{self, Key, tr};
use crate::launch::{self, Server, StartError};
use crate::settings::{self, OpenIn, Settings};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIconBuilder};

mod sys;
mod window;

const APP: &str = "Cyberbrain";

/// What a thread started from the message loop has to report. The loop never waits for any
/// of these: a project that takes twenty seconds to start, or a hub that does not answer an
/// enrolment, used to freeze the tray icon and every menu for as long as it took.
enum Outcome {
    Started {
        dir: PathBuf,
        result: Result<Server, StartError>,
    },
    Clients(Result<String, String>),
    Enrolled {
        dir: PathBuf,
        /// Chosen from the menu, as against offered by the rollout. The first gets the long
        /// explanation of what now leaves the machine; the second already had it in the
        /// question.
        from_menu: bool,
        result: Result<String, String>,
    },
}

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
    /// Whether the company hub was offered for this project in this session.
    hub_asked: bool,
}

impl Running {
    fn new(server: Server) -> Running {
        Running {
            server,
            delivery: launch::Delivery::default(),
            window: None,
            hub_asked: false,
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
    autostart: MenuId,
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
    // From here on there may be a dialog, and it speaks the language Windows does.
    i18n::set_language(i18n::lang_from_langid(sys::ui_language_id()));
    // Started by the Run key at sign-in: keep to the tray, no window and no question.
    let background = std::env::args().skip(1).any(|a| a == "--background");

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
    let show_request = sys::ShowRequest::create();

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));

    let Some(server_exe) = launch::locate_server(exe_dir.as_deref()) else {
        sys::error_box(APP, &tr(Key::ServerNotFound, &[]));
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
        // the same question, and it is the only one worth asking on startup. Not at sign-in:
        // a folder dialog nobody asked for is the thing that gets autostart switched off.
        if background {
            return;
        }
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
    if !background
        && let Some(title) = titles(&projects).pop()
        && let Some(last) = projects.last_mut()
    {
        open_page(last, &title, settings.open_in, &mut browser_only);
    }

    let Some(mut ui) = menu_for(&dirs(&projects), settings.open_in, sys::autostart_enabled())
    else {
        sys::error_box(APP, &tr(Key::TrayMenuFailed, &[]));
        return;
    };

    // Projects being started in the background, so the menu and the tooltip can say so and
    // a second "open another" cannot pick the same folder again while the first is coming up.
    let mut pending: Vec<PathBuf> = Vec::new();
    let (tx, outcomes) = mpsc::channel::<Outcome>();
    let mut shown_tip = tooltip(&projects, &pending);

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(clone_menu(&ui)))
        .with_tooltip(shown_tip.clone())
        .with_icon(tray_icon())
        .with_menu_on_left_click(true)
        .build();
    let tray = match tray {
        Ok(t) => t,
        Err(e) => {
            sys::error_box(APP, &tr(Key::TrayIconFailed, &[&e.to_string()]));
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
                //
                // Folders still starting count as open for the same reason.
                let mut open = dirs(&projects);
                open.extend(pending.iter().cloned());
                if let Some(dir) = pick_folder(&server_exe, &open) {
                    pending.push(dir.clone());
                    spawn_start(&tx, &server_exe, dir);
                }
                continue;
            }
            if event.id == ui.quit {
                stop = true;
                continue;
            }
            if event.id == ui.autostart {
                let want = !sys::autostart_enabled();
                if let Err(why) = sys::set_autostart(want) {
                    sys::error_box(APP, &tr(Key::AutostartFailed, &[&why]));
                }
                // Rebuilt either way, so the tick shows what the registry says now.
                changed = true;
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
                    let (tx, exe, dir) =
                        (tx.clone(), server_exe.clone(), p.server.project_dir.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send(Outcome::Clients(launch::set_up_clients(&exe, &dir)));
                    });
                } else if event.id == ids.enrol {
                    if let Some(file) = pick_invitation() {
                        spawn_enrol(&tx, &server_exe, p.server.project_dir.clone(), file, true);
                    }
                } else if event.id == ids.close {
                    closing.push(i);
                }
            }
        }

        // Somebody started the launcher a second time: show them what is already running.
        if show_request.take()
            && let Some(title) = titles(&projects).pop()
            && let Some(last) = projects.last_mut()
        {
            open_page(last, &title, settings.open_in, &mut browser_only);
        }

        // What the threads started above have to say.
        while let Ok(outcome) = outcomes.try_recv() {
            match outcome {
                Outcome::Started { dir, result } => {
                    pending.retain(|d| *d != dir);
                    match result {
                        Ok(server) => {
                            job.adopt(&server);
                            projects.push(Running::new(server));
                            list_changed = true;
                            changed = true;
                            // The title comes from the list it has just joined: a name that
                            // has to grow to stay unique can only be worked out once the
                            // others are known.
                            if let Some(title) = titles(&projects).pop()
                                && let Some(last) = projects.last_mut()
                            {
                                open_page(last, &title, settings.open_in, &mut browser_only);
                            }
                        }
                        Err(e) => {
                            report_start_error(&e);
                        }
                    }
                }
                Outcome::Clients(Ok(said)) => {
                    sys::info_box(APP, &tr(Key::ClientsDone, &[&said]));
                }
                Outcome::Clients(Err(why)) => {
                    sys::error_box(APP, &tr(Key::ClientsFailed, &[&why]));
                }
                Outcome::Enrolled {
                    dir,
                    from_menu,
                    result: Ok(said),
                } => {
                    let text = if from_menu {
                        tr(Key::EnrolDone, &[&said])
                    } else {
                        said
                    };
                    sys::info_box(APP, &text);
                    // Deliver on the next tick rather than in a quarter of an hour: the
                    // person who just enrolled is the one who wants to see it arrive.
                    if let Some(p) = projects.iter_mut().find(|p| p.server.project_dir == dir) {
                        p.delivery = launch::Delivery::now();
                    }
                }
                Outcome::Enrolled {
                    from_menu,
                    result: Err(why),
                    ..
                } => {
                    let key = if from_menu {
                        Key::EnrolFailed
                    } else {
                        Key::HubConnectFailed
                    };
                    sys::error_box(APP, &tr(key, &[&why]));
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
            // The list is different too: the side-by-side window holds this project's
            // address, and a pane left on a server that has just been stopped is a tile
            // that shows a dead page. Only a project dying by itself used to say so.
            list_changed = true;
        }
        for p in projects.iter_mut() {
            p.delivery.tick(&server_exe, &p.server.project_dir);
        }

        // A rollout that left a fleet invitation on this machine gets one question per
        // project, and only once a delivery has shown the project is not connected. That
        // answer arrives on a tick, which is why this comes straight after them.
        let central = launch::central_invitation();
        for p in projects.iter_mut() {
            if !launch::should_offer_hub(
                central.as_deref(),
                &p.delivery,
                &p.server.project_dir,
                &settings.hub_declined,
                p.hub_asked,
            ) {
                continue;
            }
            let Some(invitation) = central.as_deref() else {
                continue;
            };
            p.hub_asked = true;
            let name = p
                .server
                .project_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.server.project_dir.display().to_string());
            let question = tr(Key::HubOffer, &[&name]);
            if sys::ask_yes_no(APP, &question) {
                spawn_enrol(
                    &tx,
                    &server_exe,
                    p.server.project_dir.clone(),
                    invitation.to_path_buf(),
                    false,
                );
            } else {
                settings.hub_declined.push(p.server.project_dir.clone());
                if let Some(path) = settings_path.as_deref() {
                    let _ = settings::save(path, &settings);
                }
            }
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
            sys::info_box(APP, &tr(Key::TrayExplained, &[]));
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
                &tr(
                    Key::ServeStopped,
                    &[&status.to_string(), &dir.display().to_string()],
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
            match menu_for(&dirs(&projects), settings.open_in, sys::autostart_enabled()) {
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
                    sys::error_box(APP, &tr(Key::MenuRebuildFailed, &[]));
                }
            }
        }
        // On every tick rather than inside `changed`: a project that is still starting
        // changes the words and not the list.
        let tip = tooltip(&projects, &pending);
        if tip != shown_tip {
            let _ = tray.set_tooltip(Some(tip.clone()));
            shown_tip = tip;
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
    // All at once: each can take its twenty seconds, and four projects one after the other
    // were eighty seconds before the tray icon appeared. The order is kept.
    let starting: Vec<_> = settings
        .projects
        .iter()
        .map(|dir| {
            let (exe, d) = (server_exe.to_path_buf(), dir.clone());
            (dir, std::thread::spawn(move || launch::start(&exe, &d)))
        })
        .collect();
    for (dir, handle) in starting {
        match handle.join() {
            Ok(Ok(server)) => {
                job.adopt(&server);
                running.push(Running::new(server));
            }
            Ok(Err(e)) => lost.push(format!("{}\n    {e}", dir.display())),
            Err(_) => lost.push(format!("{}\n    the start crashed", dir.display())),
        }
    }
    if !lost.is_empty() {
        sys::error_box(APP, &tr(Key::ProjectsLost, &[&lost.join("\n\n")]));
    }
    running
}

/// Start a project on a thread and report to the loop, which keeps pumping meanwhile.
fn spawn_start(tx: &mpsc::Sender<Outcome>, server_exe: &Path, dir: PathBuf) {
    let (tx, exe) = (tx.clone(), server_exe.to_path_buf());
    std::thread::spawn(move || {
        let result = launch::start(&exe, &dir);
        let _ = tx.send(Outcome::Started { dir, result });
    });
}

/// Enrol a project on a thread; the answer is shown by the loop when it arrives.
fn spawn_enrol(
    tx: &mpsc::Sender<Outcome>,
    server_exe: &Path,
    dir: PathBuf,
    invitation: PathBuf,
    from_menu: bool,
) {
    let (tx, exe) = (tx.clone(), server_exe.to_path_buf());
    std::thread::spawn(move || {
        let result = launch::enrol(&exe, &dir, &invitation);
        let _ = tx.send(Outcome::Enrolled {
            dir,
            from_menu,
            result,
        });
    });
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
    // First choice: ask the running launcher to show its page itself, so the person gets
    // what `Open in` says (their window, with the terminal) and not a bare browser tab.
    if sys::ask_running_to_show() {
        return;
    }
    // An older launcher listens for nothing; its address file is the fallback.
    if let Some(instance) = instance_path.and_then(settings::read_instance)
        && let Some(url) = instance.latest().and_then(launch::loopback_url)
    {
        sys::open_in_browser(&url);
        return;
    }
    // The mutex says a launcher exists, but it has left no usable address: it is still
    // starting up, or something rewrote the file. Saying so beats a second icon appearing
    // for no visible reason.
    sys::error_box(APP, &tr(Key::AlreadyRunning, &[]));
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
fn menu_for(dirs: &[PathBuf], open_in: OpenIn, autostart_on: bool) -> Option<Ui> {
    let menu = Menu::new();
    let mut per_project = Vec::new();
    for label in launch::labels(dirs) {
        let open = MenuItem::new(tr(Key::MenuOpen, &[]), true, None);
        let folder = MenuItem::new(tr(Key::MenuFolder, &[]), true, None);
        let clients = MenuItem::new(tr(Key::MenuClients, &[]), true, None);
        let enrol = MenuItem::new(tr(Key::MenuEnrol, &[]), true, None);
        let close = MenuItem::new(tr(Key::MenuClose, &[]), true, None);
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

    let add = MenuItem::new(tr(Key::MenuAdd, &[]), true, None);
    // The command line of every open project, tiled in one window. Disabled with nothing
    // open, rather than absent: an entry that comes and goes is one people stop looking for.
    let panes = MenuItem::new(tr(Key::MenuPanes, &[]), !dirs.is_empty(), None);
    // Ticks rather than a single toggle: a toggle labelled "Open in a window" leaves the
    // person to work out what is happening now, and this is a setting they may only ever
    // look at once.
    let in_window = CheckMenuItem::new(
        tr(Key::MenuWindow, &[]),
        true,
        open_in == OpenIn::Window,
        None,
    );
    let in_browser = CheckMenuItem::new(
        tr(Key::MenuBrowser, &[]),
        true,
        open_in == OpenIn::Browser,
        None,
    );
    let open_in_menu =
        Submenu::with_items(tr(Key::MenuOpenIn, &[]), true, &[&in_window, &in_browser]).ok()?;
    // Off unless the person turns it on, and the tick shows what the registry says.
    let autostart = CheckMenuItem::new(tr(Key::MenuAutostart, &[]), true, autostart_on, None);
    let quit = MenuItem::new(tr(Key::MenuQuit, &[]), true, None);

    let mut tail: Vec<&dyn tray_icon::menu::IsMenuItem> = Vec::new();
    let sep = PredefinedMenuItem::separator();
    if !dirs.is_empty() {
        tail.push(&sep);
    }
    let sep2 = PredefinedMenuItem::separator();
    tail.push(&add);
    tail.push(&panes);
    tail.push(&open_in_menu);
    tail.push(&autostart);
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
        autostart: autostart.id().clone(),
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
        .map(|p| format!("{}#/console?pane=1", p.server.url))
        .collect();
    match window::open_panes(&tr(Key::PanesTitle, &[]), &urls) {
        Ok(w) => *panes = Some(w),
        Err(why) => {
            *browser_only = true;
            sys::error_box(APP, &tr(Key::PanesFailed, &[&why]));
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
            sys::error_box(APP, &tr(Key::PageInBrowser, &[&why]));
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

fn tooltip(projects: &[Running], pending: &[PathBuf]) -> String {
    if let Some(dir) = pending.first() {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.display().to_string());
        return tr(Key::Starting, &[&name]);
    }
    match projects {
        [] => tr(Key::TooltipNone, &[]),
        [one] => tr(
            Key::TooltipOne,
            &[&one.server.project_dir.display().to_string()],
        ),
        many => tr(Key::TooltipMany, &[&many.len().to_string()]),
    }
}

/// Ask for the invitation file the company sent.
///
/// One dialog, one answer. The alternative is a paragraph of instructions ending in a
/// command, and the people this is for do not get to the end of that paragraph. The
/// enrolment itself runs on a thread, see [`spawn_enrol`].
fn pick_invitation() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(tr(Key::InvitationTitle, &[]))
        .add_filter(tr(Key::InvitationFilter, &[]), &["json"])
        .pick_file()
}

/// Ask for a folder until it is one that can be started: not open already, and with a store
/// in it or one the person agreed to create.
///
/// A folder that is already open is not an error and not a second server: the answer to
/// "open this one too" when it is open is to say so. Starting it is the caller's, so that
/// the loop can do it on a thread and keep the tray alive meanwhile.
fn pick_folder(server_exe: &Path, open: &[PathBuf]) -> Option<PathBuf> {
    loop {
        let chosen = rfd::FileDialog::new()
            .set_title(tr(Key::ChooseProject, &[]))
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
            sys::info_box(APP, &tr(Key::AlreadyOpen, &[&dir.display().to_string()]));
            continue;
        }
        if !launch::has_store(&dir) {
            let question = tr(Key::NoStoreAsk, &[&dir.display().to_string()]);
            if !sys::ask_yes_no(APP, &question) {
                continue;
            }
            if let Err(e) = launch::init_store(server_exe, &dir) {
                sys::error_box(APP, &tr(Key::StoreFailed, &[&e]));
                continue;
            }
        }
        return Some(dir);
    }
}

/// Ask for a folder and start there, until it works or the user gives up. Only for the
/// first run, where nothing else is waiting for the loop that does not exist yet.
fn choose_project(server_exe: &Path, open: &[PathBuf], job: &sys::JobObject) -> Option<Running> {
    loop {
        let dir = pick_folder(server_exe, open)?;
        match launch::start(server_exe, &dir) {
            Ok(server) => {
                job.adopt(&server);
                return Some(Running::new(server));
            }
            Err(e) => {
                if report_start_error(&e) {
                    return None;
                }
            }
        }
    }
}

/// Tell the person why a project did not start. `true` when nothing about this installation
/// can work, so asking for another folder would only be the same dialog again.
fn report_start_error(e: &StartError) -> bool {
    match e {
        // Not a folder problem, so there is no folder to try next: this copy of cyberbrain
        // has no page, and opening a page is all this program does.
        StartError::NoPage => {
            sys::error_box(APP, &tr(Key::CannotOpen, &[&e.to_string()]));
            true
        }
        StartError::Failed { output, .. } if !output.trim().is_empty() => {
            let detail = tr(Key::PrintedHeader, &[&tail(output, 600)]);
            sys::error_box(APP, &tr(Key::StartFailed, &[&format!("{e}{detail}")]));
            false
        }
        _ => {
            sys::error_box(APP, &tr(Key::StartFailed, &[&e.to_string()]));
            false
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

/// The app's mark, as the tray wants it: straight RGBA.
///
/// `tray.png`, `window.png` and the installer's `cyberbrain.ico` are three renderings of one
/// picture (the neon brain with its orbit ring, 25.10.2026), so the icon in the notification
/// area, the window and the one on the Start menu cannot drift apart.
fn tray_icon() -> Icon {
    const PNG: &[u8] = include_bytes!("../../assets/tray.png");
    let decoder = png::Decoder::new(std::io::Cursor::new(PNG));
    let mut reader = decoder.read_info().expect("the icon is a valid png");
    let mut buf = vec![0; reader.output_buffer_size().expect("icon size is known")];
    let info = reader.next_frame(&mut buf).expect("the icon has one frame");
    buf.truncate(info.buffer_size());
    Icon::from_rgba(buf, info.width, info.height).expect("the icon is rgba")
}
