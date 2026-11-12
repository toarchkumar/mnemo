"""End-to-end test of the mnemo Python bindings.

Run after installing the wheel:  python3 test_mnemo.py
Exits non-zero on the first failed assertion.
"""

import os
import tempfile
import time

import mnemo_engine as mnemo

# FMT-9f: new files are v9, which only builds an ANN index at 20K facts
# (`V9_ANN_MIN_FACTS`). The index tests below use small files, so drop the
# threshold with the engine's hidden test override (read at build time).
# Needs a wheel built with `--features dev-hooks` (HOOKS-1): a released
# wheel ignores this switch.
os.environ["MNEMO_DEV_ANN_MIN_FACTS"] = "0"


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
        # PY-1 (2026-11-01): `close()` now releases the OS file lock
        # synchronously — no `del db` dance required to reopen the
        # same path in the same process.
        db.close()

        # Reopen without dimensions — must read them from the file.
        db2 = mnemo.open(path, "pw")
        assert len(db2) == 1
        got = db2.get(mid)
        assert got["content"] == "persist me"
        assert got["memory_type"] == "procedural"
        db2.close()
    print("ok  persistence / reopen / get")


def test_close_releases_lock_and_blocks_further_use():
    """PY-1: close() releases the file lock immediately AND makes further
    method calls raise a clear 'database is closed' error (not silently
    read stale state, not panic)."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=4)
        db.remember("x", "semantic", [1.0, 0.0, 0.0, 0.0])
        db.close()

        # Reopen the same path in the same process without del / GC —
        # pre-PY-1 this raised Locked.
        db2 = mnemo.open(path, "pw")
        assert len(db2) == 1
        db2.close()

        # Any method on the first (closed) handle must raise clearly.
        try:
            db.len = db.__len__()  # or any method; `len` is fine
        except RuntimeError as e:
            assert "closed" in str(e), f"wanted 'closed' in error, got {e!r}"
        else:
            raise AssertionError("len() on closed db should have raised RuntimeError")

        # `close()` is idempotent — second call is a no-op.
        db.close()
    print("ok  close releases lock + closed-handle errors + idempotent close")


def test_with_block_releases_lock_on_exit():
    """PY-1: a `with mnemo.open(...) as db:` block's __exit__ releases
    the lock, so immediately reopening the same path succeeds."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        with mnemo.open(path, "pw", dimensions=4) as db:
            db.remember("y", "semantic", [0.0, 1.0, 0.0, 0.0])
        # No `del db` — the `with` exit alone must have released the
        # lock. Pre-PY-1 the lock sat until the next GC pass.
        db2 = mnemo.open(path, "pw")
        assert len(db2) == 1
        db2.close()
    print("ok  with-block exit releases lock")


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
        assert s["recall_mode"] == "exact_resident"
        # v0.13 (INSP-1 step 4): what a restore can reach right now.
        assert s["snapshots_retained"] == s["snapshots"] >= 1
        assert isinstance(s["oldest_restorable_at"], int)
        assert s["snapshot_pinned_bytes"] >= 0
        # v0.12.2 (RB-001.24): metadata can't set engine keys.
        try:
            db.remember("forged", "semantic", [0.0, 1.0],
                        metadata={"_mnemo.kind": "turn", "_mnemo.thread": "t1"})
            raise AssertionError("a _mnemo.* metadata key should be refused")
        except (RuntimeError, ValueError) as e:
            assert "reserved" in str(e), e
        assert len(db) == 1
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


def _fake_embedder(dims=4):
    """A tiny deterministic in-test embedder: hashes a string onto one
    axis and emits a unit spike there. Deps-free so this test file still
    runs in a bare `maturin develop` venv."""
    def embed(texts):
        out = []
        for t in texts:
            axis = (sum(bytearray(t.encode("utf-8"))) % dims)
            v = [0.0] * dims
            v[axis] = 1.0
            out.append(v)
        return out
    return embed


def test_set_embedder_and_text_round_trip():
    """set_embedder → remember_text → recall_text brings the right
    memory back, through the deterministic fake embedder."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=4)
        db.set_embedder(_fake_embedder(4), name="fake-4d", dimensions=4, normalize=True)

        db.remember_text("alpha", "semantic")
        db.remember_text("beta", "episodic")
        db.flush()

        hits = db.recall_text("alpha", top_k=2)
        assert hits, "recall_text returned nothing"
        assert hits[0]["content"] == "alpha", f"got {hits[0]['content']}"
        db.close()
    print("ok  set_embedder + remember_text + recall_text round-trip")


def test_set_embedder_dim_mismatch_rejected():
    """Registering an embedder whose declared dimensions don't match
    the database's width raises RuntimeError."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=4)
        try:
            db.set_embedder(_fake_embedder(8), name="wrong", dimensions=8, normalize=True)
            raise AssertionError("expected RuntimeError on dim mismatch")
        except RuntimeError as e:
            assert "dimension mismatch" in str(e), f"got {e}"
        db.close()
    print("ok  set_embedder dim-mismatch rejected")


