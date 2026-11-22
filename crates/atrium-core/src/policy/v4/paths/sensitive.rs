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
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::policy::limits;
use crate::policy::model::{
    AccessMode, ComputeBudget, DirEntry, Exhausted, FileIdentity, FileType, FoldRules, FsCaps, Hit,
    HitMatch, HitSource, PartId, PartOutcome, PartResult, ProtectedSet, ResolveError, Target,
    TargetOrigin, TargetResolution, UnreadReason,
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
/// - NFD canonical decomposition (complete Latin-1, Latin Extended-A, key
///   Latin Extended-B / Additional / Greek / Cyrillic, and algorithmic Hangul)
///   with complete Combining Diacritical Marks (`U+0300..=U+036F`) CCC ordering;
/// - Full Unicode case folding beyond `str::to_lowercase()` (`ß`/`ẞ`→`ss`,
///   `İ`→`i`, `ı`→`i`, `ſ`→`s`, `K`→`k`, ligatures `ﬀ`..`ﬆ`, `ς`→`σ`, `µ`→`μ`);
/// - Stripping APFS/HFS+ default-ignorable code points;
/// - Returns `(folded_bytes, has_uncovered_non_ascii)` so any non-ASCII code
///   point outside the exact table can be treated conservatively as a possible
///   unlisted equivalence against non-ASCII protected components (§11 item 13).
///
/// Invalid UTF-8 bytes are preserved verbatim.
pub(crate) fn fold_extended_component_tracked(bytes: &[u8]) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(bytes.len());
    let mut uncovered = false;
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                if push_extended_str(&mut out, valid) {
                    uncovered = true;
                }
                break;
            }
            Err(e) => {
                let valid_len = e.valid_up_to();
                if valid_len > 0
                    && let Ok(s) = std::str::from_utf8(&rest[..valid_len])
                    && push_extended_str(&mut out, s)
                {
                    uncovered = true;
                }
                let err_len = e.error_len().unwrap_or(rest.len() - valid_len);
                out.extend_from_slice(&rest[valid_len..valid_len + err_len]);
                rest = &rest[valid_len + err_len..];
            }
        }
    }
    (out, uncovered)
}

pub(crate) fn fold_extended_component(bytes: &[u8]) -> Vec<u8> {
    fold_extended_component_tracked(bytes).0
}

#[allow(dead_code)]
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

/// Returns `true` iff `s` contains any non-ASCII scalar whose canonical
/// decomposition, combining class, or unlisted case fold is NOT guaranteed
/// completely covered by the built-in exact table.
fn push_extended_str(out: &mut Vec<u8>, s: &str) -> bool {
    let mut chars: Vec<char> = Vec::with_capacity(s.len());
    let mut uncovered = false;
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
            '\u{212B}' => {
                chars.push('a');
                chars.push('\u{030A}');
            }
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
                    if !decompose_nfd(lc, &mut chars) {
                        uncovered = true;
                    }
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
    uncovered
}

