use std::io::BufRead;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use atrium_host::supervise::{
    ProcessIdentity, identify, kill_recorded, spawn_group, terminate, terminate_blocking,
};
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
    let desc_guard = KillPidGuard(descendant_pid);

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
    // Kill the descendant through the guarded helper (pid > 1).
    drop(desc_guard);
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

    // The launcher plays the role of init: it reaps the killed leader while
    // `kill_recorded` waits for the group to drain (an unreaped zombie keeps
    // the group alive, so `kill_recorded` would rightly say `Unverified`).
    let reaper = std::thread::spawn(move || launcher.wait());
    assert_eq!(kill_recorded(&recorded), Cleanup::Verified);
    let _ = reaper.join();

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
    // The window in which a child is exiting (no executable to read yet) is
    // narrow, so the spawn is repeated to hit it often. Whichever way each
    // spawn lands (a real executable read, or the resolved path of a child
    // that is already gone), it must record a usable identity.
    for round in 0..200 {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("exit 0");
        let mut sup =
            spawn_group(cmd).unwrap_or_else(|e| panic!("spawn_group, round {round}: {e}"));
        assert_eq!(sup.identity.pgid, sup.identity.pid, "round {round}");
        assert!(
            sup.identity.executable.is_absolute(),
            "round {round}: {:?}",
            sup.identity.executable
        );
        let term = terminate(&mut sup, Duration::from_secs(1))
            .await
            .unwrap_or_else(|e| panic!("terminate, round {round}: {e}"));
        assert_eq!(term.exit_code, Some(0), "round {round}");
        assert!(!term.killed, "round {round}");
    }
}

#[test]
fn kill_recorded_never_signals_the_callers_own_process_group() {
    // The identity of this very test process: its pgid is the runner's own
    // group, which must never be signalled.
    let me = identify(std::process::id()).unwrap().expect("own identity");
    assert_eq!(kill_recorded(&me), Cleanup::Unverified);
    // Still alive to assert this.
}

#[test]
fn terminate_blocking_kills_group_with_grandchild_while_spawning_runtime_is_blocked() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime");

    let (mut sup, pid_a, pid_b) = rt.block_on(async {
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
        let stdout = sup.child.stdout.take().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let pid_a: u32 = lines
            .next_line()
            .await
            .unwrap()
            .expect("pid line 1")
            .trim()
            .parse()
            .expect("parse pid_a");
        let pid_b: u32 = lines
            .next_line()
            .await
            .unwrap()
            .expect("pid line 2")
            .trim()
            .parse()
            .expect("parse pid_b");
        (sup, pid_a, pid_b)
    });

    let leader_pid = sup.identity.pid;
    let _guard = KillGroupGuard(sup.identity.pgid);
    for pid in [leader_pid, pid_a, pid_b] {
        assert!(pid_exists(pid), "expected pid {pid} alive before terminate");
    }

    // Keep the spawning `current_thread` runtime alive and blocked in `join()`
    // while another thread terminates and reaps the child without a runtime.
    let term = std::thread::spawn(move || {
        let term = terminate_blocking(&mut sup, Duration::ZERO).expect("terminate_blocking");
        assert!(
            sup.child.id().is_none(),
            "tokio Child must record the leader as reaped"
        );
        drop(sup);
        term
    })
    .join()
    .expect("terminate thread");

    assert!(term.killed);
    assert_eq!(term.exit_code, None);
    for pid in [leader_pid, pid_a, pid_b] {
        assert!(
            !pid_exists(pid),
            "pid {pid} must be gone after terminate_blocking"
        );
        assert_eq!(identify(pid).unwrap(), None);
    }
}

