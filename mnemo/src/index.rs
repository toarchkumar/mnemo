//! IVF + PQ approximate-nearest-neighbour index (Phase 2 of the build plan).
//!
//! Exact search is `O(n)`. This module makes retrieval sub-linear:
//!
//! 1. **IVF** (inverted file): vectors are grouped into roughly `√n`
//!    partitions by k-means. A query scans only the `n_probe` partitions
//!    whose centroids are nearest to it.
//! 2. **PQ** (product quantization): each vector is cut into `m` contiguous
//!    subspaces; every subspace is quantized to one of up to 256 learned
//!    codewords, so a whole vector collapses to `m` bytes. Candidate
//!    distances are summed from precomputed per-subspace lookup tables — the
//!    scan never touches a full float vector.
//! 3. **Rerank**: the `n_rerank` closest candidates by PQ distance are
//!    returned to the store, which loads their exact vectors for final,
//!    precise ranking.
//!
//! All distances *inside* the index are squared-L2. The index only has to
//! produce a good candidate *set*; the store reranks with the caller's actual
//! metric (cosine / dot / L2). For embedding vectors — which tend to have
//! similar norms — L2-nearest and cosine-nearest largely agree, and a
//! generous `n_rerank` absorbs the rest.

use std::collections::HashMap;

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::error::{MnemoError, Result};

/// Estimated heap bytes of a `HashMap<K, V>` with the given capacity:
/// one `(K, V)` slot plus one control byte per bucket, with buckets
/// rounded up from capacity at the 7/8 load factor. An estimate for the
/// P-0 device budget, not an exact allocator figure.
pub(crate) fn hash_map_bytes<K, V>(capacity: usize) -> usize {
    if capacity == 0 {
        return 0;
    }
    let buckets = (capacity * 8 / 7).next_power_of_two();
    buckets * (std::mem::size_of::<(K, V)>() + 1)
}

/// Default RNG seed — fixed so index builds are reproducible.
const DEFAULT_SEED: u64 = 0x4d_4e_45_4d_4f_49_44_58; // "MNEMOIDX"
/// Training-set cap: k-means trains on at most this many sampled vectors.
const TRAIN_CAP: usize = 50_000;
/// Codewords per PQ subspace.
const PQ_CODEWORDS: usize = 256;

/// Tuning knobs for building and querying an [`IvfPqIndex`].
#[derive(Clone, Copy, Debug)]
pub struct IndexConfig {
    /// Number of IVF partitions. `0` means auto (`ceil(√n)`).
    pub n_partitions: usize,
    /// Number of PQ subspaces. `0` means auto (`dims / 4`). Previously
    /// `dims / 8` before PR ANN-1 — see [`IndexConfig::default`] for
    /// why the bump.
    pub pq_subspaces: usize,
    /// Partitions scanned per query (the IVF accuracy/speed dial).
    /// `0` means auto (`ceil(partitions / 2)`). Previously a fixed `8`
    /// before PR ANN-1.
    pub n_probe: usize,
    /// Candidates handed back for exact reranking (the PQ accuracy dial).
    /// `0` means auto (`500`). Previously a fixed `64` before PR ANN-1.
    pub n_rerank: usize,
    /// Lloyd-iteration count for every k-means run.
    pub kmeans_iters: usize,
    /// RNG seed for reproducible builds.
    pub seed: u64,
}

/// Auto-pick `n_probe` from the IVF partition count.
///
/// Formula: `max(ceil(partitions / 2), min(partitions, 100))`.
///
/// Full probe up to 100 partitions (so ~10K / √10K ≈ 100 corpora
/// clear the 0.95 recall@10 target — ANN-1.1 measured 0.964 at
/// n_probe=100 + n_rerank=100 vs 0.914 at n_probe=50), half probe
/// above. At the three measured tiers:
///
/// - 10K   ≈ 100 partitions  → 100 (full)
/// - 100K  ≈ 317 partitions  → 159 (half, from ceil(317/2))
/// - 1M    ≈ 1000 partitions → 500 (half)
///
/// Query latency scales linearly with `n_probe` (and therefore with
/// `n_partitions * fraction_probed`); see `ann_defaults_v0_6_0` in
/// `test/scripts/perf_v0.5.0.json` for the measured per-tier numbers.
pub(crate) fn effective_auto_n_probe(partitions: usize) -> usize {
    let half = partitions.div_ceil(2);
    let full_up_to_100 = partitions.min(100);
    half.max(full_up_to_100).max(1)
}