fn is_apfs_ignorable(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

/// Complete Unicode Canonical_Combining_Class (CCC) values for Combining
/// Diacritical Marks (`U+0300..=U+036F`).
fn combining_class(ch: char) -> u8 {
    match ch {
        '\u{0300}'..='\u{0314}'
        | '\u{033D}'..='\u{0344}'
        | '\u{0346}'
        | '\u{034A}'..='\u{034C}'
        | '\u{0350}'..='\u{0352}'
        | '\u{0357}'
        | '\u{035B}'
        | '\u{0363}'..='\u{036F}' => 230, // Above
        '\u{0315}' | '\u{031A}' | '\u{0358}' => 232, // Above Right
        '\u{0316}'..='\u{0319}'
        | '\u{031C}'..='\u{0320}'
        | '\u{0323}'..='\u{0326}'
        | '\u{0329}'..='\u{0333}'
        | '\u{0339}'..='\u{033C}'
        | '\u{0347}'..='\u{0349}'
        | '\u{034D}'..='\u{034E}'
        | '\u{0353}'..='\u{0356}'
        | '\u{0359}'..='\u{035A}' => 220, // Below
        '\u{0321}'..='\u{0322}' | '\u{0327}' | '\u{0328}' => 202, // Attached below
        '\u{031B}' => 216,                           // Attached above right (horn)
        '\u{0334}'..='\u{0338}' => 1,                // Overlay
        '\u{0345}' => 240,                           // Iota subscript
        '\u{035C}' | '\u{035F}' | '\u{0362}' => 233, // Double below
        '\u{035D}'..='\u{035E}' | '\u{0360}'..='\u{0361}' => 234, // Double above
        _ => 0,
    }
}

/// Canonical NFD decomposition for precomposed lowercase Latin, common Greek/
/// Cyrillic accented letters, and algorithmic Hangul syllables.
///
/// Returns `true` iff `ch` lies in a range whose canonical decomposition and
/// combining class are completely covered by this table; returns `false` when
/// `ch` is outside the exact table (so callers can apply §11 item 13's
/// conservative `Unread` rule without false negatives).
fn decompose_nfd(ch: char, out: &mut Vec<char>) -> bool {
    let cp = ch as u32;
    if cp <= 0x7F {
        out.push(ch);
        return true;
    }
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
        return true;
    }

    // Complete lowercase Latin-1 Supplement, Latin Extended-A, plus key Latin
    // Extended-B (including U+01FB `ǻ`), Latin Extended Additional, and Cyrillic.
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
        // Latin Extended-A (complete)
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
        // Latin Extended-B canonical decompositions (including U+01FB `ǻ`)
        '\u{01CE}' => Some(&['a', '\u{030C}']), // ǎ
        '\u{01D0}' => Some(&['i', '\u{030C}']), // ǐ
        '\u{01D2}' => Some(&['o', '\u{030C}']), // ǒ
        '\u{01D4}' => Some(&['u', '\u{030C}']), // ǔ
        '\u{01D6}' => Some(&['u', '\u{0308}', '\u{0304}']), // ǖ
        '\u{01D8}' => Some(&['u', '\u{0308}', '\u{0301}']), // ǘ
        '\u{01DA}' => Some(&['u', '\u{0308}', '\u{030C}']), // ǚ
        '\u{01DC}' => Some(&['u', '\u{0308}', '\u{0300}']), // ǜ
        '\u{01DF}' => Some(&['a', '\u{0308}', '\u{0304}']), // ǟ
        '\u{01E1}' => Some(&['a', '\u{0307}', '\u{0304}']), // ǡ
        '\u{01E3}' => Some(&['\u{00E6}', '\u{0304}']), // ǣ
        '\u{01E7}' => Some(&['g', '\u{030C}']), // ǧ
        '\u{01E9}' => Some(&['k', '\u{030C}']), // ǩ
        '\u{01EB}' => Some(&['o', '\u{0328}']), // ǫ
        '\u{01ED}' => Some(&['o', '\u{0328}', '\u{0304}']), // ǭ
        '\u{01EF}' => Some(&['\u{0292}', '\u{030C}']), // ǯ
        '\u{01F0}' => Some(&['j', '\u{030C}']), // ǰ
        '\u{01F5}' => Some(&['g', '\u{0301}']), // ǵ
        '\u{01F9}' => Some(&['n', '\u{0300}']), // ǹ
        '\u{01FB}' => Some(&['a', '\u{030A}', '\u{0301}']), // ǻ
        '\u{01FD}' => Some(&['\u{00E6}', '\u{0301}']), // ǽ
        '\u{01FF}' => Some(&['\u{00F8}', '\u{0301}']), // ǿ
        // Common Latin Extended Additional (Vietnamese / European)
        '\u{1E09}' => Some(&['c', '\u{0327}', '\u{0301}']), // ḉ
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
        return true;
    }
    out.push(ch);
    // Ranges whose canonical decompositions and combining classes are 100% covered:
    // Latin-1 Supplement, Latin Extended-A, Combining Diacritical Marks, Hangul Jamo.
    (0x0080..=0x017F).contains(&cp)
        || (0x0300..=0x036F).contains(&cp)
        || (0x1100..=0x11FF).contains(&cp)
}

/// Returns `true` iff path component `cand` could match `prot` under an
/// unlisted equivalence (NFD/NFC normalization, unlisted Unicode case fold,
/// APFS default-ignorable code points, or conservatively any uncovered
/// non-ASCII decomposition when both components contain non-ASCII bytes; §11 item 13).
fn component_may_match_unlisted(
    cand: &[u8],
    prot: &[u8],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge((cand.len() + prot.len() + 1) as u64)?;
    if cand.is_ascii() && prot.is_ascii() {
        return Ok(cand.eq_ignore_ascii_case(prot));
    }
    budget.charge_alloc(
        cand.len()
            .saturating_add(prot.len())
            .saturating_mul(2)
            .max(2),
    )?;
    let (cand_ext, cand_uncovered) = fold_extended_component_tracked(cand);
    let (prot_ext, prot_uncovered) = fold_extended_component_tracked(prot);
    if cand_ext == prot_ext {
        return Ok(true);
    }
    // Conservative §11 item 13 rule: when BOTH components contain non-ASCII
    // bytes and either contains a scalar outside our exact NFD/CCC table,
    // treat the pair as a potential unlisted equivalence (-> Unread, never a hit).
    if (cand_uncovered || prot_uncovered) && !cand.is_ascii() && !prot.is_ascii() {
        return Ok(true);
    }
    Ok(false)
}

fn split_path_components(path: &[u8]) -> Vec<&[u8]> {
    strip_firmlink(path)
        .split(|&b| b == b'/')
        .filter(|s| !s.is_empty())
        .collect()
}

