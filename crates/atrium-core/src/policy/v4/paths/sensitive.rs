//! Shared sensitive-path rules, name folding, unlisted equivalence detection,
//! and CR-12 / CR-17 credential store matching (contract §5, §11 item 13, §12).
//!
//! Incorporates:
//! - `dev` `deny.rs` credential/browser/Atrium hard-deny lists and probes;
//! - M1-29 CR-17's four additional credential stores that ask;
//! - M1-31's thread-local shared `Sensitive` cache keyed strictly by value on
//!   the folded strings it consumes;
//! - §5 / §11 item 13 unlisted-equivalence detection (NFC/NFD canonical
//!   decomposition, unlisted Unicode case folds, and APFS ignorable code
//!   points) that turns matches under unlisted equivalences into `Unread`
//!   without ever downgrading a listed-fold or identity hit.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::policy::limits;
use crate::policy::model::{
    AccessMode, ComputeBudget, Exhausted, FileIdentity, FileType, FoldRules, FsCaps, Hit, HitMatch,
    HitSource, PartId, PartOutcome, PartResult, ProtectedSet, ResolveError, Target, TargetOrigin,
    TargetResolution, UnreadReason,
};
use crate::policy::v4::paths::PathResolver;

/// Hard-deny credential files relative to home (folded lowercase; `deny.rs`).
pub(crate) const CRED_FILES: &[&str] = &[
    ".aws/credentials",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".docker/config.json",
    ".kube/config",
    ".config/gh/hosts.yml",
    ".claude/.credentials.json",
    ".claude.json",
    ".codex/auth.json",
];

/// Hard-deny credential trees relative to home (folded lowercase; `deny.rs`).
pub(crate) const CRED_TREES: &[&str] =
    &[".gnupg", ".config/gcloud", ".gemini", "library/keychains"];

/// Browser profile roots relative to home (folded lowercase; `deny.rs`).
pub(crate) const BROWSER_ROOTS: &[&str] = &[
    "library/application support/google/chrome",
    "library/application support/chromium",
    "library/application support/microsoft edge",
    "library/application support/bravesoftware",
    "library/application support/firefox",
    "library/application support/arc",
    "library/application support/vivaldi",
    "library/cookies",
    ".config/google-chrome",
    ".config/chromium",
    ".config/bravesoftware",
    ".config/microsoft-edge",
    ".config/vivaldi",
    ".mozilla",
];

/// Hard-deny credential filenames inside browser roots (folded lowercase; `deny.rs`).
pub(crate) const BROWSER_FILES: &[&str] = &[
    "login data",
    "login data for account",
    "cookies",
    "cookies.binarycookies",
    "cookies.sqlite",
    "logins.json",
    "key4.db",
    "key3.db",
    "web data",
    "local state",
];

/// Representative `~/.ssh` private-key probe names (`deny.rs`).
pub(crate) const SSH_PROBES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", "x"];

/// Representative browser file probe names (`deny.rs`).
pub(crate) const BROWSER_FILE_PROBES: &[&str] =
    &["cookies", "login data", "cookies.sqlite", "logins.json"];

/// Credential store files that ask (CR-12 + CR-17 from M1-29; folded lowercase)
/// paired with their store family label.
pub(crate) const ASK_FILES: &[(&str, &str)] = &[
    (".cargo/credentials.toml", "cargo-credentials"),
    (".cargo/credentials", "cargo-credentials"),
    (".pypirc", "pypirc"),
    (".pgpass", "pgpass"),
    (".vault-token", "vault-token"),
    (".azure/accesstokens.json", "azure-tokens"),
    (".config/git/credentials", "git-credentials"),
    (".env", "dotenv"),
    // CR-17 (M1-29, D-111)
    (".gem/credentials", "gem-credentials"),
    (
        ".terraform.d/credentials.tfrc.json",
        "terraform-credentials",
    ),
    (".config/op/config", "1password-config"),
    (".yarnrc.yml", "yarnrc"),
    // Shell history files
    (".bash_history", "shell-history"),
    (".zsh_history", "shell-history"),
    (".zhistory", "shell-history"),
    (".sh_history", "shell-history"),
    (".ksh_history", "shell-history"),
    (".history", "shell-history"),
    (".local/share/fish/fish_history", "shell-history"),
    (".config/fish/fish_history", "shell-history"),
];

/// Credential store directories that ask (CR-12; folded lowercase) paired with
/// their store family label.
pub(crate) const ASK_TREES: &[(&str, &str)] = &[
    (".aws/sso/cache", "aws-sso-cache"),
    (".zsh_sessions", "shell-history"),
    (".bash_sessions", "shell-history"),
];

/// Prefix-matched credential stores that ask (`(dir, filename_prefix, family)`).
pub(crate) const ASK_NAME_PREFIXES: &[(&str, &str, &str)] =
    &[(".azure", "msal_token_cache.", "azure-msal-cache")];

/// macOS Data volume firmlink prefix stripped before path comparison (`gate.rs`).
const FIRMLINK_LOWER: &str = "/system/volumes/data";

// ---------------------------------------------------------------------------
// Listed folding & unlisted equivalence folding (§5, §11 item 13)
// ---------------------------------------------------------------------------

/// Strips macOS `/System/Volumes/Data` firmlink prefix (case-insensitively on
/// ASCII) when followed by `/` or end of string.
pub(crate) fn strip_firmlink(bytes: &[u8]) -> &[u8] {
    let n = FIRMLINK_LOWER.len();
    if bytes.len() >= n && bytes[..n].eq_ignore_ascii_case(FIRMLINK_LOWER.as_bytes()) {
        if bytes.len() == n {
            return b"/";
        }
        if bytes[n] == b'/' {
            return &bytes[n..];
        }
    }
    bytes
}

