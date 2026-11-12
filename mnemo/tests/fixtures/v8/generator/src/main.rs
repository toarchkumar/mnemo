//! Writes the v8 fixtures with the published v0.12.3 engine (see
//! Cargo.toml). Two files, both v8, 8 dimensions, the fast KDF:
//!
//! - `v8_passphrase.mnemo`, passphrase `v8-fixture-passphrase`: a scaffold
//!   manifest naming an embedder, plain memories of every type and two
//!   agents, a keyed fact with three versions, a thread with four turns and
//!   two essence items, a project essence item, a result-cache entry, an
//!   IVF+PQ index (v8 builds one at any size), and one snapshot per flush;
//! - `v8_key.mnemo`, raw key in `v8_key.key` (64 hex digits): a few
//!   memories.
//!
//! `expected.json` records what the read tests compare against. These are
//! test values: the passphrase and the key open nothing else.

use mnemo_engine::{
    CachePutOpts, EssenceScope, EssenceSection, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig,
    ThreadTurn,
};
use serde_json::json;
use std::path::Path;

const DIMS: usize = 8;
const PASSPHRASE: &str = "v8-fixture-passphrase";

/// A deterministic, non-zero vector for item `i`.
fn vec_for(i: usize) -> Vec<f32> {
    let mut v = vec![0.05f32; DIMS];
    v[i % DIMS] = 1.0;
    v[(i * 3 + 1) % DIMS] += 0.5;
    v
}

