//! Peer verification for the daemon socket (contract §4, D-045).
//!
//! `App` authority is granted only when the connecting process is the Atrium
//! app. The check is done once per connection, at `Hello`, and pins the
//! process (audit token on macOS, pidfd on Linux) so pid reuse can't swap it.
//!
//! Platform minimums:
//! - **macOS 11+**: requires `LOCAL_PEERTOKEN`, `SecCodeCopyGuestWithAttributes`
//!   (`kSecGuestAttributeAudit`), and macOS 11+ linker-emitted ad-hoc code
//!   directory hashes for unsigned development binaries (`MacUnsigned`).
//! - **Linux 6.5+**: requires `SO_PEERPIDFD` to pin the peer process without
//!   `pidfd_open` PID-reuse races.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What the daemon accepts as "the app".
///
/// - Release builds compile it in: `MacSigned` with the designated
///   requirement, or `Linux` with the packaged app path and `dev_ino: None`
///   (the daemon checks at runtime that the file and its folders are owned
///   by root and not writable by the user, so a same-user process can't
///   replace it). The data folder can't change it.
/// - Development builds read `Paths::app_identity` (mode 0600, owner checked),
///   written by the dev app at start. That file is as strong as the user's
///   own account, which is the documented same-UID limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppIdentity {
    /// macOS, signed build: the designated requirement (bundle id + team id).
    MacSigned { requirement: String },
    /// macOS, unsigned development build: executable path + cdhash.
    MacUnsigned { executable: PathBuf, cdhash: String },
    /// Linux: the installed binary by path. `dev_ino` (device, inode) is
    /// set only in development identities; release builds can't know it.
    Linux {
        executable: PathBuf,
        dev_ino: Option<(u64, u64)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub pid: u32,
    pub uid: u32,
    pub executable: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("peer is another user (uid {0})")]
    OtherUser(u32),
    #[error("peer is not the Atrium app: {0}")]
    NotApp(String),
    #[error("peer couldn't be inspected: {0}")]
    Inspect(String),
}

/// Verifies the peer of a connected unix socket against `expected`. Any
/// failure is an error; there is no partial success. The uid is checked
/// first.
/// - macOS: `LOCAL_PEERTOKEN` → `SecCodeCopyGuestWithAttributes`
///   (`kSecGuestAttributeAudit`); signed: `SecCodeCheckValidity` against the
///   requirement; unsigned: validates dynamic code with `SecCodeCheckValidity`
///   and compares the running process's cdhash (`kSecCodeInfoUnique` via
///   `SecCodeCopySigningInformation` with `kSecCSDynamicInformation`), never a
///   hash of the file on disk.
/// - Linux: `SO_PEERCRED`, then the peer's pidfd from `SO_PEERPIDFD`
///   (kernel 6.5+; without it, App authority is refused: no `pidfd_open`
///   fallback, which would race with pid reuse). `stat` `/proc/<pid>/exe`:
///   it must be `executable` (and match `dev_ino` when set; otherwise the
///   ownership check above), then `pidfd_send_signal(pidfd, 0)` must still
///   succeed, proving the process inspected is the one connected.
pub fn verify_app_peer(
    stream: &tokio::net::UnixStream,
    expected: &AppIdentity,
) -> Result<PeerInfo, PeerError> {
    let my_uid = crate::current_euid();
    let ucred = stream
        .peer_cred()
        .map_err(|e| PeerError::Inspect(format!("peer_cred failed: {e}")))?;
    if ucred.uid() != my_uid {
        return Err(PeerError::OtherUser(ucred.uid()));
    }
    platform::verify_peer(stream, my_uid, ucred.pid(), expected)
}

