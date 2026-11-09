//! FMT-9d (Phase 2): the keyword side of hybrid recall.
//!
//! * [`tokenize`]: UAX #29 words, lowercased, plus **compound tokens**. A
//!   compound is a whitespace-separated run with its leading and trailing
//!   punctuation stripped, kept whole when it still has punctuation inside,
//!   so identifiers like `BP.1`, `v0.9.0` or `PR-10b.2` are searchable as
//!   themselves (UAX #29 splits them). No stemming in v1.
//! * Segments: the on-disk unit of the BM25 index. A segment is
//!   `[u32 dict length][dict][postings]`. The dict is an rmp list of
//!   `(term, offset, length)`, sorted by term; postings are, per term,
//!   `(doc delta varint, tf u8)` pairs in ascending doc order.
//! * [`bm25`]: the per-term score, with k1 1.2 and b 0.75 by default.

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::error::{MnemoError, Result};

/// The tokenizer above. Recorded in the BM25 manifest, so a later
/// tokenizer (a CJK bigram one, say) can be added without a format change.
pub(crate) const TOKENIZER_V1: u8 = 1;

/// Longest token indexed, in bytes (longer ones are dropped).
const MAX_TOKEN_BYTES: usize = 64;

/// The tokens of `text`, in order, with repeats.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .unicode_words()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() <= MAX_TOKEN_BYTES)
        .collect();
    for run in text.split_whitespace() {
        let t = run.trim_matches(|c: char| !c.is_alphanumeric());
        if t.len() < 2 || t.len() > MAX_TOKEN_BYTES {
            continue;
        }
        if t.chars().any(|c| !c.is_alphanumeric()) && t.chars().any(|c| c.is_alphanumeric()) {
            out.push(t.to_lowercase());
        }
    }
    out
}

/// Term frequencies of one document: `(term, tf)`, sorted by term, tf
/// saturating at 255. Also returns the document length (all tokens).
pub(crate) fn term_freqs(text: &str) -> (Vec<(String, u8)>, u16) {
    let toks = tokenize(text);
    let len = toks.len().min(u16::MAX as usize) as u16;
    let mut map: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for t in toks {
        *map.entry(t).or_default() += 1;
    }
    (
        map.into_iter()
            .map(|(t, n)| (t, n.min(255) as u8))
            .collect(),
        len,
    )
}

/// BM25 score of one term in one document.
pub(crate) fn bm25(tf: u8, idf: f32, doc_len: u16, avgdl: f32, k1: f32, b: f32) -> f32 {
    let tf = tf as f32;
    let norm = 1.0 - b + b * (doc_len as f32 / avgdl.max(1e-6));
    idf * tf * (k1 + 1.0) / (tf + k1 * norm)
}

/// BM25 idf for a term in `df` of `n` live documents.
pub(crate) fn idf(n: usize, df: usize) -> f32 {
    let (n, df) = (n as f32, df as f32);
    (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
}

/// One dict entry: a term and its postings' byte range.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct DictEntry {
    pub term: String,
    pub off: u32,
    pub len: u32,
}

fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Decode a postings slice into `(doc, tf)` pairs.
pub(crate) fn decode_postings(b: &[u8]) -> Result<Vec<(u32, u8)>> {
    let bad = || MnemoError::Invalid("malformed BM25 postings".into());
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut doc = 0u32;
    while i < b.len() {
        let mut delta = 0u32;
        let mut shift = 0u32;
        loop {
            let byte = *b.get(i).ok_or_else(bad)?;
            i += 1;
            if shift > 28 {
                return Err(bad());
            }
            delta |= ((byte & 0x7f) as u32) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        doc = doc.checked_add(delta).ok_or_else(bad)?;
        let tf = *b.get(i).ok_or_else(bad)?;
        i += 1;
        out.push((doc, tf));
    }
    Ok(out)
}

/// Build a segment from documents' term frequencies: `(doc, terms)`.
pub(crate) fn build_segment(docs: &[(u32, Vec<(String, u8)>)]) -> Result<Vec<u8>> {
    let mut by_term: std::collections::BTreeMap<&str, Vec<(u32, u8)>> =
        std::collections::BTreeMap::new();
    for (doc, terms) in docs {
        for (t, tf) in terms {
            by_term.entry(t.as_str()).or_default().push((*doc, *tf));
        }
    }
    let mut dict: Vec<DictEntry> = Vec::with_capacity(by_term.len());
    let mut postings: Vec<u8> = Vec::new();
    for (term, mut list) in by_term {
        list.sort_unstable_by_key(|&(d, _)| d);
        let off = postings.len() as u32;
        let mut prev = 0u32;
        for (d, tf) in list {
            put_varint(&mut postings, d - prev);
            postings.push(tf);
            prev = d;
        }
        dict.push(DictEntry {
            term: term.to_string(),
            off,
            len: postings.len() as u32 - off,
        });
    }
    let dict_bytes = rmp_serde::to_vec(&dict).map_err(|e| MnemoError::Serialize(e.to_string()))?;
    let mut out = Vec::with_capacity(4 + dict_bytes.len() + postings.len());
    out.extend_from_slice(&(dict_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&dict_bytes);
    out.extend_from_slice(&postings);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_keeps_identifiers_whole() {
        let t = tokenize("Ship BP.1 in v0.9.0 (see PR-10b.2), then flush().");
        for want in ["ship", "bp", "1", "bp.1", "v0.9.0", "pr-10b.2", "flush"] {
            assert!(t.contains(&want.to_string()), "{want} missing from {t:?}");
        }
        assert!(
            !t.contains(&"flush()".to_string()),
            "trailing punctuation stripped"
        );
        assert!(tokenize("Ünïcödé Wörds").contains(&"ünïcödé".to_string()));
    }

    #[test]
    fn segment_round_trips() {
        let docs = vec![
            (3u32, term_freqs("alpha beta beta").0),
            (7u32, term_freqs("beta gamma").0),
            (300u32, term_freqs("alpha").0),
        ];
        let seg = build_segment(&docs).unwrap();
        let dlen = u32::from_le_bytes(seg[0..4].try_into().unwrap()) as usize;
        let dict: Vec<DictEntry> = rmp_serde::from_slice(&seg[4..4 + dlen]).unwrap();
        let post = &seg[4 + dlen..];
        let find = |t: &str| {
            let e = dict.iter().find(|e| e.term == t).unwrap();
            decode_postings(&post[e.off as usize..(e.off + e.len) as usize]).unwrap()
        };
        assert_eq!(find("alpha"), vec![(3, 1), (300, 1)]);
        assert_eq!(find("beta"), vec![(3, 2), (7, 1)]);
        assert_eq!(find("gamma"), vec![(7, 1)]);
    }

    #[test]
    fn bm25_prefers_rarer_terms_and_shorter_docs() {
        assert!(idf(100, 1) > idf(100, 50));
        assert!(bm25(1, 1.0, 5, 10.0, 1.2, 0.75) > bm25(1, 1.0, 20, 10.0, 1.2, 0.75));
        assert!(bm25(3, 1.0, 10, 10.0, 1.2, 0.75) > bm25(1, 1.0, 10, 10.0, 1.2, 0.75));
    }
}
