//! AppKit calls for translucent windows (ui-direction §5): the window's
//! alpha, vibrancy, and the accessibility display options.
//!
//! This is the one module of the app with `unsafe` (CR-11, D-061): three
//! dereferences of the `NSWindow` pointer Tauri hands out, and registering a
//! notification observer. Ported from the M0-06b spike; audited by Opus 5.5
//! (`docs/agents/reviews/M1-09a-app-shell-unsafe.md`). New `unsafe` needs its
//! own change request.
#![allow(unsafe_code)]

use std::time::Duration;

use tauri::Emitter;
use window_vibrancy::{
    NSVisualEffectMaterial, NSVisualEffectState, apply_vibrancy, clear_vibrancy,
};

use crate::native::NativeState;

/// Corner radius of the vibrancy view: the macOS window radius, so the blur
/// doesn't show outside the rounded corners.
const WINDOW_CORNER_RADIUS: f64 = 10.0;

pub fn accessibility_opaque() -> bool {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    workspace.accessibilityDisplayShouldReduceTransparency()
        || workspace.accessibilityDisplayShouldIncreaseContrast()
}

pub fn set_vibrancy(
    window: &tauri::WebviewWindow,
    enabled: bool,
    dark: bool,
) -> Result<bool, String> {
    if enabled {
        let material = if dark {
            NSVisualEffectMaterial::UnderWindowBackground
        } else {
            NSVisualEffectMaterial::Sidebar
        };
        apply_vibrancy(
            window,
            material,
            Some(NSVisualEffectState::Active),
            Some(WINDOW_CORNER_RADIUS),
        )
        .map_err(|error| error.to_string())?;
        Ok(true)
    } else {
        clear_vibrancy(window)
            .map_err(|error| error.to_string())
            .map(|_| false)
    }
}

pub fn set_alpha(window: &tauri::WebviewWindow, alpha: f64) -> Result<f64, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    window
        .with_webview(move |webview| {
            // SAFETY: Tauri runs this closure on the main thread and the
            // pointer is the live window of this webview, borrowed only for
            // the closure.
            let native = unsafe { &*webview.ns_window().cast::<objc2_app_kit::NSWindow>() };
            native.setAlphaValue(alpha);
            let _ = sender.send(native.alphaValue());
        })
        .map_err(|error| error.to_string())?;
    receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())
}

/// The window server's number for this window, for `screencapture -l`
/// (harness screenshots, `snapshot.rs`).
pub fn window_number(window: &tauri::WebviewWindow) -> Result<isize, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    window
        .with_webview(move |webview| {
            // SAFETY: as in `set_alpha`: main thread, live window, borrowed
            // only for the closure.
            let native = unsafe { &*webview.ns_window().cast::<objc2_app_kit::NSWindow>() };
            let _ = sender.send(native.windowNumber());
        })
        .map_err(|error| error.to_string())?;
    receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())
}

pub fn apply_to_new_window(window: &tauri::WebviewWindow, target: NativeState) -> NativeState {
    let mut applied = NativeState::default();
    if target.vibrancy {
        match set_vibrancy(window, true, target.dark) {
            Ok(on) => {
                applied.vibrancy = on;
                applied.dark = target.dark;
            }
            Err(error) => eprintln!("atrium: vibrancy failed: {error}"),
        }
    }
    // `NSWindow` is main-thread only: every caller today (setup, `pop_out`,
    // `restore_windows`) runs there, and this check keeps a future caller
    // from making the reference anywhere else.
    if target.alpha < 1.0
        && objc2_foundation::MainThreadMarker::new().is_some()
        && let Ok(pointer) = window.ns_window()
    {
        // SAFETY: on the main thread (checked above), with the pointer of a
        // window that was just built and is still alive, borrowed only here.
        let native = unsafe { &*pointer.cast::<objc2_app_kit::NSWindow>() };
        native.setAlphaValue(target.alpha);
        applied.alpha = native.alphaValue();
    }
    applied
}

/// Tells every window when Reduce transparency or Increase contrast change,
/// for the life of the process.
pub fn observe_accessibility(app: tauri::AppHandle) {
    use block2::RcBlock;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
    use objc2_foundation::NSNotification;

    let block = RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
        let _ = app.emit("accessibility-changed", accessibility_opaque());
    });
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: a valid notification name and no object filter. With `queue`
    // None the block runs on whichever thread posts the notification, so it
    // must be sendable: it captures only an `AppHandle` (`Send + Sync`) and
    // only emits a Tauri event, which is safe from any thread.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(token);
}