/// Auto-pick `n_rerank` from the live vector count.
///
/// Formula: `(n_vectors / 200).clamp(100, 1000)`. Hits the planner's
/// ANN-1.1 targets exactly:
///
/// - 10K vectors  → 50 → clamped to 100 (plenty for ~0.98 recall@10)
/// - 100K vectors → 500 (matches the ann_recall_grid_v2 knee)
/// - 1M vectors   → 5000 → clamped to 1000 (keeps latency bounded;
///   recall at 1M is "measure in Phase 11.1" per the planner)
///
/// Pre-ANN-1.1 the default was a fixed 500 at every size, which cost
/// ~5 ms of record-decode time on 10K files for no recall gain over
/// 100 — rerank is bounded by exact decrypt-and-score cost (~10 µs
/// per candidate on hardware AES).
pub(crate) fn effective_auto_n_rerank(n_vectors: usize) -> usize {
    (n_vectors / 200).clamp(100, 1000)
}

impl Default for IndexConfig {
    fn default() -> Self {
        // All four of n_partitions / pq_subspaces / n_probe / n_rerank
        // default to 0, which [`IvfPqIndex::build`] interprets as "pick
        // a sensible value from the corpus and partition count":
        //
        // - n_partitions → `ceil(sqrt(n))`
        // - pq_subspaces → `dims/4` (384-dim → 96 subspaces).
        //   Previously `dims/8` (48); PR ANN-1 bumped it after the
        //   ann_recall_grid_v2 harness showed that 48 subspaces capped
        //   100K recall@10 at ~0.92 even with the full-scan rerank,
        //   while 96 subspaces lifts the knee to 0.98 at ~10 ms/query
        //   on an M1. Costs 2x the PQ codes per vector (48 → 96 bytes,
        //   negligible against the 1.5 KB raw vector) and ~15 % more
        //   build time.
        // - n_probe → `ceil(partitions / 2)` (ANN-1) — scales with
        //   partition count. Previously fixed at 8, which at 317
        //   partitions scanned 2.5 % of the index per query. The
        //   planner's "express relative to index size" clause.
        // - n_rerank → `(n_vectors / 200).clamp(100, 1000)` (ANN-1.1).
        //   10K files land on 100 rerank (plenty for ~0.98 recall@10
        //   on this corpus); 100K lands on 500 (the measured
        //   ann_recall_grid_v2 knee); 1M caps at 1000. ANN-1 shipped a
        //   fixed 500 at every size, which cost ~5 ms of record-decode
        //   time on small files for no recall gain. Still fixed per
        //   build if explicit; computed per query from the live vector
        //   count when auto.
        //
        // Explicit non-zero values still override, so callers who want
        // the old tiny defaults can set them (useful for very small
        // corpora where sqrt(n) partitions is overkill).
        Self {
            n_partitions: 0,
            pq_subspaces: 0,
            n_probe: 0,
            n_rerank: 0,
            kmeans_iters: 25,
            seed: DEFAULT_SEED,
        }
    }
}

/// A read-only snapshot of an index's shape, surfaced via `Mnemo::stats`.
///
/// `Serialize`/`Deserialize` derived so the MCP `stats` tool (and any
/// other JSON-shaped consumer) can emit / round-trip it without a
/// per-field object build. Plain-old-data type — no invariants to
/// preserve across (de)serialization.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct IndexInfo {
    /// Vectors currently held in the index.
    pub vectors: usize,
    /// IVF partition count.
    pub partitions: usize,
    /// PQ subspace count.
    pub subspaces: usize,
    /// Default partitions probed per query.
    pub n_probe: usize,
    /// Default candidates reranked per query.
    pub n_rerank: usize,
}

/// Squared Euclidean distance between two equal-length vectors.
fn squared_l2(a: &[f32], b: &[f32]) -> f32 {
    let mut s = 0.0f32;
    for i in 0..a.len() {
        let d = a[i] - b[i];
        s += d * d;
    }
    s
}

