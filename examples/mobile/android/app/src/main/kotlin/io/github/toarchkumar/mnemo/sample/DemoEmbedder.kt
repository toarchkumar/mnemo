package io.github.toarchkumar.mnemo.sample

import io.github.toarchkumar.mnemo.EmbedderIdentity
import io.github.toarchkumar.mnemo.MnemoEmbedder
import kotlin.math.sqrt

/**
 * A DEMO embedder: hashed words, so texts that share words come out similar.
 * It knows nothing about meaning, and the UI says so.
 *
 * The sample doesn't bundle a real model: MediaPipe's Text Embedder would
 * add more than the 10 MB budget (tasks-text 1.2.2 ships a 16.6 MB arm64
 * native library, and the older 7.7 MB one isn't 16 KB-aligned). An app
 * plugs its own model in the same way: implement [MnemoEmbedder].
 */
class DemoEmbedder(private val dimensions: Int = 64) : MnemoEmbedder {
    override fun identity() = EmbedderIdentity(name = "demo-word-hash", dimensions = dimensions.toUInt(), normalize = true)

    override fun embed(texts: List<String>): List<List<Float>> = texts.map { text ->
        val v = FloatArray(dimensions)
        for (word in text.lowercase().split(Regex("[^\\p{L}\\p{N}]+")).filter { it.isNotEmpty() }) {
            var h = 2_166_136_261u
            for (b in word.toByteArray(Charsets.UTF_8)) h = (h xor b.toUByte().toUInt()) * 16_777_619u
            v[(h % dimensions.toUInt()).toInt()] += 1f
        }
        if (v.all { it == 0f }) v[0] = 1f
        val norm = sqrt(v.sumOf { (it * it).toDouble() }).toFloat()
        v.map { it / norm }
    }
}
