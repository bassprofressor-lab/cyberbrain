//! A project's page in a window of its own, hosted by WebView2.
//!
//! Still no second frontend: the window is a frame around the same page `cyberbrain serve`
//! answers with, at the same loopback address the browser would have been sent to. What
//! changes is only where it is drawn. There is no navigation bar, no tab strip and no
//! chrome of our own, because every one of those would be an interface to maintain beside
//! the one the product already has.
//!
//! WebView2 is Microsoft's own COM API and its runtime is part of Windows 10 and 11, so
//! nothing here ships or downloads an engine. When the runtime is absent anyway — an old
//! machine, a stripped image — creation fails, and the caller falls back to the browser
//! rather than leaving somebody with a menu entry that does nothing.
//!
//! One window per project rather than tabs inside one window. Tabs would mean drawing a tab
//! strip, which is the second frontend again; a window per project is what the operating
//! system already gives, with its own taskbar button and its own Alt-Tab entry.

use std::sync::atomic::{AtomicBool, Ordering};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2Controller, ICoreWebView2Environment,
    ICoreWebView2EnvironmentOptions,
};
use webview2_com::{
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler,
};
use windows::Win32::Foundation::{COLORREF, E_POINTER, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::CreateSolidBrush;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{PROCESS_PER_MONITOR_DPI_AWARE, SetProcessDpiAwareness};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect,
    GetWindowLongPtrW, IsIconic, IsWindow, RegisterClassW, SW_RESTORE, SW_SHOW,
    SetForegroundWindow, SetWindowLongPtrW, ShowWindow, WINDOW_EX_STYLE, WM_NCDESTROY, WM_SIZE,
    WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{PCWSTR, w};

/// Everything the window procedure needs, owned by the window and dropped with it.
///
/// Holding the controller here rather than in the caller is deliberate: the resize has to
/// happen inside the window procedure, and two owners of a COM pointer whose lifetimes are
/// a window handle and a `Vec` entry is how a `SetBounds` lands on a released interface.
struct State {
    /// One per pane. A window showing a single project has one; the side-by-side window has
    /// one per project, and they are laid out by `launch::tile` on every resize.
    controllers: Vec<ICoreWebView2Controller>,
}

/// A window that is showing one project. Holds the handle and nothing else.
pub struct ProjectWindow {
    hwnd: HWND,
}

impl ProjectWindow {
    /// Whether the window is still there. A person who closes it has closed a window, not
    /// the project: the server keeps running and the menu entry opens a new one.
    pub fn is_open(&self) -> bool {
        unsafe { IsWindow(Some(self.hwnd)).as_bool() }
    }

    /// Bring it forward. `SW_RESTORE` first, because a minimised window that is merely
    /// raised stays minimised and the click looks like it did nothing.
    pub fn focus(&self) {
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn close(&self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

const CLASS: PCWSTR = w!("CyberbrainProjectWindow");

/// Registered once per process. Registering twice fails, and the second window would then
/// be created with no class at all.
static CLASS_READY: AtomicBool = AtomicBool::new(false);
static PROCESS_READY: AtomicBool = AtomicBool::new(false);

/// Open a window showing `url`, titled `title`.
///
/// Synchronous by the time it returns: WebView2 creates its environment and controller
/// asynchronously, and `wait_for_async_operation` pumps messages until they are there. That
/// nested loop dispatches window messages but never re-enters the launcher's own tick, so
/// menu clicks that arrive meanwhile are simply handled on the next one.
pub fn open(title: &str, url: &str) -> Result<ProjectWindow, String> {
    open_panes(title, &[url.to_string()])
}

/// One window showing several pages at once, tiled.
///
/// Each pane is the page of one project, at that project's own loopback address. Nothing
/// here reaches across them: no pane can talk to another's server, because the browser's
/// own origin rule and the page's `connect-src 'self'` both say so. The window is ours, so
/// putting several of them side by side costs a layout and no new surface at all.
pub fn open_panes(title: &str, urls: &[String]) -> Result<ProjectWindow, String> {
    if urls.is_empty() {
        return Err("there is nothing to show".to_string());
    }
    prepare_process();
    register_class()?;

    // Named, not inlined into the call: a `PCWSTR` built from a temporary reads as if the
    // buffer outlives the call, and the next person to move the expression finds out that
    // it does not.
    let title_w = wide(title);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            760,
            None,
            None,
            GetModuleHandleW(None).ok().map(|h| h.into()),
            None,
        )
    }
    .map_err(|e| format!("the window could not be created: {e}"))?;

    match attach(hwnd, urls) {
        Ok(()) => {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            Ok(ProjectWindow { hwnd })
        }
        Err(why) => {
            // A window with no page in it is worse than no window: it looks like the
            // program hung. Take it away and let the caller offer the browser.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            Err(why)
        }
    }
}

/// Create the WebView2 environment and one controller per pane, and navigate each.
///
/// One environment for all of them: it is the runtime and its profile, and a second would
/// mean a second browser process tree for no gain.
fn attach(hwnd: HWND, urls: &[String]) -> Result<(), String> {
    let environment = create_environment()?;
    let mut controllers = Vec::with_capacity(urls.len());
    for url in urls {
        let controller = create_controller(&environment, hwnd)?;
        unsafe {
            controller
                .SetIsVisible(true)
                .map_err(|e| format!("the page could not be shown: {e}"))?;
            let webview = controller
                .CoreWebView2()
                .map_err(|e| format!("the page could not be reached: {e}"))?;
            let url_w = wide(url);
            webview
                .Navigate(PCWSTR(url_w.as_ptr()))
                .map_err(|e| format!("{url} could not be opened: {e}"))?;
        }
        controllers.push(controller);
    }

    // The window owns the controllers from here. Stored before the first WM_SIZE can
    // arrive, which is why the window is not shown until this has happened.
    let state = Box::new(State { controllers });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
    }
    layout(hwnd);
    Ok(())
}

/// Put every pane where `launch::tile` says, in client coordinates.
fn layout(hwnd: HWND) {
    with_state(hwnd, |state| {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rect);
        }
        let panes = crate::launch::tile(state.controllers.len(), rect.right, rect.bottom);
        for (controller, (x, y, w, h)) in state.controllers.iter().zip(panes) {
            unsafe {
                let _ = controller.SetBounds(RECT {
                    left: x,
                    top: y,
                    right: x + w,
                    bottom: y + h,
                });
            }
        }
    });
}

