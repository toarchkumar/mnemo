//! FMT-9e: v9 thread-turn records are zstd-compressed before encryption.
//! Every test asks for its format explicitly.

use mnemo_engine::{
    Embedder, EmbedderIdentity, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, ThreadTurn,
};
use tempfile::tempdir;

const DIMS: usize = 4;

fn cfg(version: u16) -> MnemoConfig {
    MnemoConfig {
        dimensions: DIMS,
        kdf: KdfParams::fast(),
        format_version: version,
        ..Default::default()
    }
}

struct LenEmbedder;
impl Embedder for LenEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "len".into(),
            dimensions: DIMS,
            normalize: false,
        }
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| vec![1.0 + t.len() as f32, 0.5, 0.25, 0.125])
            .collect())
    }
}

/// Prose-like text that compresses about as well as real turns do.
fn prose(i: usize, len: usize) -> String {
    const WORDS: [&str; 12] = [
        "the", "agent", "decided", "to", "flush", "release", "v0.11.0", "after", "tests", "pass",
        "because", "recall",
    ];
    let mut s = format!("turn {i}:");
    let mut k = (i as u64).wrapping_mul(2_654_435_761).wrapping_add(7);
    while s.len() < len {
        k = k
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        s.push(' ');
        s.push_str(WORDS[((k >> 33) % WORDS.len() as u64) as usize]);
    }
    s.truncate(len);
    s
}

fn turn(i: usize, content: String) -> ThreadTurn {
    ThreadTurn {
        turn_id: format!("t{i}"),
        role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
        content,
        timestamp: Some(1_700_000_000 + i as i64),
    }
}

fn open_db(path: &std::path::Path, version: u16) -> Mnemo {
    let mut db = Mnemo::create(path, "pw", cfg(version)).unwrap();
    db.set_embedder(LenEmbedder).unwrap();
    db
}

#[test]
fn long_turns_are_stored_compressed_and_read_back_exactly() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = open_db(&path, 9);
    let turns: Vec<ThreadTurn> = (0..20).map(|i| turn(i, prose(i, 3_000))).collect();
    db.ingest_thread("chat", &turns).unwrap();
    // Before the first flush the records wait in memory, already compressed.
    for m in db.thread_export("chat").unwrap() {
        let (len, zstd) = db.__record_storage(&m.id).unwrap();
        assert!(zstd, "a 3 KB turn is compressed");
        assert!((len as usize) < m.content.len() / 2, "stored {len} B");
    }
    db.close().unwrap();
    drop(db);

    let mut db = Mnemo::open(&path, "pw").unwrap();
    let back = db.thread_export_turns("chat").unwrap();
    assert_eq!(back.len(), turns.len());
    for (a, b) in turns.iter().zip(&back) {
        assert_eq!(a.content, b.content);
        assert_eq!(a.turn_id, b.turn_id);
    }
    assert_eq!(db.verify().unwrap(), db.len());
}

#[test]
fn a_chunked_turn_compresses_each_chunk_and_packs_them() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = open_db(&path, 9);
    let long = prose(1, 30_000);
    db.ingest_thread("chat", &[turn(1, long.clone())]).unwrap();
    db.close().unwrap();
    let chunks = db.thread_export("chat").unwrap();
    assert_eq!(chunks.len(), 4, "30,000 bytes in 7,900-byte chunks");
    let mut pages = std::collections::HashSet::new();
    for m in &chunks {
        let (_, zstd) = db.__record_storage(&m.id).unwrap();
        assert!(zstd);
        let (page, count, _) = db.__record_location(&m.id).unwrap();
        assert_eq!(count, 1);
        pages.insert(page);
    }
    assert!(
        pages.len() < chunks.len(),
        "compressed chunks share packed pages ({} pages for {} chunks)",
        pages.len(),
        chunks.len()
    );
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.thread_export_turns("chat").unwrap()[0].content, long);
}

#[test]
fn short_turns_facts_and_v8_files_stay_plain() {
    let dir = tempdir().unwrap();
    let mut db = open_db(&dir.path().join("v9.mnemo"), 9);
    db.ingest_thread("chat", &[turn(0, "ok, thanks".into())])
        .unwrap();
    let short = db.thread_export("chat").unwrap()[0].id;
    assert!(!db.__record_storage(&short).unwrap().1, "a one-line turn");
    let fact = db
        .remember(Memory::new(
            prose(9, 3_000),
            MemoryType::Semantic,
            vec![1.0, 0.5, 0.25, 0.125],
        ))
        .unwrap();
    db.flush().unwrap();
    assert!(
        !db.__record_storage(&fact).unwrap().1,
        "facts aren't compressed"
    );

    let mut v8 = open_db(&dir.path().join("v8.mnemo"), 8);
    v8.ingest_thread("chat", &[turn(1, prose(1, 3_000))])
        .unwrap();
    v8.flush().unwrap();
    let id = v8.thread_export("chat").unwrap()[0].id;
    assert!(!v8.__record_storage(&id).unwrap().1, "v8 never compresses");
    assert_eq!(v8.verify().unwrap(), v8.len());
}

