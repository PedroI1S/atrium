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

/// Asks where to save a file named `name` (the user may change it). `None`
/// when the user cancels. Runs the platform's own dialog on the main thread
/// and waits for it.
pub async fn ask_save_path(
    app: &AppHandle,
    title: String,
    name: String,
) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = sender.send(run_dialog(&title, &name));
    })
    .map_err(|error| error.to_string())?;
    receiver
        .await
        .map_err(|_| "the save dialog closed unexpectedly".to_string())
}

#[cfg(target_os = "macos")]
fn run_dialog(title: &str, name: &str) -> Option<String> {
    use objc2_app_kit::{NSModalResponseOK, NSSavePanel};
    use objc2_foundation::{MainThreadMarker, NSString};

    let mtm = MainThreadMarker::new()?;
    let panel = NSSavePanel::savePanel(mtm);
    panel.setTitle(Some(&NSString::from_str(title)));
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

#[cfg(target_os = "linux")]
fn run_dialog(title: &str, name: &str) -> Option<String> {
    use gtk::prelude::*;

    let dialog = gtk::FileChooserDialog::with_buttons(
        Some(title),
        None::<&gtk::Window>,
        gtk::FileChooserAction::Save,
        &[
            ("_Cancel", gtk::ResponseType::Cancel),
            ("_Save", gtk::ResponseType::Accept),
        ],
    );
    dialog.set_do_overwrite_confirmation(true);
    dialog.set_current_name(name);
    let response = dialog.run();
    let path = if response == gtk::ResponseType::Accept {
        dialog.filename()
    } else {
        None
    };
    dialog.close();
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
}
