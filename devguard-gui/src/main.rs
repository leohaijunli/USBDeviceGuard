#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> impl std::process::Termination {
    #[cfg(target_os = "windows")]
    {
        env_logger::init();
        devguard_gui::desktop_windows::run()
    }
    #[cfg(not(target_os = "windows"))]
    {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid takes no pointers and has no side effects.
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("devguard-gui must run as a regular user. Remove sudo and try again.");
            return std::process::ExitCode::FAILURE;
        }
    }
    env_logger::init();
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<devguard_gui::App>()),
        platform::AppOptions {
            title: "devguard - UI validation".to_owned(),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            app_id: "dev.devguard.spike".to_owned(),
            size: (1120, 760),
            ..Default::default()
        },
    )
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("Run devguard-gui on Linux, macOS or Windows.");
