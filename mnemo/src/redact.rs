//! 12.2 step 4: redacting known secret shapes from text before it's stored
//! (`docs/design/12.2-onboard.md`, section 5). The hooks' transcript ingest
//! always runs it; `mnemo thread ingest --redact` turns it on for a direct
//! call.
//!
//! It's a best effort: it removes shapes it knows, each starting and ending
//! at a word boundary and with a minimum length taken from the vendor's real
//! format, so prose isn't redacted. Each match becomes `[redacted: <kind>]`.
//! The shapes:
//! - PEM private-key blocks, from the `BEGIN` line to the `END` line (a
//!   block split across texts loses each half: from `BEGIN` to the end of
//!   the text, and from the start of the text to `END`);
//! - AWS access key ids: `AKIA` or `ASIA` and exactly 16 of `[A-Z0-9]`;
//! - GitHub tokens: `ghp_`, `gho_`, `ghu_`, `ghs_` or `ghr_` and at least
//!   36 of `[A-Za-z0-9]`, or `github_pat_` and at least 22 of
//!   `[A-Za-z0-9_]`;
//! - Slack tokens: `xoxa-`, `xoxb-`, `xoxp-`, `xoxr-` or `xoxs-` and at
//!   least 10 of `[A-Za-z0-9-]`;
//! - `sk-` API keys (including `sk-ant-` and `sk-proj-`): at least 20 of
//!   `[A-Za-z0-9_-]` after `sk-`;
//! - JWTs: three base64url parts joined by dots, the first starting `eyJ`,
//!   each at least 10 characters;
//! - MNemo recovery codes (`MN1-…`), checked with the recovery-code parser;
//! - the value in `NAME=value`, `NAME: value` or `"name": "value"`, where the
//!   name ends in `TOKEN`, `SECRET`, `PASSWORD`, `PASSPHRASE` or `_KEY` (any
//!   case), when the value is at least 8 characters and isn't a plain word.
//!   A value isn't a plain word when it has a digit, has a character outside
//!   `[A-Za-z_.-]`, or mixes upper and lower case. A value starting with `$`
//!   or `<` is a reference or a placeholder, and stays.

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

/// Replace every known secret shape in `text` with `[redacted: <kind>]`.
pub fn secrets(text: &str) -> Redacted<'_> {
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    pem_blocks(text, &mut spans);
    tokens(text, &mut spans);
    named_values(text, &mut spans);
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

