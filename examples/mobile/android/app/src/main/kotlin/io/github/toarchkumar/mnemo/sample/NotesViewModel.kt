package io.github.toarchkumar.mnemo.sample

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.toarchkumar.mnemo.EssenceScope
import io.github.toarchkumar.mnemo.EssenceSection
import io.github.toarchkumar.mnemo.ExportedTurn
import io.github.toarchkumar.mnemo.MemoryRecord
import io.github.toarchkumar.mnemo.MemoryType
import io.github.toarchkumar.mnemo.MnemoStore
import io.github.toarchkumar.mnemo.MnemoStoreConfiguration
import io.github.toarchkumar.mnemo.RecallHit
import io.github.toarchkumar.mnemo.RecallOptions
import io.github.toarchkumar.mnemo.RememberOptions
import io.github.toarchkumar.mnemo.ThreadTurn
import java.util.UUID
import kotlinx.coroutines.launch

/** The notes demo: notes, a keyed fact with history, recall, a thread. */
class NotesViewModel(app: Application) : AndroidViewModel(app) {
    val embedderLabel = "DEMO embedder (hashed words): it matches shared words, not meaning"

    private val store = MnemoStore(app, MnemoStoreConfiguration(name = "notes", dimensions = 64u), embedder = DemoEmbedder())

    var count by mutableStateOf(0uL); private set
    var recent by mutableStateOf(listOf<String>()); private set
    var hits by mutableStateOf(listOf<RecallHit>()); private set
    var history by mutableStateOf(listOf<MemoryRecord>()); private set
    var turns by mutableStateOf(listOf<ExportedTurn>()); private set
    var essence by mutableStateOf(""); private set
    var error by mutableStateOf<String?>(null); private set

    init {
        launchSafely {
            count = store.count()
            turns = store.threadExport(THREAD)
            essence = store.essence(EssenceScope.Thread(THREAD), 2_048uL)
        }
    }

    fun addNote(text: String) {
        val note = text.trim()
        if (note.isEmpty()) return
        launchSafely {
            store.rememberText(note, RememberOptions(memoryType = MemoryType.EPISODIC))
            recent = listOf(note) + recent
            count = store.count()
        }
    }

    /** A keyed write: a new value supersedes the old one; the same value again is a no-op. */
    fun setFact(key: String, value: String) {
        val k = key.trim()
        val v = value.trim()
        if (k.isEmpty() || v.isEmpty()) return
        launchSafely {
            store.rememberText("$k: $v", RememberOptions(memoryType = MemoryType.SEMANTIC, key = "fact:$k"))
            history = store.history("fact:$k")
            count = store.count()
        }
    }

    fun loadHistory(key: String) {
        val k = key.trim()
        if (k.isNotEmpty()) launchSafely { history = store.history("fact:$k") }
    }

    /** Recall by vector, or by vector plus the query's exact words. */
    fun recall(query: String, keywords: Boolean) {
        val q = query.trim()
        if (q.isEmpty()) return
        launchSafely {
            hits = store.recallText(q, RecallOptions(topK = 5u, keywords = if (keywords) q else null, trackAccess = false)).hits
        }
    }

    fun addTurn(role: String, text: String) {
        val t = text.trim()
        if (t.isEmpty()) return
        launchSafely {
            store.ingestThread(THREAD, listOf(ThreadTurn(turnId = UUID.randomUUID().toString(), role = role, content = t)))
            turns = store.threadExport(THREAD)
            essence = store.essence(EssenceScope.Thread(THREAD), 2_048uL)
            count = store.count()
        }
    }

    /** An essence item derived from the thread: a decision (editable) or a timeline milestone. */
    fun addEssence(text: String, timeline: Boolean) {
        val t = text.trim()
        if (t.isEmpty()) return
        val scope = EssenceScope.Thread(THREAD)
        launchSafely {
            if (timeline) store.essenceAppendTimeline(t, scope) else store.essenceAdd(EssenceSection.DECISIONS, t, scope)
            essence = store.essence(scope, 2_048uL)
        }
    }

    private fun launchSafely(body: suspend () -> Unit) {
        viewModelScope.launch {
            try {
                body()
                error = null
            } catch (e: Exception) {
                error = e.toString()
            }
        }
    }

    companion object {
        const val THREAD = "demo-thread"
    }
}
