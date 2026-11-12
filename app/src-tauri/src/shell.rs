//! What the app's windows share, and what must outlive any one of them.
//!
//! Rust is the single writer: windows send patches, and every structural
//! change (settings, which window shows which thread) is published to all
//! windows with a new revision. A view's own state (its draft, its scroll
//! position) is written by the one window that owns the view and is not
//! broadcast, since no other window shows it.
//!
//! The rules are plain functions over [`ShellState`], tested here without
//! Tauri. `windows.rs` holds the glue.
//!
//! Nothing here comes from the daemon except thread ids, which are only ever
//! used as map keys: a window label is made from a counter (contract §12).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The main window's label.
pub const MAIN: &str = "main";
/// The view key of the composer for a thread that doesn't exist yet.
pub const NEW_THREAD: &str = "@new";
/// Lower bound of the opacity slider (ui-direction §5: text stays readable).
pub const MIN_ALPHA: f64 = 0.3;
/// Upper bound of the overlay blur strength, in px (ui-direction §3).
pub const MAX_BLUR_STRENGTH: u32 = 48;
/// One draft. A message can't be larger than a client frame anyway.
pub const MAX_DRAFT_BYTES: usize = 1024 * 1024;
/// All drafts together; a patch that would pass it is refused.
pub const MAX_DRAFTS_BYTES: usize = 8 * 1024 * 1024;
/// Views with state. Past it the least recently touched one is dropped,
/// those without a draft first.
pub const MAX_VIEWS: usize = 512;
pub const MAX_POPOUTS: usize = 16;
/// Quiet period before settings or the session are written; a slider drag or
/// a burst of typing writes once.
pub const PERSIST_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    Light,
    Dark,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpacityMode {
    /// Surfaces turn translucent; text and controls stay opaque.
    Background,
    /// The native window alpha: everything fades together.
    Whole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PopoutOpacity {
    Same,
    Opaque,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    Small,
    Default,
    Large,
}

/// Appearance settings. They describe the app's windows, so they live with
/// the app, not in the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    pub alpha: f64,
    pub mode: OpacityMode,
    /// Keep reading areas solid: only the frame fades.
    pub solid: bool,
    pub blur: bool,
    /// Blur of in-window overlays (palette, popovers), in px. `None` uses the
    /// theme's `glass-blur`.
    pub blur_strength: Option<u32>,
    pub popouts: PopoutOpacity,
    pub text_size: TextSize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            alpha: 1.0,
            mode: OpacityMode::Background,
            solid: false,
            blur: true,
            blur_strength: None,
            popouts: PopoutOpacity::Same,
            text_size: TextSize::Default,
        }
    }
}

fn parse<T: for<'de> Deserialize<'de>>(key: &str, value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|_| format!("invalid {key}"))
}

fn apply_setting(next: &mut Settings, key: &str, value: &Value) -> Result<(), String> {
    match key {
        "theme" => next.theme = parse(key, value)?,
        "mode" => next.mode = parse(key, value)?,
        "popouts" => next.popouts = parse(key, value)?,
        "textSize" => next.text_size = parse(key, value)?,
        "solid" => next.solid = parse(key, value)?,
        "blur" => next.blur = parse(key, value)?,
        "alpha" => {
            next.alpha = value
                .as_f64()
                .filter(|a| a.is_finite() && (MIN_ALPHA..=1.0).contains(a))
                .ok_or("invalid alpha")?;
        }
        "blurStrength" => {
            next.blur_strength = match value {
                Value::Null => None,
                other => Some(
                    other
                        .as_u64()
                        .and_then(|v| u32::try_from(v).ok())
                        .filter(|v| *v <= MAX_BLUR_STRENGTH)
                        .ok_or("invalid blurStrength")?,
                ),
            };
        }
        other => return Err(format!("unknown setting {other}")),
    }
    Ok(())
}

