"""End-to-end test of the mnemo Python bindings.

Run after installing the wheel:  python3 test_mnemo.py
Exits non-zero on the first failed assertion.
"""

import os
import tempfile
import time

import mnemo


def test_create_remember_recall():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "passphrase", dimensions=4)

        db.remember("the user likes tea", "semantic", [1.0, 0.0, 0.0, 0.0],
                    importance=0.9, agent_id="assistant",
                    metadata={"topic": "prefs", "confidence": 3})
        db.remember("the user asked for a refund", "episodic",
                    [0.0, 1.0, 0.0, 0.0], agent_id="assistant")
        db.flush()
        assert len(db) == 2, f"expected 2 memories, got {len(db)}"

        hits = db.recall([1.0, 0.0, 0.0, 0.0], top_k=2)
        assert hits, "recall returned nothing"
        assert hits[0]["content"] == "the user likes tea"
        assert "score" in hits[0] and "similarity" in hits[0]
        # metadata round-trips as a nested dict.
        assert hits[0]["metadata"]["topic"] == "prefs"
        assert hits[0]["metadata"]["confidence"] == 3
        db.close()
    print("ok  create / remember / recall / metadata")


def test_persistence_and_reopen():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=3)
        mid = db.remember("persist me", "procedural", [0.5, 0.5, 0.5])
        db.close()
        # `close()` currently flushes but does NOT release the underlying
        # OS lock — the Rust handle stays alive until the Python object
        # is garbage-collected. Force a `drop` so the reopen below can
        # acquire the exclusive lock (Phase 1.1). Follow-up: refactor
        # the Rust wrapper to make `close()` release the handle so this
        # `del` isn't required. See CHANGELOG's [Unreleased] Fixed
        # section for the tracking note.
        del db

        # Reopen without dimensions — must read them from the file.
        db2 = mnemo.open(path, "pw")
        assert len(db2) == 1
        got = db2.get(mid)
        assert got["content"] == "persist me"
        assert got["memory_type"] == "procedural"
        db2.close()
        del db2
    print("ok  persistence / reopen / get")


def test_wrong_passphrase():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        # `mnemo.open(...).close()` flushes but the transient handle
        # holds the exclusive lock until the temp-expression drops.
        # Bind + explicitly `del` so the lock is released before the
        # wrong-passphrase reopen tries to acquire.
        writer = mnemo.open(path, "correct", dimensions=2)
        writer.close()
        del writer
        try:
            mnemo.open(path, "wrong")
            raise AssertionError("wrong passphrase should have failed")
        except RuntimeError:
            pass
    print("ok  wrong passphrase is rejected")


def test_index_and_snapshots():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        with mnemo.open(path, "pw", dimensions=4) as db:
            for i in range(40):
                v = [float(i % 4 == j) for j in range(4)]
                db.remember(f"m{i}", "semantic", v)
            db.flush()

            db.build_index()
            assert db.has_index()
            hits = db.recall([1.0, 0.0, 0.0, 0.0], top_k=5)
            assert len(hits) == 5

            snaps = db.snapshots()
            assert len(snaps) >= 1
            first = snaps[0]["txn_id"]
            # context-manager __exit__ will flush.
        # `with` doesn't unbind the name; `db` still holds the Rust
        # handle (and its exclusive lock) until reassignment or `del`.
        # Force release before the reopen.
        del db
        # Reopen and roll back to the first snapshot.
        db = mnemo.open(path, "pw")
        info = db.restore_to(first)
        assert info["txn_id"] == first
        db.close()
        del db
    print("ok  index / snapshots / restore")


def test_delete_and_stats():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=2)
        a = db.remember("keep", "semantic", [1.0, 0.0])
        b = db.remember("drop", "semantic", [0.0, 1.0])
        db.delete(b)
        db.flush()
        assert len(db) == 1
        try:
            db.get(b)
            raise AssertionError("deleted memory should not be retrievable")
        except RuntimeError:
            pass
        s = db.stats()
        assert s["memories"] == 1 and s["encrypted"] is True
        assert s["dimensions"] == 2
        db.close()
    print("ok  delete / stats")


def test_export_is_encrypted():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        backup = os.path.join(d, "backup.mnemo")
        db = mnemo.open(path, "pw", dimensions=2)
        db.remember("plaintext-secret-token", "semantic", [1.0, 0.0])
        db.export_encrypted(backup)
        db.close()

        raw = open(backup, "rb").read()
        assert b"plaintext-secret-token" not in raw, "content leaked unencrypted!"
        # The backup is a valid encrypted database.
        db2 = mnemo.open(backup, "pw")
        assert len(db2) == 1
        db2.close()
    print("ok  export_encrypted produces an opaque, valid copy")


