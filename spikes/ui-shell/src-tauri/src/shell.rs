//! Spike-only shared state: no daemon contract changes. All mutations are serialized.
use serde_json::{Value, json};
use std::sync::Mutex;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub struct Shell(pub Mutex<Value>);

pub fn initial(app: &tauri::AppHandle) -> Value {
    let mut settings = json!({"theme":"atrium-calm-dark", "alpha":1.0, "mode":"background", "solid":false, "blur":true});
    if let Ok(dir) = app.path().app_config_dir()
        && let Ok(bytes) = std::fs::read(dir.join("calm-settings.json"))
        && let Ok(saved) = serde_json::from_slice::<Value>(&bytes)
        && valid_settings(&saved)
    {
        settings = saved;
    }
    if std::env::var("ATRIUM_WINDOWS").is_ok() {
        settings = json!({"theme":"atrium-calm-dark", "alpha":1.0, "mode":"background", "solid":false, "blur":true});
        if let Ok(mode) = std::env::var("ATRIUM_OPACITY") {
            settings["mode"] = json!(if mode == "whole" {
                "whole"
            } else {
                "background"
            });
            settings["alpha"] = json!(if mode == "off" { 1.0 } else { 0.6 });
        }
    }
    if let Ok(theme) = std::env::var("ATRIUM_THEME")
        && ["atrium-calm-dark", "atrium-calm-light"].contains(&theme.as_str())
    {
        settings["theme"] = json!(theme);
    }
    let mut views = json!({});
    for id in ["chat", "editor", "database"] {
        views[id] = json!({"threadId":"t-auth", "owner":"main", "draft":"", "messages":[], "scroll":-1,
            "content": if id == "editor" { "// session.rs\nfn refresh_token() {\n    // Retry once before asking the user.\n}\n" } else { "select * from sessions;" }});
    }
    json!({"revision":0, "settings":settings, "views":views})
}
fn valid_settings(v: &Value) -> bool {
    matches!(
        v["theme"].as_str(),
        Some("atrium-calm-dark" | "atrium-calm-light")
    ) && matches!(v["mode"].as_str(), Some("background" | "whole"))
        && v["alpha"]
            .as_f64()
            .is_some_and(|a| (0.3..=1.0).contains(&a))
        && v["solid"].is_boolean()
        && v["blur"].is_boolean()
}
fn publish(app: &tauri::AppHandle, value: &mut Value) -> Result<Value, String> {
    value["revision"] = json!(value["revision"].as_u64().unwrap_or(0) + 1);
    app.emit("shell-state", &*value)
        .map_err(|e| e.to_string())?;
    Ok(value.clone())
}
#[tauri::command]
pub fn shell_snapshot(shell: State<'_, Shell>) -> Value {
    shell.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn shell_patch(
    app: tauri::AppHandle,
    window: WebviewWindow,
    shell: State<'_, Shell>,
    view: Option<String>,
    patch: Value,
) -> Result<Value, String> {
    let mut state = shell.0.lock().unwrap();
    if let Some(id) = view {
        let target = &mut state["views"][&id];
        if target.is_null() || target["owner"] != window.label() {
            return Err("view is owned by another window".into());
        }
        let fields = patch.as_object().ok_or("patch must be an object")?;
        for (k, v) in fields {
            let valid = match k.as_str() {
                "draft" | "content" | "threadId" => v.is_string(),
                "scroll" => v.as_f64().is_some_and(|n| n.is_finite()),
                "messages" => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
                _ => false,
            };
            if !valid {
                return Err("invalid view patch".into());
            }
        }
        for (k, v) in fields {
            target[k] = v.clone();
        }
    } else {
        let mut next = state["settings"].clone();
        for (k, v) in patch.as_object().ok_or("patch must be an object")? {
            next[k] = v.clone();
        }
        if !valid_settings(&next) {
            return Err("invalid appearance settings".into());
        }
        if std::env::var("ATRIUM_WINDOWS").is_err() {
            let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("calm-settings.json"), next.to_string())
                .map_err(|e| e.to_string())?;
        }
        state["settings"] = next;
    }
    publish(&app, &mut state)
}

