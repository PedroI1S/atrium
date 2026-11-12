//! Links in agent text (contract §12). Agent text is untrusted: a link is
//! opened in the system browser only when its scheme is `http`, `https` or
//! `mailto`. Everything else is refused here, whatever the webview asked.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use url::Url;

/// Longer than any link a person would click.
pub const MAX_LINK_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("the link is too long")]
    TooLong,
    #[error("the link isn't a valid address")]
    Invalid,
    #[error("links of type {0} are not opened")]
    Scheme(String),
}

/// Parses `input` and accepts it only with an allowed scheme. The returned
/// URL is the normalized form, which is what gets opened.
pub fn check(input: &str) -> Result<Url, LinkError> {
    if input.len() > MAX_LINK_BYTES {
        return Err(LinkError::TooLong);
    }
    if input.chars().any(char::is_control) {
        return Err(LinkError::Invalid);
    }
    let url = Url::parse(input).map_err(|_| LinkError::Invalid)?;
    match url.scheme() {
        "http" | "https" if url.host_str().is_some_and(|host| !host.is_empty()) => Ok(url),
        "http" | "https" => Err(LinkError::Invalid),
        "mailto" => Ok(url),
        other => Err(LinkError::Scheme(other.into())),
    }
}

/// Hands the link to the system's opener as one argument, with no shell.
/// A normalized URL starts with its scheme, so it can't be taken for an
/// option.
pub fn open(url: &Url) -> io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = Command::new(opener)
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Reaped in the background: the opener exits as soon as it has handed over.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Why a `file:` link isn't revealed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RevealError {
    #[error("the path isn't absolute")]
    NotAbsolute,
    #[error("the path doesn't exist")]
    Missing,
    #[error("the path is outside this chat's folder ({0})")]
    Outside(String),
}

/// A `file:` link from agent text (contract §12): the path is resolved,
/// symlinks included, and accepted only when it is inside `folder` (the
/// thread's cwd, itself resolved). The result is what gets revealed, never
/// opened or run.
pub fn check_reveal(path: &str, folder: &Path) -> Result<PathBuf, RevealError> {
    let path = Path::new(path);
    if !path.is_absolute() || path.as_os_str().len() > MAX_LINK_BYTES {
        return Err(RevealError::NotAbsolute);
    }
    let resolved = std::fs::canonicalize(path).map_err(|_| RevealError::Missing)?;
    let folder = std::fs::canonicalize(folder)
        .map_err(|_| RevealError::Outside(folder.display().to_string()))?;
    if resolved.starts_with(&folder) {
        Ok(resolved)
    } else {
        Err(RevealError::Outside(folder.display().to_string()))
    }
}

/// Shows `path` in the file manager: selected in Finder on macOS, its folder
/// on Linux. Never opens the file itself.
pub fn reveal(path: &Path) -> io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg("-R").arg("--").arg(path);
        command
    } else {
        let folder = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let mut command = Command::new("xdg-open");
        command.arg(folder);
        command
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_inside_the_folder_is_revealed_resolved() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("notes.md");
        std::fs::write(&file, "x").unwrap();
        let resolved = check_reveal(file.to_str().unwrap(), folder.path()).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&file).unwrap());
        let dotted = format!("{}/./sub/../notes.md", folder.path().display());
        std::fs::create_dir(folder.path().join("sub")).unwrap();
        assert!(check_reveal(&dotted, folder.path()).is_ok());
    }

    #[test]
    fn a_symlink_out_of_the_folder_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret");
        std::fs::write(&target, "x").unwrap();
        let link = folder.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(
            check_reveal(link.to_str().unwrap(), folder.path()),
            Err(RevealError::Outside(_))
        ));
        let escape = format!("{}/../{}", folder.path().display(), "x");
        assert!(check_reveal(&escape, folder.path()).is_err());
    }

    #[test]
    fn relative_and_missing_paths_are_refused() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(
            check_reveal("notes.md", folder.path()),
            Err(RevealError::NotAbsolute)
        );
        let missing = folder.path().join("nope");
        assert_eq!(
            check_reveal(missing.to_str().unwrap(), folder.path()),
            Err(RevealError::Missing)
        );
    }

    #[test]
    fn web_and_mail_links_pass() {
        assert_eq!(
            check("https://example.com/a?b=c#d").unwrap().as_str(),
            "https://example.com/a?b=c#d"
        );
        assert_eq!(
            check("HTTP://Example.com").unwrap().as_str(),
            "http://example.com/"
        );
        assert!(check("mailto:someone@example.com").is_ok());
    }

    #[test]
    fn every_other_scheme_is_refused() {
        for link in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>1</script>",
            "tauri://localhost/index.html",
            "ipc://localhost/daemon_command",
            "vscode://file/x",
            "x-apple.systempreferences:com.apple.preference.security",
            "smb://server/share",
            "ftp://example.com/",
        ] {
            assert!(matches!(check(link), Err(LinkError::Scheme(_))), "{link}");
        }
    }

    #[test]
    fn what_is_not_a_link_is_refused() {
        for link in [
            "",
            "example.com",
            "/etc/passwd",
            "--help",
            "https://",
            "https://exa\nmple.com",
            "ht tp://x",
        ] {
            assert!(check(link).is_err(), "{link:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_LINK_BYTES));
        assert_eq!(check(&long), Err(LinkError::TooLong));
    }

    #[test]
    fn an_accepted_link_never_starts_like_an_option() {
        for link in [
            "https://-example.com/",
            "mailto:-x@example.com",
            "http://example.com/ --flag",
        ] {
            if let Ok(url) = check(link) {
                assert!(!url.as_str().starts_with('-'), "{url}");
                assert!(!url.as_str().contains(' '), "{url}");
            }
        }
    }
}
