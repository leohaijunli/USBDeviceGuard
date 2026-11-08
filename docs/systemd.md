# SerialGuard systemd integration (experimental)

The current service and binaries still use the development names `devguardd`
and `devguardctl`. Distribution packaging and the SerialGuard rename are later
release work. These units do **not** enable enforcement or install polkit policy.

## Implemented contracts

- `packaging/systemd/devguardd.service`: `Type=notify`, readiness after the
  initial snapshot/RPC worker are initialized, `Restart=on-failure`, and
  control-group cleanup of the daemon's own monitor process.
- `packaging/systemd/devguard-apply@.service`: the existing udev
  `SYSTEMD_WANTS=devguard-apply@%k.service` invokes verified termios apply for
  the kernel device name. `After=` orders it after a ready daemon.
- The hook has no `Wants=` or `Requires=` dependency on the daemon.
  `ExecCondition` skips it when the service is not running. A device insertion
  must not intentionally start a service the operator stopped.
- Socket recovery holds a private flock for the full daemon lifetime, refuses
  unfamiliar/symlink/hard-linked files, probes non-blockingly, and verifies the
  inode before unlinking an abandoned owned socket. `daemon.lock` deliberately
  stays on disk; the kernel lock is released on process exit.
- Explicit and automatic apply audit records include the trusted caller's
  uid/user. Service-start records describe the daemon, not an invented GUI user.

## Development installation

Requires Linux with systemd/udev, an existing `devguard` group, reviewed device
TOMLs under `/etc/devguard/devices.d`, and reviewed/activated udev rules.
Root installation is a separate administrator action; the GUI never runs as root.

Review existing files and preserve changes before installing. With `BIN_DIR`
pointing to this checkout's Linux build directory:

```sh
cargo build -p devguardd -p devguardctl --locked
sudo install -m 0755 "$BIN_DIR/devguardd" "$BIN_DIR/devguardctl" /usr/bin/
sudo install -m 0644 packaging/systemd/devguardd.service \
  packaging/systemd/devguard-apply@.service /etc/systemd/system/
sudo systemd-analyze verify /etc/systemd/system/devguardd.service \
  /etc/systemd/system/devguard-apply@.service
sudo systemctl daemon-reload
sudo systemctl start devguardd.service
```

Enable autostart only if wanted: `sudo systemctl enable devguardd.service`.
No socket activation is used. This is a development installation, not yet an
Arch or Debian package.

Applying termios opens the tty and may assert DTR. Read-back verifies kernel
settings, not on-wire UART timing or firmware behavior. Sysfs authorized/power
writes remain disabled. Unregistered devices cannot be applied by this hook.

## Current acceptance evidence

- Linux core/daemon/CLI: **153 Rust tests**; Windows: **80 Rust tests**.
- Read-only GUI integration has separate software tests; see
  [GUI connection and verification](gui-daemon.md). It is not native desktop or
  real systemd lifecycle acceptance.
- The Python checkpoint/resume helper: **8 offline tests**, on both hosts.
- WSL notify service startup and **3 complete reconnect cycles** have passed:
  one remove/add pair each; symlink, group/mode, ModemManager flag and termios
  all verified; automatic apply audited as root.
- The fourth cycle was interrupted after removal by an OpenCode server restart.
  It is not counted as completed. Original checkpoint:
  `artifacts/systemd-acceptance-2026-11-08.events.jsonl` (local, not committed).
- The device became unavailable on Windows. The engineer chose to finish the
  software work and defer further hardware tests. **Remaining 7 cycles,
  real SIGKILL recovery, clean shutdown/no restart, and replug while stopped
  are pending.** Unit-file assertions are not runtime proof of these behaviors.

WSL usbipd reconnection is not a manual physical unplug test or Arch/Wayland
acceptance. Polkit policy definitions are implemented; desktop authorization
prompts, startup reconciliation and native GUI/service-control acceptance remain
pending. GUI reconnect behavior has been tested against an isolated daemon,
not yet accepted with a real target desktop/systemd failure cycle.

## Resumable hardware verification

`scripts/verify-linux-systemd.py` is an explicitly opt-in Windows driver with a
Linux bridge. Python 3.11+ is required on both sides. It checks the exact PnP
instance (VID:PID + serial), never binds/forces a device, keeps WSL running,
asserts events and new hook execution, and records every completed step.

After reconnecting the same USB device and reviewing its **current** BUSID:

```powershell
python scripts/verify-linux-systemd.py --wsl Ubuntu-26.04 `
  --bin-dir '<Linux-build-directory>/debug' `
  --busid '<current-BUSID>' --vid-pid 1209:0001 `
  --serial V6X-RUST-001 --name rust-fmuv6x-ttyacm0 `
  --cycles 10 `
  --resume-from artifacts/systemd-acceptance-2026-11-08.events.jsonl `
  --output artifacts/systemd-acceptance-resumed.json `
  --allow-service-changes --allow-hardware-replug
```

The driver skips the already verified cycles 1–3. A fresh session establishes a
new baseline; continuity is checked per subscription, not across daemon restarts.
It then tests failure recovery, orderly stop, and hotplug without service
resurrection. The selected device is left attached and the original active/
autostart state is restored. Deliberate cleanup apply is not counted as an
automatic-apply success.

Normal execution backs up existing binaries/units under
`/var/lib/devguard/acceptance-backups` before installing this build. Use
`--skip-install` **only** if the installed binaries/units are byte-identical to
the build; final software changes may require reinstalling at the next acceptance.
Do not overwrite old output/checkpoint files. This script does not modify group
membership, udev rules, USB authorization, power settings or firmware.

Offline checkpoint tests (no service or hardware access):

```sh
python -B -m unittest discover -s scripts/tests -v
```
