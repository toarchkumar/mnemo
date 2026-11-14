//! 12.3 step 5 (v0.15): the built-in embedder (`embed-candle`), as a library.
//!
//! The tests that need the model read its three files from
//! `MNEMO_TEST_MODEL_DIR` (CI's `builtin embedder` job fetches them with
//! `mnemo embedder install`, into a cache it keeps), and say they skipped
//! when it isn't set.
#![cfg(feature = "embed-candle")]

use std::path::{Path, PathBuf};

use mnemo_engine::builtin::{self, FILES};
use mnemo_engine::{
    BuiltinEmbedder, Embedder, KdfParams, MemoryType, Mnemo, MnemoConfig, RecallRequest,
};
use tempfile::tempdir;

const PASS: &str = "builtin-embedder-test-passphrase";

fn model_dir() -> Option<PathBuf> {
    match std::env::var_os("MNEMO_TEST_MODEL_DIR") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => {
            eprintln!("skipped: set MNEMO_TEST_MODEL_DIR to a directory with the model's files");
            None
        }
    }
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

/// The first six components from Hugging Face `transformers` on the same
/// files (CLS pooling, normalised), for the documents "The MSRV is 1.92" and
/// "hello world" and the query "what is the MSRV" with arctic's prefix.
const REFERENCE: [[f32; 6]; 3] = [
    [
        -0.006_875_1,
        0.016_462_5,
        -0.007_606_2,
        -0.015_903_2,
        -0.035_247_6,
        -0.011_608_4,
    ],
    [
        0.009_263_7,
        0.061_961_3,
        0.019_606,
        0.009_086_8,
        -0.011_414_5,
        -0.035_448_5,
    ],
    [
        0.008_432_2,
        0.087_133_3,
        0.014_042_6,
        -0.058_395_6,
        0.012_2,
        -0.040_384_4,
    ],
];

#[test]
fn it_embeds_as_the_reference_does() {
    let Some(dir) = model_dir() else { return };
    let e = BuiltinEmbedder::from_dir(&dir).unwrap();
    assert!(e.is_builtin());
    assert_eq!(e.identity(), builtin::identity());
    let docs = e.embed(&["The MSRV is 1.92", "hello world"]).unwrap();
    let query = e.embed_query(&["what is the MSRV"]).unwrap();
    for (v, r) in docs.iter().chain(&query).zip(&REFERENCE) {
        assert_eq!(v.len(), 384);
        assert!(close(&v[..6], r), "{:?} vs {r:?}", &v[..6]);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }
    // Batching doesn't change a vector (padding is masked).
    let alone = e.embed(&["hello world"]).unwrap();
    assert!(close(&alone[0], &docs[1]));
    // An empty text still embeds (just the special tokens).
    assert_eq!(e.embed(&[""]).unwrap()[0].len(), 384);
}

#[test]
fn recall_is_identifier_aware() {
    let Some(dir) = model_dir() else { return };
    let e = BuiltinEmbedder::from_dir(&dir).unwrap();
    assert_eq!(
        e.query_keywords("what does `snapshot_list` return")
            .as_deref(),
        Some("snapshot_list")
    );
    assert_eq!(
        e.query_keywords("who is the product for").as_deref(),
        Some("")
    );

    let tmp = tempdir().unwrap();
    let mut db = Mnemo::create(
        tmp.path().join("b.mnemo"),
        PASS,
        MnemoConfig {
            dimensions: 384,
            kdf: KdfParams::fast(),
            ..Default::default()
        },
    )
    .unwrap();
    db.set_embedder(e).unwrap();
    for t in [
        "snapshot_list returns every snapshot, newest first",
        "The release workflow needs one approval click from the user",
        "Readers keep the intent lock while a writer commits",
        "The MSRV is 1.92, set by candle's dependencies",
    ] {
        db.remember_text(t, MemoryType::Semantic).unwrap();
    }
    let embedder = db.embedder_arc().unwrap();
    let mut top = |q: &str| {
        let req = RecallRequest::from_text(embedder.as_ref(), q)
            .unwrap()
            .track_access(false);
        db.recall(&req).unwrap()[0].memory.content.clone()
    };
    assert!(top("what does `snapshot_list` return").starts_with("snapshot_list"));
    assert!(top("who approves a release").contains("approval click"));
    assert!(top("what is the minimum Rust version").contains("MSRV"));
    // The file records the built-in identity.
    assert_eq!(db.embedder_identity().unwrap(), Some(builtin::identity()));
}

/// The model's files copied into a fresh directory, with `config.json`
/// changed (a trailing newline) when `change` is set.
fn copy_model(from: &Path, to: &Path, change: bool) {
    std::fs::create_dir_all(to).unwrap();
    for f in FILES {
        std::fs::copy(from.join(f.name), to.join(f.name)).unwrap();
    }
    if change {
        let mut c = std::fs::read(to.join("config.json")).unwrap();
        c.push(b'\n');
        std::fs::write(to.join("config.json"), c).unwrap();
    }
}

#[test]
fn a_directory_of_other_files_has_an_identity_of_its_own() {
    let Some(dir) = model_dir() else { return };
    let tmp = tempdir().unwrap();
    let other = tmp.path().join("other");
    copy_model(&dir, &other, true);
    let e = BuiltinEmbedder::from_dir(&other).unwrap();
    assert!(!e.is_builtin());
    let name = e.identity().name;
    assert!(name.starts_with("local:config.json="), "{name}");
    // The other two files' pinned hashes are in it.
    assert!(name.contains(&format!("tokenizer.json={}", FILES[1].sha256)));
    assert!(name.contains(&format!("model.safetensors={}", FILES[2].sha256)));
    assert!(!name.contains(FILES[0].sha256));
    // Not the built-in: recall keeps the default text weight.
    assert_eq!(e.query_keywords("what does `snapshot_list` return"), None);
    // It embeds all the same.
    assert_eq!(e.embed(&["x"]).unwrap()[0].len(), 384);

    // A file that records the built-in refuses it.
    let mut db = Mnemo::create(
        tmp.path().join("b.mnemo"),
        PASS,
        MnemoConfig {
            dimensions: 384,
            kdf: KdfParams::fast(),
            ..Default::default()
        },
    )
    .unwrap();
    db.set_embedder(BuiltinEmbedder::from_dir(&dir).unwrap())
        .unwrap();
    db.remember_text("a fact", MemoryType::Semantic).unwrap();
    db.close().unwrap();
    let mut db = Mnemo::open(tmp.path().join("b.mnemo"), PASS).unwrap();
    assert!(db.set_embedder(e).is_err());
}

#[test]
fn a_missing_file_is_a_clear_error() {
    let tmp = tempdir().unwrap();
    let e = BuiltinEmbedder::from_dir(tmp.path())
        .unwrap_err()
        .to_string();
    assert!(e.contains("can't read") && e.contains("config.json"), "{e}");
}
