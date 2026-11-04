#!/usr/bin/env python3
"""Tests for the pure parsers in linuxproc.py (no process is launched): `python3 spikes/test_linuxproc.py`."""
import os, sys, unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import linuxproc  # noqa: E402


class Parsers(unittest.TestCase):
    def test_stat_with_spaces_and_parens_in_comm(self):
        tail = " ".join(["S", "4242"] + ["0"] * 9 + ["300", "200"] + ["0"] * 30)
        ppid, cpu = linuxproc.parse_stat(f"123 (Web (Content) x) {tail}")
        self.assertEqual(ppid, 4242)
        self.assertAlmostEqual(cpu, 500 / linuxproc.CLK_TCK)

    def test_smaps_rollup(self):
        text = "55d0-7ffd ---p 00000000 00:00 0 [rollup]\nRss: 2000 kB\nPss: 1200 kB\nPss_Anon: 900 kB\n" \
               "Shared_Clean: 700 kB\nPrivate_Clean: 100 kB\nPrivate_Dirty: 800 kB\nSwapPss: 5 kB\n"
        self.assertEqual(linuxproc.parse_smaps_rollup(text), {"pss": 1200, "rss": 2000, "uss": 900, "swap_pss": 5})
        with self.assertRaises(ValueError):
            linuxproc.parse_smaps_rollup("Rss: 1 kB\n")

    def test_fdinfo_sums_regions_and_ignores_other_fds(self):
        text = "pos:\t0\nflags:\t02100002\ndrm-driver:\txe\ndrm-pdev:\t0000:00:02.0\ndrm-client-id:\t17\n" \
               "drm-total-system:\t12 MiB\ndrm-resident-system:\t10 MiB\ndrm-total-vram0:\t512 KiB\ndrm-resident-vram0:\t0\n" \
               "drm-engine-render:\t123 ns\ndrm-cycles-rcs:\t2591492638\ndrm-total-cycles-rcs:\t34617162024\n"
        key, mem = linuxproc.parse_fdinfo(text)
        self.assertEqual(key, ("0000:00:02.0", "17"))
        self.assertEqual(mem, {"total": 12 * 1024 + 512, "resident": 10 * 1024})
        self.assertIsNone(linuxproc.parse_fdinfo("pos:\t0\nflags:\t02\nmnt_id:\t15\n"))

    def test_display_state_current_modes(self):
        text = ("(uint32 7, [(('DP-1', 'DEL', 'DELL P2419H', 'X1'), [('1920x1080@60.000', 1920, 1080, 60.0, 1.0, [1.0], "
                "{'is-current': <true>, 'is-preferred': <true>}), ('1920x1080@50.000', 1920, 1080, 50.0, 1.0, [1.0], {})], {}), "
                "(('eDP-1', 'SDC', '0x419d', '0'), [('2560x1440@59.961', 2560, 1440, 59.96, 1.0, [1.0], {}), "
                "('2560x1440@143.991', 2560, 1440, 143.99, 1.0, [1.0], {'is-current': <true>})], {})], [], {})")
        self.assertEqual(linuxproc.parse_display_state(text), ["DP-1 1920x1080@60.000", "eDP-1 2560x1440@143.991"])

    def test_descendants(self):
        rows = {1: (0, 0, 0, ""), 10: (1, 0, 0, ""), 11: (10, 0, 0, ""), 12: (11, 0, 0, ""), 20: (1, 0, 0, "")}
        self.assertEqual(sorted(linuxproc.descendants(rows, 10)), [11, 12])


if __name__ == "__main__":
    unittest.main()
