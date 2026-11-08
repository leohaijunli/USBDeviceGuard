# SerialGuard

> **Naming note (2026-11-08):** the product will ship as **SerialGuard** — crates/GitHub/AUR name `serialguard` (see [`docs/release.md`](docs/release.md)). Crate names stay `devguard-*` until the phase-7 packaging rename.

USB serial device management for PX4 flight controllers, telemetry radios and FTDI/CP210x/CH340 adapters — with a policy daemon, an event-driven desktop UI and a pausable enforce mode.

This app manages and diagnoses USB serial devices; it does not flash or upgrade device firmware.

**Status: experimental, audit-only; not a USB security boundary.** Phase 4 hardware acceptance remains pending. At the engineer's request, the first phase-6 Linux GUI slice now connects to the daemon for viewing and diagnostics. Enforcement and pause/resume remain disabled. The Windows app remains a read-only diagnostic/validation build.

## What works today

- `devguard-gui` — Linux **Devices**, **Logs**, and **Policy and diagnostics** pages read the real daemon protocol: saved/offline registrations, online devices, inspection/registration drafts, journal filters, check and doctor. Subscription-driven updates preserve selection; unexpected disconnects reconnect with backoff, while a known stop retains stale data without reconnecting automatically. See [GUI connection and verification](docs/gui-daemon.md). Windows keeps its WMI read-only inventory and explicitly labelled **Records / Validation guide** tools. UI controls are English.
- `devguard-core` — shared configuration, validation, JSON-lines protocol, journal types and a bounded Unix-socket client.
- `devguardd` / `devguardctl` — Linux audit/configuration RPC, udev event monitoring, verified termios apply/check and a `run` command. Experimental [systemd units](docs/systemd.md) provide readiness notification and hotplug apply hooks. Three WSL reconnect cycles have passed; the full hardware/lifecycle gate remains pending.

## What is deliberately not here yet

Enforce, pause/resume, sysfs power/authorization writes, GUI configuration editing/apply and service control, restart reconciliation, native Linux GUI acceptance and distribution packages remain pending. The GUI bridge exposes **only read queries**, never starts the service, and never falls back to simulated devices. The daemon's `--root` keeps configuration/rules private and never reloads host udev, but its explicit CLI `apply` can set termios. Reading termios through inspection/check may assert DTR. WSL tests do not substitute for physical unplug or Arch/Wayland acceptance.

## Development docs

- [Release plan & naming (SerialGuard)](docs/release.md)
- [Requirements & acceptance baseline](docs/requirements.md) (Chinese, as specified)
- [Phase progress](docs/progress.md)
- [NSH spike & environment evidence](docs/nsh-spike.md)
- [Design decisions & open questions](docs/design.md)
- [UI/UX design notes](docs/ui-design.md)
- [Hardware evidence: V6X Rust CDC on Windows](docs/hardware/windows-2026-11-06.md)
- [Linux daemon / CLI development and safety limits](docs/linux-shell.md)
- [Experimental systemd integration and resumable acceptance](docs/systemd.md)
- [Linux GUI connection, supported actions and verification](docs/gui-daemon.md)

## Workspace

```text
devguard-core/    Shared schema, validation, protocol (initial draft, tested)
devguardd/        Decision core + Linux audit/configuration shell
devguardctl/      Unix-socket CLI
devguard-gui/     NSH Rust app: Linux daemon workbench + Windows diagnostics
docs/             Requirements, design, progress, hardware evidence
packaging/        systemd / polkit / Arch packaging (added per phase)
scripts/          verify-nsh.ps1, verify-nsh-linux.sh, USB inspection
```

## Building the GUI

Requires a Rust toolchain and the pinned NSH crates. Linux desktop builds need GTK 4.14+ development files; Windows needs no extra packages. [NSH CLI 0.4.0](https://nsh.dev/docs/installation.md) is used for `nsh check/test/run` tooling, not required for direct `cargo run`.

```sh
cargo run -p devguard-gui        # desktop window on this machine
cargo test -p devguard-core      # schema/protocol tests
cargo test -p devguard-gui       # headless interaction tests (no device scan)
cargo test -p devguard-gui --no-default-features  # Linux headless tests without GTK
```

Set `DEVGUARD_GUI_NO_HW_SCAN=1` to skip the Windows device watcher (tests do this automatically).
On Linux, build the daemon and GUI from this checkout together; older daemons without `event_sequence` are explicitly rejected. The current WSL lacks GTK development files, so the new Linux window's appearance and native input remain unverified.

## Phase gates

0 NSH spike → 1 core/config → 2 daemon+udev → 3 termios → 4 events/polkit/systemd → 5 policy/pause/service control → 6 production GUI → 7 packaging. Phase-6 read-only integration was brought forward at the user's request; this does not accept phase 4 or enable later hardware operations. See [`docs/progress.md`](docs/progress.md).

