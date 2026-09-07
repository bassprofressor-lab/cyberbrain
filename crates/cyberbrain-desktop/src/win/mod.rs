//! The Windows half: a tray icon, a job object, and the two dialogs a first run needs.
//!
//! Structured so that the unsafe calls are four small wrappers at the bottom of the file
//! rather than sprinkled through the logic. Everything above them is ordinary Rust, and
//! everything worth testing lives in `launch` where a Linux machine can reach it.

use crate::launch::{self, Server, StartError};
use crate::settings::{self, Settings};
use std::path::{Path, PathBuf};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

mod sys;

const APP: &str = "Cyberbrain";

pub fn run() {
    // Before anything else, and held for the whole run: two launchers would mean two tray
    // icons, two servers and two ports, with no way to tell which icon belongs to which.
    let instance_path = settings::instance_path();
    let Some(_single) = sys::SingleInstance::acquire() else {
        show_the_one_that_is_running(instance_path.as_deref());
        return;
    };

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
    // kills the members, which is what makes "the server dies with the tray icon" true
    // even when the tray icon is killed from the task manager rather than asked to quit.
    let job = sys::JobObject::new();

    let Some(mut server) = open_project(&server_exe, &mut settings, settings_path.as_deref(), &job)
    else {
        return; // The user cancelled the folder dialog. Nothing to run, nothing to say.
    };
    publish_instance(instance_path.as_deref(), &server);
    sys::open_in_browser(&server.url);

    let menu = Menu::new();
    let open = MenuItem::new("Open Cyberbrain", true, None);
    let folder = MenuItem::new("Open project folder", true, None);
    let choose = MenuItem::new("Choose project…", true, None);
    let quit = MenuItem::new("Quit", true, None);
    if menu
        .append_items(&[
            &open,
            &folder,
            &PredefinedMenuItem::separator(),
            &choose,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .is_err()
    {
        sys::error_box(APP, "the tray menu could not be built");
        return;
    }

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(tooltip(&server))
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

    let ids = Ids {
        open: open.id().clone(),
        folder: folder.id().clone(),
        choose: choose.id().clone(),
        quit: quit.id().clone(),
    };

    // A message loop, because that is what a tray icon needs to exist at all. Menu clicks
    // arrive as window messages first and reach us on the channel after dispatch.
    sys::pump_messages(|| {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == ids.open {
                sys::open_in_browser(&server.url);
            } else if event.id == ids.folder {
                sys::open_in_explorer(&server.project_dir);
            } else if event.id == ids.choose {
                if let Some(next) =
                    choose_project(&server_exe, &mut settings, settings_path.as_deref(), &job)
                {
                    server = next;
                    publish_instance(instance_path.as_deref(), &server);
                    let _ = tray.set_tooltip(Some(tooltip(&server)));
                    sys::open_in_browser(&server.url);
                }
            } else if event.id == ids.quit {
                return sys::Pump::Stop;
            }
        }

        // The server going away on its own is not something to hide: without it the tray
        // icon is a button that does nothing.
        if let Some(status) = server.exit_status() {
            sys::error_box(
                APP,
                &format!("cyberbrain serve stopped on its own ({status}). Cyberbrain will close."),
            );
            return sys::Pump::Stop;
        }
        sys::Pump::Continue
    });

    server.stop();
    if let Some(path) = instance_path.as_deref() {
        settings::clear_instance(path);
    }
    drop(job); // Belt as well as braces: the job takes anything the child left behind.
}

/// A second launcher's whole job: open the browser at the one that is already running.
///
/// It deliberately does not ask the first instance for anything. The address it left behind
/// is checked by connecting to it, so a file from a launcher that crashed, or from before a
/// reboot, cannot send anyone to a dead port.
fn show_the_one_that_is_running(instance_path: Option<&Path>) {
    if let Some(instance) = instance_path.and_then(settings::read_instance)
        && launch::responds(&instance.url)
    {
        sys::open_in_browser(&instance.url);
        return;
    }
    // The mutex says a launcher exists, but it has no live address: it is still starting,
    // or it is stuck. Saying so beats a second icon appearing for no visible reason.
    sys::error_box(
        APP,
        "Cyberbrain is already running.\n\nIf no window opened, it is still starting up — \
         give it a moment, then use the Cyberbrain icon in the notification area.",
    );
}

fn publish_instance(path: Option<&Path>, server: &Server) {
    // Not worth a dialog if it fails. The cost is that a second launch says "already
    // running" instead of opening the page, and the first one keeps working either way.
    if let Some(path) = path {
        let _ = settings::write_instance(
            path,
            &settings::Instance {
                url: server.url.clone(),
                project_dir: server.project_dir.clone(),
            },
        );
    }
}

struct Ids {
    open: MenuId,
    folder: MenuId,
    choose: MenuId,
    quit: MenuId,
}

fn tooltip(server: &Server) -> String {
    format!("Cyberbrain — {}", server.project_dir.display())
}

/// Open the remembered project, or ask for one. Returns `None` only if the user cancels.
fn open_project(
    server_exe: &Path,
    settings: &mut Settings,
    settings_path: Option<&Path>,
    job: &sys::JobObject,
) -> Option<Server> {
    if let Some(dir) = settings.project_dir.clone()
        && dir.is_dir()
    {
        match try_start(server_exe, &dir, job) {
            Started::Ok(server) => return Some(server),
            // A remembered folder that no longer works is worth one dialog, then the
            // question everyone would ask next: which folder, then?
            Started::Rejected => {}
        }
    }
    choose_project(server_exe, settings, settings_path, job)
}

/// Ask for a folder and start there, until it works or the user gives up.
fn choose_project(
    server_exe: &Path,
    settings: &mut Settings,
    settings_path: Option<&Path>,
    job: &sys::JobObject,
) -> Option<Server> {
    loop {
        let chosen = rfd::FileDialog::new()
            .set_title("Choose the project whose memory to open")
            .set_directory(
                settings
                    .project_dir
                    .clone()
                    .filter(|d| d.is_dir())
                    .unwrap_or_else(|| {
                        std::env::var_os("USERPROFILE")
                            .map(PathBuf::from)
                            .unwrap_or_default()
                    }),
            )
            .pick_folder()?;

        let dir = launch::normalise_project_dir(&chosen);
        match try_start(server_exe, &dir, job) {
            Started::Ok(server) => {
                settings.project_dir = Some(dir);
                if let Some(path) = settings_path
                    && let Err(e) = settings::save(path, settings)
                {
                    // Not fatal: it runs, it just will not remember next time.
                    sys::error_box(
                        APP,
                        &format!("Cyberbrain is open, but the choice could not be saved: {e}"),
                    );
                }
                return Some(server);
            }
            Started::Rejected => continue,
        }
    }
}

enum Started {
    Ok(Server),
    /// It did not start and the user has been told; ask again. Giving up is not a variant:
    /// cancelling the folder dialog ends `choose_project` at the `?` and never gets here.
    Rejected,
}

/// One attempt at a folder, including the offer to create a store where there is none.
fn try_start(server_exe: &Path, dir: &Path, job: &sys::JobObject) -> Started {
    match launch::start(server_exe, dir) {
        Ok(server) => {
            job.adopt(&server);
            Started::Ok(server)
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
                    Started::Ok(server)
                }
                Err(e) => {
                    sys::error_box(APP, &format!("Cyberbrain could not start:\n\n{e}"));
                    Started::Rejected
                }
            }
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
