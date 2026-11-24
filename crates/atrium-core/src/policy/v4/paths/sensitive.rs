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
    AccessMode, ComputeBudget, Exhausted, FileIdentity, FileType, FoldRules, FsCap, FsCaps,
    FsIoError, GapScope, Hit, HitMatch, HitSource, IdentityGap, PartId, PartOutcome, PartResult,
    PathMetadata, ProtectedSet, ResolveError, Target, TargetOrigin, TargetResolution, UnreadReason,
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

/// Upper bound on the heap bytes one listed fold of `len` input bytes
/// allocates: the output buffer (reserved at 3/2 of the input, which bounds
/// `to_lowercase`'s growth per scalar) plus `str::to_lowercase`'s temporary
/// `String` for each valid UTF-8 segment (at most three times the input after
/// its amortized growth). Charged before the fold runs (§8).
pub(crate) const fn fold_alloc_bound(len: usize) -> usize {
    len.saturating_mul(5).saturating_add(8)
}

/// Applies the listed rev 4 folds (`FoldRules`, CR-19 and `dev`'s
/// `deny.rs`/`engine.rs` path fold `to_lowercase().replace('ſ', "s")`) to
/// `bytes`, after charging the work and every allocation it makes (§8):
/// - string-level `str::to_lowercase()` on each valid UTF-8 segment, so its
///   contextual rules apply (final sigma: `ΟΣ` folds to `ος`, as on `dev`);
/// - then U+017F (long s) to `s` when `fold_long_s`, and U+212A (Kelvin) to
///   `k` when `fold_kelvin` (already implied by `to_lowercase`);
/// - no Unicode normalization;
/// - invalid UTF-8 bytes are kept verbatim; they end a segment, as `dev`'s
///   lossy U+FFFD (neither cased nor case-ignorable) does.
pub(crate) fn fold_listed_component(
    bytes: &[u8],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Vec<u8>, Exhausted> {
    budget.charge((bytes.len() as u64).saturating_mul(2).max(1))?;
    budget.charge_alloc(fold_alloc_bound(bytes.len()))?;
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 2 + 4);
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                push_listed_str(&mut out, valid, folds);
                break;
            }
            Err(e) => {
                let valid_len = e.valid_up_to();
                if valid_len > 0
                    && let Ok(s) = std::str::from_utf8(&rest[..valid_len])
                {
                    push_listed_str(&mut out, s, folds);
                }
                let err_len = e.error_len().unwrap_or(rest.len() - valid_len);
                out.extend_from_slice(&rest[valid_len..valid_len + err_len]);
                rest = &rest[valid_len + err_len..];
            }
        }
    }
    Ok(out)
}

fn push_listed_str(out: &mut Vec<u8>, s: &str, folds: FoldRules) {
    if s.is_ascii() {
        out.extend(s.bytes().map(|b| b.to_ascii_lowercase()));
        return;
    }
    let lower = s.to_lowercase();
    let mut buf = [0u8; 4];
    for ch in lower.chars() {
        match ch {
            '\u{017F}' if folds.fold_long_s => out.push(b's'),
            '\u{212A}' if folds.fold_kelvin => out.push(b'k'),
            other => out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes()),
        }
    }
}

/// Folds a full path using [`FoldRules`] (see [`fold_listed_component`]),
/// stripping `/System/Volumes/Data` and trailing slashes (preserving `"/"`),
/// after charging the work and allocations (§8).
pub(crate) fn fold_path_listed(
    path: &[u8],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Vec<u8>, Exhausted> {
    let p = strip_firmlink(path);
    let mut folded = fold_listed_component(p, folds, budget)?;
    while folded.len() > 1 && folded.last() == Some(&b'/') {
        folded.pop();
    }
    if folded.is_empty() {
        folded.push(b'/');
    }
    Ok(folded)
}

/// Upper bound on the heap bytes one extended fold of `len` input bytes
/// allocates: the scalar buffer (reserved for two scalars per input byte, more
/// than the 3/2 that canonical decomposition and the unlisted case folds can
/// produce) and the output buffer (reserved at three times the input, the
/// largest UTF-8 growth of a decomposition: a 3-byte Hangul syllable becomes
/// three 3-byte jamo). Both are reserved up front, so neither grows (§8).
pub(crate) const fn fold_extended_alloc_bound(len: usize) -> usize {
    let chars = len.saturating_mul(2).saturating_add(4);
    let out = len.saturating_mul(3).saturating_add(4);
    chars
        .saturating_mul(std::mem::size_of::<char>())
        .saturating_add(out)
}

/// Extended equivalence fold for §5 / §11 item 13, after charging its work and
/// its allocations (§8):
/// - canonical decomposition (NFD) from an exact table: complete Latin-1,
///   Latin Extended-A and Combining Diacritical Marks (including the four
///   marks with decompositions, U+0340, U+0341, U+0343 and U+0344), the listed
///   Latin Extended-B / Additional / Cyrillic letters, the singletons that
///   decompose into ASCII or Latin-1 (U+037E, U+0387, U+1FEF, U+1FFD), and
///   algorithmic Hangul, with canonical ordering of U+0300..=U+036F marks;
/// - unlisted Unicode case folds beyond `to_lowercase` (`ß`/`ẞ` to `ss`,
///   `İ`/`ı` to `i`, ligatures U+FB00..=U+FB06, final sigma to sigma, micro
///   sign to mu), plus the listed ones;
/// - removal of every default-ignorable code point.
///
/// Returns `(folded_bytes, uncovered)`. `uncovered` is `true` when any scalar
/// is not fully decomposed by the table, so the conservative §11 item 13
/// fallback applies to it. Invalid UTF-8 bytes are kept verbatim.
pub(crate) fn fold_extended_component_tracked(
    bytes: &[u8],
    budget: &mut ComputeBudget,
) -> Result<(Vec<u8>, bool), Exhausted> {
    budget.charge((bytes.len() as u64).saturating_mul(4).max(1))?;
    budget.charge_alloc(fold_extended_alloc_bound(bytes.len()))?;
    let mut chars: Vec<char> = Vec::with_capacity(bytes.len().saturating_mul(2) + 4);
    let mut out = Vec::with_capacity(bytes.len().saturating_mul(3) + 4);
    let mut uncovered = false;
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                uncovered |= push_extended_str(&mut out, &mut chars, valid);
                break;
            }
            Err(e) => {
                let valid_len = e.valid_up_to();
                if valid_len > 0
                    && let Ok(s) = std::str::from_utf8(&rest[..valid_len])
                {
                    uncovered |= push_extended_str(&mut out, &mut chars, s);
                }
                let err_len = e.error_len().unwrap_or(rest.len() - valid_len);
                out.extend_from_slice(&rest[valid_len..valid_len + err_len]);
                rest = &rest[valid_len + err_len..];
            }
        }
    }
    Ok((out, uncovered))
}

