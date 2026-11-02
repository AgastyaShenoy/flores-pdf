package io.github.agastyashenoy.flores

import android.content.Context

/** A file opened before, and where it was left. */
data class Recent(
    val uri: String,
    val name: String,
    val page: Int,
    val offset: Float,
    val zoom: Float,
    val pages: Int,
    val opened: Long,
)

/** Recently opened files, newest first, and the reader's settings, in shared preferences. */
class Recents(context: Context) {
    private val prefs = context.getSharedPreferences("flores", Context.MODE_PRIVATE)

    fun list(): List<Recent> = prefs.getString(KEY, "").orEmpty().lineSequence().mapNotNull { line ->
        val f = line.split('\t')
        if (f.size < 7) return@mapNotNull null
        Recent(f[0], f[1], f[2].toIntOrNull() ?: 0, f[3].toFloatOrNull() ?: 0f, f[4].toFloatOrNull() ?: 1f, f[5].toIntOrNull() ?: 0, f[6].toLongOrNull() ?: 0)
    }.toList()

    fun find(uri: String): Recent? = list().firstOrNull { it.uri == uri }

    /** Puts [r] first, replacing any entry for the same file. */
    fun put(r: Recent) {
        save(listOf(r) + list().filter { it.uri != r.uri })
    }

    fun remove(uri: String) {
        save(list().filter { it.uri != uri })
    }

    private fun save(list: List<Recent>) {
        val clean = { s: String -> s.replace('\t', ' ').replace('\n', ' ') }
        val text = list.take(MAX).joinToString("\n") {
            listOf(it.uri, clean(it.name), it.page, it.offset, it.zoom, it.pages, it.opened).joinToString("\t")
        }
        prefs.edit().putString(KEY, text).apply()
    }

    /** Page colors: 0 normal, 1 dark, 2 sepia, 3 black. */
    var look: Int
        get() = prefs.getInt("look", 0)
        set(v) = prefs.edit().putInt("look", v).apply()

    companion object {
        private const val KEY = "recent"
        private const val MAX = 30
    }
}
