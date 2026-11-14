//! FMT-9d (Phase 2): the keyword side of hybrid recall.
//!
//! * [`tokenize`]: UAX #29 words, lowercased, plus **compound tokens**. A
//!   compound is a whitespace-separated run with its leading and trailing
//!   punctuation stripped, kept whole when it still has punctuation inside,
//!   so identifiers like `BP.1`, `v0.9.0` or `PR-10b.2` are searchable as
//!   themselves (UAX #29 splits them). No stemming in v1.
//! * TOK-1 (v0.15), [`Tokenizer::V2`]: tokenizer 1's tokens plus the parts
//!   of each, split on `.`, `/`, `_`, `-` and `::`, and a possessive's stem,
//!   so `nosync` finds `mnemo.nosync` and `icloud` finds `iCloud's`. A
//!   segment written with it carries [`V2_MARK`] (see [`Tokenizer`]).
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
///
/// TOK-1: the manifest byte stays 1 whatever the segments hold, because
/// every v9 build before v0.15 refuses to open a file whose byte isn't 1.
/// Tokenizer 2 is recorded per segment instead ([`V2_MARK`]).
pub(crate) const TOKENIZER_V1: u8 = 1;

/// TOK-1: the reserved term a segment written with tokenizer 2 carries.
/// Its one posting is document 0, which is never live, so no query
/// matches it and every merge drops it. No tokenizer makes it (it starts
/// with a NUL), and it sorts first in the dict.
pub(crate) const V2_MARK: &str = "\u{0}mnemo.tokenizer.2";

/// TOK-1: the reserved term a segment carries when some of its documents
/// were written with tokenizer 2 and some with tokenizer 1 (a v0.15 merge
/// of a mixed index). Like [`V2_MARK`], it posts document 0 only.
pub(crate) const MIXED_MARK: &str = "\u{0}mnemo.tokenizer.mixed";

/// TOK-1: the tokenizer a document or a query is split with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tokenizer {
    /// UAX #29 words and compounds (FMT-9d).
    V1,
    /// Tokenizer 1 plus the parts (TOK-1, v0.15).
    V2,
}

/// TOK-1: which tokenizer a segment's documents were written with, from
/// its first dict term.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SegmentTokens {
    /// No mark: tokenizer 1 (any build), or an older build's merge.
    V1,
    /// [`V2_MARK`]: every document is tokenizer 2.
    V2,
    /// [`MIXED_MARK`]: some of each.
    Mixed,
}

impl SegmentTokens {
    /// The mark of a segment whose dict starts with `first`.
    pub(crate) fn from_first_term(first: Option<&str>) -> Self {
        match first {
            Some(V2_MARK) => SegmentTokens::V2,
            Some(MIXED_MARK) => SegmentTokens::Mixed,
            _ => SegmentTokens::V1,
        }
    }

    /// The mark a segment built from these parts carries: tokenizer 2 only
    /// when every part is, tokenizer 1 only when every part is.
    pub(crate) fn combine(parts: impl IntoIterator<Item = SegmentTokens>) -> Option<Self> {
        let mut out: Option<SegmentTokens> = None;
        for p in parts {
            out = Some(match (out, p) {
                (None, p) => p,
                (Some(a), b) if a == b => a,
                _ => SegmentTokens::Mixed,
            });
        }
        out
    }
}

/// TOK-1: the state of a file's keyword index, as `info` and `verify` show
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeywordTokenizer {
    /// Tokenizer 1: whole words as written. A compaction or `reembed`
    /// rebuilds the index with tokenizer 2.
    V1,
    /// Tokenizer 2: whole words and their parts.
    V2,
    /// Some documents have tokenizer 2 and some tokenizer 1, because a
    /// build before v0.15 wrote to or merged the index. Parts match only
    /// the tokenizer 2 documents; whole words match every document. A
    /// compaction or `reembed` rebuilds it with tokenizer 2.
    Mixed,
}

impl KeywordTokenizer {
    /// A file's state from its segments' marks: no segment is tokenizer 2
    /// (an empty index takes tokenizer 2 from its first segment).
    pub(crate) fn of(segments: impl IntoIterator<Item = SegmentTokens>) -> Self {
        match SegmentTokens::combine(segments) {
            None | Some(SegmentTokens::V2) => KeywordTokenizer::V2,
            Some(SegmentTokens::V1) => KeywordTokenizer::V1,
            Some(SegmentTokens::Mixed) => KeywordTokenizer::Mixed,
        }
    }