def test_set_embedder_identity_mismatch_rejected_after_reopen():
    """First handle registers 'MiniLM'; manifest absorbs the identity.
    A second handle registering a different name under the same
    dimensions is rejected loudly."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=4)
        db.insert_default_manifest()
        db.flush()
        db.set_embedder(_fake_embedder(4), name="MiniLM", dimensions=4, normalize=True)
        db.close()
        del db

        db2 = mnemo.open(path, "pw")
        try:
            db2.set_embedder(_fake_embedder(4), name="bge", dimensions=4, normalize=True)
            raise AssertionError("expected identity mismatch")
        except RuntimeError as e:
            assert "embedder identity mismatch" in str(e), f"got {e}"
        # Same identity round-trips.
        db2.set_embedder(_fake_embedder(4), name="MiniLM", dimensions=4, normalize=True)
        db2.close()
        del db2
    print("ok  set_embedder identity-mismatch rejected across reopen")



def test_set_embedder_identity_checked_without_a_manifest():
    """EMBED-ID-1: a file with no manifest records the first embedder too,
    and refuses another identity of the same width after a reopen. The
    record isn't a memory."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "plain.mnemo")
        db = mnemo.open(path, "pw", dimensions=4)
        db.set_embedder(_fake_embedder(4), name="model-r1", dimensions=4, normalize=True)
        db.remember("one memory", "semantic", [1.0, 0.0, 0.0, 0.0])
        db.close()
        del db

        db2 = mnemo.open(path, "pw")
        try:
            db2.set_embedder(_fake_embedder(4), name="model-r2", dimensions=4, normalize=True)
            raise AssertionError("expected identity mismatch")
        except RuntimeError as e:
            assert "embedder identity mismatch" in str(e), f"got {e}"
            assert "model-r1" in str(e) and "model-r2" in str(e), f"got {e}"
        db2.set_embedder(_fake_embedder(4), name="model-r1", dimensions=4, normalize=True)
        assert len(db2) == 1, len(db2)
        db2.close()
        del db2
    print("ok  set_embedder identity checked without a manifest")

def test_set_embedder_on_read_only_handle_adopts_without_write():
    """Reader opened via `read_only=True` can still register an
    embedder and drive `recall_text` (PR 10a.1 Bug 2 fix: `set_embedder`
    used to return `ReadOnly` before the dim/manifest checks ran)."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "ro.mnemo")
        # Writer: seed the file with the nested manifest + a text memory.
        db = mnemo.open(path, "pw", dimensions=4)
        db.set_embedder(_fake_embedder(4), name="fake-4d", dimensions=4, normalize=True)
        db.remember_text("delta", "semantic")
        db.flush()
        db.close()
        del db

        # Reader: register embedder (no write), run recall_text.
        # `track_access=False` is required — the default `True` dirties
        # the catalog on hit, which `ReadOnly` refuses. That's the
        # Phase 1.1 contract, orthogonal to the Bug 2 fix.
        r = mnemo.open(path, "pw", read_only=True)
        r.set_embedder(_fake_embedder(4), name="fake-4d", dimensions=4, normalize=True)
        hits = r.recall_text("delta", top_k=1, track_access=False)
        assert hits, "recall_text on read-only handle returned nothing"
        assert hits[0]["content"] == "delta"
        # PR 10a.2: `close()` on a read-only handle is now a no-op
        # (delegates to `Mnemo::close`, which short-circuits read-only).
        r.close()
        del r
    print("ok  set_embedder on read-only handle + recall_text works")


def test_read_only_with_block_and_close_are_noops():
    """PR 10a.2: `close()` and `__exit__` delegate to the Rust core's
    `close()`, which short-circuits read-only handles. The historic
    binding called `flush()` directly, which errored. Covers:
      1. `with mnemo.open(..., read_only=True)` exits cleanly.
      2. `db.close()` on a read-only handle returns without raising.
      3. `db.export_encrypted(dest)` on a read-only handle works (no
         flush required; the file is already fully durable)."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "ro.mnemo")
        # Seed a database so we have something to open read-only.
        writer = mnemo.open(path, "pw", dimensions=2)
        writer.remember("hello", "semantic", [1.0, 0.0])
        writer.flush()
        writer.close()
        del writer

        # 1. with-block: __exit__ must not raise on a read-only handle.
        with mnemo.open(path, "pw", read_only=True) as r:
            assert len(r) == 1
        # 2. Explicit close(): no-op on read-only, no exception.
        r2 = mnemo.open(path, "pw", read_only=True)
        r2.close()  # Was: RuntimeError before PR 10a.2.
        del r2
        # 3. export_encrypted on read-only.
        r3 = mnemo.open(path, "pw", read_only=True)
        backup = os.path.join(d, "ro-backup.mnemo")
        r3.export_encrypted(backup)
        del r3
        # Backup is a valid encrypted database.
        with mnemo.open(backup, "pw") as b:
            assert len(b) == 1
    print("ok  read-only close() / __exit__ / export_encrypted no longer raise")


