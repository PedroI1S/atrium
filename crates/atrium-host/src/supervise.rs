//! Process supervision shared by every adapter and the daemon's crash
//! recovery (contract §10).
//!
//! - Every provider process starts as the leader of its own process group, so
//!   tool descendants are killed with it. Process groups don't contain
//!   descendants that deliberately detach (`setsid`); that's documented, not
//!   solved, until the sandbox.
//! - Identity is pid + start time + executable. A recorded identity that no
//!   longer matches is never killed: the thread becomes
//!   `Interrupted { cleanup: Unverified }`.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use atrium_protocol::thread::Cleanup;
use serde::{Deserialize, Serialize};

/// Enough to recognise a process again after a daemon restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// The process group id; equals `pid` for processes started here.
    pub pgid: u32,
    /// Platform start time (macOS `proc_pidinfo` start, Linux `/proc/<pid>/stat`
    /// field 22), in the platform's units. Only compared for equality.
    pub start_time: u64,
    pub executable: PathBuf,
}

/// A child started by [`spawn_group`].
#[derive(Debug)]
pub struct SupervisedChild {
    pub child: tokio::process::Child,
    pub identity: ProcessIdentity,
}

/// How a supervised process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Termination {
    pub exit_code: Option<i32>,
    /// The group had to be killed after the grace period.
    pub killed: bool,
}

const GROUP_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "macos")]
const MACOS_EPERM_EXIT_POLL: Duration = Duration::from_millis(50);

/// Spawns `command` as the leader of a new process group, with
/// `kill_on_drop`, and records its identity.
pub fn spawn_group(mut command: tokio::process::Command) -> io::Result<SupervisedChild> {
    let raw_program = PathBuf::from(command.as_std().get_program());
    let raw_cwd = command.as_std().get_current_dir().map(PathBuf::from);
    let path_override = command
        .as_std()
        .get_envs()
        .find_map(|(k, v)| (k == "PATH").then(|| v.map(std::ffi::OsString::from)))
        .flatten();

    command.kill_on_drop(true);
    command.process_group(0);

    let mut child = command.spawn()?;
    let Some(pid) = child.id() else {
        return Err(io::Error::other("spawned child has no pid"));
    };

    match identify_spawned(
        pid,
        &raw_program,
        raw_cwd.as_deref(),
        path_override.as_deref(),
    ) {
        Ok(identity) if identity.pgid == pid => Ok(SupervisedChild { child, identity }),
        Ok(identity) => {
            let _ = signal_group(pid, libc::SIGKILL);
            let _ = child.start_kill();
            Err(io::Error::other(format!(
                "spawned child {pid} has pgid {}, expected {pid}",
                identity.pgid
            )))
        }
        Err(err) => {
            let _ = signal_group(pid, libc::SIGKILL);
            let _ = child.start_kill();
            Err(err)
        }
    }
}

/// After the adapter's provider-specific soft stop: waits up to `grace` for
/// the leader, then `SIGKILL`s the whole group and the leader itself (by pid,
/// in case it has moved to another process group), and only then reaps the
/// leader (so neither its pid nor the group id can be reused in between; an
/// early exit is seen with `WNOWAIT`, without reaping). Also kills the group
/// when the leader exited on its own but descendants remain.
///
/// The caller must **not** reap `child.child` itself first (adapters call
/// `terminate`, never `wait` or `try_wait`); reaping the leader early would
/// release its pid and process group id for reuse before they can be
/// signalled. Returns `Ok` only once the process group is verified empty
/// (`ESRCH`).
///
/// How long it takes: up to `grace`, then the reap of the `SIGKILL`ed leader
/// (prompt, unless the kernel holds it, e.g. in uninterruptible sleep), then
/// up to 2 s for the rest of the group to drain. On macOS, a group kill that
/// answers `EPERM` while the leader is exiting adds up to 50 ms
/// (`MACOS_EPERM_EXIT_POLL`).
pub async fn terminate(child: &mut SupervisedChild, grace: Duration) -> io::Result<Termination> {
    let (pid, pgid) = check_unreaped_leader(child)?;
    let exited_in_grace = wait_until_exited_no_reap(pid, grace).await?;
    let macos_zombie_eperm = kill_group_while_unreaped(pid, pgid).await?;
    kill_leader_while_unreaped(&mut child.child)?;

    let status = child.child.wait().await?;
    wait_for_group_drain(pgid, GROUP_DRAIN_TIMEOUT).await;

    finish_termination(pgid, macos_zombie_eperm, status, exited_in_grace)
}

