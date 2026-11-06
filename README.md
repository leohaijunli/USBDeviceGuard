# devguard

USB serial device management for PX4 flight controllers, telemetry radios and FTDI/CP210x/CH340 adapters — with a policy daemon, an event-driven desktop UI and a pausable enforce mode.

**Status: phase 0 (NSH UI validation) — the daemon is not implemented yet, so nothing here manages devices on Linux yet.** The current app is a read-only UI validation build: it can list this machine's USB serial ports (Windows diagnostics) and exercise the interaction patterns the final GUI will use.

## What works today

- `devguard-gui` — one Rust/NSH codebase that builds for Linux (GTK 4) and Windows. The current build opens with a read-only **Devices** tab listing USB serial ports; on Windows it refreshes from WMI device-change events (no polling, no port access). The **Records** and **Validation guide** tabs exercise forms, search, virtualized lists, dialogs and a pause-countdown preview. All UI text is English.
- `devguard-core` — the shared configuration schema, strict validation, JSON-lines protocol and journal types, with 26 passing unit tests.

## What is deliberately not here yet

Per the phased delivery plan in [`docs/requirements.md`](docs/requirements.md), later phases add: the `devguardd` daemon (udev rules, `authorized` control, termios, polkit), the CLI, service control (pause/stop/start), and packaging. The Windows Devices tab is a diagnostic only — it never opens a serial port and never writes device settings. On Linux the tab reports that device management requires `devguardd` rather than pretending to work.

## Development docs

- [Requirements & acceptance baseline](docs/requirements.md) (Chinese, as specified)
- [Phase progress](docs/progress.md)
- [NSH spike & environment evidence](docs/nsh-spike.md)
- [Design decisions & open questions](docs/design.md)
- [UI/UX design notes](docs/ui-design.md)
- [Hardware evidence: V6X Rust CDC on Windows](docs/hardware/windows-2026-11-06.md)

## Workspace

```text
devguard-core/    Shared schema, validation, protocol (initial draft, tested)
devguardd/        Linux privilege daemon (placeholder)
devguardctl/      CLI (placeholder)
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

0 NSH spike → 1 core/config → 2 daemon+udev → 3 termios → 4 events/polkit/systemd → 5 policy/pause/service control → 6 production GUI → 7 packaging. Each phase passes acceptance on the target platform before the next begins; current status is tracked in [`docs/progress.md`](docs/progress.md).

