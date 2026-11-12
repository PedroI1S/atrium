//! Links in agent text (contract §12). Agent text is untrusted: a link is
//! opened in the system browser only when its scheme is `http`, `https` or
//! `mailto`. Everything else is refused here, whatever the webview asked.

use std::io;
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

#[cfg(test)]
mod tests {
    use super::*;

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