/// Folds `s` into `out` (reusing `chars` as scratch, which the caller reserved).
/// Returns `true` iff some scalar of `s` is not fully decomposed by the exact
/// table (§11 item 13).
fn push_extended_str(out: &mut Vec<u8>, chars: &mut Vec<char>, s: &str) -> bool {
    chars.clear();
    let mut uncovered = false;
    for ch in s.chars() {
        if is_default_ignorable(ch) {
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
                    if !decompose_nfd(lc, chars) {
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
    for &ch in chars.iter() {
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
    uncovered
}

/// Unicode `Default_Ignorable_Code_Point` (DerivedCoreProperties), the code
/// points a volume may ignore when comparing names.
fn is_default_ignorable(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
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
            | '\u{1BCA0}'..='\u{1BCA3}'
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
        // The only Combining Diacritical Marks with canonical decompositions.
        '\u{0340}' => Some(&['\u{0300}']),
        '\u{0341}' => Some(&['\u{0301}']),
        '\u{0343}' => Some(&['\u{0313}']),
        '\u{0344}' => Some(&['\u{0308}', '\u{0301}']),
        // Singletons that decompose into ASCII or Latin-1.
        '\u{037E}' => Some(&[';']),
        '\u{0387}' => Some(&['\u{00B7}']),
        '\u{1FEF}' => Some(&['`']),
        '\u{1FFD}' => Some(&['\u{00B4}']),
        _ => None,
    };
    if let Some(seq) = decomp {
        out.extend_from_slice(seq);
        return true;
    }
    out.push(ch);
    // Blocks in which every scalar without a table entry has no canonical
    // decomposition, and whose combining classes `combining_class` knows:
    // Latin-1 Supplement and Latin Extended-A (`to_lowercase` runs first, and
    // the table lists every lowercase letter of both blocks that decomposes),
    // Combining Diacritical Marks (U+0340, U+0341, U+0343 and U+0344 are in
    // the table), and Hangul Jamo. Every other scalar is uncovered, so the
    // conservative §11 item 13 fallback applies to it.
    (0x0080..=0x017F).contains(&cp)
        || (0x0300..=0x036F).contains(&cp)
        || (0x1100..=0x11FF).contains(&cp)
}

/// Returns `true` iff path component `cand` could match `prot` under an
/// unlisted equivalence (NFD/NFC normalization, an unlisted Unicode case fold,
/// default-ignorable code points), or conservatively whenever either side has
/// a scalar the exact table does not fully decompose (§11 item 13).
///
/// The conservative rule is skipped only when the other side folds to pure
/// ASCII: every canonical decomposition and case fold that reaches pure ASCII
/// (Kelvin sign, long s, `ß`/`ẞ`, `İ`/`ı`, the `ﬀ`..`ﬆ` ligatures, U+037E,
/// U+1FEF) is in the exact table, so an uncovered scalar can never become
/// ASCII under any of those equivalences.
fn component_may_match_unlisted(
    cand: &[u8],
    prot: &[u8],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge((cand.len() + prot.len() + 1) as u64)?;
    if cand.is_ascii() && prot.is_ascii() {
        return Ok(cand.eq_ignore_ascii_case(prot));
    }
    let (cand_ext, cand_uncovered) = fold_extended_component_tracked(cand, budget)?;
    let (prot_ext, prot_uncovered) = fold_extended_component_tracked(prot, budget)?;
    budget.charge((cand_ext.len() + prot_ext.len() + 1) as u64)?;
    if cand_ext == prot_ext {
        return Ok(true);
    }
    Ok((cand_uncovered || prot_uncovered) && !cand_ext.is_ascii() && !prot_ext.is_ascii())
}

/// Non-empty `/`-separated components of `path` after stripping the
/// `/System/Volumes/Data` firmlink (no allocation).
fn path_components(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    strip_firmlink(path)
        .split(|&b| b == b'/')
        .filter(|s| !s.is_empty())
}

/// Collects `path`'s components after charging the vector's exact size bound
/// (a path of `n` bytes has at most `n / 2 + 1` non-empty components).
fn collect_components<'a>(
    path: &'a [u8],
    budget: &mut ComputeBudget,
) -> Result<Vec<&'a [u8]>, Exhausted> {
    let bound = path.len() / 2 + 1;
    budget.charge(path.len().max(1) as u64)?;
    budget.charge_alloc(bound.saturating_mul(std::mem::size_of::<&[u8]>()))?;
    let mut out = Vec::with_capacity(bound);
    out.extend(path_components(path));
    Ok(out)
}

/// Returns the number of leading `cand` components matched when every
/// component of `prot` may match the corresponding `cand` component under an
/// unlisted equivalence (`prot` is a prefix of `cand`), or `None`.
fn comps_match_prefix_unlisted<'p>(
    cand: &[&[u8]],
    prot: impl Iterator<Item = &'p [u8]>,
    budget: &mut ComputeBudget,
) -> Result<Option<usize>, Exhausted> {
    let mut n = 0usize;
    for p in prot {
        let Some(c) = cand.get(n) else {
            return Ok(None);
        };
        if !component_may_match_unlisted(c, p, budget)? {
            return Ok(None);
        }
        n += 1;
    }
    Ok(Some(n))
}

fn comps_match_exact_unlisted<'p>(
    cand: &[&[u8]],
    prot: impl Iterator<Item = &'p [u8]>,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    Ok(comps_match_prefix_unlisted(cand, prot, budget)? == Some(cand.len()))
}

