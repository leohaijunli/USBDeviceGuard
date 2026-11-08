#!/usr/bin/env python3
"""Opt-in WSL/systemd acceptance: auto-apply, hotplug, crash and clean stop.

Windows drives usbipd; a Linux bridge stays alive throughout the test so WSL
cannot idle out between commands. Only the explicit BUSID/serial is used.
The bridge installs this checkout's binaries/units, backing up prior files,
and restores the service's initial active state on exit. No rules, group
membership, policy, firmware, power or USB authorized attributes are changed.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import re
import select
import shutil
import socket
import stat
import subprocess
import sys
import threading
import time
import uuid


SERVICE = "devguardd.service"
CONTROL = "/run/devguard/ctl.sock"
MAX_FRAME = 4 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def run(*command, timeout=30):
    result = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
    require(result.returncode == 0,
            f"{list(command)!r}: exit {result.returncode}: {result.stderr.strip()}")
    return result.stdout


def service_state(unit=SERVICE):
    text = run("systemctl", "show", unit, "--property=" + ",".join([
        "Id", "ActiveState", "SubState", "MainPID", "NRestarts", "Result",
        "ExecMainStatus", "ExecMainStartTimestampMonotonic", "InvocationID",
        "UnitFileState", "Type", "StatusText", "Wants", "Requires",
    ]))
    return dict(line.split("=", 1) for line in text.splitlines() if "=" in line)


def wait_until(predicate, message, seconds=15):
    # A bounded acceptance-test wait for asynchronous service state, not a
    # production polling mechanism. The daemon still uses udev events.
    deadline = time.monotonic() + seconds
    while True:
        value = predicate()
        if value:
            return value
        require(time.monotonic() < deadline, message)
        time.sleep(0.1)


class Rpc:
    def __init__(self):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(15)
        self.socket.connect(CONTROL)
        self.buffer = b""
        self.sequence = None

    def close(self):
        self.socket.close()

    def line(self, timeout=15):
        deadline = time.monotonic() + timeout
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            require(remaining > 0, "timed out waiting for an RPC/event frame")
            require(select.select([self.socket], [], [], remaining)[0], "RPC/event timed out")
            data = self.socket.recv(65536)
            if not data:
                require(not self.buffer, "truncated RPC frame at EOF")
                return None
            self.buffer += data
            require(len(self.buffer) <= MAX_FRAME, "RPC frame exceeds 4 MiB")
        line, self.buffer = self.buffer.split(b"\n", 1)
        return json.loads(line)

    def request(self, op, **parameters):
        self.socket.sendall(json.dumps({"v": 1, "id": 1, "op": op, **parameters}).encode() + b"\n")
        response = self.line()
        require(response and response.get("id") == 1, "missing/miscorrelated response")
        require(response.get("ok"), f"{op} failed: {response.get('error')}")
        return response["data"]

    def event(self, timeout=15):
        event = self.line(timeout)
        require(event is not None and event.get("v") == 1 and "seq" in event, "event stream closed")
        if self.sequence is not None:
            require(event["seq"] == self.sequence + 1, "gap/duplicate in event sequence")
        self.sequence = event["seq"]
        return event


def rpc(op, **parameters):
    client = Rpc()
    try:
        return client.request(op, **parameters)
    finally:
        client.close()


def emit(message):
    print(json.dumps(message, ensure_ascii=True), flush=True)


def completed_cycles(path, name, serial, visited=None):
    """Read only fully asserted cycles; retain interrupted sessions as evidence."""
    if path is None:
        return []
    path = path.resolve()
    visited = set() if visited is None else visited
    require(path not in visited and len(visited) < 16, "cyclic/excessive resume chain")
    visited.add(path)
    rows = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    metadata = next((row["meta"] for row in rows if "meta" in row), None)
    if metadata:
        require(metadata["name"] == name and metadata["serial"] == serial, "resume evidence belongs to another device")
    else:
        # Earlier checkpoints did not have a header. Their explicit applied
        # audit still identifies the registration; the operator supplies the
        # expected serial and setup verifies it against the installed TOML.
        audits = [row.get("data", {}).get("applied_audit", {}) for row in rows]
        require(any(audit.get("name") == name for audit in audits), "unidentified legacy resume evidence")
    previous = metadata.get("resume_from") if metadata else None
    cycles = completed_cycles(Path(previous), name, serial, visited) if previous else []
    for row in rows:
        if row.get("op") == "after_attach" and row.get("ok"):
            data = row["data"]
            require(data["events"] == ["device_removed", "device_added"] and
                    all(data["check"].get(key) == "ok" for key in ("symlink", "access", "modem_manager", "termios")),
                    "resume cycle did not pass the required assertions")
            require(data["cycle"] == len(cycles) + 1, "resume cycles are not a contiguous prefix")
            cycles.append({**data, "source_checkpoint": str(path)})
    return cycles


class LinuxProbe:
    def __init__(self):
        self.initial = service_state()
        self.events = []
        self.observer = None
        self.name = None
        self.serial = None
        self.devnode = None
        self.unit = None
        self.finished = False
        self.report = {"environment": {"kernel": platform.release(), "uid": os.geteuid()}, "cycles": []}

    def setup(self, message):
        require(os.geteuid() == 0, "systemd acceptance bridge must run as root")
        self.name, self.serial = message["name"], message["serial"]
        require(re.fullmatch(r"[a-z0-9-]+", self.name), "invalid registration name")
        import tomllib
        config_file = Path("/etc/devguard/devices.d") / (self.name + ".toml")
        config = tomllib.loads(config_file.read_text())
        require(any(identity.get("serial") == self.serial for identity in config["ids"]),
                "registration does not match the explicitly selected serial")
        require(rpc("policy_get")["policy"]["mode"] == "audit", "only audit mode may be tested")
        require(self.initial["ActiveState"] == "active", "start the installed development service first")
        source = Path(message["bin_dir"])
        repo = Path(__file__).resolve().parents[1]
        backup = Path("/var/lib/devguard/acceptance-backups") / uuid.uuid4().hex
        replacements = [
            (source / "devguardd", Path("/usr/bin/devguardd"), 0o755),
            (source / "devguardctl", Path("/usr/bin/devguardctl"), 0o755),
            (repo / "packaging/systemd/devguardd.service", Path("/etc/systemd/system/devguardd.service"), 0o644),
            (repo / "packaging/systemd/devguard-apply@.service", Path("/etc/systemd/system/devguard-apply@.service"), 0o644),
        ]
        if message.get("skip_install"):
            for src, dest, _mode in replacements:
                with src.open("rb") as source, dest.open("rb") as installed:
                    require(hashlib.file_digest(source, "sha256").digest() ==
                            hashlib.file_digest(installed, "sha256").digest(),
                            f"--skip-install: installed {dest} does not match this checkout/build")
            ready = service_state()
            require(ready["Type"] == "notify" and ready["ActiveState"] == "active",
                    "installed service is not active/notify-ready")
            require("monitoring ready" in ready["StatusText"], "installed daemon has not signaled readiness")
            self.report["initial_ready_state"] = ready
            self.report["installation_skipped"] = True
            self.initial_since = int(time.monotonic() * 1_000_000)
            return {"ready": ready, "installation_skipped": True}
        backup.mkdir(parents=True, mode=0o700)
        # Preserve unfamiliar files; backups are retained for manual rollback.
        for src, dest, mode in replacements:
            require(src.is_file(), f"missing build/unit {src}")
            metadata = dest.lstat()
            require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == 0,
                    f"refusing to replace a non-regular/unowned {dest}")
            if dest.suffix == ".service":
                require("Description=SerialGuard" in dest.read_text(), f"unfamiliar unit {dest}")
            else:
                require("0.1.0" in run(str(dest), "--version"), f"unfamiliar binary {dest}")
            shutil.copy2(dest, backup / dest.name)
        self.report["backup_directory"] = str(backup)
        run("systemctl", "stop", SERVICE)
        for src, dest, mode in replacements:
            run("install", "-m", f"{mode:o}", str(src), str(dest))
        run("systemd-analyze", "verify", "/etc/systemd/system/devguardd.service",
            "/etc/systemd/system/devguard-apply@.service")
        run("systemctl", "daemon-reload")
        run("systemctl", "start", SERVICE, timeout=40)
        ready = service_state()
        require(ready["Type"] == "notify" and ready["ActiveState"] == "active", "service not ready")
        require("monitoring ready" in ready["StatusText"], "daemon did not send READY after initialization")
        require(ready["UnitFileState"] == self.initial["UnitFileState"], "autostart setting changed")
        self.report["initial_ready_state"] = ready
        self.initial_since = int(time.monotonic() * 1_000_000)
        return {"ready": ready, "backup_directory": str(backup)}

    def online(self):
        def enumerated():
            data = rpc("list")
            require(not data["problems"], f"inventory/configuration problems: {data['problems']}")
            devices = [device for device in data["online"] if device.get("serial") == self.serial]
            require(len(devices) <= 1, "selected serial is ambiguous")
            if not devices:
                return None
            require(devices[0].get("managed_by") == self.name, "selected serial is not registered")
            return devices[0]
        # usbipd can return before the kernel binds the CDC driver; settle
        # only drains existing udev work, not future kernel add events.
        device = wait_until(enumerated, "selected serial did not enumerate after attach")
        require(re.fullmatch(r"/dev/tty(?:ACM|USB)[0-9]+", device["devnode"]), "not a managed serial devnode")
        self.devnode = device["devnode"]
        self.unit = "devguard-apply@" + Path(self.devnode).name + ".service"
        return device

    def check_auto_apply(self, since=0):
        self.online()
        def applied():
            state = service_state(self.unit)
            if int(state.get("ExecMainStartTimestampMonotonic") or 0) <= since:
                return None
            if state["ActiveState"] in ("active", "activating", "deactivating"):
                return None
            require(state["Result"] == "success" and state["ExecMainStatus"] == "0",
                    f"automatic apply failed: {state}")
            return state
        state = wait_until(applied, "udev did not run a fresh automatic apply")
        check = rpc("check", name=self.name)
        items = {item["id"].rsplit(".", 1)[-1]: item["severity"] for item in check["overall"]["items"]}
        require(all(items.get(key) == "ok" for key in ("symlink", "access", "modem_manager", "termios")),
                f"automatic apply left drift: {check}")
        entries = rpc("log_query", filter={"event": "applied", "name": self.name, "limit": 20})["query"]["entries"]
        require(entries and entries[-1].get("uid") == 0, "automatic apply has no trusted root operator audit")
        return {"unit": state, "check": items, "applied_audit": entries[-1]}

    def initial_attach(self, _message):
        return self.check_auto_apply(self.initial_since)

    def subscribe(self):
        if self.observer:
            self.observer.close()
        self.observer = Rpc()
        self.observer.request("subscribe_events")

    def prepare_cycle(self, message):
        self.online()
        if self.observer is None:
            self.subscribe()
        self.cycle_start = len(self.events)
        self.previous_apply = int(service_state(self.unit).get("ExecMainStartTimestampMonotonic") or 0)
        return {"devnode": self.devnode, "cycle": message["cycle"]}

    def wait_event(self, kind):
        deadline = time.monotonic() + 15
        while True:
            event = self.observer.event(max(0.1, deadline - time.monotonic()))
            self.events.append(event)
            if event["event"] == kind:
                matches = (event.get("name") == self.name if kind == "device_removed"
                           else event.get("device", {}).get("serial") == self.serial)
                if matches:
                    return event
            require(time.monotonic() < deadline, f"missing {kind} event for the selected device")

    def after_detach(self, _message):
        removed = self.wait_event("device_removed")
        require(not Path(self.devnode).exists(), "devnode survived detach")
        require(not Path("/dev/devguard", self.name).is_symlink(), "udev left the device symlink after detach")
        return {"removed": removed}

    def after_attach(self, message):
        added = self.wait_event("device_added")
        verified = self.check_auto_apply(self.previous_apply)
        # Drain pending frames, including harmless device_updated notices,
        # then assert exactly one target remove and add for this cycle.
        while self.observer.buffer or select.select([self.observer.socket], [], [], 0.2)[0]:
            self.events.append(self.observer.event())
        events = self.events[self.cycle_start:]
        target = [event["event"] for event in events if
                  (event["event"] == "device_removed" and event.get("name") == self.name) or
                  (event["event"] == "device_added" and event.get("device", {}).get("serial") == self.serial)]
        require(target == ["device_removed", "device_added"], f"missing/duplicate target hotplug events: {target}")
        result = {"cycle": message["cycle"], "added": added, **verified}
        self.report["cycles"].append(result)
        return {"cycle": message["cycle"], "events": target, "check": verified["check"]}

    def crash(self, _message):
        self.observer.close()
        self.observer = None
        before = service_state()
        run("systemctl", "kill", "--kill-whom=main", "--signal=KILL", SERVICE)
        def restarted():
            state = service_state()
            if state["ActiveState"] == "active" and state["MainPID"] != before["MainPID"]:
                return state
            return None
        after = wait_until(restarted, "systemd did not recover after SIGKILL", seconds=25)
        require(int(after["NRestarts"]) > int(before["NRestarts"]), "Restart=on-failure was not exercised")
        require(rpc("service_state"), "restarted daemon RPC is unavailable")
        starts = rpc("log_query", filter={"event": "service_started", "limit": 10})["query"]["entries"]
        require(starts[-1]["details"]["pid"] == int(after["MainPID"]), "restart was not audited")
        require("uid" not in starts[-1], "service start impersonated a user")
        result = {"before": before, "after": after, "startup_audit": starts[-1]}
        self.report["crash_recovery"] = result
        return {"old_pid": before["MainPID"], "new_pid": after["MainPID"], "restarts": after["NRestarts"]}

    def shutdown(self, _message):
        self.subscribe()
        before = service_state()
        require(rpc("shutdown", release_blocked=True, note="systemd-acceptance-clean-stop")["accepted"], "shutdown refused")
        event = self.observer.event()
        require(event["event"] == "service_stopping", "missing final stopping event")
        require(self.observer.line() is None, "subscription did not reach EOF")
        self.observer.close()
        self.observer = None
        wait_until(lambda: service_state()["ActiveState"] == "inactive", "clean shutdown did not exit")
        time.sleep(2.5)  # More than RestartSec=1s: catch an unintended restart.
        after = service_state()
        require(after["ActiveState"] == "inactive" and after["NRestarts"] == before["NRestarts"],
                "legal shutdown was restarted")
        require(after["ExecMainStatus"] == "0" and not Path(CONTROL).exists(), "unclean exit/socket cleanup")
        self.stopped_apply = int(service_state(self.unit).get("ExecMainStartTimestampMonotonic") or 0)
        self.report["clean_stop"] = {"state": after, "event": event}
        return self.report["clean_stop"]

    def verify_stopped(self, _message):
        require(Path(self.devnode).exists(), "device did not reappear during stopped-service test")
        def skipped():
            state = service_state(self.unit)
            return state if state["Result"] == "exec-condition" else None
        unit = wait_until(skipped, "hotplug hook was not skipped while daemon was stopped")
        require(int(unit.get("ExecMainStartTimestampMonotonic") or 0) <= self.stopped_apply,
                "hook executed apply while daemon was stopped")
        require(service_state()["ActiveState"] == "inactive", "hotplug resurrected the stopped daemon")
        require(not Path(CONTROL).exists(), "a stopped daemon recreated its socket")
        require(Path("/dev/devguard", self.name).resolve() == Path(self.devnode), "udev symlink lost while daemon stopped")
        self.report["stopped_hotplug"] = {"unit": unit, "daemon": service_state()}
        return {"hook_result": unit["Result"], "daemon": "inactive", "udev_symlink_retained": True}

    def finish(self, _message):
        self.restore()
        self.finished = True
        self.report["events"] = self.events
        self.report["final_service"] = service_state()
        self.report["limits"] = ["usbipd reconnects, not manual physical cable unplug",
                                 "not desktop polkit agent acceptance", "not Arch/Wayland acceptance",
                                 "autosuspend/enforcement remain disabled"]
        return self.report

    def restore(self):
        if self.observer:
            self.observer.close()
            self.observer = None
        if self.initial["ActiveState"] == "active":
            run("systemctl", "start", SERVICE, timeout=40)
            # Deliberate cleanup, not counted as automatic-apply evidence.
            if self.name and self.serial:
                devices = rpc("list")["online"]
                if any(device.get("serial") == self.serial for device in devices):
                    rpc("apply", name=self.name)


def bridge():
    probe = LinuxProbe()
    emit({"ready": True, "initial": probe.initial})
    try:
        for line in sys.stdin:
            message = json.loads(line)
            try:
                require(message.get("op") in {"setup", "initial_attach", "prepare_cycle", "after_detach",
                                               "after_attach", "crash", "shutdown", "verify_stopped", "finish"},
                        "unknown bridge operation")
                method = getattr(probe, message["op"])
                result = method(message)
                emit({"ok": True, "data": result})
            except Exception as error:
                emit({"ok": False, "error": str(error)})
            if message["op"] == "finish":
                break
    finally:
        if not probe.finished:
            probe.restore()


def windows(args):
    require(args.allow_service_changes and args.allow_hardware_replug,
            "pass both --allow-service-changes and --allow-hardware-replug after reviewing the test")
    require(args.cycles in range(1, 21), "cycles must be between 1 and 20")
    require(not args.output.exists(), f"preserve the existing evidence file: {args.output}")
    usbipd = shutil.which("usbipd") or str(Path(os.environ["ProgramFiles"]) / "usbipd-win/usbipd.exe")
    vid, pid = args.vid_pid.upper().split(":")
    expected_id = f"USB\\VID_{vid}&PID_{pid}\\{args.serial}".upper()

    def device_state():
        devices = json.loads(run(usbipd, "state"))["Devices"]
        selected = [device for device in devices if device.get("BusId") == args.busid]
        if not selected:
            return None
        require(len(selected) == 1 and selected[0]["InstanceId"].upper() == expected_id,
                "BUSID now belongs to a different device; no attach/detach was attempted")
        require(selected[0].get("PersistedGuid") and not selected[0].get("IsForced"),
                "device must already be normally shared; do not bind/force automatically")
        return selected[0]

    initial_device = device_state()
    require(initial_device, "selected BUSID is absent")
    previous_cycles = completed_cycles(args.resume_from, args.name, args.serial)
    require(len(previous_cycles) <= args.cycles, "resume already has more cycles than requested")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    transcript = args.output.with_suffix(".events.jsonl").open("x", encoding="utf-8")
    transcript.write(json.dumps({"meta": {"name": args.name, "serial": args.serial,
                                         "busid": args.busid, "target_cycles": args.cycles,
                                         "resume_from": str(args.resume_from.resolve()) if args.resume_from else None}}) + "\n")
    transcript.flush()
    script = run("wsl", "-d", args.wsl, "--exec", "wslpath", "-a", str(Path(__file__).resolve())).strip()
    process = subprocess.Popen(["wsl", "-d", args.wsl, "-u", "root", "--exec", "python3", "-u", script, "--bridge"],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    messages = queue.Queue()

    def reader():
        for line in iter(lambda: process.stdout.readline(MAX_FRAME + 1), ""):
            messages.put(line)
        messages.put(None)

    threading.Thread(target=reader, daemon=True).start()

    def receive():
        line = messages.get(timeout=60)
        require(line is not None, "Linux bridge exited unexpectedly")
        require(len(line) <= MAX_FRAME, "oversized bridge message")
        try:
            return json.loads(line)
        except ValueError:
            raise RuntimeError(f"unexpected bridge output: {line.strip()}") from None

    def command(op, **fields):
        process.stdin.write(json.dumps({"op": op, **fields}) + "\n")
        process.stdin.flush()
        response = receive()
        transcript.write(json.dumps({"captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                     "op": op, **response}) + "\n")
        transcript.flush()
        require(response.get("ok"), f"{op}: {response.get('error')}")
        return response["data"]

    def attach():
        # detach returns before Windows finishes rediscovering the device.
        # Re-check the exact PnP identity; never attach a reused BUSID blindly.
        wait_until(device_state, "selected device did not reappear on Windows")
        deadline = time.monotonic() + 15
        while True:
            device_state()
            result = subprocess.run([usbipd, "attach", "--wsl", "--busid", args.busid],
                                    capture_output=True, text=True, timeout=45)
            if result.returncode == 0:
                break
            require("There is no device with busid" in result.stderr and time.monotonic() < deadline,
                    f"usbipd attach failed: {result.stderr.strip()}")
            time.sleep(0.2)
        run("wsl", "-d", args.wsl, "-u", "root", "--exec", "udevadm", "settle", "--timeout=15")

    try:
        require(receive().get("ready"), "Linux bridge not ready")
        command("setup", name=args.name, serial=args.serial, bin_dir=args.bin_dir, skip_install=args.skip_install)
        # Normalize only the selected device; the bridge keeps WSL alive.
        if initial_device.get("ClientIPAddress"):
            run(usbipd, "detach", "--busid", args.busid)
        attach()
        initial = command("initial_attach")
        print("Initial reconnect: automatic apply verified.", flush=True)
        if previous_cycles:
            print(f"Resuming after {len(previous_cycles)} verified cycles; no repeat installation or completed cycles.", flush=True)
        for cycle in range(len(previous_cycles) + 1, args.cycles + 1):
            command("prepare_cycle", cycle=cycle)
            run(usbipd, "detach", "--busid", args.busid)
            command("after_detach")
            attach()
            result = command("after_attach", cycle=cycle)
            print(f"Cycle {cycle}/{args.cycles}: one remove/add; symlink, access, MM and termios OK.", flush=True)
        print("Crash recovery:", command("crash"), flush=True)
        command("shutdown")
        print("Clean shutdown: exit 0, no restart, socket removed.", flush=True)
        run(usbipd, "detach", "--busid", args.busid)
        attach()
        print("Replug while stopped:", command("verify_stopped"), flush=True)
        report = command("finish")
        report["previously_verified_cycles"] = previous_cycles
        report["total_verified_cycles"] = len(previous_cycles) + len(report["cycles"])
        if previous_cycles:
            report["limits"].append("hotplug acceptance spans interrupted sessions; sequence continuity asserted per subscription")
        report.update(captured_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                      device={"busid": args.busid, "vid_pid": args.vid_pid, "serial": args.serial, "name": args.name},
                      initial_auto_apply=initial, final_windows_device=device_state())
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("x", encoding="utf-8") as output:
            json.dump(report, output, ensure_ascii=False, indent=2)
            output.write("\n")
        print(f"Evidence saved: {args.output}. Original service/autostart state restored; selected USB remains attached.")
    finally:
        transcript.close()
        process.stdin.close()  # EOF also asks the bridge to restore the service.
        try:
            process.wait(timeout=45)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--wsl", default="Ubuntu-26.04")
    parser.add_argument("--bin-dir", help="Linux build cache containing devguardd/devguardctl")
    parser.add_argument("--busid")
    parser.add_argument("--vid-pid")
    parser.add_argument("--serial")
    parser.add_argument("--name")
    parser.add_argument("--cycles", type=int, default=10)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--allow-service-changes", action="store_true")
    parser.add_argument("--allow-hardware-replug", action="store_true")
    parser.add_argument("--skip-install", action="store_true", help="resume against identical, already-installed binaries/units")
    parser.add_argument("--resume-from", type=Path, help="checkpoint JSONL from an interrupted session; skip fully verified cycles")
    args = parser.parse_args()
    if args.bridge:
        require(platform.system() == "Linux", "bridge is Linux-only")
        bridge()
    else:
        require(platform.system() == "Windows", "use the Windows driver for usbipd acceptance")
        require(all([args.bin_dir, args.busid, args.vid_pid, args.serial, args.name, args.output]),
                "--bin-dir, --busid, --vid-pid, --serial, --name and --output are required")
        windows(args)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"systemd acceptance FAILED: {error}", file=sys.stderr)
        sys.exit(1)
