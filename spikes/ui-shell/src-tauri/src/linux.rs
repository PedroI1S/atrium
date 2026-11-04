//! Linux diagnostics for the WebKitGTK checks (D-037, `docs/spikes/M0-linux-checks.md`).
//!
//! WebKitGTK paces page rendering updates near 60 fps by default, even on a faster display
//! (feature `PreferPageRenderingUpdatesNear60FPS`, on by default in 2.54). `ATRIUM_WEBKIT_NATIVE_FPS=1`
//! turns it off for every webview so a run can measure the display's own rate. Off by default.
//! Each window also logs the monitor it landed on, so a high-refresh run can be attributed.
use std::ffi::{CStr, c_char, c_int, c_void};

use gtk::glib::object::ObjectType;
use gtk::prelude::*;
use tauri::webview::PlatformWebview;

const FEATURE: &str = "PreferPageRenderingUpdatesNear60FPS";

// Available since WebKitGTK 2.42; the library is already linked through webkit2gtk-sys.
#[link(name = "webkit2gtk-4.1")]
unsafe extern "C" {
    fn webkit_web_view_get_settings(view: *mut c_void) -> *mut c_void;
    fn webkit_settings_get_all_features() -> *mut c_void;
    fn webkit_feature_list_get_length(list: *mut c_void) -> usize;
    fn webkit_feature_list_get(list: *mut c_void, index: usize) -> *mut c_void;
    fn webkit_feature_list_unref(list: *mut c_void);
    fn webkit_feature_get_identifier(feature: *mut c_void) -> *const c_char;
    fn webkit_settings_get_feature_enabled(settings: *mut c_void, feature: *mut c_void) -> c_int;
    fn webkit_settings_set_feature_enabled(settings: *mut c_void, feature: *mut c_void, enabled: c_int);
}

/// Runs `f` with the view's settings and the feature, if this WebKitGTK has it.
fn with_feature<T>(webview: &PlatformWebview, f: impl FnOnce(*mut c_void, *mut c_void) -> T) -> Option<T> {
    // SAFETY: called on the main thread inside `with_webview`; the view outlives the call, and the
    // feature list is unreffed after use (features are owned by the list).
    unsafe {
        let settings = webkit_web_view_get_settings(webview.inner().as_ptr().cast());
        let list = webkit_settings_get_all_features();
        let found = (0..webkit_feature_list_get_length(list))
            .map(|i| webkit_feature_list_get(list, i))
            .find(|&ft| CStr::from_ptr(webkit_feature_get_identifier(ft)).to_bytes() == FEATURE.as_bytes());
        let out = found.map(|ft| f(settings, ft));
        webkit_feature_list_unref(list);
        out
    }
}

/// Applies `ATRIUM_WEBKIT_NATIVE_FPS` to a new webview.
pub fn configure(webview: PlatformWebview) {
    if std::env::var("ATRIUM_WEBKIT_NATIVE_FPS").as_deref() == Ok("1") {
        // SAFETY: as in `with_feature`.
        with_feature(&webview, |s, ft| unsafe { webkit_settings_set_feature_enabled(s, ft, 0) });
    }
}

/// "monitor <model> <W>x<H>+<X>+<Y> @<Hz>, PreferPageRenderingUpdatesNear60FPS=<on|off>".
pub fn describe(webview: &PlatformWebview) -> String {
    let view = webview.inner();
    let monitor = view
        .window()
        .and_then(|w| view.display().monitor_at_window(&w))
        .map_or_else(
            || "unknown".to_string(),
            |m| {
                let g = m.geometry();
                let model = m.model().map_or_else(|| "?".to_string(), |s| s.to_string());
                format!("{model} {}x{}+{}+{} @{:.3} Hz", g.width(), g.height(), g.x(), g.y(), m.refresh_rate() as f64 / 1000.0)
            },
        );
    // SAFETY: as in `with_feature`.
    let pref = with_feature(webview, |s, ft| unsafe { webkit_settings_get_feature_enabled(s, ft) != 0 });
    let pref = pref.map_or("absent", |on| if on { "on" } else { "off" });
    format!("monitor {monitor}, {FEATURE}={pref}")
}