#[test]
fn compressed_turns_survive_vacuum_restore_and_compaction() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = open_db(&path, 9);
    db.set_max_snapshots(64);
    let turns: Vec<ThreadTurn> = (0..60).map(|i| turn(i, prose(i, 1_500))).collect();
    db.ingest_thread("chat", &turns[..30]).unwrap();
    db.flush().unwrap();
    let before = db.snapshots().last().unwrap().txn_id;
    // Delete most of the first batch so its pages go sparse, then keep
    // writing: each flush's vacuum moves the surviving frames as they are.
    let first = db.thread_export("chat").unwrap();
    for m in first.iter().skip(3) {
        db.delete(&m.id).unwrap();
    }
    for chunk in turns[30..].chunks(10) {
        db.ingest_thread("chat", chunk).unwrap();
        db.flush().unwrap();
    }
    let live = db.thread_export_turns("chat").unwrap();
    let want: Vec<&String> = turns[..3]
        .iter()
        .chain(&turns[30..])
        .map(|t| &t.content)
        .collect();
    assert_eq!(live.iter().map(|t| &t.content).collect::<Vec<_>>(), want);
    assert_eq!(db.verify().unwrap(), db.len());

    // Back to the first batch: all 30 turns read back from their frames.
    db.restore_to(before).unwrap();
    let restored = db.thread_export_turns("chat").unwrap();
    assert_eq!(restored.len(), 30);
    for (a, b) in turns[..30].iter().zip(&restored) {
        assert_eq!(a.content, b.content);
    }
    db.close().unwrap();
    drop(db);

    Mnemo::compact_file(path.to_str().unwrap(), "pw").unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let after = db.thread_export("chat").unwrap();
    assert_eq!(after.len(), 30);
    for m in &after {
        assert!(
            db.__record_storage(&m.id).unwrap().1,
            "compaction keeps frames"
        );
    }
    assert_eq!(db.verify().unwrap(), db.len());
}

#[test]
fn malformed_frames_are_errors_not_panics() {
    // A frame with no content size (streaming encoder) is refused.
    let body = rmp_like_payload();
    let streamed = zstd::stream::encode_all(&body[..], 3).unwrap();
    assert!(mnemo_engine::__fuzz::decode_v9_memory(&streamed).is_err());
    // A frame that decompresses past the 64 MiB bound is refused before
    // any allocation of that size.
    let big = zstd::bulk::compress(&vec![0u8; (64 << 20) + 1], 1).unwrap();
    assert!(mnemo_engine::__fuzz::decode_v9_memory(&big).is_err());
    // Truncated and garbage frames.
    let ok = zstd::bulk::compress(&body, 3).unwrap();
    assert!(mnemo_engine::__fuzz::decode_v9_memory(&ok[..ok.len() - 3]).is_err());
    let mut garbage = vec![0x28, 0xB5, 0x2F, 0xFD];
    garbage.extend_from_slice(&[0xff; 32]);
    assert!(mnemo_engine::__fuzz::decode_v9_memory(&garbage).is_err());
    // The failed calls dropped this thread's context; a good frame still
    // decodes on a fresh one.
    let m = Memory::new(prose(4, 2_000), MemoryType::Episodic, vec![1.0; DIMS]);
    let good = zstd::bulk::compress(&rmp_serde::to_vec(&m).unwrap(), 3).unwrap();
    mnemo_engine::__fuzz::decode_v9_memory(&good).unwrap();
}

/// Bytes that aren't a `Memory`, only something to compress.
fn rmp_like_payload() -> Vec<u8> {
    prose(3, 2_000).into_bytes()
}