    /// The tokenizer new documents and queries use: tokenizer 1 only for a
    /// pure tokenizer 1 index, so it stays pure until it's rebuilt. In a
    /// mixed index a query's whole words match every document and its parts
    /// match the tokenizer 2 ones.
    pub(crate) fn tokenizer(self) -> Tokenizer {
        match self {
            KeywordTokenizer::V1 => Tokenizer::V1,
            KeywordTokenizer::V2 | KeywordTokenizer::Mixed => Tokenizer::V2,
        }
    }

    /// A short name: `"1"`, `"2"` or `"mixed"`.
    pub fn as_str(self) -> &'static str {
        match self {
            KeywordTokenizer::V1 => "1",
            KeywordTokenizer::V2 => "2",
            KeywordTokenizer::Mixed => "mixed",
        }
    }
}

/// Longest token indexed, in bytes (longer ones are dropped).
const MAX_TOKEN_BYTES: usize = 64;

/// The tokens of `text` under tokenizer `tok`, with repeats.
///
/// Tokenizer 2 (R-008.2) works per whitespace run: the run's UAX #29 words,
/// its compound, and the parts of each, with **each distinct term counted
/// once per run**. A term in two runs counts twice. (Tokenizer 1 can emit a
/// dotted name twice, once as a UAX #29 word and once as the compound; it
/// stays as released, since existing indexes depend on it.)
pub(crate) fn tokenize_with(tok: Tokenizer, text: &str) -> Vec<String> {
    match tok {
        Tokenizer::V1 => tokenize(text),
        Tokenizer::V2 => {
            let mut out = Vec::new();
            for run in text.split_whitespace() {
                let mut terms: Vec<String> = run
                    .unicode_words()
                    .map(|w| w.to_lowercase())
                    .filter(|w| w.len() <= MAX_TOKEN_BYTES)
                    .collect();
                terms.extend(compound(run));
                let whole = terms.len();
                for i in 0..whole {
                    let parts = token_parts(&terms[i]);
                    terms.extend(parts);
                }
                let mut seen = std::collections::BTreeSet::new();
                out.extend(terms.into_iter().filter(|t| seen.insert(t.clone())));
            }
            out
        }
    }
}

/// FMT-9d: a whitespace run's compound token: the run with its leading and
/// trailing punctuation stripped, when it still has punctuation inside.
fn compound(run: &str) -> Option<String> {
    let t = run.trim_matches(|c: char| !c.is_alphanumeric());
    if t.len() < 2 || t.len() > MAX_TOKEN_BYTES {
        return None;
    }
    (t.chars().any(|c| !c.is_alphanumeric()) && t.chars().any(|c| c.is_alphanumeric()))
        .then(|| t.to_lowercase())
}

/// TOK-1: the parts of one tokenizer 1 token: a possessive's stem, and the
/// pieces between `.`, `/`, `_`, `-` and `::`. Only parts that differ from
/// the token itself, and no part that is only digits (the `9` and `0` of
/// `v0.9.0` would match nearly every versioned text; the whole token
/// still matches the version exactly).
fn token_parts(token: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let stem = token
        .strip_suffix("'s")
        .or_else(|| token.strip_suffix("\u{2019}s"))
        .filter(|s| !s.is_empty());
    if let Some(s) = stem {
        out.push(s.to_string());
    }
    let base = stem.unwrap_or(token);
    let pieces: Vec<&str> = base
        .split("::")
        .flat_map(|p| p.split(['.', '/', '_', '-']))
        .filter(|p| !p.is_empty())
        .collect();
    let pieces: Vec<&str> = if pieces.len() > 1 {
        pieces
            .into_iter()
            .filter(|p| !p.chars().all(|c| c.is_ascii_digit()))
            .collect()
    } else {
        Vec::new()
    };
    for p in pieces {
        if p != token && !out.iter().any(|o| o == p) {
            out.push(p.to_string());
        }
    }
    out
}

/// The tokenizer 1 tokens of `text`, in order, with repeats.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .unicode_words()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() <= MAX_TOKEN_BYTES)
        .collect();
    out.extend(text.split_whitespace().filter_map(compound));
    out
}

