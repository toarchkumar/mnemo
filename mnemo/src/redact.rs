//! 12.2 step 4: redacting known secret shapes from text before it's stored
//! (`docs/design/12.2-onboard.md`, section 5). The hooks' transcript ingest
//! always runs it; `mnemo thread ingest --redact` turns it on for a direct
//! call.
//!
//! It's a best effort: it removes shapes it knows, each starting and ending
//! at a word boundary and with a minimum length taken from the vendor's real
//! format, so prose isn't redacted. Each match becomes `[redacted: <kind>]`.
//! It works on any text: every position it cuts at is next to an ASCII
//! byte, so it never splits a character. The shapes:
//! - PEM private-key blocks, from the `BEGIN` line to the `END` line. A
//!   marker with no partner (a block split across texts) takes the text up
//!   to the end, or from the start, only when key material is next to it (a
//!   `Proc-Type` header, or 40 or more base64 characters on the lines
//!   beside it); a marker only mentioned in prose loses the marker alone;
//! - AWS access key ids: `AKIA` or `ASIA` and exactly 16 of `[A-Z0-9]`;
//! - GitHub tokens: `ghp_`, `gho_`, `ghu_`, `ghs_` or `ghr_` and at least
//!   36 of `[A-Za-z0-9]`, or `github_pat_` and at least 22 of
//!   `[A-Za-z0-9_]`;
//! - Slack tokens: `xoxa-`, `xoxb-`, `xoxp-`, `xoxr-` or `xoxs-` and at
//!   least 10 of `[A-Za-z0-9-]`;
//! - API keys: `sk-` (including `sk-ant-` and `sk-proj-`) and at least 20
//!   of `[A-Za-z0-9_-]`; `sk_live_` or `rk_live_` and at least 20 of
//!   `[A-Za-z0-9]`; `AIza` and at least 35 of `[A-Za-z0-9_-]`; `glpat-`
//!   and at least 20 of `[A-Za-z0-9_-]`; `npm_` and at least 36 of
//!   `[A-Za-z0-9]`; `hf_` and at least 30 of `[A-Za-z0-9]`;
//! - JWTs: three base64url parts joined by dots, the first starting `eyJ`,
//!   each at least 10 characters;
//! - MNemo recovery codes (`MN1-…`), checked with the recovery-code parser,
//!   with hyphens, spaces or line breaks between the groups;
//! - the value in `NAME=value`, `NAME: value` or `"name": "value"`:
//!   - when the name ends in `PASSWORD` or `PASSPHRASE` (any case): any
//!     value of 8 or more characters, and an unquoted one runs to the end
//!     of the line;
//!   - when it ends in `TOKEN`, `SECRET`, `_KEY`, `-KEY` or `APIKEY`: a
//!     value of 8 or more characters that isn't a plain word (it has a
//!     digit or a character outside `[A-Za-z_.-]`, or it mixes upper and
//!     lower case and is 20 or more letters). Names whose last part before
//!     `_KEY` or `-KEY` is `cache`, `sort`, `partition`, `primary`,
//!     `foreign`, `unique`, `idempotency`, `dedup` or `row` are database
//!     words, and stay;
//!   - a value starting with `$`, `<`, `{{` or `%{` is a reference or a
//!     placeholder, and stays;
//! - the credential in `Authorization: <scheme> <credential>` (8 or more
//!   characters), and after `Bearer ` (20 or more);
//! - the word after `--passphrase` or `--password` (8 or more characters),
//!   and after `--token` or `--key` when it's 16 or more of
//!   `[A-Za-z0-9+/=_-]` and isn't a plain word (so a memory key like
//!   `design.v2` or a file path stays);
//! - the password in a URL's `scheme://user:password@`.

use std::borrow::Cow;

/// The kinds, in the order counts are reported.
pub const KINDS: [&str; 8] = [
    "private-key",
    "aws-key-id",
    "github-token",
    "slack-token",
    "api-key",
    "jwt",
    "recovery-code",
    "secret-value",
];

/// What [`secrets`] found and removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted<'a> {
    /// The text with every match replaced (borrowed when nothing matched).
    pub text: Cow<'a, str>,
    /// How many matches of each kind, in [`KINDS`] order, only those found.
    pub counts: Vec<(&'static str, usize)>,
}

impl Redacted<'_> {
    /// How many matches in all.
    pub fn total(&self) -> usize {
        self.counts.iter().map(|(_, n)| n).sum()
    }
}

/// Sum the counts of several redactions, in [`KINDS`] order.
pub fn add_counts(into: &mut Vec<(&'static str, usize)>, more: &[(&'static str, usize)]) {
    for (k, n) in more {
        match into.iter_mut().find(|(have, _)| have == k) {
            Some((_, m)) => *m += n,
            None => into.push((k, *n)),
        }
    }
    into.sort_by_key(|(k, _)| KINDS.iter().position(|x| x == k).unwrap_or(usize::MAX));
}

