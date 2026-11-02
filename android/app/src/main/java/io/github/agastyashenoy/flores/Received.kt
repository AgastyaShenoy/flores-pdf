package io.github.agastyashenoy.flores

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File

/**
 * Files other apps share for one use only, as WhatsApp and mail apps do: the permission to read them
 * ends with the reader, so Flores keeps a private copy that the recent list can open again. A copy
 * is deleted once its file drops off the list.
 */
object Received {
    private fun dir(c: Context) = File(c.filesDir, "received")

    /** Whether Flores can open [uri] again later: a file of its own, or access it can keep. */
    fun lasting(c: Context, uri: Uri): Boolean {
        if (uri.scheme != "content") return true
        val r = c.contentResolver
        if (r.persistedUriPermissions.any { it.uri == uri && it.isReadPermission }) return true
        return try {
            r.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            true
        } catch (_: SecurityException) {
            false
        }
    }

    /** A copy of [uri] called [name], kept as received/size/name; the same file shared again
     *  reuses its copy. Null if it can't be read. */
    fun copy(c: Context, uri: Uri, name: String): Uri? {
        val r = c.contentResolver
        val safe = name.replace('/', '_').ifBlank { "document" }
        val size = try {
            r.query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)?.use {
                if (it.moveToFirst() && !it.isNull(0)) it.getLong(0) else -1L
            } ?: -1L
        } catch (_: Exception) {
            -1L
        }
        if (size >= 0) {
            val kept = File(dir(c), "$size/$safe")
            if (kept.isFile && kept.length() == size) return Uri.fromFile(kept)
        }
        val part = File(dir(c), "incoming.part")
        part.parentFile?.mkdirs()
        try {
            val input = r.openInputStream(uri) ?: return null
            input.use { i -> part.outputStream().use { i.copyTo(it) } }
        } catch (_: Exception) {
            part.delete()
            return null
        }
        val kept = File(dir(c), "${part.length()}/$safe")
        kept.parentFile?.mkdirs()
        kept.delete()
        if (!part.renameTo(kept)) {
            part.delete()
            return null
        }
        return Uri.fromFile(kept)
    }

    /** Deletes the copies none of [keep] (recent files' addresses) refers to; not ones made in the
     *  last minute, which a reader may be opening before it lists them. */
    fun prune(c: Context, keep: Set<String>) {
        val recent = System.currentTimeMillis() - 60_000
        for (sized in dir(c).listFiles().orEmpty()) {
            if (!sized.isDirectory) continue
            for (f in sized.listFiles().orEmpty()) {
                if (Uri.fromFile(f).toString() !in keep && f.lastModified() < recent) f.delete()
            }
            if (sized.list().isNullOrEmpty()) sized.delete()
        }
    }
}
