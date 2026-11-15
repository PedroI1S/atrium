"""Tests of the pure parts of linuxmem.py: python3 -m unittest discover -s app/scripts -p 'test_*.py'"""

import unittest

import linuxmem

# Five mappings: a library's clean text, the same library's written .data, the heap, a font
# (another file), a memfd shared buffer, and a GPU mapping.
SMAPS = "\n".join([
    "7f0000000000-7f0000100000 r-xp 00000000 00:1f 100    /usr/lib/libwebkit2gtk-4.1.so.0.19.2",
    "Rss:                1000 kB",
    "Pss:                 400 kB",
    "Pss_Dirty:             0 kB",
    "Private_Clean:       100 kB",
    "Private_Dirty:         0 kB",
    "7f0000100000-7f0000110000 rw-p 00100000 00:1f 100    /usr/lib/libwebkit2gtk-4.1.so.0.19.2",
    "Rss:                  64 kB",
    "Pss:                  64 kB",
    "Pss_Dirty:            60 kB",
    "Private_Clean:         4 kB",
    "Private_Dirty:        60 kB",
    "55d000000000-55d000400000 rw-p 00000000 00:00 0      [heap]",
    "Rss:                4096 kB",
    "Pss:                4096 kB",
    "Pss_Dirty:          4096 kB",
    "Private_Clean:         0 kB",
    "Private_Dirty:      4096 kB",
    "7f0000200000-7f0000300000 r--p 00000000 00:1f 200    /usr/share/fonts/noto/NotoSans-Regular.ttf",
    "Rss:                 200 kB",
    "Pss:                  50 kB",
    "Pss_Dirty:             0 kB",
    "Private_Clean:         0 kB",
    "Private_Dirty:         0 kB",
    "7f0000300000-7f0000400000 rw-s 00000000 00:01 300    /memfd:WebKitSharedMemory (deleted)",
    "Rss:                 512 kB",
    "Pss:                 256 kB",
    "Pss_Dirty:           256 kB",
    "Private_Clean:         0 kB",
    "Private_Dirty:         0 kB",
    "7f0000400000-7f0000500000 rw-s 00000000 00:06 400    /dev/dri/renderD128",
    "Rss:                   0 kB",
    "Pss:                   0 kB",
    "Private_Clean:         0 kB",
    "Private_Dirty:         0 kB",
    "7ffd00000000-7ffd00021000 rw-p 00000000 00:00 0      [stack]",
    "Rss:                 132 kB",
    "Pss:                 132 kB",
    "Pss_Dirty:           132 kB",
    "Private_Clean:         0 kB",
    "Private_Dirty:       132 kB",
    "VmFlags: rd wr mr mw me gd ac",
])

FDINFO_XE = "\n".join([
    "pos:\t0",
    "drm-driver:\txe",
    "drm-client-id:\t42",
    "drm-pdev:\t0000:00:02.0",
    "drm-total-system0:\t2048 KiB",
    "drm-resident-system0:\t1024 KiB",
    "drm-total-cycles-rcs:\t123456789",
    "drm-total-vram0:\t1 MiB",
])


class KindTest(unittest.TestCase):
    def test_kinds(self):
        self.assertEqual(linuxmem.kind_of("/usr/lib/libc.so.6"), "lib")
        self.assertEqual(linuxmem.kind_of("/usr/lib/libicudata.so.78.1"), "lib")
        self.assertEqual(linuxmem.kind_of("/usr/lib/ld-linux-x86-64.so.2"), "lib")
        self.assertEqual(linuxmem.kind_of("/usr/lib/gdk-pixbuf/loaders/libpixbufloader-png.so"), "lib")
        self.assertEqual(linuxmem.kind_of("/home/u/atrium/target/perf/atrium"), "file")
        self.assertEqual(linuxmem.kind_of("/usr/lib/locale/locale-archive"), "file")
        self.assertEqual(linuxmem.kind_of("/home/u/.cache/fontconfig/abc-le64.cache-9"), "file")
        self.assertEqual(linuxmem.kind_of("/home/u/notes.so.txt"), "file")
        self.assertEqual(linuxmem.kind_of("/memfd:wayland-cursor (deleted)"), "shmem")
        self.assertEqual(linuxmem.kind_of("/dev/shm/x"), "shmem")
        self.assertEqual(linuxmem.kind_of("/dev/dri/renderD128"), "gpu")
        self.assertEqual(linuxmem.kind_of("[heap]"), "anon")
        self.assertEqual(linuxmem.kind_of("[anon:glibc malloc]"), "anon")
        self.assertEqual(linuxmem.kind_of(""), "anon")


class SmapsTest(unittest.TestCase):
    def test_private_pss_leaves_out_only_clean_library_pages(self):
        by_name: dict = {}
        out = linuxmem.parse_smaps(SMAPS, by_name)
        self.assertEqual(out["pss"], 400 + 64 + 4096 + 50 + 256 + 0 + 132)
        self.assertEqual(out["lib_clean"], 400 + 4)
        self.assertEqual(out["lib_dirty"], 60)
        self.assertEqual(out["anon"], 4096 + 132)
        self.assertEqual(out["file"], 50)
        self.assertEqual(out["shmem"], 256)
        self.assertEqual(out["gpu_mapped"], 0)
        self.assertEqual(out["private_pss"], out["pss"] - 404)
        self.assertEqual(out["private_pss"], out["anon"] + out["file"] + out["shmem"] + out["lib_dirty"])
        self.assertEqual(out["uss"], 100 + 64 + 4096 + 132)
        self.assertEqual(out["rss"], 1000 + 64 + 4096 + 200 + 512 + 132)
        self.assertEqual(by_name["/usr/lib/libwebkit2gtk-4.1.so.0.19.2"], 60)
        self.assertEqual(by_name["/memfd:WebKitSharedMemory"], 256)
        self.assertEqual(by_name["[heap]"], 4096)
        self.assertNotIn("/dev/dri/renderD128", by_name)

    def test_without_pss_dirty_a_library_counts_nothing(self):
        text = "\n".join(SMAPS.splitlines()[:6])
        out = linuxmem.parse_smaps(text.replace("Pss_Dirty:             0 kB\n", ""))
        self.assertEqual(out["private_pss"], 0)

    def test_empty(self):
        self.assertEqual(linuxmem.parse_smaps("")["pss"], 0)


class FdinfoTest(unittest.TestCase):
    def test_memory_regions_only(self):
        key, res = linuxmem.parse_fdinfo(FDINFO_XE)
        self.assertEqual(key, ("0000:00:02.0", "42"))
        self.assertEqual(res["total"], 2048 + 1024)
        self.assertEqual(res["resident"], 1024)

    def test_not_drm(self):
        self.assertIsNone(linuxmem.parse_fdinfo("pos:\t0\nflags:\t02\n"))


class StatTest(unittest.TestCase):
    def test_comm_with_spaces_and_parens(self):
        self.assertEqual(linuxmem.parse_ppid("123 (Web Content (x)) S 77 123 123 0"), 77)


if __name__ == "__main__":
    unittest.main()
