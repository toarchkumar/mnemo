package io.github.toarchkumar.mnemo

import java.io.File
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The flush policy and sessions (MOBILE-2b), the counterpart of the Swift
 * FlushPolicyTests. "Committed" means on disk: a byte copy of the file,
 * opened on its own, has it. That's what survives a kill; the kill itself
 * (`am force-stop`) runs on the emulator, in scripts/emulator-tests.sh.
 */
class FlushPolicyTests {
    private fun store(name: String, policy: FlushPolicy, keys: MnemoKeyStorage = InMemoryKeyStorage()) =
        MnemoStore(MnemoStoreConfiguration(name, 4u, scratchDirectory(), flushPolicy = policy), keys)

    @Test fun theDefaultIsCoalesced() {
        assertEquals(FlushPolicy.Coalesced(1_000, 64), MnemoStoreConfiguration("x", 4u).flushPolicy)
        assertEquals(FlushPolicy.Coalesced(), FlushPolicy.DEFAULT)
    }

    @Test fun coalescedCommitsOnceWithinTheWindow(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("window", FlushPolicy.Coalesced(300, 64), keys)
        repeat(3) { s.remember("note $it", vec(it)) }
        assertEquals(0, s.commits)
        assertEquals(0uL, committedCount(s.file, keys))
        assertTrue(eventually(5_000) { s.commits == 1 })
        assertEquals(3uL, committedCount(s.file, keys))
        delay(600)
        assertEquals("no second commit without new writes", 1, s.commits)
        s.close()
    }

    @Test fun aBurstOfMaxWritesIsOneCommit(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("burst", FlushPolicy.DEFAULT, keys)
        repeat(64) { s.remember("note $it", vec(it)) }
        assertEquals("the 64th write commits", 1, s.commits)
        delay(1_300)
        assertEquals("and the timer it cancelled doesn't fire", 1, s.commits)
        assertEquals(64uL, committedCount(s.file, keys))
        s.close()
    }

    @Test fun aBurstOf100WritesIsOneCommit(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("burst100", FlushPolicy.Coalesced(300, 1_000), keys)
        repeat(100) { s.remember("note $it", vec(it)) }
        assertEquals(0, s.commits)
        assertTrue(eventually(5_000) { s.commits == 1 })
        delay(600)
        assertEquals(1, s.commits)
        assertEquals(100uL, committedCount(s.file, keys))
        s.close()
    }

    @Test fun everyWriteCommitsEachTime(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("every", FlushPolicy.EveryWrite, keys)
        for (i in 0 until 10) {
            s.remember("note $i", vec(i))
            assertEquals(i + 1, s.commits)
            assertEquals((i + 1).toULong(), committedCount(s.file, keys))
        }
        s.close()
    }

    @Test fun manualCommitsOnlyWhenAsked(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("manual", FlushPolicy.Manual, keys)
        repeat(5) { s.remember("note $it", vec(it)) }
        delay(1_300)
        assertEquals(0, s.commits)
        assertEquals(0uL, committedCount(s.file, keys))
        s.flush()
        assertEquals(1, s.commits)
        assertEquals(5uL, committedCount(s.file, keys))
        s.close()
    }

    @Test fun whileStoppedACoalescedWriteCommitsBeforeItReturns(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("stopped", FlushPolicy.DEFAULT, keys)
        s.processStopped = true
        s.remember("in the background", vec(0))
        assertEquals(1, s.commits)
        assertEquals(1uL, committedCount(s.file, keys))
        s.onProcessStart()
        s.remember("in the foreground", vec(1))
        assertEquals(1, s.commits)
        assertEquals(1uL, committedCount(s.file, keys))
        assertTrue(eventually(5_000) { s.commits == 2 })
        s.close()
    }

    @Test fun aTimerOnAClosedStoreDoesNotReopenIt(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("closed", FlushPolicy.Coalesced(300, 64), keys)
        s.remember("one", vec(0))
        s.close()
        assertEquals(1, s.commits)
        delay(700)
        assertFalse(s.isOpen)
        assertEquals(1, s.commits)
        assertUnlockedAndDurable(s.file, keys, 1uL)
    }

    @Test fun onStopCommitsAndCancelsThePendingCommit(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("stopcancel", FlushPolicy.Coalesced(300, 64), keys)
        s.remember("one", vec(0))
        s.onProcessStop()
        assertTrue(eventually { s.backgroundFlushes == 1 })
        assertEquals(1, s.commits)
        delay(700)
        assertEquals("the timer was cancelled", 1, s.commits)
        assertEquals(1uL, committedCount(s.file, keys))
        s.close()
    }

    @Test fun aSessionIsOneCommit(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("session", FlushPolicy.DEFAULT, keys)
        val kept = s.withSession { session ->
            repeat(100) { session.remember("note $it", vec(it)) }
            assertEquals(100uL, session.count())
            session
        }
        assertEquals(1, s.commits)
        assertEquals(100uL, committedCount(s.file, keys))
        expectThrows<MnemoException.Closed> { kept.count() }
        delay(1_300)
        assertEquals(1, s.commits)
        s.close()
    }

    @Test fun aSessionThatThrowsStillCommitsItsWrites(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("sessionthrow", FlushPolicy.DEFAULT, keys)
        expectThrows<IllegalStateException> {
            s.withSession { session ->
                session.remember("kept", vec(0))
                throw IllegalStateException("stop")
            }
        }
        assertEquals(1, s.commits)
        assertEquals(1uL, committedCount(s.file, keys))
        expectThrows<MnemoException.Invalid> {
            s.withSession { it.update("01J00000000000000000000000", MemoryPatch(content = "no vector")) }
        }
        s.close()
    }

    @Test fun aManualSessionDoesNotCommit(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val s = store("sessionmanual", FlushPolicy.Manual, keys)
        s.withSession { session -> repeat(5) { session.remember("note $it", vec(it)) } }
        assertEquals(0, s.commits)
        s.close()
        assertEquals(5uL, committedCount(s.file, keys))
    }
}

/**
 * [committedCount] for polling inside `eventually` while a commit may be
 * running (FLAKE-2): a copy taken mid-commit has a pending WAL, so its
 * read-only open is refused with [MnemoException.NeedsWriteOpen]. That
 * means "not committed yet", so it's null here, never an error. The store
 * is fine: a read-write open of that copy would recover it, as after a kill.
 */
fun committedCountOrNull(file: File, keys: MnemoKeyStorage): ULong? =
    try {
        committedCount(file, keys)
    } catch (e: MnemoException.NeedsWriteOpen) {
        null
    }

/** The memories a byte copy of [file] holds right now: what a kill would leave. */
fun committedCount(file: File, keys: MnemoKeyStorage): ULong {
    val copy = File(file.parentFile, "copy-${System.nanoTime()}.mnemo")
    file.copyTo(copy)
    val key = keys.key(checkNotNull(KeyId.read(file)) { "no key id on $file" })
    try {
        val db = MnemoDb.openReadOnlyWithKey(copy.path, key)
        try {
            return db.len().also { db.closeFile() }
        } finally {
            db.close()
        }
    } finally {
        key.fill(0)
        copy.delete()
    }
}
