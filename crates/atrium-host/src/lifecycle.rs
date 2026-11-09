//! Daemon lifecycle (PLAN §3.1, contract §10).

use std::path::{Path, PathBuf};

/// Held for the daemon's whole life. Taken *before* a stale socket is removed.
#[derive(Debug)]
pub struct InstanceLock {
    _file: std::fs::File,
    socket: PathBuf,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // `_file` drops after `Drop::drop` returns, so the exclusive `flock` is
        // still held while unlinking our socket.
        let _ = std::fs::remove_file(&self.socket);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("another daemon is running (pid {0:?})")]
    AlreadyRunning(Option<u32>),
    /// The runtime directory is a symlink, not owned by the user, or not 0700.
    #[error("unsafe runtime directory {0}: {1}")]
    UnsafeDir(PathBuf, String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// `~/Library/Application Support/Atrium` on macOS, `$XDG_RUNTIME_DIR/atrium`
/// (socket, lock) and `$XDG_DATA_HOME/atrium` (store) on Linux. `ATRIUM_HOME`,
/// when set, replaces both roots (tests and development only; release
/// daemons ignore it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub runtime_dir: PathBuf,
    pub data_dir: PathBuf,
    pub socket: PathBuf,
    /// Holds the daemon's `ProcessIdentity` (JSON) while it runs, so an
    /// upgraded app can find and verify it.
    pub lock: PathBuf,
    /// `data_dir/atrium.db` (+ WAL).
    pub store: PathBuf,
    pub snapshots_dir: PathBuf,
    pub logs_dir: PathBuf,
    /// `data_dir/app-identity.json`, used only by development builds.
    pub app_identity: PathBuf,
}

pub fn default_paths() -> std::io::Result<Paths> {
    #[cfg(debug_assertions)]
    if let Some(atrium_home) = std::env::var_os("ATRIUM_HOME").filter(|v| !v.is_empty()) {
        let root = PathBuf::from(atrium_home);
        return Ok(paths_from_roots(root.clone(), root));
    }

    default_platform_paths()
}

/// Creates the directory 0700 if missing; refuses a symlink, another owner
/// or a looser mode.
pub fn secure_dir(dir: &Path) -> Result<(), LifecycleError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

    let my_uid = crate::current_euid();

    match std::fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            match std::fs::DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => {
                    let dir_fd = std::fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                        .open(dir)?;
                    let fd_meta = dir_fd.metadata()?;
                    if fd_meta.uid() != my_uid {
                        return Err(LifecycleError::UnsafeDir(
                            dir.to_path_buf(),
                            format!("owned by uid {}, expected {my_uid}", fd_meta.uid()),
                        ));
                    }
                    dir_fd.set_permissions(std::fs::Permissions::from_mode(0o700))?;
                }
                Err(ce) if ce.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(ce) => return Err(LifecycleError::Io(ce)),
            }
        }
        Err(e) => return Err(LifecycleError::Io(e)),
    }

    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Err(LifecycleError::UnsafeDir(
            dir.to_path_buf(),
            "directory is a symlink".into(),
        ));
    }
    if !meta.is_dir() {
        return Err(LifecycleError::UnsafeDir(
            dir.to_path_buf(),
            "path is not a directory".into(),
        ));
    }
    if meta.uid() != my_uid {
        return Err(LifecycleError::UnsafeDir(
            dir.to_path_buf(),
            format!("owned by uid {}, expected {my_uid}", meta.uid()),
        ));
    }
    let mode = meta.mode() & 0o7777;
    if mode != 0o700 {
        return Err(LifecycleError::UnsafeDir(
            dir.to_path_buf(),
            format!("permissions {mode:04o}, expected 0700"),
        ));
    }
    Ok(())
}