type Spans = Vec<(usize, usize, &'static str)>;

/// Replace every known secret shape in `text` with `[redacted: <kind>]`.
pub fn secrets(text: &str) -> Redacted<'_> {
    let mut spans: Spans = Vec::new();
    pem_blocks(text, &mut spans);
    tokens(text, &mut spans);
    named_values(text, &mut spans);
    flag_values(text, &mut spans);
    url_passwords(text, &mut spans);
    if spans.is_empty() {
        return Redacted {
            text: Cow::Borrowed(text),
            counts: Vec::new(),
        };
    }
    // Earliest first, the longest at a start; overlaps go to the first.
    spans.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut out = String::with_capacity(text.len());
    let mut counts = Vec::new();
    let mut at = 0;
    for (s, e, kind) in spans {
        if s < at {
            continue;
        }
        out.push_str(&text[at..s]);
        out.push_str("[redacted: ");
        out.push_str(kind);
        out.push(']');
        add_counts(&mut counts, &[(kind, 1)]);
        at = e;
    }
    out.push_str(&text[at..]);
    Redacted {
        text: Cow::Owned(out),
        counts,
    }
}

/// Whether `text` has `p` at byte `i`, ignoring ASCII case. Compares bytes,
/// so `i` may be anywhere.
fn has_at(text: &str, i: usize, p: &str) -> bool {
    text.as_bytes()
        .get(i..i + p.len())
        .is_some_and(|b| b.eq_ignore_ascii_case(p.as_bytes()))
}

/// Whether `text` has `p` at byte `i`, exactly.
fn has_exact(text: &str, i: usize, p: &str) -> bool {
    text.as_bytes().get(i..i + p.len()) == Some(p.as_bytes())
}

const PEM_BEGIN: &str = "-----BEGIN ";
const PEM_END: &str = "-----END ";
const PEM_TAIL: &str = "PRIVATE KEY-----";

/// The end of a PEM marker line starting at `at` (`-----BEGIN …` or
/// `-----END …`) when it names a private key: the index after its closing
/// `-----`.
fn pem_marker_end(text: &str, at: usize, lead: &str) -> Option<usize> {
    let rest = &text[at + lead.len()..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let line = &rest[..line_end];
    let i = line.find(PEM_TAIL)?;
    // Only letters and spaces between the lead and the tail ("RSA ", "EC ",
    // "OPENSSH ", "ENCRYPTED ", or nothing).
    if !line[..i]
        .chars()
        .all(|c| c.is_ascii_uppercase() || c == ' ')
    {
        return None;
    }
    Some(at + lead.len() + i + PEM_TAIL.len())
}

fn base64_line(line: &str) -> bool {
    !line.is_empty()
        && line
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// How much key material the lines give, in base64 characters, counting
/// whole base64 lines from the first until one isn't.
fn key_material<'a>(lines: impl Iterator<Item = &'a str>) -> usize {
    lines
        .map(|l| l.trim_end_matches('\r'))
        .take_while(|l| base64_line(l))
        .map(str::len)
        .sum()
}

/// Whether key material follows a BEGIN marker ending at `head_end`.
fn material_after(text: &str, head_end: usize) -> bool {
    let rest = &text[head_end..];
    let Some(nl) = rest.find('\n') else {
        return false;
    };
    let body = &rest[nl + 1..];
    body.starts_with("Proc-Type:") || key_material(body.split('\n')) >= 40
}

/// Whether key material comes just before an END marker at `at`.
fn material_before(text: &str, at: usize) -> bool {
    let Some(head) = text[..at].strip_suffix('\n') else {
        return false;
    };
    key_material(head.rsplit('\n')) >= 40
}

fn pem_blocks(text: &str, spans: &mut Spans) {
    let mut from = 0;
    let mut first_begin: Option<usize> = None;
    while let Some(i) = text[from..].find(PEM_BEGIN).map(|i| from + i) {
        if let Some(head_end) = pem_marker_end(text, i, PEM_BEGIN) {
            first_begin.get_or_insert(i);
            match find_end_marker(text, head_end) {
                Some((_, end)) => {
                    spans.push((i, end, "private-key"));
                    from = end;
                }
                // No END: the block's other half is in a later text when key
                // material follows; otherwise the marker is only mentioned.
                None if material_after(text, head_end) => {
                    spans.push((i, text.len(), "private-key"));
                    from = text.len();
                }
                None => {
                    spans.push((i, head_end, "private-key"));
                    from = head_end;
                }
            }
        } else {
            from = i + PEM_BEGIN.len();
        }
    }
    // END lines with no BEGIN before them: the first one's block began in an
    // earlier text when key material comes before it.
    let before = first_begin.unwrap_or(text.len());
    let mut at = 0;
    let mut first = true;
    while let Some((s, e)) = find_end_marker(&text[..before], at) {
        let start = if first && material_before(text, s) {
            0
        } else {
            s
        };
        spans.push((start, e, "private-key"));
        first = false;
        at = e;
    }
}

/// The first private-key END marker at or after `from`: its start, and the
/// index after it.
fn find_end_marker(text: &str, from: usize) -> Option<(usize, usize)> {
    let mut at = from;
    while let Some(i) = text[at..].find(PEM_END).map(|i| at + i) {
        if let Some(e) = pem_marker_end(text, i, PEM_END) {
            return Some((i, e));
        }
        at = i + PEM_END.len();
    }
    None
}

/// Whether the byte before `i` leaves a word boundary (any byte that isn't
/// an ASCII letter, digit or `_`, including part of a wider character).
fn boundary_before(text: &str, i: usize) -> bool {
    i == 0 || {
        let c = text.as_bytes()[i - 1];
        !(c.is_ascii_alphanumeric() || c == b'_')
    }
}

/// How many bytes from `at` are in `alphabet`.
fn run(text: &str, at: usize, alphabet: impl Fn(u8) -> bool) -> usize {
    text.as_bytes()
        .get(at..)
        .unwrap_or_default()
        .iter()
        .take_while(|b| alphabet(**b))
        .count()
}

fn alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn upper_digit(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit()
}

fn word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Prefix tokens: `(start, end, kind)` for each match at a word boundary.
fn tokens(text: &str, spans: &mut Spans) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_alphabetic() || !boundary_before(text, i) {
            i += 1;
            continue;
        }
        if let Some((end, kind)) = token_at(text, i) {
            spans.push((i, end, kind));
            i = end;
        } else {
            i += 1;
        }
    }
}

