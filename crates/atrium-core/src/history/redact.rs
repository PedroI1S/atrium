//! Redaction for exports (contract §13; must-fix A12, A13).
//!
//! [`Redactor::text`] never changes the text it is given except by replacing
//! spans, so what is exported is the original with holes in it. The spans come
//! from a set of scanners. Each scanner runs on the text as written and again
//! on the text with its escapes decoded (up to [`ESCAPE_DEPTH`] times, for
//! JSON nested inside JSON strings); a match in a decoded view is mapped back
//! to the bytes that produced it.

use std::fmt;

use serde_json::Value;

/// What replaces a secret.
pub(super) const REDACTED: &str = "[redacted]";

/// How many layers of escaping are undone: a JSON document inside a JSON
/// string inside a JSON string.
const ESCAPE_DEPTH: usize = 3;

/// The longest key name looked at before a `=` or `:`.
const MAX_KEY_BYTES: usize = 64;

/// A key whose value is a secret, after upper-casing and turning `-` and `.`
/// into `_`.
const SECRET_KEY_SUFFIXES: &[&[u8]] = &[
    b"TOKEN",
    b"SECRET",
    b"PASSWORD",
    b"PASSWD",
    b"_PASS",
    b"API_KEY",
    b"APIKEY",
    b"SECRET_KEY",
    b"PRIVATE_KEY",
    b"ACCESS_KEY",
];

/// Redaction for exports of real sessions (secrets aren't known canaries).
/// Works on decoded content: reconstructed messages, and JSON strings nested
/// inside tool input and output, after `\uXXXX` and escape decoding.
///
/// - **Tokens**, by their shape: Anthropic (`sk-ant-…`), OpenAI (`sk-…`),
///   GitHub (`gh[pousr]_…`, `github_pat_…`), GitLab (`glpat-…`), npm
///   (`npm_…`), AWS access key ids (`AKIA…`, `ASIA…`), Google API keys
///   (`AIza…`) and OAuth tokens (`ya29.…`), Slack (`xox[abprs]-…`), Stripe live
///   keys (`sk_live_…`, `rk_live_…`), JSON Web Tokens, and PEM private-key
///   blocks.
/// - **Credentials, by where they are**: the value of an `Authorization` or
///   `Cookie` header, a `Bearer …` token, the password in
///   `scheme://user:password@host` and in `-u user:password`, and the value
///   of a key ending in `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `_PASS`,
///   `API_KEY`, `APIKEY`, `SECRET_KEY`, `PRIVATE_KEY` or `ACCESS_KEY` (any
///   case, `-` and `.` count as `_`) written as `KEY=value`, `key = "value"`,
///   `"key": "value"`, `--key value`, `Some-Key: value`, or as a line
///   `key: value` or `key = value` whose value looks random. In JSON, every
///   string under such a key.
/// - The home directory becomes `~`. The user name is kept (it's their own
///   history).
/// - Extra literal secrets can be added (tests use canaries).
///
/// Not caught: a secret with no recognisable shape that isn't next to a
/// telling key (a bare password in prose), and encodings other than JSON-style
/// escapes (base64, percent-encoding).
#[derive(Clone, Default)]
pub struct Redactor {
    /// Absolute, without a trailing slash. `None`: nothing to replace.
    home: Option<String>,
    secrets: Vec<String>,
}

/// Never prints the secrets it was given.
impl fmt::Debug for Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Redactor")
            .field("home", &self.home)
            .field("secrets", &format_args!("<{} hidden>", self.secrets.len()))
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Secret,
    Home,
}

#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
    kind: Kind,
}

/// Receives `(start, end, kind)` for each match, as byte offsets into the
/// scanned text.
type Emit<'a> = &'a mut dyn FnMut(usize, usize, Kind);

impl Redactor {
    pub fn new(home: &str, extra_secrets: &[&str]) -> Self {
        let home = home.trim_end_matches('/');
        Self {
            // A relative or root home would match far too much.
            home: (home.starts_with('/') && home.len() > 1).then(|| home.to_owned()),
            secrets: extra_secrets
                .iter()
                .filter(|s| !s.is_empty())
                .map(|s| (*s).to_owned())
                .collect(),
        }
    }

    pub fn text(&self, text: &str) -> String {
        let mut spans = Vec::new();
        self.scan(text, &mut |start, end, kind| {
            spans.push(Span { start, end, kind });
        });
        let mut view = Unescaped::of(text);
        for _ in 0..ESCAPE_DEPTH {
            let Some(v) = view else { break };
            self.scan(&v.text, &mut |start, end, kind| {
                spans.push(Span {
                    start: v.original(start),
                    end: v.original(end),
                    kind,
                });
            });
            view = v.deeper();
        }
        apply(text, spans)
    }

    /// Redacts a JSON value in place: every string (object keys included)
    /// through [`Redactor::text`], and every string under a key that names a
    /// secret as a whole, however deep (`"password": ["…"]`).
    pub(super) fn value(&self, value: &mut Value) {
        self.value_under(value, false);
    }

