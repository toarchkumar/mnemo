//! Writes the v0.20.2 merge fixtures with the published v0.20.2 engine (see
//! Cargo.toml). Four dimensions, the fast KDF, passphrase
//! `v0-20-2-fixture-passphrase` (a test value that opens nothing else):
//!
//! - `plain_base.mnemo`: a manifest and three facts, written by v0.20.2 (no
//!   clock stamps); `plain_deleted.mnemo`: a copy with `fact b` deleted, a
//!   deleted row and no tombstone;
//! - `edited.mnemo`: a copy of `edited.in.mnemo` (written by v0.21, its fact
//!   stamped) with that fact edited in place by v0.20.2, which keeps the
//!   stamp it found;
//! - `compacted.mnemo`: a copy of `compacted.in.mnemo` (v0.21 deleted a fact
//!   there and wrote its tombstone) compacted by v0.20.2, which writes the
//!   tombstone's row back as a fact;
//! - `keyed.mnemo` (RB-020.1): a manifest, key `k` with one version, key
//!   `k2` with two, and a plain fact, as v0.20.2 writes them.
//!
//! `cargo run --locked -- keyed` writes `keyed.mnemo` only, leaving the
//! other files as they are.

use mnemo_engine::{KdfParams, Memory, MemoryPatch, MemoryType, Mnemo, MnemoConfig};
use std::path::Path;

const PASSPHRASE: &str = "v0-20-2-fixture-passphrase";

fn v(x: f32) -> Vec<f32> {
    vec![x, 1.0, 0.5, 0.25]
}

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn keyed(dir: &Path) {
    let at = dir.join("keyed.mnemo");
    let _ = std::fs::remove_file(&at);
    let mut db = Mnemo::create(&at, PASSPHRASE, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    for (key, text, x) in [
        ("k", "k as v0.20.2 wrote it", 1.0),
        ("k2", "k2 first, by v0.20.2", 2.0),
        ("k2", "k2 second, by v0.20.2", 3.0),
    ] {
        db.remember_keyed(Memory::new(text, MemoryType::Semantic, v(x)), key)
            .unwrap();
    }
    db.remember(Memory::new("plain, by v0.20.2", MemoryType::Semantic, v(4.0)))
        .unwrap();
    db.close().unwrap();
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if std::env::args().nth(1).as_deref() == Some("keyed") {
        keyed(&dir);
        println!("wrote keyed.mnemo in {}", dir.display());
        return;
    }
    let at = |n: &str| dir.join(n);
    for n in ["plain_base.mnemo", "plain_deleted.mnemo", "edited.mnemo", "compacted.mnemo"] {
        let _ = std::fs::remove_file(at(n));
    }

    let mut db = Mnemo::create(at("plain_base.mnemo"), PASSPHRASE, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    let mut ids = Vec::new();
    for (i, t) in ["fact a", "fact b", "fact c"].iter().enumerate() {
        ids.push(
            db.remember(Memory::new(*t, MemoryType::Semantic, v(i as f32 + 1.0)))
                .unwrap(),
        );
    }
    db.close().unwrap();
    std::fs::copy(at("plain_base.mnemo"), at("plain_deleted.mnemo")).unwrap();
    let mut db = Mnemo::open(at("plain_deleted.mnemo"), PASSPHRASE).unwrap();
    db.delete(&ids[1]).unwrap();
    db.close().unwrap();

    std::fs::copy(at("edited.in.mnemo"), at("edited.mnemo")).unwrap();
    let mut db = Mnemo::open(at("edited.mnemo"), PASSPHRASE).unwrap();
    let id = db
        .memories()
        .unwrap()
        .into_iter()
        .find(|m| m.content == "stamped by v0.21")
        .expect("the v0.21 input")
        .id;
    db.update(
        &id,
        MemoryPatch {
            content: Some("edited by v0.20.2".into()),
            vector: Some(v(9.0)),
            ..Default::default()
        },
    )
    .unwrap();
    db.close().unwrap();

    std::fs::copy(at("compacted.in.mnemo"), at("compacted.mnemo")).unwrap();
    Mnemo::compact_file(at("compacted.mnemo").to_str().unwrap(), PASSPHRASE).unwrap();
    keyed(&dir);
    println!("wrote the v0.20.2 fixtures in {}", dir.display());
}
