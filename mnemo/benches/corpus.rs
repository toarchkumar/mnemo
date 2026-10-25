//! Shared corpus-setup helpers for the Phase 6 speed benches.
//!
//! Criterion compiles each `[[bench]]` entry as its own binary, so this
//! module is pulled in via `#[path = "corpus.rs"] mod corpus;` at the
//! top of each bench file rather than through the usual module tree.
//! That means it must stand on its own with no `super::` references.
//!
//! What's in here, in one line each:
//!
//! * [`PASSPHRASE`] — the single string used to open every bench file,
//!   so a cached corpus from one run opens cleanly in the next.
//! * [`FAST_KDF_CFG`] / [`SECURE_KDF_CFG`] — [`MnemoConfig`] presets so
//!   recall / write benches don't pay Argon2id on every setup while the
//!   dedicated open-latency bench still uses production KDF params.
//! * [`gen_clustered_vectors`] — reproducible clustered 384-dim vectors,
//!   the kind of data that lets IVF+PQ actually show its advantage over
//!   exact scan.
//! * [`setup_cached_corpus`] — builds (or reuses) a corpus file under
//!   `target/benches/`; building is amortized across every bench run.

use std::fs;
use std::path::{Path, PathBuf};

use mnemo::{IndexConfig, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};

/// Passphrase used by every bench-created file. Not a secret; the files
/// are throwaway and live in `target/`.
#[allow(dead_code)]
pub const PASSPHRASE: &str = "bench-pw";

/// Embedding dimensionality used across the benches. Matches
/// `sentence-transformers/all-MiniLM-L6-v2`, the model recorded on the
/// dogfood manifest and the one the ranking harness uses.
#[allow(dead_code)]
pub const DIMENSIONS: usize = 384;