/// Applies a settings patch. Only known keys with valid values pass; one bad
/// field rejects the whole patch, so nothing arbitrary is ever stored.
pub fn apply_settings_patch(current: &Settings, patch: &Value) -> Result<Settings, String> {
    let fields = patch.as_object().ok_or("the patch must be an object")?;
    let mut next = current.clone();
    for (key, value) in fields {
        apply_setting(&mut next, key, value)?;
    }
    Ok(next)
}

/// Reads stored settings leniently: every valid known field is kept and the
/// rest fall back to the defaults, so a file from another version doesn't
/// reset everything.
pub fn settings_from_bytes(bytes: &[u8]) -> Settings {
    let mut settings = Settings::default();
    if let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(bytes) {
        for (key, value) in &fields {
            let mut candidate = settings.clone();
            if apply_setting(&mut candidate, key, value).is_ok() {
                settings = candidate;
            }
        }
    }
    settings
}

/// A window's place on screen, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    fn is_sane(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite() && v.abs() < 100_000.0)
            && self.width >= 1.0
            && self.height >= 1.0
    }
}

/// A thread shown in its own window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Popout {
    pub thread_id: String,
    pub bounds: Option<Bounds>,
}

/// What a view keeps besides the thread's history: what the user was typing
/// and where they were reading.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewState {
    pub draft: String,
    /// Pixels from the top of the loaded transcript; `None` follows the end.
    pub scroll: Option<f64>,
    /// Order of last change, for eviction.
    #[serde(default)]
    pub touched: u64,
}