/// Prefixes followed by at least `min` bytes of an alphabet, and the kind.
type Prefix = (&'static str, usize, fn(u8) -> bool, &'static str);

const PREFIXES: [Prefix; 12] = [
    (
        "github_pat_",
        22,
        |b| b.is_ascii_alphanumeric() || b == b'_',
        "github-token",
    ),
    (
        "xoxa-",
        10,
        |b| b.is_ascii_alphanumeric() || b == b'-',
        "slack-token",
    ),
    (
        "xoxb-",
        10,
        |b| b.is_ascii_alphanumeric() || b == b'-',
        "slack-token",
    ),
    (
        "xoxp-",
        10,
        |b| b.is_ascii_alphanumeric() || b == b'-',
        "slack-token",
    ),
    (
        "xoxr-",
        10,
        |b| b.is_ascii_alphanumeric() || b == b'-',
        "slack-token",
    ),
    (
        "xoxs-",
        10,
        |b| b.is_ascii_alphanumeric() || b == b'-',
        "slack-token",
    ),
    ("sk_live_", 20, alnum, "api-key"),
    ("rk_live_", 20, alnum, "api-key"),
    ("AIza", 35, word, "api-key"),
    ("glpat-", 20, word, "api-key"),
    ("npm_", 36, alnum, "api-key"),
    ("hf_", 30, alnum, "api-key"),
];

/// The token starting at `i`, if one does, and where it ends.
fn token_at(text: &str, i: usize) -> Option<(usize, &'static str)> {
    let starts = |p: &str| has_exact(text, i, p);
    // AWS: exactly 16 after the prefix, then not another of the alphabet.
    for p in ["AKIA", "ASIA"] {
        if starts(p) {
            let n = run(text, i + 4, upper_digit);
            if n == 16 && run(text, i + 4 + n, alnum) == 0 {
                return Some((i + 4 + n, "aws-key-id"));
            }
        }
    }
    for p in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
        if starts(p) {
            let n = run(text, i + 4, alnum);
            if n >= 36 && run(text, i + 4 + n, |b| b == b'_') == 0 {
                return Some((i + 4 + n, "github-token"));
            }
        }
    }
    for (p, min, alphabet, kind) in PREFIXES {
        if starts(p) {
            let n = run(text, i + p.len(), alphabet);
            if n >= min {
                return Some((i + p.len() + n, kind));
            }
        }
    }
    if starts("sk-") {
        let n = run(text, i + 3, word);
        if n >= 20 {
            return Some((i + 3 + n, "api-key"));
        }
    }
    if starts("eyJ") {
        if let Some(end) = jwt_end(text, i) {
            return Some((end, "jwt"));
        }
    }
    if let Some(end) = recovery_code_end(text, i) {
        return Some((end, "recovery-code"));
    }
    None
}

/// Bytes of an HTTP credential (RFC 9110's token68, and `=` padding).
fn token68(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/' | b'=')
}

