import argparse
import importlib.util
import pathlib
import unittest
from types import SimpleNamespace
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "fixture", pathlib.Path(__file__).with_name("program-status-fixture.py")
)
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class ProtocolFixtureTests(unittest.TestCase):
    def test_tmux_doubles_every_inner_escape_and_keeps_outer_terminator(self):
        plain = fixture.sequence("?")
        wrapped = fixture.sequence("?", tmux=True)
        self.assertEqual(wrapped, b"\x1bPtmux;\x1b\x1b]7501;?\x07\x1b\\")
        self.assertEqual(wrapped[7:-2].replace(b"\x1b\x1b", b"\x1b"), plain)

    def test_baseline_preserves_wire_length_without_reporting_status(self):
        body = "state=working:id=fixture:progress=40"
        for tmux in [False, True]:
            status = fixture.sequence(body, tmux)
            baseline = fixture.sequence(body, tmux, opcode=7502)
            self.assertEqual(len(status), len(baseline))
            self.assertEqual(status.replace(b"7501", b"7502"), baseline)

    def test_unicode_details_are_utf8_base64_not_literal_protocol_fields(self):
        value = "permission: question = \u4e2d\u6587"
        self.assertEqual(fixture.text(value), "cGVybWlzc2lvbjogcXVlc3Rpb24gPSDkuK3mloc=")

    def test_invalid_load_rates_are_rejected(self):
        for value in ["0", "-1", "nan", "inf"]:
            with self.assertRaises(argparse.ArgumentTypeError):
                fixture.positive(value)

    def test_interrupted_load_clears_only_its_fixture_record(self):
        args = SimpleNamespace(seconds=1, rate=3, tmux=False, mode="status")
        emitted = []
        with patch.object(fixture, "write", side_effect=emitted.append), \
                patch.object(fixture.time, "monotonic", side_effect=[0, 0, 0]), \
                patch.object(fixture.time, "sleep", side_effect=KeyboardInterrupt), \
                patch("builtins.print"):
            with self.assertRaises(KeyboardInterrupt):
                fixture.load(args)
        self.assertEqual(emitted[-1], fixture.sequence("state=clear:id=fixture"))

    def test_lifecycle_restores_alternate_screen_on_interruption(self):
        args = SimpleNamespace(seconds=1, tmux=False, mode="status", alternate_screen=True)
        emitted = []
        sleeps = [None] * 7 + [KeyboardInterrupt]
        with patch.object(fixture, "write", side_effect=emitted.append), \
                patch.object(fixture.time, "sleep", side_effect=sleeps), \
                patch("builtins.print"):
            with self.assertRaises(KeyboardInterrupt):
                fixture.lifecycle(args)
        self.assertEqual(emitted[-2:], [b"\x1b[?1049h", b"\x1b[?1049l"])


if __name__ == "__main__":
    unittest.main()