impl ViewState {
    fn is_empty(&self) -> bool {
        self.draft.is_empty() && self.scroll.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellState {
    pub revision: u64,
    pub settings: Settings,
    /// By window label.
    pub popouts: BTreeMap<String, Popout>,
    /// By thread id, or [`NEW_THREAD`].
    pub views: BTreeMap<String, ViewState>,
    /// The thread the main window shows, if any.
    pub main_thread: Option<String>,
    #[serde(skip)]
    next_popout: u64,
    #[serde(skip)]
    clock: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    /// The view is shown by another window. Expected while a view moves
    /// between windows: the frontend drops the edit.
    #[error("not-owner: the view is shown by another window")]
    NotOwner,
    #[error("invalid: {0}")]
    Invalid(String),
}

impl ShellState {
    pub fn new(settings: Settings) -> Self {
        Self {
            revision: 0,
            settings,
            popouts: BTreeMap::new(),
            views: BTreeMap::new(),
            main_thread: None,
            next_popout: 1,
            clock: 0,
        }
    }

    /// The state a relaunch starts from. Pop-outs are not in it: they are
    /// reopened one by one once the main window is interactive.
    pub fn restore(settings: Settings, session: &Session) -> Self {
        let mut state = Self::new(settings);
        for (key, draft) in &session.drafts {
            if state.views.len() >= MAX_VIEWS || draft.is_empty() || draft.len() > MAX_DRAFT_BYTES {
                continue;
            }
            if state.drafts_bytes() + draft.len() > MAX_DRAFTS_BYTES {
                continue;
            }
            state.clock += 1;
            let view = ViewState {
                draft: draft.clone(),
                scroll: None,
                touched: state.clock,
            };
            state.views.insert(key.clone(), view);
        }
        state.main_thread = session.main_thread.clone();
        state
    }

    /// The label of the window that shows `key`.
    pub fn owner(&self, key: &str) -> &str {
        self.popouts
            .iter()
            .find(|(_, popout)| popout.thread_id == key)
            .map_or(MAIN, |(label, _)| label.as_str())
    }

    fn drafts_bytes(&self) -> usize {
        self.views.values().map(|view| view.draft.len()).sum()
    }

    fn bump(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    /// A view's own state, written by the window that shows it. Every field
    /// is checked before any is written. Returns the new revision.
    pub fn patch_view(
        &mut self,
        window: &str,
        key: &str,
        patch: &Value,
    ) -> Result<u64, PatchError> {
        if self.owner(key) != window {
            return Err(PatchError::NotOwner);
        }
        if key.is_empty() || key.len() > 256 {
            return Err(PatchError::Invalid("bad view key".into()));
        }
        let fields = patch
            .as_object()
            .ok_or_else(|| PatchError::Invalid("the patch must be an object".into()))?;
        let mut next = self.views.get(key).cloned().unwrap_or_default();
        for (field, value) in fields {
            let bad = || PatchError::Invalid(format!("bad field {field}"));
            match field.as_str() {
                "draft" => {
                    let draft = value.as_str().ok_or_else(bad)?;
                    if draft.len() > MAX_DRAFT_BYTES {
                        return Err(PatchError::Invalid("the draft is too long".into()));
                    }
                    next.draft = draft.into();
                }
                "scroll" => {
                    next.scroll = match value {
                        Value::Null => None,
                        other => Some(
                            other
                                .as_f64()
                                .filter(|v| v.is_finite() && *v >= 0.0)
                                .ok_or_else(bad)?,
                        ),
                    };
                }
                _ => return Err(bad()),
            }
        }
        let others = self.drafts_bytes() - self.views.get(key).map_or(0, |view| view.draft.len());
        if others + next.draft.len() > MAX_DRAFTS_BYTES {
            return Err(PatchError::Invalid("too much unsent text is kept".into()));
        }
        if next.is_empty() {
            self.views.remove(key);
        } else {
            self.clock += 1;
            next.touched = self.clock;
            self.views.insert(key.into(), next);
            self.evict();
        }
        Ok(self.bump())
    }

    fn evict(&mut self) {
        while self.views.len() > MAX_VIEWS {
            let victim = self
                .views
                .iter()
                .min_by_key(|(_, view)| (!view.draft.is_empty(), view.touched))
                .map(|(key, _)| key.clone());
            match victim {
                Some(key) => self.views.remove(&key),
                None => break,
            };
        }
    }

    /// The main window shows another thread (or none).
    pub fn set_main_thread(
        &mut self,
        window: &str,
        thread: Option<String>,
    ) -> Result<u64, PatchError> {
        if window != MAIN {
            return Err(PatchError::NotOwner);
        }
        if thread
            .as_deref()
            .is_some_and(|id| id.is_empty() || id.len() > 256)
        {
            return Err(PatchError::Invalid("bad thread id".into()));
        }
        self.main_thread = thread;
        Ok(self.bump())
    }

    /// Moves a thread's view from the main window to a new pop-out and
    /// returns the pop-out's label. The label comes from a counter, never
    /// from the thread id.
    pub fn begin_pop_out(
        &mut self,
        thread_id: &str,
        bounds: Option<Bounds>,
    ) -> Result<String, String> {
        if thread_id == NEW_THREAD || thread_id.is_empty() || thread_id.len() > 256 {
            return Err("this view can't be moved to its own window".into());
        }
        if self.owner(thread_id) != MAIN {
            return Err("the thread is already in its own window".into());
        }
        if self.popouts.len() >= MAX_POPOUTS {
            return Err(format!(
                "at most {MAX_POPOUTS} threads can have their own window"
            ));
        }
        let label = format!("view-{}", self.next_popout);
        self.next_popout += 1;
        let popout = Popout {
            thread_id: thread_id.into(),
            bounds: bounds.filter(Bounds::is_sane),
        };
        self.popouts.insert(label.clone(), popout);
        self.bump();
        Ok(label)
    }

    /// Gives a pop-out's view back to the main window. Returns whether
    /// anything moved.
    pub fn end_pop_out(&mut self, label: &str) -> bool {
        let moved = self.popouts.remove(label).is_some();
        if moved {
            self.bump();
        }
        moved
    }

    /// A pop-out moved or was resized. Not a revision: no window draws it.
    pub fn set_bounds(&mut self, label: &str, bounds: Bounds) -> bool {
        match self.popouts.get_mut(label) {
            Some(popout) if bounds.is_sane() && popout.bounds != Some(bounds) => {
                popout.bounds = Some(bounds);
                true
            }
            _ => false,
        }
    }

    /// What is written to disk.
    pub fn session(&self) -> Session {
        Session {
            drafts: self
                .views
                .iter()
                .filter(|(_, view)| !view.draft.is_empty())
                .map(|(key, view)| (key.clone(), view.draft.clone()))
                .collect(),
            popouts: self.popouts.values().cloned().collect(),
            main_thread: self.main_thread.clone(),
        }
    }
}

/// What a relaunch (after a quit, a forced close or a crash) brings back:
/// unsent drafts, which threads had their own window and where, and the
/// thread the main window showed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Session {
    pub drafts: BTreeMap<String, String>,
    pub popouts: Vec<Popout>,
    pub main_thread: Option<String>,
}

impl Session {
    /// A file that can't be read is an empty session, never an error: the
    /// app must start.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        serde_json::from_slice(bytes).unwrap_or_default()
    }
}

/// Writes `bytes` to `path` through a temporary file created for the user
/// only (0600, never following a link), then renames it into place, so a
/// crash mid-write never leaves a truncated file.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    match std::fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
}

