//! App-owned native window/tray lifecycle. NSH remains an embedded public AppView.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use nsh_platform_windows::windows::{
    core::{w, Error, Result},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        System::{
            LibraryLoader::GetModuleHandleW,
            Ole::{OleInitialize, OleUninitialize},
        },
        UI::{
            HiDpi::{
                AdjustWindowRectExForDpi, GetDpiForWindow, SetProcessDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            Input::KeyboardAndMouse::SetFocus,
            Shell::{
                SetCurrentProcessExplicitAppUserModelID, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE,
                NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NIN_BALLOONUSERCLICK,
                NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
            },
            WindowsAndMessaging::*,
        },
    },
};
use nsh_platform_windows::{App, AppOptions, AppView, Root};

const TRAY_MESSAGE: u32 = WM_APP + 1;
const SHOW_COMMAND: usize = 101;
const EXIT_COMMAND: usize = 102;

/// The window opens at this size in logical points, whatever the monitor DPI.
const DEFAULT_WIDTH: i32 = 1120;
const DEFAULT_HEIGHT: i32 = 760;
/// Below this the fixed shell rows would squeeze the pages; scrollbars cover
/// the rest, so the window never shrinks past a usable page.
const MIN_WIDTH: i32 = 900;
const MIN_HEIGHT: i32 = 700;

struct Host {
    view: RefCell<Option<Rc<AppView>>>,
    icon: Cell<NOTIFYICONDATAW>,
    tray_active: Cell<bool>,
    taskbar_created: u32,
    notifications: RefCell<Option<crate::desktop_notifications::TrayNotifications>>,
}

impl Host {
    fn restore(&self, window: HWND) {
        // SAFETY: window belongs to this host; native visibility/focus only.
        unsafe {
            let _ = ShowWindow(window, SW_RESTORE);
            let _ = SetForegroundWindow(window);
        }
    }

    fn remove_tray(&self) {
        if self.tray_active.replace(false) {
            // SAFETY: the icon record is initialized and copied for this call.
            let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &self.icon.get()) };
        }
    }

    fn add_tray(&self) -> Result<()> {
        let icon = self.icon.get();
        // SAFETY: initialized NOTIFYICONDATAW with a live host window/icon.
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &icon) }.as_bool() {
            return Err(Error::from_thread());
        }
        self.tray_active.set(true);
        // SAFETY: version was set to NOTIFYICON_VERSION_4 during initialization.
        if !unsafe { Shell_NotifyIconW(NIM_SETVERSION, &icon) }.as_bool() {
            self.remove_tray();
            return Err(Error::from_thread());
        }
        Ok(())
    }

    fn resize(&self, window: HWND) {
        let child = self.view.borrow().as_ref().map(|view| view.hwnd());
        if let Some(child) = child {
            let mut bounds = RECT::default();
            // SAFETY: both windows belong to this UI thread; bounds is writable.
            unsafe {
                if GetClientRect(window, &mut bounds).is_ok() {
                    let _ = MoveWindow(child, 0, 0, bounds.right, bounds.bottom, true);
                }
            }
        }
    }

    fn menu(&self, window: HWND) -> Result<()> {
        // SAFETY: native popup menu handles/strings, released after selection.
        unsafe {
            let menu = CreatePopupMenu()?;
            let result = (|| {
                AppendMenuW(menu, MF_STRING, SHOW_COMMAND, w!("Show"))?;
                AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
                AppendMenuW(menu, MF_STRING, EXIT_COMMAND, w!("Exit"))?;
                let mut point = POINT::default();
                GetCursorPos(&mut point)?;
                let _ = SetForegroundWindow(window);
                let command = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    point.x,
                    point.y,
                    None,
                    window,
                    None,
                )
                .0 as usize;
                match command {
                    SHOW_COMMAND => self.restore(window),
                    EXIT_COMMAND => {
                        DestroyWindow(window)?;
                    }
                    _ => {}
                }
                let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
                Ok(())
            })();
            let _ = DestroyMenu(menu);
            result
        }
    }
}

/// The window's DPI (96 at 100%).
fn dpi_of(window: HWND) -> u32 {
    // SAFETY: a plain call; 0 for a window that is gone.
    let dpi = unsafe { GetDpiForWindow(window) };
    if dpi == 0 {
        96
    } else {
        dpi
    }
}

