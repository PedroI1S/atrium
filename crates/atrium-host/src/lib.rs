//! Platform services (contract §4, §10). **Signatures are contract; bodies
//! are M1-10's.** The only crate where `unsafe` is allowed, each block with a
//! `// SAFETY:` comment.
//!
//! - [`supervise`]: process groups, verified kill and reap, crash recovery.
//! - [`peer`]: who is on the other end of a daemon socket connection (G2).
//! - [`lifecycle`]: single instance, private runtime directory, detached
//!   start, launchd/systemd "keep agents running".

pub mod lifecycle;
pub mod peer;
pub mod supervise;

pub(crate) fn current_euid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, has no preconditions, and is
    // async-signal-safe and thread-safe on all POSIX platforms.
    unsafe { libc::geteuid() }
}