def test_remember_rejects_zero_vector():
    """`remember` refuses an all-zero vector (Phase 3 PR 10a breaking
    change). Scaffold manifest uses a sentinel spike so it still
    inserts."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=3)
        try:
            db.remember("empty", "semantic", [0.0, 0.0, 0.0])
            raise AssertionError("expected RuntimeError on all-zero vector")
        except RuntimeError as e:
            assert "all-zero vector" in str(e), f"got {e}"
        # A single non-zero still works.
        db.remember("ok", "semantic", [1.0, 0.0, 0.0])
        db.close()
    print("ok  remember rejects all-zero vector")


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


def test_compact_file_with_key():
    """v0.13.1 (RB-003.6): `mnemo.compact_file_with_key(path, key)` is
    `compact_file` for a raw-key file, under the same key."""
    key = mnemo.generate_key()
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "k.mnemo")
        db = mnemo.open_with_key(path, key, dimensions=2)
        keep = db.remember("keep", "semantic", [1.0, 0.0])
        drop = db.remember("drop", "semantic", [0.0, 1.0])
        db.delete(drop)
        db.close()
        del db
        report = mnemo.compact_file_with_key(path, key)
        assert report["before"] == 1 and report["after"] == 1, report
        db = mnemo.open_with_key(path, key)
        assert len(db) == 1 and db.get(keep)["content"] == "keep"
        db.close()
        try:
            mnemo.compact_file_with_key(path, b"short")
            raise AssertionError("a short key should be refused")
        except ValueError as e:
            assert "32 bytes" in str(e), e
    print("ok  mnemo.compact_file_with_key(path, key)")


def test_recall_accepts_n_probe_n_rerank_overrides():
    """PR ANN-1: Python callers can tune the ANN index per query.

    Before ANN-1, `n_probe` and `n_rerank` were only accessible from
    Rust (`RecallRequest::n_probe/n_rerank`) and the CLI — Python users
    had no handle on the index tuning dials at all. Both `recall` and
    `recall_text` take them now, and `None` means "use what the index
    was built with" (the ANN-1 auto-formulas).
    """
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        with mnemo.open(path, "pw", dimensions=4) as db:
            # 40 one-hot vectors across 4 axes — enough that an index
            # has partitions and PQ codes to work with.
            for i in range(40):
                v = [float(i % 4 == j) for j in range(4)]
                db.remember(f"m{i}", "semantic", v)
            db.flush()
            db.build_index()
            assert db.has_index()

            query = [1.0, 0.0, 0.0, 0.0]

            # None / omitted (default) — uses the index's own n_probe /
            # n_rerank. Baseline that the explicit calls below are
            # compared against.
            default_hits = db.recall(query, top_k=5)
            assert len(default_hits) == 5

            # Explicit override: tiny shortlist. The call must succeed
            # and return at most top_k results. Correctness is tested
            # by the Rust-side ANN tests; here we're just confirming
            # the kwargs plumb through without raising or being
            # silently ignored.
            tight = db.recall(query, top_k=5, n_probe=1, n_rerank=5)
            assert len(tight) <= 5

            # Explicit override: generous shortlist. Same contract.
            wide = db.recall(query, top_k=5, n_probe=10, n_rerank=100)
            assert len(wide) == 5

            # `recall_text` takes the same kwargs. Need an embedder
            # registered first — a trivial identity-ish one is fine
            # for the plumbing test.
            def _canned(texts):
                # One hot vector per text (same shape as the corpus).
                return [[1.0, 0.0, 0.0, 0.0] for _ in texts]

            db.set_embedder(_canned, name="test-canned", dimensions=4, normalize=False)
            text_hits = db.recall_text(
                "anything", top_k=3, n_probe=2, n_rerank=20
            )
            assert len(text_hits) <= 3
        del db
    print("ok  recall / recall_text accept n_probe + n_rerank")


def test_session_methods_fail_cleanly_after_db_close():
    """PY-1 (R-001.11): a Session borrows the Mnemo it was opened
    from. If the user closes the db while the session is still open,
    the session's methods must surface a clean 'database is closed'
    error (via the closed-aware core_mut() accessor), not panic or
    silently succeed on stale state."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=4)
        session = db.session("agent")
        db.close()

        for op, call in [
            ("add_turn", lambda: session.add_turn(mnemo.Turn(
                role="user", content="hi", vector=[1.0, 0.0, 0.0, 0.0]))),
            ("recall", lambda: session.recall([1.0, 0.0, 0.0, 0.0])),
            ("close", lambda: session.close()),
        ]:
            try:
                call()
            except RuntimeError as e:
                assert "closed" in str(e), f"{op}: wanted 'closed', got {e!r}"
            else:
                raise AssertionError(
                    f"session.{op}() after db.close() should have raised RuntimeError"
                )
    print("ok  session methods raise cleanly after db.close()")


