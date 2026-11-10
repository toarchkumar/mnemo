package io.github.toarchkumar.mnemo

import android.os.Handler
import android.os.Looper
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import java.lang.ref.WeakReference

/**
 * Tracks the process state for a [MnemoStore] and flushes it when the app
 * goes to the background: `ProcessLifecycleOwner` sends `ON_START` when the
 * first activity starts, and `ON_STOP` shortly after the last one stops.
 * When added, the observer is brought up to the current state, so a
 * process with a started activity gets `ON_START` at once. Holds the store
 * weakly, and removes itself once the store is gone.
 */
internal class ProcessStopObserver private constructor(store: MnemoStore) : DefaultLifecycleObserver {
    private val store = WeakReference(store)

    override fun onStart(owner: LifecycleOwner) {
        val target = store.get()
        if (target == null) {
            owner.lifecycle.removeObserver(this)
        } else {
            target.onProcessStart()
        }
    }

    override fun onStop(owner: LifecycleOwner) {
        val target = store.get()
        if (target == null) {
            owner.lifecycle.removeObserver(this)
        } else {
            target.onProcessStop()
        }
    }

    companion object {
        /** Observes the process lifecycle for [store]; observers are added on the main thread. */
        fun attach(store: MnemoStore) {
            val observer = ProcessStopObserver(store)
            val add = Runnable { ProcessLifecycleOwner.get().lifecycle.addObserver(observer) }
            val main = Looper.getMainLooper()
            if (Looper.myLooper() == main) add.run() else Handler(main).post(add)
        }
    }
}