def test_session_lifecycle():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=3)

        sess = db.session("assistant")
        assert sess.agent() == "assistant"
        assert len(sess.id()) > 0
        assert sess.turn_count() == 0

        # Turns can be built three ways; all become working memory.
        sess.add_turn(mnemo.Turn.user("hi", [1.0, 0.0, 0.0]))
        sess.add_turn(mnemo.Turn.assistant("hello", [0.0, 1.0, 0.0]))
        sess.add_turn(mnemo.Turn("system", "be brief", [0.0, 0.0, 1.0]))
        assert sess.turn_count() == 3
        ids = sess.turn_ids()

        # Before close, turns are working memory.
        db.flush()
        assert db.get(ids[0])["memory_type"] == "working"
        assert db.get(ids[0])["metadata"]["role"] == "user"

        # Closing consolidates working -> episodic.
        promoted = sess.close()
        assert promoted == 3, f"expected 3 promoted, got {promoted}"
        for i in ids:
            assert db.get(i)["memory_type"] == "episodic"

        # A closed session rejects further use.
        try:
            sess.add_turn(mnemo.Turn.user("late", [1.0, 0.0, 0.0]))
            raise AssertionError("closed session should reject add_turn")
        except RuntimeError:
            pass
        db.close()
    print("ok  session lifecycle / consolidation")


def test_session_discard_and_context_manager():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=2)

        # discard() throws turns away.
        s1 = db.session("agent")
        s1.add_turn(mnemo.Turn.user("scratch", [1.0, 0.0]))
        s1.add_turn(mnemo.Turn.user("noise", [0.0, 1.0]))
        removed = s1.discard()
        assert removed == 2
        assert len(db) == 0

        # The `with` form consolidates on exit.
        with db.session("agent") as s2:
            s2.add_turn(mnemo.Turn.user("keep this", [1.0, 0.0]))
            tid = s2.turn_ids()[0]
        assert db.get(tid)["memory_type"] == "episodic"
        db.close()
    print("ok  session discard / context manager")


def test_session_recall_is_agent_scoped():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=2)
        db.remember("bob-secret", "semantic", [1.0, 0.0], agent_id="bob")
        db.remember("alice-fact", "semantic", [1.0, 0.0], agent_id="alice")
        db.flush()

        sess = db.session("alice")
        hits = sess.recall([1.0, 0.0], top_k=10)
        contents = [h["content"] for h in hits]
        assert "alice-fact" in contents
        assert "bob-secret" not in contents, "another agent's memory leaked"
        sess.close()
        db.close()
    print("ok  session recall is agent-scoped")


# --- PR BP: binding-parity additions (v0.4.1) ---------------------------

def test_memories_and_dimensions():
    """`memories()` enumerates all live entries; `dimensions()` reads
    the vector width without going through `stats()`."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=3)
        assert db.dimensions() == 3
        db.remember("a", "semantic", [1.0, 0.0, 0.0])
        db.remember("b", "episodic", [0.0, 1.0, 0.0])
        db.remember("c", "working", [0.0, 0.0, 1.0])
        db.flush()

        mems = db.memories()
        assert len(mems) == 3
        contents = sorted(m["content"] for m in mems)
        assert contents == ["a", "b", "c"]
        # Round-trip: each dict has the shape memory_to_dict emits.
        assert all("id" in m and "vector" in m for m in mems)
        db.close()
    print("ok  memories() enumerates all live; dimensions() getter")


def test_batched_cache_auto_flush_and_reopen():
    """Under `set_cache_flush_policy('batched')`, the Python binding
    stops auto-flushing per cache_put. Once the engine's max_dirty
    trips, buffered entries become durable and survive reopen."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=2)
        db.set_cache_flush_policy("batched", max_dirty=2, max_age_secs=3600)

        # First put: pending, not yet durable if the process died now.
        db.cache_put("llm", "k1", "v1")
        # Second put trips max_dirty=2, auto-flush inside the engine.
        db.cache_put("llm", "k2", "v2")
        # Do NOT call db.flush(); the engine already did it under batched.
        db.close()
        del db

        db2 = mnemo.open(path, "pw")
        assert db2.cache_get("llm", "k1") is not None, "k1 lost after batched auto-flush"
        assert db2.cache_get("llm", "k2") is not None, "k2 lost after batched auto-flush"
        assert db2.cache_get("llm", "k1")["value"] == b"v1"
        assert db2.cache_get("llm", "k2")["value"] == b"v2"
        db2.close()
        del db2
    print("ok  set_cache_flush_policy('batched') + auto-flush + reopen")