    fn value_under(&self, value: &mut Value, secret: bool) {
        match value {
            Value::String(s) if secret => {
                if !s.is_empty() {
                    *s = REDACTED.to_owned();
                }
            }
            Value::String(s) => {
                let redacted = self.text(s);
                if redacted != *s {
                    *s = redacted;
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| self.value_under(v, secret)),
            Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (key, mut v) in std::mem::take(map) {
                    self.value_under(&mut v, secret || is_secret_key(key.as_bytes()));
                    out.insert(self.text(&key), v);
                }
                *map = out;
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    fn scan(&self, text: &str, emit: Emit<'_>) {
        for secret in &self.secrets {
            for (at, found) in text.match_indices(secret.as_str()) {
                emit(at, at + found.len(), Kind::Secret);
            }
        }
        if let Some(home) = &self.home {
            scan_home(text, home, emit);
        }
        let bytes = text.as_bytes();
        scan_tokens(bytes, emit);
        scan_private_keys(text, emit);
        scan_authorization(bytes, emit);
        scan_assignments(bytes, emit);
        scan_flags(bytes, emit);
        scan_url_passwords(bytes, emit);
    }
}

/// Replaces the spans in `text`. Overlapping spans are merged, and a merged
/// span that contains any secret is redacted as a secret.
fn apply(text: &str, mut spans: Vec<Span>) -> String {
    if spans.is_empty() {
        return text.to_owned();
    }
    spans.sort_by_key(|s| (s.start, s.end));
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut spans = spans.into_iter().peekable();
    while let Some(first) = spans.next() {
        let (mut start, mut end, mut kind) = (first.start, first.end.min(text.len()), first.kind);
        while let Some(next) = spans.next_if(|n| n.start < end) {
            end = end.max(next.end.min(text.len()));
            if next.kind == Kind::Secret {
                kind = Kind::Secret;
            }
        }
        // Scanners only produce character boundaries. If one ever didn't,
        // redact the whole character rather than panic or leak half of it.
        while !text.is_char_boundary(start) {
            start -= 1;
        }
        while !text.is_char_boundary(end) {
            end += 1;
        }
        let start = start.max(copied);
        if end <= start {
            continue;
        }
        out.push_str(&text[copied..start]);
        out.push_str(match kind {
            Kind::Secret => REDACTED,
            Kind::Home => "~",
        });
        copied = end;
    }
    out.push_str(&text[copied..]);
    out
}

// ---------------------------------------------------------------------------
// Escape decoding
// ---------------------------------------------------------------------------

/// A text with one layer of escapes decoded, and where each part came from.
struct Unescaped {
    text: String,
    /// `(offset in text, offset in the source)` right after each decoded
    /// escape, ascending, starting with `(0, 0)`. Between two entries the
    /// offsets advance together.
    map: Vec<(usize, usize)>,
    /// The view this one was decoded from; `None` for the original text.
    parent: Option<Box<Unescaped>>,
}

impl Unescaped {
    /// `None` when `text` has no escape to decode.
    fn of(text: &str) -> Option<Self> {
        let (text, map) = unescape(text)?;
        Some(Self {
            text,
            map,
            parent: None,
        })
    }

    /// This view with one more layer decoded; `None` when nothing is left.
    fn deeper(self) -> Option<Self> {
        let (text, map) = unescape(&self.text)?;
        Some(Self {
            text,
            map,
            parent: Some(Box::new(self)),
        })
    }

    fn source_offset(&self, at: usize) -> usize {
        let after = self.map.partition_point(|&(here, _)| here <= at);
        // `map[0]` is `(0, 0)`, so `after` is at least 1.
        let (here, there) = self.map[after.saturating_sub(1)];
        there + (at - here)
    }

    /// The offset in the original text of the byte that decoded to `at`.
    fn original(&self, at: usize) -> usize {
        let at = self.source_offset(at);
        match &self.parent {
            Some(parent) => parent.original(at),
            None => at,
        }
    }
}

/// Decodes one layer of escapes. Returns the text and its offset map (see
/// [`Unescaped::map`]), or `None` when there was nothing to decode.
fn unescape(source: &str) -> Option<(String, Vec<(usize, usize)>)> {
    let bytes = source.as_bytes();
    let mut next = memchr(b'\\', bytes, 0)?;
    let mut text = String::new();
    let mut map = vec![(0, 0)];
    let mut copied = 0;
    loop {
        match decode_escape(&bytes[next + 1..]) {
            Some((c, len)) => {
                text.push_str(&source[copied..next]);
                text.push(c);
                copied = next + 1 + len;
                map.push((text.len(), copied));
                next = copied;
            }
            None => next += 1,
        }
        match memchr(b'\\', bytes, next) {
            Some(at) => next = at,
            None => break,
        }
    }
    if map.len() == 1 {
        return None;
    }
    text.push_str(&source[copied..]);
    Some((text, map))
}

fn memchr(needle: u8, bytes: &[u8], from: usize) -> Option<usize> {
    bytes
        .get(from..)?
        .iter()
        .position(|b| *b == needle)
        .map(|at| from + at)
}

/// Decodes the escape whose backslash precedes `rest`. Returns the character
/// and how many bytes of `rest` it used. JSON's escapes, plus `\'`, `\xNN`
/// (ASCII only) and `\u{…}`.
fn decode_escape(rest: &[u8]) -> Option<(char, usize)> {
    let simple = |c| Some((c, 1));
    match *rest.first()? {
        b'"' => simple('"'),
        b'\'' => simple('\''),
        b'\\' => simple('\\'),
        b'/' => simple('/'),
        b'n' => simple('\n'),
        b'r' => simple('\r'),
        b't' => simple('\t'),
        b'b' => simple('\u{8}'),
        b'f' => simple('\u{c}'),
        b'x' => {
            let code = hex_value(rest.get(1..3)?)?;
            (code < 0x80).then(|| (char::from(code as u8), 3))
        }
        b'u' if rest.get(1) == Some(&b'{') => {
            let digits = rest[2..].iter().take(7).position(|b| *b == b'}')?;
            let code = hex_value(&rest[2..2 + digits])?;
            Some((char::from_u32(code)?, digits + 3))
        }
        b'u' => {
            let unit = hex_value(rest.get(1..5)?)?;
            match unit {
                // A high surrogate needs its low half right after it.
                0xD800..=0xDBFF => {
                    if rest.get(5..7)? != b"\\u" {
                        return None;
                    }
                    let low = hex_value(rest.get(7..11)?)?;
                    if !(0xDC00..=0xDFFF).contains(&low) {
                        return None;
                    }
                    let code = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                    Some((char::from_u32(code)?, 11))
                }
                _ => Some((char::from_u32(unit)?, 5)),
            }
        }
        _ => None,
    }
}

/// The value of 1 to 6 hex digits; `None` for anything else.
fn hex_value(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() || digits.len() > 6 {
        return None;
    }
    digits
        .iter()
        .try_fold(0u32, |acc, d| Some(acc << 4 | char::from(*d).to_digit(16)?))
}

// ---------------------------------------------------------------------------
// Character classes
// ---------------------------------------------------------------------------

fn is_alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

fn is_word(c: u8) -> bool {
    is_alnum(c) || c == b'_'
}

fn is_base64url(c: u8) -> bool {
    is_word(c) || c == b'-'
}

fn is_alnum_or_dash(c: u8) -> bool {
    is_alnum(c) || c == b'-'
}

fn is_base64url_or_dot(c: u8) -> bool {
    is_base64url(c) || c == b'.'
}

/// RFC 6750's `b64token` characters.
fn is_bearer(c: u8) -> bool {
    is_alnum(c) || matches!(c, b'-' | b'.' | b'_' | b'~' | b'+' | b'/' | b'=')
}

fn is_key(c: u8) -> bool {
    is_alnum(c) || matches!(c, b'_' | b'-' | b'.')
}

fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn is_quote(c: u8) -> bool {
    c == b'"' || c == b'\''
}

/// How many bytes from `at` satisfy `class`.
fn run(bytes: &[u8], at: usize, class: fn(u8) -> bool) -> usize {
    bytes[at..].iter().take_while(|c| class(**c)).count()
}

fn skip_blanks(bytes: &[u8], at: usize) -> usize {
    at + run(bytes, at, is_blank)
}

/// The first position at or after `from` where `needle` (ASCII) occurs,
/// ignoring ASCII case.
fn find_ignoring_case(bytes: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let last = (bytes.len() + 1).saturating_sub(needle.len());
    (from..last).find(|&at| bytes[at..at + needle.len()].eq_ignore_ascii_case(needle))
}

fn is_secret_key(key: &[u8]) -> bool {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return false;
    }
    let mut name = [0u8; MAX_KEY_BYTES];
    let name = &mut name[..key.len()];
    for (to, from) in name.iter_mut().zip(key) {
        *to = match from {
            b'-' | b'.' => b'_',
            c => c.to_ascii_uppercase(),
        };
    }
    name == b"PASS" || SECRET_KEY_SUFFIXES.iter().any(|s| name.ends_with(s))
}

// ---------------------------------------------------------------------------
// Scanners
// ---------------------------------------------------------------------------

/// The home directory, when it isn't part of a longer name.
fn scan_home(text: &str, home: &str, emit: Emit<'_>) {
    let bytes = text.as_bytes();
    let in_name = |c: u8| is_base64url(c) || !c.is_ascii();
    for (at, _) in text.match_indices(home) {
        let end = at + home.len();
        // `/mnt/Users/pedro`, `/Users/pedro2`, `/Users/pedro.bak`.
        if at > 0 && in_name(bytes[at - 1]) {
            continue;
        }
        let longer = match bytes.get(end) {
            None => false,
            // A full stop ending a sentence, or the start of an extension.
            Some(b'.') => bytes.get(end + 1).is_some_and(|c| in_name(*c)),
            Some(c) => in_name(*c),
        };
        if !longer {
            emit(at, end, Kind::Home);
        }
    }
}

/// A token recognised by its prefix.
struct Prefix {
    text: &'static [u8],
    /// The characters the rest of the token is made of.
    tail: fn(u8) -> bool,
    min_tail: usize,
    /// The prefix also starts ordinary words (`task-…`, `sk-spinner`), so it
    /// must not continue a word and the tail must contain a digit.
    common: bool,
}

const fn prefix(text: &'static [u8], tail: fn(u8) -> bool, min_tail: usize) -> Prefix {
    Prefix {
        text,
        tail,
        min_tail,
        common: false,
    }
}

/// Longer prefixes come before the ones they start with.
const PREFIXES: &[Prefix] = &[
    prefix(b"sk-ant-", is_base64url, 16),
    Prefix {
        text: b"sk-",
        tail: is_base64url,
        min_tail: 20,
        common: true,
    },
    prefix(b"sk_live_", is_alnum, 16),
    prefix(b"rk_live_", is_alnum, 16),
    prefix(b"ghp_", is_word, 20),
    prefix(b"gho_", is_word, 20),
    prefix(b"ghu_", is_word, 20),
    prefix(b"ghs_", is_word, 20),
    prefix(b"ghr_", is_word, 20),
    prefix(b"github_pat_", is_word, 20),
    prefix(b"glpat-", is_base64url, 20),
    prefix(b"npm_", is_alnum, 30),
    prefix(b"xoxa-", is_alnum_or_dash, 10),
    prefix(b"xoxb-", is_alnum_or_dash, 10),
    prefix(b"xoxp-", is_alnum_or_dash, 10),
    prefix(b"xoxr-", is_alnum_or_dash, 10),
    prefix(b"xoxs-", is_alnum_or_dash, 10),
    prefix(b"AIza", is_base64url, 30),
    prefix(b"ya29.", is_base64url_or_dot, 20),
];

fn scan_tokens(bytes: &[u8], emit: Emit<'_>) {
    let mut at = 0;
    while at < bytes.len() {
        // Every token below starts with one of these.
        if !matches!(
            bytes[at],
            b's' | b'r' | b'g' | b'n' | b'x' | b'A' | b'y' | b'e'
        ) {
            at += 1;
            continue;
        }
        match token_end(bytes, at) {
            Some(end) => {
                emit(at, end, Kind::Secret);
                at = end;
            }
            None => at += 1,
        }
    }
}

fn token_end(bytes: &[u8], at: usize) -> Option<usize> {
    let rest = &bytes[at..];
    let continues_word = at > 0 && is_base64url(bytes[at - 1]);
    for p in PREFIXES {
        if !rest.starts_with(p.text) || (p.common && continues_word) {
            continue;
        }
        let tail = &rest[p.text.len()..];
        let len = tail.iter().take_while(|c| (p.tail)(**c)).count();
        if len < p.min_tail || (p.common && !tail[..len].iter().any(u8::is_ascii_digit)) {
            continue;
        }
        return Some(at + p.text.len() + len);
    }
    aws_key_end(bytes, at).or_else(|| jwt_end(bytes, at))
}

/// `AKIA`/`ASIA` and 16 capitals or digits, standing alone.
fn aws_key_end(bytes: &[u8], at: usize) -> Option<usize> {
    let rest = &bytes[at..];
    if !(rest.starts_with(b"AKIA") || rest.starts_with(b"ASIA")) {
        return None;
    }
    if at > 0 && is_alnum(bytes[at - 1]) {
        return None;
    }
    let id = rest.get(4..20)?;
    if !id
        .iter()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return None;
    }
    if rest.get(20).is_some_and(|c| is_alnum(*c)) {
        return None;
    }
    Some(at + 20)
}

/// Three base64url parts joined by dots, the first two starting with `eyJ`
/// (base64 for `{"`).
fn jwt_end(bytes: &[u8], at: usize) -> Option<usize> {
    if at > 0 && is_base64url(bytes[at - 1]) {
        return None;
    }
    let mut end = at;
    for part in 0..3 {
        if part < 2 && !bytes[end..].starts_with(b"eyJ") {
            return None;
        }
        let len = run(bytes, end, is_base64url);
        if len < 10 {
            return None;
        }
        end += len;
        if part < 2 {
            if bytes.get(end) != Some(&b'.') {
                return None;
            }
            end += 1;
        }
    }
    Some(end)
}

/// `-----BEGIN … PRIVATE KEY…-----` to the end of its `-----END …-----` line,
/// or to the end of the text when the block was cut off.
fn scan_private_keys(text: &str, emit: Emit<'_>) {
    const BEGIN: &str = "-----BEGIN ";
    const DASHES: &str = "-----";
    let mut from = 0;
    while let Some(found) = text[from..].find(BEGIN) {
        let start = from + found;
        let label_start = start + BEGIN.len();
        let label_len = text.as_bytes()[label_start..]
            .iter()
            .take(48)
            .take_while(|c| c.is_ascii_uppercase() || **c == b' ')
            .count();
        let body = label_start + label_len;
        from = body;
        if !text[label_start..body].contains("PRIVATE KEY") || !text[body..].starts_with(DASHES) {
            continue;
        }
        let end = match text[body..].find("-----END ") {
            Some(footer) => {
                let footer = body + footer + DASHES.len();
                match text[footer..].find(DASHES) {
                    Some(close) if close <= 64 => footer + close + DASHES.len(),
                    _ => footer,
                }
            }
            None => text.len(),
        };
        emit(start, end, Kind::Secret);
        from = end;
    }
}

/// The value of an `Authorization` or `Cookie` header (in a header line, a
/// curl argument or a JSON map; `Proxy-Authorization` and `Set-Cookie` too),
/// and any `Bearer <token>`.
///
/// Both names are also ordinary identifiers (`cookie: &str`,
/// `authorization = get()`), so a value is taken only when it has the shape
/// of that header's value.
fn scan_authorization(bytes: &[u8], emit: Emit<'_>) {
    let mut runs = BareRuns::default();
    for (name, is_cookie) in [(b"authorization".as_slice(), false), (b"cookie", true)] {
        let mut from = 0;
        while let Some(at) = find_ignoring_case(bytes, name, from) {
            from = at + name.len();
            // `my_cookie`, `reauthorization`; a dash is fine (`Set-Cookie`).
            if at > 0 && is_word(bytes[at - 1]) {
                continue;
            }
            // `-H "Name: value"` ends at the quote it started in;
            // `"Name": "value"` at the value's own.
            let mut closer = opening_quote(bytes, at);
            let mut i = from;
            if bytes.get(i).is_some_and(|c| is_quote(*c)) {
                i += 1;
            }
            i = skip_blanks(bytes, i);
            match bytes.get(i) {
                Some(b':') => {}
                Some(b'=') if !is_cookie => {}
                _ => continue,
            }
            i = skip_blanks(bytes, i + 1);
            if let Some(&quote) = bytes.get(i).filter(|c| is_quote(**c)) {
                closer = Some(quote);
                i += 1;
            }
            let end = if is_cookie {
                cookie_end(bytes, i, closer)
            } else {
                authorization_end(bytes, i, closer, &mut runs)
            };
            if let Some(end) = end {
                emit(i, end, Kind::Secret);
                // Nothing inside the value needs another look (and looking
                // again from every match would take quadratic time on one
                // long line).
                from = end.max(from);
            }
        }
    }
    const SCHEME: &[u8] = b"bearer";
    let mut from = 0;
    while let Some(at) = find_ignoring_case(bytes, SCHEME, from) {
        from = at + SCHEME.len();
        if at > 0 && is_alnum(bytes[at - 1]) {
            continue;
        }
        let start = skip_blanks(bytes, from);
        let len = run(bytes, start, is_bearer);
        if start > from && len >= 16 {
            emit(start, start + len, Kind::Secret);
        }
        from = start + len;
    }
}

/// Where a `Cookie` value starting at `value` ends, if it is one: it starts
/// with `name=`, as in `session=abc; theme=dark`.
fn cookie_end(bytes: &[u8], value: usize, closer: Option<u8>) -> Option<usize> {
    const MAX_NAME: usize = 128;
    let name = bytes[value..]
        .iter()
        .take(MAX_NAME + 1)
        .take_while(|c| is_cookie_name(**c))
        .count();
    let pair = (1..=MAX_NAME).contains(&name)
        && bytes.get(value + name) == Some(&b'=')
        && bytes
            .get(value + name + 1)
            .is_some_and(|c| !c.is_ascii_whitespace() && *c != b'=');
    pair.then(|| header_value_end(bytes, value, closer))
}

/// RFC 6265's cookie-name characters (`$Version`, `auth+session`), without
/// the quotes and backtick a header can be written inside.
fn is_cookie_name(c: u8) -> bool {
    c.is_ascii_graphic()
        && !matches!(
            c,
            b'(' | b')'
                | b'<'
                | b'>'
                | b'@'
                | b','
                | b';'
                | b':'
                | b'\\'
                | b'"'
                | b'\''
                | b'`'
                | b'/'
                | b'['
                | b']'
                | b'?'
                | b'='
                | b'{'
                | b'}'
        )
}

/// Where an `Authorization` value starting at `value` ends, if it is a
/// credential.
fn authorization_end(
    bytes: &[u8],
    value: usize,
    closer: Option<u8>,
    runs: &mut BareRuns,
) -> Option<usize> {
    const MAX_SCHEME: usize = 32;
    // `<scheme> <credentials>`: `Bearer …`, `Basic …`, `token …`,
    // `AWS4-HMAC-SHA256 …`. Not `str = Header()` or `string | undefined`.
    let scheme = bytes[value..]
        .iter()
        .take(MAX_SCHEME + 1)
        .take_while(|c| is_alnum_or_dash(**c))
        .count();
    if (1..=MAX_SCHEME).contains(&scheme)
        && bytes[value].is_ascii_alphabetic()
        && bytes.get(value + scheme).is_some_and(|c| is_blank(*c))
    {
        let credentials = skip_blanks(bytes, value + scheme);
        return bytes
            .get(credentials)
            .is_some_and(|c| is_bearer(*c) && *c != b'=')
            .then(|| header_value_end(bytes, value, closer));
    }
    // A credential with no scheme: one random-looking word that is the whole
    // value.
    let end = runs.random_end(bytes, value)?;
    let next = bytes.get(skip_blanks(bytes, end)).copied();
    (matches!(next, None | Some(b'\n' | b'\r')) || next == closer).then_some(end)
}

/// Answers "does the bare value starting here look random?" for many starts
/// in the same whitespace-free run, walking each run once: a value that is
/// measured and then not taken must not be measured again from every later
/// match inside it.
#[derive(Default)]
struct BareRuns {
    start: usize,
    end: usize,
    last_digit: Option<usize>,
    last_code: Option<usize>,
}

impl BareRuns {
    /// The end of the bare value starting at `from`, if it [`looks_random`].
    /// Starts must not decrease from one call to the next.
    fn random_end(&mut self, bytes: &[u8], from: usize) -> Option<usize> {
        if from < self.start || from >= self.end {
            let len = bare_value_len(&bytes[from..]);
            *self = Self {
                start: from,
                end: from + len,
                last_digit: None,
                last_code: None,
            };
            for (at, c) in bytes[from..from + len].iter().enumerate() {
                if c.is_ascii_digit() {
                    self.last_digit = Some(from + at);
                } else if is_code_punctuation(*c) {
                    self.last_code = Some(from + at);
                }
            }
        }
        let random = self.end - from >= MIN_RANDOM_LEN
            && self.last_digit.is_some_and(|at| at >= from)
            && self.last_code.is_none_or(|at| at < from);
        random.then_some(self.end)
    }
}

/// The value given to a key that names a secret: `KEY=value`,
/// `key = "value"`, `"key": "value"`, and YAML's `key: value`.
fn scan_assignments(bytes: &[u8], emit: Emit<'_>) {
    let mut sep = 0;
    while sep < bytes.len() {
        // Where to look next: past a value that was taken, since nothing
        // inside it needs another look (and looking again from every
        // separator would take quadratic time on one long line).
        sep = match assignment_value(bytes, sep) {
            Some((start, end)) => {
                emit(start, end, Kind::Secret);
                end.max(sep + 1)
            }
            None => sep + 1,
        };
    }
}

/// The span of the secret value assigned at `sep`, if `sep` is a `=` or `:`
/// after a key that names a secret.
fn assignment_value(bytes: &[u8], sep: usize) -> Option<(usize, usize)> {
    let c = bytes[sep];
    if c != b'=' && c != b':' {
        return None;
    }
    // The key, looking back over blanks and a closing quote.
    let mut key_end = sep;
    while key_end > 0 && is_blank(bytes[key_end - 1]) {
        key_end -= 1;
    }
    let spaced_before = key_end < sep;
    let quoted_key = key_end > 0 && is_quote(bytes[key_end - 1]);
    if quoted_key {
        key_end -= 1;
    }
    let key_start = key_end
        - bytes[..key_end]
            .iter()
            .rev()
            .take(MAX_KEY_BYTES + 1)
            .take_while(|c| is_key(**c))
            .count();
    if !is_secret_key(&bytes[key_start..key_end]) {
        return None;
    }

    let mut value = sep + 1;
    // `=>` (Ruby, PHP).
    if c == b'=' && bytes.get(value) == Some(&b'>') {
        value += 1;
    }
    let after_sep = value;
    value = skip_blanks(bytes, value);
    let spaced_after = value > after_sep;

    // A string literal is always taken.
    if let Some(&quote) = bytes.get(value).filter(|c| is_quote(**c)) {
        let end = closing_quote(bytes, value + 1, quote);
        return (end > value + 1).then_some((value + 1, end));
    }
    // A bare value is taken only where it is unlikely to be code. These
    // checks come before the value is measured, so a value that is skipped
    // is never walked.
    if quoted_key {
        return None;
    }
    let key = &bytes[key_start..key_end];
    match (c, spaced_before, spaced_after) {
        // `KEY=value`, `?token=value`, `--password=value`; `==` compares.
        (b'=', false, false) => {
            if bytes.get(value) == Some(&b'=') {
                return None;
            }
            let end = value + bare_value_len(&bytes[value..]);
            let bare = &bytes[value..end];
            (!bare.is_empty() && !is_keyword(bare)).then_some((value, end))
        }
        // `X-Api-Key: value`, `api-key: value`: a name with a dash is a
        // header or a config key, never a variable. The rest of the line.
        (b':', false, true) if key.contains(&b'-') => {
            let end = header_value_end(bytes, value, opening_quote(bytes, key_start));
            (end > value).then_some((value, end))
        }
        // `key: value` (YAML) and `key = value` (INI, TOML). These are also
        // what code looks like (`token: String,`, `token = next()`), so the
        // key must start its line and the value must look random. One key
        // can start a line, so each line's value is walked at most once.
        (b':', false, true) | (b'=', _, _) => {
            if !starts_line(bytes, key_start) {
                return None;
            }
            let end = value + bare_value_len(&bytes[value..]);
            let bare = &bytes[value..end];
            (ends_line(bytes, end) && looks_random(bare)).then_some((value, end))
        }
        _ => None,
    }
}

/// The quote or backtick right before `at`, if there is one: what closes a
/// header that is written inside a string (`-H "Name: value"`).
fn opening_quote(bytes: &[u8], at: usize) -> Option<u8> {
    at.checked_sub(1)
        .map(|before| bytes[before])
        .filter(|c| is_quote(*c) || *c == b'`')
}

/// Where a header's value ends: the line, or `closer`, the quote the header
/// or its value is written inside. Trailing blanks are left out. A header
/// value can hold quotes of its own (`Digest username="…"`), so nothing else
/// ends it; inside a string those are escaped (`\"`), and a backslash
/// escapes the next character there.
fn header_value_end(bytes: &[u8], value: usize, closer: Option<u8>) -> usize {
    let mut end = value;
    while let Some(&c) = bytes.get(end) {
        match c {
            b'\n' | b'\r' => break,
            b'\\' if closer.is_some() => end += 1,
            c if Some(c) == closer => break,
            _ => {}
        }
        end += 1;
    }
    // A trailing backslash steps past the end. Otherwise the loop stops at
    // an ASCII byte or the end, so this is a character boundary.
    let mut end = end.min(bytes.len());
    while end > value && is_blank(bytes[end - 1]) {
        end -= 1;
    }
    end
}

/// An unquoted value runs to the next whitespace, quote or backtick.
fn bare_value_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|c| !c.is_ascii_whitespace() && !is_quote(**c) && **c != b'`')
        .count()
}