/// Blocking variant of [`terminate`] that does not use any tokio runtime
/// reactor. Safe to call from `Drop` on a plain thread even while the runtime
/// that spawned `child` is blocked or shutting down.
///
/// Follows the same order as [`terminate`]: check for an early exit without
/// reaping (`WNOWAIT`), `SIGKILL` the group and then the leader by pid while
/// the leader is unreaped, wait for and reap the leader without touching a
/// tokio reactor (through `waitid(WNOWAIT)` + [`tokio::process::Child::try_wait`],
/// so `Child` marks itself reaped and `kill_on_drop` does not act again), then
/// verify the group is empty (`ESRCH`).
///
/// The caller must **not** reap `child.child` first, through tokio or
/// otherwise (`waitpid(-1)` included): only a reap through tokio is detected,
/// and any other one releases the pid and group id before they are signalled.
///
/// It blocks the calling thread for up to `grace`, then for the reap of the
/// `SIGKILL`ed leader (prompt, but unbounded if the kernel holds the leader,
/// e.g. in uninterruptible sleep), then for up to 2 s while the rest of the
/// group drains. On macOS, a group kill that answers `EPERM` while the leader
/// is exiting adds up to 50 ms (`MACOS_EPERM_EXIT_POLL`).
pub fn terminate_blocking(child: &mut SupervisedChild, grace: Duration) -> io::Result<Termination> {
    let (pid, pgid) = check_unreaped_leader(child)?;
    let exited_in_grace = wait_until_exited_no_reap_sync(pid, grace)?;
    let macos_zombie_eperm = kill_group_while_unreaped_blocking(pid, pgid)?;
    kill_leader_while_unreaped(&mut child.child)?;

    let status = reap_leader_blocking(&mut child.child, pid)?;
    wait_for_group_drain_sync(pgid, GROUP_DRAIN_TIMEOUT);

    finish_termination(pgid, macos_zombie_eperm, status, exited_in_grace)
}

fn check_unreaped_leader(child: &SupervisedChild) -> io::Result<(u32, u32)> {
    if child.child.id().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "supervised child leader was already reaped before terminate",
        ));
    }
    Ok((child.identity.pid, child.identity.pgid))
}

enum GroupKillOutcome {
    Done,
    #[cfg(target_os = "macos")]
    CheckMacosEperm(io::Error),
}

fn try_kill_group(pgid: u32) -> io::Result<GroupKillOutcome> {
    // Send SIGKILL to the whole process group while the leader is still an
    // unreaped child (pinned in the process table, so `pgid` cannot have been
    // recycled by the kernel yet).
    //
    // XNU quirk (`bsd/kern/kern_sig.c`, `killpg1`): when `pgfind(pgid)` finds
    // the group solely because of exiting or unreaped `SZOMB` members
    // (`nfound == 0`), `kill(-pgid, sig)` returns `EPERM` rather than `0` or
    // `ESRCH`. If the leader is still mid-exit, `waitid(WNOWAIT)` may briefly
    // report `false` before `SZOMB` is visible; callers poll `peek_exited`
    // up to `MACOS_EPERM_EXIT_POLL` before treating `EPERM` as a real error.
    // Once the leader is reaped below, `kill(-pgid, 0)` becomes `ESRCH` if
    // the group truly held only that zombie.
    match signal_group(pgid, libc::SIGKILL) {
        Ok(()) => Ok(GroupKillOutcome::Done),
        Err(err) if err.raw_os_error() == Some(libc::ESRCH) => Ok(GroupKillOutcome::Done),
        #[cfg(target_os = "macos")]
        Err(err) if err.raw_os_error() == Some(libc::EPERM) => {
            Ok(GroupKillOutcome::CheckMacosEperm(err))
        }
        Err(err) => Err(err),
    }
}

async fn kill_group_while_unreaped(
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] pid: u32,
    pgid: u32,
) -> io::Result<bool> {
    match try_kill_group(pgid)? {
        GroupKillOutcome::Done => Ok(false),
        #[cfg(target_os = "macos")]
        GroupKillOutcome::CheckMacosEperm(err) => {
            if wait_until_exited_no_reap(pid, MACOS_EPERM_EXIT_POLL).await? {
                Ok(true)
            } else {
                Err(err)
            }
        }
    }
}

fn kill_group_while_unreaped_blocking(
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] pid: u32,
    pgid: u32,
) -> io::Result<bool> {
    match try_kill_group(pgid)? {
        GroupKillOutcome::Done => Ok(false),
        #[cfg(target_os = "macos")]
        GroupKillOutcome::CheckMacosEperm(err) => {
            if wait_until_exited_no_reap_sync(pid, MACOS_EPERM_EXIT_POLL)? {
                Ok(true)
            } else {
                Err(err)
            }
        }
    }
}

/// `SIGKILL`s the leader by pid, for a leader that has left its process
/// group (`setpgid` after [`spawn_group`]), which the group kill misses. The
/// leader is still an unreaped child of this process, so its pid can't have
/// been reused. Goes through std's `Child::kill` (via tokio's `start_kill`),
/// which sends nothing once the child is reaped. A leader that is already a
/// zombie may answer `ESRCH` (XNU); it is dead, so that is not an error.
fn kill_leader_while_unreaped(child: &mut tokio::process::Child) -> io::Result<()> {
    match child.start_kill() {
        Ok(()) => Ok(()),
        Err(err) if err.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        Err(err) => Err(err),
    }
}

fn finish_termination(
    pgid: u32,
    macos_zombie_eperm: bool,
    status: std::process::ExitStatus,
    exited_in_grace: bool,
) -> io::Result<Termination> {
    if !group_is_empty(pgid) {
        if macos_zombie_eperm {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("process group {pgid} did not empty after SIGKILL"),
        ));
    }

    Ok(Termination {
        exit_code: status.code(),
        killed: !exited_in_grace,
    })
}

/// The identity of a running process, or `None` if there is none with `pid`.
pub fn identify(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    let Ok(raw_pid) = i32::try_from(pid) else {
        return Ok(None);
    };
    if raw_pid <= 0 {
        return Ok(None);
    }
    platform_identify(pid, raw_pid)
}

