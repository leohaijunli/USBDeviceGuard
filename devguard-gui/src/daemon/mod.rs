//! Live Linux daemon workspace; no mock fallback or direct device/file writes.

mod model;
mod store;
#[cfg(target_os = "linux")]
mod transport;

pub(crate) use model::{Connection, DeviceFilter, DeviceRow, Fault};
pub(crate) use store::{Inspection, Workspace};

#[cfg(test)]
mod tests;
