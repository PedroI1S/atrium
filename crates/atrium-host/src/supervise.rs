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
        Ok(identity) => Ok(SupervisedChild { child, identity }),
        Err(err) => {
            let _ = signal_group(pid, libc::SIGKILL);
            let _ = child.start_kill();
            Err(err)
        }
    }
}

/// After the adapter's provider-specific soft stop: waits up to `grace` for
/// the leader, then `SIGKILL`s the whole group, and only then reaps the
/// leader (so the group id can't be reused in between; use `WNOWAIT` to see
/// an early exit without reaping). Also kills the group when the leader
/// exited on its own but descendants remain.
pub async fn terminate(child: &mut SupervisedChild, grace: Duration) -> io::Result<Termination> {
    if child.child.id().is_none() {
        let status = child.child.wait().await?;
        return Ok(Termination {
            exit_code: status.code(),
            killed: false,
        });
    }

    let pid = child.identity.pid;
    let pgid = child.identity.pgid;

    let exited_in_grace = wait_until_exited_no_reap(pid, grace).await?;

    // Send SIGKILL to the whole process group while the leader is still an
    // unreaped child (pinned in the process table, so `pgid` cannot have been
    // recycled by the kernel yet).
    //
    // XNU quirk (`bsd/kern/kern_sig.c`, `killpg1`): when `pgfind(pgid)` finds
    // the group solely because of unreaped `SZOMB` members (`nfound == 0`),
    // `kill(-pgid, sig)` returns `EPERM` rather than `0` or `ESRCH`. Once the
    // leader is reaped below, `kill(-pgid, 0)` becomes `ESRCH` if the group
    // truly held only that zombie.
    let macos_zombie_eperm = match signal_group(pgid, libc::SIGKILL) {
        Ok(()) => false,
        Err(err) if err.raw_os_error() == Some(libc::ESRCH) => false,
        #[cfg(target_os = "macos")]
        Err(err) if err.raw_os_error() == Some(libc::EPERM) && peek_exited(pid)? => true,
        Err(err) => return Err(err),
    };

    let status = child.child.wait().await?;
    wait_for_group_drain(pgid, Duration::from_millis(500)).await;

    if macos_zombie_eperm && !group_is_empty(pgid) {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
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

/// Daemon recovery. If the leader still matches `recorded`, kills the group
/// and returns `Verified`. If the leader is gone: `Verified` if the group is
/// empty too (`kill(-pgid, 0)` fails with `ESRCH`), otherwise `Unverified`
/// and nothing is killed (after a crash the group id may have been reused,
/// and start times can't prove descent). If the pid now belongs to another
/// executable or can't be inspected: `Unverified`, nothing killed.
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
            wait_for_group_drain_sync(recorded.pgid, Duration::from_millis(500));
            Cleanup::Verified
        }
    }
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
    // SAFETY: `raw_pgid >= 2` is checked above, so `-raw_pgid <= -2` cannot
    // overflow `i32` and strictly addresses process group `raw_pgid` (never `0`
    // or `-1`). `kill` has no memory-safety preconditions.
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

/// Checks whether child `pid` has exited without reaping it (`WNOWAIT`).
fn peek_exited(pid: u32) -> io::Result<bool> {
    let raw_pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid out of i32 range"))?;
    if raw_pid <= 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid pid"));
    }
    loop {
        // SAFETY: `siginfo_t` is a C pod struct/union valid when zero-initialized.
        // `&mut info` is valid for the duration of `waitid`. `WNOWAIT` leaves
        // the zombie unreaped so `tokio::process::Child` remains sole reaper.
        let (rc, si_pid) = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            let rc = libc::waitid(
                libc::P_PID,
                raw_pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            );
            #[cfg(target_os = "macos")]
            let si_pid = info.si_pid;
            #[cfg(target_os = "linux")]
            let si_pid = info.si_pid();
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

fn identify_spawned(
    pid: u32,
    raw_program: &std::path::Path,
    raw_cwd: Option<&std::path::Path>,
    path_override: Option<&std::ffi::OsStr>,
) -> io::Result<ProcessIdentity> {
    if let Some(id) = identify(pid)? {
        return Ok(id);
    }
    // The child we just spawned has not been waited on yet, so if `identify`
    // returned `None` it has already exited into a zombie state before its
    // executable vnode could be queried. Read its kernel `pgid` and
    // `start_time` from the zombie entry and resolve `raw_program`.
    let raw_pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid out of i32 range"))?;
    let (pgid, start_time) = platform_zombie_timings(raw_pid)?;
    let executable = resolve_spawn_executable(raw_program, raw_cwd, path_override);
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

/// Parses Linux `/proc/<pid>/stat` into `(state, pgid, start_time_field_22)`.
#[cfg(any(target_os = "linux", test))]
fn parse_linux_proc_stat(stat_text: &str) -> io::Result<(char, u32, u64)> {
    let close_paren = stat_text
        .rfind(')')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed /proc/pid/stat"))?;
    let rest = stat_text[close_paren + 1..].trim_start();
    let mut fields = rest.split_whitespace();
    // Field 3: state
    let state_str = fields
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing stat state"))?;
    let state = state_str
        .chars()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty stat state"))?;
    // Field 4: ppid (skip), Field 5: pgrp
    let _ppid = fields.next();
    let pgid: u32 = fields
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing stat pgrp"))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid stat pgrp"))?;
    // Skip fields 6..=21 (16 fields) to reach field 22 (starttime).
    let start_str = fields
        .nth(16)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing stat starttime"))?;
    let start_time: u64 = start_str
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid stat starttime"))?;
    Ok((state, pgid, start_time))
}

#[cfg(target_os = "linux")]
fn read_linux_stat(pid: u32) -> io::Result<Option<(char, u32, u64)>> {
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

#[cfg(target_os = "linux")]
fn is_dead_linux_state(state: char) -> bool {
    matches!(state, 'Z' | 'X' | 'x')
}

#[cfg(target_os = "linux")]
fn platform_identify(pid: u32, _raw_pid: i32) -> io::Result<Option<ProcessIdentity>> {
    let Some((state, pgid, start_time)) = read_linux_stat(pid)? else {
        return Ok(None);
    };
    if is_dead_linux_state(state) {
        return Ok(None);
    }
    match std::fs::read_link(format!("/proc/{pid}/exe")) {
        Ok(executable) => Ok(Some(ProcessIdentity {
            pid,
            pgid,
            start_time,
            executable,
        })),
        Err(err) => match read_linux_stat(pid)? {
            None => Ok(None),
            Some((after_state, _, _)) if is_dead_linux_state(after_state) => Ok(None),
            Some(_) => Err(err),
        },
    }
}

#[cfg(target_os = "linux")]
fn platform_zombie_timings(raw_pid: i32) -> io::Result<(u32, u64)> {
    let pid = u32::try_from(raw_pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "negative pid"))?;
    let (_, pgid, start_time) = read_linux_stat(pid)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "spawned child vanished"))?;
    Ok((pgid, start_time))
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn linux_stat_parser_handles_spaces_and_parens_in_comm() {
        let sample = "4242 (weird (comm) name\nline2) S 1 4242 4242 0 -1 4194304 \
            100 0 1 0 10 5 0 0 20 0 3 0 987654321 12345678 100";
        let (state, pgid, start) = parse_linux_proc_stat(sample).unwrap();
        assert_eq!(state, 'S');
        assert_eq!(pgid, 4242);
        assert_eq!(start, 987_654_321);
    }
}
