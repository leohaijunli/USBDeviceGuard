#!/usr/bin/env python3
"""Verify the real daemon/CLI in a private tree; never install host rules."""

import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--build", action="store_true", help="build daemon/CLI from this checkout first")
    args = parser.parse_args()
    if platform.system() != "Linux" or os.geteuid() == 0:
        raise SystemExit("Run this verification as a normal user in Linux/WSL.")
    if args.build:
        user_cargo = Path.home() / ".cargo/bin/cargo"
        cargo = str(user_cargo) if user_cargo.is_file() else shutil.which("cargo")
        if not cargo:
            raise SystemExit("Linux cargo is required for --build.")
        subprocess.run([cargo, "build", "--manifest-path", str(Path(__file__).resolve().parents[1] / "Cargo.toml"),
                        "-p", "devguardd", "-p", "devguardctl", "--locked"], check=True)

    cache = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "devguard-verification"
    cache.mkdir(parents=True, exist_ok=True, mode=0o700)
    results = []
    with tempfile.TemporaryDirectory(prefix="cli-", dir=cache) as directory:
        root = Path(directory)
        socket = root / "run/devguard/ctl.sock"
        config = root / "cli-radio.toml"
        config.write_text('''name = "cli-radio"
kind = "radio"
description = "CLI acceptance fixture, not attached hardware"

[[ids]]
vid = "1a86"
pid = "7523"
path = "devguard-cli-acceptance-fixture"
role = "app"

[serial]
baud = 57600
data_bits = 8
parity = "N"
stop_bits = 1
flow = "none"
raw = true

[access]
group = "auto"
mode = "0660"
''', encoding="utf-8")
        daemon = subprocess.Popen(
            [str(args.bin_dir / "devguardd"), "--root", str(root)],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE, text=True,
        )
        try:
            ready = daemon.stderr.readline()
            if "listening on" not in ready:
                raise RuntimeError(f"daemon did not bind its private socket: {ready.strip()}")

            def ctl(*command):
                result = subprocess.run(
                    [str(args.bin_dir / "devguardctl"), "--socket", str(socket), "--json", *command],
                    capture_output=True, text=True, timeout=90,
                )
                if result.returncode:
                    raise RuntimeError(f"CLI {command!r} failed ({result.returncode}): {result.stderr.strip()}")
                results.append({"command": list(command), "exit_code": result.returncode})
                return json.loads(result.stdout)

            initial = ctl("list")
            assert initial["policy"] == "audit" and initial["devices"] == []
            # This is a read-only host snapshot, not simulated attached hardware.
            online_count = len(initial["online"])
            ctl("save", str(config), "--note", "CLI save fixture")
            show = ctl("show", "cli-radio")
            assert show["device"]["serial"]["baud"] == 57600
            assert len(ctl("list")["devices"]) == 1
            log = ctl("log", "--event", "config_changed", "--limit", "10")
            assert log["query"]["entries"][-1]["details"]["note"] == "CLI save fixture"
            rules = root / "etc/udev/rules.d/90-devguard.rules"
            verified = subprocess.run(
                ["/usr/bin/udevadm", "verify", str(rules)],
                check=True, capture_output=True, text=True, timeout=15,
            )
            results.append({"command": ["udevadm", "verify", "private rules"], "exit_code": verified.returncode})
            ctl("doctor")
            ctl("delete", "cli-radio", "--note", "CLI remove fixture")
            assert ctl("list")["devices"] == []
            assert ctl("shutdown", "--note", "CLI session complete")["accepted"] is True
            daemon.wait(timeout=10)
            assert daemon.returncode == 0 and not socket.exists()
        finally:
            if daemon.poll() is None:
                daemon.terminate()
                try:
                    daemon.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.wait(timeout=5)
            daemon.stderr.close()

    evidence = {
        "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(), "uid": os.geteuid(),
        "private_sandbox": True, "opened_serial_ports": False,
        "changed_host_rules": False, "changed_usb_authorization": False,
        "read_only_online_usb_serial_count": online_count,
        "results": results,
        "limitations": ["not udevadm test", "not USB hotplug acceptance", "not desktop polkit acceptance", "not termios acceptance"],
    }
    text = json.dumps(evidence, ensure_ascii=False, indent=2)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("x", encoding="utf-8") as output:
            output.write(text + "\n")
    print(text)


if __name__ == "__main__":
    main()