#[test]
fn terminate_blocking_after_spawning_runtime_dropped_handles_early_exit_leader() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime");

    let (mut sup, grandchild_pid) = rt.block_on(async {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("sleep 60 >/dev/null 2>&1 </dev/null & echo $!; exit 7")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut sup = spawn_group(cmd).expect("spawn_group");
        let stdout = sup.child.stdout.take().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let grandchild_pid: u32 = lines
            .next_line()
            .await
            .unwrap()
            .expect("grandchild pid line")
            .trim()
            .parse()
            .expect("parse grandchild_pid");
        assert!(lines.next_line().await.unwrap().is_none());
        (sup, grandchild_pid)
    });
    drop(rt);

    let leader_pid = sup.identity.pid;
    let _guard = KillGroupGuard(sup.identity.pgid);
    assert!(pid_exists(grandchild_pid));

    let term = terminate_blocking(&mut sup, Duration::from_secs(2)).expect("terminate_blocking");
    assert!(!term.killed, "leader exited before grace expired");
    assert_eq!(term.exit_code, Some(7));
    assert!(sup.child.id().is_none());
    drop(sup);

    for pid in [leader_pid, grandchild_pid] {
        assert!(
            !pid_exists(pid),
            "pid {pid} must be gone after terminate_blocking"
        );
        assert_eq!(identify(pid).unwrap(), None);
    }
}

/// A process whose main thread has exited while a worker thread still runs:
/// `/proc/<pid>/stat` shows the thread-group leader as `Z`, the process lives.
#[cfg(target_os = "linux")]
mod leader_exited {
    use std::io::BufRead;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use atrium_host::supervise::{ProcessIdentity, identify, kill_recorded};
    use atrium_protocol::thread::Cleanup;

    use super::{KillGroupGuard, pid_exists};

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "atrium-leader-exited-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // `exit(2)` (not `exit_group`) ends only the calling thread; `pthread_exit`
    // would unwind through Rust's `main` and abort the process.
    const HELPER: &str = r#"
use std::io::Write;

unsafe extern "C" {
    fn syscall(number: i64, ...) -> i64;
}

#[cfg(target_arch = "x86_64")]
const SYS_EXIT: i64 = 60;
#[cfg(target_arch = "aarch64")]
const SYS_EXIT: i64 = 93;

fn main() {
    std::thread::spawn(|| std::thread::sleep(std::time::Duration::from_secs(600)));
    println!("READY {}", std::process::id());
    std::io::stdout().flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    // SAFETY: `exit(2)` ends the calling (main) thread only; the worker
    // thread keeps the process alive.
    unsafe { syscall(SYS_EXIT, 0i64) };
}
"#;

    fn compile_helper(dir: &Path) -> PathBuf {
        let src = dir.join("leader_exits.rs");
        let bin = dir.join("leader_exits");
        std::fs::write(&src, HELPER).expect("write helper source");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let status = Command::new(rustc)
            .arg("--edition=2024")
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .status()
            .expect("invoke rustc");
        assert!(status.success(), "rustc failed compiling the helper");
        bin
    }

    /// The state letter of `/proc/<pid>/stat`.
    fn state_of(pid: u32) -> Option<char> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        text[text.rfind(')')? + 1..].trim_start().chars().next()
    }

    #[test]
    fn a_process_whose_main_thread_exited_is_still_identified_and_killed() {
        let dir = Dir::new();
        let helper = compile_helper(&dir.0);
        let mut child = Command::new(&helper)
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("spawn helper");
        let pid = child.id();
        let _group = KillGroupGuard(pid);
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut line)
            .expect("READY line");
        assert!(line.starts_with("READY"), "got {line:?}");

        // Wait for the main thread to be gone: the leader shows `Z` while the
        // process (its worker thread) is alive.
        let mut zombie_leader = false;
        for _ in 0..200 {
            if state_of(pid) == Some('Z') {
                zombie_leader = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(zombie_leader, "the leader never showed Z");
        // The process itself is alive: its worker thread is still there.
        let threads = std::fs::read_dir(format!("/proc/{pid}/task"))
            .expect("the process has a task folder")
            .count();
        assert!(
            threads >= 2,
            "expected the leader and a worker, got {threads}"
        );
        let expected = std::fs::canonicalize(&helper).unwrap();
        let identity: Option<ProcessIdentity> = identify(pid).unwrap();
        let identity = identity.expect("the process is alive: it has an identity");
        assert_eq!(identity.pid, pid);
        assert_eq!(identity.pgid, pid);
        assert_eq!(identity.executable, expected);

        // The identity matches, so recovery kills the group; the parent
        // (this test) plays init and reaps once the process is gone.
        let reaper = std::thread::spawn(move || child.wait());
        assert_eq!(kill_recorded(&identity), Cleanup::Verified);
        let _ = reaper.join();
        assert!(!pid_exists(pid));
    }
}