/// Applies the listed rev 4 folds (`FoldRules`: Unicode `str::to_lowercase()`,
/// `ſ`→`s` if `fold_long_s`, `K`→`k` if `fold_kelvin`, NO Unicode normalization)
/// to a single path component, preserving invalid UTF-8 bytes verbatim.
pub(crate) fn fold_listed_component(bytes: &[u8], fold_long_s: bool, fold_kelvin: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                push_listed_str(&mut out, valid, fold_long_s, fold_kelvin);
                break;
            }
            Err(e) => {
                let valid_len = e.valid_up_to();
                if valid_len > 0 {
                    let valid = &rest[..valid_len];
                    if let Ok(s) = std::str::from_utf8(valid) {
                        push_listed_str(&mut out, s, fold_long_s, fold_kelvin);
                    }
                }
                let err_len = e.error_len().unwrap_or(rest.len() - valid_len);
                out.extend_from_slice(&rest[valid_len..valid_len + err_len]);
                rest = &rest[valid_len + err_len..];
            }
        }
    }
    out
}

fn push_listed_str(out: &mut Vec<u8>, s: &str, fold_long_s: bool, fold_kelvin: bool) {
    let mut buf = [0u8; 4];
    for ch in s.chars() {
        if ch == 'ſ' {
            if fold_long_s {
                out.push(b's');
            } else {
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            continue;
        }
        if ch == '\u{212A}' {
            if fold_kelvin {
                out.push(b'k');
            } else {
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            continue;
        }
        for lc in ch.to_lowercase() {
            out.extend_from_slice(lc.encode_utf8(&mut buf).as_bytes());
        }
    }
}

/// Folds a full path using [`FoldRules`], stripping `/System/Volumes/Data` and
/// trailing slashes (preserving `"/"`).
pub(crate) fn fold_path_listed(path: &[u8], folds: FoldRules) -> Vec<u8> {
    let p = strip_firmlink(path);
    let mut folded = fold_listed_component(p, folds.fold_long_s, folds.fold_kelvin);
    while folded.len() > 1 && folded.last() == Some(&b'/') {
        folded.pop();
    }
    if folded.is_empty() {
        folded.push(b'/');
    }
    folded
}

/// Extended equivalence fold for §5 / §11 item 13:
/// - NFD canonical decomposition (Latin-1/Extended-A/B/Additional + Hangul
///   algorithmic decomposition) with canonical combining-class ordering;
/// - Full Unicode case folding beyond `str::to_lowercase()` (`ß`/`ẞ`→`ss`,
///   `İ`→`i`, `ı`→`i`, `ſ`→`s`, `K`→`k`, ligatures `ﬀ`..`ﬆ`, `ς`→`σ`, `µ`→`μ`);
/// - Stripping APFS/HFS+ default-ignorable code points (soft hyphen, ZWSP,
///   ZWNJ, ZWJ, LRM/RLM, bidi controls, BOM/FEFF).
///
/// Invalid UTF-8 bytes are preserved verbatim.
pub(crate) fn fold_extended_component(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                push_extended_str(&mut out, valid);
                break;
            }
            Err(e) => {
                let valid_len = e.valid_up_to();
                if valid_len > 0
                    && let Ok(s) = std::str::from_utf8(&rest[..valid_len])
                {
                    push_extended_str(&mut out, s);
                }
                let err_len = e.error_len().unwrap_or(rest.len() - valid_len);
                out.extend_from_slice(&rest[valid_len..valid_len + err_len]);
                rest = &rest[valid_len + err_len..];
            }
        }
    }
    out
}

pub(crate) fn fold_path_extended(path: &[u8]) -> Vec<u8> {
    let p = strip_firmlink(path);
    let mut folded = fold_extended_component(p);
    while folded.len() > 1 && folded.last() == Some(&b'/') {
        folded.pop();
    }
    if folded.is_empty() {
        folded.push(b'/');
    }
    folded
}

