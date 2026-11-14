//! 12.3: the recovery code, version 1 (the keys note, section 3).
//!
//! `MN1-` and thirteen groups of five Crockford Base32 symbols: four data
//! symbols and a check. The 256-bit key, most significant bit first, then
//! four zero bits, makes the 52 data symbols. Group `i`'s check is
//! `i ⊕ α·d0 ⊕ α²·d1 ⊕ α³·d2 ⊕ α⁴·d3` in GF(32) with x⁵ + x² + 1 and α = x,
//! which catches any one wrong character in a group, any two swapped within
//! it, and a group typed in another group's place.

use zeroize::Zeroizing;

/// The Crockford Base32 alphabet (no I, L, O or U).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// The version prefix.
const PREFIX: &str = "MN1";
/// Groups in a code.
const GROUPS: usize = 13;
/// Symbols in a group on screen: four data symbols and the check.
const GROUP_LEN: usize = 5;
/// Symbols after the prefix.
const SYMBOLS: usize = GROUPS * GROUP_LEN;

/// x⁵ + x² + 1, the field's polynomial.
const POLY: u8 = 0b10_0101;

/// Multiply in GF(32).
fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        b >>= 1;
        a <<= 1;
        if a & 0b10_0000 != 0 {
            a ^= POLY;
        }
    }
    r
}

/// The weights α¹ to α⁴ of the four data positions (α = 2).
const WEIGHTS: [u8; 4] = [2, 4, 8, 16];

/// Group `i`'s check symbol over its data symbols `d`.
fn check(i: usize, d: &[u8; 4]) -> u8 {
    let mut c = i as u8;
    for (w, v) in WEIGHTS.iter().zip(d) {
        c ^= gmul(*w, *v);
    }
    c
}

/// The code for `key`, as shown: `MN1-` and thirteen hyphenated groups.
pub fn encode(key: &[u8; 32]) -> Zeroizing<String> {
    let syms = data_symbols(key);
    let mut out = Zeroizing::new(String::with_capacity(
        PREFIX.len() + GROUPS * (GROUP_LEN + 1),
    ));
    out.push_str(PREFIX);
    for i in 0..GROUPS {
        let mut d = [0u8; 4];
        d.copy_from_slice(&syms[4 * i..4 * i + 4]);
        out.push('-');
        for v in d {
            out.push(ALPHABET[v as usize] as char);
        }
        out.push(ALPHABET[check(i, &d) as usize] as char);
    }
    out
}

/// The 52 data symbols of `key` (256 bits, then four zero bits).
fn data_symbols(key: &[u8; 32]) -> Zeroizing<[u8; 52]> {
    let mut syms = Zeroizing::new([0u8; 52]);
    for (k, sym) in syms.iter_mut().enumerate() {
        let mut v = 0u8;
        for b in 0..5 {
            let bit = 5 * k + b;
            let set = bit < 256 && key[bit / 8] & (0x80 >> (bit % 8)) != 0;
            v = (v << 1) | u8::from(set);
        }
        *sym = v;
    }
    syms
}

/// Why a typed code was refused. The message names the group (counted
/// from 1) where it can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeError(pub String);

impl std::fmt::Display for CodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodeError {}

/// One typed character's symbol value, with Crockford's aliases (`I`, `L`
/// read as `1`, `O` as `0`), case ignored.
fn symbol(c: char) -> Option<u8> {
    let c = match c.to_ascii_uppercase() {
        'I' | 'L' => '1',
        'O' => '0',
        c => c,
    };
    ALPHABET
        .iter()
        .position(|&a| a as char == c)
        .map(|p| p as u8)
}

/// Whether `text` looks like a recovery code (it starts with the prefix,
/// read with the aliases), so a caller can report its errors as a code's.
pub fn looks_like_code(text: &str) -> bool {
    let t: String = text.trim().chars().take(3).collect();
    // M, N and 1 are symbols 20, 21 and 1.
    t.chars().count() == 3
        && t.chars().map(symbol).collect::<Option<Vec<_>>>() == Some(vec![20, 21, 1])
}

