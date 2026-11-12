#!/usr/bin/env python3
"""Live check of the app shell against a real `atriumd` (M1-09a Definition of Done).

    python3 app/scripts/live_check.py [--keep]

Builds debug binaries of the app and the daemon, then, in a throwaway ATRIUM_HOME:

  1. starts the app, which starts the daemon and runs the window and draft round trip
     (app/src/roundtrip.ts): a draft survives a move to its own window, the return, and a
     forced return of a window that doesn't answer;
  2. kills the daemon: the app must reconnect by itself (starting a new daemon) and still
     list the thread, now interrupted;
  3. kills the app (a crash) and starts it again: the draft and the thread shown come back.

Everything the app reports is printed. Exit status 0 only if every check passed.
Run it on the machine's own desktop session: it opens windows. `--keep` leaves the
ATRIUM_HOME folder and the logs in place.
"""

import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
APP = ROOT / "target" / "debug" / "atrium"
STEP_TIMEOUT_S = 90


def build() -> None:
    subprocess.run(["pnpm", "build"], cwd=ROOT / "app", check=True, stdout=subprocess.DEVNULL)
    subprocess.run(
        ["cargo", "build", "-p", "atrium-app", "--features", "custom-protocol", "-p", "atrium-daemon"],
        cwd=ROOT, check=True,
    )


class App:
    def __init__(self, home: pathlib.Path, mode: str, log: pathlib.Path):
        env = dict(os.environ, ATRIUM_HOME=str(home), ATRIUM_BENCH=mode)
        self.log = log
        self.seen = 0
        self.out = open(log, "wb")
        self.process = subprocess.Popen([str(APP)], env=env, stdout=self.out, stderr=subprocess.STDOUT)

    def reports(self) -> list[dict]:
        found = []
        for line in self.log.read_text(errors="replace").splitlines():
            if line.startswith("ATRIUM_REPORT "):
                try:
                    found.append(json.loads(line[len("ATRIUM_REPORT "):]))
                except json.JSONDecodeError:
                    pass
        return found

    def wait_for(self, what: str, match) -> dict:
        deadline = time.monotonic() + STEP_TIMEOUT_S
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise SystemExit(f"the app exited ({self.process.returncode}) while waiting for {what}; see {self.log}")
            reports = self.reports()
            for report in reports[self.seen:]:
                self.seen += 1
                if match(report):
                    return report
            time.sleep(0.1)
        raise SystemExit(f"timed out waiting for {what}; see {self.log}")

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        self.out.close()


def daemon_pid(home: pathlib.Path) -> int:
    for lock in home.rglob("*.lock"):
        try:
            return int(json.loads(lock.read_text())["pid"])
        except (ValueError, KeyError):
            continue
    raise SystemExit(f"no daemon lock file under {home}")


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def main() -> int:
    keep = "--keep" in sys.argv[1:]
    build()
    # A short path: a unix socket's path is limited to about 100 bytes.
    home = pathlib.Path(tempfile.mkdtemp(prefix="atrium-live-", dir="/tmp"))
    home.chmod(0o700)
    results: list[tuple[str, bool, str]] = []
    first = second = None
    daemons: set[int] = set()
    try:
        first = App(home, "roundtrip", home / "app-1.log")
        report = first.wait_for("the round trip", lambda r: r.get("kind") in ("roundtrip", "failure"))
        if report["kind"] == "failure":
            raise SystemExit(f"the app failed at stage {report.get('stage')}: {report.get('error')}")
        for check in report["checks"]:
            results.append((check["name"], check["pass"], check["detail"]))
        thread, expected = report["thread"], report["expected"]

        old = daemon_pid(home)
        daemons.add(old)
        first.wait_for("the connected state", lambda r: r.get("kind") == "connection" and r["state"] == "connected")
        os.kill(old, signal.SIGKILL)
        first.wait_for("the app to notice", lambda r: r.get("kind") == "connection" and r["state"] != "connected")
        first.wait_for("the app to reconnect", lambda r: r.get("kind") == "connection" and r["state"] == "connected")
        # The list the app held is kept while it reconnects; the new daemon's list replaces it
        # a moment later, with the session it found cut off.
        back = first.wait_for(
            "the new daemon's thread list",
            lambda r: r.get("kind") == "connection" and r["state"] == "connected"
            and any(t["id"] == thread and t["state"] != "running" for t in r["threads"]),
        )
        new = daemon_pid(home)
        daemons.add(new)
        results.append(("the app started a new daemon after the old one was killed", new != old and alive(new), f"pid {old} -> {new}"))
        listed = {t["id"]: t["state"] for t in back["threads"]}
        results.append(("the thread is still listed after the reconnect", thread in listed, json.dumps(listed)))
        results.append(("its cut-off session shows as interrupted", listed.get(thread) == "interrupted", str(listed.get(thread))))

        # The crash: no chance to flush anything. Drafts are written a quarter of a second
        # after the last keystroke, and the round trip's last one was long ago by now.
        first.kill()
        second = App(home, "roundtrip-verify", home / "app-2.log")
        verify = second.wait_for("the relaunch", lambda r: r.get("kind") in ("roundtrip-verify", "failure"))
        if verify["kind"] == "failure":
            raise SystemExit(f"the relaunch failed at stage {verify.get('stage')}: {verify.get('error')}")
        results.append(("after a crash the main window shows the same thread", verify["thread"] == thread and verify["inList"], str(verify["thread"])))
        results.append(("after a crash the draft is back", verify["draft"] == expected, json.dumps(verify["draft"])))
        results.append(("the relaunched app used the running daemon", daemon_pid(home) == new, f"pid {daemon_pid(home)}"))
    finally:
        for app in (first, second):
            if app is not None:
                app.kill()
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
