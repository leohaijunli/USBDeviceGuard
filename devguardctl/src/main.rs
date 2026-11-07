use clap::Parser;
use devguardctl::Args;

fn main() {
    let args = Args::parse();
    if let Err(error) = run(args) {
        eprintln!("devguardctl: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(unix)]
fn run(args: Args) -> anyhow::Result<()> {
    use devguard_core::client::{Client, ClientError};
    use devguard_core::protocol::ListResponse;
    use devguardctl::Command;

    let mut client = Client::connect(&args.socket)
        .map_err(|error| anyhow::anyhow!("cannot connect to {}: {error}; start devguardd or use --socket", args.socket.display()))?;
    let request = args.command.request()?;
    let response = client.request(request).map_err(|error| match error {
        ClientError::Protocol(error) => anyhow::anyhow!("{}{}", error, error.next_step.as_ref().map(|step| format!("\nNext: {step}")).unwrap_or_default()),
        other => anyhow::anyhow!(other),
    })?;
    if matches!(args.command, Command::Watch) {
        while let Some(event) = client.next_event()? { println!("{}", serde_json::to_string(&event)?); }
    } else if matches!(args.command, Command::List) && !args.json {
        let list: ListResponse = response.into_data()?;
        println!("Policy: {:?}; registered: {}; online USB serial devices: {}", list.policy, list.devices.len(), list.online.len());
        for device in list.online {
            println!("{}  {}:{}  {}  {:?}", device.devnode, device.vid, device.pid, device.managed_by.as_deref().unwrap_or("unregistered"), device.status);
        }
        for problem in list.problems { eprintln!("{:?}: {}", problem.severity, problem.message); }
    } else {
        println!("{}", serde_json::to_string_pretty(&response.data)?);
    }
    Ok(())
}

#[cfg(not(unix))]
fn run(_args: Args) -> anyhow::Result<()> {
    anyhow::bail!("devguardctl requires a Linux/WSL Unix socket; Windows diagnostics are in devguard-gui")
}
