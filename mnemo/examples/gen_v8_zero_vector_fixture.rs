//! Writes `tests/fixtures/v8/v8_zero_vectors.mnemo`: a v8 file holding two
//! all-zero vectors, as files written before the zero-vector rule (v0.5,
//! Phase 3 PR 10a) can. One is a legacy scaffold manifest (the manifest
//! used a zero vector before it switched to the sentinel spike), the other
//! an ordinary memory. Three ordinary memories with real vectors sit
//! beside them. Passphrase `v8-fixture-passphrase`, fast KDF, 8 dimensions.
//!
//! The published v0.12.3 engine can't write a zero vector (every write
//! goes through the check), so this uses the `__put_unvalidated_for_testing`
//! hook (`dev-hooks`, which examples get). It ran at the commit that added
//! it (v0.13 step 2a), the last state of the engine that still creates v8
//! files; the v8 writer is unchanged since v0.12.3.
//!
//!   cargo run --example gen_v8_zero_vector_fixture

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/v8/v8_zero_vectors.mnemo");
    let _ = std::fs::remove_file(&path);
    let mut db = Mnemo::create(
        &path,
        "v8-fixture-passphrase",
        MnemoConfig {
            format_version: 8,
            dimensions: 8,
            kdf: KdfParams::fast(),
            ..Default::default()
        },
    )
    .expect("create v8");

    let mut manifest = Memory::scaffold_manifest(8);
    manifest.vector = vec![0.0; 8];
    db.__put_unvalidated_for_testing(manifest)
        .expect("legacy manifest");

    let zero = Memory::new(
        "A note written with an all-zero vector",
        MemoryType::Semantic,
        vec![0.0; 8],
    );
    db.__put_unvalidated_for_testing(zero)
        .expect("legacy zero vector");

    for i in 0..3 {
        let mut v = vec![0.05f32; 8];
        v[i] = 1.0;
        db.remember(Memory::new(
            format!("ordinary memory {i}"),
            MemoryType::Episodic,
            v,
        ))
        .expect("remember");
    }
    db.flush().expect("flush");
    drop(db);
    println!("wrote {}", path.display());
}
