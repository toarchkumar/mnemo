//! v0.16.1: a turn whose content is only transcript markers (`[thinking]`,
//! `[tool_use name=…]`, `[tool_result ok, N bytes]`) is kept in the thread,
//! so `thread export` shows the whole flow, but isn't embedded and is never
//! a recall hit (it matched any query weakly and crowded turn recall).

use mnemo_engine::{
    read_transcript_with, Filter, KdfParams, Mnemo, MnemoConfig, RecallRequest, ToolResults,
    TranscriptFormat, TranscriptOptions, MNEMO_META_KIND, MNEMO_META_MARKERS_ONLY,
};
use serde_json::{json, Value};

const PW: &str = "marker-turns-test-passphrase";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

/// A Claude Code transcript: text turns between a thinking-only, a
/// tool-use-only and a tool-result-only turn, and one with text beside a
/// marker.
fn transcript() -> Vec<u8> {
    let lines = [
        json!({"type": "user", "uuid": "u0", "message": {"role": "user",
            "content": "Where does this project deploy?"}}),
        json!({"type": "assistant", "uuid": "u1", "message": {"role": "assistant",
            "content": [{"type": "thinking", "thinking": "Let me look it up."}]}}),
        json!({"type": "assistant", "uuid": "u2", "message": {"role": "assistant",
            "content": [{"type": "tool_use", "name": "mcp__mnemo__recall", "input": {}}]}}),
        json!({"type": "user", "uuid": "u3", "message": {"role": "user",
            "content": [{"type": "tool_result", "content": "the deploy target is staging-7"}]}}),
        json!({"type": "assistant", "uuid": "u4", "message": {"role": "assistant",
            "content": [{"type": "text", "text": "It deploys to staging-7."},
                        {"type": "tool_use", "name": "Bash", "input": {}}]}}),
        json!({"type": "assistant", "uuid": "u5", "message": {"role": "assistant",
            "content": [{"type": "redacted_thinking", "data": "x"},
                        {"type": "tool_use", "name": "Read", "input": {}}]}}),
    ];
    let mut s = String::new();
    for l in lines {
        s.push_str(&l.to_string());
        s.push('\n');
    }
    s.into_bytes()
}

fn turns() -> Vec<mnemo_engine::ThreadTurn> {
    let opts = TranscriptOptions {
        tool_results: ToolResults::Omit,
        lenient: false,
    };
    read_transcript_with(&transcript()[..], TranscriptFormat::ClaudeCodeJsonl, opts).unwrap()
}

#[test]
fn marker_only_turns_are_kept_but_never_recalled() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    let turns = turns();
    assert_eq!(turns.len(), 6);
    assert_eq!(db.ingest_thread_new_turns("s", &turns).unwrap(), 6);
    // Only the two turns with text are embedded.
    let texts = db.ingest_thread_texts("s", &turns).unwrap();
    assert_eq!(
        texts,
        [
            "Where does this project deploy?",
            "It deploys to staging-7.\n[tool_use name=Bash]"
        ]
    );
    let vectors = vec![vec![1.0, 0.0, 0.0, 0.0], vec![0.9, 0.1, 0.0, 0.0]];
    let r = db
        .ingest_thread_with_vectors("s", &turns, &vectors)
        .unwrap();
    assert_eq!(r.added_records, 6);
    db.flush().unwrap();

    // Export: all six, in order; the four marker-only ones flagged.
    let out = db.thread_export("s").unwrap();
    let contents: Vec<&str> = out.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        contents,
        [
            "Where does this project deploy?",
            "[thinking]",
            "[tool_use name=mcp__mnemo__recall]",
            "[tool_result ok, 30 bytes]",
            "It deploys to staging-7.\n[tool_use name=Bash]",
            "[redacted_thinking]\n[tool_use name=Read]",
        ]
    );
    let flagged: Vec<bool> = out
        .iter()
        .map(|m| m.metadata.get(MNEMO_META_MARKERS_ONLY) == Some(&Value::Bool(true)))
        .collect();
    assert_eq!(flagged, [false, true, true, true, false, true]);

    // A turn-filtered recall returns only the text turns, with any query.
    for q in [
        vec![1.0, 0.0, 0.0, 0.0],
        vec![0.0, 0.0, 0.0, 1.0],
        vec![0.5, 0.5, 0.5, 0.5],
    ] {
        let hits = db
            .recall(
                &RecallRequest::new(q)
                    .top_k(10)
                    .track_access(false)
                    .filter(Filter::default().metadata_eq(MNEMO_META_KIND, json!("turn"))),
            )
            .unwrap();
        let got: Vec<&str> = hits.iter().map(|h| h.memory.content.as_str()).collect();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.iter().all(|c| c.contains("deploy")), "{got:?}");
    }

    // A re-run adds nothing, after a reopen too.
    db.close().unwrap();
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert_eq!(db.ingest_thread_new_turns("s", &turns).unwrap(), 0);
    assert!(db.ingest_thread_texts("s", &turns).unwrap().is_empty());
    let r = db.ingest_thread_with_vectors("s", &turns, &[]).unwrap();
    assert_eq!(r.added_records, 0);
    assert_eq!(r.skipped_existing_turns, 6);
    assert_eq!(db.thread_export("s").unwrap().len(), 6);
}

/// A save whose new turns are all marker-only (the tail of a session that
/// ended on tool calls) writes them, with no vectors to embed.
#[test]
fn a_batch_of_marker_only_turns_is_still_written() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("t.mnemo"), PW, cfg()).unwrap();
    let all = turns();
    let tail: Vec<_> = all
        .iter()
        .filter(|t| t.turn_id != "u0" && t.turn_id != "u4")
        .cloned()
        .collect();
    assert!(db.ingest_thread_texts("s", &tail).unwrap().is_empty());
    assert_eq!(db.ingest_thread_new_turns("s", &tail).unwrap(), 4);
    let r = db.ingest_thread_with_vectors("s", &tail, &[]).unwrap();
    assert_eq!(r.added_records, 4);
    // And through ingest_thread, which needs no embedder for them.
    let r = db.ingest_thread("other", &tail).unwrap();
    assert_eq!(r.added_records, 4);
}