/// Daemon recovery. If the leader still matches `recorded`, kills the group,
/// waits up to `GROUP_DRAIN_TIMEOUT` for it to drain, and returns `Verified`
/// only if the group is actually empty (`Unverified` if any member remains).
/// If the leader is gone: waits up to `GROUP_DRAIN_TIMEOUT` for the group to
/// drain without signalling it, then returns `Verified` if the group is empty
/// (`kill(-pgid, 0)` fails with `ESRCH`), otherwise `Unverified` and nothing is
/// killed (after a crash the group id may have been reused, and start times
/// can't prove descent). If the pid now belongs to another executable or can't
/// be inspected: `Unverified`, nothing killed.
///
/// This function blocks briefly while waiting for a process group to drain;
/// async callers should run it on `tokio::task::spawn_blocking`.
pub fn kill_recorded(recorded: &ProcessIdentity) -> Cleanup {
    if recorded.pid <= 1
        || recorded.pgid <= 1
        || i32::try_from(recorded.pid).is_err()
        || i32::try_from(recorded.pgid).is_err()
    {
        return Cleanup::Unverified;
    }

    match identify(recorded.pid) {
        Err(_) => Cleanup::Unverified,
        Ok(None) => {
            wait_for_group_drain_sync(recorded.pgid, GROUP_DRAIN_TIMEOUT);
            if group_is_empty(recorded.pgid) {
                Cleanup::Verified
            } else {
                Cleanup::Unverified
            }
        }
        Ok(Some(live)) => {
            if live != *recorded {
                return Cleanup::Unverified;
            }
            match signal_group(recorded.pgid, libc::SIGKILL) {
                Ok(()) => {}
                Err(err) if err.raw_os_error() == Some(libc::ESRCH) => {
                    return Cleanup::Verified;
                }
                Err(_) => return Cleanup::Unverified,
            }
            wait_for_group_drain_sync(recorded.pgid, GROUP_DRAIN_TIMEOUT);
            if group_is_empty(recorded.pgid) {
                Cleanup::Verified
            } else {
                Cleanup::Unverified
            }
        }
    }
}

fn current_pgid() -> libc::pid_t {
    // SAFETY: `getpgrp` takes no arguments, has no preconditions, and is
    // async-signal-safe and thread-safe on POSIX platforms.
    unsafe { libc::getpgrp() }
}

fn signal_group(pgid: u32, sig: libc::c_int) -> io::Result<()> {
    let raw_pgid = i32::try_from(pgid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pgid out of i32 range"))?;
    if raw_pgid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to signal pgid <= 1",
        ));
    }
    if raw_pgid == current_pgid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to signal current process group",
        ));
    }
    // SAFETY: `raw_pgid >= 2` and `raw_pgid != getpgrp()` are checked above, so
    // `-raw_pgid <= -2` cannot overflow `i32`, never addresses `0` or `-1`, and
    // never targets the caller's own process group. `kill` has no memory-safety
    // preconditions.
    let rc = unsafe { libc::kill(-raw_pgid, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn group_is_empty(pgid: u32) -> bool {
    matches!(
        signal_group(pgid, 0),
        Err(err) if err.raw_os_error() == Some(libc::ESRCH)
    )
}

async fn wait_for_group_drain(pgid: u32, max_wait: Duration) {
    let deadline = tokio::time::Instant::now() + max_wait;
    while !group_is_empty(pgid) {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2).min(deadline - now)).await;
    }
}

fn wait_for_group_drain_sync(pgid: u32, max_wait: Duration) {
    let deadline = std::time::Instant::now() + max_wait;
    while !group_is_empty(pgid) {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2).min(deadline - now));
    }
}

async fn wait_until_exited_no_reap(pid: u32, grace: Duration) -> io::Result<bool> {
    if peek_exited(pid)? {
        return Ok(true);
    }
    if grace.is_zero() {
        return Ok(false);
    }
    let deadline = tokio::time::Instant::now() + grace;
    let mut step = Duration::from_millis(1);
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return peek_exited(pid);
        }
        tokio::time::sleep(step.min(deadline - now)).await;
        if peek_exited(pid)? {
            return Ok(true);
        }
        step = (step * 2).min(Duration::from_millis(10));
    }
}

fn wait_until_exited_no_reap_sync(pid: u32, grace: Duration) -> io::Result<bool> {
    if peek_exited(pid)? {
        return Ok(true);
    }
    if grace.is_zero() {
        return Ok(false);
    }
    let deadline = std::time::Instant::now() + grace;
    let mut step = Duration::from_millis(1);
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return peek_exited(pid);
        }
        std::thread::sleep(step.min(deadline - now));
        if peek_exited(pid)? {
            return Ok(true);
        }
        step = (step * 2).min(Duration::from_millis(10));
    }
}

fn parse_raw_pid(pid: u32) -> io::Result<i32> {
    let raw_pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid out of i32 range"))?;
    if raw_pid <= 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid pid"));
    }
    Ok(raw_pid)
}