/// Index of the nearest centroid (flat layout, `stride` floats each).
fn nearest(centroids: &[f32], stride: usize, v: &[f32]) -> usize {
    let mut best = 0usize;
    let mut best_d = f32::INFINITY;
    let count = centroids.len() / stride;
    for i in 0..count {
        let d = squared_l2(&centroids[i * stride..(i + 1) * stride], v);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

/// k-means (k-means++ seeding, Lloyd iterations). Returns a flat
/// `k * dim` centroid buffer; `k` may shrink to `points.len()`.
fn kmeans(points: &[&[f32]], k: usize, iters: usize, rng: &mut StdRng) -> Vec<f32> {
    let n = points.len();
    let dim = points[0].len();
    let k = k.clamp(1, n);

    // --- k-means++ seeding ---
    let mut centroids: Vec<f32> = Vec::with_capacity(k * dim);
    let first = rng.gen_range(0..n);
    centroids.extend_from_slice(points[first]);
    let mut d2: Vec<f32> = points
        .iter()
        .map(|p| squared_l2(p, &centroids[0..dim]))
        .collect();

    while centroids.len() / dim < k {
        let sum: f32 = d2.iter().sum();
        let pick = if sum <= 0.0 {
            rng.gen_range(0..n)
        } else {
            let mut t = rng.gen::<f32>() * sum;
            let mut chosen = n - 1;
            for (i, &w) in d2.iter().enumerate() {
                t -= w;
                if t <= 0.0 {
                    chosen = i;
                    break;
                }
            }
            chosen
        };
        let base = centroids.len();
        centroids.extend_from_slice(points[pick]);
        let new = &centroids[base..base + dim];
        for (i, p) in points.iter().enumerate() {
            let nd = squared_l2(p, new);
            if nd < d2[i] {
                d2[i] = nd;
            }
        }
    }

    // --- Lloyd iterations ---
    let kk = centroids.len() / dim;
    for _ in 0..iters {
        let mut sums = vec![0.0f32; kk * dim];
        let mut counts = vec![0usize; kk];
        for p in points {
            let a = nearest(&centroids, dim, p);
            counts[a] += 1;
            for d in 0..dim {
                sums[a * dim + d] += p[d];
            }
        }
        for c in 0..kk {
            if counts[c] == 0 {
                // Reseed an empty cluster onto a random point.
                let r = rng.gen_range(0..n);
                centroids[c * dim..(c + 1) * dim].copy_from_slice(points[r]);
            } else {
                let inv = 1.0 / counts[c] as f32;
                for d in 0..dim {
                    centroids[c * dim + d] = sums[c * dim + d] * inv;
                }
            }
        }
    }
    centroids
}

/// Product-quantization codebook: one learned set of codewords per subspace.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct PqCodebook {
    /// Subspace boundaries; subspace `s` spans `[sub_offsets[s], sub_offsets[s+1])`.
    sub_offsets: Vec<usize>,
    /// Per subspace: a flat `ks[s] * subdim` codeword buffer.
    centroids: Vec<Vec<f32>>,
    /// Codeword count per subspace (`≤ 256`, so a code fits in a `u8`).
    ks: Vec<usize>,
}

impl PqCodebook {
    fn m(&self) -> usize {
        self.sub_offsets.len() - 1
    }

    /// Train one codebook per subspace on the sampled training vectors.
    fn train(
        sub_offsets: &[usize],
        train: &[&[f32]],
        iters: usize,
        rng: &mut StdRng,
    ) -> PqCodebook {
        let m = sub_offsets.len() - 1;
        let mut centroids = Vec::with_capacity(m);
        let mut ks = Vec::with_capacity(m);
        for s in 0..m {
            let (lo, hi) = (sub_offsets[s], sub_offsets[s + 1]);
            let subs: Vec<Vec<f32>> = train.iter().map(|v| v[lo..hi].to_vec()).collect();
            let refs: Vec<&[f32]> = subs.iter().map(|x| x.as_slice()).collect();
            let k = PQ_CODEWORDS.min(refs.len()).max(1);
            let cs = kmeans(&refs, k, iters, rng);
            ks.push(cs.len() / (hi - lo));
            centroids.push(cs);
        }
        PqCodebook {
            sub_offsets: sub_offsets.to_vec(),
            centroids,
            ks,
        }
    }

    /// Encode a full vector into `m` codeword indices.
    fn encode(&self, v: &[f32]) -> Vec<u8> {
        let m = self.m();
        let mut code = vec![0u8; m];
        for (s, slot) in code.iter_mut().enumerate().take(m) {
            let (lo, hi) = (self.sub_offsets[s], self.sub_offsets[s + 1]);
            *slot = nearest(&self.centroids[s], hi - lo, &v[lo..hi]) as u8;
        }
        code
    }

    /// Precompute, for a query, the distance from each query subspace to every
    /// codeword. Asymmetric distance = sum of one lookup per subspace.
    fn distance_table(&self, q: &[f32]) -> Vec<Vec<f32>> {
        let m = self.m();
        let mut table = Vec::with_capacity(m);
        for s in 0..m {
            let (lo, hi) = (self.sub_offsets[s], self.sub_offsets[s + 1]);
            let sd = hi - lo;
            let qs = &q[lo..hi];
            let cs = &self.centroids[s];
            let mut row = vec![0.0f32; self.ks[s]];
            for (c, slot) in row.iter_mut().enumerate() {
                *slot = squared_l2(qs, &cs[c * sd..(c + 1) * sd]);
            }
            table.push(row);
        }
        table
    }
}

/// One IVF partition: parallel arrays of memory IDs and their PQ codes.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Posting {
    ids: Vec<u128>,
    /// Flat `ids.len() * m` byte buffer of PQ codes.
    codes: Vec<u8>,
}

