#!/usr/bin/env python3
"""End-to-end M1 gate demo against a real `atriumd` with the synthetic agent.

    python3 app/scripts/m1_demo.py [--keep]

Builds debug binaries of the app and the daemon, then, in a throwaway ATRIUM_HOME, starts the
app with ATRIUM_BENCH=m1-demo (app/src/m1-demo.ts) and walks through M1's promises in order:

  1. Home chat with the fake agent streams;
  2. Fake approval (`approval_at_ms`): Allow, Deny, "Always" rule + next turn skips asking;
  3. Fake question (`question_at_ms`): Answer, Decline, streaming continues both times;
  4. Inbox lists threads with their states, and marking a thread read works;
  5. Cmd+J jumps through inbox items in rank order;
  6. Notifications: Settings toggles persist in the live daemon, and the daemon's recording
     notifier suite (`requests_notifications`) proves per-kind posting and suppression;
  7. Stop a turn / Shutdown to Ended / attempt Resume right after Ended (plus daemon N1 check);
  8. Kill the daemon mid-turn: the app reconnects and the thread shows `Interrupted`;
  9. Export the thread to Markdown and JSONL: both private (0600) files exist with transcript;
 10. Archive removes the thread from lists and inbox.

Exit status 0 only if every check passed. Run on the machine's desktop session: it opens
windows. `--keep` leaves ATRIUM_HOME, logs, and exported files in place.
"""

import json
import os
import pathlib
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

from live_check import App, ROOT, alive, build, daemon_pid


