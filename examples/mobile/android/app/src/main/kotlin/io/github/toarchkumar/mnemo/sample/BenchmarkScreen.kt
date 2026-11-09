package io.github.toarchkumar.mnemo.sample

import android.content.Context
import android.content.Intent
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

@Composable
fun BenchmarkScreen() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var running by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf<File?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val log = remember { mutableStateListOf<String>() }
    val summary = remember { mutableStateListOf<String>() }
    val small = MaterialTheme.typography.bodySmall

    LazyColumn(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item {
            Text("Benchmark", style = MaterialTheme.typography.headlineSmall, modifier = Modifier.padding(top = 16.dp))
            Text(
                "The device_budget cells at 1K and 10K memories (384-dim synthetic vectors, raw-key files). " +
                    "Use the release build, keep the app in the foreground and the phone unplugged. " +
                    "The first run takes a few minutes while it builds the corpus.",
                style = small,
            )
            Button(
                enabled = !running,
                onClick = {
                    running = true
                    error = null
                    log.clear()
                    summary.clear()
                    keepScreenOn(context, true)
                    scope.launch {
                        try {
                            val doc = withContext(Dispatchers.Default) {
                                DeviceBudget(context.applicationContext) { line ->
                                    scope.launch { log.add(0, line) }
                                }.run()
                            }
                            result = write(context, doc)
                            summary.addAll(summarize(doc))
                        } catch (e: Exception) {
                            logError(e)
                            error = e.toString()
                        } finally {
                            running = false
                            keepScreenOn(context, false)
                        }
                    }
                },
            ) { Text(if (running) "Running..." else "Run the benchmark") }
            result?.let { file ->
                OutlinedButton({ share(context, file) }) { Text("Share the JSON") }
                Text(file.path, style = small)
            }
            error?.let { Text(it, color = Color.Red, style = small) }
        }
        items(summary) { Text(it, style = small.copy(fontFamily = FontFamily.Monospace)) }
        items(log) { Text(it, style = small) }
    }
}

private fun keepScreenOn(context: Context, on: Boolean) {
    val window = (context as? ComponentActivity)?.window ?: return
    if (on) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    else window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
}

/** App files (external, so `adb pull` reaches it): Android/data/<package>/files/device_budget/. */
private fun write(context: Context, doc: JSONObject): File {
    val dir = File(context.getExternalFilesDir(null) ?: context.filesDir, "device_budget").apply { mkdirs() }
    val device = doc.getJSONObject("device").optString("model_identifier", "device").replace(Regex("[^A-Za-z0-9._-]"), "_")
    val stamp = doc.getString("timestamp").replace(":", "").substringBefore('.')
    return File(dir, "device_budget_${device}_$stamp.json").apply { writeText(doc.toString(2)) }
}

/** Shares the JSON as text, so no FileProvider is needed. */
private fun share(context: Context, file: File) {
    val send = Intent(Intent.ACTION_SEND).apply {
        type = "application/json"
        putExtra(Intent.EXTRA_SUBJECT, file.name)
        putExtra(Intent.EXTRA_TEXT, file.readText())
    }
    context.startActivity(Intent.createChooser(send, "Share ${file.name}"))
}

private fun summarize(doc: JSONObject): List<String> {
    val results = doc.getJSONObject("results")
    return results.keys().asSequence().sortedBy { it.toInt() }.map { n ->
        val c = results.getJSONObject(n)
        fun num(phase: String, key: String) = c.getJSONObject(phase).optDouble(key)
        "%s: %.0f B/mem, handle %.1f MB (PSS %.1f), key open %.1f ms, warm recall %.2f ms (p95 %.2f), write %.1f ms, close %.1f / reopen %.1f ms".format(
            n, num("file", "bytes_per_memory"), num("handle", "handle_rss_kb") / 1024, num("handle", "handle_pss_kb") / 1024,
            num("keyopen", "open_with_key_median_ms"), num("handle", "warm_recall_median_ms"), num("handle", "warm_recall_p95_ms"),
            num("writes", "single_write_median_ms"), num("background", "close_median_ms"), num("background", "reopen_median_ms"),
        )
    }.toList()
}