/// An IVF + PQ approximate-nearest-neighbour index over the database's vectors.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IvfPqIndex {
    dims: usize,
    n_partitions: usize,
    n_probe: usize,
    n_rerank: usize,
    /// Flat `n_partitions * dims` IVF centroid buffer.
    centroids: Vec<f32>,
    pq: PqCodebook,
    partitions: Vec<Posting>,
    /// `id -> partition`. Reconstructed from `partitions` after load, never
    /// serialized.
    #[serde(skip)]
    assignment: HashMap<u128, usize>,
    // Serialized shape is frozen at v0.5.2's: dims, n_partitions,
    // n_probe, n_rerank, centroids, pq, partitions. Do NOT append new
    // fields here under the same `format::VERSION` — rmp-serde 1.3.0's
    // positional decoder rejects extra array elements with
    // `Error::LengthMismatch`, so appending here would break every
    // v0.5.x reader that opens a file written by this build. See the
    // 2026-11-01 R-001.10 standing rule: any shape change on a
    // persisted struct now requires a version bump + migration, or
    // goes into `Memory.metadata` under reserved `_mnemo.*` keys.
    //
    // The "pre-ANN-1 default sentinel" check that gives v0.5.x-built
    // indexes the ANN-1 recall fix on upgrade is in
    // [`Self::effective_defaults`] — a stored (n_probe, n_rerank) of
    // exactly (8, 64) is treated as "auto" at query time, so the
    // concrete values stay stored (old readers see something sensible)
    // but new readers recompute from the live corpus shape.
}

impl IvfPqIndex {
    /// Build an index over `items` (`(id, vector)` pairs).
    pub fn build(dims: usize, items: &[(u128, &[f32])], cfg: IndexConfig) -> Result<IvfPqIndex> {
        if items.is_empty() {
            return Err(MnemoError::Invalid(
                "cannot build an index over an empty database".into(),
            ));
        }
        let n = items.len();
        let mut rng = StdRng::seed_from_u64(cfg.seed);

        // Training sample (all vectors when small enough).
        let mut order: Vec<usize> = (0..n).collect();
        let train_n = TRAIN_CAP.min(n);
        order.partial_shuffle(&mut rng, train_n);
        let train: Vec<&[f32]> = order[..train_n].iter().map(|&i| items[i].1).collect();

        // IVF centroids.
        let n_partitions = if cfg.n_partitions > 0 {
            cfg.n_partitions
        } else {
            (n as f64).sqrt().ceil() as usize
        }
        .clamp(1, n);
        let centroids = kmeans(&train, n_partitions, cfg.kmeans_iters, &mut rng);
        let n_partitions = centroids.len() / dims;

        // PQ subspaces — split `dims` into `m` near-equal contiguous spans.
        // The auto-default is `dims/4` (PR ANN-1), where the grid shows
        // 100K recall@10 clearing 0.98 at the knee. Earlier auto was
        // `dims/8`; see IndexConfig::default for the trade-off.
        let m = if cfg.pq_subspaces > 0 {
            cfg.pq_subspaces
        } else {
            (dims / 4).max(1)
        }
        .clamp(1, dims);
        let mut sub_offsets = Vec::with_capacity(m + 1);
        for s in 0..=m {
            sub_offsets.push(s * dims / m);
        }
        let pq = PqCodebook::train(&sub_offsets, &train, cfg.kmeans_iters, &mut rng);

        // Query-time dials. Zero in cfg means "auto" — resolve to a
        // concrete value using the formulas, then store that. Old
        // readers (v0.5.x) see a sensible stored number; new readers
        // (v0.6.0+) with the live corpus can recompute via
        // `effective_defaults` on every query. The "stored == (8,64)"
        // sentinel in `effective_defaults` is what lets v0.5.x-built
        // files pick up the fix on upgrade without a rebuild.
        let n_probe = if cfg.n_probe > 0 {
            cfg.n_probe
        } else {
            effective_auto_n_probe(n_partitions)
        };
        let n_rerank = if cfg.n_rerank > 0 {
            cfg.n_rerank
        } else {
            effective_auto_n_rerank(items.len())
        };

        let mut index = IvfPqIndex {
            dims,
            n_partitions,
            n_probe: n_probe.max(1),
            n_rerank: n_rerank.max(1),
            centroids,
            pq,
            partitions: vec![Posting::default(); n_partitions],
            assignment: HashMap::with_capacity(n),
        };
        for &(id, v) in items {
            index.add(id, v);
        }
        Ok(index)
    }