fn str_components(rel: &str) -> impl Iterator<Item = &[u8]> {
    rel.split('/').filter(|s| !s.is_empty()).map(str::as_bytes)
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

/// Pushes `item` onto `v` after charging the vector's growth (§8): when `v`
/// is full, the buffer it is about to allocate (double its capacity, at least
/// four slots) is charged first, so outcome and reason lists never grow
/// uncharged.
///
/// A hit (or an Exhausted record) found before the budget ran out still
/// stands (§8): when charging its slot is what exhausts the budget, it is
/// recorded anyway, like the final Exhausted record, and the error returned.
pub(crate) fn push_charged<T: ChargedRecord>(
    v: &mut Vec<T>,
    item: T,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    if v.len() == v.capacity() {
        let new_cap = v.capacity().saturating_mul(2).max(4);
        if let Err(e) = budget.charge_alloc(new_cap.saturating_mul(std::mem::size_of::<T>())) {
            if item.keep_on_exhaustion() {
                v.push(item);
            }
            return Err(e);
        }
        v.reserve_exact(new_cap - v.len());
    }
    v.push(item);
    Ok(())
}

/// A record kept by [`push_charged`] even when its slot exhausts the budget.
pub(crate) trait ChargedRecord {
    fn keep_on_exhaustion(&self) -> bool {
        false
    }
}

impl ChargedRecord for PartResult {
    fn keep_on_exhaustion(&self) -> bool {
        matches!(
            self.outcome,
            PartOutcome::HardDenyHit(_)
                | PartOutcome::StoreHit(_)
                | PartOutcome::Exhausted(_)
                | PartOutcome::TooLarge(_)
        )
    }
}

impl ChargedRecord for UnreadReason {}
impl ChargedRecord for IdentityGap {}
impl ChargedRecord for Vec<u8> {}
impl ChargedRecord for (FileIdentity, &'static str) {}
impl ChargedRecord for (Vec<u8>, u8) {}
impl ChargedRecord for (Vec<u8>, u8, bool) {}
impl ChargedRecord for (Vec<u8>, Option<FileIdentity>) {}
impl ChargedRecord for (Vec<u8>, Vec<u8>, usize) {}

/// Folds every path of `paths`, charging the list's allocation before building
/// it (each fold charges its own work and allocation).
fn fold_path_list(
    paths: &[Vec<u8>],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Vec<Vec<u8>>, Exhausted> {
    budget.charge_alloc(paths.len().saturating_mul(std::mem::size_of::<Vec<u8>>()))?;
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        out.push(fold_path_listed(p, folds, budget)?);
    }
    Ok(out)
}

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

        // Each fold charges its own work and allocations before running; the
        // second cache comparison below is charged here (§8).
        budget.charge(input_bytes.max(1) as u64)?;
        let key_home = fold_path_listed(&protected.home, folds, budget)?;
        let key_data = fold_path_list(&protected.atrium_data_paths, folds, budget)?;
        let key_protected = fold_path_list(&protected.atrium_protected_paths, folds, budget)?;
        let key_ssh_linked = fold_path_list(&protected.ssh_linked_dirs, folds, budget)?;

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
    /// under extended equivalences (NFC/NFD, unlisted case folds,
    /// default-ignorable code points, or conservatively any scalar the exact
    /// table does not fully decompose) when it did NOT match under the listed
    /// folds (§11 item 13). Every protected name is compared component by
    /// component without allocating; the candidate's component list is charged
    /// before it is built (§8).
    pub(crate) fn matches_unlisted_equivalence(
        &self,
        canonical: &[u8],
        access: AccessMode,
        budget: &mut ComputeBudget,
    ) -> Result<bool, Exhausted> {
        let writes = access != AccessMode::Read;
        let cand_comps = collect_components(canonical, budget)?;

        for d in &self.raw_data {
            if comps_match_prefix_unlisted(&cand_comps, path_components(d), budget)?.is_some() {
                return Ok(true);
            }
        }
        if writes {
            for d in &self.raw_protected {
                if comps_match_prefix_unlisted(&cand_comps, path_components(d), budget)?.is_some() {
                    return Ok(true);
                }
            }
        }
        for d in &self.raw_ssh_linked {
            if let Some(n) = comps_match_prefix_unlisted(&cand_comps, path_components(d), budget)?
                && (cand_comps.len() > n || writes)
            {
                return Ok(true);
            }
        }

        let Some(home_n) =
            comps_match_prefix_unlisted(&cand_comps, path_components(&self.raw_home), budget)?
        else {
            return Ok(false);
        };
        let rest = &cand_comps[home_n..];
        if rest.is_empty() {
            return Ok(false);
        }

        for f in CRED_FILES {
            if comps_match_exact_unlisted(rest, str_components(f), budget)? {
                return Ok(true);
            }
        }
        for t in CRED_TREES {
            if comps_match_prefix_unlisted(rest, str_components(t), budget)?.is_some() {
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
                let (last_ext, _) = fold_extended_component_tracked(last, budget)?;
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
            if let Some(n) = comps_match_prefix_unlisted(rest, str_components(b), budget)?
                && rest.len() > n
            {
                let last = rest[rest.len() - 1];
                for bf in BROWSER_FILES {
                    if component_may_match_unlisted(last, bf.as_bytes(), budget)? {
                        return Ok(true);
                    }
                }
            }
        }
        for &(f, _) in ASK_FILES {
            if comps_match_exact_unlisted(rest, str_components(f), budget)? {
                return Ok(true);
            }
        }
        for &(t, _) in ASK_TREES {
            if comps_match_prefix_unlisted(rest, str_components(t), budget)?.is_some() {
                return Ok(true);
            }
        }
        for &(dir, pre, _) in ASK_NAME_PREFIXES {
            if let Some(n) = comps_match_prefix_unlisted(rest, str_components(dir), budget)?
                && rest.len() == n + 1
            {
                let (last_ext, _) = fold_extended_component_tracked(rest[n], budget)?;
                if last_ext.starts_with(pre.as_bytes()) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Charges the work and an upper bound of the allocations of a probe list
    /// of `count` entries before it is built (§8): each probe is a folded base
    /// (home or an Atrium data path) joined with a relative name of at most 80
    /// bytes, and building it makes at most one temporary of the same size.
    fn charge_probe_list(&self, count: usize, budget: &mut ComputeBudget) -> Result<(), Exhausted> {
        let base = self
            .key_data
            .iter()
            .map(Vec::len)
            .chain(std::iter::once(self.key_home.len()))
            .max()
            .unwrap_or(0);
        let per_probe = base
            .saturating_add(80)
            .saturating_mul(2)
            .saturating_add(std::mem::size_of::<(Vec<u8>, &str)>());
        budget.charge((count as u64).saturating_mul(base as u64 + 80))?;
        budget.charge_alloc(count.saturating_mul(per_probe))
    }

    /// Representative hard-deny probe paths (folded lowercase, absolute) for
    /// §2/§5 filter probe matching (same set as `dev`'s `Sensitive::probes`).
    pub(crate) fn hard_deny_probes(
        &self,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<(Vec<u8>, &'static str)>, Exhausted> {
        let count = CRED_FILES.len()
            + 2 * CRED_TREES.len()
            + SSH_PROBES.len()
            + 2 * BROWSER_ROOTS.len() * BROWSER_FILE_PROBES.len()
            + 2 * self.key_data.len();
        self.charge_probe_list(count, budget)?;
        let join = |base: &[u8], rel: &str| join_folded(base, rel.as_bytes());
        let mut out = Vec::with_capacity(count);
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
        Ok(out)
    }

    /// Representative credential-store probe paths (folded lowercase, absolute)
    /// for §5 filter probe matching (CR-12 + CR-17).
    pub(crate) fn store_probes(
        &self,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<(Vec<u8>, &'static str)>, Exhausted> {
        let count = ASK_FILES.len() + 5 * ASK_TREES.len() + 2 * ASK_NAME_PREFIXES.len();
        self.charge_probe_list(count, budget)?;
        let join = |base: &[u8], rel: &str| join_folded(base, rel.as_bytes());
        let mut out = Vec::with_capacity(count);
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
        Ok(out)
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

/// `check_target` (§5, D-144): the exact check, then the identity gaps. A
/// target the exact check finds Clear or a Store hit, and that a gap of
/// `protected.identity_gaps` can affect, is Unread with the gap's reason
/// (the stricter verdict for a single result; a hard-deny hit is never
/// touched).
pub(crate) fn check_target_impl(
    target: &Target,
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
) -> Result<PartResult, Exhausted> {
    let (exact, gap) = check_target_parts(target, protected, budget)?;
    Ok(gap.unwrap_or(exact))
}

/// The exact check of `target` and, when the exact outcome is Clear or a
/// Store hit and an identity gap applies, the gap's Unread part. The
/// resolver-backed flows record both, so a Store hit is kept beside the doubt.
pub(crate) fn check_target_parts(
    target: &Target,
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
) -> Result<(PartResult, Option<PartResult>), Exhausted> {
    let exact = check_target_exact(target, protected, budget)?;
    if !matches!(exact.outcome, PartOutcome::Clear | PartOutcome::StoreHit(_)) {
        return Ok((exact, None));
    }
    let Some(gap) = applicable_gap(target, protected, budget)? else {
        return Ok((exact, None));
    };
    budget.charge_alloc(unread_reason_alloc_bytes(&gap.reason))?;
    let unread = PartResult {
        part: exact.part.clone(),
        outcome: PartOutcome::Unread(gap.reason.clone()),
    };
    Ok((exact, Some(unread)))
}

fn check_target_exact(
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

    // Precharge the identity scans (every identity against every protected
    // list it is compared with) and the output path copy BEFORE any work or
    // heap allocation (§8); the fold below charges its own work.
    let writes = target.access != AccessMode::Read;
    let id_lists = protected.hard_deny_identities.len()
        + protected.store_identities.len()
        + if writes {
            protected.write_only_hard_deny_identities.len()
        } else {
            0
        };
    budget.charge(
        (canonical.len() as u64 + 1)
            .saturating_add((identities.len() as u64).saturating_mul(id_lists as u64 + 1)),
    )?;
    budget.charge_alloc(canonical.len().max(1))?;

    // 1. Hard-deny identity check (path and every existing ancestor), plus
    //    the write-only identities (`~/.ssh/config`, `~/.ssh/authorized_keys`,
    //    Atrium's protected paths and their members) for any access other
    //    than Read (§5, D-142).
    for id in identities {
        let found = protected
            .hard_deny_identities
            .iter()
            .chain(
                protected
                    .write_only_hard_deny_identities
                    .iter()
                    .filter(|_| writes),
            )
            .find(|(pid, _)| pid == id);
        if let Some((_, hard_id)) = found {
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
    let folded = fold_path_listed(canonical, protected.folds, budget)?;
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
// ProtectedSet builder: the only collector of protected identities (D-145)
// ---------------------------------------------------------------------------
//
// What the builder promises (D-145, contract §5 read with §1 step 4), which
// is what `dev` promises:
// 1. Protected trees and files are protected by name (every listed-fold
//    spelling) and by the identities of their roots: each root's own
//    identity (`lstat`, so a dangling link counts) and what it names after
//    following links (as `dev`'s `metadata`). A target's path and existing
//    ancestors are compared with them.
// 2. Members' identities are collected for hard links only: regular files
//    with more than one link, within the walk caps. A cutoff there is Clear,
//    as in §5's bounded hard-link walk; an uncollected multiply-linked target
//    is already Ask in every setting (§1 step 4). Browser roots are not
//    protected wholesale, so their credential files (and the folders they sit
//    in, through linked profile folders too) are their roots.
// 3. Link targets are recorded only for `~/.ssh` members.
// 4. A gap is Unread only where a root or its name cannot be established: a
//    root or `~/.ssh` that cannot be stat'ed or listed within the caps, or an
//    unreadable folder on the path to a root. Its scope is `Through(root)`,
//    or `AnyTarget` for `~/.ssh` and for a root whose identity is unknown.
// Linked directories are walked like directories.

/// The identities a target can be compared with: a gap applies to it when its
/// scope covers the target and the access matches (contract §2, §5, D-144).
pub(crate) fn gap_applies(gap: &IdentityGap, target: &Target) -> bool {
    if gap.write_only && target.access == AccessMode::Read {
        return false;
    }
    let TargetResolution::Resolved {
        identities,
        multiply_linked,
        ..
    } = &target.resolution
    else {
        return false;
    };
    match &gap.scope {
        GapScope::AnyTarget => true,
        GapScope::HardLinks => *multiply_linked,
        GapScope::Through(id) => *multiply_linked || identities.contains(id),
    }
}

/// The first identity gap of `protected` that applies to `target`, after
/// charging the scan (§8).
pub(crate) fn applicable_gap<'p>(
    target: &Target,
    protected: &'p ProtectedSet,
    budget: &mut ComputeBudget,
) -> Result<Option<&'p IdentityGap>, Exhausted> {
    let ids = match &target.resolution {
        TargetResolution::Resolved { identities, .. } => identities.len(),
        TargetResolution::Unread(_) => 0,
    };
    budget.charge(
        (protected.identity_gaps.len() as u64)
            .saturating_mul(ids as u64 + 1)
            .saturating_add(1),
    )?;
    Ok(protected
        .identity_gaps
        .iter()
        .find(|g| gap_applies(g, target)))
}

/// One cached directory entry with its listed-fold name, folded once per
/// build (round-4 verification: re-folding every entry of `/` and `/tmp` for
/// every protected name exhausted the allocation budget).
struct Listed {
    name: Vec<u8>,
    folded: Vec<u8>,
}

/// A cached directory listing: its entries, absence, or the reason it could
/// not be listed (kept so every consumer records its own gap).
#[derive(Clone)]
enum Listing {
    Entries(Rc<[Listed]>),
    Absent,
    Failed(UnreadReason),
}

type DirCache = HashMap<Vec<u8>, Listing>;

/// Identity lists being filled, with the sets that deduplicate them.
#[derive(Default)]
struct IdentityLists {
    hard_deny: Vec<(FileIdentity, &'static str)>,
    write_only: Vec<(FileIdentity, &'static str)>,
    store: Vec<(FileIdentity, &'static str)>,
    seen_hard_deny: HashSet<FileIdentity>,
    seen_write_only: HashSet<FileIdentity>,
    seen_store: HashSet<FileIdentity>,
}

/// Which identity list an entry goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListKind {
    HardDeny,
    WriteOnly,
    Store,
}

impl ListKind {
    /// A gap in a write-only collection affects only non-Read targets.
    fn write_only(self) -> bool {
        self == Self::WriteOnly
    }
}

impl IdentityLists {
    fn push(
        &mut self,
        kind: ListKind,
        id: FileIdentity,
        tag: &'static str,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        let (list, seen) = match kind {
            ListKind::HardDeny => (&mut self.hard_deny, &mut self.seen_hard_deny),
            ListKind::WriteOnly => (&mut self.write_only, &mut self.seen_write_only),
            ListKind::Store => (&mut self.store, &mut self.seen_store),
        };
        budget.charge(1)?;
        if seen.contains(&id) {
            return Ok(());
        }
        // The deduplicating set's slot (with its growth), then the list entry.
        budget.charge_alloc(4 * std::mem::size_of::<FileIdentity>())?;
        seen.insert(id);
        push_charged(list, (id, tag), budget)
    }
}

/// Which members of a protected tree are recorded (D-145 rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Members {
    /// Regular files with more than one link (hard links).
    HardLinks,
    /// Entries named in `BROWSER_FILES` (a browser root's protected names).
    BrowserFiles,
}

/// The builder's working state.
struct Collector<'r> {
    resolver: &'r dyn PathResolver,
    folds: FoldRules,
    lists: IdentityLists,
    gaps: Vec<IdentityGap>,
    dir_cache: DirCache,
}

/// Maps a resolver failure that is not absence to the Unread reason it
/// stands for (`None` for absence; `Exhausted` is returned).
fn failure_reason(
    err: &ResolveError,
    path: &[u8],
    budget: &mut ComputeBudget,
) -> Result<Option<UnreadReason>, Exhausted> {
    match err {
        ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory) => Ok(None),
        ResolveError::CapReached(c) => Ok(Some(UnreadReason::from(*c))),
        ResolveError::Io(_) => {
            budget.charge_alloc(path.len())?;
            Ok(Some(UnreadReason::UnresolvablePath {
                path: path.to_vec(),
            }))
        }
        ResolveError::Exhausted(e) => Err(*e),
    }
}

/// The gap scope for a root: through its identity when known, else any target.
fn root_scope(id: Option<FileIdentity>) -> GapScope {
    id.map_or(GapScope::AnyTarget, GapScope::Through)
}

impl Collector<'_> {
    fn gap(
        &mut self,
        reason: UnreadReason,
        scope: GapScope,
        write_only: bool,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        let gap = IdentityGap {
            reason,
            scope,
            write_only,
        };
        budget.charge((self.gaps.len() as u64).saturating_add(1))?;
        if self.gaps.contains(&gap) {
            return Ok(());
        }
        // A read-applicable gap subsumes the same write-only one.
        if gap.write_only
            && self
                .gaps
                .iter()
                .any(|g| !g.write_only && g.reason == gap.reason && g.scope == gap.scope)
        {
            return Ok(());
        }
        budget.charge_alloc(unread_reason_alloc_bytes(&gap.reason))?;
        push_charged(&mut self.gaps, gap, budget)
    }

    /// Records a resolver failure on `path` as a gap of `scope` (absence is
    /// not one).
    fn failure(
        &mut self,
        err: ResolveError,
        path: &[u8],
        scope: GapScope,
        write_only: bool,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        if let Some(reason) = failure_reason(&err, path, budget)? {
            self.gap(reason, scope, write_only, budget)?;
        }
        Ok(())
    }

    /// Lists `dir` once per build, folding each name once. A failure is
    /// cached with its reason and returned to every consumer, which records
    /// the gap in its own scope (round-3 finding 3).
    fn list_dir(
        &mut self,
        dir: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Listing, Exhausted> {
        budget.charge(dir.len().max(1) as u64)?;
        if let Some(cached) = self.dir_cache.get(dir) {
            return Ok(cached.clone());
        }
        let listing = match self.resolver.read_dir(dir, caps, budget) {
            Ok(entries) => {
                // The `Vec` of entries and the `Rc` buffer it is moved into.
                budget.charge_alloc(
                    entries
                        .len()
                        .saturating_mul(2 * std::mem::size_of::<Listed>())
                        .saturating_add(32),
                )?;
                let mut listed = Vec::with_capacity(entries.len());
                for e in entries {
                    let folded = fold_listed_component(&e.name, self.folds, budget)?;
                    listed.push(Listed {
                        name: e.name,
                        folded,
                    });
                }
                Listing::Entries(Rc::from(listed))
            }
            Err(err) => match failure_reason(&err, dir, budget)? {
                Some(reason) => Listing::Failed(reason),
                None => Listing::Absent,
            },
        };
        budget.charge_alloc(
            dir.len()
                + std::mem::size_of::<Listing>()
                + 4 * std::mem::size_of::<Vec<u8>>()
                + match &listing {
                    Listing::Failed(r) => unread_reason_alloc_bytes(r),
                    _ => 0,
                },
        )?;
        self.dir_cache.insert(dir.to_vec(), listing.clone());
        Ok(listing)
    }

    /// Every existing listed-fold spelling of the absolute configured path
    /// `abs`, discovered component by component from `/`, so fold variants of
    /// home's own components count too (round-3 finding 4); then the
    /// configured spelling itself, for equivalences the listings don't show.
    /// A folder on the way that cannot be listed may hide spellings whose
    /// identity is unknown: an `AnyTarget` gap (D-145 rule 4), and the
    /// configured spelling is still tried there.
    fn folded_spellings(
        &mut self,
        abs: &[u8],
        write_only: bool,
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<Vec<u8>>, Exhausted> {
        budget.charge_alloc(1 + std::mem::size_of::<Vec<u8>>())?;
        let mut frontier: Vec<Vec<u8>> = vec![b"/".to_vec()];
        for comp in abs.split(|&b| b == b'/').filter(|c| !c.is_empty()) {
            let comp_folded = fold_listed_component(comp, self.folds, budget)?;
            let mut next: Vec<Vec<u8>> = Vec::new();
            for dir in &frontier {
                let listing = self.list_dir(dir, caps, budget)?;
                let names: Vec<&[u8]> = match &listing {
                    Listing::Entries(entries) => {
                        budget.charge(2 * entries.len() as u64 + 1)?;
                        let matches = entries.iter().filter(|l| l.folded == comp_folded).count();
                        budget
                            .charge_alloc(matches.saturating_mul(std::mem::size_of::<&[u8]>()))?;
                        entries
                            .iter()
                            .filter(|l| l.folded == comp_folded)
                            .map(|l| l.name.as_slice())
                            .collect()
                    }
                    Listing::Absent => continue,
                    Listing::Failed(reason) => {
                        self.gap(reason.clone(), GapScope::AnyTarget, write_only, budget)?;
                        vec![comp]
                    }
                };
                for name in names {
                    budget.charge_alloc(dir.len() + 1 + name.len())?;
                    let cand = join_folded(dir, name);
                    budget.charge(next.len() as u64 + 1)?;
                    if !next.contains(&cand) {
                        push_charged(&mut next, cand, budget)?;
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }
        budget.charge(frontier.len() as u64 + 1)?;
        if !frontier.iter().any(|p| p.as_slice() == abs) {
            match self.resolver.lstat(abs, caps, budget) {
                Ok(_) => {
                    budget.charge_alloc(abs.len())?;
                    push_charged(&mut frontier, abs.to_vec(), budget)?;
                }
                Err(err) => self.failure(err, abs, GapScope::AnyTarget, write_only, budget)?,
            }
        }
        Ok(frontier)
    }

    /// `home` joined with the relative protected name `rel`.
    fn under_home(
        home: &[u8],
        rel: &str,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, Exhausted> {
        budget.charge_alloc(home.len() + 1 + rel.len())?;
        Ok(join_folded(home, rel.as_bytes()))
    }

    /// Records a root (D-145 rule 1): its own identity (`lstat`) and, when it
    /// is a link to an existing entry, what it names (as `dev`'s `metadata`).
    /// A root that cannot be stat'ed is an `AnyTarget` gap (its identity is
    /// unknown). Returns what the root names after following links.
    fn record_root(
        &mut self,
        path: &[u8],
        kind: ListKind,
        tag: &'static str,
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Option<PathMetadata>, Exhausted> {
        let meta = match self.resolver.lstat(path, caps, budget) {
            Ok(m) => m,
            Err(err) => {
                self.failure(err, path, GapScope::AnyTarget, kind.write_only(), budget)?;
                return Ok(None);
            }
        };
        if let Some(id) = meta.identity {
            self.lists.push(kind, id, tag, budget)?;
        }
        if meta.file_type != FileType::Symlink {
            return Ok(Some(meta));
        }
        match self.resolver.stat(path, caps, budget) {
            Ok(target) => {
                if let Some(id) = target.identity {
                    self.lists.push(kind, id, tag, budget)?;
                }
                Ok(Some(target))
            }
            Err(ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory)) => {
                Ok(Some(meta))
            }
            Err(err) => {
                // The link's own identity is known, so its scope is through it.
                self.failure(
                    err,
                    path,
                    root_scope(meta.identity),
                    kind.write_only(),
                    budget,
                )?;
                Ok(Some(meta))
            }
        }
    }

    /// Records a root and, when it names a folder (directly or through a
    /// link), its members for hard links (D-145 rules 1-2).
    fn record_tree(
        &mut self,
        path: &[u8],
        kind: ListKind,
        tag: &'static str,
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        if let Some(m) = self.record_root(path, kind, tag, caps, budget)?
            && m.file_type == FileType::Dir
        {
            self.walk_members(path, m, kind, tag, Members::HardLinks, caps, budget)?;
        }
        Ok(())
    }

    /// Walks the folder `root` (whose followed metadata is `root_meta`)
    /// within `WALK_MAX_ENTRIES` entries and `WALK_MAX_DEPTH` levels,
    /// recording `members` (D-145 rule 2). A folder reached through a link is
    /// walked like the folder; folders are visited once by identity.
    ///
    /// Only the root's own listing failing is a gap (`Through(root)`, D-145
    /// rule 4). A cutoff, an unreadable subfolder or a member that cannot be
    /// stat'ed is Clear, as in §5's bounded hard-link walk.
    #[allow(clippy::too_many_arguments)]
    fn walk_members(
        &mut self,
        root: &[u8],
        root_meta: PathMetadata,
        kind: ListKind,
        tag: &'static str,
        members: Members,
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        budget.charge_alloc(root.len() + 2 * std::mem::size_of::<(Vec<u8>, u8)>())?;
        let mut stack: Vec<(Vec<u8>, u8)> = vec![(root.to_vec(), 0)];
        let mut visited: HashSet<FileIdentity> = HashSet::new();
        if let Some(id) = root_meta.identity {
            budget.charge_alloc(4 * std::mem::size_of::<FileIdentity>())?;
            visited.insert(id);
        }
        let mut entries_seen = 0usize;
        while let Some((dir, depth)) = stack.pop() {
            let entries = match self.resolver.read_dir(&dir, caps, budget) {
                Ok(e) => e,
                Err(ResolveError::Exhausted(e)) => return Err(e),
                Err(err) => {
                    if depth == 0 {
                        let scope = root_scope(root_meta.identity);
                        self.failure(err, &dir, scope, kind.write_only(), budget)?;
                    }
                    continue;
                }
            };
            for entry in &entries {
                if entry.name == b"." || entry.name == b".." {
                    continue;
                }
                entries_seen += 1;
                if entries_seen > limits::WALK_MAX_ENTRIES {
                    return Ok(());
                }
                budget.charge(entry.name.len().max(1) as u64)?;
                let named =
                    members == Members::BrowserFiles && entry.file_type != FileType::Dir && {
                        let folded = fold_listed_component(&entry.name, self.folds, budget)?;
                        std::str::from_utf8(&folded).is_ok_and(|n| BROWSER_FILES.contains(&n))
                    };
                // In a browser root only the browser files are protected.
                if members == Members::BrowserFiles
                    && !named
                    && !matches!(entry.file_type, FileType::Dir | FileType::Symlink)
                {
                    continue;
                }
                budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
                let child = join_folded(&dir, &entry.name);
                let meta = match self.resolver.lstat(&child, caps, budget) {
                    Ok(m) => m,
                    Err(ResolveError::Exhausted(e)) => return Err(e),
                    Err(_) => continue,
                };
                if named {
                    // A browser credential file is a root of its own name.
                    if let Some(id) = meta.identity {
                        self.lists.push(kind, id, tag, budget)?;
                    }
                } else if members == Members::HardLinks
                    && meta.file_type == FileType::File
                    && meta.nlink > 1
                    && let Some(id) = meta.identity
                {
                    self.lists.push(kind, id, tag, budget)?;
                }
                // Folders, and links to folders, are walked.
                let folder = match meta.file_type {
                    FileType::Dir => Some(meta),
                    FileType::Symlink => match self.resolver.stat(&child, caps, budget) {
                        Ok(m) if m.file_type == FileType::Dir => Some(m),
                        Ok(_) => None,
                        Err(ResolveError::Exhausted(e)) => return Err(e),
                        Err(_) => None,
                    },
                    _ => None,
                };
                let Some(m) = folder else { continue };
                if depth + 1 >= limits::WALK_MAX_DEPTH {
                    continue;
                }
                if m.identity.is_none_or(|id| !visited.contains(&id)) {
                    if let Some(id) = m.identity {
                        budget.charge_alloc(4 * std::mem::size_of::<FileIdentity>())?;
                        visited.insert(id);
                    }
                    push_charged(&mut stack, (child, depth + 1), budget)?;
                }
            }
        }
        Ok(())
    }

    /// Walks every listed-fold spelling of `~/.ssh` (dev's `ssh_members`,
    /// within the walk caps): private members, with their link targets, go to
    /// the hard-deny list (D-145 rule 3); the top-level `config` and
    /// `authorized_keys` to the write-only list; public names (`*.pub`,
    /// `known_hosts*`) to neither; folders linked into `~/.ssh` are walked and
    /// their real paths returned. Any incompleteness is an `AnyTarget` gap,
    /// since a missed linked folder protects files anywhere by name.
    fn walk_ssh(
        &mut self,
        home: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<Vec<u8>>, Exhausted> {
        let any = GapScope::AnyTarget;
        let mut linked_dirs: Vec<Vec<u8>> = Vec::new();
        let ssh = Self::under_home(home, ".ssh", budget)?;
        let roots = self.folded_spellings(&ssh, false, caps, budget)?;
        let mut stack: Vec<(Vec<u8>, u8, bool)> = Vec::new();
        for r in roots {
            push_charged(&mut stack, (r, 0, false), budget)?;
        }
        let mut visited: HashSet<FileIdentity> = HashSet::new();
        let mut entries_seen = 0usize;
        while let Some((dir, depth, linked)) = stack.pop() {
            let entries = match self.resolver.read_dir(&dir, caps, budget) {
                Ok(e) => e,
                Err(err) => {
                    self.failure(err, &dir, any.clone(), false, budget)?;
                    continue;
                }
            };
            for entry in &entries {
                if entry.name == b"." || entry.name == b".." {
                    continue;
                }
                entries_seen += 1;
                if entries_seen > limits::WALK_MAX_ENTRIES {
                    self.gap(
                        UnreadReason::WalkCapReached(FsCap::EntriesRead),
                        any,
                        false,
                        budget,
                    )?;
                    return Ok(linked_dirs);
                }
                let name = fold_listed_component(&entry.name, self.folds, budget)?;
                let name = std::str::from_utf8(&name).ok();
                let public = !linked
                    && name.is_some_and(|n| n.ends_with(".pub") || n.starts_with("known_hosts"));
                let write_only = !linked
                    && depth == 0
                    && name.is_some_and(|n| n == "config" || n == "authorized_keys");
                let kind = if write_only {
                    ListKind::WriteOnly
                } else {
                    ListKind::HardDeny
                };
                budget.charge_alloc(dir.len() + 1 + entry.name.len())?;
                let child = join_folded(&dir, &entry.name);
                let lmeta = match self.resolver.lstat(&child, caps, budget) {
                    Ok(m) => m,
                    Err(err) => {
                        self.failure(err, &child, any.clone(), false, budget)?;
                        continue;
                    }
                };
                let followed = if lmeta.file_type == FileType::Symlink {
                    match self.resolver.stat(&child, caps, budget) {
                        Ok(m) => Some(m),
                        Err(
                            ResolveError::NotFound | ResolveError::Io(FsIoError::NotADirectory),
                        ) => None,
                        Err(err) => {
                            self.failure(err, &child, any.clone(), false, budget)?;
                            None
                        }
                    }
                } else {
                    Some(lmeta)
                };
                let is_dir = followed.is_some_and(|m| m.file_type == FileType::Dir);
                if !public && lmeta.file_type != FileType::Dir {
                    // The member itself, and a link's target unless it is a
                    // folder (a linked folder is walked, not protected whole).
                    if let Some(id) = lmeta.identity {
                        self.lists.push(kind, id, "credential-read", budget)?;
                    }
                    if !is_dir
                        && lmeta.file_type == FileType::Symlink
                        && let Some(id) = followed.and_then(|m| m.identity)
                    {
                        self.lists.push(kind, id, "credential-read", budget)?;
                    }
                }
                if !is_dir {
                    continue;
                }
                let via_link = lmeta.file_type == FileType::Symlink;
                if via_link {
                    match self.resolver.canonicalize(&child, caps, budget) {
                        Ok(real) => {
                            budget.charge(linked_dirs.len() as u64 + 1)?;
                            if !linked_dirs.contains(&real) {
                                push_charged(&mut linked_dirs, real, budget)?;
                            }
                        }
                        Err(err) => self.failure(err, &child, any.clone(), false, budget)?,
                    }
                }
                if depth + 1 >= limits::WALK_MAX_DEPTH {
                    self.gap(
                        UnreadReason::WalkCapReached(FsCap::WalkDepth),
                        any.clone(),
                        false,
                        budget,
                    )?;
                    continue;
                }
                let m = followed.unwrap_or(lmeta);
                if m.identity.is_none_or(|id| !visited.contains(&id)) {
                    if let Some(id) = m.identity {
                        budget.charge_alloc(4 * std::mem::size_of::<FileIdentity>())?;
                        visited.insert(id);
                    }
                    push_charged(&mut stack, (child, depth + 1, linked || via_link), budget)?;
                }
            }
        }
        Ok(linked_dirs)
    }
}

/// Builds a [`ProtectedSet`] using `resolver` (charging `caps` and `budget`
/// before every operation), applying D-145 (see the rules above):
/// - `hard_deny_identities`: credential files and trees, Atrium's data paths
///   and browser credential files (their roots), the multiply-linked members
///   of those trees, and `~/.ssh`'s private members with their link targets;
/// - `write_only_hard_deny_identities`: `~/.ssh/config`,
///   `~/.ssh/authorized_keys`, and Atrium's protected paths (roots and
///   multiply-linked members), checked only when access is not Read;
/// - `store_identities`: the CR-12 / CR-17 stores (roots and multiply-linked
///   tree members);
/// - `identity_gaps`: only where a root or its name could not be established;
/// - `ssh_linked_dirs`: the real paths of folders linked into `~/.ssh`.
///
/// Every protected name is found under every listed-fold spelling of every
/// component, from `/`. The caller should give the builder its own
/// [`FsCaps`], so its listings don't use up the check's caps.
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
    let mut c = Collector {
        resolver,
        folds,
        lists: IdentityLists::default(),
        gaps: Vec::new(),
        dir_cache: HashMap::new(),
    };
    let hd = ListKind::HardDeny;

    // 1. Credential files and trees.
    for f in CRED_FILES {
        let abs = Collector::under_home(home, f, budget)?;
        for p in c.folded_spellings(&abs, false, caps, budget)? {
            c.record_root(&p, hd, "credential-read", caps, budget)?;
        }
    }
    for t in CRED_TREES {
        let abs = Collector::under_home(home, t, budget)?;
        for p in c.folded_spellings(&abs, false, caps, budget)? {
            c.record_tree(&p, hd, "credential-read", caps, budget)?;
        }
    }

    // 2. Atrium data paths.
    for d in &atrium_data_paths {
        for p in c.folded_spellings(d, false, caps, budget)? {
            c.record_tree(&p, hd, "atrium-self", caps, budget)?;
        }
    }

    // 3. `~/.ssh` members, write-only `config` / `authorized_keys`, linked folders.
    let ssh_linked_dirs = c.walk_ssh(home, caps, budget)?;

    // 4. Browser credential files below every spelling of every browser root
    //    (through linked profile folders too). A root that cannot be stat'ed
    //    or listed is a gap through it.
    for b in BROWSER_ROOTS {
        let abs = Collector::under_home(home, b, budget)?;
        for root in c.folded_spellings(&abs, false, caps, budget)? {
            let meta = match resolver.stat(&root, caps, budget) {
                Ok(m) => m,
                Err(err) => {
                    c.failure(err, &root, GapScope::AnyTarget, false, budget)?;
                    continue;
                }
            };
            if meta.file_type == FileType::Dir {
                c.walk_members(
                    &root,
                    meta,
                    hd,
                    "credential-read",
                    Members::BrowserFiles,
                    caps,
                    budget,
                )?;
            }
        }
    }

    // 5. Atrium protected paths: write-only.
    for d in &atrium_protected_paths {
        for p in c.folded_spellings(d, true, caps, budget)? {
            c.record_tree(&p, ListKind::WriteOnly, "atrium-self", caps, budget)?;
        }
    }

    // 6. Credential stores that ask (CR-12 + CR-17).
    let st = ListKind::Store;
    for &(f, family) in ASK_FILES {
        let abs = Collector::under_home(home, f, budget)?;
        for p in c.folded_spellings(&abs, false, caps, budget)? {
            c.record_root(&p, st, family, caps, budget)?;
        }
    }
    for &(t, family) in ASK_TREES {
        let abs = Collector::under_home(home, t, budget)?;
        for p in c.folded_spellings(&abs, false, caps, budget)? {
            c.record_tree(&p, st, family, caps, budget)?;
        }
    }
    // The name-prefixed families: the folder is their root; each entry with
    // the prefix is a root of its own name.
    for &(dir, pre, family) in ASK_NAME_PREFIXES {
        let abs = Collector::under_home(home, dir, budget)?;
        for dir_path in c.folded_spellings(&abs, false, caps, budget)? {
            let entries = match c.list_dir(&dir_path, caps, budget)? {
                Listing::Entries(entries) => entries,
                Listing::Absent => continue,
                Listing::Failed(reason) => {
                    let scope = match resolver.stat(&dir_path, caps, budget) {
                        Ok(m) => root_scope(m.identity),
                        Err(ResolveError::Exhausted(e)) => return Err(e),
                        Err(_) => GapScope::AnyTarget,
                    };
                    c.gap(reason, scope, false, budget)?;
                    continue;
                }
            };
            for entry in entries.iter().take(limits::WALK_MAX_ENTRIES) {
                budget.charge(entry.folded.len().max(1) as u64)?;
                if entry.folded.starts_with(pre.as_bytes()) {
                    budget.charge_alloc(dir_path.len() + 1 + entry.name.len())?;
                    let p = join_folded(&dir_path, &entry.name);
                    c.record_root(&p, st, family, caps, budget)?;
                }
            }
        }
    }

    let Collector { lists, gaps, .. } = c;
    Ok(ProtectedSet {
        home: home.to_vec(),
        cwd: cwd.to_vec(),
        atrium_data_paths,
        atrium_protected_paths,
        hard_deny_identities: lists.hard_deny,
        write_only_hard_deny_identities: lists.write_only,
        identity_gaps: gaps,
        store_identities: lists.store,
        ssh_linked_dirs,
        folds,
    })
}
