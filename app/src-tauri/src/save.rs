//! What the webview can't do by itself for the M1 UI (M1-23): the native
//! save dialog behind "Export…", and the read-only safety lists Settings
//! shows. The daemon still writes the export file (`ExportThread`, contract
//! §13): this module only asks the user where.

use atrium_protocol::policy::HARD_DENY_DEFAULTS;
use serde::Serialize;
use tauri::AppHandle;

/// One entry of the hard-deny list, as Settings shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HardDeny {
    pub id: &'static str,
    pub description: &'static str,
}

/// What Settings → Security lists, read-only.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyInfo {
    /// `HARD_DENY_DEFAULTS` (D-050), in list order.
    pub hard_deny: Vec<HardDeny>,
    /// The home folder, so the rules editor can write `~/…` globs as the
    /// absolute paths the policy engine matches.
    pub home: Option<String>,
}

pub fn safety_info() -> SafetyInfo {
    SafetyInfo {
        hard_deny: HARD_DENY_DEFAULTS
            .iter()
            .map(|d| HardDeny {
                id: d.id,
                description: d.description,
            })
            .collect(),
        home: std::env::home_dir()
            .filter(|home| !home.as_os_str().is_empty())
            .map(|home| home.to_string_lossy().into_owned()),
    }
}

/// The file the user picked in the save dialog.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SavePick {
    pub path: String,
    /// Something is already there. The daemon writes an export only as a new
    /// file (contract §13), so the app asks again rather than letting the
    /// export fail after the dialog's own "Replace?" (M1-23 round 2, minor 5).
    pub exists: bool,
}

impl SavePick {
    fn at(path: String) -> Self {
        let exists = std::fs::symlink_metadata(&path).is_ok();
        Self { path, exists }
    }
}

/// What the save dialog says under its title: an export never replaces a
/// file. macOS only: GTK's chooser has no message line.
#[cfg(target_os = "macos")]
const SAVE_MESSAGE: &str = "Atrium never replaces a file: pick a new name.";

/// Asks where to save a file named `name` (the user may change it). `None`
/// when the user cancels. Runs the platform's own dialog on the main thread
/// and waits for it.
pub async fn ask_save_path(
    app: &AppHandle,
    title: String,
    name: String,
) -> Result<Option<SavePick>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = sender.send(run_dialog(&title, &name));
    })
    .map_err(|error| error.to_string())?;
    receiver
        .await
        .map(|path| path.map(SavePick::at))
        .map_err(|_| "the save dialog closed unexpectedly".to_string())
}

#[cfg(target_os = "macos")]
fn run_dialog(title: &str, name: &str) -> Option<String> {
    use objc2_app_kit::{NSModalResponseOK, NSSavePanel};
    use objc2_foundation::{MainThreadMarker, NSString};

    let mtm = MainThreadMarker::new()?;
    let panel = NSSavePanel::savePanel(mtm);
    panel.setTitle(Some(&NSString::from_str(title)));
    // NSSavePanel always asks "Replace?" for an existing file and has no
    // public switch for it: the message says up front that Atrium won't, and
    // the app asks again if the user picks one anyway.
    panel.setMessage(Some(&NSString::from_str(SAVE_MESSAGE)));
    panel.setNameFieldStringValue(&NSString::from_str(name));
    panel.setCanCreateDirectories(true);
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    panel
        .URL()
        .and_then(|url| url.path())
        .map(|path| path.to_string())
}

/// GTK 3's portal-aware native chooser (M1-23 round 2, major 3): `run()`
/// shows it modally and hides it before returning, and dropping the last
/// reference frees it, so no dialog is left on screen. (A
/// `GtkFileChooserDialog` closed with `close()` after `run()` stays visible:
/// `GtkDialog`'s delete-event handler stops the close.) Not built on this
/// Mac: checked against the gtk 0.18 API by reading, not by compiling.
#[cfg(target_os = "linux")]
fn run_dialog(title: &str, name: &str) -> Option<String> {
    use gtk::prelude::*;

    let dialog = gtk::FileChooserNative::new(
        Some(title),
        None::<&gtk::Window>,
        gtk::FileChooserAction::Save,
        Some("_Save"),
        Some("_Cancel"),
    );
    // The app asks again for an existing file; GTK's "Replace?" would only
    // lead to a refused export.
    dialog.set_do_overwrite_confirmation(false);
    dialog.set_current_name(name);
    let path = if dialog.run() == gtk::ResponseType::Accept {
        dialog.filename()
    } else {
        None
    };
    path.map(|path| path.to_string_lossy().into_owned())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn run_dialog(_title: &str, _name: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_info_lists_every_hard_deny_default_in_order() {
        let info = safety_info();
        let ids: Vec<&str> = info.hard_deny.iter().map(|d| d.id).collect();
        let expected: Vec<&str> = HARD_DENY_DEFAULTS.iter().map(|d| d.id).collect();
        assert_eq!(ids, expected);
        assert!(info.hard_deny.iter().all(|d| !d.description.is_empty()));
        let json = serde_json::to_value(&info).unwrap();
        assert!(json["hardDeny"][0]["description"].is_string());
    }

    #[test]
    fn a_save_pick_says_whether_something_is_already_there() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chat.md");
        std::fs::write(&file, "x").unwrap();
        let taken = SavePick::at(file.to_string_lossy().into_owned());
        assert!(taken.exists);
        let new = SavePick::at(dir.path().join("new.md").to_string_lossy().into_owned());
        assert!(!new.exists);
        // A dangling symlink is "something": the daemon's create_new refuses it too.
        #[cfg(unix)]
        {
            let link = dir.path().join("link.md");
            std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
            assert!(SavePick::at(link.to_string_lossy().into_owned()).exists);
        }
        let json = serde_json::to_value(&new).unwrap();
        assert_eq!(json["exists"], false);
        assert!(json["path"].is_string());
    }
}