/// Term frequencies of one document: `(term, tf)`, sorted by term, tf
/// saturating at 255. Also returns the document length (all tokens).
pub(crate) fn term_freqs(tok: Tokenizer, text: &str) -> (Vec<(String, u8)>, u16) {
    let toks = tokenize_with(tok, text);
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

/// An in-memory keyword index over a set of texts, with the tokenizer and
/// BM25 scoring (k1 1.2, b 0.75) of v9's keyword recall, for tools that
/// search what they've already read, without the file open (`mnemo
/// inspect`). It needs no embedder and works on texts from any format.
/// It uses tokenizer 2 (TOK-1), so a part of a dotted name matches.
///
/// ```
/// use mnemo_engine::KeywordIndex;
/// let ix = KeywordIndex::new(["the deploy runs on v0.9.0", "tea, not coffee"]);
/// assert_eq!(ix.search("v0.9.0", 5)[0].0, 0);
/// assert!(ix.search("nothing", 5).is_empty());
/// let ix = KeywordIndex::new(["the repo is mnemo.nosync", "iCloud's copy"]);
/// assert_eq!(ix.search("nosync", 5)[0].0, 0);
/// assert_eq!(ix.search("icloud", 5)[0].0, 1);
/// ```
#[derive(Clone, Debug, Default)]
pub struct KeywordIndex {
    postings: std::collections::BTreeMap<String, Vec<(usize, u8)>>,
    lens: Vec<u16>,
    avgdl: f32,
}

impl KeywordIndex {
    /// Index `texts`; a hit's index is the text's position here.
    pub fn new<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        let mut ix = Self::default();
        for (doc, text) in texts.into_iter().enumerate() {
            let (tfs, len) = term_freqs(Tokenizer::V2, text);
            for (term, tf) in tfs {
                ix.postings.entry(term).or_default().push((doc, tf));
            }
            ix.lens.push(len);
        }
        let total: u64 = ix.lens.iter().map(|&l| l as u64).sum();
        ix.avgdl = if ix.lens.is_empty() {
            0.0
        } else {
            total as f32 / ix.lens.len() as f32
        };
        ix
    }

    /// The texts that match any of `query`'s terms, best first, at most
    /// `top_k`, as `(index, score)`. Ties keep the texts' order.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(usize, f32)> {
        let terms: std::collections::BTreeSet<String> =
            tokenize_with(Tokenizer::V2, query).into_iter().collect();
        let n = self.lens.len();
        let mut scores: std::collections::BTreeMap<usize, f32> = std::collections::BTreeMap::new();
        for term in &terms {
            let Some(posts) = self.postings.get(term) else {
                continue;
            };
            let idf = idf(n, posts.len());
            for &(doc, tf) in posts {
                *scores.entry(doc).or_default() +=
                    bm25(tf, idf, self.lens[doc], self.avgdl, 1.2, 0.75);
            }
        }
        let mut hits: Vec<(usize, f32)> = scores.into_iter().collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        hits.truncate(top_k);
        hits
    }

    /// How many texts it holds.
    pub fn len(&self) -> usize {
        self.lens.len()
    }

    /// Whether it holds no text.
    pub fn is_empty(&self) -> bool {
        self.lens.is_empty()
    }
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
/// TOK-1: a segment whose documents are tokenizer 2, or a mix, carries its
/// mark ([`V2_MARK`], [`MIXED_MARK`]) as a term posting document 0.
pub(crate) fn build_segment(
    docs: &[(u32, Vec<(String, u8)>)],
    tokens: SegmentTokens,
) -> Result<Vec<u8>> {
    let mut by_term: std::collections::BTreeMap<&str, Vec<(u32, u8)>> =
        std::collections::BTreeMap::new();
    match tokens {
        SegmentTokens::V1 => {}
        SegmentTokens::V2 => {
            by_term.insert(V2_MARK, vec![(0, 1)]);
        }
        SegmentTokens::Mixed => {
            by_term.insert(MIXED_MARK, vec![(0, 1)]);
        }
    }
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
            (3u32, term_freqs(Tokenizer::V1, "alpha beta beta").0),
            (7u32, term_freqs(Tokenizer::V1, "beta gamma").0),
            (300u32, term_freqs(Tokenizer::V1, "alpha").0),
        ];
        let seg = build_segment(&docs, SegmentTokens::V1).unwrap();
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
    fn tokenizer_2_adds_the_parts() {
        let t = tokenize_with(
            Tokenizer::V2,
            "Moved to ~/Documents/GitHub/mnemo.nosync, out of iCloud's reach; \
             see std::fs::rename, max_bytes and PR-10b.2 in v0.9.0. It\u{2019}s done.",
        );
        for want in [
            "mnemo.nosync",
            "mnemo",
            "nosync",
            "icloud's",
            "icloud",
            "std::fs::rename",
            "std",
            "fs",
            "rename",
            "max_bytes",
            "max",
            "bytes",
            "pr-10b.2",
            "pr",
            "10b",
            "v0",
            "documents",
            "github",
            "it",
        ] {
            assert!(t.contains(&want.to_string()), "{want} missing from {t:?}");
        }
        // No digit-only parts.
        let v = tokenize_with(Tokenizer::V2, "v0.9.0");
        assert!(v.contains(&"v0.9.0".to_string()) && v.contains(&"v0".to_string()));
        assert!(!v.contains(&"9".to_string()), "{v:?}");
        // Tokenizer 1 is unchanged: whole words only.
        let v1 = tokenize_with(Tokenizer::V1, "mnemo.nosync iCloud's");
        assert!(!v1.contains(&"nosync".to_string()));
        assert!(!v1.contains(&"icloud".to_string()));
        // A token with nothing to split adds nothing.
        assert_eq!(
            tokenize_with(Tokenizer::V2, "plain words"),
            tokenize_with(Tokenizer::V1, "plain words")
        );
    }

    /// R-008.2 (REVIEWER-A's case): tokenizer 2 counts each term once per
    /// whitespace run; a term in two runs counts twice.
    #[test]
    fn tokenizer_2_counts_a_term_once_per_run() {
        let (tfs, len) = term_freqs(
            Tokenizer::V2,
            "mnemo.nosync key-file open_shared Mnemo::open iCloud's",
        );
        let tf = |t: &str| tfs.iter().find(|(x, _)| x == t).map(|(_, n)| *n);
        for (term, n) in &tfs {
            let want = if term == "mnemo" || term == "open" {
                2
            } else {
                1
            };
            assert_eq!(*n, want, "{term}: {tfs:?}");
        }
        for t in [
            "mnemo.nosync",
            "nosync",
            "key-file",
            "key",
            "file",
            "open_shared",
            "shared",
            "mnemo::open",
            "icloud's",
            "icloud",
        ] {
            assert_eq!(tf(t), Some(1), "{t}: {tfs:?}");
        }
        assert_eq!(tf("mnemo"), Some(2));
        assert_eq!(tf("open"), Some(2));
        // The length counts the distinct terms of each run.
        assert_eq!(
            len as usize,
            tfs.iter().map(|(_, n)| *n as usize).sum::<usize>()
        );
        // Tokenizer 1 is unchanged: the dotted name twice, as in v0.14.0.
        let (v1, _) = term_freqs(Tokenizer::V1, "mnemo.nosync");
        assert_eq!(v1, vec![("mnemo.nosync".to_string(), 2)]);
    }

    #[test]
    fn a_marked_segment_starts_with_its_mark_and_posts_document_0() {
        let docs = vec![(5u32, term_freqs(Tokenizer::V2, "mnemo.nosync").0)];
        for (tokens, mark) in [
            (SegmentTokens::V2, Some(V2_MARK)),
            (SegmentTokens::Mixed, Some(MIXED_MARK)),
            (SegmentTokens::V1, None),
        ] {
            let seg = build_segment(&docs, tokens).unwrap();
            let dlen = u32::from_le_bytes(seg[0..4].try_into().unwrap()) as usize;
            let dict: Vec<DictEntry> = rmp_serde::from_slice(&seg[4..4 + dlen]).unwrap();
            let first = dict.first().map(|e| e.term.as_str());
            assert_eq!(SegmentTokens::from_first_term(first), tokens);
            if let Some(m) = mark {
                let e = &dict[0];
                assert_eq!(e.term, m);
                let post = &seg[4 + dlen..];
                let p = decode_postings(&post[e.off as usize..(e.off + e.len) as usize]).unwrap();
                assert_eq!(p, vec![(0, 1)]);
            }
        }
    }

    #[test]
    fn a_files_tokenizer_comes_from_its_segments() {
        use SegmentTokens as S;
        assert_eq!(KeywordTokenizer::of([]), KeywordTokenizer::V2);
        assert_eq!(KeywordTokenizer::of([S::V2, S::V2]), KeywordTokenizer::V2);
        assert_eq!(KeywordTokenizer::of([S::V1, S::V1]), KeywordTokenizer::V1);
        assert_eq!(
            KeywordTokenizer::of([S::V2, S::V1]),
            KeywordTokenizer::Mixed
        );
        assert_eq!(KeywordTokenizer::of([S::Mixed]), KeywordTokenizer::Mixed);
        assert_eq!(KeywordTokenizer::V1.tokenizer(), Tokenizer::V1);
        assert_eq!(KeywordTokenizer::Mixed.tokenizer(), Tokenizer::V2);
    }

    #[test]
    fn bm25_prefers_rarer_terms_and_shorter_docs() {
        assert!(idf(100, 1) > idf(100, 50));
        assert!(bm25(1, 1.0, 5, 10.0, 1.2, 0.75) > bm25(1, 1.0, 20, 10.0, 1.2, 0.75));
        assert!(bm25(3, 1.0, 10, 10.0, 1.2, 0.75) > bm25(1, 1.0, 10, 10.0, 1.2, 0.75));
    }
}
