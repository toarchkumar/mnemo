//! 12.3 step 5 (v0.15): the identifier rule of the built-in embedder's
//! identifier-aware recall (PLANNER-A, 2026-11-14; measured in step 5
//! part 3b).
//!
//! A recall by text with the built-in embedder scores no keywords for the
//! question's plain words: arctic-xs's vector does better on its own there.
//! The tokens shaped like identifiers become explicit keywords instead, at
//! the explicit-keyword weight, because a vector blurs an exact name.
//!
//! A token is the query split on whitespace, with `` ` ' " , . ; : ? ! ( )
//! [ ] { } `` stripped from each end. It's an identifier if it was inside
//! backticks in the query, or if it:
//! - contains `::`, `@` or `=`;
//! - starts with `--`;
//! - has `_` between two letters or digits;
//! - is camelCase or PascalCase with an inner capital (`[a-z][A-Z]` or
//!   `[A-Z][a-z]+[A-Z]`);
//! - is a `name(…)` call (`(` after a letter, before stripping);
//! - is a dotted name (`[A-Za-z_]+\.[A-Za-z_]+`, so `store.rs` too).
//!
//! The token right after a `--flag` is its value, and counts if it's an
//! identifier itself or holds `,`, `=`, `.` or `/`, so `--features
//! failpoints,benches` takes both and `--model-dir for this` takes only the
//! flag. Plain words, numbers and versions like `1.92` are not identifiers.

// Used only by the built-in embedder; compiled (and tested) in every build.
#![cfg_attr(not(feature = "embed-candle"), allow(dead_code))]

const STRIP: &[char] = &[
    '`', '\'', '"', ',', '.', ';', ':', '?', '!', '(', ')', '[', ']', '{', '}',
];

fn has_between(t: &str, mid: char) -> bool {
    let c: Vec<char> = t.chars().collect();
    (1..c.len().saturating_sub(1))
        .any(|i| c[i] == mid && c[i - 1].is_alphanumeric() && c[i + 1].is_alphanumeric())
}

fn camel(t: &str) -> bool {
    let c: Vec<char> = t.chars().collect();
    // [a-z][A-Z]
    if c.windows(2)
        .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
    {
        return true;
    }
    // [A-Z][a-z]+[A-Z]
    for i in 0..c.len() {
        if c[i].is_ascii_uppercase() {
            let mut j = i + 1;
            while j < c.len() && c[j].is_ascii_lowercase() {
                j += 1;
            }
            if j > i + 1 && j < c.len() && c[j].is_ascii_uppercase() {
                return true;
            }
        }
    }
    false
}

fn dotted(t: &str) -> bool {
    let c: Vec<char> = t.chars().collect();
    let word = |ch: char| ch.is_ascii_alphabetic() || ch == '_';
    (1..c.len().saturating_sub(1)).any(|i| c[i] == '.' && word(c[i - 1]) && word(c[i + 1]))
}

fn call(raw: &str) -> bool {
    let c: Vec<char> = raw.chars().collect();
    (1..c.len()).any(|i| c[i] == '(' && c[i - 1].is_alphabetic())
}

/// Whether a stripped token (with its raw form) is an identifier by shape.
fn shaped(raw: &str, t: &str) -> bool {
    t.contains("::")
        || t.contains('@')
        || t.contains('=')
        || t.starts_with("--")
        || has_between(t, '_')
        || camel(t)
        || call(raw)
        || dotted(t)
}

/// The identifiers in `query`, in order, without repeats.
pub(crate) fn identifiers(query: &str) -> Vec<String> {
    // Which byte ranges are inside backticks.
    let mut in_tick = vec![false; query.len()];
    let mut open: Option<usize> = None;
    for (i, ch) in query.char_indices() {
        if ch == '`' {
            match open.take() {
                Some(s) => in_tick[s..i].iter_mut().for_each(|b| *b = true),
                None => open = Some(i),
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut after_flag = false;
    let mut pos = 0;
    for raw in query.split_whitespace() {
        let start = query[pos..].find(raw).map(|o| pos + o).unwrap_or(pos);
        pos = start + raw.len();
        let t = raw.trim_matches(STRIP);
        if t.is_empty() {
            after_flag = false;
            continue;
        }
        let ticked = in_tick[start..start + raw.len()].iter().any(|b| *b);
        let is_ident = ticked || shaped(raw, t);
        // A flag's value counts only if it's an identifier itself or holds
        // `,`, `=`, `.` or `/`.
        let flag_value = after_flag && t.contains([',', '=', '.', '/']);
        if (is_ident || flag_value) && !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
        after_flag = is_ident && t.starts_with("--");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::identifiers;

    #[test]
    fn the_dispatch_cases() {
        assert_eq!(
            identifiers("what does `snapshot_list` return"),
            ["snapshot_list"]
        );
        assert_eq!(
            identifiers("why use --features failpoints,benches,keychain"),
            ["--features", "failpoints,benches,keychain"]
        );
        assert_eq!(
            identifiers("RecallRequest::from_text defaults"),
            ["RecallRequest::from_text"]
        );
        assert_eq!(
            identifiers("where does keyring@3.6.3 store it"),
            ["keyring@3.6.3"]
        );
        assert!(identifiers("why 1.92").is_empty());
        assert!(identifiers("who is the product for").is_empty());
    }

    #[test]
    fn the_other_shapes() {
        assert_eq!(
            identifiers("does moveToState(CREATED) stop it?"),
            ["moveToState(CREATED"]
        );
        assert_eq!(identifiers("what's in store.rs"), ["store.rs"]);
        assert_eq!(identifiers("set max_bytes"), ["max_bytes"]);
        assert_eq!(
            identifiers("is MnemoError::Replaced safe"),
            ["MnemoError::Replaced"]
        );
        assert!(identifiers("v0.14.0 and 2.5 s").is_empty());
        assert!(identifiers("it's done, isn't it?").is_empty());
    }

    #[test]
    fn a_flags_value_counts_only_when_it_looks_like_one() {
        // A plain word after a flag isn't its value.
        assert_eq!(identifiers("what does --shared do"), ["--shared"]);
        assert_eq!(identifiers("--wait for a writer"), ["--wait"]);
        // `,`, `=`, `.` or `/` make it one, and so does an identifier shape.
        assert_eq!(
            identifiers("run with --model-dir models/arctic"),
            ["--model-dir", "models/arctic"]
        );
        assert_eq!(identifiers("--wait 2.5s please"), ["--wait", "2.5s"]);
        assert_eq!(
            identifiers("--filter area=decisions"),
            ["--filter", "area=decisions"]
        );
        assert_eq!(
            identifiers("--agent cursor_agent"),
            ["--agent", "cursor_agent"]
        );
        // Only the token right after the flag.
        assert_eq!(
            identifiers("--key fact.msrv then a,b"),
            ["--key", "fact.msrv"]
        );
    }

    #[test]
    fn backticks_and_repeats() {
        // Anything in backticks, even a plain word.
        assert_eq!(identifiers("why is `recall` slow"), ["recall"]);
        assert_eq!(
            identifiers("`a b` and `a`"),
            ["a", "b"],
            "each token inside the ticks, once"
        );
        assert_eq!(identifiers("max_bytes or max_bytes?"), ["max_bytes"]);
        // An unclosed backtick marks nothing.
        assert!(identifiers("a `plain question").is_empty());
        assert!(identifiers("").is_empty());
    }
}
