package io.github.toarchkumar.mnemo.sample

import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch

/**
 * The MNemo sample: a notes demo and the device Benchmark. Launched with
 * `--ez selftest true`, it runs [SelfTest] instead and logs the result;
 * with `--es durability write|count`, it runs a step of [DurabilityTest].
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (intent.getBooleanExtra("selftest", false)) {
            setContent { MaterialTheme { Text("MNemo self-test running...") } }
            lifecycleScope.launch {
                val result = SelfTest.run(applicationContext)
                setContent { MaterialTheme { Text(result) } }
            }
            return
        }
        intent.getStringExtra("durability")?.let { mode ->
            setContent { MaterialTheme { Text("MNemo durability test: $mode") } }
            lifecycleScope.launch {
                val result = DurabilityTest.run(applicationContext, mode)
                setContent { MaterialTheme { Text(result) } }
            }
            return
        }
        setContent { MaterialTheme { SampleApp() } }
    }
}

@androidx.compose.runtime.Composable
private fun SampleApp() {
    var tab by rememberSaveable { mutableIntStateOf(0) }
    Scaffold(
        bottomBar = {
            NavigationBar {
                NavigationBarItem(selected = tab == 0, onClick = { tab = 0 }, icon = { Text("N") }, label = { Text("Notes") })
                NavigationBarItem(selected = tab == 1, onClick = { tab = 1 }, icon = { Text("B") }, label = { Text("Benchmark") })
            }
        },
    ) { padding ->
        Box(Modifier.padding(padding)) {
            when (tab) {
                0 -> NotesScreen()
                else -> BenchmarkScreen()
            }
        }
    }
}

internal fun logError(e: Throwable) = Log.w("MnemoSample", e)