/// The end of a quoted string that starts at `from`: its closing quote, the
/// end of the line, or the end of the text. A backslash escapes the next
/// character.
fn closing_quote(bytes: &[u8], from: usize, quote: u8) -> usize {
    let mut i = from;
    while let Some(&c) = bytes.get(i) {
        match c {
            b'\\' => i += 1,
            b'\n' | b'\r' => return i,
            c if c == quote => return i,
            _ => {}
        }
        i += 1;
    }
    // Only a quote, a line end or the end of the text is returned, so the
    // result is a character boundary even when a backslash stepped into the
    // middle of a multi-byte character.
    bytes.len()
}

/// Values that say nothing: `password=null`, `use_token: true`.
fn is_keyword(value: &[u8]) -> bool {
    const WORDS: &[&[u8]] = &[
        b"true",
        b"false",
        b"null",
        b"none",
        b"nil",
        b"yes",
        b"no",
        b"undefined",
    ];
    WORDS.iter().any(|w| value.eq_ignore_ascii_case(w))
}

/// The key starts its line, after indentation and an optional list dash.
fn starts_line(bytes: &[u8], key_start: usize) -> bool {
    bytes[..key_start]
        .iter()
        .rev()
        .take_while(|c| **c != b'\n')
        .all(|c| is_blank(*c) || *c == b'-')
}