/// Waits for child `pid` to exit without reaping it (`WNOWAIT`), then reaps
/// through [`tokio::process::Child::try_wait`] so tokio marks the child reaped
/// without touching the spawning runtime's reactor.
fn reap_leader_blocking(
    child: &mut tokio::process::Child,
    pid: u32,
) -> io::Result<std::process::ExitStatus> {
    if let Some(status) = child.try_wait()? {
        return Ok(status);
    }
    let raw_pid = parse_raw_pid(pid)?;
    loop {
        // SAFETY: `siginfo_t` is a C POD struct/union valid when all-bits-zero,
        // and `&mut info` is valid for the duration of `waitid`. `WNOWAIT`
        // leaves the zombie unreaped so `child.try_wait()` remains sole reaper.
        let rc = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(
                libc::P_PID,
                raw_pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc != 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            // `ECHILD` (someone else reaped the leader, e.g. `waitpid(-1)`)
            // is returned as is: std holds no cached status (the
            // `try_wait` above returned `None`), so retrying can't recover it.
            return Err(err);
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
    }
}

/// Checks whether child `pid` has exited without reaping it (`WNOWAIT`).
fn peek_exited(pid: u32) -> io::Result<bool> {
    let raw_pid = parse_raw_pid(pid)?;
    loop {
        // SAFETY: `siginfo_t` is a C POD struct/union valid when all-bits-zero.
        // `&mut info` is valid for the duration of `waitid`, and `WNOWAIT`
        // leaves any zombie unreaped so `tokio::process::Child` remains sole
        // reaper. On Linux, `libc::siginfo_t::si_pid()` is an `unsafe` union
        // field read (`_sifields._sigchld.si_pid`); calling it only when
        // `rc == 0` is sound because `waitid(WEXITED | WNOHANG | WNOWAIT)`
        // either populates the `SIGCHLD` union variant with the child's
        // non-zero PID or (when `WNOHANG` finds the child still running) leaves
        // `info` zero-initialized so `si_pid` reads `0`.
        let (rc, si_pid) = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            let rc = libc::waitid(
                libc::P_PID,
                raw_pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            );
            let si_pid = if rc == 0 {
                #[cfg(target_os = "macos")]
                {
                    info.si_pid
                }
                #[cfg(target_os = "linux")]
                {
                    info.si_pid()
                }
            } else {
                0
            };
            (rc, si_pid)
        };
        if rc == 0 {
            return Ok(si_pid != 0);
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::ECHILD) => return Ok(true),
            _ => return Err(err),
        }
    }
}

#[cfg(target_os = "linux")]
const VFORK_EXEC_POLL: Duration = Duration::from_millis(50);

/// Whether an observed child executable is still the parent's `mm` across a
/// Linux `CLONE_VFORK` wake before `exec_mmap` swaps `tsk->mm`.
#[cfg(any(target_os = "linux", test))]
fn is_pre_exec_vfork_exe(
    observed: &std::path::Path,
    self_exe: &std::path::Path,
    expected_exe: &std::path::Path,
) -> bool {
    observed == self_exe && expected_exe != self_exe
}

#[cfg(target_os = "linux")]
fn identify_after_vfork_exec(
    pid: u32,
    expected_exe: &std::path::Path,
) -> io::Result<Option<ProcessIdentity>> {
    let self_exe = std::fs::read_link("/proc/self/exe").ok();
    let deadline = std::time::Instant::now() + VFORK_EXEC_POLL;
    let mut first_try = true;
    loop {
        let Some(id) = identify(pid)? else {
            return Ok(None);
        };
        let Some(self_exe) = self_exe.as_deref() else {
            return Ok(Some(id));
        };
        if !is_pre_exec_vfork_exe(&id.executable, self_exe, expected_exe) {
            return Ok(Some(id));
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Ok(Some(id));
        }
        if first_try {
            first_try = false;
            std::thread::yield_now();
        } else {
            std::thread::sleep(Duration::from_millis(1).min(deadline - now));
        }
    }
}

fn identify_spawned(
    pid: u32,
    raw_program: &std::path::Path,
    raw_cwd: Option<&std::path::Path>,
    path_override: Option<&std::ffi::OsStr>,
) -> io::Result<ProcessIdentity> {
    let executable = resolve_spawn_executable(raw_program, raw_cwd, path_override);
    #[cfg(target_os = "linux")]
    let id = identify_after_vfork_exec(pid, &executable)?;
    #[cfg(not(target_os = "linux"))]
    let id = identify(pid)?;

    if let Some(id) = id {
        return Ok(id);
    }
    // The child we just spawned has not been waited on yet, so if `identify`
    // returned `None` it has already exited or is exiting (a zombie, or a
    // task that has dropped its memory and so has no executable to read yet).
    // Read its kernel `pgid` and `start_time` from its entry and resolve
    // `raw_program`: this identity's `executable` is a resolved guess, not
    // read from the kernel (a shebang script's real one is its interpreter).
    let raw_pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid out of i32 range"))?;
    let (pgid, start_time) = platform_zombie_timings(raw_pid)?;
    Ok(ProcessIdentity {
        pid,
        pgid,
        start_time,
        executable,
    })
}

fn resolve_spawn_executable(
    program: &std::path::Path,
    cwd: Option<&std::path::Path>,
    path_override: Option<&std::ffi::OsStr>,
) -> PathBuf {
    if program.components().count() > 1 || program.is_absolute() {
        let candidate = if program.is_absolute() {
            program.to_path_buf()
        } else if let Some(base) = cwd {
            base.join(program)
        } else {
            program.to_path_buf()
        };
        return std::fs::canonicalize(&candidate).unwrap_or(candidate);
    }
    let path_var = path_override
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    if let Some(paths) = path_var {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(program);
            if candidate.is_file() {
                return std::fs::canonicalize(&candidate).unwrap_or(candidate);
            }
        }
    }
    program.to_path_buf()
}

#[cfg(target_os = "macos")]
const MACOS_SZOMB: u32 = 5;

