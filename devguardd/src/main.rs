#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use clap::Parser;
    use devguard_core::Paths;
    use devguardd::linux::{AccessPolicy, LinuxIo, Server};
    use devguardd::linux::monitor::UdevMonitor;

    #[derive(Parser)]
    #[command(version, about = "SerialGuard USB serial daemon (audit-only; termios apply enabled)")]
    struct Args {
        /// Private configuration/rules tree. Explicit apply still sets termios.
        #[arg(long)]
        root: Option<std::path::PathBuf>,
    }
    let args = Args::parse();
    let sandbox = args.root.is_some();
    let (paths, access) = match args.root {
        Some(root) => {
            anyhow::ensure!(root.is_absolute() && root != std::path::Path::new("/"), "--root must be an absolute private directory, not /");
            (Paths::rooted(root), AccessPolicy::sandbox())
        }
        None => (Paths::system(), AccessPolicy::system()?),
    };
    let monitor = UdevMonitor::start().await?;
    let server = Server::bind(paths.clone(), access)?;
    eprintln!("devguardd: listening on {} (audit-only{})", paths.socket().display(), if sandbox { ", sandbox" } else { "" });
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    server.serve_monitored(LinuxIo::new(paths, sandbox), monitor, async move {
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }).await
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("devguardd is Linux-only; run it in Linux/WSL. Windows device diagnostics are in devguard-gui.");
    std::process::exit(1);
}