/// Fast KDF config — only safe for throwaway bench files. Keeps setup
/// cheap so write/recall benches aren't dominated by Argon2id. The
/// dedicated `open_latency` bench uses `SECURE_KDF_CFG` instead.
#[allow(dead_code)]
pub fn fast_kdf_cfg(dimensions: usize) -> MnemoConfig {
    MnemoConfig {
        dimensions,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

/// Production KDF config — what a real user's file is encrypted with.
/// Only used by `open_latency` so we can report the actual cost split.
#[allow(dead_code)]
pub fn secure_kdf_cfg(dimensions: usize) -> MnemoConfig {
    MnemoConfig {
        dimensions,
        kdf: KdfParams::secure(),
        ..Default::default()
    }
}

/// Generate `n` clustered 384-dim vectors.
///
/// `k_clusters` cluster centres are drawn uniformly on the unit sphere;
/// each vector is a centre plus Gaussian noise, renormalized back to
/// unit length. Reproducible from `seed` — same seed, byte-identical
/// output across runs and machines, which keeps a cached corpus
/// portable (if size and seed match, the file is the same).
///
/// The resulting distribution is *locality-friendly*: IVF+PQ learns the
/// cluster structure during build and `n_probe < k_clusters` becomes
/// meaningfully faster than exact scan. Uniform random vectors wouldn't
/// show that advantage.
#[allow(dead_code)]
pub fn gen_clustered_vectors(n: usize, dims: usize, k_clusters: usize, seed: u64) -> Vec<Vec<f32>> {
    // Hand-rolled splitmix64 → xoroshiro128+ because the `rand` crate's
    // RNG API moved between 0.8 and 0.9 and we're pinned to 0.8.6; a
    // tiny deterministic RNG here keeps the benches independent of
    // whichever `rand` the engine is pinned to.
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    let mut next_u64 = || -> u64 {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    // Box-Muller normal sample from two uniforms in (0, 1].
    let mut next_normal = || -> f32 {
        let u1 = ((next_u64() >> 11) as f64 / (1u64 << 53) as f64).max(1e-12);
        let u2 = (next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        let r = (-2.0 * u1.ln()).sqrt();
        (r * (2.0 * std::f64::consts::PI * u2).cos()) as f32
    };

    let normalize = |v: &mut [f32]| {
        let mut norm = 0.0f32;
        for x in v.iter() {
            norm += x * x;
        }
        norm = norm.sqrt();
        if norm == 0.0 {
            // Degenerate: nudge the first dim so the engine's all-zero
            // guard doesn't trip. Vanishingly rare at 384 dims.
            v[0] = 1.0;
        } else {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    };

    // Cluster centres on the unit sphere.
    let mut centres: Vec<Vec<f32>> = Vec::with_capacity(k_clusters);
    for _ in 0..k_clusters {
        let mut c = (0..dims).map(|_| next_normal()).collect::<Vec<_>>();
        normalize(&mut c);
        centres.push(c);
    }

    // Noise scale: small enough that cluster assignment survives, large
    // enough that same-cluster vectors aren't identical. 0.1 is a
    // reasonable realistic value — tunes to roughly 0.8-0.95 cosine
    // similarity within a cluster.
    const NOISE_SIGMA: f32 = 0.1;

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let cluster = i % k_clusters;
        let mut v: Vec<f32> = centres[cluster]
            .iter()
            .map(|&c| c + NOISE_SIGMA * next_normal())
            .collect();
        normalize(&mut v);
        out.push(v);
    }
    out
}

/// Return the on-disk path to a cached corpus of exactly `n` memories
/// with `dimensions`-dim clustered vectors, built with fast KDF.
///
/// Uses `target/benches/corpus_<n>_<dims>d.mnemo` so the file survives
/// across bench runs and is reused by every bench that needs that
/// combination. The first build is slow (encryption + flush of every
/// insert); subsequent runs on the same machine open it in milliseconds.
///
/// On a dimension mismatch the cached file is deleted and rebuilt;
/// that would only happen if someone bumps `DIMENSIONS` here.
#[allow(dead_code)]
pub fn setup_cached_corpus(n: usize, dimensions: usize) -> PathBuf {
    let base = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    let dir = PathBuf::from(base).join("benches");
    fs::create_dir_all(&dir).expect("create target/benches directory");
    let path = dir.join(format!("corpus_{n}_{dimensions}d.mnemo"));

    if path.exists() {
        // Quick dim probe: open and check, delete and rebuild on
        // mismatch rather than silently returning a corrupt corpus.
        match Mnemo::open(&path, PASSPHRASE) {
            Ok(db) if db.dimensions() == dimensions => return path,
            Ok(_) => {
                let _ = fs::remove_file(&path);
            }
            Err(_) => {
                let _ = fs::remove_file(&path);
            }
        }
    }

    build_corpus(&path, n, dimensions);
    path
}

/// Create a fresh corpus at `path`, inserts, builds an IVF+PQ index
/// in-place, and flushes once at the end. The cached file always
/// ships an index so recall benches don't rebuild it; the exact-scan
/// bench works around this by opening a cheap copy and calling
/// [`Mnemo::drop_index`] in-memory, which doesn't touch the file.
#[allow(dead_code)]
pub fn build_corpus(path: &Path, n: usize, dimensions: usize) {
    let cfg = fast_kdf_cfg(dimensions);
    let mut db = Mnemo::create(path, PASSPHRASE, cfg).expect("create bench corpus file");
    let k_clusters = ((n as f64).sqrt().ceil() as usize).max(16);
    let vectors = gen_clustered_vectors(n, dimensions, k_clusters, 0xBEEF_C0DE);
    for (i, v) in vectors.into_iter().enumerate() {
        let content = format!("bench memory #{i}");
        let m = Memory::new(content, MemoryType::Semantic, v);
        db.remember(m).expect("remember bench memory");
    }
    db.flush().expect("flush bench corpus");
    db.build_index_with(IndexConfig::default())
        .expect("build IVF+PQ index on bench corpus");
    db.flush().expect("flush after index build");
}

/// Open a cached corpus and verify the IVF+PQ index is present. If
/// absent (older cached file from a corpus_rs that didn't build one),
/// rebuild in place. Returns the opened handle, ready for recall.
#[allow(dead_code)]
pub fn open_corpus_with_index(path: &Path) -> Mnemo {
    let mut db = Mnemo::open(path, PASSPHRASE).expect("open bench corpus");
    if db.stats().expect("stats").index.is_none() {
        db.build_index_with(IndexConfig::default())
            .expect("build IVF+PQ index on bench corpus");
        db.flush().expect("flush after index build");
    }
    db
}

/// Open a *copy* of the cached corpus and unload its in-memory index.
/// The cached file keeps its on-disk index intact so later runs don't
/// rebuild. The copy is written to a tempdir the caller owns via the
/// returned `TempDir` guard; drop it to clean up.
///
/// `drop_index()` is in-memory only; we deliberately *don't* flush, so
/// the copy on disk still has the index (it just isn't used by the
/// returned handle). The copy itself gets removed with the TempDir.
#[allow(dead_code)]
pub fn open_corpus_exact_only(cached: &Path) -> (tempfile::TempDir, Mnemo) {
    let td = tempfile::TempDir::new().expect("tempdir for exact-scan corpus copy");
    let copy = td.path().join("corpus.mnemo");
    std::fs::copy(cached, &copy).expect("copy cached corpus");
    let mut db = Mnemo::open(&copy, PASSPHRASE).expect("open corpus copy");
    db.drop_index();
    (td, db)
}