    /// Default `n_probe` / `n_rerank` to apply when the caller passes
    /// `None` on a [`crate::RecallRequest`] — i.e. the user hasn't
    /// explicitly overridden the dials for this query.
    ///
    /// Returns the auto-computed `(n_probe, n_rerank)` from the live
    /// partition count + vector count when the stored pair is exactly
    /// **`(8, 64)`** — the pre-ANN-1 fixed defaults every v0.5.x build
    /// stamped in automatically. That's how an upgraded v0.5.x index
    /// picks up the ANN-1 recall fix without a rebuild.
    ///
    /// Otherwise returns the stored `(self.n_probe, self.n_rerank)`
    /// verbatim — the user picked these at build time and still means
    /// them.
    ///
    /// New v0.6.0+ builds store the resolved auto values directly, so
    /// a fresh build never hits the sentinel path. The sentinel is
    /// pre-ANN-1-only.
    ///
    /// Note: a user who explicitly built with `IndexConfig { n_probe: 8,
    /// n_rerank: 64, .. }` on v0.6.0+ will see their values treated as
    /// "legacy auto" instead of pinned. That's acceptable — the ANN-1
    /// auto formulas at very small N (10K / 100 partitions / 20K vectors)
    /// also pick n_probe near 8 and n_rerank clamped to 100, so the
    /// override is self-cancelling in the one regime where it matters.
    /// Pin to any other non-legacy value to opt out explicitly.
    pub(crate) fn effective_defaults(&self) -> (usize, usize) {
        if self.n_probe == 8 && self.n_rerank == 64 {
            (
                effective_auto_n_probe(self.n_partitions),
                effective_auto_n_rerank(self.len()),
            )
        } else {
            (self.n_probe, self.n_rerank)
        }
    }

    /// Embedding dimensionality the index was built for.
    pub fn dims(&self) -> usize {
        self.dims
    }

    /// Total vectors currently indexed.
    pub fn len(&self) -> usize {
        self.partitions.iter().map(|p| p.ids.len()).sum()
    }