def test_batched_cache_uncommitted_is_a_miss():
    """The Phase 1.3 invariant, mirrored from Rust: a batched put
    that never flushed and is lost across reopen surfaces as a miss,
    never as corruption or a stale value."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=2)
        db.set_cache_flush_policy("batched", max_dirty=99, max_age_secs=3600)
        # No flush, no close: simulate SIGKILL by dropping the handle
        # while the entry is only in the pending batch.
        db.cache_put("llm", "will-be-lost", "gone")
        del db

        db2 = mnemo.open(path, "pw")
        assert db2.cache_get("llm", "will-be-lost") is None, \
            "unflushed batched entry must be a miss on reopen"
        db2.close()
        del db2
    print("ok  batched uncommitted put -> miss (never corruption)")


def test_cache_budget_evicts_lru():
    """`set_cache_budget` caps a namespace; subsequent puts over the
    cap drop the least-recently-used entries.

    Cache timestamps have SECOND granularity (`memory::now_secs()`).
    Without a sleep, all three puts and the touching get would share
    the same `accessed_at`, and the LRU tiebreak (`min_by_key`) would
    fall back to insertion order — evicting `a` regardless of the
    touch. The `time.sleep(1.1)` below shifts `a`'s access time past
    `b`'s creation time so the eviction picks `b` deterministically."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=2)
        # Cap the namespace at 2 entries, plenty of bytes.
        db.set_cache_budget("small", max_entries=2, max_bytes=1024 * 1024)
        db.cache_put("small", "a", "aaa")
        db.cache_put("small", "b", "bbb")
        # Advance past the shared-second so the following get bumps
        # `a`'s accessed_at into a strictly-later second than `b`.
        time.sleep(1.1)
        assert db.cache_get("small", "a") is not None
        # A third put should evict `b` (now the true LRU).
        db.cache_put("small", "c", "ccc")

        assert db.cache_get("small", "a") is not None, "a is MRU; should survive"
        assert db.cache_get("small", "c") is not None, "c was just put; should be present"
        assert db.cache_get("small", "b") is None, "b was LRU; should have been evicted"
        stats = db.cache_stats("small")
        assert stats["entries"] == 2
        assert stats["evictions"] >= 1
        db.close()
    print("ok  set_cache_budget + LRU eviction")


def test_rekey_and_reopen_with_new_passphrase():
    """`rekey` re-wraps the DEK; the old passphrase stops working,
    the new one opens the same content unchanged."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "old-pw", dimensions=2)
        db.remember("survive-rekey", "semantic", [1.0, 0.0])
        db.flush()
        # `fast=True` keeps this test cheap — production callers omit it.
        db.rekey("new-pw", fast=True)
        db.close()
        del db

        # Old passphrase must be rejected.
        try:
            mnemo.open(path, "old-pw")
            raise AssertionError("old passphrase should have been rejected after rekey")
        except RuntimeError:
            pass

        # New passphrase opens; content is intact.
        db2 = mnemo.open(path, "new-pw")
        assert len(db2) == 1
        assert db2.memories()[0]["content"] == "survive-rekey"
        db2.close()
        del db2
    print("ok  rekey + reopen with new passphrase; old is rejected")


def test_compact_file_module_fn():
    """Module-level `mnemo.compact_file(path, passphrase)` rewrites the
    file, dropping tombstones. Returns `{before, after}`."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=2)
        keep = db.remember("keep", "semantic", [1.0, 0.0])
        drop = db.remember("drop", "semantic", [0.0, 1.0])
        db.delete(drop)
        db.flush()
        assert len(db) == 1
        db.close()
        del db

        report = mnemo.compact_file(path, "pw")
        assert report["before"] == 1, f"compact 'before' should count live only: {report}"
        assert report["after"] == 1
        assert isinstance(report["before"], int)
        assert isinstance(report["after"], int)

        # File still opens; the surviving memory is intact.
        db2 = mnemo.open(path, "pw")
        assert len(db2) == 1
        assert db2.get(keep)["content"] == "keep"
        db2.close()
        del db2
    print("ok  mnemo.compact_file(path, pp) -> {before, after}")


if __name__ == "__main__":
    print(f"mnemo {mnemo.__version__}")
    test_create_remember_recall()
    test_persistence_and_reopen()
    test_wrong_passphrase()
    test_index_and_snapshots()
    test_delete_and_stats()
    test_export_is_encrypted()
    test_session_lifecycle()
    test_session_discard_and_context_manager()
    test_session_recall_is_agent_scoped()
    test_memories_and_dimensions()
    test_batched_cache_auto_flush_and_reopen()
    test_batched_cache_uncommitted_is_a_miss()
    test_cache_budget_evicts_lru()
    test_rekey_and_reopen_with_new_passphrase()
    test_compact_file_module_fn()
    print("\nall Python binding tests passed")