fn push_extended_str(out: &mut Vec<u8>, s: &str) {
    let mut chars: Vec<char> = Vec::with_capacity(s.len());
    for ch in s.chars() {
        if is_apfs_ignorable(ch) {
            continue;
        }
        // Unlisted full Unicode case folds (before NFD so `İ` → `i`, not `i` + U+0307).
        match ch {
            '\u{00DF}' | '\u{1E9E}' => {
                chars.push('s');
                chars.push('s');
            }
            '\u{0130}' | '\u{0131}' => chars.push('i'),
            '\u{017F}' => chars.push('s'),
            '\u{212A}' => chars.push('k'),
            '\u{03C2}' => chars.push('\u{03C3}'),
            '\u{00B5}' => chars.push('\u{03BC}'),
            '\u{FB00}' => chars.extend(['f', 'f']),
            '\u{FB01}' => chars.extend(['f', 'i']),
            '\u{FB02}' => chars.extend(['f', 'l']),
            '\u{FB03}' => chars.extend(['f', 'f', 'i']),
            '\u{FB04}' => chars.extend(['f', 'f', 'l']),
            '\u{FB05}' | '\u{FB06}' => chars.extend(['s', 't']),
            other => {
                for lc in other.to_lowercase() {
                    decompose_nfd(lc, &mut chars);
                }
            }
        }
    }
    // Canonical ordering of combining mark runs (CCC > 0).
    let mut i = 0;
    while i < chars.len() {
        if combining_class(chars[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && combining_class(chars[i]) != 0 {
            i += 1;
        }
        chars[start..i].sort_by_key(|&c| combining_class(c));
    }
    let mut buf = [0u8; 4];
    for ch in chars {
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
}

fn is_apfs_ignorable(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

fn combining_class(ch: char) -> u8 {
    match ch {
        '\u{0300}'..='\u{0314}'
        | '\u{033D}'..='\u{0344}'
        | '\u{0346}'
        | '\u{034A}'..='\u{034C}'
        | '\u{0350}'..='\u{0352}'
        | '\u{0357}'
        | '\u{035B}' => 230, // Above
        '\u{0316}'..='\u{0319}'
        | '\u{031C}'..='\u{0320}'
        | '\u{0323}'..='\u{0326}'
        | '\u{0329}'..='\u{0333}'
        | '\u{0339}'..='\u{033C}'
        | '\u{0347}'..='\u{0349}'
        | '\u{034D}'..='\u{034E}'
        | '\u{0353}'..='\u{0356}'
        | '\u{0359}'..='\u{035A}' => 220, // Below
        '\u{0327}' | '\u{0328}' => 202, // Attached below (cedilla, ogonek)
        '\u{031B}' => 216,              // Attached above right (horn)
        '\u{0334}'..='\u{0338}' => 1,   // Overlay
        '\u{0345}' => 240,              // Iota subscript
        _ => 0,
    }
}

/// Canonical NFD decomposition for precomposed lowercase Latin, common Greek/
/// Cyrillic accented letters, and algorithmic Hangul syllables.
fn decompose_nfd(ch: char, out: &mut Vec<char>) {
    let cp = ch as u32;
    // Hangul algorithmic decomposition (Unicode Standard §3.12).
    if (0xAC00..=0xD7A3).contains(&cp) {
        let s_index = cp - 0xAC00;
        let l = 0x1100 + s_index / 588;
        let v = 0x1161 + (s_index % 588) / 28;
        let t = 0x11A7 + (s_index % 28);
        if let Some(lc) = char::from_u32(l) {
            out.push(lc);
        }
        if let Some(vc) = char::from_u32(v) {
            out.push(vc);
        }
        if t != 0x11A7
            && let Some(tc) = char::from_u32(t)
        {
            out.push(tc);
        }
        return;
    }

    // Lowercase Latin-1 Supplement & Latin Extended-A/B/Additional canonical decompositions.
    let decomp: Option<&[char]> = match ch {
        '\u{00E0}' => Some(&['a', '\u{0300}']), // à
        '\u{00E1}' => Some(&['a', '\u{0301}']), // á
        '\u{00E2}' => Some(&['a', '\u{0302}']), // â
        '\u{00E3}' => Some(&['a', '\u{0303}']), // ã
        '\u{00E4}' => Some(&['a', '\u{0308}']), // ä
        '\u{00E5}' => Some(&['a', '\u{030A}']), // å
        '\u{00E7}' => Some(&['c', '\u{0327}']), // ç
        '\u{00E8}' => Some(&['e', '\u{0300}']), // è
        '\u{00E9}' => Some(&['e', '\u{0301}']), // é
        '\u{00EA}' => Some(&['e', '\u{0302}']), // ê
        '\u{00EB}' => Some(&['e', '\u{0308}']), // ë
        '\u{00EC}' => Some(&['i', '\u{0300}']), // ì
        '\u{00ED}' => Some(&['i', '\u{0301}']), // í
        '\u{00EE}' => Some(&['i', '\u{0302}']), // î
        '\u{00EF}' => Some(&['i', '\u{0308}']), // ï
        '\u{00F1}' => Some(&['n', '\u{0303}']), // ñ
        '\u{00F2}' => Some(&['o', '\u{0300}']), // ò
        '\u{00F3}' => Some(&['o', '\u{0301}']), // ó
        '\u{00F4}' => Some(&['o', '\u{0302}']), // ô
        '\u{00F5}' => Some(&['o', '\u{0303}']), // õ
        '\u{00F6}' => Some(&['o', '\u{0308}']), // ö
        '\u{00F9}' => Some(&['u', '\u{0300}']), // ù
        '\u{00FA}' => Some(&['u', '\u{0301}']), // ú
        '\u{00FB}' => Some(&['u', '\u{0302}']), // û
        '\u{00FC}' => Some(&['u', '\u{0308}']), // ü
        '\u{00FD}' => Some(&['y', '\u{0301}']), // ý
        '\u{00FF}' => Some(&['y', '\u{0308}']), // ÿ
        // Latin Extended-A
        '\u{0101}' => Some(&['a', '\u{0304}']), // ā
        '\u{0103}' => Some(&['a', '\u{0306}']), // ă
        '\u{0105}' => Some(&['a', '\u{0328}']), // ą
        '\u{0107}' => Some(&['c', '\u{0301}']), // ć
        '\u{0109}' => Some(&['c', '\u{0302}']), // ĉ
        '\u{010B}' => Some(&['c', '\u{0307}']), // ċ
        '\u{010D}' => Some(&['c', '\u{030C}']), // č
        '\u{010F}' => Some(&['d', '\u{030C}']), // ď
        '\u{0113}' => Some(&['e', '\u{0304}']), // ē
        '\u{0115}' => Some(&['e', '\u{0306}']), // ĕ
        '\u{0117}' => Some(&['e', '\u{0307}']), // ė
        '\u{0119}' => Some(&['e', '\u{0328}']), // ę
        '\u{011B}' => Some(&['e', '\u{030C}']), // ě
        '\u{011D}' => Some(&['g', '\u{0302}']), // ĝ
        '\u{011F}' => Some(&['g', '\u{0306}']), // ğ
        '\u{0121}' => Some(&['g', '\u{0307}']), // ġ
        '\u{0123}' => Some(&['g', '\u{0327}']), // ģ
        '\u{0125}' => Some(&['h', '\u{0302}']), // ĥ
        '\u{0129}' => Some(&['i', '\u{0303}']), // ĩ
        '\u{012B}' => Some(&['i', '\u{0304}']), // ī
        '\u{012D}' => Some(&['i', '\u{0306}']), // ĭ
        '\u{012F}' => Some(&['i', '\u{0328}']), // į
        '\u{0135}' => Some(&['j', '\u{0302}']), // ĵ
        '\u{0137}' => Some(&['k', '\u{0327}']), // ķ
        '\u{013A}' => Some(&['l', '\u{0301}']), // ĺ
        '\u{013C}' => Some(&['l', '\u{0327}']), // ļ
        '\u{013E}' => Some(&['l', '\u{030C}']), // ľ
        '\u{0144}' => Some(&['n', '\u{0301}']), // ń
        '\u{0146}' => Some(&['n', '\u{0327}']), // ņ
        '\u{0148}' => Some(&['n', '\u{030C}']), // ň
        '\u{014D}' => Some(&['o', '\u{0304}']), // ō
        '\u{014F}' => Some(&['o', '\u{0306}']), // ŏ
        '\u{0151}' => Some(&['o', '\u{030B}']), // ő
        '\u{0155}' => Some(&['r', '\u{0301}']), // ŕ
        '\u{0157}' => Some(&['r', '\u{0327}']), // ŗ
        '\u{0159}' => Some(&['r', '\u{030C}']), // ř
        '\u{015B}' => Some(&['s', '\u{0301}']), // ś
        '\u{015D}' => Some(&['s', '\u{0302}']), // ŝ
        '\u{015F}' => Some(&['s', '\u{0327}']), // ş
        '\u{0161}' => Some(&['s', '\u{030C}']), // š
        '\u{0163}' => Some(&['t', '\u{0327}']), // ţ
        '\u{0165}' => Some(&['t', '\u{030C}']), // ť
        '\u{0169}' => Some(&['u', '\u{0303}']), // ũ
        '\u{016B}' => Some(&['u', '\u{0304}']), // ū
        '\u{016D}' => Some(&['u', '\u{0306}']), // ŭ
        '\u{016F}' => Some(&['u', '\u{030A}']), // ů
        '\u{0171}' => Some(&['u', '\u{030B}']), // ű
        '\u{0173}' => Some(&['u', '\u{0328}']), // ų
        '\u{0175}' => Some(&['w', '\u{0302}']), // ŵ
        '\u{0177}' => Some(&['y', '\u{0302}']), // ŷ
        '\u{017A}' => Some(&['z', '\u{0301}']), // ź
        '\u{017C}' => Some(&['z', '\u{0307}']), // ż
        '\u{017E}' => Some(&['z', '\u{030C}']), // ž
        // Common Latin Extended Additional (Vietnamese / European)
        '\u{1EBF}' => Some(&['e', '\u{0302}', '\u{0301}']), // ế
        '\u{1EC1}' => Some(&['e', '\u{0302}', '\u{0300}']), // ề
        '\u{1EDB}' => Some(&['o', '\u{031B}', '\u{0301}']), // ớ
        '\u{1EEB}' => Some(&['u', '\u{031B}', '\u{0300}']), // ừ
        // Cyrillic й / ё
        '\u{0439}' => Some(&['\u{0438}', '\u{0306}']),
        '\u{0451}' => Some(&['\u{0435}', '\u{0308}']),
        _ => None,
    };
    if let Some(seq) = decomp {
        out.extend_from_slice(seq);
    } else {
        out.push(ch);
    }
}

// ---------------------------------------------------------------------------
// Byte-slice path prefix matching
// ---------------------------------------------------------------------------

/// Returns `Some(rest)` when `path` is equal to `base` (`rest = b""`) or
/// strictly below `base` (`base + "/" + rest`).
pub(crate) fn under_bytes<'a>(path: &'a [u8], base: &[u8]) -> Option<&'a [u8]> {
    if path == base {
        return Some(b"");
    }
    if base == b"/" {
        return path.strip_prefix(b"/");
    }
    let rest = path.strip_prefix(base)?;
    rest.strip_prefix(b"/")
}

/// Returns `Some(rest)` when `path` is equal to `base` (`rest = ""`) or
/// strictly below `base`.
pub(crate) fn under_str<'a>(path: &'a str, base: &str) -> Option<&'a str> {
    if path == base {
        return Some("");
    }
    let b = if base == "/" { "" } else { base };
    path.strip_prefix(b).and_then(|r| r.strip_prefix('/'))
}