/// Chromium switches that stop the runtime talking to anything on its own.
///
/// SPEC §12.1 says every path bytes can take off this machine is registered, and §15
/// disqualifies a dependency that performs network I/O of its own. A browser engine is the
/// hardest case there is for both, so the engine is pointed at loopback and told not to do
/// the things it would otherwise do unasked: no reputation lookup on the address it is
/// showing, no component updates, no background fetches. What remains is the page this
/// product already serves, over a socket that never leaves the machine.
const NO_PHONING_HOME: &str = "--disable-background-networking --disable-component-update      --disable-sync --no-service-autorun --disable-features=msSmartScreenProtection";

/// Where WebView2 keeps its profile.
///
/// Named rather than left to the default, and this is not tidiness: the default is a folder
/// beside the executable, and after an install that executable is under `Program Files`,
/// which the user cannot write to. Left alone, every window would fail to open on exactly
/// the machines the installer was made for, and never on a machine where it was tried from
/// a build directory.
fn user_data_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("cyberbrain").join("WebView2");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn create_environment() -> Result<ICoreWebView2Environment, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let data_dir = user_data_dir();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            let options = CoreWebView2EnvironmentOptions::default();
            options.set_additional_browser_arguments(NO_PHONING_HOME.to_string());
            let options = ICoreWebView2EnvironmentOptions::from(options);
            let data = data_dir.as_ref().map(|d| wide(&d.display().to_string()));
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                data.as_ref()
                    .map(|d| PCWSTR(d.as_ptr()))
                    .unwrap_or_else(PCWSTR::null),
                Some(&options),
                &handler,
            )
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |code, environment| {
            code?;
            let _ = tx.send(environment.ok_or_else(|| windows::core::Error::from(E_POINTER)));
            Ok(())
        }),
    )
    .map_err(runtime_missing)?;
    rx.recv()
        .map_err(|_| runtime_missing_text())?
        .map_err(runtime_missing)
}