impl InstanceLock {
    pub fn acquire(paths: &Paths) -> Result<Self, LifecycleError> {
        use std::io::{Read, Seek, SeekFrom, Write};
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        secure_dir(&paths.runtime_dir)?;
        if let Some(lock_parent) = paths.lock.parent()
            && !lock_parent.as_os_str().is_empty()
            && lock_parent != paths.runtime_dir
        {
            secure_dir(lock_parent)?;
        }

        let my_uid = crate::current_euid();
        if let Ok(existing) = std::fs::symlink_metadata(&paths.lock) {
            if existing.file_type().is_symlink() {
                return Err(LifecycleError::UnsafeDir(
                    paths.lock.clone(),
                    "lock file is a symlink".into(),
                ));
            }
            if !existing.is_file() {
                return Err(LifecycleError::UnsafeDir(
                    paths.lock.clone(),
                    "lock path is not a regular file".into(),
                ));
            }
            if existing.uid() != my_uid {
                return Err(LifecycleError::UnsafeDir(
                    paths.lock.clone(),
                    format!(
                        "lock file owned by uid {}, expected {my_uid}",
                        existing.uid()
                    ),
                ));
            }
        }

        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&paths.lock)
        {
            Ok(f) => f,
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                return Err(LifecycleError::UnsafeDir(
                    paths.lock.clone(),
                    "lock file is a symlink".into(),
                ));
            }
            Err(e) => return Err(LifecycleError::Io(e)),
        };

        let fd_meta = file.metadata()?;
        if !fd_meta.is_file() {
            return Err(LifecycleError::UnsafeDir(
                paths.lock.clone(),
                "lock path is not a regular file".into(),
            ));
        }
        if fd_meta.uid() != my_uid {
            return Err(LifecycleError::UnsafeDir(
                paths.lock.clone(),
                format!(
                    "lock file owned by uid {}, expected {my_uid}",
                    fd_meta.uid()
                ),
            ));
        }

        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                let mut buf = Vec::new();
                let _ = file.seek(SeekFrom::Start(0));
                let _ = (&file).take(8192).read_to_end(&mut buf);
                let holder_pid = serde_json::from_slice::<crate::supervise::ProcessIdentity>(&buf)
                    .ok()
                    .map(|id| id.pid);
                return Err(LifecycleError::AlreadyRunning(holder_pid));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(LifecycleError::Io(e)),
        }

        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;

        let my_identity = crate::supervise::identify(std::process::id())?.ok_or_else(|| {
            std::io::Error::other("cannot determine current daemon ProcessIdentity")
        })?;
        let json = serde_json::to_vec(&my_identity).map_err(std::io::Error::other)?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&json)?;
        file.sync_data()?;

        // Stale socket cleanup runs strictly AFTER taking the exclusive lock.
        match std::fs::remove_file(&paths.socket) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(LifecycleError::Io(e)),
        }

        Ok(Self {
            _file: file,
            socket: paths.socket.clone(),
        })
    }
}

/// Starts `atriumd` detached (new session, stdio to the log file), for the
/// app when it finds no daemon. The log is opened with `O_NOFOLLOW`, created
/// mode 0600, and refused if it exists with another owner or a looser mode.
/// Returns its pid.
///
/// `atriumd` handles `SIGTERM` by exiting once no turn is running, and
/// `SIGINT` by interrupting running turns (resumable) and exiting now.
pub fn spawn_detached(executable: &Path, args: &[String], log: &Path) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let stdout_log = open_private_log(log)?;
    let stderr_log = stdout_log.try_clone()?;

    let mut cmd = Command::new(executable);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));

    // SAFETY: `setsid` is async-signal-safe (POSIX.1-2008), takes no arguments,
    // and touches no Rust heap or shared state between `fork` and `exec`.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd.spawn()?;
    let pid = child.id();

    // Reap in the background if the daemon exits while this caller is still
    // running, without killing the daemon if this caller exits first.
    let _ = std::thread::Builder::new()
        .name("atriumd-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        });

    Ok(pid)
}

/// "Keep agents running after quitting": a launchd LaunchAgent (macOS) or a
/// systemd user unit (Linux) that starts `executable` at login.
pub fn install_keep_running(executable: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let home = resolve_home_dir()?;
        install_launchd_agent(&home, executable)
    }
    #[cfg(target_os = "linux")]
    {
        let config_home = resolve_linux_config_home()?;
        install_systemd_unit(&config_home, executable)
    }
}