fn ends_line(bytes: &[u8], at: usize) -> bool {
    matches!(
        bytes.get(skip_blanks(bytes, at)),
        None | Some(b'\n' | b'\r')
    )
}

/// A bare value on a `key: value` or `key = value` line is only taken when
/// it can't be a type or an expression (`token: String,`,
/// `secret: Vec<u8>`): long enough, with a digit, and none of the
/// punctuation code has.
fn looks_random(value: &[u8]) -> bool {
    value.len() >= MIN_RANDOM_LEN
        && value.iter().any(u8::is_ascii_digit)
        && !value.iter().any(|c| is_code_punctuation(*c))
}

const MIN_RANDOM_LEN: usize = 8;

fn is_code_punctuation(c: u8) -> bool {
    matches!(
        c,
        b'<' | b'>' | b'(' | b')' | b'[' | b']' | b'{' | b'}' | b';' | b','
    )
}

/// `--password hunter2`, `--api-key "…"`: a flag that names a secret and
/// takes its value as the next argument. Also the password in
/// `-u user:password` (`--user`, `--proxy-user`).
fn scan_flags(bytes: &[u8], emit: Emit<'_>) {
    let mut at = 0;
    while let Some(found) = memchr(b'-', bytes, at) {
        at = found + 1;
        if found > 0 && is_key(bytes[found - 1]) {
            continue;
        }
        let long = bytes.get(found + 1) == Some(&b'-');
        let key = found + if long { 2 } else { 1 };
        let key_end = key + run(bytes, key, is_key);
        at = key_end.max(at);
        let name = &bytes[key..key_end];
        let user = name == b"u" || (long && (name == b"user" || name == b"proxy-user"));
        let value = skip_blanks(bytes, key_end);
        let value = match bytes.get(key_end) {
            // `--user=user:password`.
            Some(b'=') if user && long => key_end + 1,
            _ if value == key_end => continue,
            _ => value,
        };
        // `curl -u user:password`, `--user "user:password"`.
        if user {
            let (start, end) = match bytes.get(value) {
                Some(&quote) if is_quote(quote) => {
                    (value + 1, closing_quote(bytes, value + 1, quote))
                }
                _ => (value, value + bare_value_len(&bytes[value..])),
            };
            if let Some(colon) = memchr(b':', &bytes[..end], start).filter(|c| c + 1 < end) {
                emit(colon + 1, end, Kind::Secret);
            }
            at = end.max(at);
            continue;
        }
        if !long || !is_secret_key(name) {
            continue;
        }
        match bytes.get(value) {
            Some(&quote) if is_quote(quote) => {
                let end = closing_quote(bytes, value + 1, quote);
                if end > value + 1 {
                    emit(value + 1, end, Kind::Secret);
                }
                at = end;
            }
            // Another flag follows: this one took no value.
            Some(b'-') | None => {}
            Some(_) => {
                let end = value + bare_value_len(&bytes[value..]);
                if end > value {
                    emit(value, end, Kind::Secret);
                }
                at = end;
            }
        }
    }
}