fn cfg() -> MnemoConfig {
    MnemoConfig {
        format_version: 8,
        dimensions: DIMS,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

/// `v8_pending_wal.mnemo` (RB-003.3): what a kill between a commit and
/// its checkpoint leaves under v0.12. Ten memories are checkpointed; five
/// more are committed in the WAL but not written to their home pages.
///
/// A child process (`crash <path> <n>`) adds the five, arms the published
/// engine's write failpoint at the `n`th write and flushes, then exits
/// without running destructors, as a kill would. The smallest `n` that
/// leaves all five recoverable is the one between the WAL commit and the
/// checkpoint.
fn pending_wal(out: &Path) {
    let path = out.join("v8_pending_wal.mnemo");
    let exe = std::env::current_exe().expect("own path");
    for n in 0..400 {
        let _ = std::fs::remove_file(&path);
        let mut db = Mnemo::create(&path, PASSPHRASE, cfg()).expect("create v8");
        for i in 0..10 {
            db.remember(Memory::new(format!("checkpointed note {i}"), MemoryType::Semantic, vec_for(i)))
                .unwrap();
        }
        db.close().unwrap();
        let status = std::process::Command::new(&exe)
            .args(["crash", path.to_str().unwrap(), &n.to_string()])
            .status()
            .expect("run the crash child");
        assert!(status.success());
        // Still pending? Then a copy opened read-write (v0.12.3 can) must
        // recover all fifteen.
        if Mnemo::open_read_only(&path, PASSPHRASE).is_ok() {
            continue;
        }
        let probe = out.join("v8_pending_wal.probe");
        std::fs::copy(&path, &probe).unwrap();
        let recovered = Mnemo::open(&probe, PASSPHRASE).map(|d| d.len()).unwrap_or(0);
        let _ = std::fs::remove_file(&probe);
        if recovered == 15 {
            println!("wrote {} (failpoint at write {n})", path.display());
            return;
        }
    }
    panic!("no failpoint left a committed, un-checkpointed WAL");
}

fn crash(path: &str, n: i64) -> ! {
    let mut db = Mnemo::open(path, PASSPHRASE).expect("open");
    for i in 0..5 {
        db.remember(Memory::new(format!("note in the WAL {i}"), MemoryType::Episodic, vec_for(20 + i)))
            .unwrap();
    }
    mnemo_engine::__failpoints::set_writes_until_fail(n);
    let _ = db.flush();
    // A kill: no destructor, no further write.
    std::process::exit(0)
}

fn main() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("crash") => crash(&args[2], args[3].parse().unwrap()),
        Some("pending-wal") => return pending_wal(&out),
        _ => {}
    }
    let pass_path = out.join("v8_passphrase.mnemo");
    let key_path = out.join("v8_key.mnemo");
    for p in [&pass_path, &key_path] {
        let _ = std::fs::remove_file(p);
    }

    // --- the passphrase file -------------------------------------------
    let mut db = Mnemo::create(&pass_path, PASSPHRASE, cfg()).expect("create v8");
    let mut manifest = Memory::scaffold_manifest(DIMS);
    manifest.metadata.insert(
        "embedder".into(),
        json!({ "name": "fixture-embedder-r1", "dimensions": DIMS, "normalize": true }),
    );
    manifest.metadata.remove("scaffold");
    db.remember(manifest).expect("manifest");
    db.flush().expect("flush 1");

    let plain = [
        ("The user prefers green tea in the morning", MemoryType::Semantic, "agent-a"),
        ("Flight UA 931 leaves on 12 March", MemoryType::Episodic, "agent-a"),
        ("Run cargo fmt before pushing", MemoryType::Procedural, "agent-b"),
        ("The draft is on the second monitor", MemoryType::Working, "agent-b"),
        ("Invoice 4471 was paid on 3 November", MemoryType::Episodic, "agent-a"),
        ("The office wifi is called Meridian", MemoryType::Semantic, "agent-b"),
    ];
    let mut plain_ids = Vec::new();
    for (i, (text, ty, agent)) in plain.iter().enumerate() {
        let mut m = Memory::new(*text, *ty, vec_for(i));
        m.agent_id = agent.to_string();
        m.importance = 0.2 + 0.1 * i as f32;
        m.metadata.insert("topic".into(), json!(format!("t{i}")));
        plain_ids.push(db.remember(m).expect("remember").to_string());
    }
    db.flush().expect("flush 2");

    for (i, value) in ["tea", "coffee", "water"].iter().enumerate() {
        let m = Memory::new(format!("drink: {value}"), MemoryType::Semantic, vec_for(10 + i));
        db.remember_keyed(m, "fact:drink").expect("keyed");
        db.flush().expect("flush keyed");
    }

    let turns: Vec<ThreadTurn> = [
        ("u1", "user", "Can you plan the offsite for March?"),
        ("a1", "assistant", "Yes: three days in Lisbon, flights on the 12th."),
        ("u2", "user", "Book the hotel near the river."),
        ("a2", "assistant", "Booked the Riverside, 3 nights."),
    ]
    .iter()
    .map(|(id, role, content)| ThreadTurn {
        turn_id: id.to_string(),
        role: role.to_string(),
        content: content.to_string(),
        timestamp: None,
    })
    .collect();
    let tvecs: Vec<Vec<f32>> = (0..turns.len()).map(|i| vec_for(20 + i)).collect();
    db.ingest_thread_with_vectors("offsite", &turns, &tvecs).expect("thread");
    let thread = EssenceScope::Thread("offsite".into());
    db.essence_add_with_vector(thread.clone(), EssenceSection::Decisions, "Lisbon, three days", None, vec_for(30))
        .expect("essence decision");
    db.essence_append_timeline_with_vector(thread.clone(), "Hotel booked: Riverside", None, vec_for(31))
        .expect("essence timeline");
    db.essence_add_with_vector(EssenceScope::Project, EssenceSection::Conventions, "Dates are day month", None, vec_for(32))
        .expect("project essence");
    db.cache_put("app", "weather:lisbon", b"sunny, 21C", CachePutOpts { content_type: "text".into(), ttl_secs: None, cost_hint_ms: None })
        .expect("cache");
    db.flush().expect("flush thread");
    db.build_index().expect("index");
    db.flush().expect("flush index");

    let history: Vec<String> = db.history("fact:drink", None).expect("history").into_iter().map(|m| m.content).collect();
    let thread_essence = db.essence(thread.clone(), 4096).expect("render");
    let project_essence = db.essence(EssenceScope::Project, 4096).expect("render");
    let turns_out: Vec<String> = db.thread_export_turns("offsite").expect("export").into_iter().map(|t| t.content).collect();
    let snapshots = db.snapshots().len();
    let stats = db.stats().expect("stats");
    let has_index = db.has_index();
    drop(db);

    // --- the raw-key file ----------------------------------------------
    let key: [u8; 32] = std::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(11));
    let mut kdb = Mnemo::create_with_key(&key_path, &key, cfg()).expect("create v8 key");
    for i in 0..3 {
        kdb.remember(Memory::new(format!("key file memory {i}"), MemoryType::Semantic, vec_for(40 + i)))
            .expect("remember");
    }
    kdb.flush().expect("flush key");
    drop(kdb);
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(out.join("v8_key.key"), format!("{hex}\n")).expect("key file");

    let expected = json!({
        "generator": "mnemo/tests/fixtures/v8/generator (mnemo-engine =0.12.3 from crates.io)",
        "passphrase_file": {
            "path": "v8_passphrase.mnemo",
            "passphrase": PASSPHRASE,
            "format_version": 8,
            "dimensions": DIMS,
            "memories": stats.memories,
            "plain_ids": plain_ids,
            "plain_contents": plain.iter().map(|p| p.0).collect::<Vec<_>>(),
            "keyed_history_newest_first": history,
            "thread_turns": turns_out,
            "thread_essence": thread_essence,
            "project_essence": project_essence,
            "manifest_embedder": "fixture-embedder-r1",
            "cache": { "namespace": "app", "key": "weather:lisbon", "value": "sunny, 21C" },
            "snapshots": snapshots,
            "has_index": has_index,
        },
        "key_file": { "path": "v8_key.mnemo", "key_hex_file": "v8_key.key", "memories": 3 },
    });
    std::fs::write(out.join("expected.json"), serde_json::to_string_pretty(&expected).unwrap() + "\n").expect("expected.json");
    println!("wrote the v8 fixtures in {}", out.display());
}