/// The end of a recovery code at `i`: `MN1` and 65 symbols, with up to 8
/// separators (hyphens, spaces, line breaks) between symbols, when the
/// parser accepts it.
fn recovery_code_end(text: &str, i: usize) -> Option<usize> {
    let b = text.as_bytes();
    if !has_at(text, i, "MN1")
        || !b
            .get(i + 3)
            .is_some_and(|c| *c == b'-' || c.is_ascii_whitespace())
    {
        return None;
    }
    let sep = |c: u8| c == b'-' || c.is_ascii_whitespace();
    let mut at = i + 3;
    let mut symbols = 0;
    while symbols < 65 {
        let gap = run(text, at, sep);
        if gap > 8 || !b.get(at + gap).is_some_and(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        at += gap + 1;
        symbols += 1;
    }
    crate::keys::recovery::parse(&text[i..at]).ok().map(|_| at)
}

/// The end of a JWT starting at `i` (three base64url parts of 10 or more,
/// joined by dots).
fn jwt_end(text: &str, i: usize) -> Option<usize> {
    let b64 = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let mut at = i;
    for part in 0..3 {
        let n = run(text, at, b64);
        if n < 10 {
            return None;
        }
        at += n;
        if part < 2 {
            if text.as_bytes().get(at) != Some(&b'.') {
                return None;
            }
            at += 1;
        }
    }
    Some(at)
}

/// Names whose values are secret at any length of 8 or more.
const PASSWORD_SUFFIXES: [&str; 2] = ["password", "passphrase"];
/// Names whose values are secret unless they're plain words.
const NAME_SUFFIXES: [&str; 5] = ["token", "secret", "_key", "-key", "apikey"];
/// The last part of a `…_key` name that makes it a database or cache word.
const KEY_STEMS: [&str; 9] = [
    "cache",
    "sort",
    "partition",
    "primary",
    "foreign",
    "unique",
    "idempotency",
    "dedup",
    "row",
];

/// How a name's value is judged.
#[derive(Clone, Copy, PartialEq)]
enum Name {
    Password,
    Secret,
}

fn name_kind(name: &str) -> Option<Name> {
    let lower = name.to_ascii_lowercase();
    if PASSWORD_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
        return Some(Name::Password);
    }
    let s = NAME_SUFFIXES.iter().find(|s| lower.ends_with(**s))?;
    if s.ends_with("key") && s.len() == 4 {
        let stem = &lower[..lower.len() - 4];
        let last = stem.rsplit(['_', '-']).next().unwrap_or("");
        if KEY_STEMS.contains(&last) {
            return None;
        }
    }
    Some(Name::Secret)
}

/// Values of names that look secret: `NAME=value`, `NAME: value`,
/// `"name": "value"`; and `Authorization: <scheme> <credential>`.
fn named_values(text: &str, spans: &mut Spans) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let is_name = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
        if !is_name(bytes[i]) || !boundary_before(text, i) {
            i += 1;
            continue;
        }
        let n = run(text, i, is_name);
        let name = &text[i..i + n];
        let after = i + n;
        i = after.max(i + 1);
        if name.eq_ignore_ascii_case("authorization") {
            if let Some((s, e)) = authorization_value(text, after) {
                spans.push((s, e, "secret-value"));
                i = e;
            }
            continue;
        }
        // `Bearer <credential>`: the credential goes, the word stays.
        if name.eq_ignore_ascii_case("bearer") && bytes.get(after) == Some(&b' ') {
            let s = after + run(text, after, |c| c == b' ');
            let n = run(text, s, token68);
            if n >= 20 && !has_exact(text, s, "eyJ") {
                spans.push((s, s + n, "secret-value"));
                i = s + n;
            }
            continue;
        }
        // A flag's value is `flag_values`'s (a memory key after `--key=`
        // isn't a secret).
        if name.starts_with('-') {
            continue;
        }
        let Some(kind) = name_kind(name) else {
            continue;
        };
        if let Some((s, e, quoted)) = value_after(text, after) {
            let e = if kind == Name::Password && !quoted {
                line_end(text, s)
            } else {
                e
            };
            let v = &text[s..e];
            let secret = match kind {
                Name::Password => v.len() >= 8 && !reference(v),
                Name::Secret => !plain(v),
            };
            if secret {
                spans.push((s, e, "secret-value"));
                i = e;
            }
        }
    }
}

/// The end of the line at `s`, without trailing spaces or `\r`.
fn line_end(text: &str, s: usize) -> usize {
    let line = &text[s..];
    let n = line.find('\n').unwrap_or(line.len());
    s + line[..n].trim_end().len()
}

/// The credential after `Authorization`, ending at `at`: `: <scheme>
/// <credential>` (or `=`, or quoted), when the credential is 8 or more.
fn authorization_value(text: &str, at: usize) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let (s, _, _) = value_after(text, at)?;
    let scheme = run(text, s, |c| c.is_ascii_alphabetic());
    if scheme == 0 || b.get(s + scheme) != Some(&b' ') {
        return None;
    }
    let c = s + scheme + run(text, s + scheme, |c| c == b' ');
    let n = run(text, c, token68);
    (n >= 8 && !has_exact(text, c, "eyJ")).then_some((c, c + n))
}

/// The value after a name ending at `at`: an optional closing quote, spaces,
/// `=` or `:`, spaces, then a quoted or bare value. The third field says
/// whether it was quoted.
fn value_after(text: &str, at: usize) -> Option<(usize, usize, bool)> {
    let b = text.as_bytes();
    let mut j = at;
    if b.get(j) == Some(&b'"') || b.get(j) == Some(&b'\'') {
        j += 1;
    }
    while b.get(j) == Some(&b' ') {
        j += 1;
    }
    if b.get(j) != Some(&b'=') && b.get(j) != Some(&b':') {
        return None;
    }
    j += 1;
    while b.get(j) == Some(&b' ') {
        j += 1;
    }
    match b.get(j) {
        Some(&q) if q == b'"' || q == b'\'' => {
            let s = j + 1;
            let len = b[s..].iter().take_while(|&&c| c != q && c != b'\n').count();
            Some((s, s + len, true))
        }
        Some(_) => {
            let len = b[j..]
                .iter()
                .take_while(|&&c| {
                    !c.is_ascii_whitespace() && !matches!(c, b',' | b';' | b'}' | b')' | b'&')
                })
                .count();
            Some((j, j + len, false))
        }
        None => None,
    }
}

