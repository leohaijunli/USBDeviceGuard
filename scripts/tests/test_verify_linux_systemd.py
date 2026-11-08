"""Offline checkpoint tests; these never invoke systemd, WSL or usbipd."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "verify-linux-systemd.py"
SPEC = importlib.util.spec_from_file_location("verify_linux_systemd", SCRIPT)
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)


class CheckpointTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def write(self, name, rows):
        path = self.root / name
        path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
        return path

    def header(self, previous=None):
        return {"meta": {"name": "radio-1", "serial": "SERIAL-1", "resume_from": str(previous) if previous else None}}

    def cycle(self, number):
        return {"op": "after_attach", "ok": True, "data": {
            "cycle": number, "events": ["device_removed", "device_added"],
            "check": {key: "ok" for key in ("symlink", "access", "modem_manager", "termios")},
        }}

    def test_no_checkpoint_means_no_completed_cycles(self):
        self.assertEqual(VERIFY.completed_cycles(None, "radio-1", "SERIAL-1"), [])

    def test_partial_cycle_is_not_counted_as_completed(self):
        path = self.write("first.jsonl", [self.header(), self.cycle(1),
                                          {"op": "after_detach", "ok": True, "data": {"cycle": 2}}])
        self.assertEqual([row["cycle"] for row in VERIFY.completed_cycles(path, "radio-1", "SERIAL-1")], [1])

    def test_resume_chain_retains_previously_completed_cycles(self):
        first = self.write("first.jsonl", [self.header(), self.cycle(1)])
        second = self.write("second.jsonl", [self.header(first), self.cycle(2), self.cycle(3)])
        cycles = VERIFY.completed_cycles(second, "radio-1", "SERIAL-1")
        self.assertEqual([row["cycle"] for row in cycles], [1, 2, 3])
        self.assertEqual(cycles[0]["source_checkpoint"], str(first.resolve()))

    def test_other_devices_are_not_accepted_as_resume_evidence(self):
        path = self.write("wrong.jsonl", [self.header(), self.cycle(1)])
        with self.assertRaisesRegex(RuntimeError, "another device"):
            VERIFY.completed_cycles(path, "radio-1", "SERIAL-2")

    def test_missing_or_duplicated_cycles_are_rejected(self):
        for numbers in ([2], [1, 1], [1, 3]):
            with self.subTest(numbers=numbers):
                path = self.write("gap.jsonl", [self.header(), *(self.cycle(number) for number in numbers)])
                with self.assertRaisesRegex(RuntimeError, "contiguous prefix"):
                    VERIFY.completed_cycles(path, "radio-1", "SERIAL-1")

    def test_a_failed_check_or_duplicate_event_never_counts_as_success(self):
        for field in ("check", "events"):
            row = self.cycle(1)
            if field == "check":
                row["data"]["check"]["termios"] = "warning"
            else:
                row["data"]["events"].append("device_added")
            path = self.write("bad.jsonl", [self.header(), row])
            with self.assertRaisesRegex(RuntimeError, "required assertions"):
                VERIFY.completed_cycles(path, "radio-1", "SERIAL-1")

    def test_recursive_resume_links_are_rejected(self):
        path = self.root / "loop.jsonl"
        self.write(path.name, [self.header(path)])
        with self.assertRaisesRegex(RuntimeError, "cyclic"):
            VERIFY.completed_cycles(path, "radio-1", "SERIAL-1")

    def test_legacy_checkpoint_needs_an_identified_registration(self):
        rows = [{"op": "initial_attach", "ok": True, "data": {"applied_audit": {"name": "radio-1"}}}, self.cycle(1)]
        path = self.write("legacy.jsonl", rows)
        self.assertEqual(len(VERIFY.completed_cycles(path, "radio-1", "SERIAL-1")), 1)
        with self.assertRaisesRegex(RuntimeError, "unidentified"):
            VERIFY.completed_cycles(path, "another-radio", "SERIAL-1")


if __name__ == "__main__":
    unittest.main()