// ---- M1-07c (N8): a leader that leaves its group, SIGTERM-ignoring leaders,
// and a group that holds only the zombie leader.

/// `kill(-pgid, 0)`: `true` while the group has any member (a zombie counts).
fn group_exists(pgid: u32) -> bool {
    let Ok(raw) = i32::try_from(pgid) else {
        return false;
    };
    if raw <= 1 {
        return false;
    }
    // SAFETY: Signal 0 only checks existence and permission; `raw > 1`, so
    // `-raw <= -2` never addresses every process or the caller's group 0.
    let rc = unsafe { libc::kill(-raw, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

const LEAVE_GROUP_HELPER_ENV: &str = "ATRIUM_SUPERVISE_LEAVE_GROUP_HELPER";

/// In helper mode (the env var set), this test binary is the provider leader:
/// it moves itself out of the group `spawn_group` gave it, prints `LEFT <pgid>`
/// and sleeps. A process-group leader can't `setsid` (`EPERM`) and
/// `setpgid(0, 0)` would keep it in its own group, so it joins a new group
/// made by a short-lived child of its own, which it then kills and reaps.
/// Without the env var this test does nothing.
#[test]
fn helper_leader_leaves_its_group() {
    use std::os::unix::process::CommandExt;

    if std::env::var_os(LEAVE_GROUP_HELPER_ENV).is_none() {
        return;
    }
    let mut kid = std::process::Command::new("sleep")
        .arg("60")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("helper: spawn group maker");
    let new_pgid = i32::try_from(kid.id()).expect("helper: pid fits i32");
    // SAFETY: `setpgid` has no memory-safety preconditions. It moves only
    // this process into `new_pgid`, the group of its own live child in the
    // same session.
    let rc = unsafe { libc::setpgid(0, new_pgid) };
    assert_eq!(
        rc,
        0,
        "helper: setpgid: {}",
        std::io::Error::last_os_error()
    );
    let _ = kid.kill();
    let _ = kid.wait();
    println!("LEFT {new_pgid}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    // Backstop only: the parent test kills this process long before.
    std::thread::sleep(Duration::from_secs(60));
}

fn leave_group_command() -> Command {
    let mut cmd = Command::new(std::env::current_exe().expect("current_exe"));
    cmd.arg("--exact")
        .arg("helper_leader_leaves_its_group")
        .arg("--nocapture")
        .arg("--quiet")
        .env(LEAVE_GROUP_HELPER_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

/// Reads stdout up to `LEFT <pgid>` and checks the leader really moved.
async fn wait_until_left(sup: &mut atrium_host::supervise::SupervisedChild) -> u32 {
    let stdout = sup.child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let new_pgid: u32 = loop {
        let line = lines
            .next_line()
            .await
            .unwrap()
            .expect("helper exited before LEFT");
        if let Some(rest) = line.trim().strip_prefix("LEFT ") {
            break rest.parse().expect("parse new pgid");
        }
    };
    let old_pgid = sup.identity.pgid;
    assert_ne!(new_pgid, old_pgid);
    let live = identify(sup.identity.pid)
        .unwrap()
        .expect("leader alive after leaving");
    assert_eq!(live.pgid, new_pgid, "the leader must be in its new group");
    assert!(
        !group_exists(old_pgid),
        "the old group must hold nobody now"
    );
    new_pgid
}

#[tokio::test]
async fn terminate_kills_a_leader_that_left_its_group() {
    let mut sup = spawn_group(leave_group_command()).expect("spawn_group");
    let (leader_pid, old_pgid) = (sup.identity.pid, sup.identity.pgid);
    let new_pgid = wait_until_left(&mut sup).await;

    let started = std::time::Instant::now();
    // Bounded: on code that never signals the leader by pid, this times out
    // (and dropping `sup` then kills the leader through `kill_on_drop`).
    let term = tokio::time::timeout(Duration::from_secs(10), terminate(&mut sup, Duration::ZERO))
        .await
        .expect("terminate must not wait for a leader outside its group")
        .expect("terminate");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(term.killed);
    assert_eq!(term.exit_code, None);
    assert!(!pid_exists(leader_pid), "the leader must be gone");
    assert!(!group_exists(old_pgid), "the old group must be gone");
    assert!(
        !group_exists(new_pgid),
        "the leader's new group must be gone"
    );
}

#[test]
fn terminate_blocking_kills_a_leader_that_left_its_group() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime");
    let (mut sup, new_pgid) = rt.block_on(async {
        let mut sup = spawn_group(leave_group_command()).expect("spawn_group");
        let new_pgid = wait_until_left(&mut sup).await;
        (sup, new_pgid)
    });
    let (leader_pid, old_pgid) = (sup.identity.pid, sup.identity.pgid);

    let started = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let term = terminate_blocking(&mut sup, Duration::ZERO);
        let reaped = sup.child.id().is_none();
        let _ = tx.send((term, reaped));
    });
    let Ok((term, reaped)) = rx.recv_timeout(Duration::from_secs(10)) else {
        // Bounded: the worker is blocked reaping the leader, which is still
        // unreaped, so its pid is ours to kill. Then fail.
        drop(KillPidGuard(leader_pid));
        let _ = worker.join();
        panic!("terminate_blocking waited for a leader outside its group");
    };
    worker.join().expect("terminate thread");
    let term = term.expect("terminate_blocking");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(reaped, "tokio Child must record the leader as reaped");
    assert!(term.killed);
    assert_eq!(term.exit_code, None);
    assert!(!pid_exists(leader_pid), "the leader must be gone");
    assert!(!group_exists(old_pgid), "the old group must be gone");
    assert!(
        !group_exists(new_pgid),
        "the leader's new group must be gone"
    );
    drop(rt);
}

/// A leader that ignores `SIGTERM`, with a `sleep` child in its group.
fn ignores_sigterm_command() -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg("trap '' TERM; echo ready; while :; do sleep 1; done")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

/// Waits for `ready`, then sends the soft stop (`SIGTERM`) to the leader,
/// which this test spawned and nobody has reaped.
async fn ready_then_sigterm(sup: &mut atrium_host::supervise::SupervisedChild) {
    let stdout = sup.child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let line = lines.next_line().await.unwrap().expect("ready line");
    assert_eq!(line.trim(), "ready");
    let raw = i32::try_from(sup.identity.pid).unwrap();
    assert!(raw > 1);
    // SAFETY: `raw > 1` is this test's own unreaped child, so the pid can't
    // have been reused; `kill` has no memory-safety preconditions.
    let rc = unsafe { libc::kill(raw, libc::SIGTERM) };
    assert_eq!(rc, 0);
}

const SOFT_GRACE: Duration = Duration::from_millis(300);

#[tokio::test]
async fn terminate_kills_a_leader_ignoring_sigterm_after_the_grace() {
    let mut sup = spawn_group(ignores_sigterm_command()).expect("spawn_group");
    let (leader_pid, pgid) = (sup.identity.pid, sup.identity.pgid);
    ready_then_sigterm(&mut sup).await;

    let started = std::time::Instant::now();
    let term = tokio::time::timeout(Duration::from_secs(10), terminate(&mut sup, SOFT_GRACE))
        .await
        .expect("terminate bounded")
        .expect("terminate");
    let elapsed = started.elapsed();
    assert!(elapsed >= SOFT_GRACE, "returned after {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(5),
        "returned after {elapsed:?}"
    );
    assert!(
        term.killed,
        "the leader ignored SIGTERM: it had to be killed"
    );
    assert_eq!(term.exit_code, None);
    assert!(!pid_exists(leader_pid));
    assert!(!group_exists(pgid));
}

#[test]
fn terminate_blocking_kills_a_leader_ignoring_sigterm_after_the_grace() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime");
    let mut sup = rt.block_on(async {
        let mut sup = spawn_group(ignores_sigterm_command()).expect("spawn_group");
        ready_then_sigterm(&mut sup).await;
        sup
    });
    let (leader_pid, pgid) = (sup.identity.pid, sup.identity.pgid);

    let started = std::time::Instant::now();
    let term = terminate_blocking(&mut sup, SOFT_GRACE).expect("terminate_blocking");
    let elapsed = started.elapsed();
    assert!(elapsed >= SOFT_GRACE, "returned after {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(5),
        "returned after {elapsed:?}"
    );
    assert!(
        term.killed,
        "the leader ignored SIGTERM: it had to be killed"
    );
    assert_eq!(term.exit_code, None);
    assert!(sup.child.id().is_none());
    assert!(!pid_exists(leader_pid));
    assert!(!group_exists(pgid));
    drop(sup);
    drop(rt);
}

/// `waitid(P_PID, WEXITED | WNOHANG | WNOWAIT)`: `true` once child `pid` is a
/// waitable zombie; it is left unreaped.
fn exited_unreaped(pid: u32) -> bool {
    let raw = i32::try_from(pid).unwrap();
    assert!(raw > 1);
    // SAFETY: `siginfo_t` is a C POD valid when all-bits-zero and `&mut info`
    // outlives the call. `WNOWAIT` leaves the zombie for tokio to reap.
    // `si_pid` is read only after `rc == 0`, when `waitid` has either filled
    // the `SIGCHLD` variant or (with `WNOHANG`, nothing to report) left the
    // zeroed `info` as is, so the union read sees initialised bytes.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let rc = libc::waitid(
            libc::P_PID,
            raw as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        );
        assert_eq!(rc, 0, "waitid: {}", std::io::Error::last_os_error());
        #[cfg(target_os = "macos")]
        let si_pid = info.si_pid;
        #[cfg(target_os = "linux")]
        let si_pid = info.si_pid();
        si_pid != 0
    }
}

