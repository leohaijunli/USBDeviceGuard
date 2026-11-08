//! Linux phase-2/3 shell.  Hardware writes beyond termios remain disabled
//! until their gates pass.

mod accounts;
pub mod inventory;
mod io;
mod polkit;
pub mod monitor;
pub mod server;
mod socket;
mod systemd;
pub use devguard_core::termios;

pub use io::LinuxIo;
pub use polkit::Checker;
pub use server::{AccessPolicy, Server};
