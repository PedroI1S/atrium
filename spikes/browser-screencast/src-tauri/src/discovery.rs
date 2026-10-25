//! Chromium discovery (PLAN §3.13).
//!
//! * macOS: a fixed list of `.app` bundles in `/Applications` and `~/Applications`.
//! * Linux: executables on `$PATH` plus flatpak and snap export dirs. Code-only in this
//!   spike (not run on Linux; see the report for the checklist).
//!
//! Both entry points take their search roots as arguments so tests can point them at
//! fake app directories. `discover()` wires in the real ones.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserInfo {
    /// Stable id used for the "default browser" setting, e.g. `helium`.
    pub id: String,
    pub name: String,
    /// The `.app` bundle on macOS; `None` on Linux.
    pub app_path: Option<PathBuf>,
    /// The executable to spawn.
    pub binary: PathBuf,
}

/// `(id, bundle directory name, executable name inside Contents/MacOS)`.
/// Order is the order of preference when the user has not chosen a default.
pub const MACOS_APPS: &[(&str, &str, &str)] = &[
    ("chrome", "Google Chrome.app", "Google Chrome"),
    ("chrome-canary", "Google Chrome Canary.app", "Google Chrome Canary"),
    ("brave", "Brave Browser.app", "Brave Browser"),
    ("edge", "Microsoft Edge.app", "Microsoft Edge"),
    ("chromium", "Chromium.app", "Chromium"),
    ("vivaldi", "Vivaldi.app", "Vivaldi"),
    ("arc", "Arc.app", "Arc"),
    ("helium", "Helium.app", "Helium"),
];

/// `(id, display name, executable names looked up on $PATH / in export dirs)`.
pub const LINUX_BINARIES: &[(&str, &str, &[&str])] = &[
    ("chrome", "Google Chrome", &["google-chrome", "google-chrome-stable", "com.google.Chrome"]),
    ("chromium", "Chromium", &["chromium", "chromium-browser", "org.chromium.Chromium"]),
    ("brave", "Brave", &["brave-browser", "brave", "com.brave.Browser"]),
    ("edge", "Microsoft Edge", &["microsoft-edge", "microsoft-edge-stable", "com.microsoft.Edge"]),
];

fn is_executable_file(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// Finds the executable inside a macOS bundle: the conventional name first, then the
/// only executable in `Contents/MacOS` (some forks rename it).
fn macos_binary(app: &Path, exe_name: &str) -> Option<PathBuf> {
    let dir = app.join("Contents/MacOS");
    let conventional = dir.join(exe_name);
    if is_executable_file(&conventional) {
        return Some(conventional);
    }
    let mut candidates: Vec<PathBuf> = fs::read_dir(&dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| is_executable_file(p))
        .collect();
    candidates.sort();
    if candidates.len() == 1 { candidates.pop() } else { None }
}

/// Scans `roots` (normally `/Applications` and `~/Applications`) for the known bundles.
/// A browser present in several roots is reported once, first root wins.
pub fn discover_macos(roots: &[PathBuf]) -> Vec<BrowserInfo> {
    let mut out = Vec::new();
    for (id, bundle, exe) in MACOS_APPS {
        for root in roots {
            let app = root.join(bundle);
            if !app.is_dir() {
                continue;
            }
            if let Some(binary) = macos_binary(&app, exe) {
                out.push(BrowserInfo {
                    id: (*id).to_string(),
                    name: bundle.trim_end_matches(".app").to_string(),
                    app_path: Some(app),
                    binary,
                });
                break;
            }
        }
    }
    out
}

/// Scans `path_dirs` (the entries of `$PATH`) and `extra_dirs` (flatpak/snap export dirs)
/// for the known executable names.
pub fn discover_linux(path_dirs: &[PathBuf], extra_dirs: &[PathBuf]) -> Vec<BrowserInfo> {
    let mut out = Vec::new();
    for (id, name, exes) in LINUX_BINARIES {
        'found: for dir in path_dirs.iter().chain(extra_dirs) {
            for exe in *exes {
                let p = dir.join(exe);
                if is_executable_file(&p) {
                    out.push(BrowserInfo {
                        id: (*id).to_string(),
                        name: (*name).to_string(),
                        app_path: None,
                        binary: p,
                    });
                    break 'found;
                }
            }
        }
    }
    out
}

pub fn macos_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots
}

pub fn linux_extra_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/var/lib/flatpak/exports/bin"), PathBuf::from("/snap/bin")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/flatpak/exports/bin"));
    }
    dirs
}