// ---------------------------------------------------------------------------
// Shared Sensitive (ported from M1-31 N11, keyed strictly by value)
// ---------------------------------------------------------------------------

/// Cached folded representation of [`ProtectedSet`]'s path lists, shared across
/// checks on the same thread while the folded key stays equal (M1-31 N11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SharedSensitive {
    key_home: Vec<u8>,
    key_data: Vec<Vec<u8>>,
    key_protected: Vec<Vec<u8>>,
    key_ssh_linked: Vec<Vec<u8>>,
    key_folds: FoldRules,
    // Extended-folded equivalents for §11 item 13 checks.
    ext_home: Vec<u8>,
    ext_data: Vec<Vec<u8>>,
    ext_protected: Vec<Vec<u8>>,
    ext_ssh_linked: Vec<Vec<u8>>,
}

impl SharedSensitive {
    /// Returns an `Rc<SharedSensitive>` for `protected`, reusing the
    /// thread-local cache when the folded home, data, protected, ssh-linked
    /// lists and `FoldRules` match by value (M1-31 review 1, finding 1).
    pub(crate) fn for_protected(protected: &ProtectedSet) -> Rc<Self> {
        thread_local! {
            static LAST: RefCell<Option<Rc<SharedSensitive>>> = const { RefCell::new(None) };
        }
        let folds = protected.folds;
        let key_home = fold_path_listed(&protected.home, folds);
        let key_data: Vec<Vec<u8>> = protected
            .atrium_data_paths
            .iter()
            .map(|p| fold_path_listed(p, folds))
            .collect();
        let key_protected: Vec<Vec<u8>> = protected
            .atrium_protected_paths
            .iter()
            .map(|p| fold_path_listed(p, folds))
            .collect();
        let key_ssh_linked: Vec<Vec<u8>> = protected
            .ssh_linked_dirs
            .iter()
            .map(|p| fold_path_listed(p, folds))
            .collect();

        LAST.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(cached) = slot.as_ref()
                && cached.key_folds == folds
                && cached.key_home == key_home
                && cached.key_data == key_data
                && cached.key_protected == key_protected
                && cached.key_ssh_linked == key_ssh_linked
            {
                return Rc::clone(cached);
            }
            let ext_home = fold_path_extended(&protected.home);
            let ext_data = protected
                .atrium_data_paths
                .iter()
                .map(|p| fold_path_extended(p))
                .collect();
            let ext_protected = protected
                .atrium_protected_paths
                .iter()
                .map(|p| fold_path_extended(p))
                .collect();
            let ext_ssh_linked = protected
                .ssh_linked_dirs
                .iter()
                .map(|p| fold_path_extended(p))
                .collect();
            let built = Rc::new(Self {
                key_home,
                key_data,
                key_protected,
                key_ssh_linked,
                key_folds: folds,
                ext_home,
                ext_data,
                ext_protected,
                ext_ssh_linked,
            });
            *slot = Some(Rc::clone(&built));
            built
        })
    }

    pub(crate) fn home_folded(&self) -> &[u8] {
        &self.key_home
    }

    pub(crate) fn data_folded(&self) -> &[Vec<u8>] {
        &self.key_data
    }

    pub(crate) fn ssh_linked_folded(&self) -> &[Vec<u8>] {
        &self.key_ssh_linked
    }

    /// Checks a folded path against hard-deny rules for `access`.
    pub(crate) fn hard_deny_hit_folded(
        &self,
        pl: &[u8],
        access: AccessMode,
    ) -> Option<&'static str> {
        hard_deny_match_parts(
            pl,
            &self.key_home,
            &self.key_data,
            &self.key_protected,
            &self.key_ssh_linked,
            access,
        )
    }

    /// Checks a folded path against CR-12 + CR-17 credential store rules.
    pub(crate) fn store_hit_folded(&self, pl: &[u8]) -> Option<&'static str> {
        store_match_parts(pl, &self.key_home)
    }

    /// Checks whether `canonical` matches a protected path (hard-deny or store)
    /// under extended equivalences (NFC/NFD, unlisted case folds, APFS
    /// ignorables) when it did NOT match under the listed folds (§11 item 13).
    pub(crate) fn matches_unlisted_equivalence(
        &self,
        canonical: &[u8],
        access: AccessMode,
    ) -> bool {
        let ext = fold_path_extended(canonical);
        hard_deny_match_parts(
            &ext,
            &self.ext_home,
            &self.ext_data,
            &self.ext_protected,
            &self.ext_ssh_linked,
            access,
        )
        .is_some()
            || store_match_parts(&ext, &self.ext_home).is_some()
    }

    /// Representative hard-deny probe paths (folded lowercase, absolute) for
    /// §2/§5 filter probe matching (same set as `dev`'s `Sensitive::probes`).
    pub(crate) fn hard_deny_probes(&self) -> Vec<(Vec<u8>, &'static str)> {
        let join = |base: &[u8], rel: &str| join_folded(base, rel.as_bytes());
        let mut out = Vec::new();
        for f in CRED_FILES {
            out.push((join(&self.key_home, f), "credential-read"));
        }
        for t in CRED_TREES {
            let tree = join(&self.key_home, t);
            out.push((join(&tree, "x"), "credential-read"));
            out.push((tree, "credential-read"));
        }
        let ssh = join(&self.key_home, ".ssh");
        for n in SSH_PROBES {
            out.push((join(&ssh, n), "credential-read"));
        }
        for b in BROWSER_ROOTS {
            let root = join(&self.key_home, b);
            for f in BROWSER_FILE_PROBES {
                out.push((join(&root, f), "credential-read"));
                out.push((join(&join(&root, "default"), f), "credential-read"));
            }
        }
        for d in &self.key_data {
            out.push((d.clone(), "atrium-self"));
            out.push((join(d, "x"), "atrium-self"));
        }
        out
    }

    /// Representative credential-store probe paths (folded lowercase, absolute)
    /// for §5 filter probe matching (CR-12 + CR-17).
    pub(crate) fn store_probes(&self) -> Vec<(Vec<u8>, &'static str)> {
        let join = |base: &[u8], rel: &str| join_folded(base, rel.as_bytes());
        let mut out = Vec::new();
        for &(f, family) in ASK_FILES {
            out.push((join(&self.key_home, f), family));
        }
        for &(t, family) in ASK_TREES {
            let tree = join(&self.key_home, t);
            out.push((tree.clone(), family));
            out.push((join(&tree, "x.json"), family));
            if t == ".zsh_sessions" {
                out.push((join(&tree, "x.session"), family));
                out.push((join(&tree, "x.history"), family));
                out.push((join(&tree, "x.historynew"), family));
            } else if t == ".bash_sessions" {
                out.push((join(&tree, "x.session"), family));
                out.push((join(&tree, "x.history"), family));
            }
        }
        for &(dir, pre, family) in ASK_NAME_PREFIXES {
            let base = join(&self.key_home, dir);
            out.push((join(&base, &format!("{pre}json")), family));
            out.push((join(&base, &format!("{pre}bin")), family));
        }
        out
    }
}

