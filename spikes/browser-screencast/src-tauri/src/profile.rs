//! Ownership of a browser profile directory, and cleanup of a browser orphaned by a dead owner.
//!
//! Closing `--remote-debugging-pipe` does NOT make Chromium exit (measured: it ran on for 30 s
//! after its owner was `kill -9`'d), so the next owner has to reap it. Reaping is dangerous
//! (kill the wrong process, or a live owner's browser), so it is gated:
//!
//! 1. **Lock.** `ProfileLock` holds an exclusive `flock` on `<profile>/atrium-owner.lock` for
//!    the owner's lifetime. The kernel drops it when the owner dies, however it dies, so a held
//!    lock proves a live owner and a free lock proves the previous owner is gone. A second
//!    owner is refused (`ProfileError::InUse`). The fd is close-on-exec, so Chromium never
//!    inherits it (an inherited lock would outlive the owner and look like a live one).
//! 2. **Record.** After spawning, the owner writes `atrium-browser.json` with the browser's pid,
//!    start time, executable and profile path.
//! 3. **Verification.** `reap_stale` takes `&ProfileLock` (so it cannot run without owning the
//!    profile) and only signals a pid if all of these still hold: same start time, same
//!    executable, parent is init (orphaned), and its argv contains the exact token
//!    `--user-data-dir=<this profile>`. Anything else is left alone.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const LOCK_FILE: &str = "atrium-owner.lock";
const RECORD_FILE: &str = "atrium-browser.json";

#[derive(Debug)]
pub enum ProfileError {
    /// Another live process owns this profile.
    InUse(PathBuf),
    Io(std::io::Error),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InUse(p) => write!(f, "profile {} is in use by another running Atrium instance", p.display()),
            Self::Io(e) => write!(f, "profile lock: {e}"),
        }
    }
}
impl std::error::Error for ProfileError {}

pub struct ProfileLock {
    _file: File,
    dir: PathBuf,
}

impl ProfileLock {
    /// Creates the profile dir if needed and takes the exclusive lock without waiting.
    pub fn acquire(dir: &Path) -> Result<Self, ProfileError> {
        std::fs::create_dir_all(dir).map_err(ProfileError::Io)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK_FILE))
            .map_err(ProfileError::Io)?;
        // SAFETY: `file` is an open fd for the duration of the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let err = std::io::Error::last_os_error();
            return if err.kind() == std::io::ErrorKind::WouldBlock {
                Err(ProfileError::InUse(dir.to_path_buf()))
            } else {
                Err(ProfileError::Io(err))
            };
        }
        Ok(Self { _file: file, dir: dir.to_path_buf() })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserRecord {
    pub pid: u32,
    /// `ps -o lstart=` of the process when it was recorded (second resolution).
    pub started: String,
    pub exe: String,
    pub user_data_dir: String,
}

