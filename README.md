# SerialGuard

> **Naming note (2026-11-08):** the product will ship as **SerialGuard** — crates/GitHub/AUR name `serialguard` (see [`docs/release.md`](docs/release.md)). Crate names stay `devguard-*` until the phase-7 packaging rename.

USB serial device management for PX4 flight controllers, telemetry radios and FTDI/CP210x/CH340 adapters — with a policy daemon, an event-driven desktop UI and a pausable enforce mode.

This app manages and diagnoses USB serial devices; it does not flash or upgrade device firmware.

**Status: phase 4 in progress — experimental, audit-only; not a USB security boundary.** Linux configuration, termios apply/check/run and initial systemd integration are implemented. Enforcement and pause/resume remain disabled. The Windows app remains a read-only UI validation build.

## What works today

- `devguard-gui` — one Rust/NSH codebase that builds for Linux (GTK 4) and Windows. The current build opens with a read-only **Devices** tab listing USB serial ports; on Windows it refreshes from WMI device-change events (no polling, no port access). The **Records** and **Validation guide** tabs exercise forms, search, virtualized lists, dialogs and a pause-countdown preview. All UI text is English.
- `devguard-core` — shared configuration, validation, JSON-lines protocol, journal types and a bounded Unix-socket client.
- `devguardd` / `devguardctl` — Linux audit/configuration RPC, udev event monitoring, verified termios apply/check and a `run` command. Experimental [systemd units](docs/systemd.md) provide readiness notification and hotplug apply hooks. Three WSL reconnect cycles have passed; the full hardware/lifecycle gate remains pending.

## What is deliberately not here yet

Enforce, pause/resume, sysfs power/authorization writes, complete polkit/service-control integration, restart reconciliation, production GUI integration and distribution packages remain pending. `--root` keeps configuration/rules in a private tree and never reloads host udev, but explicit `apply` can open the tty and set its line discipline. The Windows Devices tab never opens a serial port or writes device settings. The Linux GUI has not yet been connected to this daemon. WSL reconnection tests do not substitute for manual physical unplug or Arch/Wayland acceptance.

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

## Workspace

```text
devguard-core/    Shared schema, validation, protocol (initial draft, tested)
devguardd/        Decision core + Linux audit/configuration shell
devguardctl/      Unix-socket CLI
devguard-gui/     NSH Rust app: Devices / Records / Validation guide
docs/             Requirements, design, progress, hardware evidence
packaging/        systemd / polkit / Arch packaging (added per phase)
scripts/          verify-nsh.ps1, verify-nsh-linux.sh, USB inspection
```

## Building the GUI

Requires the [NSH CLI 0.4.0](https://nsh.dev/docs/installation.md) and a Rust toolchain. Linux additionally needs GTK 4.14+ development files; Windows needs no extra packages.

```sh
cargo run -p devguard-gui        # desktop window on this machine
cargo test -p devguard-core      # schema/protocol tests
cargo test -p devguard-gui       # headless interaction tests (no device scan)
```

Set `DEVGUARD_GUI_NO_HW_SCAN=1` to skip the Windows device watcher (tests do this automatically).

## Phase gates

0 NSH spike → 1 core/config → 2 daemon+udev → 3 termios → 4 events/polkit/systemd → 5 policy/pause/service control → 6 production GUI → 7 packaging. Current work is phase 4; later-phase hardware operations stay disabled until their acceptance gates pass. Target Linux/Wayland validation remains pending as recorded in [`docs/progress.md`](docs/progress.md).