pub(crate) fn join_folded(base: &[u8], rel: &[u8]) -> Vec<u8> {
    if rel.is_empty() {
        return base.to_vec();
    }
    let mut v = Vec::with_capacity(base.len() + 1 + rel.len());
    if base != b"/" {
        v.extend_from_slice(base);
    }
    v.push(b'/');
    v.extend_from_slice(rel);
    v
}

fn hard_deny_match_parts(
    pl: &[u8],
    home: &[u8],
    data: &[Vec<u8>],
    protected: &[Vec<u8>],
    ssh_linked: &[Vec<u8>],
    access: AccessMode,
) -> Option<&'static str> {
    let writes = access != AccessMode::Read;
    for d in data {
        if under_bytes(pl, d).is_some() {
            return Some("atrium-self");
        }
    }
    if writes {
        for d in protected {
            if under_bytes(pl, d).is_some() {
                return Some("atrium-self");
            }
        }
    }
    for d in ssh_linked {
        if let Some(rest) = under_bytes(pl, d)
            && (!rest.is_empty() || writes)
        {
            return Some("credential-read");
        }
    }
    let rest_bytes = under_bytes(pl, home)?;
    let rest = std::str::from_utf8(rest_bytes).ok()?;
    if CRED_FILES.contains(&rest) || CRED_TREES.iter().any(|t| under_str(rest, t).is_some()) {
        return Some("credential-read");
    }
    if let Some(r) = under_str(rest, ".ssh") {
        if r.is_empty() {
            if writes {
                return Some("credential-read");
            }
        } else {
            let name = r.rsplit('/').next().unwrap_or(r);
            let public = name.ends_with(".pub") || name.starts_with("known_hosts");
            let readable_config = r == "config" || r == "authorized_keys";
            if !(public || (readable_config && !writes)) {
                return Some("credential-read");
            }
        }
    }
    for b in BROWSER_ROOTS {
        if let Some(r) = under_str(rest, b) {
            let name = r.rsplit('/').next().unwrap_or(r);
            if BROWSER_FILES.contains(&name) {
                return Some("credential-read");
            }
        }
    }
    None
}