/// Whether a value is a reference or a placeholder (`$X`, `${x}`, `<x>`,
/// `{{ x }}`, `%{x}`).
fn reference(v: &str) -> bool {
    v.starts_with('$') || v.starts_with('<') || v.starts_with("{{") || v.starts_with("%{")
}

/// Whether a value is too short or too plain to be a secret.
fn plain(v: &str) -> bool {
    if v.len() < 8 || reference(v) {
        return true;
    }
    let has_digit = v.bytes().any(|c| c.is_ascii_digit());
    let other = v
        .bytes()
        .any(|c| !(c.is_ascii_alphabetic() || matches!(c, b'_' | b'.' | b'-')));
    let upper = v.bytes().any(|c| c.is_ascii_uppercase());
    let lower = v.bytes().any(|c| c.is_ascii_lowercase());
    // Mixed case alone counts only at a length an identifier rarely has
    // (`useEnvironment` stays).
    !(has_digit || other || (upper && lower && v.len() >= 20))
}

/// The word after `--passphrase`, `--password`, `--token` or `--key`.
fn flag_values(text: &str, spans: &mut Spans) {
    let b = text.as_bytes();
    let mut from = 0;
    while let Some(i) = text[from..].find("--").map(|i| from + i) {
        from = i + 2;
        if !boundary_before(text, i) || (i > 0 && b[i - 1] == b'-') {
            continue;
        }
        let n = run(text, i + 2, |c| c.is_ascii_alphabetic());
        let flag = &text[i + 2..i + 2 + n];
        let password = ["passphrase", "password"].contains(&flag);
        if !password && flag != "token" && flag != "key" {
            continue;
        }
        let mut s = i + 2 + n;
        match b.get(s) {
            Some(b'=') => s += 1,
            Some(b' ') => s += run(text, s, |c| c == b' '),
            _ => continue,
        }
        let (s, e) = match b.get(s) {
            Some(&q) if q == b'"' || q == b'\'' => {
                let len = run(text, s + 1, |c| c != q && c != b'\n');
                (s + 1, s + 1 + len)
            }
            _ => (s, s + run(text, s, |c| !c.is_ascii_whitespace())),
        };
        let v = &text[s..e];
        let secret = if password {
            v.len() >= 8 && !reference(v) && !v.starts_with('-')
        } else {
            v.len() >= 16
                && v.bytes().all(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'_' | b'-')
                })
                && !plain(v)
        };
        if secret {
            spans.push((s, e, "secret-value"));
            from = e;
        }
    }
}

