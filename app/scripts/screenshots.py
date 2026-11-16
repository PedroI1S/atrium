#!/usr/bin/env python3
"""Screenshots of the app shell for reviews (Linux).

    python3 app/scripts/screenshots.py <out-dir> [real] [chat] [refused] [gaps]

Starts the app four times, each in a throwaway ATRIUM_HOME (name the runs to do only some):
  - real: against a real daemon it starts itself, with synthetic threads: the main window in
    light and dark (a chat streaming from the fake agent), the sidebar hidden, the panel
    minimized, the inbox, Settings with its Advanced card, and a thread in its own window;
  - chat (M1-09b): against a real daemon, with scenes seeded in the window, because the fake
    agent can't write markdown or raise approvals and questions (app/src/chat-shots.ts says
    what is seeded and what is the daemon's): a streaming chat with markdown and tool calls,
    an approval card and its "Always…" choices, a question form, the inbox with items,
    Settings → Agents, Approvals and Notifications, and a refused approval; light and dark;
  - refused: against a stand-in daemon that refuses the app as an older version would: the
    banner;
  - gaps (M1-23): against a real daemon, with the chat scenes seeded as above (app/src/gap-shots.ts):
    the thread menu, renaming, the archive confirmation, the enforcement details, batch
    approvals selected and their (real) refusals, the rules editor (valid and refused), Settings →
    Security and the palette's "This chat" commands; light and dark.

The pictures are the webviews' own snapshots (the desktop session lets no script capture
windows). Run `python3 app/scripts/live_check.py` first, or build the same way: this script
uses the binaries it finds.
"""

import json
import os
import pathlib
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
APP = ROOT / "target" / "debug" / "atrium"
TIMEOUT_S = 120
# Run name -> (ATRIUM_BENCH mode, pictures it takes).
RUNS = {"real": ("shots", 9), "chat": ("shots-chat", 18), "refused": ("shots-incompatible", 1), "gaps": ("shots-gaps", 20)}


def run_app(home: pathlib.Path, mode: str, shots: pathlib.Path, log: pathlib.Path) -> None:
    env = dict(os.environ, ATRIUM_HOME=str(home), ATRIUM_BENCH=mode, ATRIUM_SHOT_DIR=str(shots))
    with open(log, "wb") as out:
        app = subprocess.Popen([str(APP)], env=env, stdout=out, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + TIMEOUT_S
            while time.monotonic() < deadline:
                text = log.read_text(errors="replace")
                if 'ATRIUM_REPORT {"kind":"shots"' in text:
                    return
                if 'ATRIUM_REPORT {"kind":"failure"' in text or app.poll() is not None:
                    raise SystemExit(f"the app failed in {mode}; see {log}:\n{text[-2000:]}")
                time.sleep(0.2)
            raise SystemExit(f"timed out in {mode}; see {log}")
        finally:
            if app.poll() is None:
                app.kill()
                app.wait()


def stop_daemon(home: pathlib.Path) -> None:
    lock = home / "daemon.lock"
    try:
        os.kill(int(json.loads(lock.read_text())["pid"]), signal.SIGINT)
    except (OSError, ValueError, KeyError):
        pass


def refusing_daemon(home: pathlib.Path) -> socket.socket:
    """Answers every `Hello` the way an older daemon does (wire.rs: `Incompatible`)."""
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(str(home / "daemon.sock"))
    os.chmod(home / "daemon.sock", 0o600)
    listener.listen(4)
    refusal = json.dumps({
        "type": "incompatible", "protocol_version": 0,
        "message": "It speaks an older protocol than this app.",
        "daemon_version": "0.0.0-older", "daemon_pid": os.getpid(), "running_turns": 2,
    }).encode()
    frame = struct.pack(">IB", len(refusal) + 1, 0) + refusal

    def serve() -> None:
        while True:
            try:
                connection, _ = listener.accept()
            except OSError:
                return
            with connection:
                try:
                    header = connection.recv(5)
                    if len(header) == 5:
                        connection.recv(struct.unpack(">I", header[:4])[0] - 1)
                        connection.sendall(frame)
                        time.sleep(0.2)
                except OSError:
                    pass

    threading.Thread(target=serve, daemon=True).start()
    return listener


def main() -> int:
    wanted = sys.argv[2:] or list(RUNS)
    if len(sys.argv) < 2 or any(name not in RUNS for name in wanted):
        print(__doc__)
        return 2
    shots = pathlib.Path(sys.argv[1]).resolve()
    shots.mkdir(parents=True, exist_ok=True)
    # Build first, as live_check.py does: a stale binary takes stale pictures.
    subprocess.run(["pnpm", "build"], cwd=ROOT / "app", check=True, stdout=subprocess.DEVNULL)
    subprocess.run(
        ["cargo", "build", "-p", "atrium-app", "--features", "custom-protocol", "-p", "atrium-daemon"],
        cwd=ROOT, check=True,
    )
    for name in wanted:
        mode = RUNS[name][0]
        home = pathlib.Path(tempfile.mkdtemp(prefix="atrium-shots-", dir="/tmp"))
        home.chmod(0o700)
        listener = refusing_daemon(home) if mode == "shots-incompatible" else None
        try:
            run_app(home, mode, shots, shots / f"{name}.log")
        finally:
            if listener is not None:
                listener.close()
            else:
                stop_daemon(home)
            time.sleep(0.5)
            shutil.rmtree(home, ignore_errors=True)
    pictures = sorted(p.name for p in shots.glob("*.png"))
    print("\n".join(pictures))
    return 0 if len(pictures) >= sum(RUNS[name][1] for name in wanted) else 1


if __name__ == "__main__":
    sys.exit(main())