/// `points` logical points in this window's device pixels.
fn scaled(points: i32, dpi: u32) -> i32 {
    (points as f32 * dpi as f32 / 96.0).round() as i32
}

/// The outer size of a window whose client is `width` by `height` device pixels.
fn frame_size(width: i32, height: i32, dpi: u32) -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    // SAFETY: a local rectangle; the style matches the window's.
    unsafe {
        let _ = AdjustWindowRectExForDpi(
            &mut rect,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
            dpi,
        );
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// The primary monitor's work area in device pixels.
fn work_area() -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: a local rectangle the system fills in.
    unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&mut rect as *mut RECT).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    Some(rect)
}

/// Sizes the host as `DEFAULT_WIDTH` by `DEFAULT_HEIGHT` logical points and
/// centers it, so a 150% monitor gets a window as roomy as a 100% one.
fn fit_to_work_area(window: HWND) {
    let dpi = dpi_of(window);
    let (mut width, mut height) =
        frame_size(scaled(DEFAULT_WIDTH, dpi), scaled(DEFAULT_HEIGHT, dpi), dpi);
    let (mut x, mut y) = (CW_USEDEFAULT, CW_USEDEFAULT);
    if let Some(work) = work_area() {
        let (work_width, work_height) = (work.right - work.left, work.bottom - work.top);
        width = width.min(work_width);
        height = height.min(work_height);
        x = work.left + (work_width - width) / 2;
        y = work.top + (work_height - height) / 2;
    }
    // SAFETY: the window is live on this thread; a plain placement call.
    unsafe {
        let _ = SetWindowPos(
            window,
            None,
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: Windows supplies CREATESTRUCTW for WM_NCCREATE; Host lives until teardown.
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    // SAFETY: userdata is either null or the Host supplied by run_on_ui_thread.
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Host;
    if !pointer.is_null() {
        let host = unsafe { &*pointer };
        if host.taskbar_created != 0 && message == host.taskbar_created {
            host.tray_active.set(false);
            if let Err(error) = host.add_tray() {
                host.restore(window); // Never leave an inaccessible hidden window.
                nsh_platform_windows::fatal_error(&format!(
                    "Could not restore the tray icon. The window will stay visible: {error}"
                ));
            }
            return LRESULT(0);
        }
        match message {
            WM_GETMINMAXINFO => {
                // SAFETY: Windows passes a MINMAXINFO for this message.
                let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                let dpi = dpi_of(window);
                let (mut width, mut height) =
                    frame_size(scaled(MIN_WIDTH, dpi), scaled(MIN_HEIGHT, dpi), dpi);
                if let Some(work) = work_area() {
                    // A smaller screen still gets a window that fits it.
                    width = width.min(work.right - work.left);
                    height = height.min(work.bottom - work.top);
                }
                info.ptMinTrackSize = POINT {
                    x: width,
                    y: height,
                };
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                // SAFETY: Windows passes the suggested rectangle for the new DPI.
                let suggested = unsafe { &*(lparam.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        window,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                host.resize(window);
                return LRESULT(0);
            }
            WM_CLOSE => {
                // X/Alt+F4 hide, never destroy the engine or background watcher.
                if host.tray_active.get() {
                    let _ = unsafe { ShowWindow(window, SW_HIDE) };
                } else {
                    host.restore(window);
                }
                return LRESULT(0);
            }
            WM_SIZE => {
                host.resize(window);
                if let Some(notifications) = host.notifications.borrow().as_ref() {
                    notifications.visible(wparam.0 as u32 != SIZE_MINIMIZED);
                }
                return LRESULT(0);
            }
            WM_SHOWWINDOW => {
                if let Some(notifications) = host.notifications.borrow().as_ref() {
                    notifications.visible(wparam.0 != 0);
                }
            }
            crate::desktop_notifications::NOTICE_MESSAGE => {
                if let Some(notifications) = host.notifications.borrow_mut().as_mut() {
                    notifications.show_pending(host.icon.get());
                }
                return LRESULT(0);
            }
            WM_SETFOCUS => {
                let child = host.view.borrow().as_ref().map(|view| view.hwnd());
                if let Some(child) = child {
                    let _ = unsafe { SetFocus(Some(child)) };
                }
                return LRESULT(0);
            }
            TRAY_MESSAGE => {
                match (lparam.0 as u32) & 0xffff {
                    WM_CONTEXTMENU => {
                        if let Err(error) = host.menu(window) {
                            eprintln!("devguard: tray menu failed: {error}");
                        }
                    }
                    NIN_SELECT => host.restore(window),
                    value if value == NIN_SELECT | 1 => host.restore(window), // NIN_KEYSELECT
                    NIN_BALLOONUSERCLICK => {
                        if let Some(notifications) = host.notifications.borrow().as_ref() {
                            notifications.open_latest();
                        }
                        host.restore(window);
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_DESTROY => {
                host.notifications.borrow_mut().take();
                host.remove_tray();
                unsafe { PostQuitMessage(0) };
                return LRESULT(0);
            }
            _ => {}
        }
    }
    // SAFETY: delegate messages not owned by the application to Windows.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn run_on_ui_thread() -> Result<()> {
    // SAFETY: initialize the app's UI apartment and physical window DPI boundary.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        OleInitialize(None)?;
        SetCurrentProcessExplicitAppUserModelID(w!("dev.devguard.spike"))?;
    }
    let mut host = Box::new(Host {
        view: RefCell::new(None),
        icon: Cell::new(NOTIFYICONDATAW::default()),
        tray_active: Cell::new(false),
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        notifications: RefCell::new(None),
    });
    let mut window = HWND::default();
    let result = (|| -> Result<()> {
        // SAFETY: class procedure and host userdata remain valid on this UI thread.
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None)?.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance,
                lpszClassName: w!("DevguardDesktopHost"),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hIcon: LoadIconW(None, IDI_APPLICATION)?,
                ..WNDCLASSW::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(Error::from_thread());
            }
            window = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class.lpszClassName,
                w!("devguard - UI validation"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1120,
                760,
                None,
                None,
                Some(instance),
                Some((&mut *host as *mut Host).cast()),
            )?;
            fit_to_work_area(window);
            let mut icon = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: window,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
                uCallbackMessage: TRAY_MESSAGE,
                hIcon: class.hIcon,
                ..NOTIFYICONDATAW::default()
            };
            let tip: Vec<u16> = "devguard - USB serial management".encode_utf16().collect();
            icon.szTip[..tip.len()].copy_from_slice(&tip);
            icon.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            host.icon.set(icon);
            host.add_tray()?; // Establish the recovery/exit path before showing the window.
        }
        let view = AppView::new(
            Some(window),
            AppOptions {
                title: "devguard - UI validation".into(),
                app_id: "dev.devguard.spike".into(),
                size: (1120, 760),
                ..AppOptions::default()
            },
        )?;
        *host.notifications.borrow_mut() =
            Some(crate::desktop_notifications::TrayNotifications::new(window));
        view.load(App::Rust(Root::of::<crate::App>()));
        *host.view.borrow_mut() = Some(view);
        host.resize(window);
        // SAFETY: show the initialized host, then dispatch its UI messages.
        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            let mut message = MSG::default();
            loop {
                match GetMessageW(&mut message, None, 0, 0).0 {
                    -1 => return Err(Error::from_thread()),
                    0 => break,
                    _ => {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            }
        }
        Ok(())
    })();
    if let Some(view) = host.view.borrow_mut().take() {
        view.stop();
        drop(view);
    }
    host.remove_tray();
    // SAFETY: tear down only this host, before its userdata and COM apartment go away.
    unsafe {
        if IsWindow(Some(window)).as_bool() {
            let _ = DestroyWindow(window);
        }
        OleUninitialize();
    }
    drop(host);
    result
}

pub fn run() -> std::process::ExitCode {
    // Construct all Rc-backed NSH objects on a dedicated thread; driver resources
    // are dropped before main ExitProcess, as required by the public embedding API.
    let ui = std::thread::Builder::new()
        .name("devguard-ui".into())
        .stack_size(8 << 20)
        .spawn(run_on_ui_thread);
    match ui.map(std::thread::JoinHandle::join) {
        Ok(Ok(Ok(()))) => std::process::ExitCode::SUCCESS,
        Ok(Ok(Err(error))) => {
            nsh_platform_windows::fatal_error(&format!(
                "Cannot start devguard desktop/tray: {error}"
            ));
            std::process::ExitCode::FAILURE
        }
        _ => std::process::ExitCode::FAILURE,
    }
}
