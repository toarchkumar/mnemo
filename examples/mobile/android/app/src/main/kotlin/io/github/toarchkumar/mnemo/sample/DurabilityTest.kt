package io.github.toarchkumar.mnemo.sample

import android.content.Context
import android.util.Log
import io.github.toarchkumar.mnemo.MnemoStore
import io.github.toarchkumar.mnemo.MnemoStoreConfiguration
import java.io.File
import kotlinx.coroutines.delay

/**
 * The kill test for the default flush policy (MOBILE-2b), driven by
 * scripts/emulator-tests.sh in mnemo/bindings/kotlin:
 *
 * 1. `--es durability write`: a fresh store with the default policy
 *    (coalesced, about 1 s) gets [SETTLED] writes, then waits past the
 *    window and logs `MNEMO_DURABILITY settled`. It then makes [LATE] more
 *    writes and logs `MNEMO_DURABILITY late`, with nothing flushed by hand.
 * 2. The script runs `am force-stop` right after that line: no lifecycle
 *    event, no flush.
 * 3. `--es durability count`: reopens the file and logs
 *    `MNEMO_DURABILITY count=N`. The [SETTLED] writes, older than the window,
 *    must be there; the late ones may not be.
 *
 * Logs under the tag `MnemoDurability`.
 */
object DurabilityTest {
    const val TAG = "MnemoDurability"
    const val SETTLED = 20
    const val LATE = 5

    private fun store(context: Context) = MnemoStore(
        context,
        MnemoStoreConfiguration(name = "durability", dimensions = 4u, directory = File(context.filesDir, "durability")),
    )

    private fun vector(i: Int) = listOf(i + 1f, (i % 3).toFloat(), 0.5f, 1f)

    suspend fun run(context: Context, mode: String): String = try {
        when (mode) {
            "write" -> {
                File(context.filesDir, "durability").deleteRecursively()
                val store = store(context)
                repeat(SETTLED) { store.remember("settled $it", vector(it)) }
                delay(2_500)
                Log.i(TAG, "MNEMO_DURABILITY settled $SETTLED")
                repeat(LATE) { store.remember("late $it", vector(SETTLED + it)) }
                "MNEMO_DURABILITY late $LATE"
            }
            "count" -> {
                val store = store(context)
                val n = store.count()
                store.close()
                "MNEMO_DURABILITY count=$n"
            }
            else -> "MNEMO_DURABILITY failed: unknown mode $mode"
        }
    } catch (e: Throwable) {
        "MNEMO_DURABILITY failed: $e"
    }.also { Log.i(TAG, it) }
}
