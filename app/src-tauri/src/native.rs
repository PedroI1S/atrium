//! The native half of a window's appearance: vibrancy behind the window and
//! the window's own alpha (ui-direction §5, contract §11.1).
//!
//! Translucent windows exist on macOS only. On Linux windows are opaque (L1),
//! so every function here answers "opaque" and touches nothing.

use serde::Serialize;

use crate::shell::{OpacityMode, PopoutOpacity, Settings, Theme};

/// What a window shows natively right now.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeState {
    pub vibrancy: bool,
    /// Which vibrancy material: the dark one or the light one.
    pub dark: bool,
    pub alpha: f64,
}

impl Default for NativeState {
    fn default() -> Self {
        Self {
            vibrancy: false,
            dark: false,
            alpha: 1.0,
        }
    }
}

/// Whether this platform can draw a translucent window at all.
pub const fn translucency_supported() -> bool {
    cfg!(target_os = "macos")
}

/// What a window should show natively. Mirrors `appearanceFor` in
/// `src/window-model.ts`; the frontend reconciles any difference after it
/// mounts, so this only gets the first frame right.
pub fn native_target(
    settings: &Settings,
    popout: bool,
    reduce: bool,
    system_dark: bool,
    supported: bool,
) -> NativeState {
    let opaque = !supported || reduce || (popout && settings.popouts == PopoutOpacity::Opaque);
    let alpha = if opaque { 1.0 } else { settings.alpha };
    let background = settings.mode == OpacityMode::Background;
    NativeState {
        vibrancy: background && alpha < 1.0 && settings.blur,
        dark: match settings.theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => system_dark,
        },
        alpha: if background { 1.0 } else { alpha },
    }
}

/// The window theme to force; `None` follows the system.
pub fn window_theme(settings: &Settings) -> Option<tauri::Theme> {
    match settings.theme {
        Theme::Light => Some(tauri::Theme::Light),
        Theme::Dark => Some(tauri::Theme::Dark),
        Theme::System => None,
    }
}

/// Whether the OS asks for opaque surfaces (Reduce transparency, Increase
/// contrast).
pub fn accessibility_opaque() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::macos::accessibility_opaque()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Turns vibrancy on or off. Returns whether it is on now.
pub fn set_vibrancy(
    window: &tauri::WebviewWindow,
    enabled: bool,
    dark: bool,
) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::set_vibrancy(window, enabled, dark)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, enabled, dark);
        Ok(false)
    }
}

/// Sets the window's alpha from any thread. Returns the alpha it has now.
pub fn set_alpha(window: &tauri::WebviewWindow, alpha: f64) -> Result<f64, String> {
    if !alpha.is_finite() || !(crate::shell::MIN_ALPHA..=1.0).contains(&alpha) {
        return Err("alpha outside 0.3..1".into());
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::set_alpha(window, alpha)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Ok(1.0)
    }
}

/// Applies `target` to a new window before its webview has loaded, on the
/// main thread, so the first frame is right. Returns what was applied.
pub fn apply_to_new_window(window: &tauri::WebviewWindow, target: NativeState) -> NativeState {
    #[cfg(target_os = "macos")]
    {
        crate::macos::apply_to_new_window(window, target)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, target);
        NativeState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_native_target_follows_the_opacity_rules() {
        let background = Settings {
            alpha: 0.6,
            theme: Theme::Dark,
            ..Settings::default()
        };
        let target = native_target(&background, false, false, false, true);
        assert_eq!(
            (target.vibrancy, target.alpha, target.dark),
            (true, 1.0, true)
        );
        let whole = Settings {
            mode: OpacityMode::Whole,
            ..background.clone()
        };
        let target = native_target(&whole, false, false, false, true);
        assert_eq!((target.vibrancy, target.alpha), (false, 0.6));
        let opaque_popouts = Settings {
            popouts: PopoutOpacity::Opaque,
            ..whole.clone()
        };
        assert_eq!(
            native_target(&opaque_popouts, true, false, false, true).alpha,
            1.0
        );
        assert_eq!(
            native_target(&opaque_popouts, false, false, false, true).alpha,
            0.6,
            "main is not a pop-out"
        );
        assert!(
            !native_target(&background, false, true, false, true).vibrancy,
            "accessibility forces opaque"
        );
        assert!(
            !native_target(
                &Settings {
                    blur: false,
                    ..background.clone()
                },
                false,
                false,
                false,
                true
            )
            .vibrancy
        );
        let system = Settings {
            theme: Theme::System,
            ..background.clone()
        };
        assert!(native_target(&system, false, false, true, true).dark);
        assert!(!native_target(&system, false, false, false, true).dark);
    }

    #[test]
    fn a_platform_without_translucency_is_always_opaque() {
        let settings = Settings {
            alpha: 0.4,
            mode: OpacityMode::Whole,
            ..Settings::default()
        };
        assert_eq!(
            native_target(&settings, false, false, false, false).alpha,
            1.0
        );
        let settings = Settings {
            alpha: 0.4,
            ..Settings::default()
        };
        assert!(!native_target(&settings, false, false, false, false).vibrancy);
    }
}