#[cfg(target_os = "macos")]
fn proc_bsd_info(raw_pid: i32) -> io::Result<Option<libc::proc_bsdinfo>> {
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: `proc_bsdinfo` contains only integer and fixed-size array fields,
    // so zero-initialization is valid. `proc_pidinfo` with `PROC_PIDTBSDINFO`
    // writes at most `size` bytes to `&mut info`.
    let (ret, info) = unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let ret = libc::proc_pidinfo(
            raw_pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size as libc::c_int,
        );
        (ret, info)
    };
    if ret <= 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(err);
    }
    if (ret as usize) < size {
        return Err(io::Error::other("short proc_bsdinfo response"));
    }
    Ok(Some(info))
}

#[cfg(target_os = "macos")]
fn proc_exe_path(raw_pid: i32) -> io::Result<PathBuf> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is a mutable stack buffer of `PROC_PIDPATHINFO_MAXSIZE`
    // (4096) bytes, matching `proc_pidpath`'s required buffer capacity.
    let ret = unsafe {
        libc::proc_pidpath(
            raw_pid,
            buf.as_mut_ptr().cast::<libc::c_void>(),
            buf.len() as u32,
        )
    };
    if ret <= 0 {
        return Err(io::Error::last_os_error());
    }
    let len = (ret as usize).min(buf.len());
    let slice = match buf[..len].iter().position(|&b| b == 0) {
        Some(nul) => &buf[..nul],
        None => &buf[..len],
    };
    if slice.is_empty() {
        return Err(io::Error::other("empty proc_pidpath"));
    }
    Ok(PathBuf::from(OsStr::from_bytes(slice)))
}

#[cfg(target_os = "macos")]
fn bsd_start_time(info: &libc::proc_bsdinfo) -> u64 {
    info.pbi_start_tvsec
        .saturating_mul(1_000_000)
        .saturating_add(info.pbi_start_tvusec)
}

#[cfg(target_os = "macos")]
fn platform_identify(pid: u32, raw_pid: i32) -> io::Result<Option<ProcessIdentity>> {
    let Some(info) = proc_bsd_info(raw_pid)? else {
        return Ok(None);
    };
    if info.pbi_status == MACOS_SZOMB {
        return Ok(None);
    }
    match proc_exe_path(raw_pid) {
        Ok(executable) => Ok(Some(ProcessIdentity {
            pid,
            pgid: info.pbi_pgid,
            start_time: bsd_start_time(&info),
            executable,
        })),
        Err(err) => {
            if err.raw_os_error() == Some(libc::ESRCH) {
                return Ok(None);
            }
            match proc_bsd_info(raw_pid)? {
                None => Ok(None),
                Some(after) if after.pbi_status == MACOS_SZOMB => Ok(None),
                Some(_) => Err(err),
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn platform_zombie_timings(raw_pid: i32) -> io::Result<(u32, u64)> {
    let info = proc_bsd_info(raw_pid)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "spawned child vanished"))?;
    Ok((info.pbi_pgid, bsd_start_time(&info)))
}

/// `PF_EXITING`: the task is being torn down (`include/linux/sched.h`).
#[cfg(any(target_os = "linux", test))]
const PF_EXITING: u64 = 0x0000_0004;
/// `PF_KTHREAD`: a kernel thread, never a process Atrium started.
#[cfg(any(target_os = "linux", test))]
const PF_KTHREAD: u64 = 0x0020_0000;

/// What Linux `/proc/<pid>/stat` says about a task.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LinuxStat {
    /// Field 3.
    state: char,
    /// Field 9 (`task->flags`).
    flags: u64,
    /// Field 20: the threads of the process (the leader included, even once
    /// it has exited and is waiting for the others).
    num_threads: u64,
    /// Field 5.
    pgid: u32,
    /// Field 22.
    start_time: u64,
}

#[cfg(any(target_os = "linux", test))]
impl LinuxStat {
    /// The task is gone for good. A thread-group leader that has exited (`Z`)
    /// while other threads run is not: the process is alive, and only
    /// `/proc/<pid>/stat`, which reports the leader thread, says otherwise.
    fn is_dead(&self) -> bool {
        match self.state {
            'X' | 'x' => true,
            'Z' => self.num_threads <= 1,
            _ => false,
        }
    }
}

/// Parses Linux `/proc/<pid>/stat`.
#[cfg(any(target_os = "linux", test))]
fn parse_linux_proc_stat(stat_text: &str) -> io::Result<LinuxStat> {
    fn bad(what: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, what)
    }
    let close_paren = stat_text
        .rfind(')')
        .ok_or_else(|| bad("malformed /proc/pid/stat"))?;
    let rest = stat_text[close_paren + 1..].trim_start();
    let mut fields = rest.split_whitespace();
    // Field 3: state
    let state = fields
        .next()
        .ok_or_else(|| bad("missing stat state"))?
        .chars()
        .next()
        .ok_or_else(|| bad("empty stat state"))?;
    // Field 4: ppid (skip), field 5: pgrp
    let _ppid = fields.next();
    let pgid: u32 = fields
        .next()
        .ok_or_else(|| bad("missing stat pgrp"))?
        .parse()
        .map_err(|_| bad("invalid stat pgrp"))?;
    // Skip fields 6..=8 (session, tty_nr, tpgid) to reach field 9 (flags).
    let flags: u64 = fields
        .nth(3)
        .ok_or_else(|| bad("missing stat flags"))?
        .parse()
        .map_err(|_| bad("invalid stat flags"))?;
    // Skip fields 10..=19 (10 fields) to reach field 20 (num_threads), then
    // field 21 to reach field 22 (starttime).
    let num_threads: u64 = fields
        .nth(10)
        .ok_or_else(|| bad("missing stat num_threads"))?
        .parse()
        .map_err(|_| bad("invalid stat num_threads"))?;
    let start_time: u64 = fields
        .nth(1)
        .ok_or_else(|| bad("missing stat starttime"))?
        .parse()
        .map_err(|_| bad("invalid stat starttime"))?;
    Ok(LinuxStat {
        state,
        flags,
        num_threads,
        pgid,
        start_time,
    })
}