fn ps(field: &str, pid: u32) -> Option<String> {
    let out = Command::new("ps").args(["-ww", "-o", &format!("{field}="), "-p", &pid.to_string()]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn exe_of(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        return Some(p.display().to_string());
    }
    ps("comm", pid)
}

/// Describes the live process `pid` (None if it is gone).
pub fn describe(pid: u32, user_data_dir: &Path) -> Option<BrowserRecord> {
    Some(BrowserRecord {
        pid,
        started: ps("lstart", pid)?,
        exe: exe_of(pid)?,
        user_data_dir: user_data_dir.display().to_string(),
    })
}

pub fn write_record(lock: &ProfileLock, record: &BrowserRecord) -> std::io::Result<()> {
    std::fs::write(lock.dir.join(RECORD_FILE), serde_json::to_vec_pretty(record).unwrap_or_default())
}

pub fn clear_record(lock: &ProfileLock) {
    let _ = std::fs::remove_file(lock.dir.join(RECORD_FILE));
}

/// Does `args` (a `ps` command line) contain exactly `--user-data-dir=<dir>` as one argument?
/// A longer path that merely starts with `dir` does not count.
pub fn args_name_profile(args: &str, dir: &str) -> bool {
    let needle = format!("--user-data-dir={dir}");
    args.match_indices(&needle).any(|(i, _)| {
        let before_ok = i == 0 || args[..i].ends_with(' ');
        let after = &args[i + needle.len()..];
        before_ok && (after.is_empty() || after.starts_with(' '))
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reaped {
    /// No record, or the recorded process is already gone.
    Nothing,
    Killed(u32),
    /// A process exists but could not be proven to be our orphan; it was not signalled.
    Refused(&'static str),
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks existence.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Verifies the recorded process is this profile's orphaned browser.
fn verify(record: &BrowserRecord, dir: &Path) -> Result<(), &'static str> {
    if record.pid <= 1 {
        return Err("invalid pid in record");
    }
    if record.user_data_dir != dir.display().to_string() {
        return Err("record is for another profile");
    }
    let now = describe(record.pid, dir).ok_or("process vanished")?;
    if now.started != record.started {
        return Err("start time differs (pid was reused)");
    }
    if now.exe != record.exe {
        return Err("executable differs");
    }
    if ps("ppid", record.pid).as_deref() != Some("1") {
        return Err("process is not orphaned (its parent is alive)");
    }
    let args = ps("args", record.pid).ok_or("cannot read command line")?;
    if !args_name_profile(&args, &record.user_data_dir) {
        return Err("command line does not carry the exact --user-data-dir");
    }
    Ok(())
}

/// Kills the browser a previous (now dead) owner left behind, if and only if it verifies.
pub fn reap_stale(lock: &ProfileLock) -> Reaped {
    let dir = lock.dir();
    let Ok(text) = std::fs::read_to_string(dir.join(RECORD_FILE)) else { return Reaped::Nothing };
    let Ok(record) = serde_json::from_str::<BrowserRecord>(&text) else {
        return Reaped::Refused("unreadable record");
    };
    if !alive(record.pid) {
        return Reaped::Nothing;
    }
    if let Err(why) = verify(&record, dir) {
        return Reaped::Refused(why);
    }
    // SAFETY: the pid was just verified as this profile's orphaned browser.
    unsafe { libc::kill(record.pid as i32, libc::SIGTERM) };
    for _ in 0..50 {
        if !alive(record.pid) {
            return Reaped::Killed(record.pid);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Still alive after 5 s: re-verify (it could have exited and the pid been reused) before SIGKILL.
    if verify(&record, dir).is_ok() {
        // SAFETY: as above, verified again just now.
        unsafe { libc::kill(record.pid as i32, libc::SIGKILL) };
    }
    Reaped::Killed(record.pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn exact_profile_argument_only() {
        let d = "/data/profile";
        assert!(args_name_profile("/x/Helium --headless=new --user-data-dir=/data/profile --no-first-run", d));
        assert!(args_name_profile("/x/Helium --user-data-dir=/data/profile", d));
        // Longer path with the same prefix, editor/shell commands that merely mention it:
        assert!(!args_name_profile("/x/Helium --user-data-dir=/data/profile-other --a", d));
        assert!(!args_name_profile("vim /data/profile/notes.txt", d));
        assert!(!args_name_profile("sh -c sleep 30 #--user-data-dir=/data/profile", d));
        assert!(!args_name_profile("/x/Helium x--user-data-dir=/data/profile", d));
    }

    #[test]
    fn second_live_owner_is_refused_and_the_lock_frees_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = ProfileLock::acquire(dir.path()).unwrap();
        assert!(matches!(ProfileLock::acquire(dir.path()), Err(ProfileError::InUse(_))));
        drop(first);
        assert!(ProfileLock::acquire(dir.path()).is_ok());
    }

    #[test]
    fn lock_is_released_when_the_owner_process_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        // A separate process holds the lock via flock(1)-equivalent: python keeps the fd open.
        let script = format!(
            "import fcntl,os,time\nf=open({:?},'w')\nfcntl.flock(f,fcntl.LOCK_EX)\nprint('locked',flush=True)\ntime.sleep(60)\n",
            dir.path().join(LOCK_FILE)
        );
        let mut child = Command::new("python3")
            .args(["-I", "-c", &script])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut line).unwrap();
        assert_eq!(line.trim(), "locked");
        assert!(matches!(ProfileLock::acquire(dir.path()), Err(ProfileError::InUse(_))), "a live owner holds it");
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ProfileLock::acquire(dir.path()).is_ok(), "SIGKILL of the owner releases the lock");
    }

    /// Starts `argv` detached so that it is reparented to init, like a browser whose owner died.
    fn spawn_orphan(script: &Path, profile: &Path) -> u32 {
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("'{}' --user-data-dir='{}' >/dev/null 2>&1 & echo $!", script.display(), profile.display()))
            .output()
            .unwrap();
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        for _ in 0..50 {
            if ps("ppid", pid).as_deref() == Some("1") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        pid
    }

    fn fake_browser(dir: &Path) -> PathBuf {
        let p = dir.join("fakechrome");
        std::fs::write(&p, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn kill(pid: u32) {
        // SAFETY: test cleanup of a process this test spawned.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }

    #[test]
    fn reaps_a_verified_orphan() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("profile");
        let lock = ProfileLock::acquire(&profile).unwrap();
        // The fake browser keeps its shell alive (no `exec`), so its argv still carries the
        // profile argument, like a real browser's.
        let script = tmp.path().join("fakechrome");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let pid = spawn_orphan(&script, &profile);
        let record = describe(pid, &profile).expect("alive");
        write_record(&lock, &record).unwrap();
        assert_eq!(reap_stale(&lock), Reaped::Killed(pid));
        assert!(!alive(pid) || ps("stat", pid).is_some_and(|s| s.starts_with('Z')));
        kill(pid);
    }

    #[test]
    fn never_signals_an_unrelated_process_whose_command_mentions_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("profile");
        let lock = ProfileLock::acquire(&profile).unwrap();
        // An orphaned shell whose command line contains the path as a comment, not as an argument.
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("sh -c 'sleep 30 #--user-data-dir={}' >/dev/null 2>&1 & echo $!", profile.display()))
            .output()
            .unwrap();
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        // Even with a forged-perfect record (right pid, start time, exe), the argv check refuses.
        let record = describe(pid, &profile).expect("alive");
        write_record(&lock, &record).unwrap();
        assert_eq!(reap_stale(&lock), Reaped::Refused("command line does not carry the exact --user-data-dir"));
        assert!(alive(pid), "the bystander must still be running");
        kill(pid);
    }

    #[test]
    fn refuses_a_reused_pid_a_different_exe_or_a_live_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("profile");
        let lock = ProfileLock::acquire(&profile).unwrap();
        let script = fake_browser(tmp.path());
        let pid = spawn_orphan(&script, &profile);
        let good = describe(pid, &profile).unwrap();

        let mut wrong_start = good.clone();
        wrong_start.started = "Mon Jan  1 00:00:00 2001".into();
        write_record(&lock, &wrong_start).unwrap();
        assert_eq!(reap_stale(&lock), Reaped::Refused("start time differs (pid was reused)"));
        assert!(alive(pid));

        let mut wrong_exe = good.clone();
        wrong_exe.exe = "/Applications/Helium.app/Contents/MacOS/Helium".into();
        write_record(&lock, &wrong_exe).unwrap();
        assert_eq!(reap_stale(&lock), Reaped::Refused("executable differs"));

        let mut other_profile = good.clone();
        other_profile.user_data_dir = "/somewhere/else".into();
        write_record(&lock, &other_profile).unwrap();
        assert_eq!(reap_stale(&lock), Reaped::Refused("record is for another profile"));
        assert!(alive(pid));
        kill(pid);

        // A process that still has a live parent (here: this test process) is not an orphan.
        let mut child = Command::new(&script).arg(format!("--user-data-dir={}", profile.display())).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        write_record(&lock, &describe(child.id(), &profile).unwrap()).unwrap();
        assert_eq!(reap_stale(&lock), Reaped::Refused("process is not orphaned (its parent is alive)"));
        assert!(alive(child.id()));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn missing_garbage_and_dead_records_are_harmless() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = ProfileLock::acquire(&tmp.path().join("p")).unwrap();
        assert_eq!(reap_stale(&lock), Reaped::Nothing);
        std::fs::write(lock.dir().join(RECORD_FILE), "not json").unwrap();
        assert!(matches!(reap_stale(&lock), Reaped::Refused(_)));
        let dead = BrowserRecord { pid: 999_999, started: "x".into(), exe: "y".into(), user_data_dir: lock.dir().display().to_string() };
        write_record(&lock, &dead).unwrap();
        assert_eq!(reap_stale(&lock), Reaped::Nothing);
    }
}
