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
    /// Number of PQ subspaces. `0` means auto (≈ 8 dims per subspace).
    pub pq_subspaces: usize,
    /// Partitions scanned per query (the IVF accuracy/speed dial).
    pub n_probe: usize,
    /// Candidates handed back for exact reranking (the PQ accuracy dial).
    pub n_rerank: usize,
    /// Lloyd-iteration count for every k-means run.
    pub kmeans_iters: usize,
    /// RNG seed for reproducible builds.
    pub seed: u64,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            n_partitions: 0,
            pq_subspaces: 0,
            n_probe: 8,
            n_rerank: 64,
            kmeans_iters: 25,
            seed: DEFAULT_SEED,
        }
    }
}

/// A read-only snapshot of an index's shape, surfaced via `Mnemo::stats`.
#[derive(Clone, Copy, Debug)]
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
    fn train(sub_offsets: &[usize], train: &[&[f32]], iters: usize, rng: &mut StdRng) -> PqCodebook {
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
        PqCodebook { sub_offsets: sub_offsets.to_vec(), centroids, ks }
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
        let m = if cfg.pq_subspaces > 0 {
            cfg.pq_subspaces
        } else {
            (dims / 8).max(1)
        }
        .clamp(1, dims);
        let mut sub_offsets = Vec::with_capacity(m + 1);
        for s in 0..=m {
            sub_offsets.push(s * dims / m);
        }
        let pq = PqCodebook::train(&sub_offsets, &train, cfg.kmeans_iters, &mut rng);

        let mut index = IvfPqIndex {
            dims,
            n_partitions,
            n_probe: cfg.n_probe.max(1),
            n_rerank: cfg.n_rerank.max(1),
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

    /// Embedding dimensionality the index was built for.
    pub fn dims(&self) -> usize {
        self.dims
    }

    /// Total vectors currently indexed.
    pub fn len(&self) -> usize {
        self.partitions.iter().map(|p| p.ids.len()).sum()
    }

    /// Default partitions probed per query.
    pub fn n_probe(&self) -> usize {
        self.n_probe
    }

    /// Default candidates reranked per query.
    pub fn n_rerank(&self) -> usize {
        self.n_rerank
    }

    /// Shape snapshot for statistics.
    pub fn info(&self) -> IndexInfo {
        IndexInfo {
            vectors: self.len(),
            partitions: self.n_partitions,
            subspaces: self.pq.m(),
            n_probe: self.n_probe,
            n_rerank: self.n_rerank,
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
    /// rerank exactly. `n_probe`/`n_rerank` of `None` use the build defaults.
    pub fn query(
        &self,
        q: &[f32],
        n_probe: Option<usize>,
        n_rerank: Option<usize>,
    ) -> Vec<u128> {
        let n_probe = n_probe.unwrap_or(self.n_probe).clamp(1, self.n_partitions);
        let n_rerank = n_rerank.unwrap_or(self.n_rerank).max(1);

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

        // Stage 3 — keep the closest candidates for exact reranking.
        cands.sort_by(|a, b| a.1.total_cmp(&b.1));
        cands.truncate(n_rerank);
        cands.into_iter().map(|(id, _)| id).collect()
    }
}
