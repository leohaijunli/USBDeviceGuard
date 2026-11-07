//! devguardd's platform-independent core.  The Linux shell (socket, monitor,
//! termios, sysfs, polkit) lives in `linux/` and compiles only on Linux;
//! everything here is unit-tested on any host.

pub mod io;
pub mod rules;

pub mod core;
pub use core::{BlockedDevice, Caller, Daemon, Effect, Handled};

#[cfg(target_os = "linux")]
pub mod linux;