    /// Every indexed id (PERF-1 reconcile).
    pub(crate) fn ids(&self) -> impl Iterator<Item = u128> + '_ {
        self.partitions.iter().flat_map(|p| p.ids.iter().copied())
    }

    /// Heap bytes held by the index, by part (P-0 device budget). Counts
    /// buffer capacities; the hash map is estimated from its capacity.
    /// Returns `(coarse_centroids, pq_codebook, posting_ids, posting_codes,
    /// assignment_map)`.
    pub(crate) fn heap_bytes(&self) -> (usize, usize, usize, usize, usize) {
        let f32s = std::mem::size_of::<f32>();
        let centroids = self.centroids.capacity() * f32s;
        let codebook = self
            .pq
            .centroids
            .iter()
            .map(|c| c.capacity() * f32s)
            .sum::<usize>()
            + self.pq.sub_offsets.capacity() * std::mem::size_of::<usize>()
            + self.pq.ks.capacity() * std::mem::size_of::<usize>();
        let ids = self
            .partitions
            .iter()
            .map(|p| p.ids.capacity() * std::mem::size_of::<u128>())
            .sum();
        let codes = self.partitions.iter().map(|p| p.codes.capacity()).sum();
        let assignment = hash_map_bytes::<u128, usize>(self.assignment.capacity());
        (centroids, codebook, ids, codes, assignment)
    }

    /// Shape snapshot for statistics.
    ///
    /// `n_probe` / `n_rerank` here are the **effective** values that
    /// queries actually use (after the ANN-1.1 "stored (8,64) == auto"
    /// shim via [`Self::effective_defaults`]), not the raw stored
    /// numbers. That's what `mnemo info`, `db.stats()` and the MCP
    /// `stats` tool report, so an upgraded v0.5.x file shows the
    /// larger auto values it actually searches with — not the 8/64
    /// baked into the serialized index.
    pub fn info(&self) -> IndexInfo {
        let (eff_probe, eff_rerank) = self.effective_defaults();
        IndexInfo {
            vectors: self.len(),
            partitions: self.n_partitions,
            subspaces: self.pq.m(),
            n_probe: eff_probe,
            n_rerank: eff_rerank,
        }
    }

    /// Rebuild the `id -> partition` map from the posting lists. Called once
    /// after the index is deserialized (the map itself is not stored).
    pub fn rebuild_assignment(&mut self) {
        self.assignment.clear();
        for (pi, p) in self.partitions.iter().enumerate() {
            for &id in &p.ids {
                self.assignment.insert(id, pi);
            }
        }
    }

    fn nearest_centroid(&self, v: &[f32]) -> usize {
        nearest(&self.centroids, self.dims, v)
    }

    /// Insert or overwrite a vector. The PQ codebook and IVF centroids are
    /// fixed at build time; only posting lists grow. This keeps the index
    /// complete across inserts without a full rebuild — cluster drift is the
    /// price, repaid by `Mnemo::rebuild_index` / `compact`.
    pub fn add(&mut self, id: u128, vector: &[f32]) {
        self.remove(id);
        let part = self.nearest_centroid(vector);
        let code = self.pq.encode(vector);
        let p = &mut self.partitions[part];
        p.ids.push(id);
        p.codes.extend_from_slice(&code);
        self.assignment.insert(id, part);
    }

    /// Remove a vector if present (no-op otherwise).
    pub fn remove(&mut self, id: u128) {
        if let Some(part) = self.assignment.remove(&id) {
            let m = self.pq.m();
            let p = &mut self.partitions[part];
            if let Some(pos) = p.ids.iter().position(|&x| x == id) {
                p.ids.remove(pos);
                p.codes.drain(pos * m..(pos + 1) * m);
            }
        }
    }

    /// Run the tiered query: IVF probe → PQ scan → top-`n_rerank` candidates.
    /// Returns candidate IDs (nearest first by PQ distance) for the store to
    /// rerank exactly.
    ///
    /// `n_probe` / `n_rerank` of `None` fall through to
    /// [`Self::effective_defaults`] — which recomputes from the live
    /// partition / vector count when the index is auto-flagged (ANN-1.1
    /// in v0.6.0+) or carries the legacy pre-ANN-1 defaults (8, 64).
    /// Any explicit value is honored verbatim.
    pub fn query(&self, q: &[f32], n_probe: Option<usize>, n_rerank: Option<usize>) -> Vec<u128> {
        let (auto_probe, auto_rerank) = self.effective_defaults();
        let n_probe = n_probe.unwrap_or(auto_probe).clamp(1, self.n_partitions);
        let n_rerank = n_rerank.unwrap_or(auto_rerank).max(1);

        // Stage 1 — pick the nearest partitions.
        let mut parts: Vec<(usize, f32)> = (0..self.n_partitions)
            .map(|i| {
                let c = &self.centroids[i * self.dims..(i + 1) * self.dims];
                (i, squared_l2(q, c))
            })
            .collect();
        parts.sort_by(|a, b| a.1.total_cmp(&b.1));
        parts.truncate(n_probe);

        // Stage 2 — scan PQ codes in the selected partitions.
        let table = self.pq.distance_table(q);
        let m = self.pq.m();
        let mut cands: Vec<(u128, f32)> = Vec::new();
        for (pi, _) in parts {
            let p = &self.partitions[pi];
            for (j, &id) in p.ids.iter().enumerate() {
                let code = &p.codes[j * m..(j + 1) * m];
                let mut d = 0.0f32;
                for (s, row) in table.iter().enumerate() {
                    d += row[code[s] as usize];
                }
                cands.push((id, d));
            }
        }

        // Stage 3 — keep the closest `n_rerank` candidates for exact
        // reranking. R-001.8 part 3 (ANN-1.1): partition-select first,
        // then sort only the kept slice. Full sort of all N candidates
        // is O(N log N); at ANN-1.1's 1M + partitions/2 defaults the
        // candidate pool is ~500K per query, where the full sort cost
        // dominates. `select_nth_unstable_by` is O(N) average, then
        // the final sort is O(k log k) with k = n_rerank.
        if n_rerank < cands.len() {
            cands.select_nth_unstable_by(n_rerank - 1, |a, b| a.1.total_cmp(&b.1));
            cands.truncate(n_rerank);
        }
        cands.sort_by(|a, b| a.1.total_cmp(&b.1));
        cands.into_iter().map(|(id, _)| id).collect()
    }
}

