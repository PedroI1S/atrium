use std::io::BufRead;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use atrium_host::supervise::{ProcessIdentity, identify, kill_recorded, spawn_group, terminate};
use atrium_protocol::thread::Cleanup;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

fn pid_exists(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    if raw <= 0 {
        return false;
    }
    // SAFETY: Signal 0 performs an existence/permission check without sending
    // any signal.
    let rc = unsafe { libc::kill(raw, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

struct KillGroupGuard(u32);

impl Drop for KillGroupGuard {
    fn drop(&mut self) {
        if let Ok(raw) = i32::try_from(self.0)
            && raw > 1
        {
            // SAFETY: Best-effort test cleanup of a process group this test
            // created (`raw > 1`, so `-raw <= -2`).
            unsafe {
                libc::kill(-raw, libc::SIGKILL);
            }
        }
    }
}

struct KillPidGuard(u32);

impl Drop for KillPidGuard {
    fn drop(&mut self) {
        if let Ok(raw) = i32::try_from(self.0)
            && raw > 1
        {
            // SAFETY: Best-effort test cleanup of a process this test spawned.
            unsafe {
                libc::kill(raw, libc::SIGKILL);
            }
        }
    }
}

#[tokio::test]
async fn child_spawning_grandchildren_is_fully_gone_after_terminate() {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(
            "sh -c 'sleep 60 & echo $!; wait' & \
             mid=$!; \
             echo $mid; \
             wait",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut sup = spawn_group(cmd).expect("spawn_group");
    let leader_pid = sup.identity.pid;
    assert_eq!(sup.identity.pgid, leader_pid);
    let _guard = KillGroupGuard(sup.identity.pgid);

    let stdout = sup.child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let line1 = lines.next_line().await.unwrap().expect("pid line 1");
    let line2 = lines.next_line().await.unwrap().expect("pid line 2");
    let pid_a: u32 = line1.trim().parse().expect("parse pid_a");
    let pid_b: u32 = line2.trim().parse().expect("parse pid_b");

    // Confirm leader, child shell, and grandchild sleep are all alive in the group.
    for pid in [leader_pid, pid_a, pid_b] {
        assert!(
            pid_exists(pid),
            "expected pid {pid} to be alive before terminate"
        );
        let id = identify(pid).unwrap().expect("running process identity");
        assert_eq!(id.pgid, sup.identity.pgid);
    }

    let term = terminate(&mut sup, Duration::from_millis(80))
        .await
        .expect("terminate");
    assert!(term.killed, "group should have been killed after grace");
    assert_eq!(term.exit_code, None);

    for pid in [leader_pid, pid_a, pid_b] {
        assert!(
            !pid_exists(pid),
            "pid {pid} must be completely gone (ESRCH) after terminate"
        );
        assert_eq!(identify(pid).unwrap(), None);
    }
}

#[tokio::test]
async fn leader_exiting_early_still_has_descendants_killed() {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(
            "sleep 60 >/dev/null 2>&1 </dev/null & \
             d1=$!; \
             (sleep 60 >/dev/null 2>&1 </dev/null & echo $!) ; \
             echo $d1; \
             exit 7",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut sup = spawn_group(cmd).expect("spawn_group");
    let _guard = KillGroupGuard(sup.identity.pgid);

    let stdout = sup.child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let grandchild_pid: u32 = lines
        .next_line()
        .await
        .unwrap()
        .expect("grandchild line")
        .trim()
        .parse()
        .expect("grandchild pid");
    let child_pid: u32 = lines
        .next_line()
        .await
        .unwrap()
        .expect("child line")
        .trim()
        .parse()
        .expect("child pid");
    // Wait for EOF on stdout so we know the leader has executed `exit 7`.
    assert!(lines.next_line().await.unwrap().is_none());
    tokio::time::sleep(Duration::from_millis(30)).await;

    assert!(pid_exists(child_pid));
    assert!(pid_exists(grandchild_pid));

    let term = terminate(&mut sup, Duration::from_secs(2))
        .await
        .expect("terminate");
    assert!(!term.killed, "leader exited before grace expired");
    assert_eq!(term.exit_code, Some(7));

    for pid in [sup.identity.pid, child_pid, grandchild_pid] {
        assert!(
            !pid_exists(pid),
            "pid {pid} must be gone after terminate cleans up early-exit leader's group"
        );
        assert_eq!(identify(pid).unwrap(), None);
    }
}

#[tokio::test]
async fn kill_recorded_returns_unverified_for_reused_pid_and_never_kills_it() {
    use std::os::unix::process::CommandExt;

    let mut bystander = std::process::Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .expect("spawn bystander");
    let pid = bystander.id();
    let _guard = KillPidGuard(pid);

    let real_id = identify(pid).unwrap().expect("bystander alive");

    // Simulate PID reuse by a different executable.
    let wrong_exe = ProcessIdentity {
        executable: PathBuf::from("/usr/bin/true"),
        ..real_id.clone()
    };
    assert_eq!(kill_recorded(&wrong_exe), Cleanup::Unverified);
    assert!(
        pid_exists(pid),
        "bystander with different exe must never be killed"
    );
    assert_eq!(identify(pid).unwrap().as_ref(), Some(&real_id));

    // Simulate PID reuse by the same executable with a different start_time.
    let wrong_start = ProcessIdentity {
        start_time: real_id.start_time.wrapping_sub(1),
        ..real_id.clone()
    };
    assert_eq!(kill_recorded(&wrong_start), Cleanup::Unverified);
    assert!(
        pid_exists(pid),
        "bystander with different start_time must never be killed"
    );

    // Guard against degenerate PIDs 0 and 1.
    assert_eq!(
        kill_recorded(&ProcessIdentity {
            pid: 1,
            pgid: 1,
            start_time: 0,
            executable: PathBuf::from("/sbin/launchd"),
        }),
        Cleanup::Unverified
    );

    let _ = bystander.kill();
    let _ = bystander.wait();
}

#[tokio::test]
async fn gone_leader_with_live_descendant_in_group_is_not_verified() {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg("sleep 60 >/dev/null 2>&1 </dev/null & echo $!; exit 0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut sup = spawn_group(cmd).expect("spawn_group");
    let recorded = sup.identity.clone();
    let _guard = KillGroupGuard(recorded.pgid);

    let stdout = sup.child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let descendant_pid: u32 = lines
        .next_line()
        .await
        .unwrap()
        .expect("descendant pid line")
        .trim()
        .parse()
        .expect("parse descendant pid");
    let _desc_guard = KillPidGuard(descendant_pid);

    // Reap ONLY the leader (simulating an exited leader reaped by init after a
    // daemon crash while a tool descendant remains in `recorded.pgid`).
    let status = sup.child.wait().await.expect("reap leader");
    assert!(status.success());

    assert_eq!(identify(recorded.pid).unwrap(), None, "leader must be gone");
    let desc_id = identify(descendant_pid)
        .unwrap()
        .expect("descendant must still be alive");
    assert_eq!(desc_id.pgid, recorded.pgid);

    // Must return Unverified and MUST NOT kill the remaining process in `pgid`.
    assert_eq!(kill_recorded(&recorded), Cleanup::Unverified);
    assert!(
        pid_exists(descendant_pid),
        "kill_recorded must not kill unverified group members when leader is gone"
    );

    // Once the descendant is gone too, the group is empty, so recovery is Verified.
    // SAFETY: Killing the descendant this test spawned so we can verify the
    // empty-group branch.
    unsafe {
        libc::kill(descendant_pid as i32, libc::SIGKILL);
    }
    for _ in 0..100 {
        if !pid_exists(descendant_pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!pid_exists(descendant_pid));
    assert_eq!(kill_recorded(&recorded), Cleanup::Verified);
}

#[test]
fn kill_recorded_kills_matching_live_orphan_group() {
    use std::os::unix::process::CommandExt;

    if std::env::var_os("ATRIUM_SUPERVISE_ORPHAN_HELPER").is_some() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("orphan helper spawn sleep");
        println!("READY:{} {}", std::process::id(), child.id());
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let _ = child.wait();
        return;
    }

    let mut launcher = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("kill_recorded_kills_matching_live_orphan_group")
        .arg("--nocapture")
        .arg("--quiet")
        .env("ATRIUM_SUPERVISE_ORPHAN_HELPER", "1")
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("spawn orphan group leader");
    let _launcher_guard = KillPidGuard(launcher.id());

    let mut reader = std::io::BufReader::new(launcher.stdout.take().expect("stdout"));
    let (leader_pid, child_pid): (u32, u32) = loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).expect("read helper stdout");
        assert!(n > 0, "orphan helper exited before READY");
        if let Some(rest) = line.trim().strip_prefix("READY:") {
            let mut parts = rest.split_whitespace();
            break (
                parts.next().unwrap().parse().unwrap(),
                parts.next().unwrap().parse().unwrap(),
            );
        }
    };
    let _group_guard = KillGroupGuard(leader_pid);

    let recorded = identify(leader_pid).unwrap().expect("orphan leader alive");
    assert_eq!(recorded.pgid, leader_pid);
    assert!(pid_exists(child_pid));

    assert_eq!(kill_recorded(&recorded), Cleanup::Verified);
    let _ = launcher.wait();

    for _ in 0..100 {
        if !pid_exists(child_pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(!pid_exists(leader_pid));
    assert!(!pid_exists(child_pid));
}

#[tokio::test]
async fn fast_exiting_process_records_identity_and_terminates_cleanly() {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg("exit 0");
    let mut sup = spawn_group(cmd).expect("spawn_group fast exit");
    assert_eq!(sup.identity.pgid, sup.identity.pid);
    let term = terminate(&mut sup, Duration::from_secs(1))
        .await
        .expect("terminate fast exit");
    assert_eq!(term.exit_code, Some(0));
    assert!(!term.killed);
}