def test_embedder_callback_can_reenter_db_without_already_borrowed():
    """PY-1 (R-001.11): the Python embedder callback runs *after* we
    drop the borrow on the Rust core (by cloning the embedder Arc under
    a short borrow), so a callback that touches `db` itself doesn't
    hit PyO3's 'Already borrowed' panic. Pre-PY-1 the borrow spanned
    the callback, so the first `db.dimensions()` from inside the
    embedder deadlocked or panicked."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        with mnemo.open(path, "pw", dimensions=4) as db:
            saw_dims = {}

            def reentrant_embedder(texts):
                # Re-enter the Mnemo mid-callback — pre-PY-1 crash shape.
                saw_dims["dims"] = db.dimensions()
                return [[1.0, 0.0, 0.0, 0.0] for _ in texts]

            db.set_embedder(reentrant_embedder, name="re", dimensions=4,
                            normalize=False)
            db.insert_default_manifest()
            db.flush()
            # Drive the embedder once.
            db.remember_text("hello", "semantic")
            assert saw_dims["dims"] == 4
    print("ok  embedder callback can re-enter db.dimensions()")


def test_background_thread_makes_progress_during_long_flush():
    """PY-1 (R-001.11): heavy calls release the GIL. A background
    Python thread must make progress while the main thread is in a
    blocking recall / flush / verify. We measure "progress" by having
    the thread tick a counter in a tight Python loop and assert its
    tick count rises during the main thread's heavy call.

    We warm up the ticker first and snapshot its count *immediately
    before* calling the heavy op, so the assertion is "ticks rose
    during the heavy call", not "ticks > 0 at any point" — which
    would pass trivially from the ticker's own startup.

    The threshold is deliberately generous (>= 2 extra ticks during
    the blocking call) — the point is "any Python thread ran at all
    during the blocking call", which pre-PY-1 was zero because the
    GIL stayed pinned."""
    import threading
    import time as _time

    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "pw", dimensions=64)
        # Seed enough records that `verify()` takes tens of ms even on
        # a hardware-AES M1 Pro. 2000 records × 64-dim f32 vectors
        # ~= 2000 × ~500B = ~1 MB of record bodies; `verify()`
        # decrypts every one of them.
        for i in range(2000):
            v = [float((i + k) % 17 + 1) for k in range(64)]
            db.remember(f"m{i}", "semantic", v)
        db.flush()

        ticks = [0]
        stop = threading.Event()

        def ticker():
            while not stop.is_set():
                ticks[0] += 1
                # Yield the GIL frequently so the ticker races the
                # main thread fairly during the blocking call.
                _time.sleep(0.0001)

        t = threading.Thread(target=ticker)
        t.start()
        # Warm up: let the ticker get going before we start timing.
        _time.sleep(0.02)
        before = ticks[0]
        try:
            db.verify()  # heavy call; releases GIL on PY-1
        finally:
            stop.set()
            t.join()
        after_verify = ticks[0]
        during = after_verify - before

        db.close()
        assert during >= 2, (
            f"background thread made no progress during verify() — "
            f"GIL was held (before={before}, after={after_verify}, "
            f"during={during})"
        )
    print(f"ok  background thread ticked {during} times during blocking call")


def test_mnemo_alias_is_as_import_only_and_wheel_ships_no_mnemo_package():
    """NAME-1 (Option A, 2026-11-05 Planner DECISION): the v0.7.0
    rename **drops** the `import mnemo` alias instead of shipping it
    as a pure-Python shim alongside the extension. Rationale: while
    our wheel wrote a top-level `mnemo/` package, it still silently
    clobbered (or was clobbered by) PyPI `mnemo-db` and the old
    `mnemo` 0.0.2 — exactly the collision NAME-1 exists to end.
    Users migrate with one edit: `import mnemo_engine as mnemo`.

    This test enforces both halves of that decision:

    (a) `import mnemo_engine as mnemo` works end-to-end — same
        objects as the direct `import mnemo_engine`, full public
        API surface exposed under the local alias.
    (b) The installed wheel ships **no** top-level `mnemo` package,
        checked against `importlib.metadata.files("mnemo-engine")`.
        This is robust even when some other `mnemo` package happens
        to be installed in the same environment; it also guards
        against the alias accidentally sneaking back in via a
        `[tool.maturin] python-packages = ["mnemo"]` setting at some
        future refactor."""
    # Part (a): `import mnemo_engine as mnemo` is the recommended
    # drop-in — same objects as the direct form, full public API.
    import mnemo_engine as mnemo
    import mnemo_engine as direct
    for name in ("Mnemo", "Session", "Turn", "open", "compact_file",
                 "build_info", "__version__"):
        assert hasattr(mnemo, name), (
            f"`import mnemo_engine as mnemo` missing public name `{name}`"
        )
        assert getattr(mnemo, name) is getattr(direct, name), (
            f"`as` alias should be the same object as the direct import "
            f"for `{name}`"
        )

    # Part (b): our wheel must ship no `mnemo/` top-level package.
    # `importlib.metadata.files` gives the paths the wheel installed
    # under site-packages; any entry at `mnemo/...` would mean the
    # alias snuck back in (or we picked up an unrelated wheel).
    import importlib.metadata
    import pathlib
    files = importlib.metadata.files("mnemo-engine") or []
    mnemo_owned = [
        str(pathlib.PurePath(str(f))) for f in files
        if pathlib.PurePath(str(f)).parts[:1] == ("mnemo",)
    ]
    assert not mnemo_owned, (
        f"mnemo-engine wheel shipped files under `mnemo/`, which would "
        f"clash with mnemo-db and the old mnemo PyPI package — the exact "
        f"collision NAME-1 exists to end. Offending paths: {mnemo_owned}"
    )
    print(
        "ok  `import mnemo_engine as mnemo` works + wheel ships no `mnemo/` "
        "package (NAME-1 Option A)"
    )


def test_rebuild_index_reclusters_under_current_defaults():
    """BP.1: `db.rebuild_index()` re-runs `build_index_with(IndexConfig::
    default())`, so an index built under old defaults picks up the
    current auto-formulas (ANN-1.1: `pq_subspaces = dims/4` etc).
    Before BP.1 Python had no such method — the v0.6.0 CHANGELOG
    promised it but called it raised `AttributeError`.
    """
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "r.mnemo")
        with mnemo.open(path, "pw", dimensions=4) as db:
            # Seed enough vectors to actually have an ANN index.
            for i in range(40):
                v = [float((i + k) % 7 + 1) for k in range(4)]
                db.remember(f"m{i}", "semantic", v)
            db.flush()
            db.build_index()
            db.flush()
            # Rebuild should succeed and leave the index in place.
            db.rebuild_index()
            db.flush()
            assert db.has_index(), "rebuild must leave an index behind"
            assert len(db) == 40, "rebuild must not drop memories"
    print("ok  db.rebuild_index() runs + persists")


def test_history_default_slot_matches_remember_default_slot():
    """v0.8.1 bugfix (lane A): `remember(key=K)` with no explicit
    agent writes to the default agent's private slot; `history(K)`
    with no arguments must walk that same slot — pre-v0.8.1 it
    walked the shared slot by accident and returned []."""
    import tempfile, os
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "hist.mnemo")
        db = mnemo.open(path, "pw", dimensions=4)
        try:
            r1 = db.remember(
                "v1", "semantic", [1.0, 0.0, 0.0, 0.0], key="release"
            )
            r2 = db.remember(
                "v2", "semantic", [0.0, 1.0, 0.0, 0.0], key="release"
            )
            assert isinstance(r1, dict) and r1["outcome"] == "added"
            assert isinstance(r2, dict) and r2["outcome"] == "superseded"

            # The common-case call: no agent_id, no shared.
            chain = db.history("release")
            assert len(chain) == 2, (
                f"history('release') should return the private chain "
                f"(2 entries); got {len(chain)}"
            )
            assert chain[0]["content"] == "v2"
            assert chain[0]["state"] == "live"
            assert chain[1]["content"] == "v1"
            assert chain[1]["state"] == "superseded"

            # Shared slot is empty (we only wrote private).
            shared = db.history("release", shared=True)
            assert shared == [], f"shared slot should be empty, got {shared}"

            # Named-agent lookup is also independent.
            alice = db.history("release", agent_id="alice")
            assert alice == [], f"'alice' slot should be empty, got {alice}"

            # `shared=True` with `agent_id=...` is rejected.
            try:
                db.history("release", agent_id="alice", shared=True)
            except ValueError as e:
                assert "mutually exclusive" in str(e)
            else:
                raise AssertionError("expected ValueError on shared+agent_id")
        finally:
            db.close()

    print("ok  history default slot matches remember default slot (v0.8.1 bugfix)")


def test_thread_start_and_essence_continuation():
    """Phase 12.7c: thread_start links threads; essence(scope=new) pulls prev."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "cont.mnemo")
        db = mnemo.open(path, "passphrase", dimensions=4)
        try:
            # Prev thread: an open item, a next item, and a milestone.
            db.essence_add("open", "finish 12.7c", thread="t1")
            db.essence_add("next", "release v0.9.0", thread="t1")
            db.essence_append("first milestone", thread="t1")

            link_id = db.thread_start("t2", "t1")
            assert isinstance(link_id, str) and len(link_id) == 26, \
                f"thread_start should return a ULID string; got {link_id!r}"

            db.essence_add("intent", "pick up where t1 left off", thread="t2")
            rendered = db.essence(thread="t2", budget=4096)
            assert "continued from" in rendered, \
                f"missing continuation prelude:\n{rendered}"
            assert "finish 12.7c" in rendered, \
                f"prev open missing:\n{rendered}"
            assert "release v0.9.0" in rendered, \
                f"prev next missing:\n{rendered}"
            assert "first milestone" in rendered, \
                f"prev timeline missing:\n{rendered}"
            assert "pick up where t1 left off" in rendered, \
                f"new intent missing:\n{rendered}"
        finally:
            db.close()
    print("ok  thread_start + essence continuation (12.7c)")