def run_cargo_test(test_args: list[str]) -> tuple[bool, str]:
    proc = subprocess.run(
        ["cargo", "test", *test_args],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    summary_line = ""
    for line in proc.stdout.splitlines():
        if "test result:" in line:
            summary_line = line.strip()
    if not summary_line:
        lines = [ln.strip() for ln in proc.stdout.splitlines() if ln.strip()]
        summary_line = lines[-1] if lines else f"exit {proc.returncode}"
    return proc.returncode == 0, summary_line


def verify_exports(export_paths: dict) -> list[tuple[str, bool, str]]:
    out: list[tuple[str, bool, str]] = []
    md_path = pathlib.Path(export_paths["markdown"])
    jsonl_path = pathlib.Path(export_paths["jsonl"])
    expected_text = export_paths["expectedText"]

    md_ok = md_path.exists()
    md_mode = stat.S_IMODE(md_path.stat().st_mode) if md_ok else None
    md_body = md_path.read_text(encoding="utf-8", errors="replace") if md_ok else ""
    out.append((
        "9b. Markdown export file exists (0600) and contains transcript",
        md_ok and md_mode == 0o600 and expected_text in md_body and "synthetic tokens" in md_body,
        f"exists={md_ok} mode={oct(md_mode) if md_mode is not None else None} bytes={len(md_body.encode('utf-8'))}",
    ))

    jsonl_ok = jsonl_path.exists()
    jsonl_mode = stat.S_IMODE(jsonl_path.stat().st_mode) if jsonl_ok else None
    jsonl_body = jsonl_path.read_text(encoding="utf-8", errors="replace") if jsonl_ok else ""
    jsonl_lines = [ln for ln in jsonl_body.splitlines() if ln.strip()]
    jsonl_valid = False
    if jsonl_lines:
        try:
            for ln in jsonl_lines:
                json.loads(ln)
            jsonl_valid = True
        except json.JSONDecodeError:
            jsonl_valid = False
    out.append((
        "9c. JSONL export file exists (0600), parses as JSONL, and contains transcript",
        jsonl_ok and jsonl_mode == 0o600 and jsonl_valid and expected_text in jsonl_body and "synthetic tokens" in jsonl_body,
        f"exists={jsonl_ok} mode={oct(jsonl_mode) if jsonl_mode is not None else None} lines={len(jsonl_lines)} valid_json={jsonl_valid}",
    ))
    return out


def main() -> int:
    keep = "--keep" in sys.argv[1:]
    build()

    # Short path under /tmp so the Unix socket path stays below ~100 bytes.
    home = pathlib.Path(tempfile.mkdtemp(prefix="atrium-demo-", dir="/tmp"))
    home.chmod(0o700)
    export_dir = home / "exports"
    export_dir.mkdir(mode=0o700)
    results: list[tuple[str, bool, str]] = []
    export_files: list[pathlib.Path] = []
    app = None
    daemons: set[int] = set()

    try:
        app = App(home, f"m1-demo:{export_dir}", home / "app.log")
        try:
            kill_req = app.wait_for(
                "the mid-turn stream before killing the daemon",
                lambda r: r.get("kind") in ("m1-demo-kill-daemon", "failure"),
            )
            if kill_req["kind"] == "failure":
                raise SystemExit(f"the app failed at stage {kill_req.get('stage')}: {kill_req.get('error')}")

            old_daemon = daemon_pid(home)
            daemons.add(old_daemon)
            os.kill(old_daemon, signal.SIGKILL)

            report = app.wait_for(
                "the M1 demo report",
                lambda r: r.get("kind") in ("m1-demo", "failure"),
            )
            if report["kind"] == "failure":
                raise SystemExit(f"the app failed at stage {report.get('stage')}: {report.get('error')}")
        except BaseException:
            if (home / "app.log").exists():
                print("--- app.log ---", file=sys.stderr)
                print((home / "app.log").read_text(errors="replace"), file=sys.stderr)
                print("--- end app.log ---", file=sys.stderr)
            raise

        new_daemon = daemon_pid(home)
        daemons.add(new_daemon)

        for check in report["checks"]:
            results.append((check["name"], check["pass"], check["detail"]))
        results.append((
            "8b. App started a new daemon after SIGKILL",
            new_daemon != old_daemon and alive(new_daemon) and not alive(old_daemon),
            f"pid {old_daemon} -> {new_daemon}",
        ))

        export_paths = report.get("exportPaths") or {}
        if "markdown" in export_paths:
            export_files.append(pathlib.Path(export_paths["markdown"]))
        if "jsonl" in export_paths:
            export_files.append(pathlib.Path(export_paths["jsonl"]))
        if export_paths:
            results.extend(verify_exports(export_paths))
    finally:
        if app is not None:
            app.kill()
        try:
            daemons.add(daemon_pid(home))
        except (SystemExit, OSError):
            pass
        for pid in daemons:
            if alive(pid):
                os.kill(pid, signal.SIGINT)
        if keep:
            print(f"kept: {home}")
            for p in export_files:
                if p.exists():
                    print(f"kept export: {p}")
        else:
            time.sleep(0.5)
            shutil.rmtree(home, ignore_errors=True)
            for p in export_files:
                p.unlink(missing_ok=True)

    # Step 6b & Step 7c: daemon-level checks using the recording notifier and N1 tight loop.
    notif_ok, notif_detail = run_cargo_test(["-p", "atrium-daemon", "--test", "requests_notifications"])
    results.append((
        "6b. Daemon recording notifier posts approval/question/failed and respects off-switches",
        notif_ok,
        notif_detail,
    ))
    n1_ok, n1_detail = run_cargo_test([
        "-p", "atrium-daemon", "--test", "sessions",
        "resume_right_after_final_state_in_a_tight_loop",
    ])
    results.append((
        "7c. Daemon N1 regression test (resume right after Ended/Interrupted/Failed/Shutdown)",
        n1_ok,
        n1_detail,
    ))

    width = max(len(name) for name, _, _ in results)
    for name, ok, detail in results:
        print(f"{'ok  ' if ok else 'FAIL'} {name.ljust(width)}  {detail}")
    failed = [name for name, ok, _ in results if not ok]
    print(f"{len(results) - len(failed)}/{len(results)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