fn comps_match_prefix_unlisted(
    cand: &[&[u8]],
    prot: &[&[u8]],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    if cand.len() < prot.len() {
        return Ok(false);
    }
    for (c, p) in cand.iter().zip(prot.iter()) {
        if !component_may_match_unlisted(c, p, budget)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn comps_match_exact_unlisted(
    cand: &[&[u8]],
    prot: &[&[u8]],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    if cand.len() != prot.len() {
        return Ok(false);
    }
    comps_match_prefix_unlisted(cand, prot, budget)
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
    raw_home: Vec<u8>,
    raw_data: Vec<Vec<u8>>,
    raw_protected: Vec<Vec<u8>>,
    raw_ssh_linked: Vec<Vec<u8>>,
    key_home: Vec<u8>,
    key_data: Vec<Vec<u8>>,
    key_protected: Vec<Vec<u8>>,
    key_ssh_linked: Vec<Vec<u8>>,
    key_folds: FoldRules,
}

impl SharedSensitive {
    /// Returns an `Rc<SharedSensitive>` for `protected`, reusing the
    /// thread-local cache when the folded home, data, protected, ssh-linked
    /// lists and `FoldRules` match by value (M1-31 review 1, finding 1).
    ///
    /// Precharges compute and heap allocations against `budget` before folding
    /// or building a cache entry (§8).
    pub(crate) fn for_protected_budgeted(
        protected: &ProtectedSet,
        budget: &mut ComputeBudget,
    ) -> Result<Rc<Self>, Exhausted> {
        thread_local! {
            static LAST: RefCell<Option<Rc<SharedSensitive>>> = const { RefCell::new(None) };
        }
        let folds = protected.folds;
        let input_bytes = protected.home.len()
            + protected
                .atrium_data_paths
                .iter()
                .map(Vec::len)
                .sum::<usize>()
            + protected
                .atrium_protected_paths
                .iter()
                .map(Vec::len)
                .sum::<usize>()
            + protected
                .ssh_linked_dirs
                .iter()
                .map(Vec::len)
                .sum::<usize>();
        budget.charge(input_bytes.max(1) as u64)?;

        // Zero-allocation fast path when raw inputs are identical to the cached entry.
        if let Some(hit) = LAST.with(|slot| {
            let slot = slot.borrow();
            if let Some(cached) = slot.as_ref()
                && cached.key_folds == folds
                && cached.raw_home == protected.home
                && cached.raw_data == protected.atrium_data_paths
                && cached.raw_protected == protected.atrium_protected_paths
                && cached.raw_ssh_linked == protected.ssh_linked_dirs
            {
                return Some(Rc::clone(cached));
            }
            None
        }) {
            return Ok(hit);
        }

        // Precharge folding work and allocations before folding keys.
        budget.charge(input_bytes.max(1) as u64)?;
        budget.charge_alloc(input_bytes.max(1))?;

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

        if let Some(hit) = LAST.with(|slot| {
            let slot = slot.borrow();
            if let Some(cached) = slot.as_ref()
                && cached.key_folds == folds
                && cached.key_home == key_home
                && cached.key_data == key_data
                && cached.key_protected == key_protected
                && cached.key_ssh_linked == key_ssh_linked
            {
                return Some(Rc::clone(cached));
            }
            None
        }) {
            return Ok(hit);
        }

        // Cache miss: precharge allocation for storing raw copies + `SharedSensitive`.
        budget.charge_alloc(std::mem::size_of::<Self>() + input_bytes)?;
        let built = Rc::new(Self {
            raw_home: protected.home.clone(),
            raw_data: protected.atrium_data_paths.clone(),
            raw_protected: protected.atrium_protected_paths.clone(),
            raw_ssh_linked: protected.ssh_linked_dirs.clone(),
            key_home,
            key_data,
            key_protected,
            key_ssh_linked,
            key_folds: folds,
        });
        LAST.with(|slot| {
            *slot.borrow_mut() = Some(Rc::clone(&built));
        });
        Ok(built)
    }

    /// Convenience wrapper for tests that inspects the thread-local cache
    /// using default limits.
    #[cfg(test)]
    pub(crate) fn for_protected(protected: &ProtectedSet) -> Rc<Self> {
        let mut budget = ComputeBudget::with_default_limits();
        Self::for_protected_budgeted(protected, &mut budget)
            .expect("SharedSensitive fits default budget")
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
    /// ignorables, or conservatively any uncovered non-ASCII decomposition)
    /// when it did NOT match under the listed folds (§11 item 13).
    pub(crate) fn matches_unlisted_equivalence(
        &self,
        canonical: &[u8],
        access: AccessMode,
        budget: &mut ComputeBudget,
    ) -> Result<bool, Exhausted> {
        let writes = access != AccessMode::Read;
        let cand_comps = split_path_components(canonical);
        budget.charge((canonical.len() + cand_comps.len() + 1) as u64)?;
        budget.charge_alloc(cand_comps.len() * std::mem::size_of::<&[u8]>())?;

        for d in &self.raw_data {
            let d_comps = split_path_components(d);
            if comps_match_prefix_unlisted(&cand_comps, &d_comps, budget)? {
                return Ok(true);
            }
        }
        if writes {
            for d in &self.raw_protected {
                let d_comps = split_path_components(d);
                if comps_match_prefix_unlisted(&cand_comps, &d_comps, budget)? {
                    return Ok(true);
                }
            }
        }
        for d in &self.raw_ssh_linked {
            let d_comps = split_path_components(d);
            if comps_match_prefix_unlisted(&cand_comps, &d_comps, budget)?
                && (cand_comps.len() > d_comps.len() || writes)
            {
                return Ok(true);
            }
        }

        let home_comps = split_path_components(&self.raw_home);
        if !comps_match_prefix_unlisted(&cand_comps, &home_comps, budget)? {
            return Ok(false);
        }
        let rest = &cand_comps[home_comps.len()..];
        if rest.is_empty() {
            return Ok(false);
        }

        for f in CRED_FILES {
            let prot_comps: Vec<&[u8]> = f.split('/').map(str::as_bytes).collect();
            if comps_match_exact_unlisted(rest, &prot_comps, budget)? {
                return Ok(true);
            }
        }
        for t in CRED_TREES {
            let prot_comps: Vec<&[u8]> = t.split('/').map(str::as_bytes).collect();
            if comps_match_prefix_unlisted(rest, &prot_comps, budget)? {
                return Ok(true);
            }
        }
        if component_may_match_unlisted(rest[0], b".ssh", budget)? {
            if rest.len() == 1 {
                if writes {
                    return Ok(true);
                }
            } else {
                let last = rest[rest.len() - 1];
                budget.charge_alloc(last.len().max(1))?;
                let last_ext = fold_extended_component(last);
                let is_public = last_ext.ends_with(b".pub") || last_ext.starts_with(b"known_hosts");
                let is_readable_cfg = rest.len() == 2
                    && (component_may_match_unlisted(rest[1], b"config", budget)?
                        || component_may_match_unlisted(rest[1], b"authorized_keys", budget)?);
                if !(is_public || (is_readable_cfg && !writes)) {
                    return Ok(true);
                }
            }
        }
        for b in BROWSER_ROOTS {
            let b_comps: Vec<&[u8]> = b.split('/').map(str::as_bytes).collect();
            if rest.len() > b_comps.len() && comps_match_prefix_unlisted(rest, &b_comps, budget)? {
                let last = rest[rest.len() - 1];
                for bf in BROWSER_FILES {
                    if component_may_match_unlisted(last, bf.as_bytes(), budget)? {
                        return Ok(true);
                    }
                }
            }
        }
        for &(f, _) in ASK_FILES {
            let prot_comps: Vec<&[u8]> = f.split('/').map(str::as_bytes).collect();
            if comps_match_exact_unlisted(rest, &prot_comps, budget)? {
                return Ok(true);
            }
        }
        for &(t, _) in ASK_TREES {
            let prot_comps: Vec<&[u8]> = t.split('/').map(str::as_bytes).collect();
            if comps_match_prefix_unlisted(rest, &prot_comps, budget)? {
                return Ok(true);
            }
        }
        for &(dir, pre, _) in ASK_NAME_PREFIXES {
            let dir_comps: Vec<&[u8]> = dir.split('/').map(str::as_bytes).collect();
            if rest.len() == dir_comps.len() + 1
                && comps_match_prefix_unlisted(rest, &dir_comps, budget)?
            {
                let last = rest[dir_comps.len()];
                budget.charge_alloc(last.len().max(1))?;
                let last_ext = fold_extended_component(last);
                if last_ext.starts_with(pre.as_bytes()) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
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
fn unread_reason_alloc_bytes(reason: &UnreadReason) -> usize {
    match reason {
        UnreadReason::ProcessDependentPath { path }
        | UnreadReason::UnresolvablePath { path }
        | UnreadReason::UnlistedNameEquivalence { path }
        | UnreadReason::FollowedLinkUnderFilter { path } => path.len(),
        UnreadReason::VariantOnlyGlobMatch { pattern } => pattern.len(),
        UnreadReason::ReservedWordOrConstruct { construct } => construct.len(),
        UnreadReason::UnsupportedWrapperForm { wrapper } => wrapper.len(),
        UnreadReason::UnsupportedNestedShellForm { shell } => shell.len(),
        UnreadReason::CodeRunnerInlineCode { tool }
        | UnreadReason::OutsideToolModel { tool }
        | UnreadReason::DerivedDestinations { tool } => tool.len(),
        UnreadReason::Program { program_words } => {
            program_words.iter().map(String::len).sum::<usize>()
        }
        UnreadReason::UnknownFlag { tool, flag }
        | UnreadReason::UnsupportedBaseDirChange { tool, flag }
        | UnreadReason::UnsupportedFlagEffect { tool, flag } => tool.len() + flag.len(),
        UnreadReason::GitWildcardPathspec { pathspec } => pathspec.len(),
        UnreadReason::UnsupportedRedirect { op } => op.len(),
        _ => 0,
    }
}

pub(crate) fn check_target_impl(
    target: &Target,
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
) -> Result<PartResult, Exhausted> {
    let part = origin_to_part(&target.origin);

    let (canonical, identities) = match &target.resolution {
        TargetResolution::Unread(reason) => {
            budget.charge(1)?;
            let alloc = unread_reason_alloc_bytes(reason);
            if alloc > 0 {
                budget.charge_alloc(alloc)?;
            }
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
        budget.charge_alloc(canonical.len())?;
        return Ok(PartResult {
            part,
            outcome: PartOutcome::Unread(UnreadReason::VariantOnlyGlobMatch {
                pattern: canonical.clone(),
            }),
        });
    }

    // Precharge identity scan, path folding, and output path allocation BEFORE
    // any work or heap allocation (§8).
    budget.charge((canonical.len() + identities.len() + 1) as u64)?;
    budget.charge_alloc(canonical.len().max(1))?;

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
    let shared = SharedSensitive::for_protected_budgeted(protected, budget)?;
    let folded = fold_path_listed(canonical, protected.folds);
    if folded.len() > canonical.len() {
        budget.charge_alloc(folded.len() - canonical.len())?;
    }
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
    if shared.matches_unlisted_equivalence(canonical, target.access, budget)? {
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
// ProtectedSet builder & write-only identity collector
// ---------------------------------------------------------------------------

type DirCache = HashMap<Vec<u8>, Option<Vec<DirEntry>>>;

fn read_dir_cached<'a>(
    dir: &[u8],
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    cache: &'a mut DirCache,
    unreads: Option<&mut Vec<UnreadReason>>,
) -> Result<Option<&'a [DirEntry]>, Exhausted> {
    if !cache.contains_key(dir) {
        budget.charge_alloc(dir.len())?;
        let val = match resolver.read_dir(dir, caps, budget) {
            Ok(entries) => Some(entries),
            Err(ResolveError::Exhausted(e)) => return Err(e),
            Err(
                ResolveError::NotFound
                | ResolveError::Io(crate::policy::model::FsIoError::NotADirectory),
            ) => None,
            Err(ResolveError::CapReached(c)) => {
                if let Some(u) = unreads {
                    let r = UnreadReason::from(c);
                    if !u.contains(&r) {
                        u.push(r);
                    }
                }
                None
            }
            Err(ResolveError::Io(_)) => {
                if let Some(u) = unreads {
                    let r = UnreadReason::UnresolvablePath { path: dir.to_vec() };
                    if !u.contains(&r) {
                        u.push(r);
                    }
                }
                None
            }
        };
        cache.insert(dir.to_vec(), val);
    }
    Ok(cache.get(dir).and_then(|opt| opt.as_deref()))
}

/// Discovers all existing listed-fold spellings of `rel_folded` under `base`
/// using cached `read_dir` calls (falling back to the exact lowercase path when
/// a parent directory cannot be listed).
#[allow(clippy::too_many_arguments)]
fn resolve_folded_under_base(
    base: &[u8],
    rel_folded: &str,
    folds: FoldRules,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    cache: &mut DirCache,
    mut unreads: Option<&mut Vec<UnreadReason>>,
) -> Result<Vec<Vec<u8>>, Exhausted> {
    let comps: Vec<&str> = rel_folded.split('/').filter(|s| !s.is_empty()).collect();
    if comps.is_empty() {
        budget.charge_alloc(base.len())?;
        return Ok(vec![base.to_vec()]);
    }
    budget.charge_alloc(base.len())?;
    let mut frontier: Vec<Vec<u8>> = vec![base.to_vec()];
    for comp in comps {
        let mut next: Vec<Vec<u8>> = Vec::new();
        for dir in &frontier {
            let entries_opt =
                read_dir_cached(dir, resolver, caps, budget, cache, unreads.as_deref_mut())?;
            if let Some(entries) = entries_opt {
                for entry in entries {
                    budget.charge(entry.name.len().max(1) as u64)?;
                    let folded =
                        fold_listed_component(&entry.name, folds.fold_long_s, folds.fold_kelvin);
                    if folded == comp.as_bytes() {
                        let cand_len = dir.len() + 1 + entry.name.len();
                        budget.charge_alloc(cand_len)?;
                        let cand = join_folded(dir, &entry.name);
                        if !next.contains(&cand) {
                            next.push(cand);
                        }
                    }
                }
            } else {
                // Fallback when `dir` is non-listable (e.g. chmod 0111) or cap-reached.
                let cand_len = dir.len() + 1 + comp.len();
                budget.charge_alloc(cand_len)?;
                let cand = join_folded(dir, comp.as_bytes());
                if !next.contains(&cand) {
                    next.push(cand);
                }
            }
        }
        if next.is_empty() {
            return Ok(Vec::new());
        }
        frontier = next;
    }
    Ok(frontier)
}

/// Discovers exact `path` plus any listed-fold sibling spellings in `path`'s
/// parent directory (used for Atrium data / protected paths).
#[allow(clippy::too_many_arguments)]
fn resolve_folded_abs_path(
    path: &[u8],
    folds: FoldRules,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    cache: &mut DirCache,
    unreads: Option<&mut Vec<UnreadReason>>,
) -> Result<Vec<Vec<u8>>, Exhausted> {
    let mut out = Vec::new();
    budget.charge_alloc(path.len())?;
    out.push(path.to_vec());
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    let trimmed = &path[..end];
    if let Some(pos) = trimmed.iter().rposition(|&b| b == b'/') {
        let parent = if pos == 0 {
            b"/".as_slice()
        } else {
            &trimmed[..pos]
        };
        let name = &trimmed[pos + 1..];
        if !name.is_empty() {
            let target_folded = fold_listed_component(name, folds.fold_long_s, folds.fold_kelvin);
            if let Some(entries) = read_dir_cached(parent, resolver, caps, budget, cache, unreads)?
            {
                for entry in entries {
                    budget.charge(entry.name.len().max(1) as u64)?;
                    let ef =
                        fold_listed_component(&entry.name, folds.fold_long_s, folds.fold_kelvin);
                    if ef == target_folded {
                        let cand_len = parent.len() + 1 + entry.name.len();
                        budget.charge_alloc(cand_len)?;
                        let cand = join_folded(parent, &entry.name);
                        if !out.contains(&cand) {
                            out.push(cand);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Records BOTH `lstat` (retaining dangling symlinks and symlink spellings under
/// their own name, §5) and `stat` (when `path` is a symlink pointing to an
/// existing target).
#[allow(clippy::too_many_arguments)]
fn record_lstat_and_stat(
    path: &[u8],
    tag: &'static str,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    seen: &mut HashSet<FileIdentity>,
    out: &mut Vec<(FileIdentity, &'static str)>,
    mut unreads: Option<&mut Vec<UnreadReason>>,
) -> Result<Option<FileType>, Exhausted> {
    let meta = match resolver.lstat(path, caps, budget) {
        Ok(m) => m,
        Err(ResolveError::Exhausted(e)) => return Err(e),
        Err(
            ResolveError::NotFound
            | ResolveError::Io(crate::policy::model::FsIoError::NotADirectory),
        ) => {
            return Ok(None);
        }
        Err(ResolveError::CapReached(c)) => {
            if let Some(u) = unreads.as_deref_mut() {
                let r = UnreadReason::from(c);
                if !u.contains(&r) {
                    u.push(r);
                }
            }
            return Ok(None);
        }
        Err(ResolveError::Io(_)) => {
            if let Some(u) = unreads.as_deref_mut() {
                let r = UnreadReason::UnresolvablePath {
                    path: path.to_vec(),
                };
                if !u.contains(&r) {
                    u.push(r);
                }
            }
            return Ok(None);
        }
    };
    if let Some(id) = meta.identity
        && seen.insert(id)
    {
        budget.charge_alloc(std::mem::size_of::<(FileIdentity, &str)>())?;
        out.push((id, tag));
    }
    if meta.file_type == FileType::Symlink {
        match resolver.stat(path, caps, budget) {
            Ok(target_meta) => {
                if let Some(tid) = target_meta.identity
                    && seen.insert(tid)
                {
                    budget.charge_alloc(std::mem::size_of::<(FileIdentity, &str)>())?;
                    out.push((tid, tag));
                }
                return Ok(Some(target_meta.file_type));
            }
            Err(ResolveError::Exhausted(e)) => return Err(e),
            Err(ResolveError::CapReached(c)) => {
                if let Some(u) = unreads {
                    let r = UnreadReason::from(c);
                    if !u.contains(&r) {
                        u.push(r);
                    }
                }
            }
            Err(ResolveError::NotFound | ResolveError::Io(_)) => {}
        }
    }
    Ok(Some(meta.file_type))
}

/// Bounded walk inside a protected tree (`atrium_data_paths` or
/// `atrium_protected_paths`) to record member file/symlink identities so hard
/// links to files inside the tree are caught by identity (§5).
#[allow(clippy::too_many_arguments)]
fn walk_protected_tree_members(
    root: &[u8],
    tag: &'static str,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
    seen: &mut HashSet<FileIdentity>,
    out: &mut Vec<(FileIdentity, &'static str)>,
    mut unreads: Option<&mut Vec<UnreadReason>>,
) -> Result<(), Exhausted> {
    const TREE_MAX_ENTRIES: usize = 1024;
    const TREE_MAX_DEPTH: u8 = 8;

    let mut stack: Vec<(Vec<u8>, u8)> = vec![(root.to_vec(), 0)];
    let mut visited_dirs: HashSet<FileIdentity> = HashSet::new();
    let mut entries_seen = 0usize;

    while let Some((dir, depth)) = stack.pop() {
        if depth >= TREE_MAX_DEPTH || entries_seen >= TREE_MAX_ENTRIES {
            continue;
        }
        let entries = match resolver.read_dir(&dir, caps, budget) {
            Ok(e) => e,
            Err(ResolveError::Exhausted(e)) => return Err(e),
            Err(
                ResolveError::NotFound
                | ResolveError::Io(crate::policy::model::FsIoError::NotADirectory),
            ) => {
                continue;
            }
            Err(ResolveError::CapReached(c)) => {
                if let Some(u) = unreads.as_deref_mut() {
                    let r = UnreadReason::from(c);
                    if !u.contains(&r) {
                        u.push(r);
                    }
                }
                return Ok(());
            }
            Err(ResolveError::Io(_)) => {
                if let Some(u) = unreads.as_deref_mut() {
                    let r = UnreadReason::UnresolvablePath { path: dir };
                    if !u.contains(&r) {
                        u.push(r);
                    }
                }
                continue;
            }
        };
        for entry in entries {
            if entry.name == b"." || entry.name == b".." {
                continue;
            }
            entries_seen += 1;
            if entries_seen > TREE_MAX_ENTRIES {
                break;
            }
            budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
            let child = join_folded(&dir, &entry.name);
            let ft = record_lstat_and_stat(
                &child,
                tag,
                resolver,
                caps,
                budget,
                seen,
                out,
                unreads.as_deref_mut(),
            )?;
            if ft == Some(FileType::Dir) && depth + 1 < TREE_MAX_DEPTH {
                match resolver.stat(&child, caps, budget) {
                    Ok(md) if md.identity.is_none_or(|id| visited_dirs.insert(id)) => {
                        stack.push((child, depth + 1));
                    }
                    Err(ResolveError::Exhausted(e)) => return Err(e),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

/// Builds a [`ProtectedSet`] using `resolver` (charging `caps` and `budget`),
/// populating `(dev, ino)` identities for existing hard-deny paths (across
/// listed-fold spellings and including dangling symlinks via `lstat`),
/// `~/.ssh` members (including dangling links and folded `.SSH` / `.ſsh`
/// spellings), directories linked into `~/.ssh`, browser credential files,
/// Atrium data paths, and CR-12 / CR-17 credential stores.
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
    budget.charge((home.len() + cwd.len() + 1) as u64)?;
    budget.charge_alloc(home.len() + cwd.len())?;
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
    let mut dir_cache: DirCache = HashMap::new();

    // 1. Hard-deny files & trees (across listed-fold spellings, retaining dangling links via lstat).
    for f in CRED_FILES {
        for p in
            resolve_folded_under_base(home, f, folds, resolver, caps, budget, &mut dir_cache, None)?
        {
            record_lstat_and_stat(
                &p,
                "credential-read",
                resolver,
                caps,
                budget,
                &mut seen_hd,
                &mut set.hard_deny_identities,
                None,
            )?;
        }
    }
    for t in CRED_TREES {
        for p in
            resolve_folded_under_base(home, t, folds, resolver, caps, budget, &mut dir_cache, None)?
        {
            record_lstat_and_stat(
                &p,
                "credential-read",
                resolver,
                caps,
                budget,
                &mut seen_hd,
                &mut set.hard_deny_identities,
                None,
            )?;
        }
    }

    // 2. Atrium data paths (exact + folded sibling spellings, lstat+stat, plus member files).
    let data_paths = set.atrium_data_paths.clone();
    for d in &data_paths {
        for cand in resolve_folded_abs_path(d, folds, resolver, caps, budget, &mut dir_cache, None)?
        {
            let ft = record_lstat_and_stat(
                &cand,
                "atrium-self",
                resolver,
                caps,
                budget,
                &mut seen_hd,
                &mut set.hard_deny_identities,
                None,
            )?;
            if ft == Some(FileType::Dir) {
                walk_protected_tree_members(
                    &cand,
                    "atrium-self",
                    resolver,
                    caps,
                    budget,
                    &mut seen_hd,
                    &mut set.hard_deny_identities,
                    None,
                )?;
            }
        }
    }

    // 3. Walk all listed-fold spellings of `~/.ssh` (e.g. `.ssh`, `.SSH`, `.ſsh`).
    let ssh_dirs = resolve_folded_under_base(
        home,
        ".ssh",
        folds,
        resolver,
        caps,
        budget,
        &mut dir_cache,
        None,
    )?;
    let mut stack: Vec<(Vec<u8>, u8, bool)> = ssh_dirs.into_iter().map(|d| (d, 0, false)).collect();
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
            if entry.name == b"." || entry.name == b".." {
                continue;
            }
            entries_seen += 1;
            if entries_seen > SSH_MAX_ENTRIES {
                break;
            }
            budget.charge(entry.name.len().max(1) as u64)?;
            let name_lower =
                fold_listed_component(&entry.name, folds.fold_long_s, folds.fold_kelvin);
            let is_public_ssh = !linked
                && std::str::from_utf8(&name_lower).is_ok_and(|n| {
                    n.ends_with(".pub")
                        || n.starts_with("known_hosts")
                        || (depth == 0 && (n == "config" || n == "authorized_keys"))
                });
            budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
            let child = join_folded(&dir, &entry.name);
            match entry.file_type {
                FileType::Symlink => {
                    // Record dangling symlink's own identity AND resolved target identity.
                    if !is_public_ssh {
                        match resolver.lstat(&child, caps, budget) {
                            Ok(md) => {
                                if let Some(id) = md.identity
                                    && seen_hd.insert(id)
                                {
                                    budget
                                        .charge_alloc(std::mem::size_of::<(FileIdentity, &str)>())?;
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
                                budget.charge_alloc(std::mem::size_of::<(FileIdentity, &str)>())?;
                                set.hard_deny_identities.push((id, "credential-read"));
                            }
                        }
                        Err(ResolveError::Exhausted(e)) => return Err(e),
                        Err(_) => {}
                    }
                }
                FileType::Dir => match resolver.lstat(&child, caps, budget) {
                    Ok(md) => {
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
                        record_lstat_and_stat(
                            &child,
                            "credential-read",
                            resolver,
                            caps,
                            budget,
                            &mut seen_hd,
                            &mut set.hard_deny_identities,
                            None,
                        )?;
                    }
                }
            }
        }
    }

    // 4. Bounded walk of existing browser roots for `BROWSER_FILES` identities
    //    (catches hard links and dangling/symlinked browser credential files
    //    without adding ordinary profile folders or `Preferences`).
    const BROWSER_ID_MAX_ENTRIES: usize = 1024;
    const BROWSER_ID_MAX_DEPTH: u8 = 8;
    let mut b_entries = 0usize;
    let mut b_visited: HashSet<FileIdentity> = HashSet::new();
    for b in BROWSER_ROOTS {
        if b_entries >= BROWSER_ID_MAX_ENTRIES {
            break;
        }
        let b_dirs = resolve_folded_under_base(
            home,
            b,
            folds,
            resolver,
            caps,
            budget,
            &mut dir_cache,
            None,
        )?;
        let mut b_stack: Vec<(Vec<u8>, u8)> = b_dirs.into_iter().map(|d| (d, 0)).collect();
        while let Some((dir, depth)) = b_stack.pop() {
            if depth >= BROWSER_ID_MAX_DEPTH || b_entries >= BROWSER_ID_MAX_ENTRIES {
                continue;
            }
            let entries = match resolver.read_dir(&dir, caps, budget) {
                Ok(e) => e,
                Err(ResolveError::Exhausted(e)) => return Err(e),
                Err(_) => continue,
            };
            for entry in entries {
                if entry.name == b"." || entry.name == b".." {
                    continue;
                }
                b_entries += 1;
                if b_entries > BROWSER_ID_MAX_ENTRIES {
                    break;
                }
                budget.charge(entry.name.len().max(1) as u64)?;
                let nl = fold_listed_component(&entry.name, folds.fold_long_s, folds.fold_kelvin);
                if entry.file_type == FileType::Dir {
                    if depth + 1 < BROWSER_ID_MAX_DEPTH {
                        budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
                        let child = join_folded(&dir, &entry.name);
                        match resolver.lstat(&child, caps, budget) {
                            Ok(md) if md.identity.is_none_or(|id| b_visited.insert(id)) => {
                                b_stack.push((child, depth + 1));
                            }
                            Err(ResolveError::Exhausted(e)) => return Err(e),
                            _ => {}
                        }
                    }
                } else if std::str::from_utf8(&nl).is_ok_and(|n| BROWSER_FILES.contains(&n)) {
                    budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
                    let child = join_folded(&dir, &entry.name);
                    record_lstat_and_stat(
                        &child,
                        "credential-read",
                        resolver,
                        caps,
                        budget,
                        &mut seen_hd,
                        &mut set.hard_deny_identities,
                        None,
                    )?;
                }
            }
        }
    }

    // 5. Store identities (CR-12 + CR-17, across listed-fold spellings, retaining dangling links).
    for &(f, family) in ASK_FILES {
        for p in
            resolve_folded_under_base(home, f, folds, resolver, caps, budget, &mut dir_cache, None)?
        {
            record_lstat_and_stat(
                &p,
                family,
                resolver,
                caps,
                budget,
                &mut seen_store,
                &mut set.store_identities,
                None,
            )?;
        }
    }
    for &(t, family) in ASK_TREES {
        for p in
            resolve_folded_under_base(home, t, folds, resolver, caps, budget, &mut dir_cache, None)?
        {
            record_lstat_and_stat(
                &p,
                family,
                resolver,
                caps,
                budget,
                &mut seen_store,
                &mut set.store_identities,
                None,
            )?;
        }
    }
    for &(dir, pre, family) in ASK_NAME_PREFIXES {
        for dir_path in resolve_folded_under_base(
            home,
            dir,
            folds,
            resolver,
            caps,
            budget,
            &mut dir_cache,
            None,
        )? {
            let entries = match resolver.read_dir(&dir_path, caps, budget) {
                Ok(e) => e,
                Err(ResolveError::Exhausted(e)) => return Err(e),
                Err(_) => continue,
            };
            for entry in entries.into_iter().take(limits::WALK_MAX_ENTRIES.min(256)) {
                budget.charge(entry.name.len().max(1) as u64)?;
                let nl = fold_listed_component(&entry.name, folds.fold_long_s, folds.fold_kelvin);
                if nl.starts_with(pre.as_bytes()) {
                    budget.charge_alloc(dir_path.len() + 1 + entry.name.len())?;
                    let p = join_folded(&dir_path, &entry.name);
                    record_lstat_and_stat(
                        &p,
                        family,
                        resolver,
                        caps,
                        budget,
                        &mut seen_store,
                        &mut set.store_identities,
                        None,
                    )?;
                }
            }
        }
    }

    Ok(set)
}

/// Collects `(dev, ino)` identities of write-only protected files/trees
/// (`~/.ssh/config`, `~/.ssh/authorized_keys`, and `protected.atrium_protected_paths`
/// plus their member files, across listed-fold spellings and including dangling
/// symlinks via `lstat`).
///
/// Used by resolver-backed flows (`resolve_command_targets`,
/// `resolve_subject_paths`, `check_recursive_root`) for targets with
/// `AccessMode::Write` or `AccessMode::ReadWrite` so hard-linked aliases to
/// write-only protected files produce `HardDenyHit` on write while staying
/// readable on `AccessMode::Read` (§5).
pub(crate) struct WriteOnlyIdentities {
    pub(crate) identities: Vec<(FileIdentity, &'static str)>,
    pub(crate) unreads: Vec<UnreadReason>,
}

pub(crate) fn collect_write_only_identities(
    protected: &ProtectedSet,
    resolver: &dyn PathResolver,
    caps: &mut FsCaps,
    budget: &mut ComputeBudget,
) -> Result<WriteOnlyIdentities, Exhausted> {
    let mut out: Vec<(FileIdentity, &'static str)> = Vec::new();
    let mut seen: HashSet<FileIdentity> = HashSet::new();
    let mut unreads: Vec<UnreadReason> = Vec::new();
    let mut dir_cache: DirCache = HashMap::new();
    let folds = protected.folds;

    // 1. `~/.ssh/config` and `~/.ssh/authorized_keys` (across listed-fold spellings).
    for rel in [".ssh/config", ".ssh/authorized_keys"] {
        for p in resolve_folded_under_base(
            &protected.home,
            rel,
            folds,
            resolver,
            caps,
            budget,
            &mut dir_cache,
            Some(&mut unreads),
        )? {
            record_lstat_and_stat(
                &p,
                "credential-read",
                resolver,
                caps,
                budget,
                &mut seen,
                &mut out,
                Some(&mut unreads),
            )?;
        }
    }

    // 2. `atrium_protected_paths` (exact + folded spellings + bounded member walk).
    for d in &protected.atrium_protected_paths {
        for cand in resolve_folded_abs_path(
            d,
            folds,
            resolver,
            caps,
            budget,
            &mut dir_cache,
            Some(&mut unreads),
        )? {
            let ft = record_lstat_and_stat(
                &cand,
                "atrium-self",
                resolver,
                caps,
                budget,
                &mut seen,
                &mut out,
                Some(&mut unreads),
            )?;
            if ft == Some(FileType::Dir) {
                walk_protected_tree_members(
                    &cand,
                    "atrium-self",
                    resolver,
                    caps,
                    budget,
                    &mut seen,
                    &mut out,
                    Some(&mut unreads),
                )?;
            }
        }
    }

    Ok(WriteOnlyIdentities {
        identities: out,
        unreads,
    })
}