def test_commit_deltas_end_to_end():
    """Phase 12.7c: commit_deltas applies facts + essence + turns atomically."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "batch.mnemo")
        db = mnemo.open(path, "passphrase", dimensions=4)
        try:
            db.set_embedder(_fake_embedder(4), name="fake-4d", dimensions=4,
                            normalize=True)
            report = db.commit_deltas({
                "facts": [{
                    "content": "fact-a",
                    "memory_type": "semantic",
                    "vector": [1.0, 0.0, 0.0, 0.0],
                    "importance": 0.8,
                }],
                "essence_ops": [{
                    "kind": "add",
                    "thread": "tA",
                    "section": "intent",
                    "text": "ship it",
                }],
                "turns": [{
                    "turn_id": "u-1",
                    "role": "user",
                    "content": "hello",
                }],
                "thread_id": "tA",
                "turn_policy": "archive",
            })
            assert report["facts_added"] == 1, report
            assert report["essence_ops_applied"] == 1, report
            assert report["turns_added"] == 1, report
            out = db.essence(thread="tA", budget=2048)
            assert "ship it" in out, f"essence not rendered:\n{out}"
        finally:
            db.close()
    print("ok  commit_deltas end-to-end (12.7c)")


def test_commit_deltas_validation_is_atomic():
    """Phase 12.7c: a dim-mismatched fact fails the whole batch; no write."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "atomic.mnemo")
        db = mnemo.open(path, "passphrase", dimensions=4)
        try:
            before = len(db)
            try:
                db.commit_deltas({
                    "facts": [
                        {
                            "content": "ok",
                            "memory_type": "semantic",
                            "vector": [1.0, 0.0, 0.0, 0.0],
                        },
                        {
                            "content": "wrong-dim",
                            "memory_type": "semantic",
                            "vector": [1.0, 0.0],
                        },
                    ],
                })
            except RuntimeError as e:
                msg = str(e).lower()
                assert "dim" in msg or "vector" in msg, msg
            else:
                raise AssertionError("expected RuntimeError on dim mismatch")
            after = len(db)
            assert before == after, \
                f"commit_deltas must be atomic on validation failure: {before} -> {after}"
        finally:
            db.close()
    print("ok  commit_deltas validation is atomic (12.7c)")



