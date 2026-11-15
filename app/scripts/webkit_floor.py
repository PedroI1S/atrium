#!/usr/bin/env python3
"""WebKitGTK's own memory floor on this machine, to tell it from Atrium's (M1-14b, D-085).

    python3 app/scripts/webkit_floor.py [--pages blank,text,atrium,minibrowser] [--launches 3]
        [--dist DIR] [--seconds 31]

Opens a plain GTK 3 window with one WebKitWebView (webkit2gtk-4.1, the library Tauri uses on
Linux, through PyGObject), the size of Atrium's main window, and samples its process tree
exactly as startup_bench.py does (linuxmem.py: D-040's private PSS, per process, and GPU
memory), at 1 Hz from 5 s to 35 s after the page finished loading. Pages:
  - blank: `about:blank`;
  - text: one paragraph of text in the system font;
  - minibrowser: WebKitGTK's MiniBrowser (C, GTK 3) on about:blank, for a UI process without
    Python (it has a toolbar and its own settings, so it is close to a bare one, not equal);
  - atrium: Atrium's built frontend (`app/dist`, run `pnpm build` first) served over
    http://127.0.0.1 by this script, which the frontend runs in its browser mode (no Tauri: no
    daemon, no IPC, an empty shell), so its cost in the web process is the page's own.
The window process is a child of this script (this script samples, the child only shows the
page), so the UI process row is Python + GTK + WebKit's UI side: not Atrium's UI process, but
its WebKit share. The web and network rows compare one to one with Atrium's.
"""

import argparse
import functools
import json
import http.server
import pathlib
import statistics
import subprocess
import sys
import threading
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import linuxmem  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
MINIBROWSER = "/usr/lib/webkit2gtk-4.1/MiniBrowser"
TEXT = "data:text/html;charset=utf-8,<!doctype html><title>t</title><p>Atrium measures what WebKitGTK costs on its own.</p>"


def window(uri: str) -> None:
    """The child: one window with one web view, as Atrium's main window (1360 × 860)."""
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("WebKit2", "4.1")
    from gi.repository import Gtk, WebKit2

    win = Gtk.Window(title="webkit floor")
    win.set_default_size(1360, 860)
    view = WebKit2.WebView()

    from gi.repository import GLib

    def timing():
        # The page's own clock: navigation start → first contentful paint, and the window's
        # process start (`t0`, from the script) is not comparable, so only page-relative times.
        script = ("JSON.stringify({fcp: (performance.getEntriesByType('paint').find(p => p.name === 'first-contentful-paint') || {}).startTime ?? null,"
                  " dcl: performance.getEntriesByType('navigation')[0]?.domContentLoadedEventStart ?? null})")

        def done(v, result):
            try:
                print("TIMING", v.evaluate_javascript_finish(result).to_string(), flush=True)
            except Exception as error:  # noqa: BLE001 — report and go on: timing is optional
                print("TIMING", "{}", "#", error, flush=True)

        view.evaluate_javascript(script, -1, None, None, None, done)
        return False

    def changed(_view, event):
        if event == WebKit2.LoadEvent.FINISHED:
            print("LOADED", flush=True)
            GLib.timeout_add(1000, timing)

    view.connect("load-changed", changed)
    win.add(view)
    win.connect("destroy", Gtk.main_quit)
    win.show_all()
    view.load_uri(uri)
    Gtk.main()


def serve_dist(dist: pathlib.Path) -> tuple[http.server.ThreadingHTTPServer, str]:
    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *args):  # noqa: D102 — no access log on the terminal
            pass

    handler = functools.partial(Quiet, directory=str(dist))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_address[1]}/index.html"


def measure(uri: str, seconds: int) -> dict:
    if uri == MINIBROWSER:
        # WebKitGTK's own test browser (C, GTK 3): a UI process without Python, on about:blank.
        child = subprocess.Popen([MINIBROWSER, "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        page_timing: dict = {}
        time.sleep(3)
    else:
        child = subprocess.Popen([sys.executable, __file__, "--window", uri], stdout=subprocess.PIPE, text=True)
    try:
        if uri != MINIBROWSER:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if child.stdout.readline().strip() == "LOADED":
                    break
            else:
                raise SystemExit("the page never finished loading")
            line = child.stdout.readline().split(None, 1)
            page_timing = json.loads(line[1].split(" #")[0]) if len(line) == 2 and line[0] == "TIMING" else {}
        if seconds:
            time.sleep(4)
        samples = [linuxmem.sample(child.pid)]
        for _ in range(seconds):
            tick = time.monotonic()
            samples.append(linuxmem.sample(child.pid))
            time.sleep(max(0.0, 1.0 - (time.monotonic() - tick)))
    finally:
        child.kill()
        child.wait()
    if seconds:
        samples = samples[1:]
    roles = sorted({r for s in samples for r in s["roles"]})
    out = {"privatePss": statistics.median(linuxmem.mib(s["total"]["private_pss"]) for s in samples),
           "pss": statistics.median(linuxmem.mib(s["total"]["pss"]) for s in samples),
           "gpuResident": statistics.median(linuxmem.mib(s["gpu"]["resident"]) if s["gpu"] else 0 for s in samples),
           "timing": page_timing}
    for role in roles:
        out[role] = statistics.median(linuxmem.mib(s["roles"].get(role, {}).get("private_pss", 0)) for s in samples)
    return out


def main() -> int:
    if len(sys.argv) == 3 and sys.argv[1] == "--window":
        window(sys.argv[2])
        return 0
    parser = argparse.ArgumentParser()
    parser.add_argument("--pages", default="blank,text,atrium")
    parser.add_argument("--launches", type=int, default=3)
    parser.add_argument("--dist", type=pathlib.Path, default=ROOT / "app" / "dist", help="the built frontend the atrium page serves")
    parser.add_argument("--seconds", type=int, default=31, help="memory samples per launch (0: page timing only)")
    args = parser.parse_args()
    server, atrium = serve_dist(args.dist)
    uris = {"blank": "about:blank", "text": TEXT, "atrium": atrium, "minibrowser": MINIBROWSER}
    try:
        for page in args.pages.split(","):
            for run in range(args.launches):
                m = measure(uris[page], args.seconds)
                roles = ", ".join(f"{k} {v:.1f}" for k, v in m.items() if k not in ("privatePss", "pss", "gpuResident", "timing"))
                print(f"{page} launch {run + 1}: private PSS {m['privatePss']:.1f} MiB [{roles}]; PSS {m['pss']:.1f}; "
                      f"GPU resident {m['gpuResident']:.1f}; page timing (ms from navigation start) {m['timing']}")
    finally:
        server.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