/// What to make of an attempt to read `/proc/<pid>/exe`.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExeVerdict {
    /// The executable that was read belongs to the task `first` described.
    Identity,
    /// There is no running process to identify (`Ok(None)`).
    NoProcess,
    /// The answer can't be trusted (`Err`, so cleanup stays `Unverified`).
    Unverifiable,
}

/// The decision behind `platform_identify`, pure so that it is tested on every
/// platform. `exe` is the result of reading `/proc/<pid>/exe` (only the error
/// kind matters); `first` is the stat read before it and `after` the stat read
/// after it (`None` if `/proc/<pid>` is gone by then).
///
/// - Gone or replaced: `after` missing means no process now; a different
///   `start_time` or `pgid` means the pid was reused (or the group changed)
///   while we looked, and nothing read in between can be attributed to one
///   task (`Unverifiable`).
/// - Read succeeded and it is the same task: `Identity`.
/// - Read failed: `NoProcess` if the task is dead (`X`/`x`, or `Z` with no other
///   thread: see `LinuxStat::is_dead`), is exiting with no other thread
///   (`PF_EXITING` with `num_threads <= 1`: it has dropped its `mm`, and with
///   it `exe`, but is not `Z` yet) or is a kernel thread (`PF_KTHREAD`: never
///   has an `exe`) and the error is `ENOENT`. Any other failure of a live
///   task, `ENOENT` of an ordinary live process or a multithreaded process
///   whose leader is exiting included, stays `Unverifiable`.
///
/// Nothing here turns a failure into an `Identity`.
#[cfg(any(target_os = "linux", test))]
fn judge_exe_read(
    exe: Result<(), io::ErrorKind>,
    first: &LinuxStat,
    after: Option<&LinuxStat>,
) -> ExeVerdict {
    let Some(after) = after else {
        return ExeVerdict::NoProcess;
    };
    // The identity is built from `first` (pgid, start time) and the exe read
    // in between, so everything it takes from `first` must still hold.
    if after.start_time != first.start_time || after.pgid != first.pgid {
        return ExeVerdict::Unverifiable;
    }
    match exe {
        Ok(()) => ExeVerdict::Identity,
        Err(_) if after.is_dead() => ExeVerdict::NoProcess,
        Err(io::ErrorKind::NotFound)
            if (after.flags & PF_EXITING != 0 && after.num_threads <= 1)
                || after.flags & PF_KTHREAD != 0 =>
        {
            ExeVerdict::NoProcess
        }
        Err(_) => ExeVerdict::Unverifiable,
    }
}