pub fn remove_keep_running() -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let home = resolve_home_dir()?;
        remove_launchd_agent(&home)
    }
    #[cfg(target_os = "linux")]
    {
        let config_home = resolve_linux_config_home()?;
        remove_systemd_unit(&config_home)
    }
}

fn paths_from_roots(runtime_dir: PathBuf, data_dir: PathBuf) -> Paths {
    Paths {
        socket: runtime_dir.join("daemon.sock"),
        lock: runtime_dir.join("daemon.lock"),
        store: data_dir.join("atrium.db"),
        snapshots_dir: data_dir.join("snapshots"),
        logs_dir: data_dir.join("logs"),
        app_identity: data_dir.join("app-identity.json"),
        runtime_dir,
        data_dir,
    }
}

fn resolve_home_dir() -> std::io::Result<PathBuf> {
    if let Some(h) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(h));
    }
    std::env::home_dir()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "cannot determine user home directory",
            )
        })
}

#[cfg(target_os = "macos")]
fn default_platform_paths() -> std::io::Result<Paths> {
    let home = resolve_home_dir()?;
    let root = home.join("Library/Application Support/Atrium");
    Ok(paths_from_roots(root.clone(), root))
}

#[cfg(target_os = "linux")]
fn default_platform_paths() -> std::io::Result<Paths> {
    use std::os::unix::fs::MetadataExt;

    let home = resolve_home_dir()?;
    let data_dir = env_abs_dir("XDG_DATA_HOME")
        .unwrap_or_else(|| home.join(".local/share"))
        .join("atrium");

    let uid = crate::current_euid();
    let runtime_dir = if let Some(xdg_runtime) = env_abs_dir("XDG_RUNTIME_DIR") {
        xdg_runtime.join("atrium")
    } else {
        let run_user = PathBuf::from(format!("/run/user/{uid}"));
        match std::fs::symlink_metadata(&run_user) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() && m.uid() == uid => {
                run_user.join("atrium")
            }
            _ => data_dir.clone(),
        }
    };

    Ok(paths_from_roots(runtime_dir, data_dir))
}

#[cfg(target_os = "linux")]
fn env_abs_dir(var: &str) -> Option<PathBuf> {
    let raw = std::env::var_os(var).filter(|v| !v.is_empty())?;
    let p = PathBuf::from(raw);
    p.is_absolute().then_some(p)
}

#[cfg(target_os = "linux")]
fn resolve_linux_config_home() -> std::io::Result<PathBuf> {
    if let Some(cfg) = env_abs_dir("XDG_CONFIG_HOME") {
        return Ok(cfg);
    }
    Ok(resolve_home_dir()?.join(".config"))
}

fn open_private_log(log: &Path) -> std::io::Result<std::fs::File> {
    use std::io::ErrorKind;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

    if let Some(parent) = log.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }

    let my_uid = crate::current_euid();

    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .append(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(log)
        {
            Ok(file) => {
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
                return Ok(file);
            }
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                return Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "log path is a symlink",
                ));
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let lmeta = std::fs::symlink_metadata(log)?;
                if lmeta.file_type().is_symlink() {
                    return Err(std::io::Error::new(
                        ErrorKind::PermissionDenied,
                        "log path is a symlink",
                    ));
                }
                if !lmeta.is_file() {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidInput,
                        "log path is not a regular file",
                    ));
                }
                let file = match std::fs::OpenOptions::new()
                    .append(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(log)
                {
                    Ok(f) => f,
                    Err(oe) if oe.kind() == ErrorKind::NotFound => continue,
                    Err(oe) if oe.raw_os_error() == Some(libc::ELOOP) => {
                        return Err(std::io::Error::new(
                            ErrorKind::PermissionDenied,
                            "log path is a symlink",
                        ));
                    }
                    Err(oe) => return Err(oe),
                };
                let meta = file.metadata()?;
                if !meta.is_file() {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidInput,
                        "log path is not a regular file",
                    ));
                }
                if meta.uid() != my_uid {
                    return Err(std::io::Error::new(
                        ErrorKind::PermissionDenied,
                        format!("log file owned by uid {}, expected {my_uid}", meta.uid()),
                    ));
                }
                let mode = meta.mode() & 0o7777;
                if mode != 0o600 {
                    return Err(std::io::Error::new(
                        ErrorKind::PermissionDenied,
                        format!("log file permissions {mode:04o}, expected 0600"),
                    ));
                }
                return Ok(file);
            }
            Err(e) => return Err(e),
        }
    }

    Err(std::io::Error::other("concurrent modification of log path"))
}