/// Everything installed, in preference order. `ATRIUM_BROWSER=<path to an executable>`
/// puts a custom binary first (used by tests and the harness).
pub fn discover() -> Vec<BrowserInfo> {
    let mut found = Vec::new();
    if let Some(custom) = std::env::var_os("ATRIUM_BROWSER").map(PathBuf::from)
        && is_executable_file(&custom)
    {
        found.push(BrowserInfo {
            id: "custom".into(),
            name: custom.file_name().map_or_else(|| "custom".into(), |n| n.to_string_lossy().into_owned()),
            app_path: None,
            binary: custom,
        });
    }
    if cfg!(target_os = "macos") {
        found.extend(discover_macos(&macos_roots()));
    } else {
        let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        found.extend(discover_linux(&path_dirs, &linux_extra_dirs()));
    }
    found
}

/// Picks the browser: the user's default id if installed, else the first discovered.
pub fn pick(found: &[BrowserInfo], default_id: Option<&str>) -> Option<BrowserInfo> {
    default_id
        .and_then(|id| found.iter().find(|b| b.id == id))
        .or_else(|| found.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_app(root: &Path, bundle: &str, exe: &str, executable: bool) -> PathBuf {
        let macos = root.join(bundle).join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        let bin = macos.join(exe);
        fs::write(&bin, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 })).unwrap();
        bin
    }

    #[test]
    fn finds_every_listed_macos_browser_in_order() {
        let sys = tempfile::tempdir().unwrap();
        for (_, bundle, exe) in MACOS_APPS {
            fake_app(sys.path(), bundle, exe, true);
        }
        let found = discover_macos(&[sys.path().to_path_buf()]);
        let ids: Vec<&str> = found.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["chrome", "chrome-canary", "brave", "edge", "chromium", "vivaldi", "arc", "helium"]);
        assert_eq!(found[7].name, "Helium");
        assert!(found[7].binary.ends_with("Helium.app/Contents/MacOS/Helium"));
    }

    #[test]
    fn searches_both_roots_and_first_root_wins() {
        let sys = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        fake_app(sys.path(), "Helium.app", "Helium", true);
        fake_app(user.path(), "Helium.app", "Helium", true);
        fake_app(user.path(), "Vivaldi.app", "Vivaldi", true);
        let found = discover_macos(&[sys.path().to_path_buf(), user.path().to_path_buf()]);
        assert_eq!(found.len(), 2);
        let helium = found.iter().find(|b| b.id == "helium").unwrap();
        assert!(helium.binary.starts_with(sys.path()));
        let vivaldi = found.iter().find(|b| b.id == "vivaldi").unwrap();
        assert!(vivaldi.binary.starts_with(user.path()));
    }

    #[test]
    fn ignores_non_executables_unknown_apps_and_empty_bundles() {
        let sys = tempfile::tempdir().unwrap();
        fake_app(sys.path(), "Arc.app", "Arc", false);
        fake_app(sys.path(), "Safari.app", "Safari", true);
        fs::create_dir_all(sys.path().join("Brave Browser.app/Contents/MacOS")).unwrap();
        assert!(discover_macos(&[sys.path().to_path_buf()]).is_empty());
    }

    #[test]
    fn falls_back_to_the_only_executable_when_the_name_differs() {
        let sys = tempfile::tempdir().unwrap();
        let bin = fake_app(sys.path(), "Chromium.app", "chromium-renamed", true);
        let found = discover_macos(&[sys.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].binary, bin);
        // Two executables and no conventional name: ambiguous, so skipped.
        fake_app(sys.path(), "Chromium.app", "other", true);
        assert!(discover_macos(&[sys.path().to_path_buf()]).is_empty());
    }

    #[test]
    fn missing_roots_are_fine() {
        assert!(discover_macos(&[PathBuf::from("/definitely/not/here")]).is_empty());
        assert!(discover_linux(&[PathBuf::from("/definitely/not/here")], &[]).is_empty());
    }

    #[test]
    fn linux_scan_uses_path_then_export_dirs() {
        let bin = tempfile::tempdir().unwrap();
        let flat = tempfile::tempdir().unwrap();
        for (dir, exe) in [(bin.path(), "chromium"), (flat.path(), "com.brave.Browser"), (flat.path(), "chromium")] {
            let p = dir.join(exe);
            fs::write(&p, "").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let found = discover_linux(&[bin.path().to_path_buf()], &[flat.path().to_path_buf()]);
        let ids: Vec<&str> = found.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["chromium", "brave"]);
        assert!(found[0].binary.starts_with(bin.path()), "PATH beats export dirs");
        assert!(found[1].binary.starts_with(flat.path()));
    }

    #[test]
    fn default_pick_prefers_the_setting_then_the_first() {
        let mk = |id: &str| BrowserInfo { id: id.into(), name: id.into(), app_path: None, binary: id.into() };
        let found = vec![mk("chrome"), mk("helium")];
        assert_eq!(pick(&found, Some("helium")).unwrap().id, "helium");
        assert_eq!(pick(&found, Some("brave")).unwrap().id, "chrome");
        assert_eq!(pick(&found, None).unwrap().id, "chrome");
        assert!(pick(&[], None).is_none());
    }
}
