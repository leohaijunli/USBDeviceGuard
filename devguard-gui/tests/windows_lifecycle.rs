#![cfg(target_os = "windows")]

//! Observe the native window interface, not a private lifecycle implementation.

use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command, Stdio};

use nsh_platform_windows::windows::{
    core::BOOL,
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, WPARAM},
        System::Threading::WaitForInputIdle,
        UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindow, IsWindowVisible, SendMessageW, WM_CLOSE},
    },
};

struct OwnedApp(Child);

impl Drop for OwnedApp {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

struct Search { pid: u32, window: HWND }

unsafe extern "system" fn find_window(window: HWND, parameter: LPARAM) -> BOOL {
    // SAFETY: EnumWindows receives the live Search supplied below.
    let search = unsafe { &mut *(parameter.0 as *mut Search) };
    let mut pid = 0;
    // SAFETY: the enumerated window is valid and pid is a writable output.
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    // Only the visible user-facing window, not a hidden IME/helper window.
    if pid == search.pid && unsafe { IsWindowVisible(window) }.as_bool() { search.window = window; }
    BOOL(1)
}

#[test]
fn window_close_hides_the_gui_instead_of_destroying_it() {
    let mut app = OwnedApp(Command::new(env!("CARGO_BIN_EXE_devguard-gui"))
        .env("DEVGUARD_GUI_NO_HW_SCAN", "1")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    // SAFETY: the process handle remains owned by app for this wait.
    assert_eq!(unsafe { WaitForInputIdle(HANDLE(app.0.as_raw_handle()), 15000) }, 0);
    let mut search = Search { pid: app.0.id(), window: HWND::default() };
    // SAFETY: Search remains on this stack throughout synchronous enumeration.
    unsafe { EnumWindows(Some(find_window), LPARAM((&mut search as *mut Search) as isize)).unwrap() };
    assert!(!search.window.is_invalid());
    // SAFETY: exercise the same native close request as the window X/Alt+F4.
    unsafe { SendMessageW(search.window, WM_CLOSE, Some(WPARAM(0)), Some(LPARAM(0))) };
    assert!(unsafe { IsWindow(Some(search.window)) }.as_bool(), "X destroyed the window");
    assert!(!unsafe { IsWindowVisible(search.window) }.as_bool(), "X did not hide the window");
    assert!(app.0.try_wait().unwrap().is_none(), "X ended the GUI process");
}