#[tauri::command]
pub async fn set_window_alpha(window: WebviewWindow, alpha: f64) -> Result<f64, String> {
    if !alpha.is_finite() || !(0.3..=1.0).contains(&alpha) {
        return Err("alpha outside 0.3..1".into());
    }
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        window
            .with_webview(move |webview| {
                // Tauri guarantees this closure runs on the main thread. Pointer is borrowed
                // only for this callback; it belongs to the still-live WebviewWindow.
                let native = unsafe { &*webview.ns_window().cast::<objc2_app_kit::NSWindow>() };
                native.setAlphaValue(alpha);
                let _ = tx.send(native.alphaValue());
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Ok(1.0)
    } // Explicit opaque fallback; no compositor assumptions.
}

#[tauri::command]
pub async fn pop_out(app: tauri::AppHandle, view: String) -> Result<Value, String> {
    if !["chat", "editor", "database"].contains(&view.as_str()) {
        return Err("unknown view".into());
    }
    let label = format!("view-{view}");
    if let Some(w) = app.get_webview_window(&label) {
        w.set_focus().map_err(|e| e.to_string())?;
        return Ok(app.state::<Shell>().0.lock().unwrap().clone());
    }
    // Publish the ownership before the new webview can take its initial snapshot.
    {
        let shell = app.state::<Shell>();
        let mut state = shell.0.lock().unwrap();
        if state["views"][&view]["owner"] != "main" {
            return Err("view is already moving".into());
        }
        state["views"][&view]["owner"] = json!(label);
        publish(&app, &mut state)?;
    }
    let builder = WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?view={view}").into()),
    )
    .title(format!("Atrium · {view}"))
    .inner_size(760.0, 680.0)
    .min_inner_size(480.0, 420.0);
    #[cfg(target_os = "macos")]
    let builder = builder
        .transparent(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    if let Err(e) = builder.build() {
        return_view(&app, &label);
        return Err(e.to_string());
    }
    Ok(app.state::<Shell>().0.lock().unwrap().clone())
}

pub fn return_view(app: &tauri::AppHandle, label: &str) {
    let shell = app.state::<Shell>();
    let mut state = shell.0.lock().unwrap();
    restore_owned_views(&mut state, label);
    let _ = publish(app, &mut state);
}
fn restore_owned_views(state: &mut Value, label: &str) {
    if let Some(views) = state["views"].as_object_mut() {
        for value in views.values_mut() {
            if value["owner"] == label {
                value["owner"] = json!("main");
            }
        }
    }
}

#[tauri::command]
pub async fn bring_back(
    app: tauri::AppHandle,
    window: WebviewWindow,
    view: String,
) -> Result<Value, String> {
    let label = format!("view-{view}");
    if window.label() != label {
        return Err("only the owning window can return a live view".into());
    }
    if let Some(w) = app.get_webview_window(&label) {
        w.destroy().map_err(|e| e.to_string())?;
    }
    return_view(&app, &label);
    Ok(app.state::<Shell>().0.lock().unwrap().clone())
}

#[derive(Default)]
pub struct Ready(pub Mutex<std::collections::HashSet<String>>);
#[tauri::command]
pub fn window_ready(window: WebviewWindow, ready: State<'_, Ready>) -> usize {
    let mut labels = ready.0.lock().unwrap();
    labels.insert(window.label().to_string());
    labels.len()
}
#[tauri::command]
pub fn ready_count(ready: State<'_, Ready>) -> usize {
    ready.0.lock().unwrap().len()
}

#[tauri::command]
pub fn opaque_accessibility() -> bool {
    #[cfg(target_os = "macos")]
    {
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        workspace.accessibilityDisplayShouldReduceTransparency()
            || workspace.accessibilityDisplayShouldIncreaseContrast()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_close_preserves_serialized_view_state() {
        for id in ["chat", "editor", "database"] {
            let label = format!("view-{id}");
            let before = json!({"owner":"main", "draft":"unfinished", "messages":["hello"],
                "scroll":425, "content":"edited", "threadId":"t-flaky"});
            let mut state = json!({"views": {id: before.clone(), "other": {"owner":"main", "content":"untouched"}}});
            state["views"][id]["owner"] = json!(label);
            let mut received: Value = serde_json::from_str(&state.to_string()).unwrap();
            restore_owned_views(&mut received, &label);
            assert_eq!(received["views"][id], before);
            assert_eq!(received["views"]["other"], state["views"]["other"]);
        }
    }
    #[test]
    fn persisted_settings_reject_invalid_modes_and_alpha() {
        let mut value = json!({"theme":"atrium-calm-light", "alpha":0.6, "mode":"whole", "solid":true, "blur":false});
        assert!(valid_settings(&value));
        value["alpha"] = json!(0.2);
        assert!(!valid_settings(&value));
        value["alpha"] = json!(0.6);
        value["mode"] = json!("css-opacity");
        assert!(!valid_settings(&value));
    }
}