/// Waits until the group's only member, the leader, is a waitable, unreaped
/// zombie. On macOS that is the state where `kill(-pgid, _)` answers `EPERM`
/// (XNU `killpg1`), so `terminate_blocking` takes its `EPERM` branch; on
/// Linux the group kill succeeds on the zombie.
fn wait_for_zombie_only_group(pid: u32, pgid: u32) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !exited_unreaped(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "the leader never exited"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let raw = i32::try_from(pgid).unwrap();
    assert!(raw > 1);
    // SAFETY: Signal 0 only checks existence and permission; `raw > 1`.
    let rc = unsafe { libc::kill(-raw, 0) };
    let errno = std::io::Error::last_os_error().raw_os_error();
    #[cfg(target_os = "macos")]
    assert!(
        rc != 0 && errno == Some(libc::EPERM),
        "XNU answers EPERM for a group of zombies (rc {rc}, errno {errno:?})"
    );
    #[cfg(target_os = "linux")]
    assert_eq!(rc, 0, "a zombie still counts as a group member ({errno:?})");
}

#[test]
fn terminate_blocking_with_only_the_zombie_leader_in_the_group() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime");
    for round in 0..20 {
        let mut sup = rt.block_on(async {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c")
                .arg("exit 3")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            spawn_group(cmd).expect("spawn_group")
        });
        let (pid, pgid) = (sup.identity.pid, sup.identity.pgid);
        wait_for_zombie_only_group(pid, pgid);

        let term = terminate_blocking(&mut sup, Duration::ZERO)
            .unwrap_or_else(|e| panic!("terminate_blocking, round {round}: {e}"));
        assert!(!term.killed, "round {round}: the leader had exited");
        assert_eq!(term.exit_code, Some(3), "round {round}");
        assert!(sup.child.id().is_none(), "round {round}");
        assert!(!pid_exists(pid), "round {round}");
        assert!(!group_exists(pgid), "round {round}");
    }
}
