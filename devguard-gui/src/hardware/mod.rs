//! Phase 0 read-only diagnostics, separate from the Linux daemon protocol.

#[cfg(any(target_os = "windows", test))]
mod arrivals;
mod store;
#[cfg(test)]
pub(crate) mod test_support;
mod types;
#[cfg(target_os = "windows")]
mod windows;

pub(crate) use store::HardwareStore;
pub(crate) use types::UsbSerialDevice;
