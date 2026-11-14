#!/usr/bin/env python3
"""Live check of the chat against a real `atriumd` with the synthetic agent (M1-09b DoD).

    python3 app/scripts/chat_check.py [--keep]

Builds debug binaries of the app and the daemon, then, in a throwaway ATRIUM_HOME, starts the
app with ATRIUM_BENCH=chat-live (app/src/chat-live.ts):

  1. a Home chat is started from the composer and streams in the main window;
  2. the thread moves to its own window: the second window shows the same thread, streaming;
  3. this script kills the daemon mid-stream: the app reconnects (starting a new daemon), the
     thread shows as interrupted and both windows show its closed transcript;
  4. a new chat started after the reconnect streams, and Stop ends its turn.

Every check the app reports is printed. Exit status 0 only if every check passed. Run it on the
machine's own desktop session: it opens windows. `--keep` leaves the ATRIUM_HOME folder and the
log in place.
"""

import os
import pathlib
import shutil
import signal
import sys
import tempfile
import time

from live_check import App, alive, build, daemon_pid


def main() -> int:
    keep = "--keep" in sys.argv[1:]
    build()
    # A short path: a unix socket's path is limited to about 100 bytes.
    home = pathlib.Path(tempfile.mkdtemp(prefix="atrium-chat-", dir="/tmp"))
    home.chmod(0o700)
    results: list[tuple[str, bool, str]] = []
    app = None
    daemons: set[int] = set()
    try:
        app = App(home, "chat-live", home / "app.log")
        streaming = app.wait_for("the chat to stream in two windows", lambda r: r.get("kind") in ("chat-live-streaming", "failure"))
        if streaming["kind"] == "failure":
            raise SystemExit(f"the app failed at stage {streaming.get('stage')}: {streaming.get('error')}")
        old = daemon_pid(home)
        daemons.add(old)
        os.kill(old, signal.SIGKILL)
        report = app.wait_for("the checks after the daemon was killed", lambda r: r.get("kind") in ("chat-live", "failure"))
        if report["kind"] == "failure":
            raise SystemExit(f"the app failed at stage {report.get('stage')}: {report.get('error')}")
        for check in report["checks"]:
            results.append((check["name"], check["pass"], check["detail"]))
        new = daemon_pid(home)
        daemons.add(new)
        results.append(("the daemon was killed mid-stream and the app started a new one", new != old and alive(new) and not alive(old), f"pid {old} -> {new}"))
    finally:
        if app is not None:
            app.kill()
        try:
            # Whatever daemon the app last started, also when a step failed before it was noted.
            daemons.add(daemon_pid(home))
        except (SystemExit, OSError):
            pass
        for pid in daemons:
            if alive(pid):
                os.kill(pid, signal.SIGINT)
        if keep:
            print(f"kept: {home}")
        else:
            time.sleep(0.5)
            shutil.rmtree(home, ignore_errors=True)

    width = max(len(name) for name, _, _ in results)
    for name, ok, detail in results:
        print(f"{'ok  ' if ok else 'FAIL'} {name.ljust(width)}  {detail}")
    failed = [name for name, ok, _ in results if not ok]
    print(f"{len(results) - len(failed)}/{len(results)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
