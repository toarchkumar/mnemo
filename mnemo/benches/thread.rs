//! Phase 12.7d speed benches for the thread layers.
//!
//! Three entries, all with a trivial in-process embedder (a canned
//! 384-dim vector), so they measure the engine's cost rather than a
//! model's:
//!
//! 1. `thread/ingest_1000_turns`: `ingest_thread` of 1,000 ~200-byte
//!    turns into an empty thread, plus the flush. Throughput is reported
//!    in turns per second.
//! 2. `thread/essence_render_1000_items`: `essence(thread, 6144)` over a
//!    thread holding 1,000 essence items (700 section items across the
//!    seven sections, 300 timeline entries).
//! 3. `thread/about_with_essence_1000_items`: the full `about` briefing,
//!    `about()` plus `about_essence()`, on a file with a manifest and
//!    1,000 project-scope essence items.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use mnemo_engine::{
    Embedder, EmbedderIdentity, EssenceScope, EssenceSection, Memory, Mnemo, Result, ThreadTurn,
};
use tempfile::TempDir;

#[path = "corpus.rs"]
mod corpus;
use corpus::{fast_kdf_cfg, gen_clustered_vectors, DIMENSIONS, PASSPHRASE};

const ITEMS: usize = 1_000;
const TURNS: usize = 1_000;

struct CannedEmbedder {
    vector: Vec<f32>,
}

impl Embedder for CannedEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "bench-canned-embedder".into(),
            dimensions: self.vector.len(),
            normalize: true,
        }
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| self.vector.clone()).collect())
    }
}

fn canned_vector() -> Vec<f32> {
    gen_clustered_vectors(1, DIMENSIONS, 1, 0x7E8D_0001)
        .into_iter()
        .next()
        .unwrap()
}

fn make_db(td: &TempDir) -> Mnemo {
    let path = td.path().join("thread-bench.mnemo");
    let mut db = Mnemo::create(&path, PASSPHRASE, fast_kdf_cfg(DIMENSIONS)).expect("create");
    db.remember(Memory::scaffold_manifest(DIMENSIONS))
        .expect("manifest");
    db.set_embedder(CannedEmbedder {
        vector: canned_vector(),
    })
    .expect("set_embedder");
    db
}

const SECTIONS: [EssenceSection; 7] = [
    EssenceSection::Intent,
    EssenceSection::Conventions,
    EssenceSection::Decisions,
    EssenceSection::State,
    EssenceSection::Open,
    EssenceSection::Next,
    EssenceSection::Lessons,
];

/// Fill `scope` with `ITEMS` essence items: 70% section items spread
/// over the seven sections, 30% timeline entries.
fn fill_essence(db: &mut Mnemo, scope: &EssenceScope) {
    for i in 0..ITEMS {
        if i % 10 < 7 {
            let section = SECTIONS[i % SECTIONS.len()];
            let text =
                format!("item {i:04}: a one-line essence entry of typical length for a bench");
            db.essence_add(scope.clone(), section, &text, None)
                .expect("essence_add");
        } else {
            let text =
                format!("milestone {i:04}: shipped a thing, measured it, and wrote down why");
            db.essence_append_timeline(scope.clone(), &text, None)
                .expect("essence_append_timeline");
        }
    }
    db.flush().expect("flush");
}

fn bench_ingest(c: &mut Criterion) {
    let turns: Vec<ThreadTurn> = (0..TURNS)
        .map(|i| ThreadTurn {
            turn_id: format!("turn-{i:05}"),
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!("turn {i:05}: {}", "lorem ipsum dolor sit amet ".repeat(7)),
            timestamp: Some(1_700_000_000 + i as i64),
        })
        .collect();

    let mut group = c.benchmark_group("thread");
    group.sample_size(10);
    group.throughput(Throughput::Elements(TURNS as u64));
    group.bench_function("ingest_1000_turns", |b| {
        b.iter_batched(
            || {
                let td = TempDir::new().expect("tempdir");
                let db = make_db(&td);
                (td, db)
            },
            |(_td, mut db)| {
                let report = db.ingest_thread("bench", &turns).expect("ingest");
                db.flush().expect("flush");
                black_box(report);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn bench_render(c: &mut Criterion) {
    let td = TempDir::new().expect("tempdir");
    let mut db = make_db(&td);
    let scope = EssenceScope::Thread("bench".into());
    fill_essence(&mut db, &scope);

    let mut group = c.benchmark_group("thread");
    group.sample_size(20);
    group.bench_function("essence_render_1000_items", |b| {
        b.iter(|| black_box(db.essence(scope.clone(), 6144).expect("render")));
    });
    group.finish();
}

fn bench_about(c: &mut Criterion) {
    let td = TempDir::new().expect("tempdir");
    let mut db = make_db(&td);
    fill_essence(&mut db, &EssenceScope::Project);

    let mut group = c.benchmark_group("thread");
    group.sample_size(20);
    group.bench_function("about_with_essence_1000_items", |b| {
        b.iter(|| {
            let records = db.about().expect("about");
            let essence = db.about_essence().expect("about_essence");
            black_box((records, essence))
        });
    });
    group.finish();
}

criterion_group!(thread_benches, bench_ingest, bench_render, bench_about);
criterion_main!(thread_benches);