fn pem_blocks(text: &str, spans: &mut Vec<(usize, usize, &'static str)>) {
    let mut from = 0;
    let mut first_begin: Option<usize> = None;
    while let Some(i) = text[from..].find(PEM_BEGIN).map(|i| from + i) {
        if let Some(head_end) = pem_marker_end(text, i, PEM_BEGIN) {
            first_begin.get_or_insert(i);
            // To the matching END line, else to the end of the text (the
            // block's other half is in a later text).
            let end = find_end_marker(text, head_end).unwrap_or(text.len());
            spans.push((i, end, "private-key"));
            from = end;
        } else {
            from = i + PEM_BEGIN.len();
        }
    }
    // An END line with no BEGIN before it: the block began in an earlier
    // text, so everything up to it goes.
    let before = first_begin.unwrap_or(text.len());
    if let Some(end) = find_end_marker(&text[..before], 0) {
        spans.push((0, end, "private-key"));
    }
}

/// The index after the first private-key END marker at or after `from`.
fn find_end_marker(text: &str, from: usize) -> Option<usize> {
    let mut at = from;
    while let Some(i) = text[at..].find(PEM_END).map(|i| at + i) {
        if let Some(e) = pem_marker_end(text, i, PEM_END) {
            return Some(e);
        }
        at = i + PEM_END.len();
    }
    None
}

/// Whether the character before `i` leaves a word boundary.
fn boundary_before(text: &str, i: usize) -> bool {
    text[..i]
        .chars()
        .next_back()
        .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
}

/// How many bytes from `at` are in `alphabet`.
fn run(text: &str, at: usize, alphabet: impl Fn(u8) -> bool) -> usize {
    text.as_bytes()[at..]
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
fn tokens(text: &str, spans: &mut Vec<(usize, usize, &'static str)>) {
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

/// The token starting at `i`, if one does, and where it ends.
fn token_at(text: &str, i: usize) -> Option<(usize, &'static str)> {
    let rest = &text[i..];
    let starts = |p: &str| rest.len() >= p.len() && rest.as_bytes()[..p.len()] == *p.as_bytes();
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
    if starts("github_pat_") {
        let n = run(text, i + 11, |b| b.is_ascii_alphanumeric() || b == b'_');
        if n >= 22 {
            return Some((i + 11 + n, "github-token"));
        }
    }
    for p in ["xoxa-", "xoxb-", "xoxp-", "xoxr-", "xoxs-"] {
        if starts(p) {
            let n = run(text, i + 5, |b| b.is_ascii_alphanumeric() || b == b'-');
            if n >= 10 {
                return Some((i + 5 + n, "slack-token"));
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
    if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("MN1-") {
        let n = run(text, i, |b| b.is_ascii_alphanumeric() || b == b'-');
        let cand = text[i..i + n].trim_end_matches('-');
        if crate::keys::recovery::parse(cand).is_ok() {
            return Some((i + cand.len(), "recovery-code"));
        }
    }
    None
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

const NAME_SUFFIXES: [&str; 5] = ["token", "secret", "password", "passphrase", "_key"];

/// Values of names that look secret: `NAME=value`, `NAME: value`,
/// `"name": "value"`.
fn named_values(text: &str, spans: &mut Vec<(usize, usize, &'static str)>) {
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
        let lower = name.to_ascii_lowercase();
        if !NAME_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
            continue;
        }
        if let Some((s, e)) = value_after(text, after) {
            let v = &text[s..e];
            if !plain(v) {
                spans.push((s, e, "secret-value"));
                i = e;
            }
        }
    }
}

/// The value after a name ending at `at`: an optional closing quote, spaces,
/// `=` or `:`, spaces, then a quoted or bare value.
fn value_after(text: &str, at: usize) -> Option<(usize, usize)> {
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
            Some((s, s + len))
        }
        Some(_) => {
            let len = b[j..]
                .iter()
                .take_while(|&&c| {
                    !c.is_ascii_whitespace() && !matches!(c, b',' | b';' | b'}' | b')' | b'&')
                })
                .count();
            Some((j, j + len))
        }
        None => None,
    }
}

/// Whether a value is too short or too plain to be a secret.
fn plain(v: &str) -> bool {
    if v.len() < 8 || v.starts_with('$') || v.starts_with('<') {
        return true;
    }
    let has_digit = v.bytes().any(|c| c.is_ascii_digit());
    let other = v
        .bytes()
        .any(|c| !(c.is_ascii_alphabetic() || matches!(c, b'_' | b'.' | b'-')));
    let upper = v.bytes().any(|c| c.is_ascii_uppercase());
    let lower = v.bytes().any(|c| c.is_ascii_lowercase());
    !(has_digit || other || (upper && lower))
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

    /// None of the false cases is redacted.
    #[test]
    fn the_false_cases() {
        let cases = [
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
        for c in cases {
            let r = secrets(c);
            assert_eq!(r.text, c, "redacted a false case: {c}");
            assert!(r.counts.is_empty(), "{c}: {:?}", r.counts);
        }
    }

    /// A PEM block split across two texts (two turns): each half goes.
    #[test]
    fn a_pem_block_split_across_texts() {
        let a = format!("here it is\n-----BEGIN EC {PK}-----\nMHcCAQEEIOab");
        let b = format!("cdEFgh12\n-----END EC {PK}-----\nthat was it");
        let ra = secrets(&a);
        assert_eq!(ra.text, "here it is\n[redacted: private-key]");
        let rb = secrets(&b);
        assert_eq!(rb.text, "[redacted: private-key]\nthat was it");
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