#[test]
fn a_cold_handle_decodes_only_the_threads_it_uses() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = open_db(&path, 9);
    for th in 0..3 {
        let turns: Vec<ThreadTurn> = (0..10)
            .map(|i| turn(th * 100 + i, prose(th * 100 + i, 1_200)))
            .collect();
        db.ingest_thread(&format!("chat-{th}"), &turns).unwrap();
    }
    db.remember(Memory::new(
        "a fact".to_string(),
        MemoryType::Semantic,
        vec![1.0, 0.5, 0.25, 0.125],
    ))
    .unwrap();
    let victim = db.thread_export("chat-2").unwrap()[0].id;
    db.close().unwrap();
    drop(db);

    let mut db = Mnemo::open(&path, "pw").unwrap();
    db.set_embedder(LenEmbedder).unwrap();
    // `set_embedder` reads `about()`, which builds the caches without
    // decoding a single turn.
    assert_eq!(
        db.__cold_thread_buckets(),
        Some(3),
        "about() decodes no turns"
    );
    assert_eq!(db.thread_export_turns("chat-1").unwrap().len(), 10);
    assert_eq!(
        db.__cold_thread_buckets(),
        Some(2),
        "the other two threads' turns stay undecoded"
    );

    // Re-ingesting a cold thread's turns is still a no-op (its turn ids
    // are found once it's warmed), and new turns continue its sequence.
    let again: Vec<ThreadTurn> = (0..10).map(|i| turn(i, prose(i, 1_200))).collect();
    let rep = db.ingest_thread("chat-0", &again).unwrap();
    assert_eq!(rep.skipped_existing_turns, 10);
    assert_eq!(rep.added_records, 0);
    db.ingest_thread("chat-0", &[turn(10, prose(10, 1_200))])
        .unwrap();
    assert_eq!(db.thread_export_turns("chat-0").unwrap().len(), 11);
    assert_eq!(db.__cold_thread_buckets(), Some(1));

    // Deleting a turn of the last cold thread warms it first, so its
    // layer entry stays exact.
    db.delete(&victim).unwrap();
    assert_eq!(db.__cold_thread_buckets(), Some(0));
    assert_eq!(db.thread_export_turns("chat-2").unwrap().len(), 9);
    assert_eq!(db.thread_forget("chat-2").unwrap(), 9);
    assert!(db.thread_export("chat-2").unwrap().is_empty());
    assert_eq!(db.verify().unwrap(), db.len());
}

fn recent_turn(i: usize, content: String) -> ThreadTurn {
    ThreadTurn {
        timestamp: None,
        ..turn(i, content)
    }
}

fn compressed_count(db: &mut Mnemo, thread: &str) -> usize {
    let ids: Vec<_> = db
        .thread_export(thread)
        .unwrap()
        .iter()
        .map(|m| m.id)
        .collect();
    ids.iter()
        .filter(|id| db.__record_storage(id).unwrap().1)
        .count()
}

#[test]
fn recent_turns_stay_plain_until_they_age_past_the_cutoff() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = open_db(&path, 9);
    let turns: Vec<ThreadTurn> = (0..300).map(|i| recent_turn(i, prose(i, 1_500))).collect();
    db.ingest_thread("live", &turns).unwrap();
    db.flush().unwrap();
    assert_eq!(
        compressed_count(&mut db, "live"),
        0,
        "recent turns are plain"
    );

    // Once they're past the cutoff, each commit that writes compresses up
    // to 256 of them.
    db.set_turn_compress_after(0);
    db.ingest_thread("other", &[recent_turn(1_000, prose(1_000, 200))])
        .unwrap();
    db.flush().unwrap();
    assert_eq!(compressed_count(&mut db, "live"), 256);
    db.ingest_thread("other", &[recent_turn(1_001, prose(1_001, 200))])
        .unwrap();
    db.flush().unwrap();
    assert_eq!(compressed_count(&mut db, "live"), 300);
    db.close().unwrap();
    drop(db);

    let mut db = Mnemo::open(&path, "pw").unwrap();
    let back = db.thread_export_turns("live").unwrap();
    assert_eq!(back.len(), 300);
    for (a, b) in turns.iter().zip(&back) {
        assert_eq!(a.content, b.content);
    }
    assert_eq!(db.verify().unwrap(), db.len());
}

#[test]
fn compaction_compresses_recent_turns_too() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = open_db(&path, 9);
    let turns: Vec<ThreadTurn> = (0..20).map(|i| recent_turn(i, prose(i, 2_000))).collect();
    db.ingest_thread("live", &turns).unwrap();
    db.close().unwrap();
    assert_eq!(compressed_count(&mut db, "live"), 0);
    drop(db);
    Mnemo::compact_file(path.to_str().unwrap(), "pw").unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(compressed_count(&mut db, "live"), 20);
    let back = db.thread_export_turns("live").unwrap();
    assert_eq!(back.len(), 20);
    assert_eq!(back[7].content, turns[7].content);
}
