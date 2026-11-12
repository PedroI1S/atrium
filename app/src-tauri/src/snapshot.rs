//! Screenshots for reviews and reports, taken from inside the app.
//!
//! A desktop session doesn't let a script capture windows (GNOME refuses,
//! and Wayland has no general way), so a harness run asks its own webview
//! for a snapshot of what it shows. On Linux the app draws the whole window,
//! controls included, so the snapshot is the window. On macOS the window
//! server captures the window instead (see `take`).
//!
//! Only in a run that sets `ATRIUM_SHOT_DIR`, and only into that folder: the
//! webview names the picture, never the path.

use std::path::PathBuf;

/// `<ATRIUM_SHOT_DIR>/<name>.png`, when the run asked for screenshots and
/// `name` is a plain word.
pub fn target(name: &str) -> Result<PathBuf, String> {
    let folder = std::env::var_os("ATRIUM_SHOT_DIR")
        .filter(|folder| !folder.is_empty())
        .ok_or("this run takes no screenshots (ATRIUM_SHOT_DIR is not set)")?;
    let plain = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !plain {
        return Err("a screenshot's name is lowercase letters, digits and dashes".into());
    }
    Ok(PathBuf::from(folder).join(format!("{name}.png")))
}

/// Writes what `window` shows to `path`. The picture is written a moment
/// later, on the main thread; `ATRIUM_SNAPSHOT <path> <ok|error…>` on stdout
/// says when.
#[cfg(target_os = "linux")]
pub fn take(window: &tauri::WebviewWindow, path: PathBuf) -> Result<(), String> {
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

    window
        .with_webview(move |webview| {
            webview.inner().snapshot(
                SnapshotRegion::Visible,
                SnapshotOptions::NONE,
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    let written = result
                        .map_err(|error| error.to_string())
                        .and_then(|surface| {
                            let mut file =
                                std::fs::File::create(&path).map_err(|error| error.to_string())?;
                            surface
                                .write_to_png(&mut file)
                                .map_err(|error| error.to_string())
                        });
                    match written {
                        Ok(()) => println!("ATRIUM_SNAPSHOT {} ok", path.display()),
                        Err(error) => println!("ATRIUM_SNAPSHOT {} error: {error}", path.display()),
                    }
                },
            );
        })
        .map_err(|error| error.to_string())
}

/// On macOS the window server captures the window itself (`screencapture
/// -l`), so the picture shows what the webview can't: the translucency, the
/// vibrancy and the traffic lights. The first capture asks the user for
/// Screen Recording permission for the app the harness runs from.
#[cfg(target_os = "macos")]
pub fn take(window: &tauri::WebviewWindow, path: PathBuf) -> Result<(), String> {
    // Synchronous: the harness moves on to its next step when this returns,
    // so the capture must be finished by then (an asynchronous one caught
    // the next step's state).
    let number = crate::macos::window_number(window)?;
    let status = std::process::Command::new("/usr/sbin/screencapture")
        .arg("-x")
        .arg(format!("-l{number}"))
        .arg(&path)
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() || !path.is_file() {
        let error = format!("screencapture {status}");
        println!("ATRIUM_SNAPSHOT {} error: {error}", path.display());
        return Err(error);
    }
    println!("ATRIUM_SNAPSHOT {} ok", path.display());
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn take(_window: &tauri::WebviewWindow, _path: PathBuf) -> Result<(), String> {
    Err("screenshots from inside the app are implemented on Linux and macOS only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_a_plain_word_and_the_folder_comes_from_the_environment() {
        // One test: the environment is shared by the process.
        // SAFETY-free: `set_var` is unsafe in this edition, so the variable is
        // only read here; the folder cases are covered by running the harness.
        if std::env::var_os("ATRIUM_SHOT_DIR").is_none() {
            assert!(target("main").is_err(), "no folder, no screenshot");
            return;
        }
        assert!(target("main-dark").is_ok());
        for bad in ["", "../x", "a/b", "A", "x.png", "a b", &"x".repeat(65)] {
            assert!(target(bad).is_err(), "{bad:?}");
        }
    }
}
