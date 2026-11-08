#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(all(target_os = "linux", feature = "linux-desktop"))]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;

#[cfg(any(
    all(target_os = "linux", feature = "linux-desktop"),
    target_os = "macos",
    target_os = "windows"
))]
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
            use clap::Parser;
            #[derive(Parser)]
            #[command(version, about = "SerialGuard Linux daemon workspace")]
            struct Args {
                /// Alternate daemon socket for an isolated development session.
                #[arg(long, default_value = "/run/devguard/ctl.sock")]
                socket: std::path::PathBuf,
            }
            let args = Args::parse();
            if !args.socket.is_absolute() {
                eprintln!("--socket must be an absolute Linux path");
                return std::process::ExitCode::FAILURE;
            }
            std::env::set_var("DEVGUARD_GUI_SOCKET", args.socket);
        }
        env_logger::init();
        platform::run_app(
            platform::AppSource::Rust(platform::Root::of::<devguard_gui::App>()),
            platform::AppOptions {
                title: if cfg!(target_os = "linux") {
                    "SerialGuard"
                } else {
                    "devguard - UI validation"
                }
                .to_owned(),
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                app_id: "dev.devguard.spike".to_owned(),
                size: (1120, 760),
                ..Default::default()
            },
        )
    }
}

#[cfg(all(target_os = "linux", not(feature = "linux-desktop")))]
fn main() -> std::process::ExitCode {
    eprintln!("This build is for headless testing only. Install GTK development files and build with --features linux-desktop to run the GUI.");
    std::process::ExitCode::FAILURE
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("Run devguard-gui on Linux, macOS or Windows.");
