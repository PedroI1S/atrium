#!/usr/bin/env python3
"""End-to-end M1 gate demo against a real `atriumd` with the synthetic agent.

    python3 app/scripts/m1_demo.py [--keep]

Builds debug binaries of the app and the daemon, then, in a throwaway ATRIUM_HOME, starts the
app with ATRIUM_BENCH=m1-demo (app/src/m1-demo.ts) and walks through M1's promises in order:

  1. Home chat with the fake agent streams;
  2. Fake approval (`approval_at_ms`): Allow, Deny, "Always" rule + next turn skips asking;
  3. Fake question (`question_at_ms`): Answer, Decline, streaming continues both times;
  4. Inbox lists threads with their states, and marking a thread read works;
  5. Cmd+J jumps through inbox items in rank order, and Cmd+Shift+J back;
  6. Notifications: Settings toggle persists in the live daemon (toggle only), and the daemon's
     recording notifier suite (`requests_notifications`) proves per-kind posting and suppression;
  7. Stop a turn / End session from the thread menu to Ended / verify fake resume refusal (plus
     daemon-only scripted-adapter N1 test);
  8. Kill the daemon mid-turn: the app reconnects and the thread shows `Interrupted`;
  9. Rename the thread, then export it to Markdown and JSONL from the thread menu (the save
     dialog is bypassed in bench mode): both private (0600) files exist with the transcript, and
     exporting to the same name again is refused with a clear message (never replaced);
 10. Archive from the thread menu (confirmed) removes the thread from lists and inbox;
 11. Batch: select all approvals in the inbox, X on a focused checkbox unselects the third,
     Allow once two together, Deny the third; the result clears on leaving the inbox;
 12. Settings: the rules editor refuses a compound command and an allow rule on `bash -c`,
     offers no path limit on a command rule, then adds a command-prefix rule;
     Security lists what is always blocked and the known limits;
 13. The enforcement badge opens the details (coverage, policy, isolation).

Exit status 0 only if every check passed. Run on the machine's desktop session: it opens
windows. `--keep` leaves ATRIUM_HOME, logs, and exported files in place.
"""

import json
import os
import pathlib
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

from live_check import App, ROOT, alive, build, daemon_pid


def all_daemon_pids(home: pathlib.Path) -> set[int]:
    pids: set[int] = set()
    if not home.exists():
        return pids
    for lock in home.rglob("*.lock"):
        try:
            pids.add(int(json.loads(lock.read_text())["pid"]))
        except (OSError, ValueError, json.JSONDecodeError, KeyError, TypeError):
            continue
    return pids


def run_cargo_test(test_args: list[str], min_passed: int = 1) -> tuple[bool, str]:
    proc = subprocess.run(
        ["cargo", "test", *test_args],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    result_lines = [ln.strip() for ln in proc.stdout.splitlines() if "test result:" in ln]
    passed = 0
    for ln in result_lines:
        m = re.search(r"(\d+)\s+passed\b", ln)
        if m:
            passed += int(m.group(1))
    summary_line = result_lines[-1] if result_lines else ""
    if not summary_line:
        lines = [ln.strip() for ln in proc.stdout.splitlines() if ln.strip()]
        summary_line = lines[-1] if lines else f"exit {proc.returncode}"
    ok = proc.returncode == 0 and bool(result_lines) and passed >= min_passed
    if proc.returncode == 0 and passed < min_passed:
        summary_line = f"{summary_line} (expected >= {min_passed} passed, got {passed})"
    return ok, summary_line


def verify_exports(export_paths: dict) -> list[tuple[str, bool, str]]:
    out: list[tuple[str, bool, str]] = []
    md_path = pathlib.Path(export_paths["markdown"])
    jsonl_path = pathlib.Path(export_paths["jsonl"])
    expected_text = export_paths["expectedText"]

    md_ok = md_path.exists()
    md_mode = stat.S_IMODE(md_path.stat().st_mode) if md_ok else None
    md_body = md_path.read_text(encoding="utf-8", errors="replace") if md_ok else ""
    out.append((
        "9c. Markdown export file exists (0600) and contains transcript",
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
        "9d. JSONL export file exists (0600), parses as JSONL, and contains transcript",
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
    cleaned = False

    def cleanup() -> None:
        nonlocal cleaned
        if cleaned:
            return
        cleaned = True
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        if app is not None:
            app.kill()
        daemons.update(all_daemon_pids(home))
        for pid in list(daemons):
            if alive(pid):
                try:
                    os.kill(pid, signal.SIGINT)
                except ProcessLookupError:
                    pass
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline:
            daemons.update(all_daemon_pids(home))
            if not any(alive(pid) for pid in daemons):
                break
            time.sleep(0.1)
        for pid in list(daemons):
            if alive(pid):
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        kill_deadline = time.monotonic() + 0.5
        while time.monotonic() < kill_deadline and any(alive(pid) for pid in daemons):
            time.sleep(0.05)
        if keep:
            print(f"kept: {home}")
            for p in export_files:
                if p.exists():
                    print(f"kept export: {p}")
        else:
            shutil.rmtree(home, ignore_errors=True)

    def handle_sigterm(_signum: int, _frame: object) -> None:
        raise SystemExit(128 + signal.SIGTERM)

    prev_sigterm = signal.signal(signal.SIGTERM, handle_sigterm)
    try:
        app = App(home, f"m1-demo:{export_dir}", home / "app.log")
        try:
            kill_req = app.wait_for(
                "the mid-turn stream before killing the daemon",
                lambda r: r.get("kind") in ("m1-demo-kill-daemon", "failure"),
            )
            if kill_req["kind"] == "failure":
                raise SystemExit(f"the app failed at stage {kill_req.get('stage')}: {kill_req.get('error')}")

            daemons.update(all_daemon_pids(home))
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
            if app is not None:
                partial = [
                    (r["name"], r["pass"], r["detail"])
                    for r in app.reports()
                    if r.get("kind") == "m1-demo-check"
                ]
                if partial:
                    print("--- partial checks before failure ---", file=sys.stderr)
                    for name, ok, detail in partial:
                        print(f"{'ok  ' if ok else 'FAIL'} {name}  {detail}", file=sys.stderr)
            if (home / "app.log").exists():
                print("--- app.log ---", file=sys.stderr)
                print((home / "app.log").read_text(errors="replace"), file=sys.stderr)
                print("--- end app.log ---", file=sys.stderr)
            raise

        current_pids = all_daemon_pids(home)
        daemons.update(current_pids)
        live_new = [p for p in current_pids if p != old_daemon and alive(p)]
        new_daemon = live_new[0] if live_new else daemon_pid(home)
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
        cleanup()
        signal.signal(signal.SIGTERM, prev_sigterm)

    # Step 6b & Step 7d: daemon-level checks using the recording notifier and N1 tight loop.
    notif_ok, notif_detail = run_cargo_test(
        ["-p", "atrium-daemon", "--test", "requests_notifications"],
        min_passed=2,
    )
    results.append((
        "6b. Daemon recording notifier posts approval/question/failed and respects off-switches",
        notif_ok,
        notif_detail,
    ))
    n1_ok, n1_detail = run_cargo_test(
        [
            "-p", "atrium-daemon", "--test", "sessions",
            "resume_right_after_final_state_in_a_tight_loop",
        ],
        min_passed=1,
    )
    results.append((
        "7d (daemon test, scripted adapter). N1 resume after Ended/Interrupted/Failed/Shutdown",
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