fn store_match_parts(pl: &[u8], home: &[u8]) -> Option<&'static str> {
    let rest_bytes = under_bytes(pl, home)?;
    let rest = std::str::from_utf8(rest_bytes).ok()?;
    for &(f, family) in ASK_FILES {
        if rest == f {
            return Some(family);
        }
    }
    for &(t, family) in ASK_TREES {
        if under_str(rest, t).is_some() {
            return Some(family);
        }
    }
    for &(dir, pre, family) in ASK_NAME_PREFIXES {
        if let Some(r) = under_str(rest, dir)
            && !r.contains('/')
            && r.starts_with(pre)
        {
            return Some(family);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Target checking (§5)
// ---------------------------------------------------------------------------

pub(crate) fn origin_to_part(origin: &TargetOrigin) -> PartId {
    match origin {
        TargetOrigin::Operand { unit, argv_index } => PartId::Operand {
            unit: *unit,
            argv_index: *argv_index,
        },
        TargetOrigin::Redirect {
            unit,
            redirect_index,
        } => PartId::Redirect {
            unit: *unit,
            redirect_index: *redirect_index,
        },
        TargetOrigin::SubjectPath { path_index } => PartId::SubjectPath {
            path_index: *path_index,
        },
        TargetOrigin::ImplicitCwd { unit } => unit.map_or(PartId::WholeCall, PartId::Unit),
        TargetOrigin::ExpandedFrom { source, .. } => source.clone(),
    }
}

/// Checks one resolved (or unread) [`Target`] against `protected`: identities,
/// listed name folds, and unlisted volume equivalences (§5, §11 item 13).
pub(crate) fn check_target_impl(
    target: &Target,
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
) -> Result<PartResult, Exhausted> {
    let part = origin_to_part(&target.origin);

    let (canonical, identities) = match &target.resolution {
        TargetResolution::Unread(reason) => {
            budget.charge(1)?;
            return Ok(PartResult {
                part,
                outcome: PartOutcome::Unread(reason.clone()),
            });
        }
        TargetResolution::Resolved {
            canonical,
            identities,
            ..
        } => (canonical, identities),
    };

    // Over-approximation never produces a hit (§1, §5): variant-only glob
    // matches always stay Unread.
    if matches!(
        target.origin,
        TargetOrigin::ExpandedFrom {
            variant_only: true,
            ..
        }
    ) {
        budget.charge(canonical.len().max(1) as u64)?;
        return Ok(PartResult {
            part,
            outcome: PartOutcome::Unread(UnreadReason::VariantOnlyGlobMatch {
                pattern: canonical.clone(),
            }),
        });
    }

    budget.charge((canonical.len() + identities.len() + 1) as u64)?;

    // 1. Hard-deny identity check (path and every existing ancestor).
    for id in identities {
        if let Some((_, hard_id)) = protected
            .hard_deny_identities
            .iter()
            .find(|(pid, _)| pid == id)
        {
            return Ok(PartResult {
                part: part.clone(),
                outcome: PartOutcome::HardDenyHit(Hit {
                    hard_deny_id: Some(hard_id),
                    source: HitSource::Model,
                    matched: HitMatch::ProtectedPath {
                        path: canonical.clone(),
                    },
                    part,
                }),
            });
        }
    }

    // 2. Listed-fold hard-deny check (in addition to identity).
    let shared = SharedSensitive::for_protected(protected);
    let folded = fold_path_listed(canonical, protected.folds);
    if let Some(hard_id) = shared.hard_deny_hit_folded(&folded, target.access) {
        return Ok(PartResult {
            part: part.clone(),
            outcome: PartOutcome::HardDenyHit(Hit {
                hard_deny_id: Some(hard_id),
                source: HitSource::Model,
                matched: HitMatch::ProtectedPath {
                    path: canonical.clone(),
                },
                part,
            }),
        });
    }

    // 3. Store identity check (CR-12 + CR-17).
    for id in identities {
        if let Some((_, family)) = protected.store_identities.iter().find(|(pid, _)| pid == id) {
            return Ok(PartResult {
                part: part.clone(),
                outcome: PartOutcome::StoreHit(Hit {
                    hard_deny_id: None,
                    source: HitSource::Model,
                    matched: HitMatch::StoreFamily { family },
                    part,
                }),
            });
        }
    }

    // 4. Listed-fold store check (CR-12 + CR-17).
    if let Some(family) = shared.store_hit_folded(&folded) {
        return Ok(PartResult {
            part: part.clone(),
            outcome: PartOutcome::StoreHit(Hit {
                hard_deny_id: None,
                source: HitSource::Model,
                matched: HitMatch::StoreFamily { family },
                part,
            }),
        });
    }

    // 5. Unlisted equivalence check (§5, §11 item 13): never downgrades a hit.
    budget.charge(canonical.len().max(1) as u64)?;
    if shared.matches_unlisted_equivalence(canonical, target.access) {
        return Ok(PartResult {
            part,
            outcome: PartOutcome::Unread(UnreadReason::UnlistedNameEquivalence {
                path: canonical.clone(),
            }),
        });
    }

    Ok(PartResult {
        part,
        outcome: PartOutcome::Clear,
    })
}

// ---------------------------------------------------------------------------
// ProtectedSet builder helper (populates identities and ~/.ssh linked dirs)
// ---------------------------------------------------------------------------

/// Builds a [`ProtectedSet`] using `resolver` (charging `caps` and `budget`),
/// populating `(dev, ino)` identities for existing hard-deny paths, `~/.ssh`
/// members (including dangling links inside `~/.ssh`, M1-32 rework4 finding 2),
/// directories linked into `~/.ssh`, and CR-12 / CR-17 credential stores.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_protected_set(
    home: &[u8],
    cwd: &[u8],
    atrium_data_paths: Vec<Vec<u8>>,
    atrium_protected_paths: Vec<Vec<u8>>,
    folds: FoldRules,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<ProtectedSet, Exhausted> {
    let mut set = ProtectedSet {
        home: home.to_vec(),
        cwd: cwd.to_vec(),
        atrium_data_paths,
        atrium_protected_paths,
        hard_deny_identities: Vec::new(),
        store_identities: Vec::new(),
        ssh_linked_dirs: Vec::new(),
        folds,
    };

    let mut seen_hd: HashSet<FileIdentity> = HashSet::new();
    let mut seen_store: HashSet<FileIdentity> = HashSet::new();

    #[allow(clippy::too_many_arguments)]
    fn add_hd_path(
        path: &[u8],
        id_str: &'static str,
        resolver: &dyn PathResolver,
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
        seen_hd: &mut HashSet<FileIdentity>,
        set: &mut ProtectedSet,
    ) -> Result<(), Exhausted> {
        match resolver.stat(path, caps, budget) {
            Ok(meta) => {
                if let Some(id) = meta.identity
                    && seen_hd.insert(id)
                {
                    set.hard_deny_identities.push((id, id_str));
                }
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
            Err(ResolveError::NotFound | ResolveError::Io(_) | ResolveError::CapReached(_)) => {}
        }
        Ok(())
    }

    // Hard-deny files, trees, and Atrium data paths.
    for f in CRED_FILES {
        let p = join_folded(home, f.as_bytes());
        add_hd_path(
            &p,
            "credential-read",
            resolver,
            caps,
            budget,
            &mut seen_hd,
            &mut set,
        )?;
    }
    for t in CRED_TREES {
        let p = join_folded(home, t.as_bytes());
        add_hd_path(
            &p,
            "credential-read",
            resolver,
            caps,
            budget,
            &mut seen_hd,
            &mut set,
        )?;
    }
    let data_paths = set.atrium_data_paths.clone();
    for d in &data_paths {
        add_hd_path(
            d,
            "atrium-self",
            resolver,
            caps,
            budget,
            &mut seen_hd,
            &mut set,
        )?;
    }

    // Walk ~/.ssh (bounded, matching gate.rs ssh_members + M1-32 dangling members).
    let ssh_dir = join_folded(home, b".ssh");
    let mut stack: Vec<(Vec<u8>, u8, bool)> = vec![(ssh_dir, 0, false)];
    let mut visited_dirs: HashSet<FileIdentity> = HashSet::new();
    let mut entries_seen = 0usize;
    const SSH_MAX_ENTRIES: usize = 1024;
    const SSH_MAX_DEPTH: u8 = 8;

    while let Some((dir, depth, linked)) = stack.pop() {
        if depth >= SSH_MAX_DEPTH || entries_seen >= SSH_MAX_ENTRIES {
            continue;
        }
        let entries = match resolver.read_dir(&dir, caps, budget) {
            Ok(e) => e,
            Err(ResolveError::Exhausted(e)) => return Err(e),
            Err(ResolveError::NotFound | ResolveError::Io(_) | ResolveError::CapReached(_)) => {
                continue;
            }
        };
        for entry in entries {
            entries_seen += 1;
            if entries_seen > SSH_MAX_ENTRIES {
                break;
            }
            let name_lower =
                fold_listed_component(&entry.name, folds.fold_long_s, folds.fold_kelvin);
            let is_public_ssh = !linked
                && std::str::from_utf8(&name_lower).is_ok_and(|n| {
                    n.ends_with(".pub")
                        || n.starts_with("known_hosts")
                        || (depth == 0 && (n == "config" || n == "authorized_keys"))
                });
            let child = join_folded(&dir, &entry.name);
            match entry.file_type {
                FileType::Symlink => {
                    // Record dangling symlink's own identity too (M1-32 rework4 finding 2).
                    if !is_public_ssh {
                        match resolver.lstat(&child, caps, budget) {
                            Ok(md) => {
                                if let Some(id) = md.identity
                                    && seen_hd.insert(id)
                                {
                                    set.hard_deny_identities.push((id, "credential-read"));
                                }
                            }
                            Err(ResolveError::Exhausted(e)) => return Err(e),
                            Err(_) => {}
                        }
                    }
                    match resolver.stat(&child, caps, budget) {
                        Ok(md) if md.file_type == FileType::Dir => {
                            match resolver.canonicalize(&child, caps, budget) {
                                Ok(real) => {
                                    if !set.ssh_linked_dirs.contains(&real) {
                                        set.ssh_linked_dirs.push(real);
                                    }
                                }
                                Err(ResolveError::Exhausted(e)) => return Err(e),
                                Err(_) => {}
                            }
                            let visit = md.identity.is_none_or(|id| visited_dirs.insert(id));
                            if visit {
                                stack.push((child, depth + 1, true));
                            }
                        }
                        Ok(md) => {
                            if !is_public_ssh
                                && let Some(id) = md.identity
                                && seen_hd.insert(id)
                            {
                                set.hard_deny_identities.push((id, "credential-read"));
                            }
                        }
                        Err(ResolveError::Exhausted(e)) => return Err(e),
                        Err(_) => {}
                    }
                }
                FileType::Dir => match resolver.lstat(&child, caps, budget) {
                    Ok(md) => {
                        // Do not add subdir inodes to `hard_deny_identities`:
                        // `~/.ssh/sub/key.pub` must keep its `.pub` exception
                        // while private files inside `sub` get their own file
                        // identities below and `~/.ssh/sub` itself stays
                        // blocked by listed path name.
                        let visit = md.identity.is_none_or(|id| visited_dirs.insert(id));
                        if visit {
                            stack.push((child, depth + 1, linked));
                        }
                    }
                    Err(ResolveError::Exhausted(e)) => return Err(e),
                    Err(_) => {}
                },
                FileType::File | FileType::Other => {
                    if !is_public_ssh {
                        add_hd_path(
                            &child,
                            "credential-read",
                            resolver,
                            caps,
                            budget,
                            &mut seen_hd,
                            &mut set,
                        )?;
                    }
                }
            }
        }
    }

    // Store identities (CR-12 + CR-17).
    let mut add_store = |path: &[u8],
                         family: &'static str,
                         caps: &mut FsCaps,
                         budget: &mut ComputeBudget,
                         set: &mut ProtectedSet|
     -> Result<(), Exhausted> {
        match resolver.stat(path, caps, budget) {
            Ok(meta) => {
                if let Some(id) = meta.identity
                    && seen_store.insert(id)
                {
                    set.store_identities.push((id, family));
                }
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
            Err(ResolveError::NotFound | ResolveError::Io(_) | ResolveError::CapReached(_)) => {}
        }
        Ok(())
    };

    for &(f, family) in ASK_FILES {
        let p = join_folded(home, f.as_bytes());
        add_store(&p, family, caps, budget, &mut set)?;
    }
    for &(t, family) in ASK_TREES {
        let p = join_folded(home, t.as_bytes());
        add_store(&p, family, caps, budget, &mut set)?;
    }
    for &(dir, pre, family) in ASK_NAME_PREFIXES {
        let dir_path = join_folded(home, dir.as_bytes());
        match resolver.read_dir(&dir_path, caps, budget) {
            Ok(entries) => {
                for entry in entries.into_iter().take(limits::WALK_MAX_ENTRIES.min(256)) {
                    let nl =
                        fold_listed_component(&entry.name, folds.fold_long_s, folds.fold_kelvin);
                    if nl.starts_with(pre.as_bytes()) {
                        let p = join_folded(&dir_path, &entry.name);
                        add_store(&p, family, caps, budget, &mut set)?;
                    }
                }
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
            Err(_) => {}
        }
    }

    Ok(set)
}