def test_commit_deltas_text_only_facts_use_one_embed_call():
    """12.7d: facts without `vector` are embedded with the registered
    embedder, together with essence texts and turns, in one call."""
    calls = []
    inner = _fake_embedder(4)

    def counting(texts):
        calls.append(len(texts))
        return inner(texts)

    with tempfile.TemporaryDirectory() as d:
        db = mnemo.open(os.path.join(d, "embed.mnemo"), "passphrase", dimensions=4)
        try:
            db.insert_default_manifest()
            db.set_embedder(counting, name="fake-4d", dimensions=4, normalize=True)
            calls.clear()
            report = db.commit_deltas({
                "facts": [
                    {"content": "the user prefers tea", "memory_type": "semantic"},
                    {"content": "explicit", "memory_type": "semantic",
                     "vector": [0.0, 1.0, 0.0, 0.0]},
                ],
                "essence_ops": [{"kind": "add", "thread": "t", "section": "intent",
                                 "text": "ship 12.7d"}],
                "turns": [{"turn_id": "u1", "role": "user", "content": "hello"}],
                "thread_id": "t",
            })
            assert report["facts_added"] == 2, report
            assert report["essence_ops_applied"] == 1, report
            assert report["turns_added"] == 1, report
            assert calls == [3], f"expected one embed call of 3 texts, got {calls}"
            hits = db.recall_text("the user prefers tea", top_k=1)
            assert hits and hits[0]["content"] == "the user prefers tea", hits
        finally:
            db.close()
    print("ok  commit_deltas embeds text-only facts in one call (12.7d)")