/// Read a typed code back into its key.
///
/// Case and spaces are ignored, `I`/`L` read as `1` and `O` as `0` (the
/// prefix included). With the right length, hyphens are ignored wherever
/// they are. With the wrong length, the hyphens, when there are any, say
/// which group is short or long; otherwise the error names "group N or the
/// one before it", or the last group when every complete group checks out.
pub fn parse(text: &str) -> Result<Zeroizing<[u8; 32]>, CodeError> {
    let typed: Zeroizing<String> =
        Zeroizing::new(text.chars().filter(|c| !c.is_whitespace()).collect());
    if !looks_like_code(&typed) {
        return Err(CodeError(
            "a recovery code starts with MN1 (this one doesn't, or it was made by a newer mnemo)"
                .into(),
        ));
    }
    let body = typed[3..].trim_start_matches('-');
    let mut values: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::with_capacity(SYMBOLS));
    for (pos, c) in body.chars().filter(|&c| c != '-').enumerate() {
        match symbol(c) {
            Some(v) => values.push(v),
            None => {
                return Err(CodeError(format!(
                    "{:?} isn't a recovery-code character (group {})",
                    c,
                    (pos / GROUP_LEN + 1).min(GROUPS)
                )))
            }
        }
    }
    if values.len() != SYMBOLS {
        return Err(CodeError(wrong_length(body, &values)));
    }
    let bad: Vec<usize> = (0..GROUPS)
        .filter(|&i| !group_checks(i, &values[GROUP_LEN * i..GROUP_LEN * i + GROUP_LEN]))
        .collect();
    if !bad.is_empty() {
        let names: Vec<String> = bad.iter().map(|i| (i + 1).to_string()).collect();
        return Err(CodeError(match names.len() {
            1 => format!(
                "group {} doesn't check out; look for a typo there",
                names[0]
            ),
            _ => format!(
                "groups {} don't check out; look for typos there",
                names.join(", ")
            ),
        }));
    }
    // The last data symbol holds one key bit and four zero bits.
    let last = values[GROUP_LEN * (GROUPS - 1) + 3];
    if last != 0 && last != 16 {
        return Err(CodeError(format!(
            "group {GROUPS} doesn't check out (its fourth character must be 0 or G)"
        )));
    }
    let mut key = Zeroizing::new([0u8; 32]);
    let mut bit = 0usize;
    for i in 0..GROUPS {
        for &v in &values[GROUP_LEN * i..GROUP_LEN * i + 4] {
            for b in (0..5).rev() {
                if bit < 256 && (v >> b) & 1 == 1 {
                    key[bit / 8] |= 0x80 >> (bit % 8);
                }
                bit += 1;
            }
        }
    }
    Ok(key)
}

/// Whether group `i` (its five symbols) checks out.
fn group_checks(i: usize, g: &[u8]) -> bool {
    g.len() == GROUP_LEN && check(i, &[g[0], g[1], g[2], g[3]]) == g[4]
}

/// The message for a code of the wrong length.
fn wrong_length(body: &str, values: &[u8]) -> String {
    let count = format!(
        "{} characters after {PREFIX}, {SYMBOLS} expected",
        values.len()
    );
    // With hyphens, the person's own grouping says where.
    let parts: Vec<&str> = body.split('-').filter(|p| !p.is_empty()).collect();
    if parts.len() > 1 {
        let odd: Vec<String> = parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.chars().count() != GROUP_LEN)
            .map(|(i, p)| format!("group {} has {}", i + 1, p.chars().count()))
            .collect();
        if !odd.is_empty() {
            return format!("{count}: {}, {GROUP_LEN} expected", odd.join(", "));
        }
    }
    let complete = values.len() / GROUP_LEN;
    let first_bad = (0..complete.min(GROUPS))
        .find(|&i| !group_checks(i, &values[GROUP_LEN * i..GROUP_LEN * i + GROUP_LEN]));
    match first_bad {
        Some(0) => format!("{count}; group 1 doesn't check out, so look there"),
        Some(i) => format!(
            "{count}; groups 1 to {i} check out and group {} doesn't, so look in group {} or \
             the one before it",
            i + 1,
            i + 1
        ),
        None => format!(
            "{count}; every complete group checks out, so look in the last group ({})",
            (complete + 1).min(GROUPS)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worked_example_encodes_as_the_note_shows() {
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        assert_eq!(
            encode(&key).as_str(),
            "MN1-000GD-40R4X-0M300-E209K-185G3-R38ED-1W811-24GKZ-2GAH1-C5RR2-34D1B-P70XX-3RFGZ"
        );
        assert_eq!(*parse(&encode(&key)).unwrap(), key);
    }

    #[test]
    fn group_one_of_the_example_checks_as_the_note_works_it() {
        // 40R4 with i = 1: 1 ⊕ 8 ⊕ 0 ⊕ 30 ⊕ 10 = 29, which is X.
        assert_eq!(gmul(16, 4), 10);
        assert_eq!(gmul(8, 24), 30);
        assert_eq!(check(1, &[4, 0, 24, 4]), 29);
        assert_eq!(ALPHABET[29], b'X');
    }
}