/// The identity of the running executable, for the app to write at install.
pub fn current_app_identity() -> std::io::Result<AppIdentity> {
    platform::current_identity()
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::OsStr;
    use std::io;
    use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::ptr;

    use super::{AppIdentity, PeerError, PeerInfo};

    type CFTypeRef = *const libc::c_void;
    type CFTypeID = usize;
    type CFAllocatorRef = *const libc::c_void;
    type CFDataRef = *const libc::c_void;
    type CFDictionaryRef = *const libc::c_void;
    type CFStringRef = *const libc::c_void;
    type CFURLRef = *const libc::c_void;
    type CFIndex = isize;
    type CFStringEncoding = u32;
    type Boolean = u8;
    type OSStatus = i32;
    type SecCSFlags = u32;
    type SecCodeRef = *const libc::c_void;
    type SecStaticCodeRef = *const libc::c_void;
    type SecRequirementRef = *const libc::c_void;

    const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;
    const K_SEC_CS_DEFAULT_FLAGS: SecCSFlags = 0;
    const K_SEC_CS_DYNAMIC_INFORMATION: SecCSFlags = 1 << 3;
    const ERR_SEC_SUCCESS: OSStatus = 0;
    const ERR_SEC_CS_UNSIGNED: OSStatus = -67062;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AuditToken {
        val: [u32; 8],
    }

    #[repr(C)]
    struct CFDictionaryKeyCallBacks {
        version: CFIndex,
        retain: *const libc::c_void,
        release: *const libc::c_void,
        copy_description: *const libc::c_void,
        equal: *const libc::c_void,
        hash: *const libc::c_void,
    }

    #[repr(C)]
    struct CFDictionaryValueCallBacks {
        version: CFIndex,
        retain: *const libc::c_void,
        release: *const libc::c_void,
        copy_description: *const libc::c_void,
        equal: *const libc::c_void,
    }

    // SAFETY: Declarations match `<bsm/libbsm.h>` on macOS (`audit_token_t` is
    // `unsigned int val[8]`, passed by value on Darwin ABI).
    #[link(name = "bsm")]
    unsafe extern "C" {
        fn audit_token_to_euid(atoken: AuditToken) -> libc::uid_t;
        fn audit_token_to_pid(atoken: AuditToken) -> libc::pid_t;
    }

    // SAFETY: Declarations match Apple's `<CoreFoundation/CoreFoundation.h>` on
    // LP64 macOS (`CFTypeID` = `unsigned long`, `CFIndex` = `long`,
    // `Boolean` = `unsigned char`).
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
        static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;

        fn CFRelease(cf: CFTypeRef);
        fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
        fn CFDataGetTypeID() -> CFTypeID;
        fn CFURLGetTypeID() -> CFTypeID;
        fn CFDataCreate(allocator: CFAllocatorRef, bytes: *const u8, length: CFIndex) -> CFDataRef;
        fn CFDataGetBytePtr(the_data: CFDataRef) -> *const u8;
        fn CFDataGetLength(the_data: CFDataRef) -> CFIndex;
        fn CFDictionaryCreate(
            allocator: CFAllocatorRef,
            keys: *const *const libc::c_void,
            values: *const *const libc::c_void,
            num_values: CFIndex,
            key_call_backs: *const CFDictionaryKeyCallBacks,
            value_call_backs: *const CFDictionaryValueCallBacks,
        ) -> CFDictionaryRef;
        fn CFDictionaryGetValue(
            the_dict: CFDictionaryRef,
            key: *const libc::c_void,
        ) -> *const libc::c_void;
        fn CFStringCreateWithBytes(
            alloc: CFAllocatorRef,
            bytes: *const u8,
            num_bytes: CFIndex,
            encoding: CFStringEncoding,
            is_external_representation: Boolean,
        ) -> CFStringRef;
        fn CFURLGetFileSystemRepresentation(
            url: CFURLRef,
            resolve_against_base: Boolean,
            buffer: *mut u8,
            max_buf_len: CFIndex,
        ) -> Boolean;
    }

    // SAFETY: Declarations match Apple's public `<Security/CSCommon.h>`,
    // `<Security/SecCode.h>`, `<Security/SecStaticCode.h>`, and
    // `<Security/SecRequirement.h>` headers in `Security.framework`.
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        static kSecGuestAttributeAudit: CFStringRef;
        static kSecCodeInfoUnique: CFStringRef;
        static kSecCodeInfoMainExecutable: CFStringRef;

        fn SecCodeCopySelf(flags: SecCSFlags, self_code: *mut SecCodeRef) -> OSStatus;
        fn SecCodeCopyGuestWithAttributes(
            host: SecCodeRef,
            attributes: CFDictionaryRef,
            flags: SecCSFlags,
            guest: *mut SecCodeRef,
        ) -> OSStatus;
        fn SecCodeCheckValidity(
            code: SecCodeRef,
            flags: SecCSFlags,
            requirement: SecRequirementRef,
        ) -> OSStatus;
        fn SecCodeCopySigningInformation(
            code: SecStaticCodeRef,
            flags: SecCSFlags,
            information: *mut CFDictionaryRef,
        ) -> OSStatus;
        fn SecRequirementCreateWithString(
            text: CFStringRef,
            flags: SecCSFlags,
            requirement: *mut SecRequirementRef,
        ) -> OSStatus;
    }

    /// Owns a non-null CoreFoundation reference obtained from a Create/Copy API.
    struct CfRef(CFTypeRef);

    impl CfRef {
        /// Takes ownership of a CoreFoundation pointer returned under the
        /// Create or Copy rule.
        ///
        /// # Safety
        ///
        /// `ptr` must be either null or a valid CoreFoundation object reference
        /// with a +1 retain count whose ownership the caller transfers to
        /// `Self` (never an unowned Get-rule reference).
        unsafe fn from_create_rule(ptr: CFTypeRef) -> Option<Self> {
            (!ptr.is_null()).then_some(Self(ptr))
        }

        fn as_ptr(&self) -> CFTypeRef {
            self.0
        }
    }

    impl Drop for CfRef {
        fn drop(&mut self) {
            // SAFETY: `self.0` is guaranteed non-null and +1 owned by
            // `CfRef::from_create_rule`'s invariant, released once on drop.
            unsafe { CFRelease(self.0) };
        }
    }

    struct SigningDetails {
        cdhash_hex: Option<String>,
        executable: Option<PathBuf>,
    }

    fn hex_lower(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            use std::fmt::Write;
            let _ = write!(&mut out, "{b:02x}");
        }
        out
    }

    fn is_valid_cdhash_hex(s: &str) -> bool {
        !s.is_empty() && s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit())
    }

    fn peer_audit_token(fd: BorrowedFd<'_>) -> Result<(AuditToken, u32, u32), PeerError> {
        let expected_len = std::mem::size_of::<AuditToken>() as libc::socklen_t;
        let mut len = expected_len;
        let mut token = AuditToken { val: [0; 8] };
        // SAFETY: `token` is a plain `[u32; 8]` C struct, `fd` is a live
        // borrowed socket fd, and `&mut len` bounds `getsockopt` writes to
        // `sizeof(AuditToken)`.
        let rc = unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERTOKEN,
                (&raw mut token).cast::<libc::c_void>(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(PeerError::Inspect(format!(
                "LOCAL_PEERTOKEN failed: {}",
                io::Error::last_os_error()
            )));
        }
        if len != expected_len {
            return Err(PeerError::Inspect(format!(
                "short LOCAL_PEERTOKEN length {len}"
            )));
        }
        // SAFETY: `getsockopt` succeeded and wrote a complete `AuditToken`
        // (`len == sizeof(AuditToken)` checked above). `audit_token_to_*` are
        // pure functions reading `token` by value.
        let (euid, pid) = unsafe { (audit_token_to_euid(token), audit_token_to_pid(token)) };
        let pid_u32 = u32::try_from(pid)
            .ok()
            .filter(|&p| p > 0)
            .ok_or_else(|| PeerError::Inspect(format!("invalid peer pid {pid}")))?;
        Ok((token, euid, pid_u32))
    }

    fn sec_code_from_token(token: &AuditToken) -> Result<CfRef, PeerError> {
        let token_bytes = ptr::from_ref(token).cast::<u8>();
        let token_len = std::mem::size_of::<AuditToken>() as CFIndex;
        // SAFETY: `token_bytes` points to `size_of::<AuditToken>()` initialized
        // bytes valid during `CFDataCreate`, which copies them and returns null
        // or a +1 owned `CFDataRef` under the Create rule.
        let audit_data =
            unsafe { CfRef::from_create_rule(CFDataCreate(ptr::null(), token_bytes, token_len)) }
                .ok_or_else(|| PeerError::Inspect("CFDataCreate for audit token failed".into()))?;

        // SAFETY: `kSecGuestAttributeAudit` is an immutable `CFStringRef`
        // constant exported by `Security.framework`, and `audit_data` is a live
        // `CFDataRef`. `CFDictionaryCreate` retains key and value using Core
        // Foundation's standard type callbacks and returns null or a +1 owned
        // `CFDictionaryRef` under the Create rule.
        let attrs = unsafe {
            let key = kSecGuestAttributeAudit;
            if key.is_null() {
                return Err(PeerError::Inspect("kSecGuestAttributeAudit is null".into()));
            }
            let keys: [*const libc::c_void; 1] = [key];
            let values: [*const libc::c_void; 1] = [audit_data.as_ptr()];
            CfRef::from_create_rule(CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            ))
        }
        .ok_or_else(|| {
            PeerError::Inspect("CFDictionaryCreate for guest attributes failed".into())
        })?;

        // SAFETY: `host` is null (system host), `attrs.as_ptr()` is a live
        // `CFDictionaryRef` kept alive by `attrs`, and `&mut code_raw` is a
        // valid out pointer. On `ERR_SEC_SUCCESS`, `SecCodeCopyGuestWithAttributes`
        // writes a +1 owned `SecCodeRef` under the Copy rule.
        unsafe {
            let mut code_raw: SecCodeRef = ptr::null();
            let status = SecCodeCopyGuestWithAttributes(
                ptr::null(),
                attrs.as_ptr(),
                K_SEC_CS_DEFAULT_FLAGS,
                &mut code_raw,
            );
            if status == ERR_SEC_CS_UNSIGNED {
                return Err(PeerError::NotApp("peer binary is unsigned".into()));
            }
            if status != ERR_SEC_SUCCESS {
                return Err(PeerError::Inspect(format!(
                    "SecCodeCopyGuestWithAttributes failed (OSStatus {status})"
                )));
            }
            CfRef::from_create_rule(code_raw).ok_or_else(|| {
                PeerError::Inspect("SecCodeCopyGuestWithAttributes returned null".into())
            })
        }
    }

    fn check_code_validity(code: &CfRef, requirement: Option<&str>) -> Result<(), PeerError> {
        let req_ref = match requirement {
            None => None,
            Some(req_str) => {
                let len = CFIndex::try_from(req_str.len())
                    .map_err(|_| PeerError::NotApp("requirement string too long".into()))?;
                // SAFETY: `req_str.as_ptr()` is valid for `len` UTF-8 bytes
                // during `CFStringCreateWithBytes`, which copies them and
                // returns null or a +1 owned `CFStringRef`. On `ERR_SEC_SUCCESS`,
                // `SecRequirementCreateWithString` writes a +1 owned
                // `SecRequirementRef` to `raw_req`.
                let created = unsafe {
                    let Some(cf_str) = CfRef::from_create_rule(CFStringCreateWithBytes(
                        ptr::null(),
                        req_str.as_ptr(),
                        len,
                        K_CF_STRING_ENCODING_UTF8,
                        0,
                    )) else {
                        return Err(PeerError::Inspect("CFStringCreateWithBytes failed".into()));
                    };
                    let mut raw_req: SecRequirementRef = ptr::null();
                    let status = SecRequirementCreateWithString(
                        cf_str.as_ptr(),
                        K_SEC_CS_DEFAULT_FLAGS,
                        &mut raw_req,
                    );
                    if status != ERR_SEC_SUCCESS {
                        return Err(PeerError::NotApp(format!(
                            "invalid designated requirement (OSStatus {status})"
                        )));
                    }
                    CfRef::from_create_rule(raw_req).ok_or_else(|| {
                        PeerError::NotApp("SecRequirementCreateWithString returned null".into())
                    })?
                };
                Some(created)
            }
        };

        let raw_req = req_ref.as_ref().map_or(ptr::null(), CfRef::as_ptr);
        // SAFETY: `code.as_ptr()` is a live dynamic `SecCodeRef`, and `raw_req`
        // is either null (validate dynamic code signature) or a live
        // `SecRequirementRef` kept alive by `req_ref` across this call.
        let status =
            unsafe { SecCodeCheckValidity(code.as_ptr(), K_SEC_CS_DEFAULT_FLAGS, raw_req) };
        if status != ERR_SEC_SUCCESS {
            return Err(PeerError::NotApp(format!(
                "code signature check failed (OSStatus {status})"
            )));
        }
        Ok(())
    }

    /// Reads dynamic signing information directly from a live `SecCodeRef`
    /// (`kSecCSDynamicInformation`), never via `SecCodeCopyStaticCode`.
    fn extract_dynamic_signing_details(code: &CfRef) -> Result<SigningDetails, String> {
        // SAFETY: `code.as_ptr()` is a valid running `SecCodeRef` (accepted by
        // `SecCodeCopySigningInformation` directly without converting to a
        // disk-backed static code object). On `ERR_SEC_SUCCESS`, `raw_dict` is a
        // +1 owned `CFDictionaryRef` under the Copy rule. Dictionary values are
        // unowned borrows tied to `dict`'s lifetime, runtime-type-checked with
        // `CFGetTypeID` before reading `CFData` or `CFURL` bytes.
        unsafe {
            let mut raw_dict: CFDictionaryRef = ptr::null();
            let info_status = SecCodeCopySigningInformation(
                code.as_ptr(),
                K_SEC_CS_DYNAMIC_INFORMATION,
                &mut raw_dict,
            );
            if info_status != ERR_SEC_SUCCESS {
                return Err(format!(
                    "SecCodeCopySigningInformation failed (OSStatus {info_status})"
                ));
            }
            let dict = CfRef::from_create_rule(raw_dict)
                .ok_or_else(|| "SecCodeCopySigningInformation returned null".to_string())?;

            let unique_val = CFDictionaryGetValue(dict.as_ptr(), kSecCodeInfoUnique);
            let cdhash_hex =
                if !unique_val.is_null() && CFGetTypeID(unique_val) == CFDataGetTypeID() {
                    let len = CFDataGetLength(unique_val);
                    let ptr = CFDataGetBytePtr(unique_val);
                    if len > 0 && !ptr.is_null() {
                        let bytes = std::slice::from_raw_parts(ptr, len as usize);
                        Some(hex_lower(bytes))
                    } else {
                        None
                    }
                } else {
                    None
                };

            let exe_val = CFDictionaryGetValue(dict.as_ptr(), kSecCodeInfoMainExecutable);
            let executable = if !exe_val.is_null() && CFGetTypeID(exe_val) == CFURLGetTypeID() {
                let mut buf = [0u8; libc::PATH_MAX as usize];
                let ok = CFURLGetFileSystemRepresentation(
                    exe_val,
                    1,
                    buf.as_mut_ptr(),
                    buf.len() as CFIndex,
                );
                if ok != 0 {
                    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                    let raw_path = PathBuf::from(OsStr::from_bytes(&buf[..end]));
                    Some(std::fs::canonicalize(&raw_path).unwrap_or(raw_path))
                } else {
                    None
                }
            } else {
                None
            };

            Ok(SigningDetails {
                cdhash_hex,
                executable,
            })
        }
    }

    pub(super) fn verify_peer(
        stream: &tokio::net::UnixStream,
        my_uid: u32,
        _cred_pid: Option<libc::pid_t>,
        expected: &AppIdentity,
    ) -> Result<PeerInfo, PeerError> {
        let (token, token_euid, pid) = peer_audit_token(stream.as_fd())?;
        if token_euid != my_uid {
            return Err(PeerError::OtherUser(token_euid));
        }

        let code = sec_code_from_token(&token)?;

        match expected {
            AppIdentity::MacSigned { requirement } => {
                check_code_validity(&code, Some(requirement))?;
                let details = extract_dynamic_signing_details(&code).map_err(PeerError::Inspect)?;
                let executable = match details.executable {
                    Some(p) => p,
                    None => crate::supervise::identify(pid)
                        .map_err(|e| PeerError::Inspect(e.to_string()))?
                        .map(|i| i.executable)
                        .ok_or_else(|| PeerError::Inspect("peer executable unavailable".into()))?,
                };
                Ok(PeerInfo {
                    pid,
                    uid: my_uid,
                    executable,
                })
            }
            AppIdentity::MacUnsigned {
                executable: expected_exe,
                cdhash: expected_cdhash,
            } => {
                if !is_valid_cdhash_hex(expected_cdhash) {
                    return Err(PeerError::NotApp("expected cdhash is invalid".into()));
                }
                // Validate the dynamic running code object against the expected
                // cdhash requirement first so any post-launch on-disk replacement
                // (`errSecCSStaticCodeChanged`) or signature mismatch fails closed
                // before inspecting signing dictionary fields.
                let cdhash_req = format!("cdhash H\"{expected_cdhash}\"");
                check_code_validity(&code, Some(&cdhash_req))?;

                let details = extract_dynamic_signing_details(&code).map_err(|e| {
                    PeerError::NotApp(format!("cannot read peer dynamic signing info: {e}"))
                })?;
                let Some(actual_cdhash) = details.cdhash_hex else {
                    return Err(PeerError::NotApp("peer code object has no cdhash".into()));
                };
                if !actual_cdhash.eq_ignore_ascii_case(expected_cdhash) {
                    return Err(PeerError::NotApp(format!(
                        "cdhash mismatch (peer {actual_cdhash})"
                    )));
                }
                let executable = details.executable.ok_or_else(|| {
                    PeerError::Inspect("peer code object has no main executable".into())
                })?;
                let expected_norm =
                    std::fs::canonicalize(expected_exe).unwrap_or_else(|_| expected_exe.clone());
                if executable != *expected_exe && executable != expected_norm {
                    return Err(PeerError::NotApp(format!(
                        "executable mismatch (peer {})",
                        executable.display()
                    )));
                }
                Ok(PeerInfo {
                    pid,
                    uid: my_uid,
                    executable,
                })
            }
            AppIdentity::Linux { .. } => Err(PeerError::NotApp(
                "Linux app identity cannot verify a macOS peer".into(),
            )),
        }
    }

    pub(super) fn current_identity() -> io::Result<AppIdentity> {
        // SAFETY: `SecCodeCopySelf` writes a +1 owned `SecCodeRef` to `raw_self`
        // under the Copy rule when returning `ERR_SEC_SUCCESS`.
        let self_code = unsafe {
            let mut raw_self: SecCodeRef = ptr::null();
            let status = SecCodeCopySelf(K_SEC_CS_DEFAULT_FLAGS, &mut raw_self);
            if status != ERR_SEC_SUCCESS {
                return Err(io::Error::other(format!(
                    "SecCodeCopySelf failed (OSStatus {status})"
                )));
            }
            CfRef::from_create_rule(raw_self)
                .ok_or_else(|| io::Error::other("SecCodeCopySelf returned null"))?
        };
        check_code_validity(&self_code, None).map_err(io::Error::other)?;
        let details = extract_dynamic_signing_details(&self_code).map_err(io::Error::other)?;
        let cdhash = details
            .cdhash_hex
            .ok_or_else(|| io::Error::other("current executable has no code-object cdhash"))?;
        let executable = match details.executable {
            Some(p) => p,
            None => {
                let exe = std::env::current_exe()?;
                std::fs::canonicalize(&exe).unwrap_or(exe)
            }
        };
        Ok(AppIdentity::MacUnsigned { executable, cdhash })
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::io;
    use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    use std::ptr;

    use super::{AppIdentity, PeerError, PeerInfo};

    fn peer_pidfd(fd: BorrowedFd<'_>) -> Result<OwnedFd, PeerError> {
        let mut raw_pidfd: libc::c_int = -1;
        let expected_len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        let mut len = expected_len;
        // SAFETY: `fd` is a valid borrowed Unix socket fd, and `raw_pidfd` and
        // `len` are valid mutable stack locals (`len` initialized to
        // `sizeof(c_int)`). When `getsockopt(SO_PEERPIDFD)` returns `0` and
        // writes a non-negative descriptor to `raw_pidfd`, ownership of that new
        // open fd belongs to the caller, so we wrap it in `OwnedFd` immediately
        // before checking `len` so it cannot leak.
        let pidfd = unsafe {
            let rc = libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERPIDFD,
                (&raw mut raw_pidfd).cast::<libc::c_void>(),
                &mut len,
            );
            if rc != 0 {
                return Err(PeerError::Inspect(format!(
                    "SO_PEERPIDFD failed (kernel 6.5+ required): {}",
                    io::Error::last_os_error()
                )));
            }
            if raw_pidfd < 0 {
                return Err(PeerError::Inspect("invalid SO_PEERPIDFD descriptor".into()));
            }
            OwnedFd::from_raw_fd(raw_pidfd)
        };
        if len != expected_len {
            return Err(PeerError::Inspect(format!(
                "unexpected SO_PEERPIDFD length {len}"
            )));
        }
        Ok(pidfd)
    }

    fn verify_pidfd_alive(pidfd: BorrowedFd<'_>) -> Result<(), PeerError> {
        let sig: libc::c_int = 0;
        let flags: libc::c_uint = 0;
        // SAFETY: `pidfd` is a valid open pidfd; signal `0` with null `siginfo_t`
        // and flags `0` performs a kernel existence check without sending a signal.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                pidfd.as_raw_fd(),
                sig,
                ptr::null::<libc::c_void>(),
                flags,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(PeerError::Inspect(format!(
                "pidfd_send_signal(0) failed: {}",
                io::Error::last_os_error()
            )))
        }
    }

    fn check_root_owned_tree(
        expected_exe: &Path,
        proc_dev_ino: (u64, u64),
    ) -> Result<(), PeerError> {
        if !expected_exe.is_absolute() {
            return Err(PeerError::NotApp(
                "release Linux executable path must be absolute".into(),
            ));
        }
        let disk_meta = std::fs::metadata(expected_exe).map_err(|e| {
            PeerError::NotApp(format!("cannot stat {}: {e}", expected_exe.display()))
        })?;
        if (disk_meta.dev(), disk_meta.ino()) != proc_dev_ino {
            return Err(PeerError::NotApp(
                "peer inode does not match packaged executable on disk".into(),
            ));
        }

        for ancestor in expected_exe.ancestors() {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            let meta = std::fs::symlink_metadata(ancestor).map_err(|e| {
                PeerError::NotApp(format!("cannot lstat {}: {e}", ancestor.display()))
            })?;
            if meta.uid() != 0 {
                return Err(PeerError::NotApp(format!(
                    "{} is not owned by root (uid {})",
                    ancestor.display(),
                    meta.uid()
                )));
            }
            if !meta.file_type().is_symlink() && (meta.mode() & 0o022) != 0 {
                return Err(PeerError::NotApp(format!(
                    "{} is writable by group or others (mode {:o})",
                    ancestor.display(),
                    meta.mode() & 0o7777
                )));
            }
        }

        let canonical = std::fs::canonicalize(expected_exe).map_err(|e| {
            PeerError::NotApp(format!(
                "cannot canonicalize {}: {e}",
                expected_exe.display()
            ))
        })?;
        let canon_meta = std::fs::metadata(&canonical)
            .map_err(|e| PeerError::NotApp(format!("cannot stat {}: {e}", canonical.display())))?;
        if (canon_meta.dev(), canon_meta.ino()) != proc_dev_ino {
            return Err(PeerError::NotApp(
                "canonical executable inode changed during check".into(),
            ));
        }
        for ancestor in canonical.ancestors() {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            let meta = std::fs::symlink_metadata(ancestor).map_err(|e| {
                PeerError::NotApp(format!("cannot lstat {}: {e}", ancestor.display()))
            })?;
            if meta.uid() != 0 || meta.file_type().is_symlink() || (meta.mode() & 0o022) != 0 {
                return Err(PeerError::NotApp(format!(
                    "{} is not root-owned non-writable",
                    ancestor.display()
                )));
            }
        }
        Ok(())
    }

    pub(super) fn verify_peer(
        stream: &tokio::net::UnixStream,
        my_uid: u32,
        cred_pid: Option<libc::pid_t>,
        expected: &AppIdentity,
    ) -> Result<PeerInfo, PeerError> {
        let raw_pid = cred_pid
            .filter(|&p| p > 0)
            .ok_or_else(|| PeerError::Inspect("SO_PEERCRED returned no pid".into()))?;
        let pid =
            u32::try_from(raw_pid).map_err(|_| PeerError::Inspect("negative peer pid".into()))?;

        // Obtain the kernel-pinned pidfd from SO_PEERPIDFD before inspecting /proc.
        let pidfd = peer_pidfd(stream.as_fd())?;

        let (expected_exe, expected_dev_ino) = match expected {
            AppIdentity::Linux {
                executable,
                dev_ino,
            } => (executable, *dev_ino),
            AppIdentity::MacSigned { .. } | AppIdentity::MacUnsigned { .. } => {
                return Err(PeerError::NotApp(
                    "macOS app identity cannot verify a Linux peer".into(),
                ));
            }
        };

        let proc_exe = format!("/proc/{pid}/exe");
        let actual_exe = std::fs::read_link(&proc_exe)
            .map_err(|e| PeerError::Inspect(format!("readlink {proc_exe}: {e}")))?;
        let proc_meta = std::fs::metadata(&proc_exe)
            .map_err(|e| PeerError::Inspect(format!("stat {proc_exe}: {e}")))?;

        if actual_exe != *expected_exe {
            return Err(PeerError::NotApp(format!(
                "executable mismatch (peer {})",
                actual_exe.display()
            )));
        }

        let proc_dev_ino = (proc_meta.dev(), proc_meta.ino());
        match expected_dev_ino {
            Some(dev_ino) => {
                if proc_dev_ino != dev_ino {
                    return Err(PeerError::NotApp("executable dev/ino mismatch".into()));
                }
            }
            None => {
                check_root_owned_tree(expected_exe, proc_dev_ino)?;
            }
        }

        // Prove the process inspected through `/proc/<pid>/exe` is still the
        // exact process pinned by `SO_PEERPIDFD` at connect time.
        verify_pidfd_alive(pidfd.as_fd())?;

        Ok(PeerInfo {
            pid,
            uid: my_uid,
            executable: actual_exe,
        })
    }

    pub(super) fn current_identity() -> io::Result<AppIdentity> {
        let executable = std::fs::read_link("/proc/self/exe")?;
        let meta = std::fs::metadata("/proc/self/exe")?;
        Ok(AppIdentity::Linux {
            executable,
            dev_ino: Some((meta.dev(), meta.ino())),
        })
    }
}
