#!/usr/bin/env python3
"""Non-GUI tests for the harness (no app is launched): `python3 -m unittest scripts/test_measure.py`
from spikes/ui-shell, or `python3 scripts/test_measure.py`."""
import io, os, sys, unittest
from contextlib import redirect_stdout
from types import SimpleNamespace

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure  # noqa: E402


def report(paint, input_ready, t0=1000.0):
    marks = {"jsStart": t0 + 250, "mounted": t0 + 320, "firstInputHandled": t0 + input_ready,
             "paletteReady": t0 + 400, "allWindowsReady": t0 + 420}
    if paint is not None:
        marks["firstPaint"] = t0 + paint
    return {"mainEpochMs": t0 + 10, "marks": marks}


class Timing(unittest.TestCase):
    """Review 2 #3: TTI is reached once the window has painted AND handled input."""

    def test_tti_is_the_later_of_paint_and_input(self):
        row = measure.timing(report(paint=357.5, input_ready=336.0), 1000.0)
        self.assertEqual(row["tti"], 357.5)
        self.assertEqual(row["input_ready"], 336.0)
        self.assertGreaterEqual(row["tti"], row["first_paint"])
        row = measure.timing(report(paint=300.0, input_ready=340.0), 1000.0)
        self.assertEqual(row["tti"], 340.0)

    def test_a_report_without_paint_fails(self):
        with self.assertRaises(measure.HarnessError):
            measure.timing(report(paint=None, input_ready=336.0), 1000.0)


class ColdMatrix(unittest.TestCase):
    """Review 2 #6: one failing configuration must not stop the others."""

    def test_continues_past_a_failed_configuration_and_reports_it(self):
        calls = {"n": 0}

        def run_once():
            calls["n"] += 1
            if measure.CONFIG["ATRIUM_WINDOWS"] == "3" and measure.CONFIG["ATRIUM_OPACITY"] == "whole":
                raise measure.HarnessError("app failed at stage 'gate'")
            return measure.timing(report(paint=350.0, input_ready=330.0), 1000.0)

        out = io.StringIO()
        with redirect_stdout(out), self.assertRaises(measure.HarnessError) as err:
            measure.coldmatrix(SimpleNamespace(runs=3, bin="bin"), run_once=run_once, pause=0)
        text = out.getvalue()
        summaries = [l for l in text.splitlines() if l.startswith("SUMMARY ")]
        self.assertEqual(len(summaries), 6, text)
        self.assertEqual(sum('"failed"' in l for l in summaries), 1)
        self.assertIn("3/whole", str(err.exception))
        self.assertEqual(calls["n"], 5 * 3 + 1, "the failed configuration is not retried; the others all run")

    def test_warmup_launches_are_not_recorded(self):
        calls = {"n": 0}

        def run_once():
            calls["n"] += 1
            return measure.timing(report(paint=350.0, input_ready=330.0), 1000.0)

        out = io.StringIO()
        with redirect_stdout(out):
            measure.coldmatrix(SimpleNamespace(runs=2, bin="bin", warmup=1), run_once=run_once, pause=0)
        summaries = [l for l in out.getvalue().splitlines() if l.startswith("SUMMARY ")]
        self.assertEqual(calls["n"], 6 * 2 + 1)
        self.assertTrue(all('"runs": 2' in l for l in summaries), summaries)

    def test_cold_modes_default_to_ten_launches(self):
        """Review 2 #7: the documented command must reproduce ten launches per configuration."""
        self.assertEqual(measure.default_runs("coldmatrix"), 10)
        self.assertEqual(measure.default_runs("cold"), 10)
        self.assertEqual(measure.default_runs("frames"), 3)


def window(alpha, mode, chrome, content, native, vibrancy):
    return {"theme": "atrium-calm-dark", "settings": {"alpha": alpha, "mode": mode}, "chromeAlpha": chrome, "contentAlpha": content,
            "nativeAlpha": native, "vibrancy": vibrancy, "error": "", "accessibilityOpaque": False, "rafAt": 1}


class OpaqueLinuxMode(unittest.TestCase):
    """D-037: without translucency the request is kept, but every window must apply alpha 1."""

    def setUp(self):
        self.saved = dict(measure.CONFIG)
        measure.CONFIG["ATRIUM_WINDOWS"], measure.CONFIG["ATRIUM_OPACITY"] = "1", "background"

    def tearDown(self):
        measure.CONFIG.update(self.saved)

    def check(self, w, translucency):
        with redirect_stdout(io.StringIO()):
            measure.validate_report({"configuration": {"perWindow": {"main": w}}}, translucency=translucency)

    def test_requested_background_applied_opaque_passes_without_translucency(self):
        self.check(window(measure.ALPHA, "background", 1, 1, 1, False), translucency=False)

    def test_a_translucent_window_fails_without_translucency(self):
        with self.assertRaises(measure.HarnessError):
            self.check(window(measure.ALPHA, "background", measure.ALPHA, measure.CONTENT_ALPHA, 1, True), translucency=False)

    def test_macos_rules_are_unchanged(self):
        self.check(window(measure.ALPHA, "background", measure.ALPHA, measure.CONTENT_ALPHA, 1, True), translucency=True)
        with self.assertRaises(measure.HarnessError):
            self.check(window(measure.ALPHA, "background", 1, 1, 1, False), translucency=True)


class DisplayCheck(unittest.TestCase):
    def test_every_run_must_reach_97_percent_of_the_refresh_rate(self):
        runs = [{"scroll": {"fps": 143.5, "p99Ms": 9.0}}, {"scroll": {"fps": 139.0, "p99Ms": 15.0}}]
        c = measure.display_check(runs, "scroll", 144)
        self.assertFalse(c["pass"])  # 139.0 < 139.68
        self.assertTrue(c["p99_over_2_intervals"])  # 15.0 > 13.9
        c = measure.display_check(runs[:1], "scroll", 144)
        self.assertTrue(c["pass"])
        self.assertFalse(c["p99_over_2_intervals"])


if __name__ == "__main__":
    unittest.main()
