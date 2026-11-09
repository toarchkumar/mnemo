package io.github.toarchkumar.mnemo.sample

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel

@Composable
fun NotesScreen(model: NotesViewModel = viewModel()) {
    var note by rememberSaveable { mutableStateOf("") }
    var factKey by rememberSaveable { mutableStateOf("drink") }
    var factValue by rememberSaveable { mutableStateOf("") }
    var query by rememberSaveable { mutableStateOf("") }
    var keywords by rememberSaveable { mutableStateOf(false) }
    var turn by rememberSaveable { mutableStateOf("") }
    var role by rememberSaveable { mutableStateOf("user") }
    var derived by rememberSaveable { mutableStateOf("") }
    val small = MaterialTheme.typography.bodySmall

    LazyColumn(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        item {
            Text("MNemo notes", style = MaterialTheme.typography.headlineSmall, modifier = Modifier.padding(top = 16.dp))
            Text("Memories: ${model.count}")
            Text(model.embedderLabel, style = small, color = Color(0xFFB3261E))
            model.error?.let { Text(it, style = small, color = Color.Red) }
        }
        item { Section("Add a note") }
        item {
            OutlinedTextField(note, { note = it }, Modifier.fillMaxWidth(), label = { Text("Bought oat milk for the office") })
            Button({ model.addNote(note); note = "" }) { Text("Remember") }
            model.recent.take(5).forEach { Text(it, style = small) }
        }
        item { Section("A keyed fact") }
        item {
            OutlinedTextField(factKey, { factKey = it }, Modifier.fillMaxWidth(), label = { Text("Key") })
            OutlinedTextField(factValue, { factValue = it }, Modifier.fillMaxWidth(), label = { Text("Value (tea, then coffee...)") })
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button({ model.setFact(factKey, factValue) }) { Text("Set") }
                OutlinedButton({ model.loadHistory(factKey) }) { Text("Show history") }
            }
            model.history.forEachIndexed { i, record ->
                Text("${record.content}  (${if (i == 0) "current" else "superseded"})", style = small)
            }
            Text("A new value supersedes the old one, which stays in the history. The same value again is a no-op.", style = small)
        }
        item { Section("Recall") }
        item {
            OutlinedTextField(query, { query = it }, Modifier.fillMaxWidth(), label = { Text("What did I buy?") })
            Row(verticalAlignment = Alignment.CenterVertically) {
                Switch(keywords, { keywords = it })
                Text("  Also match the exact words")
            }
            Button({ model.recall(query, keywords) }) { Text("Recall") }
            model.hits.forEach { hit ->
                Column {
                    Text(hit.memory.content)
                    Text("score %.3f, similarity %.3f".format(hit.score, hit.similarity), style = small)
                }
            }
        }
        item { Section("A thread") }
        item {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("user", "assistant").forEach { r ->
                    if (r == role) Button({}) { Text(r) } else OutlinedButton({ role = r }) { Text(r) }
                }
            }
            OutlinedTextField(turn, { turn = it }, Modifier.fillMaxWidth(), label = { Text("A turn of the conversation") })
            Button({ model.addTurn(role, turn); turn = "" }) { Text("Add turn") }
            model.turns.forEach { Text("${it.role}: ${it.content}", style = small) }
            OutlinedTextField(derived, { derived = it }, Modifier.fillMaxWidth(), label = { Text("A decision or a milestone") })
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button({ model.addEssence(derived, timeline = false); derived = "" }) { Text("Add decision") }
                OutlinedButton({ model.addEssence(derived, timeline = true); derived = "" }) { Text("Add to timeline") }
            }
            Text("Essence", style = MaterialTheme.typography.labelMedium)
            Text(model.essence.ifEmpty { "(empty)" }, style = small.copy(fontFamily = FontFamily.Monospace))
            Text(
                "Turns are kept losslessly. The essence holds the small items an agent derives from them, " +
                    "such as decisions (editable) and timeline milestones (append-only).",
                style = small,
                modifier = Modifier.padding(bottom = 24.dp),
            )
        }
    }
}

@Composable
internal fun Section(title: String) {
    HorizontalDivider()
    Text(title, style = MaterialTheme.typography.titleMedium)
}
