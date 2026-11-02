package io.github.agastyashenoy.flores

import android.app.Activity
import android.content.Intent
import android.graphics.BitmapFactory
import android.graphics.Outline
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.text.format.DateUtils
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.ViewOutlineProvider
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.window.OnBackInvokedDispatcher
import java.util.concurrent.Executors

/**
 * The app's one activity. Screens are views swapped in and out of it, as in Flax Reader: the start
 * page, settings, and the reader, which slides in over the start page.
 */
class MainActivity : Activity() {
    lateinit var recents: Recents
    lateinit var pal: Pal
    lateinit var root: FrameLayout
    private lateinit var screen: FrameLayout
    lateinit var panels: Panels
    private var reader: ReaderScreen? = null
    private var settingsOpen = false
    private var insets = IntArray(4)
    /** Decodes thumbnails off the UI thread. */
    private val loader = Executors.newSingleThreadExecutor()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        recents = Recents(this)
        pal = Pal.of(this)
        preferFastRefresh()
        root = FrameLayout(this).apply { setBackgroundColor(pal.bg) }
        screen = FrameLayout(this)
        root.addView(screen, frame())
        panels = Panels(root, pal)
        root.setOnApplyWindowInsetsListener { _, i ->
            insets = i.bars()
            reader?.insets(insets)
            if (reader == null) show(if (settingsOpen) settings() else start(), animate = false)
            i
        }
        setContentView(root)
        // after setContentView: the window's bars need its decor view
        edgeToEdge(pal)
        window.setBackgroundDrawable(android.graphics.drawable.ColorDrawable(pal.bg))
        if (Build.VERSION.SDK_INT >= 33) {
            onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT) { back() }
        }
        show(start(), animate = false)
        openIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        openIntent(intent)
    }

    private fun openIntent(i: Intent?) {
        if (i?.action == Intent.ACTION_VIEW) i.data?.let { openReader(it, null, shared = true) }
    }

    /** Puts [v] on screen in place of the last screen. */
    private fun show(v: View, animate: Boolean = true) {
        screen.removeAllViews()
        screen.addView(v, frame())
        if (animate) Anim.enter(v, dy = dpf(10f), ms = 170)
    }

    // The start page

    private fun start(): View {
        val p = pal
        val col = column().apply { setPadding(dp(16f) + insets[0], insets[1] + dp(8f), dp(16f) + insets[2], insets[3] + dp(24f)) }
        val head = row().apply { setPadding(dp(4f), dp(8f), 0, dp(8f)) }
        head.addView(ImageView(this).apply { setImageResource(R.mipmap.ic_launcher) }, LinearLayout.LayoutParams(dp(40f), dp(40f)))
        head.addView(label("Flores PDF", 24f, p.text, bold = true).apply { setPadding(dp(12f), 0, 0, 0) }, LinearLayout.LayoutParams(0, -2, 1f))
        head.addView(iconButton(p, R.drawable.ic_settings, "Settings") { openSettings() })
        col.addView(head)
        col.addView(button(p, "Open a file", Kind.Primary, R.drawable.ic_open) { pick() }, LinearLayout.LayoutParams(-2, -2).apply { topMargin = dp(16f) })
        val items = recents.list()
        if (items.isEmpty()) {
            col.addView(label("Open a PDF, EPUB, DjVu, a CBZ or CB7 comic, or a picture. Files you open show up here.", 15f, p.muted).apply {
                setLineSpacing(0f, 1.2f)
            }, lp(top = dp(20f)))
        } else {
            col.addView(section(p, "Recent"))
            val columns = (resources.configuration.screenWidthDp / 170).coerceAtLeast(2)
            items.chunked(columns).forEach { group ->
                val line = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
                for (k in 0 until columns) {
                    val r = group.getOrNull(k)
                    val cell = if (r != null) recentCard(r) else View(this)
                    line.addView(cell, LinearLayout.LayoutParams(0, -2, 1f).apply { setMargins(dp(5f), dp(5f), dp(5f), dp(5f)) })
                }
                col.addView(line)
            }
        }
        return ScrollView(this).apply {
            isVerticalScrollBarEnabled = false
            clipToPadding = false
            addView(col)
        }
    }

    private fun recentCard(r: Recent): View {
        val p = pal
        val card = card(p).apply {
            clipToOutline = true
            isClickable = true
            foreground = android.graphics.drawable.RippleDrawable(android.content.res.ColorStateList.valueOf(alpha(p.text, 0.12f)), null, rounded(-1, dpf(16f)))
        }
        val thumb = ImageView(this).apply {
            scaleType = ImageView.ScaleType.CENTER_CROP
            setBackgroundColor(alpha(p.text, 0.06f))
        }
        val columns = (resources.configuration.screenWidthDp / 170).coerceAtLeast(2)
        val width = (resources.displayMetrics.widthPixels - dp(32f) - insets[0] - insets[2]) / columns - dp(10f)
        card.addView(thumb, LinearLayout.LayoutParams(-1, (width * 1.3f).toInt()))
        card.addView(label(r.name, 14f, p.text, bold = true, lines = 2).apply { setPadding(dp(12f), dp(10f), dp(12f), 0) })
        val where = if (r.pages > 0) "Page ${r.page + 1} of ${r.pages}" else "Page ${r.page + 1}"
        val ago = DateUtils.getRelativeTimeSpanString(r.opened, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)
        card.addView(label("$where · $ago", 12f, p.muted, lines = 1).apply { setPadding(dp(12f), dp(2f), dp(12f), dp(12f)) })
        val file = Recents.thumbnail(this, r.uri)
        if (file.exists()) {
            loader.execute {
                val bitmap = BitmapFactory.decodeFile(file.path) ?: return@execute
                runOnUiThread { thumb.setImageBitmap(bitmap) }
            }
        }
        card.setOnClickListener { openReader(Uri.parse(r.uri), r.name, shared = false) }
        card.setOnLongClickListener {
            val body = column()
            body.addView(listRow(p, R.drawable.ic_delete, "Remove from recent files") {
                panels.close()
                recents.remove(r.uri)
                show(start(), animate = false)
            })
            panels.show(body, r.name)
            true
        }
        return card
    }

    @Suppress("DEPRECATION")
    private fun pick() {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "*/*"
            putExtra(Intent.EXTRA_MIME_TYPES, MIME_TYPES)
        }
        startActivityForResult(intent, PICK)
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        val uri = data?.data
        if (requestCode != PICK || resultCode != RESULT_OK || uri == null) return
        // so the recent list can open it again after a restart
        try {
            contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (_: SecurityException) {
        }
        openReader(uri, null, shared = false)
    }

    // Settings

    fun openSettings() {
        settingsOpen = true
        if (reader != null) closeReader()
        show(settings())
    }

    private fun settings(): View = SettingsScreen(this, insets).view

    /** After a change of theme: everything again, in the new colors. */
    fun restyle() = recreate()

    // The reader

    fun openReader(uri: Uri, name: String?, shared: Boolean) {
        reader?.let { closeReader(animate = false) }
        val r = ReaderScreen(this, uri, name, shared)
        reader = r
        root.addView(r.view, 1, frame())
        r.insets(insets)
        Anim.enter(r.view, dy = dpf(28f), ms = 220)
        r.open()
    }

    fun closeReader(animate: Boolean = true) {
        val r = reader ?: return
        reader = null
        panels.close(animate = false)
        r.close()
        if (!settingsOpen) show(start(), animate = false)
        if (animate) Anim.exit(r.view, dy = dpf(28f), ms = 160) { root.removeView(r.view) } else root.removeView(r.view)
    }

    // Back and keys

    private fun back() {
        when {
            panels.open -> panels.close()
            reader?.back() == true -> {}
            reader != null -> closeReader()
            settingsOpen -> {
                settingsOpen = false
                show(start())
            }
            else -> finish()
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onBackPressed() = back()

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean = reader?.key(keyCode) == true || super.onKeyDown(keyCode, event)

    override fun onPause() {
        super.onPause()
        reader?.remember()
    }

    override fun onDestroy() {
        super.onDestroy()
        reader?.close()
        loader.shutdownNow()
    }

    companion object {
        private const val PICK = 1
        private val MIME_TYPES = arrayOf(
            "application/pdf", "application/epub+zip", "image/vnd.djvu", "image/x-djvu",
            "application/vnd.comicbook+zip", "application/x-cbz", "application/x-cb7", "application/zip",
            "application/x-7z-compressed", "application/octet-stream",
            "image/jpeg", "image/png", "image/gif", "image/webp",
        )
    }
}