fn create_controller(
    environment: &ICoreWebView2Environment,
    hwnd: HWND,
) -> Result<ICoreWebView2Controller, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let env = environment.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            env.CreateCoreWebView2Controller(hwnd, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |code, controller| {
            code?;
            let _ = tx.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER)));
            Ok(())
        }),
    )
    .map_err(|e| format!("the page could not be created in the window: {e}"))?;
    rx.recv()
        .map_err(|_| "the page could not be created in the window".to_string())?
        .map_err(|e| format!("the page could not be created in the window: {e}"))
}

/// The one failure worth naming, because it has an answer a person can act on.
fn runtime_missing<E: std::fmt::Display>(e: E) -> String {
    format!("{}\n\n({e})", runtime_missing_text())
}

fn runtime_missing_text() -> String {
    "The WebView2 runtime could not be started. It is part of Windows 11 and of current \
     Windows 10, and can be installed from Microsoft's Evergreen download page."
        .to_string()
}

/// COM and DPI, once. Both are process-wide, both are harmless to ask for twice, and
/// neither is worth failing over: without DPI awareness the page is blurry on a scaled
/// display, and that is a worse window rather than no window.
fn prepare_process() {
    if PROCESS_READY.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        // Apartment-threaded, because that is the apartment a window message loop is.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
    }
}

fn register_class() -> Result<(), String> {
    if CLASS_READY.load(Ordering::SeqCst) {
        return Ok(());
    }
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        lpszClassName: CLASS,
        // The frame exists for a moment before WebView2 has painted anything into it. With
        // no brush that moment is whatever was on screen before; with this one it is the
        // page's own background: #0C0E12, which is `--bg` from `ui/src/index.css`
        // (oklch(0.165 0.008 260)) converted, written here as COLORREF's 0x00BBGGRR. The
        // page picks light or dark from the system, so this is the dark one being wrong for
        // a fraction of a second on a light desktop, rather than white being wrong on every
        // dark one.
        hbrBackground: unsafe { CreateSolidBrush(COLORREF(0x00120E0C)) },
        hInstance: unsafe { GetModuleHandleW(None) }
            .map(|h| h.into())
            .unwrap_or_default(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err("the window class could not be registered".to_string());
    }
    CLASS_READY.store(true, Ordering::SeqCst);
    Ok(())
}

/// Resize the page with the window, and let go of it when the window is gone.
///
/// Deliberately does not post a quit message on destroy, which is what a single-window
/// program would do here: this process is a notification area icon that outlives every
/// window it opens, and closing one project's window must leave the others and the servers
/// behind them alone.
extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout(hwnd);
            LRESULT(0)
        }
        // WM_NCDESTROY is the last message a window ever gets, so it is the one place the
        // state can be dropped knowing nothing else will look for it.
        WM_NCDESTROY => {
            let ptr = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            if ptr != 0 {
                // Safety: the pointer came from `Box::into_raw` in `attach`, this runs
                // once per window, and the slot is cleared above so no later message can
                // reach it again.
                drop(unsafe { Box::from_raw(ptr as *mut State) });
            }
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

/// Run `f` against the window's state, if it has one yet.
///
/// A closure rather than a function returning a reference: a `fn(HWND) -> Option<&State>`
/// has no lifetime to borrow from, so it would hand out a reference the compiler cannot
/// check against the `Box::from_raw` in WM_NCDESTROY.
fn with_state(hwnd: HWND, f: impl FnOnce(&State)) {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if ptr == 0 {
        return;
    }
    // Safety: set once in `attach` from a leaked Box, cleared in WM_NCDESTROY before that
    // Box is dropped, and read only on the thread that owns the window.
    f(unsafe { &*(ptr as *const State) });
}

fn wide(s: &str) -> Vec<u16> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}
