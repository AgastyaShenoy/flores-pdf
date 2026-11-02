package io.github.agastyashenoy.flores

import android.content.Context
import java.io.File
import java.security.MessageDigest

/** A file opened before, and where and how it was left. */
data class Recent(
    val uri: String,
    val name: String,
    val page: Int,
    val offset: Float,
    val zoom: Float,
    val pages: Int,
    val opened: Long,
    /** Quarter turns clockwise. */
    val rotation: Int = 0,
    /** 0 one page to a row, 1 two, 2 a book. */
    val spread: Int = 0,
)

/** A bookmark: a page and the name it was given. */
data class Bookmark(val page: Int, val name: String)

/** Recently opened files, newest first, bookmarks, and the app's settings, in shared preferences. */
class Recents(context: Context) {
    private val context = context.applicationContext
    private val prefs = context.getSharedPreferences("flores", Context.MODE_PRIVATE)

    fun list(): List<Recent> = prefs.getString(KEY, "").orEmpty().lineSequence().mapNotNull { line ->
        val f = line.split('\t')
        if (f.size < 7) return@mapNotNull null
        Recent(
            f[0], f[1], f[2].toIntOrNull() ?: 0, f[3].toFloatOrNull() ?: 0f, f[4].toFloatOrNull() ?: 1f,
            f[5].toIntOrNull() ?: 0, f[6].toLongOrNull() ?: 0,
            f.getOrNull(7)?.toIntOrNull() ?: 0, f.getOrNull(8)?.toIntOrNull() ?: 0,
        )
    }.toList()

    fun find(uri: String): Recent? = list().firstOrNull { it.uri == uri }

    /** Puts [r] first, replacing any entry for the same file. */
    fun put(r: Recent) {
        save(listOf(r) + list().filter { it.uri != r.uri })
    }

    fun remove(uri: String) {
        save(list().filter { it.uri != uri })
    }

    fun clear() {
        save(emptyList())
    }

    /** Writes the list, cut to [limit], and deletes what the files that left it had: private copies
     *  of shared files, thumbnails and bookmarks. */
    private fun save(list: List<Recent>) {
        val kept = list.take(limit)
        val clean = { s: String -> s.replace('\t', ' ').replace('\n', ' ') }
        val text = kept.joinToString("\n") {
            listOf(it.uri, clean(it.name), it.page, it.offset, it.zoom, it.pages, it.opened, it.rotation, it.spread).joinToString("\t")
        }
        val uris = kept.map { it.uri }.toSet()
        val edit = prefs.edit().putString(KEY, text)
        for (key in prefs.all.keys.filter { it.startsWith(BOOKMARKS) && it.removePrefix(BOOKMARKS) !in uris }) edit.remove(key)
        edit.apply()
        Received.prune(context, uris)
        val thumbs = uris.map { thumbnail(context, it).name }.toSet()
        thumbDir(context).listFiles()?.forEach { if (it.name !in thumbs) it.delete() }
    }

    /** How many files the list keeps, 10 to 50. */
    var limit: Int
        get() = prefs.getInt("recent_limit", 20).coerceIn(MIN_LIMIT, MAX_LIMIT)
        set(v) {
            prefs.edit().putInt("recent_limit", v.coerceIn(MIN_LIMIT, MAX_LIMIT)).apply()
            save(list())
        }

    /** Page colors: 0 normal, 1 dark, 2 sepia, 3 black. */
    var look: Int
        get() = prefs.getInt("look", 0)
        set(v) = prefs.edit().putInt("look", v).apply()

    /** The app's theme: 0 as the system, 1 light, 2 dark, 3 black. */
    var theme: Int
        get() = prefs.getInt("theme", 0)
        set(v) = prefs.edit().putInt("theme", v).apply()

    var keepScreenOn: Boolean
        get() = prefs.getBoolean("keep_screen_on", false)
        set(v) = prefs.edit().putBoolean("keep_screen_on", v).apply()

    var volumeKeys: Boolean
        get() = prefs.getBoolean("volume_keys", true)
        set(v) = prefs.edit().putBoolean("volume_keys", v).apply()

    fun bookmarks(uri: String): List<Bookmark> = prefs.getString(BOOKMARKS + uri, "").orEmpty().lineSequence().mapNotNull {
        val f = it.split('\t', limit = 2)
        if (f.size < 2) null else Bookmark(f[0].toIntOrNull() ?: return@mapNotNull null, f[1])
    }.toList()

    fun setBookmarks(uri: String, list: List<Bookmark>) {
        val text = list.sortedBy { it.page }.joinToString("\n") { "${it.page}\t${it.name.replace('\n', ' ')}" }
        prefs.edit().putString(BOOKMARKS + uri, text).apply()
    }

    companion object {
        private const val KEY = "recent"
        private const val BOOKMARKS = "bookmarks:"
        const val MIN_LIMIT = 10
        const val MAX_LIMIT = 50

        private fun thumbDir(c: Context) = File(c.cacheDir, "thumbs")

        /** Where the start page's picture of [uri]'s first page is kept. */
        fun thumbnail(c: Context, uri: String): File {
            val hash = MessageDigest.getInstance("SHA-1").digest(uri.toByteArray()).take(10).joinToString("") { "%02x".format(it) }
            return File(thumbDir(c).apply { mkdirs() }, "$hash.png")
        }
    }
}