def test_thread_export_is_ingest_shape_and_round_trips():
    """12.7d: thread_export returns {turn_id, role, content, timestamp}
    dicts in order; re-ingesting them skips every turn."""
    with tempfile.TemporaryDirectory() as d:
        db = mnemo.open(os.path.join(d, "export.mnemo"), "passphrase", dimensions=4)
        try:
            db.insert_default_manifest()
            db.set_embedder(_fake_embedder(4), name="fake-4d", dimensions=4, normalize=True)
            long_text = "x" * 10_000  # chunked at 8 KB
            db.ingest_thread("t", [
                {"turn_id": "b", "role": "user", "content": "first", "timestamp": 1700000000},
                {"turn_id": "a", "role": "assistant", "content": long_text},
            ])
            exported = db.thread_export("t")
            assert [t["turn_id"] for t in exported] == ["b", "a"], exported
            assert exported[0] == {"turn_id": "b", "role": "user", "content": "first",
                                   "timestamp": 1700000000}, exported[0]
            assert exported[1]["content"] == long_text
            assert "vector" not in exported[1] and "metadata" not in exported[1]
            again = db.ingest_thread("t", exported)
            assert again["added_records"] == 0, again
            assert again["skipped_existing_turns"] == 2, again
        finally:
            db.close()
    print("ok  thread_export ingest shape + no-op round trip (12.7d)")


def test_about_essence():
    """12.7d: about_essence() renders the project essence, or None."""
    with tempfile.TemporaryDirectory() as d:
        db = mnemo.open(os.path.join(d, "about.mnemo"), "passphrase", dimensions=4)
        try:
            db.insert_default_manifest()
            assert db.about_essence() is None
            db.essence_add("conventions", "repeat relay prompts verbatim", project=True)
            render = db.about_essence()
            assert render and "repeat relay prompts verbatim" in render, render
        finally:
            db.close()
    print("ok  about_essence (12.7d)")


def test_open_with_key():
    """PERF-1 step 7: raw-key databases."""
    key = mnemo.generate_key()
    assert isinstance(key, bytes) and len(key) == 32
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "keyed.mnemo")
        db = mnemo.open_with_key(path, key, dimensions=4)
        db.remember("held by a keychain", "semantic", [1.0, 0.0, 0.0, 0.0])
        db.close()
        db = mnemo.open_with_key(path, key, read_only=True)
        assert len(db) == 1
        db.close()
        try:
            mnemo.open_with_key(path, mnemo.generate_key())
        except RuntimeError:
            pass
        else:
            raise AssertionError("a wrong key must not open the file")
        try:
            mnemo.open_with_key(path, b"short")
        except ValueError:
            pass
        else:
            raise AssertionError("a key that isn't 32 bytes is a ValueError")
        key2 = mnemo.generate_key()
        db = mnemo.open_with_key(path, key)
        db.rekey_with_key(key2)
        db.close()
        db = mnemo.open_with_key(path, key2)
        assert len(db) == 1
        db.close()
    print("ok  open_with_key / generate_key / rekey_with_key (PERF-1 step 7)")