/// A background writer: `save` never blocks the caller, a burst of saves is
/// written once after `debounce` of quiet, and `shutdown` writes the last one.
pub struct Persister<T: Send + 'static> {
    sender: Mutex<Option<mpsc::Sender<T>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl<T: Send + 'static> Persister<T> {
    pub fn spawn(debounce: Duration, mut write: impl FnMut(&T) + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel::<T>();
        let worker = std::thread::spawn(move || {
            while let Ok(mut latest) = receiver.recv() {
                loop {
                    match receiver.recv_timeout(debounce) {
                        Ok(next) => latest = next,
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            write(&latest);
                            return;
                        }
                    }
                }
                write(&latest);
            }
        });
        Self {
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(Some(worker)),
        }
    }

    pub fn save(&self, value: T) {
        let sender = self.sender.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(value);
        }
    }

    /// Writes what is pending and stops the worker. Safe to call twice.
    pub fn shutdown(&self) {
        drop(
            self.sender
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

/// A persister that writes JSON to `path`, privately.
pub fn json_persister<T: Serialize + Send + 'static>(path: PathBuf) -> Persister<T> {
    Persister::spawn(PERSIST_DEBOUNCE, move |value: &T| {
        let result = serde_json::to_vec(value)
            .map_err(io::Error::other)
            .and_then(|bytes| write_private(&path, &bytes));
        if let Err(error) = result {
            eprintln!("atrium: saving {} failed: {error}", path.display());
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;

    fn state() -> ShellState {
        ShellState::new(Settings::default())
    }

    #[test]
    fn settings_patches_are_checked_field_by_field() {
        let settings = Settings::default();
        let next = apply_settings_patch(
            &settings,
            &json!({"alpha": 0.6, "mode": "whole", "popouts": "opaque", "theme": "dark", "textSize": "large", "blurStrength": 48}),
        )
        .unwrap();
        assert_eq!(next.alpha, 0.6);
        assert_eq!(next.mode, OpacityMode::Whole);
        assert_eq!(next.popouts, PopoutOpacity::Opaque);
        assert_eq!(next.theme, Theme::Dark);
        assert_eq!(next.text_size, TextSize::Large);
        assert_eq!(next.blur_strength, Some(48));
        let cleared = apply_settings_patch(&next, &json!({"blurStrength": null})).unwrap();
        assert_eq!(cleared.blur_strength, None);
        for bad in [
            json!({"alpha": 0.2}),
            json!({"alpha": "0.5"}),
            json!({"alpha": f64::NAN}),
            json!({"mode": "css-opacity"}),
            json!({"theme": "glass-dark"}),
            json!({"blur": "yes"}),
            json!({"blurStrength": 49}),
            json!({"blurStrength": -1}),
            json!({"alpha": 0.5, "evil": "x"}),
            json!([1]),
        ] {
            assert!(apply_settings_patch(&settings, &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn stored_settings_keep_valid_fields_and_drop_the_rest() {
        let settings =
            settings_from_bytes(br#"{"theme":"light","alpha":7,"mode":"whole","unknown":1}"#);
        assert_eq!(settings.theme, Theme::Light);
        assert_eq!(
            settings.alpha, 1.0,
            "an invalid alpha falls back to the default"
        );
        assert_eq!(settings.mode, OpacityMode::Whole);
        assert_eq!(settings_from_bytes(b"not json"), Settings::default());
    }

    #[test]
    fn only_the_window_that_shows_a_view_can_patch_it() {
        let mut state = state();
        state
            .patch_view(MAIN, "t1", &json!({"draft": "hello"}))
            .unwrap();
        let label = state.begin_pop_out("t1", None).unwrap();
        let before = state.clone();
        assert_eq!(
            state.patch_view(MAIN, "t1", &json!({"draft": "x"})),
            Err(PatchError::NotOwner)
        );
        assert_eq!(state, before);
        state
            .patch_view(&label, "t1", &json!({"draft": "x"}))
            .unwrap();
        assert_eq!(state.views["t1"].draft, "x");
        assert_eq!(
            state.patch_view(&label, "t2", &json!({"draft": "y"})),
            Err(PatchError::NotOwner),
            "a pop-out shows one thread"
        );
        assert!(PatchError::NotOwner.to_string().starts_with("not-owner"));
    }

    #[test]
    fn an_invalid_field_rejects_the_whole_view_patch() {
        let mut state = state();
        state
            .patch_view(MAIN, "t1", &json!({"draft": "kept"}))
            .unwrap();
        let before = state.clone();
        for bad in [
            json!({"draft": "ok", "scroll": "far"}),
            json!({"owner": "view-1"}),
            json!({"scroll": -4.0}),
            json!({"draft": 1}),
            json!("x"),
        ] {
            assert!(
                matches!(
                    state.patch_view(MAIN, "t1", &bad),
                    Err(PatchError::Invalid(_))
                ),
                "{bad}"
            );
            assert_eq!(state, before);
        }
    }

    #[test]
    fn a_view_without_state_is_not_kept() {
        let mut state = state();
        state
            .patch_view(MAIN, "t1", &json!({"draft": "a", "scroll": 10.0}))
            .unwrap();
        state.patch_view(MAIN, "t1", &json!({"draft": ""})).unwrap();
        assert!(
            state.views.contains_key("t1"),
            "the scroll position is still state"
        );
        state
            .patch_view(MAIN, "t1", &json!({"scroll": null}))
            .unwrap();
        assert!(state.views.is_empty());
    }

    #[test]
    fn drafts_are_bounded_one_by_one_and_together() {
        let mut state = state();
        let too_long = "x".repeat(MAX_DRAFT_BYTES + 1);
        assert!(
            state
                .patch_view(MAIN, "t0", &json!({"draft": too_long}))
                .is_err()
        );
        let big = "x".repeat(MAX_DRAFT_BYTES);
        for index in 0..MAX_DRAFTS_BYTES / MAX_DRAFT_BYTES {
            state
                .patch_view(MAIN, &format!("t{index}"), &json!({"draft": big}))
                .unwrap();
        }
        assert!(
            state
                .patch_view(MAIN, "one-more", &json!({"draft": "x"}))
                .is_err()
        );
        state
            .patch_view(MAIN, "t0", &json!({"draft": "short"}))
            .unwrap();
        state
            .patch_view(MAIN, "one-more", &json!({"draft": "x"}))
            .unwrap();
    }

    #[test]
    fn eviction_drops_views_without_a_draft_first_then_the_oldest() {
        let mut state = state();
        state
            .patch_view(MAIN, "draft-old", &json!({"draft": "a"}))
            .unwrap();
        state
            .patch_view(MAIN, "scroll-only", &json!({"scroll": 1.0}))
            .unwrap();
        for index in 0..MAX_VIEWS - 2 {
            state
                .patch_view(MAIN, &format!("t{index}"), &json!({"draft": "b"}))
                .unwrap();
        }
        assert_eq!(state.views.len(), MAX_VIEWS);
        state
            .patch_view(MAIN, "new-1", &json!({"draft": "c"}))
            .unwrap();
        assert!(!state.views.contains_key("scroll-only"));
        assert!(state.views.contains_key("draft-old"));
        state
            .patch_view(MAIN, "new-2", &json!({"draft": "c"}))
            .unwrap();
        assert!(
            !state.views.contains_key("draft-old"),
            "then the oldest draft"
        );
        assert_eq!(state.views.len(), MAX_VIEWS);
    }

    #[test]
    fn a_pop_out_takes_the_view_and_gives_it_back_with_its_draft() {
        let mut state = state();
        state
            .patch_view(MAIN, "t1", &json!({"draft": "unfinished", "scroll": 425.0}))
            .unwrap();
        let first = state.begin_pop_out("t1", None).unwrap();
        let second = state.begin_pop_out("t2", None).unwrap();
        assert_ne!(first, second);
        assert_eq!(state.owner("t1"), first);
        assert!(
            state.begin_pop_out("t1", None).is_err(),
            "a thread lives in one window"
        );
        assert!(state.begin_pop_out(NEW_THREAD, None).is_err());
        assert!(state.end_pop_out(&first));
        assert_eq!(state.owner("t1"), MAIN);
        assert_eq!(state.views["t1"].draft, "unfinished");
        assert_eq!(state.views["t1"].scroll, Some(425.0));
        assert_eq!(state.owner("t2"), second, "other pop-outs keep their view");
        assert!(!state.end_pop_out(&first), "a second return does nothing");
    }

    #[test]
    fn labels_come_from_a_counter_and_never_from_the_thread_id() {
        let mut state = state();
        let hostile = "main\" onload=\"x";
        let label = state.begin_pop_out(hostile, None).unwrap();
        assert_eq!(label, "view-1");
        state.end_pop_out(&label);
        assert_eq!(
            state.begin_pop_out(hostile, None).unwrap(),
            "view-2",
            "labels are not reused"
        );
    }

    #[test]
    fn pop_outs_are_bounded() {
        let mut state = state();
        for index in 0..MAX_POPOUTS {
            state.begin_pop_out(&format!("t{index}"), None).unwrap();
        }
        assert!(state.begin_pop_out("one-more", None).is_err());
    }

    #[test]
    fn only_the_main_window_sets_its_thread() {
        let mut state = state();
        assert!(state.set_main_thread("view-1", Some("t1".into())).is_err());
        state.set_main_thread(MAIN, Some("t1".into())).unwrap();
        assert_eq!(state.main_thread.as_deref(), Some("t1"));
    }

    #[test]
    fn the_session_round_trips_drafts_pop_outs_and_the_main_thread() {
        let mut state = state();
        state
            .patch_view(MAIN, "t1", &json!({"draft": "one"}))
            .unwrap();
        state
            .patch_view(MAIN, NEW_THREAD, &json!({"draft": "new"}))
            .unwrap();
        state
            .patch_view(MAIN, "t3", &json!({"scroll": 3.0}))
            .unwrap();
        let label = state.begin_pop_out("t1", None).unwrap();
        let bounds = Bounds {
            x: 10.0,
            y: 20.0,
            width: 700.0,
            height: 600.0,
        };
        assert!(state.set_bounds(&label, bounds));
        assert!(
            !state.set_bounds(&label, bounds),
            "an unchanged place is not a change"
        );
        assert!(!state.set_bounds(
            &label,
            Bounds {
                width: f64::NAN,
                ..bounds
            }
        ));
        state.set_main_thread(MAIN, Some("t2".into())).unwrap();

        let bytes = serde_json::to_vec(&state.session()).unwrap();
        let session = Session::from_bytes(&bytes);
        assert_eq!(
            session.popouts,
            vec![Popout {
                thread_id: "t1".into(),
                bounds: Some(bounds)
            }]
        );
        let restored = ShellState::restore(Settings::default(), &session);
        assert_eq!(restored.views["t1"].draft, "one");
        assert_eq!(restored.views[NEW_THREAD].draft, "new");
        assert!(
            !restored.views.contains_key("t3"),
            "scroll positions are not kept across launches"
        );
        assert_eq!(restored.main_thread.as_deref(), Some("t2"));
        assert!(
            restored.popouts.is_empty(),
            "pop-outs are reopened by the app, one by one"
        );
        assert_eq!(Session::from_bytes(b"{broken"), Session::default());
    }

    #[test]
    fn the_persister_writes_a_burst_once_and_the_last_value_on_shutdown() {
        let writes = Arc::new(Mutex::new(Vec::<u32>::new()));
        let sink = writes.clone();
        let persister = Persister::spawn(Duration::from_millis(200), move |value: &u32| {
            sink.lock().unwrap().push(*value);
        });
        for value in 0..100 {
            persister.save(value);
        }
        persister.shutdown();
        assert_eq!(*writes.lock().unwrap(), vec![99]);
        persister.shutdown();
    }

    #[test]
    fn the_persister_writes_again_after_a_quiet_period() {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        let persister = Persister::spawn(Duration::from_millis(20), move |_: &u32| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        persister.save(1);
        std::thread::sleep(Duration::from_millis(150));
        persister.save(2);
        persister.shutdown();
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_draft_survives_a_forced_close_of_its_window() {
        // A forced close destroys the window without its webview saying
        // anything: the view goes back to main with what Rust already holds.
        let mut state = state();
        let label = state.begin_pop_out("t1", None).unwrap();
        state
            .patch_view(&label, "t1", &json!({"draft": "typed in the pop-out"}))
            .unwrap();
        assert!(state.end_pop_out(&label));
        assert_eq!(state.views["t1"].draft, "typed in the pop-out");
        state
            .patch_view(
                MAIN,
                "t1",
                &json!({"draft": "typed in the pop-out, and more"}),
            )
            .unwrap();
    }

    #[test]
    fn a_draft_survives_a_crash_once_the_quiet_period_has_passed() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("session.json");
        let mut state = state();
        state
            .patch_view(MAIN, "t1", &json!({"draft": "not sent yet"}))
            .unwrap();
        let persister = json_persister::<Session>(path.clone());
        persister.save(state.session());
        std::thread::sleep(PERSIST_DEBOUNCE * 3);
        // The crash: no shutdown, nothing flushed.
        std::mem::forget(persister);
        let session = Session::from_bytes(&std::fs::read(&path).unwrap());
        let restored = ShellState::restore(Settings::default(), &session);
        assert_eq!(restored.views["t1"].draft, "not sent yet");
    }

    #[test]
    fn files_are_written_for_the_user_only_and_replace_the_old_one() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("settings.json");
        write_private(&path, b"{\"a\":1}").unwrap();
        // A stale temporary file, or a link planted in its place, is removed.
        std::os::unix::fs::symlink(
            folder.path().join("elsewhere"),
            folder.path().join("settings.json.tmp"),
        )
        .unwrap();
        write_private(&path, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":2}");
        assert!(
            !folder.path().join("elsewhere").exists(),
            "the link was not followed"
        );
        assert!(!folder.path().join("settings.json.tmp").exists());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