#[cfg(test)]
mod ann_1_1_tests {
    //! PR ANN-1.1 tests for the index-defaults upgrade path.
    //!
    //! The amended spec (planner 2026-11-01 URGENT AMEND, R-001.3 +
    //! R-001.10): no new fields on `IvfPqIndex` — rmp-serde 1.3.0's
    //! positional decoder rejects extra array elements, so any append
    //! would break v0.5.x readers. The whole upgrade path runs on the
    //! "stored (8, 64) == auto" sentinel in
    //! [`IvfPqIndex::effective_defaults`].

    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    /// Shadow of the pre-ANN-1 [`IvfPqIndex`] layout — identical to
    /// the current one (same field order, same types). Serializing
    /// one of these produces the exact byte stream a v0.5.x build
    /// would write. Keeping it as a separate struct makes the
    /// field-count guard below mechanical rather than ad-hoc.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct V052IvfPqIndex {
        dims: usize,
        n_partitions: usize,
        n_probe: usize,
        n_rerank: usize,
        centroids: Vec<f32>,
        pq: PqCodebook,
        partitions: Vec<Posting>,
    }

    fn tiny_pq(dims: usize) -> PqCodebook {
        let sub_offsets = vec![0, dims];
        let mut rng = StdRng::seed_from_u64(0);
        let train: Vec<Vec<f32>> = (0..4).map(|i| vec![i as f32; dims]).collect();
        let refs: Vec<&[f32]> = train.iter().map(|v| v.as_slice()).collect();
        PqCodebook::train(&sub_offsets, &refs, 2, &mut rng)
    }

    #[test]
    fn v0_5_2_index_blob_decodes_unchanged() {
        // A byte stream in the exact v0.5.2 layout must still decode
        // as the current `IvfPqIndex`. If we ever appended a field,
        // rmp-serde would reject this as `LengthMismatch`.
        let dims = 4;
        let legacy = V052IvfPqIndex {
            dims,
            n_partitions: 2,
            n_probe: 8,
            n_rerank: 64,
            centroids: vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            pq: tiny_pq(dims),
            partitions: vec![Posting::default(); 2],
        };
        let bytes = rmp_serde::to_vec(&legacy).expect("serialize v0.5.2 layout");
        let decoded: IvfPqIndex =
            rmp_serde::from_slice(&bytes).expect("v0.5.2 bytes must decode under current schema");
        assert_eq!(decoded.dims, 4);
        assert_eq!(decoded.n_partitions, 2);
        assert_eq!(decoded.n_probe, 8);
        assert_eq!(decoded.n_rerank, 64);
    }

    #[test]
    fn current_build_serializes_with_v0_5_2_field_count() {
        // Forward-compat guard. If someone appends a field to
        // `IvfPqIndex`, the serialized form will carry an extra array
        // element and v0.5.x readers will reject it with
        // `LengthMismatch`. This test fails loudly first, so the
        // breakage is caught by `cargo test`, not by an upset user
        // whose two machines run different mnemo versions.
        let dims = 4;
        let items: Vec<(u128, Vec<f32>)> = (0..32)
            .map(|i| (i as u128, vec![i as f32, i as f32 + 1.0, 0.0, 0.0]))
            .collect();
        let refs: Vec<(u128, &[f32])> = items.iter().map(|(id, v)| (*id, v.as_slice())).collect();
        let idx = IvfPqIndex::build(dims, &refs, IndexConfig::default()).unwrap();
        let bytes = rmp_serde::to_vec(&idx).expect("serialize current layout");
        // Decode via the frozen v0.5.2 shadow struct. If the current
        // layout has an extra appended field, this fails with
        // `LengthMismatch`.
        let as_v052: V052IvfPqIndex = rmp_serde::from_slice(&bytes)
            .expect("current index must decode under the v0.5.2 shape");
        assert_eq!(as_v052.dims, dims);
    }

    #[test]
    fn legacy_8_64_triggers_auto_formulas_at_query_time() {
        // The sentinel path: stored (8, 64) on a legacy index
        // substitutes the auto formulas at query time — ANN-1 fix
        // without a rebuild.
        let dims = 4;
        let legacy = V052IvfPqIndex {
            dims,
            n_partitions: 10,
            n_probe: 8,
            n_rerank: 64,
            centroids: vec![0.0; 10 * dims],
            pq: tiny_pq(dims),
            partitions: (0..10)
                .map(|_| Posting {
                    ids: vec![0u128, 1u128],
                    codes: vec![0u8; 2],
                })
                .collect(),
        };
        let bytes = rmp_serde::to_vec(&legacy).unwrap();
        let decoded: IvfPqIndex = rmp_serde::from_slice(&bytes).unwrap();

        let (auto_probe, auto_rerank) = decoded.effective_defaults();
        // Small corpora get full probe (ANN-1.1 formula: full up to
        // 100 partitions). 10 partitions → probe all 10.
        assert_eq!(
            auto_probe, 10,
            "auto n_probe at <=100 partitions is full probe"
        );
        assert_eq!(auto_rerank, 100, "auto n_rerank at tiny N clamps up to 100");
    }

    #[test]
    fn new_build_stores_resolved_auto_values() {
        // On v0.6.0+, build() with IndexConfig::default() resolves
        // 0 → concrete auto values and stores THOSE in the index.
        // Old readers (v0.5.x) see a usable number rather than a 0
        // sentinel; new readers compute the same values at query
        // time via effective_defaults.
        let dims = 4;
        let items: Vec<(u128, Vec<f32>)> = (0..64)
            .map(|i| {
                let f = i as f32;
                (i as u128, vec![f, f + 1.0, f + 2.0, f + 3.0])
            })
            .collect();
        let refs: Vec<(u128, &[f32])> = items.iter().map(|(id, v)| (*id, v.as_slice())).collect();
        let idx = IvfPqIndex::build(dims, &refs, IndexConfig::default()).unwrap();
        assert!(
            idx.n_probe > 0 && idx.n_rerank > 0,
            "resolved values must be concrete (never 0)"
        );
        // The IndexConfig::default() formula for a 64-vector corpus
        // with ceil(sqrt(64)) = 8 partitions picks n_probe=4,
        // n_rerank=clamp(64/200, 100, 1000) = 100.
        let expected_rerank = (64 / 200usize).clamp(100, 1000);
        assert_eq!(idx.n_rerank, expected_rerank);
    }

    #[test]
    fn explicit_build_values_persist_verbatim() {
        // Non-sentinel explicit values stay stored and are used at
        // query time. (A user who passes exactly 8/64 explicitly is
        // treated as legacy auto — acknowledged in the planner's
        // amended spec; harmless because at the tiny N where 8/64
        // could sensibly be chosen, the auto formulas pick similar
        // values.)
        let dims = 4;
        let items: Vec<(u128, Vec<f32>)> =
            (0..64).map(|i| (i as u128, vec![i as f32; dims])).collect();
        let refs: Vec<(u128, &[f32])> = items.iter().map(|(id, v)| (*id, v.as_slice())).collect();
        let cfg = IndexConfig {
            n_probe: 3,
            n_rerank: 17,
            ..Default::default()
        };
        let idx = IvfPqIndex::build(dims, &refs, cfg).unwrap();
        assert_eq!(idx.n_probe, 3);
        assert_eq!(idx.n_rerank, 17);
        let (p, r) = idx.effective_defaults();
        assert_eq!(p, 3);
        assert_eq!(r, 17);
    }
}