#[cfg(any(target_os = "macos", test))]
const LAUNCHD_LABEL: &str = "dev.atrium.daemon";
#[cfg(any(target_os = "macos", test))]
const LAUNCHD_PLIST_FILE: &str = "dev.atrium.daemon.plist";
#[cfg(any(target_os = "linux", test))]
const SYSTEMD_UNIT_FILE: &str = "atriumd.service";

fn validate_executable_str(executable: &Path) -> std::io::Result<&str> {
    if executable.as_os_str().is_empty() || !executable.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "service executable path must be absolute",
        ));
    }
    let s = executable.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "service executable path is not valid UTF-8",
        )
    })?;
    if s.contains('\0') || s.contains('\n') || s.contains('\r') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "service executable path contains control characters",
        ));
    }
    Ok(s)
}

#[cfg(any(target_os = "macos", test))]
fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(any(target_os = "macos", test))]
fn render_launchd_plist(executable: &Path) -> std::io::Result<String> {
    let exe_str = validate_executable_str(executable)?;
    let escaped = xml_escape(exe_str);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
             <key>Label</key>\n\
             <string>{LAUNCHD_LABEL}</string>\n\
             <key>ProgramArguments</key>\n\
             <array>\n\
                 <string>{escaped}</string>\n\
             </array>\n\
             <key>RunAtLoad</key>\n\
             <true/>\n\
             <key>KeepAlive</key>\n\
             <dict>\n\
                 <key>SuccessfulExit</key>\n\
                 <false/>\n\
             </dict>\n\
         </dict>\n\
         </plist>\n"
    ))
}

#[cfg(any(target_os = "linux", test))]
fn systemd_quote(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("$$"),
            '%' => out.push_str("%%"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(any(target_os = "linux", test))]
fn render_systemd_unit(executable: &Path) -> std::io::Result<String> {
    let exe_str = validate_executable_str(executable)?;
    let quoted = systemd_quote(exe_str);
    Ok(format!(
        "[Unit]\n\
         Description=Atrium background daemon\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={quoted}\n\
         Restart=on-failure\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    ))
}

fn write_atomic_no_symlink(dest: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    if let Ok(meta) = std::fs::symlink_metadata(dest)
        && meta.file_type().is_symlink()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing to overwrite symlink {}", dest.display()),
        ));
    }

    let parent = dest.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination has no parent",
        )
    })?;
    std::fs::create_dir_all(parent)?;

    let file_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("service");
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{file_name}.tmp.{}.{seq}", std::process::id()));

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)?;
    let write_res = (|| -> std::io::Result<()> {
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(contents)?;
        f.sync_data()?;
        Ok(())
    })();
    drop(f);
    if let Err(err) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(any(target_os = "macos", test))]
fn install_launchd_agent(home: &Path, executable: &Path) -> std::io::Result<()> {
    let plist = render_launchd_plist(executable)?;
    let dest = home.join("Library/LaunchAgents").join(LAUNCHD_PLIST_FILE);
    write_atomic_no_symlink(&dest, plist.as_bytes())
}

#[cfg(any(target_os = "macos", test))]
fn remove_launchd_agent(home: &Path) -> std::io::Result<()> {
    let dest = home.join("Library/LaunchAgents").join(LAUNCHD_PLIST_FILE);
    remove_if_exists(&dest)
}

