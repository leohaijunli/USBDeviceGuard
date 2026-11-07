//! Phase-2 command line, using the same protocol as the GUI.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use devguard_core::{DeviceConfig, LogFilter, Request};

#[derive(Debug, Parser)]
#[command(version, about = "USB serial device management client")]
pub struct Args {
    #[arg(long, global = true, default_value = "/run/devguard/ctl.sock")]
    pub socket: PathBuf,
    /// Print the response data as JSON, including for list.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    List,
    Show { #[arg(value_parser = valid_name)] name: String },
    Learn { devnode: String },
    Save { file: PathBuf, #[arg(long)] note: Option<String> },
    Delete { #[arg(value_parser = valid_name)] name: String, #[arg(long)] note: Option<String> },
    Check { #[arg(value_parser = valid_name)] name: Option<String> },
    Doctor,
    ServiceState,
    PolicyGet,
    Log {
        #[arg(long)] name: Option<String>,
        #[arg(long)] event: Option<String>,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=5000))] limit: u16,
    },
    /// Stream events until disconnected or interrupted.
    Watch,
    Shutdown { #[arg(long)] keep_blocked: bool, #[arg(long)] note: Option<String> },
}

impl Command {
    pub fn request(&self) -> anyhow::Result<Request> {
        Ok(match self {
            Self::List => Request::List,
            Self::Show { name } => Request::Show { name: name.clone() },
            Self::Learn { devnode } => Request::LearnCandidate { devnode: devnode.clone() },
            Self::Save { file, note } => {
                use std::io::Read;
                let mut text = String::new();
                let size = std::fs::File::open(file)?.take((devguard_core::protocol::MAX_REQUEST_BYTES + 1) as u64).read_to_string(&mut text)?;
                anyhow::ensure!(size <= devguard_core::protocol::MAX_REQUEST_BYTES, "configuration file exceeds 64 KiB");
                Request::SaveDevice { config: DeviceConfig::from_toml(&text)?, note: note.clone() }
            }
            Self::Delete { name, note } => Request::DeleteDevice { name: name.clone(), note: note.clone() },
            Self::Check { name } => Request::Check { name: name.clone() },
            Self::Doctor => Request::Doctor,
            Self::ServiceState => Request::ServiceState,
            Self::PolicyGet => Request::PolicyGet,
            Self::Log { name, event, limit } => Request::LogQuery { filter: LogFilter { name: name.clone(), event: event.clone(), limit: Some(usize::from(*limit)), ..LogFilter::default() } },
            Self::Watch => Request::SubscribeEvents,
            Self::Shutdown { keep_blocked, note } => Request::Shutdown { release_blocked: !keep_blocked, note: note.clone() },
        })
    }
}

fn valid_name(value: &str) -> Result<String, String> {
    devguard_core::util::validate_name(value)?;
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_and_json_flags_are_global() {
        let args = Args::try_parse_from(["devguardctl", "list", "--socket", "/private/ctl.sock", "--json"]).unwrap();
        assert_eq!(args.socket, PathBuf::from("/private/ctl.sock"));
        assert!(args.json);
        assert!(matches!(args.command.request().unwrap(), Request::List));
    }

    #[test]
    fn names_and_query_limits_are_checked() {
        assert!(Args::try_parse_from(["devguardctl", "delete", "../outside"]).is_err());
        assert!(Args::try_parse_from(["devguardctl", "log", "--limit", "5001"]).is_err());
        assert!(Args::try_parse_from(["devguardctl", "log", "--limit", "0"]).is_err());
    }

    #[test]
    fn shutdown_releases_by_default_and_keeps_the_note() {
        let args = Args::try_parse_from(["devguardctl", "shutdown", "--note", "maintenance"]).unwrap();
        assert!(matches!(args.command.request().unwrap(), Request::Shutdown { release_blocked: true, note: Some(note) } if note == "maintenance"));
    }

    #[test]
    fn save_accepts_documented_uppercase_and_existing_lowercase_parity() {
        use devguard_core::Parity;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radio.toml");
        for (value, expected) in [("N", Parity::N), ("n", Parity::N), ("E", Parity::E), ("e", Parity::E), ("O", Parity::O), ("o", Parity::O)] {
            let text = format!(r#"name = "radio-1"
kind = "radio"
[[ids]]
vid = "1a86"
pid = "7523"
serial = "CLI-PARITY"
[serial]
baud = 57600
data_bits = 8
parity = "{value}"
stop_bits = 1
flow = "none"
raw = true
"#);
            std::fs::write(&path, text).unwrap();
            let request = Command::Save { file: path.clone(), note: None }.request().unwrap();
            assert!(matches!(request, Request::SaveDevice { config, .. } if config.serial.parity == expected));
        }
    }
}