/// The password in `scheme://user:password@host`.
fn url_passwords(text: &str, spans: &mut Spans) {
    let b = text.as_bytes();
    let mut from = 0;
    while let Some(i) = text[from..].find("://").map(|i| from + i) {
        from = i + 3;
        if i == 0 || !b[i - 1].is_ascii_alphanumeric() {
            continue;
        }
        let s = i + 3;
        let n = run(text, s, |c| {
            !c.is_ascii_whitespace()
                && !matches!(c, b'/' | b'?' | b'#' | b'@' | b'"' | b'\'' | b'<' | b'>')
        });
        if b.get(s + n) != Some(&b'@') {
            continue;
        }
        let Some(colon) = text[s..s + n].find(':') else {
            continue;
        };
        let (ps, pe) = (s + colon + 1, s + n);
        if pe > ps && !reference(&text[ps..pe]) {
            spans.push((ps, pe, "secret-value"));
            from = pe;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every secret-shaped test value is built at run time from parts, so
    // no file holds one a secret scanner would flag (GitHub's flagged an
    // `ASIA…` literal here once). AWS's documented example id stays.
    const PK: &str = "PRIVATE KEY";

    fn aws(prefix: &str) -> String {
        format!("{prefix}{}", "ABCDEFGHIJKLMNOP")
    }

    fn kinds(text: &str) -> Vec<&'static str> {
        secrets(text).counts.iter().map(|(k, _)| *k).collect()
    }

    /// Each true case is redacted, with its kind, and its secret part gone.
    #[test]
    fn the_true_cases() {
        let key = crate::keys::recovery::encode(&[7u8; 32]);
        let cases: Vec<(String, &str, &str)> = vec![
            (
                "id AKIAIOSFODNN7EXAMPLE here".into(),
                "aws-key-id",
                "AKIAIOSFODNN7EXAMPLE",
            ),
            (
                format!("{}.", aws(concat!("AS", "IA"))),
                "aws-key-id",
                "ABCDEFGHIJKLMNOP",
            ),
            (
                format!("token ghp_{}", "a1B2".repeat(9)),
                "github-token",
                "ghp_a1B2",
            ),
            (
                format!("gho_{} end", "Z9".repeat(18)),
                "github-token",
                "gho_Z9",
            ),
            (
                format!("github_pat_{}", "11ABC_def".repeat(3)),
                "github-token",
                "github_pat_11",
            ),
            (
                format!("{}-1234567890-abcdefghij", concat!("xo", "xb")),
                "slack-token",
                "1234567890-abcd",
            ),
            (
                format!("key {}-{}", concat!("s", "k"), "abcdefghijklmnopqrstuvwx"),
                "api-key",
                "abcdefghijklm",
            ),
            (
                format!("sk-ant-api03-{}", "Ab9_".repeat(10)),
                "api-key",
                "sk-ant-api03",
            ),
            (
                format!("sk-proj-{}", "x".repeat(30)),
                "api-key",
                "sk-proj-xxx",
            ),
            (
                format!(
                    "Bearer {}.{}.{}",
                    concat!("ey", "JhbGciOiJIUzI1NiJ9"),
                    concat!("ey", "JzdWIiOiIxMjM0NTY3ODkwIn0"),
                    "dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
                ),
                "jwt",
                "eyJhbGciOiJIUzI1NiJ9",
            ),
            (
                format!("the code is {}", key.as_str()),
                "recovery-code",
                &key.as_str()[4..14],
            ),
            (key.to_lowercase(), "recovery-code", "mn1-"),
            ("API_KEY=a8f3k2m9q7".into(), "secret-value", "a8f3k2m9q7"),
            (
                "export GITHUB_TOKEN=Hunter2Hunter".into(),
                "secret-value",
                "Hunter2Hunter",
            ),
            (
                "{\"password\": \"correct horse 99\"}".into(),
                "secret-value",
                "correct horse 99",
            ),
            (
                "db_password: CorrectHorse".into(),
                "secret-value",
                "CorrectHorse",
            ),
            (
                "MNEMO_KEY=4f2a9c0e1b3d5f7a".into(),
                "secret-value",
                "4f2a9c0e1b3d5f7a",
            ),
            (
                format!(
                    "-----BEGIN RSA {PK}-----\nMIIEpAIBAAKCAQEA\n-----END RSA {PK}-----\nafter"
                ),
                "private-key",
                "MIIEpAIBAAKCAQEA",
            ),
            (
                format!(
                    "-----BEGIN OPENSSH {PK}-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH {PK}-----"
                ),
                "private-key",
                "b3BlbnNzaC1rZXk",
            ),
        ];
        for (text, kind, gone) in &cases {
            let r = secrets(text);
            assert_eq!(kinds(text), [*kind], "{text}");
            assert!(!r.text.contains(gone), "{text} -> {}", r.text);
            assert!(
                r.text.contains(&format!("[redacted: {kind}]")),
                "{}",
                r.text
            );
        }
        // The text around a match stays.
        assert_eq!(
            secrets("id AKIAIOSFODNN7EXAMPLE here").text,
            "id [redacted: aws-key-id] here"
        );
        assert_eq!(
            secrets("API_KEY=a8f3k2m9q7 ok").text,
            "API_KEY=[redacted: secret-value] ok"
        );
        let pem = format!("-----BEGIN RSA {PK}-----\nx\n-----END RSA {PK}-----\nafter");
        assert!(secrets(&pem).text.ends_with("\nafter"));
    }

    const FALSE_CASES: [&str; 23] = [
        "task-list",
        "sk-1",
        "the risk-assessment is done",
        concat!("as", "k-for-help-with-this-long-name-here"),
        "PRIMARY_KEY=id",
        "SORT_KEY=created_at",
        "password: hunter",
        "the password field is required",
        "commit 9fceb02d0ae598e95dc970b74767f19372d61af8",
        "ULID 01M7F1G7YYRRRDBS43Y36C1QHR",
        "uuid 123e4567-e89b-12d3-a456-426614174000",
        "a ghp_ token goes here",
        "eyJ alone",
        "AKIA plus some words",
        "AKIAIOSFODNN7EXAMPLEX is too long",
        "MN1-0000-not-a-code",
        "GITHUB_TOKEN=${{ secrets.GITHUB_TOKEN }}",
        "API_KEY=<your key>",
        "MNEMO_KEY_FILE=/home/me/brain.key",
        "-----BEGIN PUBLIC KEY-----\nMFkwEwYH\n-----END PUBLIC KEY-----",
        "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
        "xoxb-short",
        "sk-learn is a library",
    ];

    /// None of the false cases is redacted.
    #[test]
    fn the_false_cases() {
        for c in FALSE_CASES {
            let r = secrets(c);
            assert_eq!(r.text, c, "redacted a false case: {c}");
            assert!(r.counts.is_empty(), "{c}: {:?}", r.counts);
        }
    }

    /// A line of key material, as a PEM body has (64 base64 characters).
    fn b64_line(seed: &str) -> String {
        seed.repeat(64).chars().take(64).collect()
    }

    /// A PEM block split across two texts (two turns): each half goes.
    #[test]
    fn a_pem_block_split_across_texts() {
        let a = format!(
            "here it is\n-----BEGIN EC {PK}-----\n{}\nMHcCAQEEIOab",
            b64_line("MHcCAQEEIOab")
        );
        let b = format!(
            "{}\ncdEFgh12\n-----END EC {PK}-----\nthat was it",
            b64_line("cdEFgh12")
        );
        let ra = secrets(&a);
        assert_eq!(ra.text, "here it is\n[redacted: private-key]");
        let rb = secrets(&b);
        assert_eq!(rb.text, "[redacted: private-key]\nthat was it");
        // An encrypted key's header counts as key material.
        let c = format!("x\n-----BEGIN RSA {PK}-----\nProc-Type: 4,ENCRYPTED\nDEK");
        assert_eq!(secrets(&c).text, "x\n[redacted: private-key]");
    }

    /// A marker only mentioned in prose loses the marker, not the turn
    /// (RB-011.4).
    #[test]
    fn a_pem_marker_in_prose_takes_only_the_marker() {
        let begin = format!(
            "Look for the line `-----BEGIN {PK}-----` in the file. Then tell me what \
             follows, because the rest of this turn matters a lot."
        );
        assert_eq!(
            secrets(&begin).text,
            "Look for the line `[redacted: private-key]` in the file. Then tell me what \
             follows, because the rest of this turn matters a lot."
        );
        let end = format!(
            "A long explanation of the format comes first and it matters. The block \
             closes with `-----END RSA {PK}-----` and that is all."
        );
        assert_eq!(
            secrets(&end).text,
            "A long explanation of the format comes first and it matters. The block \
             closes with `[redacted: private-key]` and that is all."
        );
        // A short line after BEGIN isn't key material either.
        let short = format!("-----BEGIN {PK}-----\nwhere does it go?");
        assert_eq!(
            secrets(&short).text,
            "[redacted: private-key]\nwhere does it go?"
        );
    }

    /// The shapes added for RB-011.2 (mnemo's own secrets first, then
    /// common ones), each with its secret part gone.
    #[test]
    fn the_added_true_cases() {
        let key = crate::keys::recovery::encode(&[9u8; 32]);
        let spaced = key.replace('-', " ");
        let groups: Vec<&str> = key.split('-').collect();
        let wrapped = format!("{}\n{}", groups[..7].join("-"), groups[7..].join("-"));
        let cases: Vec<(String, &str, &str)> = vec![
            (
                "export MNEMO_PASSPHRASE=correcthorsebattery".into(),
                "secret-value",
                "correcthorse",
            ),
            (
                "MNEMO_PASSPHRASE=correct horse battery staple\nnext".into(),
                "secret-value",
                "horse battery",
            ),
            (
                "mnemo init --passphrase Tr0ub4dor&3xyz".into(),
                "secret-value",
                "Tr0ub4dor",
            ),
            (
                "tool --password=\"long enough words\"".into(),
                "secret-value",
                "enough",
            ),
            (
                format!("gh --token {}", "a1B2c3D4".repeat(3)),
                "secret-value",
                "a1B2c3D4",
            ),
            (format!("code: {spaced}"), "recovery-code", groups[3]),
            (format!("code:\n{wrapped}"), "recovery-code", groups[9]),
            (
                "Authorization: Bearer abcDEF123456ghiJKL789".into(),
                "secret-value",
                "abcDEF123456",
            ),
            (
                "\"Authorization\": \"Basic dXNlcjpwYXNzd29yZA==\"".into(),
                "secret-value",
                "dXNlcjpw",
            ),
            (
                "curl -H 'bearer abcDEF123456ghiJKL789xyz'".into(),
                "secret-value",
                "abcDEF123456",
            ),
            (
                "X-Api-Key: abcDEF123456ghiJKL789".into(),
                "secret-value",
                "abcDEF123456",
            ),
            (
                "{\"apiKey\": \"abcDEF123456ghiJKL789\"}".into(),
                "secret-value",
                "abcDEF123456",
            ),
            (
                format!("{}_live_{}", concat!("s", "k"), "a1B2".repeat(6)),
                "api-key",
                "a1B2a1B2",
            ),
            (
                format!("{}_live_{}", concat!("r", "k"), "Z9y8".repeat(6)),
                "api-key",
                "Z9y8Z9y8",
            ),
            (
                format!("key={}{}", concat!("AI", "za"), "Sy_1-".repeat(7)),
                "api-key",
                "Sy_1-Sy_1",
            ),
            (
                format!("{}-{}", concat!("gl", "pat"), "x1Y2z3".repeat(4)),
                "api-key",
                "x1Y2z3x1",
            ),
            (
                format!("{}_{}", concat!("n", "pm"), "q7".repeat(18)),
                "api-key",
                "q7q7q7",
            ),
            (
                format!("{}_{}", concat!("h", "f"), "AbCd".repeat(8)),
                "api-key",
                "AbCdAbCd",
            ),
            (
                "postgres://admin:S3cr3tPassw0rd@db.example.com/app".into(),
                "secret-value",
                "S3cr3t",
            ),
        ];
        for (text, kind, gone) in &cases {
            let r = secrets(text);
            assert_eq!(kinds(text), [*kind], "{text} -> {}", r.text);
            assert!(!r.text.contains(gone), "{text} -> {}", r.text);
        }
        // Only the secret part goes.
        assert_eq!(
            secrets("Authorization: Bearer abcDEF123456ghiJKL789").text,
            "Authorization: Bearer [redacted: secret-value]"
        );
        assert_eq!(
            secrets("postgres://admin:S3cr3tPassw0rd@db.example.com/app").text,
            "postgres://admin:[redacted: secret-value]@db.example.com/app"
        );
        assert_eq!(
            secrets("mnemo init --passphrase Tr0ub4dor&3xyz now").text,
            "mnemo init --passphrase [redacted: secret-value] now"
        );
    }

    /// The false cases found in RB-011.4, and a near miss beside each shape
    /// added for RB-011.2.
    #[test]
    fn the_added_false_cases() {
        let cases = [
            "cache_key: 3f9a2b7c",
            "SORT_KEY=created_at/2024",
            "client_secret: useEnvironment",
            "password: \"{{ vault_db_password }}\"",
            "PASSWORD=%{db_password}",
            "partition_key: tenant_42",
            "row-key: r1234567",
            "redis_cache_key=user:42:profile",
            "MNEMO_PASSPHRASE: set",
            "the passphrase is set",
            "--passphrase $PP",
            "--password-stdin < file",
            "--keychain on",
            "use the --token flag",
            "remember --key design.122-post-release-order",
            "curl --key ~/.ssh/id_ed25519 --cert c.pem",
            "Authorization: required",
            "the authorization: see below",
            "Bearer token",
            "AIza short",
            concat!("gl", "pat-short"),
            "npm_install",
            "hf_hub_download",
            concat!("s", "k_live_short"),
            "https://example.com:8080/path",
            "ssh://git@github.com/owner/repo",
            "postgres://user:${DB_PASS}@host/db",
            "MN1 is the code's prefix",
        ];
        for c in cases {
            let r = secrets(c);
            assert_eq!(r.text, c, "redacted a false case: {c}");
        }
    }

    /// Text that isn't ASCII, around the cases and on its own (RB-011.1:
    /// these panicked).
    #[test]
    fn text_that_isnt_ascii() {
        for t in [
            "it’s done",
            "don’t do that",
            "the café is open",
            "so—yes",
            "a😀",
            "ok日本語",
            "run it ok→ done",
            "I’m fine",
            "MN😀",
            "mn1-é",
            "sk-é",
            "eyJé",
            "postgres://ü:é@x",
            "--key é",
            "Bearer 日本語",
            "-----BEGIN PRIVATE KEY-----\né",
        ] {
            let _ = secrets(t);
        }
        // The suffix starts a line: a password's unquoted value runs to the end
        // of its line.
        let around = |t: &str| format!("it’s — café 日本 😀 {t}\n— ok’s 🎉");
        for t in ["it’s done", "the café is open", "so—yes", "a😀", "ok日本語"] {
            assert_eq!(secrets(t).text, t);
        }
        let hit = "id AKIAIOSFODNN7EXAMPLE here";
        assert_eq!(
            secrets(&around(hit)).text,
            around("id [redacted: aws-key-id] here")
        );
        // The false tables, wrapped, stay as they are; the true ones are
        // still found.
        for c in FALSE_CASES {
            let w = around(c);
            assert_eq!(secrets(&w).text, w, "{c}");
        }
        for c in ["API_KEY=a8f3k2m9q7", "db_password: CorrectHorse"] {
            let w = around(c);
            assert_eq!(secrets(&w).counts, [("secret-value", 1)], "{w}");
        }
    }

    proptest::proptest! {
        /// No input panics, and the output differs from the input exactly
        /// when something was counted.
        #[test]
        fn never_panics(parts in proptest::collection::vec(proptest::prop_oneof![
            proptest::prelude::any::<String>(),
            "[A-Za-z0-9]{0,40}",
            proptest::strategy::Just("MN1-".to_string()),
            proptest::strategy::Just("-----BEGIN PRIVATE KEY-----\n".to_string()),
            proptest::strategy::Just("-----END RSA PRIVATE KEY-----".to_string()),
            proptest::strategy::Just("password: ".to_string()),
            proptest::strategy::Just("Authorization: Basic ".to_string()),
            proptest::strategy::Just("--key ".to_string()),
            proptest::strategy::Just("://u:".to_string()),
            proptest::strategy::Just("@".to_string()),
            proptest::strategy::Just("Bearer ".to_string()),
            proptest::strategy::Just("’é😀→日".to_string()),
            proptest::strategy::Just("sk-".to_string()),
            proptest::strategy::Just("eyJ".to_string()),
            proptest::strategy::Just(" \n=:\"".to_string()),
        ], 0..24)) {
            let text = parts.concat();
            let r = secrets(&text);
            proptest::prop_assert_eq!(r.text == text, r.counts.is_empty());
        }

        /// Words, spaces and punctuation outside ASCII are never redacted.
        #[test]
        fn prose_stays(text in "([a-z]{1,9}|[ ’—–é日本😀→“”,.!?]){0,60}") {
            proptest::prop_assert_eq!(secrets(&text).text, text.as_str());
        }
    }

    #[test]
    fn counts_add_up_in_kind_order() {
        let t = format!(
            "AKIAIOSFODNN7EXAMPLE and ghp_{} and {}",
            "a".repeat(36),
            aws(concat!("AK", "IA"))
        );
        let r = secrets(&t);
        assert_eq!(r.counts, [("aws-key-id", 2), ("github-token", 1)]);
        assert_eq!(r.total(), 3);
        let mut all = vec![("github-token", 1)];
        add_counts(&mut all, &[("aws-key-id", 2), ("github-token", 1)]);
        assert_eq!(all, [("aws-key-id", 2), ("github-token", 2)]);
        // Nothing found borrows.
        assert!(matches!(secrets("plain text").text, Cow::Borrowed(_)));
    }
}