def test_keyword_and_hybrid_recall_on_v9():
    """FMT-9d: `keywords` on a v9 file (the default format from v0.11)."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "passphrase", dimensions=4)
        assert db.stats()["format_version"] == 9, "new files are v9"
        db.remember("shipped BP.1 with the new parser", "semantic",
                    [1.0, 0.0, 0.0, 0.0])
        db.remember("BP review, item 1", "semantic", [0.0, 1.0, 0.0, 0.0])
        db.remember("the user likes tea", "semantic", [0.0, 0.0, 1.0, 0.0])
        db.flush()
        # Keyword-only: no vector, no embedder.
        hits = db.recall([], keywords="BP.1", top_k=3)
        assert hits[0]["content"].startswith("shipped BP.1"), hits
        assert all("tea" not in h["content"] for h in hits)
        # Hybrid: the keyword match lifts a weaker vector match.
        hits = db.recall([0.0, 0.0, 1.0, 0.2], keywords="BP.1",
                         keyword_weight=2.0, top_k=1)
        assert hits[0]["content"].startswith("shipped BP.1"), hits
        db.close()
    print("ok  keyword and hybrid recall (v9)")


def test_turn_compression_knob_on_v9():
    """FMT-9e: set_turn_compress_after on a v9 file; turns round-trip
    whether they're stored plain or compressed."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "passphrase", dimensions=4)
        db.set_embedder(_fake_embedder(4), name="fake-4d", dimensions=4, normalize=True)
        text = "the agent decided to flush after tests pass " * 60
        db.ingest_thread("recent", [{"turn_id": "r", "role": "user", "content": text}])
        db.set_turn_compress_after(0)
        db.ingest_thread("now", [{"turn_id": "n", "role": "user", "content": text}])
        db.flush()
        db.close()
        db = mnemo.open(path, "passphrase", dimensions=4)
        assert db.thread_export("recent")[0]["content"] == text
        assert db.thread_export("now")[0]["content"] == text
        db.close()
    print("ok  turn compression knob (v9)")


def test_recall_text_keywords_and_vector_cache_budget():
    """v0.11: `recall_text(keywords=)` (hybrid recall from text) and
    `set_vector_cache_budget` (a budget below the column pages it)."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        db = mnemo.open(path, "passphrase", dimensions=4)
        # Every text embeds to the same vector, so only the keywords can
        # separate the candidates.
        db.set_embedder(lambda texts: [[1.0, 0.5, 0.25, 0.125] for _ in texts],
                        name="flat-4d", dimensions=4, normalize=False)
        db.remember_text("shipped BP.1 with the new parser", "semantic")
        db.remember_text("BP review, item 1", "semantic")
        db.remember_text("the user likes tea", "semantic")
        db.flush()
        hits = db.recall_text("which release had the parser?", top_k=3,
                              keywords="BP.1", keyword_weight=2.0)
        assert hits[0]["content"].startswith("shipped BP.1"), hits
        plain = db.recall_text("which release had the parser?", top_k=3)
        assert len(plain) == 3

        want = [h["id"] for h in db.recall([1.0, 0.0, 0.0, 0.0], top_k=3,
                                           track_access=False)]
        db.set_vector_cache_budget(0)  # paged: same results, less memory
        got = [h["id"] for h in db.recall([1.0, 0.0, 0.0, 0.0], top_k=3,
                                          track_access=False)]
        assert got == want, (got, want)
        db.set_vector_cache_budget(96 * 1024 * 1024)
        db.close()
    print("ok  recall_text keywords + set_vector_cache_budget (v0.11)")

if __name__ == "__main__":
    print(f"mnemo {mnemo.__version__}")
    test_create_remember_recall()
    test_persistence_and_reopen()
    test_close_releases_lock_and_blocks_further_use()
    test_with_block_releases_lock_on_exit()
    test_session_methods_fail_cleanly_after_db_close()
    test_embedder_callback_can_reenter_db_without_already_borrowed()
    test_background_thread_makes_progress_during_long_flush()
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
    test_set_embedder_and_text_round_trip()
    test_set_embedder_dim_mismatch_rejected()
    test_set_embedder_identity_mismatch_rejected_after_reopen()
    test_set_embedder_identity_checked_without_a_manifest()
    test_set_embedder_on_read_only_handle_adopts_without_write()
    test_read_only_with_block_and_close_are_noops()
    test_remember_rejects_zero_vector()
    test_compact_file_module_fn()
    test_compact_file_with_key()
    test_recall_accepts_n_probe_n_rerank_overrides()
    test_rebuild_index_reclusters_under_current_defaults()
    test_mnemo_alias_is_as_import_only_and_wheel_ships_no_mnemo_package()
    test_history_default_slot_matches_remember_default_slot()
    test_thread_start_and_essence_continuation()
    test_commit_deltas_end_to_end()
    test_commit_deltas_validation_is_atomic()
    test_commit_deltas_text_only_facts_use_one_embed_call()
    test_thread_export_is_ingest_shape_and_round_trips()
    test_about_essence()
    test_open_with_key()
    test_keyword_and_hybrid_recall_on_v9()
    test_turn_compression_knob_on_v9()
    test_recall_text_keywords_and_vector_cache_budget()
    print("\nall Python binding tests passed")