#[cfg(target_os = "linux")]
fn read_linux_stat(pid: u32) -> io::Result<Option<LinuxStat>> {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => parse_linux_proc_stat(&text).map(Some),
        Err(err)
            if err.kind() == io::ErrorKind::NotFound || err.raw_os_error() == Some(libc::ESRCH) =>
        {
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

/// `/proc/<pid>/exe`. For a thread-group leader that is exiting or has exited
/// while other threads run (`num_threads > 1`), the leader may already have
/// dropped its `mm` so its link is `ENOENT` even while the process lives
/// (whatever `first.state` was): then a live thread's link, which is the same
/// executable (threads share the `mm`), is read instead. If there is none,
/// the leader's own error stands.
#[cfg(target_os = "linux")]
fn read_linux_exe(pid: u32, first: &LinuxStat) -> io::Result<PathBuf> {
    let own = std::fs::read_link(format!("/proc/{pid}/exe"));
    let Err(err) = &own else {
        return own;
    };
    if err.kind() != io::ErrorKind::NotFound || first.num_threads <= 1 {
        return own;
    }
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return own;
    };
    for task in tasks.flatten() {
        if task
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            == Some(pid)
        {
            continue;
        }
        if let Ok(executable) = std::fs::read_link(task.path().join("exe")) {
            return Ok(executable);
        }
    }
    own
}
#[cfg(target_os = "linux")]
fn platform_identify(pid: u32, _raw_pid: i32) -> io::Result<Option<ProcessIdentity>> {
    let Some(first) = read_linux_stat(pid)? else {
        return Ok(None);
    };
    if first.is_dead() {
        return Ok(None);
    }
    let exe = read_linux_exe(pid, &first);
    // Stat and exe are two reads, so the pid could be reused in between.
    // Read stat again and let `judge_exe_read` compare what `first` gave.
    let after = read_linux_stat(pid)?;
    let verdict = judge_exe_read(
        exe.as_ref().map(|_| ()).map_err(io::Error::kind),
        &first,
        after.as_ref(),
    );
    match (verdict, exe) {
        (ExeVerdict::Identity, Ok(executable)) => Ok(Some(ProcessIdentity {
            pid,
            pgid: first.pgid,
            start_time: first.start_time,
            executable,
        })),
        (ExeVerdict::NoProcess, _) => Ok(None),
        (_, Err(err)) => Err(err),
        (_, Ok(_)) => Err(io::Error::other(format!(
            "pid {pid} changed while its identity was being read"
        ))),
    }
}

#[cfg(target_os = "linux")]
fn platform_zombie_timings(raw_pid: i32) -> io::Result<(u32, u64)> {
    let pid = u32::try_from(raw_pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "negative pid"))?;
    let stat = read_linux_stat(pid)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "spawned child vanished"))?;
    Ok((stat.pgid, stat.start_time))
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn linux_stat_parser_handles_spaces_and_parens_in_comm() {
        // From field 3: state S, ppid 1, pgrp 4242, session 4242, tty 0,
        // tpgid -1, flags 4194304 (field 9), ..., starttime 987654321 (field 22).
        let sample = "4242 (weird (comm) name\nline2) S 1 4242 4242 0 -1 4194304 \
            100 0 1 0 10 5 0 0 20 0 3 0 987654321 12345678 100";
        let stat = parse_linux_proc_stat(sample).unwrap();
        assert_eq!(stat.state, 'S');
        assert_eq!(stat.pgid, 4242);
        assert_eq!(stat.flags, 4_194_304);
        assert_eq!(stat.num_threads, 3);
        assert_eq!(stat.start_time, 987_654_321);
    }

    #[test]
    fn linux_stat_parser_rejects_truncated_text() {
        // Ends before field 22.
        let sample = "7 (x) S 1 7 7 0 -1 4194304 100 0 1 0 10 5 0 0 20 0 3 0";
        assert!(parse_linux_proc_stat(sample).is_err());
        assert!(parse_linux_proc_stat("no parenthesis").is_err());
    }
    /// A task as `/proc/<pid>/stat` shows it: one thread, process group 100.
    fn stat(state: char, flags: u64, start_time: u64) -> LinuxStat {
        LinuxStat {
            state,
            flags,
            num_threads: 1,
            pgid: 100,
            start_time,
        }
    }

    /// The same task seen in another process group.
    fn in_group(mut stat: LinuxStat, pgid: u32) -> LinuxStat {
        stat.pgid = pgid;
        stat
    }

    /// The same task with `threads` threads in its process.
    fn with_threads(mut stat: LinuxStat, threads: u64) -> LinuxStat {
        stat.num_threads = threads;
        stat
    }

    const LIVE: u64 = 0x0040_0000; // PF_RANDOMIZE, an ordinary task
    const ENOENT: Result<(), io::ErrorKind> = Err(io::ErrorKind::NotFound);
    const EACCES: Result<(), io::ErrorKind> = Err(io::ErrorKind::PermissionDenied);

    #[test]
    fn dead_states_table() {
        for (state, threads, dead) in [
            ('R', 1, false),
            ('S', 1, false),
            ('D', 1, false),
            ('T', 1, false),
            ('X', 1, true),
            ('x', 1, true),
            ('Z', 1, true),
            ('Z', 0, true),
            // A leader that exited while other threads run: not dead.
            ('Z', 2, false),
            // A dead task is dead whatever its thread count says.
            ('X', 3, true),
            ('x', 3, true),
        ] {
            assert_eq!(
                with_threads(stat(state, LIVE, 500), threads).is_dead(),
                dead,
                "state {state}, {threads} threads"
            );
        }
    }

    #[test]
    fn exe_read_decision_table() {
        use ExeVerdict::{Identity, NoProcess, Unverifiable};
        let first = stat('S', LIVE, 500);
        // The same task, a moment later.
        let same = |state, flags| stat(state, flags, 500);
        // A thread-group leader that has exited while its worker runs.
        let leader_z = with_threads(stat('Z', LIVE | PF_EXITING, 500), 2);
        // A thread-group leader that is mid-exit (not yet Z) while its worker runs.
        let leader_exiting = with_threads(stat('S', LIVE | PF_EXITING, 500), 2);
        let table = [
            // The read worked.
            (
                "exe ok, same task",
                first,
                Ok(()),
                Some(same('S', LIVE)),
                Identity,
            ),
            (
                "exe ok, task now zombie",
                first,
                Ok(()),
                Some(same('Z', LIVE)),
                Identity,
            ),
            (
                "exe ok, pid reused (other start time)",
                first,
                Ok(()),
                Some(stat('S', LIVE, 501)),
                Unverifiable,
            ),
            ("exe ok, /proc entry gone", first, Ok(()), None, NoProcess),
            // The identity is built from `first`, so a changed process group
            // is a different identity, whatever else the second read says.
            (
                "exe ok, process group changed",
                first,
                Ok(()),
                Some(in_group(same('S', LIVE), 9000)),
                Unverifiable,
            ),
            (
                "ENOENT, exiting, process group changed",
                first,
                ENOENT,
                Some(in_group(same('S', LIVE | PF_EXITING), 9000)),
                Unverifiable,
            ),
            (
                "EACCES, zombie, process group changed",
                first,
                EACCES,
                Some(in_group(same('Z', LIVE), 9000)),
                Unverifiable,
            ),
            (
                "exe ok, process group went down",
                first,
                Ok(()),
                Some(in_group(same('S', LIVE), 50)),
                Unverifiable,
            ),
            (
                "ENOENT, kernel thread, process group went down",
                first,
                ENOENT,
                Some(in_group(same('S', PF_KTHREAD), 0)),
                Unverifiable,
            ),
            // ENOENT: only dead, exiting or kernel threads are "no process".
            (
                "ENOENT, exiting",
                first,
                ENOENT,
                Some(same('S', LIVE | PF_EXITING)),
                NoProcess,
            ),
            (
                "ENOENT, zombie",
                first,
                ENOENT,
                Some(same('Z', LIVE)),
                NoProcess,
            ),
            (
                "ENOENT, dead X",
                first,
                ENOENT,
                Some(same('X', LIVE)),
                NoProcess,
            ),
            (
                "ENOENT, kernel thread",
                first,
                ENOENT,
                Some(same('S', PF_KTHREAD)),
                NoProcess,
            ),
            (
                "ENOENT, ordinary live process",
                first,
                ENOENT,
                Some(same('S', LIVE)),
                Unverifiable,
            ),
            ("ENOENT, /proc entry gone", first, ENOENT, None, NoProcess),
            (
                "ENOENT, exiting but pid reused",
                first,
                ENOENT,
                Some(stat('S', LIVE | PF_EXITING, 501)),
                Unverifiable,
            ),
            (
                "ENOENT, zombie but pid reused",
                first,
                ENOENT,
                Some(stat('Z', LIVE, 501)),
                Unverifiable,
            ),
            // A replaced pid is `Unverifiable` before anything else about the
            // new task is looked at (mutation checks: the order of the tests).
            (
                "ENOENT, kernel thread, but pid reused",
                first,
                ENOENT,
                Some(stat('S', PF_KTHREAD, 501)),
                Unverifiable,
            ),
            (
                "EACCES, zombie, but pid reused",
                first,
                EACCES,
                Some(stat('Z', LIVE, 501)),
                Unverifiable,
            ),
            (
                "exe ok, start time went backwards",
                first,
                Ok(()),
                Some(stat('S', LIVE, 499)),
                Unverifiable,
            ),
            (
                "ENOENT, exiting, start time went backwards",
                first,
                ENOENT,
                Some(stat('S', LIVE | PF_EXITING, 499)),
                Unverifiable,
            ),
            (
                "ENOENT, dead x",
                first,
                ENOENT,
                Some(same('x', LIVE)),
                NoProcess,
            ),
            (
                "EACCES, dead x",
                first,
                EACCES,
                Some(same('x', LIVE)),
                NoProcess,
            ),
            (
                "EACCES, dead X",
                first,
                EACCES,
                Some(same('X', LIVE)),
                NoProcess,
            ),
            ("EACCES, /proc entry gone", first, EACCES, None, NoProcess),
            // Other errors stay errors for a live task.
            (
                "EACCES, live",
                first,
                EACCES,
                Some(same('S', LIVE)),
                Unverifiable,
            ),
            (
                "EACCES, exiting",
                first,
                EACCES,
                Some(same('S', LIVE | PF_EXITING)),
                Unverifiable,
            ),
            (
                "EACCES, kernel thread",
                first,
                EACCES,
                Some(same('S', PF_KTHREAD)),
                Unverifiable,
            ),
            // A dead task is gone whatever the error was.
            (
                "EACCES, zombie",
                first,
                EACCES,
                Some(same('Z', LIVE)),
                NoProcess,
            ),
            // A leader that exited while other threads run is judged as a
            // live task, not as a dead one.
            (
                "Z leader with a worker, exe read from the worker",
                leader_z,
                Ok(()),
                Some(leader_z),
                Identity,
            ),
            (
                "Z leader with a worker, no exe, exiting flag",
                leader_z,
                ENOENT,
                Some(leader_z),
                Unverifiable,
            ),
            (
                "exiting leader (not yet Z) with a worker, exe read from the worker",
                leader_exiting,
                Ok(()),
                Some(leader_exiting),
                Identity,
            ),
            (
                "exiting leader (not yet Z) with a worker, no exe",
                with_threads(first, 2),
                ENOENT,
                Some(with_threads(same('S', LIVE | PF_EXITING), 2)),
                Unverifiable,
            ),
            (
                "Z leader with a worker, no exe, no exiting flag",
                leader_z,
                ENOENT,
                Some(with_threads(stat('Z', LIVE, 500), 2)),
                Unverifiable,
            ),
            (
                "Z leader with a worker, EACCES",
                leader_z,
                EACCES,
                Some(leader_z),
                Unverifiable,
            ),
            (
                "Z leader with a worker, pid reused",
                leader_z,
                Ok(()),
                Some(with_threads(stat('Z', LIVE | PF_EXITING, 501), 2)),
                Unverifiable,
            ),
            (
                "Z leader, the worker is gone by the second read",
                leader_z,
                ENOENT,
                Some(stat('Z', LIVE | PF_EXITING, 500)),
                NoProcess,
            ),
            (
                "exiting leader, the worker is gone by the second read",
                with_threads(first, 2),
                ENOENT,
                Some(same('S', LIVE | PF_EXITING)),
                NoProcess,
            ),
            (
                "Z leader, the worker is gone by the second read, EACCES",
                leader_z,
                EACCES,
                Some(stat('Z', LIVE | PF_EXITING, 500)),
                NoProcess,
            ),
        ];
        for (name, first, exe, after, expected) in table {
            assert_eq!(
                judge_exe_read(exe, &first, after.as_ref()),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn vfork_pre_exec_predicate_detects_parent_exe_only() {
        let self_exe = std::path::Path::new("/usr/bin/atriumd");
        let sleep_exe = std::path::Path::new("/usr/bin/sleep");

        assert!(is_pre_exec_vfork_exe(self_exe, self_exe, sleep_exe));
        assert!(!is_pre_exec_vfork_exe(sleep_exe, self_exe, sleep_exe));
        assert!(!is_pre_exec_vfork_exe(self_exe, self_exe, self_exe));
    }
}