/// The password in `scheme://user:password@host`.
fn scan_url_passwords(bytes: &[u8], emit: Emit<'_>) {
    const MAX_USERINFO: usize = 256;
    let mut at = 0;
    while let Some(found) = memchr(b':', bytes, at) {
        at = found + 1;
        if !bytes[at..].starts_with(b"//") {
            continue;
        }
        let userinfo = at + 2;
        let mut colon = None;
        for (i, &c) in bytes[userinfo..].iter().take(MAX_USERINFO).enumerate() {
            match c {
                b':' if colon.is_none() => colon = Some(userinfo + i),
                b'@' => {
                    if let Some(colon) = colon.filter(|c| c + 1 < userinfo + i) {
                        emit(colon + 1, userinfo + i, Kind::Secret);
                    }
                    break;
                }
                b'/' | b'?' | b'#' | b'"' | b'\'' | b'`' | b'<' | b'>' => break,
                c if c.is_ascii_whitespace() => break,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_offsets_map_back_to_the_escape_that_produced_them() {
        let source = r#"a\nb\u00e9c\"d"#;
        let view = Unescaped::of(source).expect("has escapes");
        assert_eq!(view.text, "a\nbéc\"d");
        // `\n` is bytes 1..3 of the source and byte 1 of the text.
        assert_eq!((view.original(1), view.original(2)), (1, 3));
        // `é` is two bytes in the text and six in the source.
        let e = view.text.find('é').expect("decoded");
        assert_eq!((view.original(e), view.original(e + 2)), (4, 10));
        assert_eq!(view.original(view.text.len()), source.len());
    }

    #[test]
    fn nested_views_map_through_every_layer() {
        // A JSON string holding a JSON string: the quote is `\\\"` here.
        let source = r#"x\\\"y"#;
        let first = Unescaped::of(source).expect("layer 1");
        assert_eq!(first.text, r#"x\"y"#);
        let second = first.deeper().expect("layer 2");
        assert_eq!(second.text, "x\"y");
        assert_eq!((second.original(1), second.original(2)), (1, 5));
    }

    #[test]
    fn escapes_that_are_not_escapes_are_left_alone() {
        assert!(Unescaped::of("no escapes").is_none());
        assert!(Unescaped::of(r"C:\Users \q \u12 \uD800 \x4 \").is_none());
        let pair = Unescaped::of(r"\uD83D\uDE00 \x41 \u{1F600}").expect("decodes");
        assert_eq!(pair.text, "😀 A 😀");
    }

    #[test]
    fn overlapping_spans_merge_and_a_secret_wins() {
        let spans = vec![
            Span {
                start: 2,
                end: 6,
                kind: Kind::Home,
            },
            Span {
                start: 4,
                end: 8,
                kind: Kind::Secret,
            },
            Span {
                start: 9,
                end: 10,
                kind: Kind::Home,
            },
        ];
        assert_eq!(apply("0123456789", spans), "01[redacted]8~");
    }

    #[test]
    fn a_span_inside_a_character_takes_the_whole_character() {
        let spans = vec![Span {
            start: 1,
            end: 2,
            kind: Kind::Secret,
        }];
        assert_eq!(apply("aéb", spans), "a[redacted]b");
    }

    #[test]
    fn secret_keys_are_recognised_in_any_spelling() {
        for key in [
            "GITHUB_TOKEN",
            "client-secret",
            "db.password",
            "apiKey",
            "X-Api-Key",
            "pass",
            "DB_PASS",
            "aws_secret_access_key",
        ] {
            assert!(is_secret_key(key.as_bytes()), "{key}");
        }
        for key in ["", "tokens", "bypass", "compass", "key", "PWD", "author"] {
            assert!(!is_secret_key(key.as_bytes()), "{key}");
        }
        assert!(!is_secret_key(&[b'a'; MAX_KEY_BYTES + 1]));
    }
}
