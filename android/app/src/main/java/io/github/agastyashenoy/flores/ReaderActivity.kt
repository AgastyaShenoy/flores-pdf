package io.github.agastyashenoy.flores

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.content.res.ColorStateList
import android.content.res.Configuration
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.text.InputType
import android.text.TextUtils
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.PopupMenu
import android.widget.SeekBar
import android.widget.TextView
import android.widget.Toast
import android.window.OnBackInvokedDispatcher

/** One document: the pages, a bar of tools above them and a page slider below, shown by a tap. */
class ReaderActivity : Activity(), PageView.Listener {
    private lateinit var recents: Recents
    private lateinit var uri: Uri
    private lateinit var pages: PageView
    private lateinit var topBar: LinearLayout
    private lateinit var tools: LinearLayout
    private lateinit var find: LinearLayout
    private lateinit var query: EditText
    private lateinit var found: TextView
    private lateinit var bottomBar: LinearLayout
    private lateinit var pageLabel: TextView
    private lateinit var slider: SeekBar
    private lateinit var titleView: TextView
    private lateinit var note: TextView

    private var handle = 0L
    private var file: ParcelFileDescriptor? = null
    private var name = ""
    private var count = 0
    private var shownPage = -1
    private var barsShown = true
    private var searching = false
    private var lastStatus = intArrayOf()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        recents = Recents(this)
        uri = intent.data ?: run { finish(); return }
        name = displayName(uri)
        edgeToEdge()
        preferFastRefresh()
        build()
        if (Build.VERSION.SDK_INT >= 33) {
            onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT) { goBack() }
        }
        open(null)
    }

    // Layout

    private fun build() {
        val text = color(R.color.text)
        val dim = color(R.color.text_dim)
        val bar = (color(R.color.surface) and 0x00FFFFFF) or 0xF2000000.toInt()

        pages = PageView(this, this)
        titleView = label(17f, text, name).apply {
            maxLines = 1
            ellipsize = TextUtils.TruncateAt.END
        }
        tools = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            addView(iconButton(R.drawable.ic_back, "Back", text) { finish() })
            addView(titleView, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f).apply { marginStart = dp(4f) })
            addView(iconButton(R.drawable.ic_search, "Search", text) { startSearch() })
            addView(iconButton(R.drawable.ic_contents, "Contents", text) { contents() })
            addView(iconButton(R.drawable.ic_more, "More", text) { more(it) })
        }
        query = EditText(this).apply {
            hint = "Search"
            setSingleLine()
            imeOptions = EditorInfo.IME_ACTION_SEARCH
            background = null
            setTextColor(text)
            setHintTextColor(dim)
            setOnEditorActionListener { _, action, _ ->
                if (action == EditorInfo.IME_ACTION_SEARCH) {
                    search()
                    true
                } else false
            }
        }
        found = label(13f, dim)
        find = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            visibility = View.GONE
            addView(iconButton(R.drawable.ic_close, "Close search", text) { endSearch() })
            addView(query, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))
            addView(found)
            addView(iconButton(R.drawable.ic_up, "Previous match", text) { step(-1) })
            addView(iconButton(R.drawable.ic_down, "Next match", text) { step(1) })
        }
        topBar = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(bar)
            elevation = dp(2f).toFloat()
            addView(tools, LinearLayout.LayoutParams(MATCH_PARENT, dp(56f)))
            addView(find, LinearLayout.LayoutParams(MATCH_PARENT, dp(56f)))
        }

        pageLabel = label(14f, text).apply {
            minWidth = dp(72f)
            gravity = Gravity.CENTER
        }
        slider = SeekBar(this).apply {
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(s: SeekBar, value: Int, fromUser: Boolean) {
                    if (!fromUser || handle == 0L) return
                    pages.stopFling()
                    Native.goToPage(handle, value)
                    pages.redraw()
                }

                override fun onStartTrackingTouch(s: SeekBar) {}
                override fun onStopTrackingTouch(s: SeekBar) {}
            })
        }
        bottomBar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setBackgroundColor(bar)
            elevation = dp(2f).toFloat()
            setPadding(dp(8f), 0, dp(8f), 0)
            addView(pageLabel)
            addView(slider, LinearLayout.LayoutParams(0, dp(56f), 1f))
        }
        note = label(15f, text).apply {
            gravity = Gravity.CENTER
            setPadding(dp(32f), 0, dp(32f), 0)
        }

        val root = FrameLayout(this).apply {
            setBackgroundColor(color(R.color.background))
            addView(pages, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT))
            addView(note, FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT, Gravity.CENTER))
            addView(topBar, FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT, Gravity.TOP))
            addView(bottomBar, FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT, Gravity.BOTTOM))
            setOnApplyWindowInsetsListener { _, insets ->
                val b = insets.bars()
                // keep the bars' room while the system's are hidden, so they don't jump
                if (b[1] > 0) topBar.setPadding(b[0], b[1], b[2], 0)
                if (b[3] > 0 || b[0] > 0 || b[2] > 0) bottomBar.setPadding(b[0] + dp(8f), 0, b[2] + dp(8f), b[3])
                insets
            }
        }
        setContentView(root)
        bottomBar.visibility = View.GONE
    }

    private fun displayName(uri: Uri): String {
        try {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) it.getString(0)?.let { n -> return n }
            }
        } catch (_: Exception) {
        }
        return uri.lastPathSegment?.substringAfterLast('/') ?: "Document"
    }

    // Opening

    private fun open(password: String?) {
        note.text = "Opening…"
        note.visibility = View.VISIBLE
        val gap = dp(6f)
        Thread {
            val fd = try {
                contentResolver.openFileDescriptor(uri, "r")
            } catch (e: Exception) {
                android.util.Log.w("Flores", "can't open $uri", e)
                null
            }
            val h = if (fd == null) Native.UNREADABLE else Native.open(fd.fd, password.orEmpty(), gap)
            runOnUiThread {
                if (isDestroyed) {
                    if (Native.isHandle(h)) Native.close(h)
                    fd?.close()
                    return@runOnUiThread
                }
                when {
                    Native.isHandle(h) -> opened(h, fd!!)
                    h == Native.NEEDS_PASSWORD -> {
                        fd?.close()
                        askPassword(password != null)
                    }
                    else -> {
                        fd?.close()
                        failed(h)
                    }
                }
            }
        }.start()
    }

    private fun opened(h: Long, fd: ParcelFileDescriptor) {
        handle = h
        file = fd
        count = Native.pageCount(h)
        Native.title(h)?.let { titleView.text = it }
        slider.max = (count - 1).coerceAtLeast(0)
        bottomBar.visibility = if (count > 1) View.VISIBLE else View.GONE
        note.visibility = View.GONE
        applyLook()
        pages.attach(h)
    }

    private fun askPassword(wrong: Boolean) {
        note.text = "This document needs a password."
        val field = EditText(this).apply {
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            hint = "Password"
        }
        val box = FrameLayout(this).apply {
            setPadding(dp(24f), dp(8f), dp(24f), 0)
            addView(field)
        }
        AlertDialog.Builder(this)
            .setTitle(if (wrong) "That password didn't work" else "Password")
            .setMessage("$name is protected by a password.")
            .setView(box)
            .setPositiveButton("Open") { _, _ -> open(field.text.toString()) }
            .setNegativeButton("Cancel") { _, _ -> finish() }
            .setOnCancelListener { finish() }
            .show()
        field.requestFocus()
    }

    private fun failed(code: Long) {
        note.text = when (code) {
            Native.UNSUPPORTED -> "Flores PDF can't read this kind of file."
            Native.ENCRYPTED -> "This document is encrypted in a way Flores PDF can't open."
            Native.DAMAGED -> "This file is damaged."
            else -> "Flores PDF can't get at this file. It may have been moved or deleted."
        }
        note.visibility = View.VISIBLE
        bottomBar.visibility = View.GONE
    }

    // PageView.Listener

    override fun onReady() {
        val bottom = if (count > 1) bottomBar.height else 0
        Native.setInsets(handle, topBar.height.toFloat(), bottom.toFloat())
        val saved = recents.find(uri.toString())
        if (saved != null) Native.restore(handle, saved.page, saved.offset, saved.zoom)
        remember()
    }

    override fun onFrame() {
        val h = handle
        if (h == 0L) return
        val page = Native.currentPage(h)
        if (page != shownPage) {
            shownPage = page
            pageLabel.text = "${page + 1} / $count"
            if (!slider.isPressed) slider.progress = page
        }
        if (searching) showStatus()
    }

    override fun onTap() {
        showBars(!barsShown)
    }

    override fun onLink(uri: String) {
        AlertDialog.Builder(this)
            .setTitle("Open link?")
            .setMessage(uri)
            .setPositiveButton("Open") { _, _ ->
                try {
                    startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(uri)))
                } catch (_: Exception) {
                    Toast.makeText(this, "No app opens this link.", Toast.LENGTH_SHORT).show()
                }
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    // Bars

    @Suppress("DEPRECATION")
    private fun showBars(show: Boolean) {
        barsShown = show
        for (v in listOf(topBar, bottomBar)) {
            if (v === bottomBar && count <= 1) continue
            v.animate().cancel()
            if (show) {
                v.visibility = View.VISIBLE
                v.animate().alpha(1f).setDuration(150).start()
            } else {
                v.animate().alpha(0f).setDuration(150).withEndAction { v.visibility = View.GONE }.start()
            }
        }
        if (Build.VERSION.SDK_INT >= 30) {
            val c = window.insetsController ?: return
            c.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            if (show) c.show(WindowInsets.Type.systemBars()) else c.hide(WindowInsets.Type.systemBars())
        } else {
            val base = View.SYSTEM_UI_FLAG_LAYOUT_STABLE or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
            window.decorView.systemUiVisibility = if (show) base else base or View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
        }
    }

    // Tools

    private fun contents() {
        if (handle == 0L) return
        val items = Native.outline(handle).lineSequence().mapNotNull { line ->
            val f = line.split('\t', limit = 4)
            if (f.size < 4) null else Entry(f[0].toIntOrNull() ?: 0, f[1].toIntOrNull() ?: -1, f[2].toFloatOrNull() ?: -1f, f[3])
        }.toList()
        if (items.isEmpty()) {
            Toast.makeText(this, "This document has no contents.", Toast.LENGTH_SHORT).show()
            return
        }
        // indented with em spaces
        val labels = items.map { " ".repeat(it.depth.coerceAtMost(6)) + it.title }.toTypedArray()
        AlertDialog.Builder(this)
            .setTitle("Contents")
            .setItems(labels) { _, i ->
                val e = items[i]
                if (e.page >= 0) {
                    pages.stopFling()
                    Native.goTo(handle, e.page, e.y)
                    pages.redraw()
                }
            }
            .show()
    }

    /** An outline entry: where it leads, in pages and points down the page (-1 for its top). */
    private class Entry(val depth: Int, val page: Int, val y: Float, val title: String)

    private fun more(anchor: View) {
        if (handle == 0L) return
        val menu = PopupMenu(this, anchor)
        menu.menu.add(0, 1, 0, "Go to page…")
        menu.menu.add(0, 2, 0, "Page colors…")
        menu.setOnMenuItemClickListener {
            when (it.itemId) {
                1 -> goToPage()
                2 -> chooseLook()
            }
            true
        }
        menu.show()
    }

    private fun goToPage() {
        val field = EditText(this).apply {
            inputType = InputType.TYPE_CLASS_NUMBER
            hint = "1 to $count"
        }
        val box = FrameLayout(this).apply {
            setPadding(dp(24f), dp(8f), dp(24f), 0)
            addView(field)
        }
        val go = {
            field.text.toString().toIntOrNull()?.let {
                pages.stopFling()
                Native.goToPage(handle, (it - 1).coerceIn(0, count - 1))
                pages.redraw()
            }
        }
        val dialog = AlertDialog.Builder(this)
            .setTitle("Go to page")
            .setView(box)
            .setPositiveButton("Go") { _, _ -> go() }
            .setNegativeButton("Cancel", null)
            .create()
        field.setOnEditorActionListener { _, _, _ ->
            go()
            dialog.dismiss()
            true
        }
        dialog.window?.setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE)
        dialog.show()
        field.requestFocus()
    }

    private fun chooseLook() {
        val names = arrayOf("Normal", "Dark", "Sepia", "Black (for OLED screens)")
        AlertDialog.Builder(this)
            .setTitle("Page colors")
            .setSingleChoiceItems(names, recents.look) { d, i ->
                recents.look = i
                applyLook()
                d.dismiss()
            }
            .show()
    }

    private fun applyLook() {
        if (handle == 0L) return
        val night = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
        val background = when (recents.look) {
            1 -> 0x121212
            2 -> 0xD9CDB0
            3 -> 0x000000
            else -> if (night) 0x1E2121 else 0xE3E6E6
        }
        Native.setLook(handle, recents.look, background)
        pages.redraw()
    }

    // Search

    private fun startSearch() {
        if (handle == 0L) return
        searching = true
        tools.visibility = View.GONE
        find.visibility = View.VISIBLE
        found.text = ""
        query.requestFocus()
        getSystemService(InputMethodManager::class.java).showSoftInput(query, 0)
    }

    private fun search() {
        val q = query.text.toString().trim()
        if (q.isEmpty()) return
        getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(query.windowToken, 0)
        Native.search(handle, q)
        lastStatus = intArrayOf()
        pendingJump = true
        pages.redraw()
    }

    /** Jump to the first match once there is one. */
    private var pendingJump = false

    private fun showStatus() {
        val s = Native.searchStatus(handle)
        if (s.contentEquals(lastStatus)) return
        lastStatus = s
        val (n, searched, total, current) = s.toList()
        if (pendingJump && n > 0) {
            pendingJump = false
            Native.searchStep(handle, 1)
            pages.redraw()
            return
        }
        found.text = when {
            query.text.isBlank() -> ""
            n == 0 && searched < total -> "…"
            n == 0 -> "None"
            current >= 0 -> "${current + 1} of $n${if (searched < total) "+" else ""}"
            else -> "$n"
        }
        found.setPadding(dp(8f), 0, dp(4f), 0)
    }

    private fun step(dir: Int) {
        if (handle == 0L) return
        if (lastStatus.isEmpty()) search()
        if (Native.searchStep(handle, dir)) pages.redraw()
    }

    private fun endSearch() {
        searching = false
        pendingJump = false
        getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(query.windowToken, 0)
        find.visibility = View.GONE
        tools.visibility = View.VISIBLE
        query.setText("")
        if (handle != 0L) {
            Native.search(handle, "")
            pages.redraw()
        }
    }

    // Back, keys, saving

    private fun goBack() {
        when {
            searching -> endSearch()
            handle != 0L && Native.back(handle) -> pages.redraw()
            else -> finish()
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        goBack()
    }

    /** Volume keys turn pages, as in other readers. */
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        val h = handle
        if (h != 0L && (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN || keyCode == KeyEvent.KEYCODE_VOLUME_UP)) {
            val screen = pages.height * 0.9f
            pages.stopFling()
            Native.scrollBy(h, 0f, if (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN) screen else -screen)
            pages.redraw()
            return true
        }
        return super.onKeyDown(keyCode, event)
    }

    private fun remember() {
        val h = handle
        if (h == 0L) return
        val p = Native.position(h)
        recents.put(Recent(uri.toString(), name, p[0].toInt(), p[1], p[2], count, System.currentTimeMillis()))
    }

    override fun onPause() {
        super.onPause()
        remember()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        pages.redraw()
    }

    override fun onDestroy() {
        super.onDestroy()
        pages.detach()
        if (handle != 0L) Native.close(handle)
        handle = 0L
        file?.close()
        file = null
    }
}
