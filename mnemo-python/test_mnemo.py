"""End-to-end test of the mnemo Python bindings.

Run after installing the wheel:  python3 test_mnemo.py
Exits non-zero on the first failed assertion.
"""

import os
import tempfile

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

        # Reopen without dimensions — must read them from the file.
        db2 = mnemo.open(path, "pw")
        assert len(db2) == 1
        got = db2.get(mid)
        assert got["content"] == "persist me"
        assert got["memory_type"] == "procedural"
        db2.close()
    print("ok  persistence / reopen / get")


def test_wrong_passphrase():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "t.mnemo")
        mnemo.open(path, "correct", dimensions=2).close()
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
        # Reopen and roll back to the first snapshot.
        db = mnemo.open(path, "pw")
        info = db.restore_to(first)
        assert info["txn_id"] == first
        db.close()
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
    print("\nall Python binding tests passed")
