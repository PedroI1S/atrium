use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use atrium_host::peer::{AppIdentity, PeerError, current_app_identity, verify_app_peer};
use tokio::net::{UnixListener, UnixStream};

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        // Keep path short so Unix domain socket paths stay well under the
        // 103-byte sun_path limit on macOS.
        let dir = PathBuf::from("/tmp").join(format!(
            "ah-{prefix}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .expect("create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn wait(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.wait();
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn compile_helper(dir: &Path, name: &str, marker: &str) -> PathBuf {
    let src = dir.join(format!("{name}.rs"));
    let bin = dir.join(name);
    let code = format!(
        r#"use std::io::{{Read, Write}};
use std::os::unix::net::UnixStream;

static MARKER: &str = {marker:?};

fn main() {{
    let mut args = std::env::args_os().skip(1);
    let mode = args.next().expect("mode");
    let sock = args.next().expect("sock");
    if mode == "wait-stdin" {{
        let mut out = std::io::stdout();
        out.write_all(b"ready\n").unwrap();
        out.flush().unwrap();
        let mut gate = [0u8; 1];
        let _ = std::io::stdin().read(&mut gate);
    }}
    let _ = std::hint::black_box(MARKER);
    let mut stream = UnixStream::connect(sock).expect("helper connect");
    let mut buf = [0u8; 1];
    let _ = stream.read(&mut buf);
}}
"#
    );
    std::fs::write(&src, code).expect("write helper source");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = Command::new(rustc)
        .arg("--edition=2024")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke rustc");
    assert!(status.success(), "rustc failed compiling helper {name}");
    bin
}

#[tokio::test]
async fn test_binary_verifying_itself_succeeds() {
    let tmp = TempDir::new("peer-self");
    let sock = tmp.path().join("d.sock");
    let listener = UnixListener::bind(&sock).expect("bind listener");

    let my_identity = current_app_identity().expect("current_app_identity");
    let connect_fut = UnixStream::connect(&sock);
    let accept_fut = listener.accept();
    let (client_res, accept_res) = tokio::join!(connect_fut, accept_fut);
    let _client = client_res.expect("client connect");
    let (server_stream, _) = accept_res.expect("accept");

    let peer = verify_app_peer(&server_stream, &my_identity).expect("verify self peer");
    assert_eq!(peer.pid, std::process::id());
    // SAFETY: `geteuid` has no preconditions.
    let expected_uid = unsafe { libc::geteuid() };
    assert_eq!(peer.uid, expected_uid);
    assert!(!peer.executable.as_os_str().is_empty());
}

#[tokio::test]
async fn different_executable_connecting_after_exec_fails_with_not_app() {
    let tmp = TempDir::new("peer-diff");
    let helper = compile_helper(tmp.path(), "peer_helper", "helper-v1");
    let sock = tmp.path().join("d.sock");
    let listener = UnixListener::bind(&sock).expect("bind listener");

    let my_identity = current_app_identity().expect("current_app_identity");

    // Spawn the separate compiled binary; it calls `connect()` itself *after* `exec`.
    let child = Command::new(&helper)
        .arg("direct")
        .arg(&sock)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helper");
    let mut guard = ChildGuard::new(child);

    let (server_stream, _) = listener.accept().await.expect("accept helper");
    let err = verify_app_peer(&server_stream, &my_identity)
        .expect_err("different executable must be rejected");
    assert!(
        matches!(err, PeerError::NotApp(_)),
        "expected PeerError::NotApp, got {err:?}"
    );

    // Even if `executable` matches the helper's path, a wrong cdhash / dev_ino
    // must still fail with `NotApp`.
    let canon_helper = std::fs::canonicalize(&helper).unwrap_or_else(|_| helper.clone());
    let forged_identity = match &my_identity {
        AppIdentity::MacUnsigned { cdhash, .. } => AppIdentity::MacUnsigned {
            executable: canon_helper,
            cdhash: cdhash.clone(),
        },
        AppIdentity::MacSigned { requirement } => AppIdentity::MacSigned {
            requirement: requirement.clone(),
        },
        AppIdentity::Linux { dev_ino, .. } => AppIdentity::Linux {
            executable: canon_helper,
            dev_ino: dev_ino.map(|(d, i)| (d, i.wrapping_add(1))),
        },
    };
    let err2 = verify_app_peer(&server_stream, &forged_identity)
        .expect_err("mismatched cdhash / dev_ino must be rejected");
    assert!(
        matches!(err2, PeerError::NotApp(_)),
        "expected PeerError::NotApp on forged identity, got {err2:?}"
    );

    drop(server_stream);
    guard.wait();
}

#[cfg(target_os = "macos")]
fn read_codesign_cdhash(bin: &Path) -> String {
    let out = Command::new("/usr/bin/codesign")
        .arg("-dvvv")
        .arg(bin)
        .output()
        .expect("invoke codesign -dvvv");
    assert!(out.status.success(), "codesign -dvvv failed on {bin:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr
        .lines()
        .find_map(|l| l.strip_prefix("CDHash="))
        .expect("CDHash= line in codesign -dvvv output")
        .trim()
        .to_string()
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_requirement_check_and_in_kernel_code_object_cdhash() {
    use std::io::{BufRead, Write};

    let tmp = TempDir::new("peer-mac");
    let sock = tmp.path().join("d.sock");
    let listener = UnixListener::bind(&sock).expect("bind listener");

    let my_id = current_app_identity().expect("current_app_identity");
    let AppIdentity::MacUnsigned {
        cdhash: my_cdhash, ..
    } = &my_id
    else {
        panic!("expected MacUnsigned from current_app_identity, got {my_id:?}");
    };

    let (client_res, accept_res) = tokio::join!(UnixStream::connect(&sock), listener.accept());
    let _client = client_res.unwrap();
    let (server_stream, _) = accept_res.unwrap();

    // 1. Signed designated requirement that does not match this test binary -> NotApp.
    let bad_signed = AppIdentity::MacSigned {
        requirement: "identifier \"dev.atrium.nonexistent\" and anchor apple generic".into(),
    };
    assert!(matches!(
        verify_app_peer(&server_stream, &bad_signed),
        Err(PeerError::NotApp(_))
    ));

    // 2. Valid Apple Requirement Language matching this binary's cdhash -> Ok.
    let good_signed = AppIdentity::MacSigned {
        requirement: format!("cdhash H\"{my_cdhash}\""),
    };
    let info = verify_app_peer(&server_stream, &good_signed).expect("valid requirement");
    assert_eq!(info.pid, std::process::id());

    // 3. Cross-platform Linux identity on macOS -> NotApp.
    let wrong_os = AppIdentity::Linux {
        executable: PathBuf::from("/usr/bin/true"),
        dev_ino: None,
    };
    assert!(matches!(
        verify_app_peer(&server_stream, &wrong_os),
        Err(PeerError::NotApp(_))
    ));
    drop(server_stream);
    drop(_client);

    // 4. Prove `MacUnsigned` inspects the live kernel code object (`kSecCodeInfoUnique`
    // via `SecCodeCreateWithAuditToken`) and NEVER hashes the file on disk:
    // Start `helper_a`, overwrite its path on disk with `helper_b` while `helper_a`
    // is already running, then let `helper_a` connect.
    let helper_a_path = compile_helper(tmp.path(), "helper_a", "alpha-code-directory-payload");
    let helper_b_path = compile_helper(tmp.path(), "helper_b", "bravo-different-payload-9999");
    // Re-sign helper_b with a fresh ad-hoc signature and distinct identifier so
    // it is an explicitly differently signed binary on disk.
    let codesign_status = Command::new("/usr/bin/codesign")
        .args(["-s", "-", "-f", "-i", "dev.atrium.test.helper-b"])
        .arg(&helper_b_path)
        .status()
        .expect("invoke codesign");
    assert!(codesign_status.success(), "codesign failed on helper_b");
    let canon_a = std::fs::canonicalize(&helper_a_path).unwrap();

    let cdhash_a = read_codesign_cdhash(&helper_a_path);
    let cdhash_b = read_codesign_cdhash(&helper_b_path);
    assert_ne!(cdhash_a, cdhash_b);

    let running_id_a = AppIdentity::MacUnsigned {
        executable: canon_a.clone(),
        cdhash: cdhash_a.clone(),
    };
    let forged_disk_id = AppIdentity::MacUnsigned {
        executable: canon_a.clone(),
        cdhash: cdhash_b.clone(),
    };

    // Sanity check before disk replacement: `helper_a` succeeds against its own
    // dynamic cdhash (`running_id_a`) and fails against `forged_disk_id`.
    let child_a0 = Command::new(&helper_a_path)
        .arg("direct")
        .arg(&sock)
        .spawn()
        .unwrap();
    let child_a0_pid = child_a0.id();
    let mut guard_a0 = ChildGuard::new(child_a0);
    let (stream_a0, _) = listener.accept().await.unwrap();
    let info_a0 = verify_app_peer(&stream_a0, &running_id_a)
        .expect("helper_a must verify against its own MacUnsigned identity");
    assert_eq!(info_a0.pid, child_a0_pid);
    assert_eq!(info_a0.executable, canon_a);
    assert!(matches!(
        verify_app_peer(&stream_a0, &forged_disk_id),
        Err(PeerError::NotApp(_))
    ));
    drop(stream_a0);
    guard_a0.wait();

    // Launch `helper_a` in wait-stdin mode so it is already `exec`'d in memory.
    let mut child_a = Command::new(&helper_a_path)
        .arg("wait-stdin")
        .arg(&sock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let running_a_pid = child_a.id();
    let mut ready_line = String::new();
    std::io::BufReader::new(child_a.stdout.take().unwrap())
        .read_line(&mut ready_line)
        .unwrap();
    assert_eq!(ready_line.trim(), "ready");

    // Replace `helper_a` on disk with differently signed `helper_b` BEFORE `helper_a` connects!
    std::fs::rename(&helper_b_path, &helper_a_path).unwrap();
    assert_eq!(read_codesign_cdhash(&helper_a_path), cdhash_b);

    // Signal `helper_a` to call `connect()` now.
    let mut stdin_a = child_a.stdin.take().unwrap();
    stdin_a.write_all(b"g").unwrap();
    drop(stdin_a);
    let mut guard_a = ChildGuard::new(child_a);

    let (stream_a, _) = listener.accept().await.unwrap();

    // 4a. Replacing the binary on disk MUST NEVER allow the running process to
    // pass verification under the new on-disk file's cdhash (`cdhash_b`).
    let disk_err = verify_app_peer(&stream_a, &forged_disk_id)
        .expect_err("on-disk replacement binary cdhash must NOT match running process");
    assert!(matches!(disk_err, PeerError::NotApp(_)));

    // 4b. Against the running process's own pre-replacement identity (`cdhash_a`),
    // `verify_app_peer` either reflects the running process (`Ok` with `running_a_pid`)
    // or fails closed (`Err(PeerError::NotApp)` when Security.framework flags
    // `errSecCSStaticCodeChanged` -67034).
    match verify_app_peer(&stream_a, &running_id_a) {
        Ok(info) => {
            assert_eq!(info.pid, running_a_pid);
            assert_eq!(info.executable, canon_a);
        }
        Err(PeerError::NotApp(msg)) => {
            assert!(
                msg.contains("-67034") || msg.contains("code signature check failed"),
                "unexpected NotApp message on replaced binary: {msg}"
            );
        }
        Err(other) => panic!("unexpected error variant on replaced binary: {other:?}"),
    }

    drop(stream_a);
    guard_a.wait();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_release_identity_refuses_user_writable_path_and_wrong_os_identity() {
    let tmp = TempDir::new("peer-lin");
    let sock = tmp.path().join("d.sock");
    let listener = UnixListener::bind(&sock).expect("bind listener");

    let (client_res, accept_res) = tokio::join!(UnixStream::connect(&sock), listener.accept());
    let _client = client_res.unwrap();
    let (server_stream, _) = accept_res.unwrap();

    // 1. Cross-platform macOS identity on Linux -> NotApp.
    let wrong_os = AppIdentity::MacSigned {
        requirement: "identifier \"dev.atrium\"".into(),
    };
    assert!(matches!(
        verify_app_peer(&server_stream, &wrong_os),
        Err(PeerError::NotApp(_))
    ));

    // 2. Release identity (`dev_ino: None`) pointing to a user-owned / `/tmp`-backed
    // test binary must be refused because the file/ancestors are not root-owned
    // and non-writable.
    let self_exe = std::fs::read_link("/proc/self/exe").expect("read /proc/self/exe");
    let release_untrusted = AppIdentity::Linux {
        executable: self_exe,
        dev_ino: None,
    };
    assert!(matches!(
        verify_app_peer(&server_stream, &release_untrusted),
        Err(PeerError::NotApp(_))
    ));
}