#[cfg(any(target_os = "linux", test))]
fn install_systemd_unit(config_home: &Path, executable: &Path) -> std::io::Result<()> {
    let unit = render_systemd_unit(executable)?;
    let user_dir = config_home.join("systemd/user");
    let unit_path = user_dir.join(SYSTEMD_UNIT_FILE);
    write_atomic_no_symlink(&unit_path, unit.as_bytes())?;

    let wants_dir = user_dir.join("default.target.wants");
    std::fs::create_dir_all(&wants_dir)?;
    let link_path = wants_dir.join(SYSTEMD_UNIT_FILE);
    remove_if_exists(&link_path)?;
    std::os::unix::fs::symlink(&unit_path, &link_path)?;
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn remove_systemd_unit(config_home: &Path) -> std::io::Result<()> {
    let user_dir = config_home.join("systemd/user");
    let link_path = user_dir
        .join("default.target.wants")
        .join(SYSTEMD_UNIT_FILE);
    let unit_path = user_dir.join(SYSTEMD_UNIT_FILE);
    remove_if_exists(&link_path)?;
    remove_if_exists(&unit_path)?;
    Ok(())
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn launchd_plist_and_systemd_unit_preserve_unrelated_files() {
        let tmp = std::env::temp_dir().join(format!(
            "atrium-lifecycle-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        let fake_home = tmp.join("home");
        let existing_plist = fake_home.join("Library/LaunchAgents/com.example.other.plist");
        std::fs::create_dir_all(existing_plist.parent().unwrap()).unwrap();
        std::fs::write(&existing_plist, "keep me").unwrap();

        let exe = Path::new("/Applications/Atrium & Co.app/Contents/MacOS/atriumd");
        install_launchd_agent(&fake_home, exe).unwrap();
        let plist_path = fake_home
            .join("Library/LaunchAgents")
            .join(LAUNCHD_PLIST_FILE);
        let text = std::fs::read_to_string(&plist_path).unwrap();
        assert!(text.contains("<string>dev.atrium.daemon</string>"));
        assert!(text.contains("/Applications/Atrium &amp; Co.app/Contents/MacOS/atriumd"));
        assert_eq!(std::fs::read_to_string(&existing_plist).unwrap(), "keep me");

        remove_launchd_agent(&fake_home).unwrap();
        assert!(!plist_path.exists());
        remove_launchd_agent(&fake_home).unwrap();
        assert_eq!(std::fs::read_to_string(&existing_plist).unwrap(), "keep me");

        let fake_cfg = tmp.join("config");
        let other_unit = fake_cfg.join("systemd/user/other.service");
        let other_want = fake_cfg.join("systemd/user/default.target.wants/other.service");
        std::fs::create_dir_all(other_want.parent().unwrap()).unwrap();
        std::fs::write(&other_unit, "[Unit]\nDescription=Other\n").unwrap();
        std::os::unix::fs::symlink(&other_unit, &other_want).unwrap();

        install_systemd_unit(&fake_cfg, Path::new("/opt/atrium bin/atriumd")).unwrap();
        let unit_path = fake_cfg.join("systemd/user").join(SYSTEMD_UNIT_FILE);
        let want_path = fake_cfg
            .join("systemd/user/default.target.wants")
            .join(SYSTEMD_UNIT_FILE);
        let unit_text = std::fs::read_to_string(&unit_path).unwrap();
        assert!(unit_text.contains("ExecStart=\"/opt/atrium bin/atriumd\""));
        assert!(
            want_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_link(&want_path).unwrap(), unit_path);
        assert!(other_unit.exists());
        assert!(
            other_want
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );

        remove_systemd_unit(&fake_cfg).unwrap();
        assert!(unit_path.symlink_metadata().is_err());
        assert!(want_path.symlink_metadata().is_err());
        remove_systemd_unit(&fake_cfg).unwrap();
        assert!(other_unit.exists());
        assert!(
            other_want
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
