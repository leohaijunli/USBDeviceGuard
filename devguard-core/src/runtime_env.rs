//! The runtime env file `/run/devguard/<name>.env` (sandbox: `<root>/run/...`).
//!
//! `apply` writes it after termios has been set and verified; `devguardctl
//! run <name> -- <cmd>` reads it back, sets the same line discipline on the
//! device and execs.  The file is the stable contract between the daemon and
//! the CLI, so `run` does not need the daemon to be reachable — but it fails
//! clearly when the env file is missing or its device is gone.

use crate::config::{BaudRate, FlowControl, Parity, SerialConfig};

/// One parsed `<name>.env` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEnv {
    pub device: String,
    pub serial: SerialConfig,
}

impl RuntimeEnv {
    pub fn new(device: impl Into<String>, serial: SerialConfig) -> Self {
        Self { device: device.into(), serial }
    }

    /// Render as `KEY=VALUE` lines.  `BAUD=n/a` keeps the CDC-ACM convention;
    /// `run` then leaves the line speed untouched.
    pub fn render(&self) -> String {
        let baud = match self.serial.baud {
            BaudRate::Rate(rate) => rate.to_string(),
            BaudRate::NotApplicable => "n/a".to_owned(),
        };
        let mut lines = vec![
            format!("DEVICE={}", self.device),
            format!("BAUD={baud}"),
            format!("DATA_BITS={}", self.serial.data_bits),
            format!("PARITY={}", self.serial.parity.serial_key()),
            format!("STOP_BITS={}", self.serial.stop_bits),
            format!("FLOW={}", self.serial.flow),
            format!("RAW={}", u8::from(self.serial.raw)),
        ];
        lines.sort_by_key(|line| line.split('=').next().unwrap_or_default().to_owned());
        lines.push(String::new());
        lines.join("\n")
    }

    /// Parse a rendered file.  Unknown keys are rejected so a mismatched
    /// daemon/CLI pair cannot silently disagree about the line discipline.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut device: Option<String> = None;
        let mut baud: Option<BaudRate> = None;
        let mut data_bits: Option<u8> = None;
        let mut parity: Option<Parity> = None;
        let mut stop_bits: Option<u8> = None;
        let mut flow: Option<FlowControl> = None;
        let mut raw: Option<bool> = None;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("env line is not KEY=VALUE: `{line}`"))?;
            match key {
                "DEVICE" => device = Some(value.to_owned()),
                "BAUD" => {
                    baud = Some(if value.trim() == "n/a" {
                        BaudRate::NotApplicable
                    } else {
                        BaudRate::Rate(value.trim().parse().map_err(|error| {
                            format!("env BAUD is not a number or \"n/a\": {error}")
                        })?)
                    });
                }
                "DATA_BITS" => {
                    data_bits = Some(value.trim().parse().map_err(|error| {
                        format!("env DATA_BITS is not a number: {error}")
                    })?)
                }
                "PARITY" => {
                    parity = Some(match value.trim() {
                        "n" => Parity::N,
                        "e" => Parity::E,
                        "o" => Parity::O,
                        other => return Err(format!("env PARITY `{other}` is not n/e/o")),
                    })
                }
                "STOP_BITS" => {
                    stop_bits = Some(value.trim().parse().map_err(|error| {
                        format!("env STOP_BITS is not a number: {error}")
                    })?)
                }
                "FLOW" => {
                    flow = Some(match value.trim() {
                        "none" => FlowControl::None,
                        "hardware" => FlowControl::Hardware,
                        "software" => FlowControl::Software,
                        other => return Err(format!("env FLOW `{other}` is not none/hardware/software")),
                    })
                }
                "RAW" => {
                    raw = Some(match value.trim() {
                        "1" => true,
                        "0" => false,
                        other => return Err(format!("env RAW `{other}` is not 0 or 1")),
                    })
                }
                other => return Err(format!("unknown env key `{other}`")),
            }
        }
        Ok(Self {
            device: device.ok_or("env is missing DEVICE")?,
            serial: SerialConfig {
                baud: baud.ok_or("env is missing BAUD")?,
                data_bits: data_bits.ok_or("env is missing DATA_BITS")?,
                parity: parity.ok_or("env is missing PARITY")?,
                stop_bits: stop_bits.ok_or("env is missing STOP_BITS")?,
                flow: flow.ok_or("env is missing FLOW")?,
                raw: raw.ok_or("env is missing RAW")?,
            },
        })
    }
}

impl Parity {
    /// Lowercase key used in env files and termios logs.
    pub fn serial_key(self) -> &'static str {
        match self {
            Parity::N => "n",
            Parity::E => "e",
            Parity::O => "o",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_and_parse_round_trip() {
        let env = RuntimeEnv::new(
            "/dev/devguard/px4-main",
            SerialConfig {
                baud: BaudRate::Rate(57600),
                data_bits: 8,
                parity: Parity::E,
                stop_bits: 2,
                flow: FlowControl::Hardware,
                raw: false,
            },
        );
        let parsed = RuntimeEnv::parse(&env.render()).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn cdc_acm_baud_is_na_and_unknown_keys_are_rejected() {
        let serial = SerialConfig { baud: BaudRate::NotApplicable, ..SerialConfig::default() };
        let env = RuntimeEnv::new("/dev/ttyACM0", serial);
        let text = env.render();
        assert!(text.contains("BAUD=n/a"), "{text}");
        let parsed = RuntimeEnv::parse(&text).unwrap();
        assert_eq!(parsed.serial.baud, BaudRate::NotApplicable);
        assert_eq!(parsed.device, "/dev/ttyACM0");

        let broken = "DEVICE=/dev/ttyACM0\nSURPRISE=1\n";
        assert!(RuntimeEnv::parse(broken).is_err());
        assert!(RuntimeEnv::parse("BAUD=115200\n").is_err());
        assert!(RuntimeEnv::parse("DEVICE=/dev/ttyACM0\nBAUD=fast\n").is_err());
    }
}
