package io.github.agastyashenoy.flores

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import androidx.activity.addCallback
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.core.view.updatePadding
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.bottomsheet.BottomSheetBehavior
import com.google.android.material.bottomsheet.BottomSheetDialog
import com.google.android.material.button.MaterialButton
import com.google.android.material.button.MaterialButtonToggleGroup
import com.google.android.material.card.MaterialCardView
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.slider.Slider

/** One document: the pages, a bar of tools above them and a page slider below, shown by a tap. */
class ReaderActivity : FloresActivity(), PageView.Listener {
    private lateinit var recents: Recents
    private lateinit var uri: Uri
    /** A copy of a file shared for one use has been made, or wasn't needed. */
    private var copied = false
    /** The file's app no longer lets Flores read it. */
    private var denied = false

    private lateinit var root: FrameLayout
    private lateinit var pages: PageView
    private lateinit var top: LinearLayout
    private lateinit var toolbar: MaterialToolbar
    private lateinit var find: LinearLayout
    private lateinit var query: EditText
    private lateinit var found: TextView
    private lateinit var bottom: LinearLayout
    private lateinit var pageLabel: TextView
    private lateinit var slider: Slider
    private lateinit var note: TextView
    private lateinit var handleStart: ImageView
    private lateinit var handleEnd: ImageView
    private lateinit var selectionBar: MaterialCardView

    private var handle = 0L
    private var file: ParcelFileDescriptor? = null
    private var name = ""
    private var count = 0
    private var shownPage = -1
    private var barsShown = true
    private var searching = false
    private var lastStatus = intArrayOf()
    /** Jump to the first match once there is one. */
    private var pendingJump = false
    private var rotation = 0
    private var spread = 0
    /** Text is selected; the handle being dragged, 0 the start or 1 the end. */
    private var selecting = false
    private var dragging = 1

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        recents = Recents(this)
        uri = intent.data ?: run { finish(); return }
        // the recent list knows the name even when the file's app no longer answers
        name = intent.getStringExtra(NAME) ?: displayName(uri)
        preferFastRefresh()
        if (recents.keepScreenOn) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        build()
        onBackPressedDispatcher.addCallback(this) { goBack() }
        open(null)
    }

    // Layout

    private fun build() {
        setContentView(R.layout.activity_reader)
        root = findViewById(R.id.root)
        top = findViewById(R.id.top)
        toolbar = findViewById(R.id.toolbar)
        find = findViewById(R.id.find)
        query = findViewById(R.id.query)
        found = findViewById(R.id.found)
        bottom = findViewById(R.id.bottom)
        pageLabel = findViewById(R.id.page_label)
        slider = findViewById(R.id.slider)
        note = findViewById(R.id.note)
        handleStart = findViewById(R.id.handle_start)
        handleEnd = findViewById(R.id.handle_end)
        selectionBar = findViewById(R.id.selection_bar)

        pages = PageView(this, this)
        // an empty surface is black: shown once there's a document to draw
        pages.visibility = View.INVISIBLE
        findViewById<FrameLayout>(R.id.pages).addView(pages)

        // the bars sit over the pages, nearly opaque
        val bar = root.themeColor(com.google.android.material.R.attr.colorSurfaceContainer) and 0x00FFFFFF or (0xF2 shl 24)
        top.setBackgroundColor(bar)
        bottom.setBackgroundColor(bar)
        bottom.visibility = View.GONE

        toolbar.title = name
        toolbar.setNavigationOnClickListener { finish() }
        toolbar.setOnMenuItemClickListener {
            when (it.itemId) {
                R.id.search -> startSearch()
                R.id.contents -> contents()
                R.id.more -> more()
            }
            true
        }
        findViewById<View>(R.id.find_close).setOnClickListener { endSearch() }
        findViewById<View>(R.id.find_up).setOnClickListener { step(-1) }
        findViewById<View>(R.id.find_down).setOnClickListener { step(1) }
        query.setOnEditorActionListener { _, action, _ ->
            if (action == EditorInfo.IME_ACTION_SEARCH) {
                search()
                true
            } else false
        }

        slider.setLabelFormatter { "Page ${it.toInt()}" }
        slider.addOnChangeListener { _, value, fromUser ->
            if (!fromUser || handle == 0L) return@addOnChangeListener
            pages.stopFling()
            Native.goToPage(handle, value.toInt() - 1)
            pages.redraw()
        }

        ViewCompat.setOnApplyWindowInsetsListener(root) { _, insets ->
            val b = insets.bars()
            // keep the bars' room while the system's are hidden, so they don't jump
            if (b.top > 0) top.updatePadding(left = b.left, top = b.top, right = b.right)
            if (b.bottom > 0 || b.left > 0 || b.right > 0) bottom.updatePadding(left = b.left + dp(16f), right = b.right + dp(16f), bottom = b.bottom + dp(4f))
            insets
        }

        handleStart.setOnTouchListener { _, e -> dragHandle(0, e) }
        handleEnd.setOnTouchListener { _, e -> dragHandle(1, e) }
        findViewById<View>(R.id.sel_copy).setOnClickListener { copySelection() }
        findViewById<View>(R.id.sel_share).setOnClickListener { shareSelection() }
        findViewById<View>(R.id.sel_search).setOnClickListener { findSelection() }
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
        val shared = intent.action == Intent.ACTION_VIEW && !copied
        copied = true
        Thread {
            // an app that lets Flores read the file just this once: open a copy, which the recent
            // list can open again
            if (shared && !Received.lasting(this, uri)) {
                Received.copy(this, uri, name)?.let { uri = it }
            }
            val fd = try {
                contentResolver.openFileDescriptor(uri, "r")
            } catch (e: SecurityException) {
                denied = true
                null
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
        Native.title(h)?.let { toolbar.title = it }
        toolbar.subtitle = if (Native.title(h) != null) name else null
        if (count > 1) {
            slider.valueTo = count.toFloat()
            bottom.visibility = View.VISIBLE
        }
        note.visibility = View.GONE
        applyLook()
        pages.visibility = View.VISIBLE
        pages.attach(h)
    }

    private fun askPassword(wrong: Boolean) {
        note.text = "This document needs a password."
        Sheets.ask(
            this,
            title = if (wrong) "That password didn't work" else "Password",
            message = "$name is protected by a password.",
            hint = "Password",
            ok = "Open",
            password = true,
            onCancel = { finish() },
        ) { open(it) }
    }

    private fun failed(code: Long) {
        if (denied) {
            // it can't be opened from the list again, so it leaves it
            recents.remove(uri.toString())
        }
        note.text = when {
            denied -> "The app this file came from let Flores PDF open it only once. Open it from that app again."
            code == Native.UNSUPPORTED -> "Flores PDF can't read this kind of file."
            code == Native.ENCRYPTED -> "This document is encrypted in a way Flores PDF can't open."
            code == Native.DAMAGED -> "This file is damaged."
            else -> "Flores PDF can't get at this file. It may have been moved or deleted."
        }
        note.visibility = View.VISIBLE
        bottom.visibility = View.GONE
    }

    // PageView.Listener

    override fun onReady() {
        val h = handle
        Native.setInsets(h, top.height.toFloat(), if (count > 1) bottom.height.toFloat() else 0f)
        recents.find(uri.toString())?.let {
            rotation = it.rotation
            spread = it.spread
            Native.setView(h, rotation, spread)
            Native.restore(h, it.page, it.offset, it.zoom)
        }
        remember()
        val thumb = Recents.thumbnail(this, uri.toString())
        if (!thumb.exists()) Native.saveThumbnail(h, thumb.path, 360)
    }

    override fun onFrame() {
        val h = handle
        if (h == 0L) return
        val page = Native.currentPage(h)
        if (page != shownPage) {
            shownPage = page
            pageLabel.text = "${page + 1} / $count"
            if (count > 1) slider.value = (page + 1).toFloat().coerceIn(1f, count.toFloat())
        }
        if (searching) showStatus()
        if (selecting) placeSelection(selectionBar.visibility == View.VISIBLE)
    }

    override fun onTap() {
        if (selecting) clearSelection() else showBars(!barsShown)
    }

    override fun onLink(uri: String) {
        MaterialAlertDialogBuilder(this)
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

    // Selecting text

    override fun onLongPress(x: Float, y: Float): Boolean {
        val h = handle
        if (h == 0L) return false
        if (Native.selectWord(h, x, y)) {
            startSelection()
            return true
        }
        // the page's text may still be on its way: try again in a moment
        pages.postDelayed({ if (handle == h && Native.selectWord(h, x, y)) startSelection(showBar = true) }, 200)
        return false
    }

    override fun onSelectDrag(x: Float, y: Float) {
        dragging = Native.selectTo(handle, dragging, x, y)
        pages.redraw()
        placeSelection(false)
    }

    override fun onSelectEnd() {
        placeSelection(true)
    }

    private fun startSelection(showBar: Boolean = false) {
        selecting = true
        dragging = 1
        pages.redraw()
        placeSelection(showBar)
    }

    /** Moves the handles to the selection's ends, and the bar of actions above it if [showBar]. */
    private fun placeSelection(showBar: Boolean) {
        val p = Native.selectionHandles(handle) ?: return
        handleStart.x = p[0] - handleStart.width
        handleStart.y = p[1]
        handleEnd.x = p[2]
        handleEnd.y = p[3]
        handleStart.visibility = View.VISIBLE
        handleEnd.visibility = View.VISIBLE
        if (!showBar) {
            selectionBar.visibility = View.GONE
            return
        }
        selectionBar.visibility = View.VISIBLE
        selectionBar.post {
            val w = selectionBar.width
            val h = selectionBar.height
            val margin = dp(8f)
            selectionBar.x = ((p[0] + p[2]) / 2 - w / 2).coerceIn(margin.toFloat(), (root.width - w - margin).toFloat().coerceAtLeast(0f))
            // above the selection's first line, or under its end when there's no room
            val above = p[1] - dp(40f) - h
            val ceiling = if (barsShown) top.height else 0
            selectionBar.y = if (above > ceiling) above else p[3] + handleEnd.height + margin
        }
    }

    private fun dragHandle(which: Int, e: MotionEvent): Boolean {
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                dragging = which
                selectionBar.visibility = View.GONE
            }
            MotionEvent.ACTION_MOVE -> {
                val at = IntArray(2)
                root.getLocationOnScreen(at)
                // the text line is just above the handle the finger holds
                val x = e.rawX - at[0]
                val y = e.rawY - at[1] - handleEnd.height * 1.2f
                dragging = Native.selectTo(handle, dragging, x, y)
                pages.redraw()
                placeSelection(false)
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> placeSelection(true)
        }
        return true
    }

    private fun clearSelection() {
        selecting = false
        if (handle != 0L) Native.clearSelection(handle)
        handleStart.visibility = View.GONE
        handleEnd.visibility = View.GONE
        selectionBar.visibility = View.GONE
        pages.redraw()
    }

    private fun copySelection() {
        val text = Native.selectionText(handle) ?: return
        getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText(name, text))
        // Android 13 and later show their own note of what was copied
        if (Build.VERSION.SDK_INT < 33) Toast.makeText(this, "Copied", Toast.LENGTH_SHORT).show()
        clearSelection()
    }

    private fun shareSelection() {
        val text = Native.selectionText(handle) ?: return
        startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text), null))
        clearSelection()
    }

    private fun findSelection() {
        val text = Native.selectionText(handle) ?: return
        clearSelection()
        startSearch()
        query.setText(text)
        search()
    }

    // Bars

    private fun showBars(show: Boolean) {
        barsShown = show
        for (v in listOf(top, bottom)) {
            if (v === bottom && count <= 1) continue
            v.animate().cancel()
            if (show) {
                v.visibility = View.VISIBLE
                v.animate().alpha(1f).setDuration(150).start()
            } else {
                v.animate().alpha(0f).setDuration(150).withEndAction { v.visibility = View.GONE }.start()
            }
        }
        val c = WindowCompat.getInsetsController(window, window.decorView)
        c.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        if (show) c.show(WindowInsetsCompat.Type.systemBars()) else c.hide(WindowInsetsCompat.Type.systemBars())
    }

    // Sheets

    /** Contents and bookmarks, in one sheet. */
    private fun contents() {
        val h = handle
        if (h == 0L) return
        val sheet = BottomSheetDialog(this)
        val items = mutableListOf<Item>()
        val marks = recents.bookmarks(uri.toString())
        items += Item.Heading("Bookmarks")
        for (b in marks) {
            items += Item.Row(R.drawable.ic_bookmark, b.name, "Page ${b.page + 1}", 0, action = {
                recents.setBookmarks(uri.toString(), recents.bookmarks(uri.toString()) - b)
                sheet.dismiss()
                contents()
            }) {
                sheet.dismiss()
                go(b.page, -1f)
            }
        }
        items += Item.Row(R.drawable.ic_bookmark_add, "Add a bookmark here", null, 0) {
            sheet.dismiss()
            addBookmark()
        }
        val outline = Native.outline(h).lineSequence().mapNotNull { line ->
            val f = line.split('\t', limit = 4)
            if (f.size < 4) null else Triple(f[0].toIntOrNull() ?: 0, f[1].toIntOrNull() ?: -1, f[2].toFloatOrNull() ?: -1f) to f[3]
        }.toList()
        if (outline.isNotEmpty()) {
            items += Item.Heading("Contents")
            for ((where, title) in outline) {
                val (depth, page, y) = where
                items += Item.Row(0, title, if (page >= 0) "${page + 1}" else null, depth) {
                    if (page >= 0) {
                        sheet.dismiss()
                        go(page, y)
                    }
                }
            }
        }
        val list = RecyclerView(this).apply {
            layoutManager = LinearLayoutManager(context)
            adapter = ItemAdapter(items)
            setPadding(0, 0, 0, dp(16f))
            clipToPadding = false
        }
        sheet.setContentView(list)
        sheet.behavior.state = BottomSheetBehavior.STATE_EXPANDED
        sheet.behavior.skipCollapsed = true
        sheet.show()
    }

    private fun go(page: Int, y: Float) {
        pages.stopFling()
        Native.goTo(handle, page, y)
        pages.redraw()
    }

    private fun addBookmark() {
        val page = Native.currentPage(handle)
        Sheets.ask(this, title = "Add a bookmark", hint = "Name", ok = "Add", initial = "Page ${page + 1}") { text ->
            val label = text.trim().ifEmpty { "Page ${page + 1}" }
            val key = uri.toString()
            recents.setBookmarks(key, recents.bookmarks(key).filter { it.page != page } + Bookmark(page, label))
            Toast.makeText(this, "Bookmarked page ${page + 1}", Toast.LENGTH_SHORT).show()
        }
    }

    /** Page colors, layout, rotation, and the rest. */
    private fun more() {
        if (handle == 0L) return
        val sheet = BottomSheetDialog(this)
        val column = Sheets.column(this)
        column.addView(Sheets.section(this, "Page colors"))
        column.addView(LookPicker(this).apply {
            selected = recents.look
            onPick = {
                recents.look = it
                applyLook()
            }
            setPadding(dp(24f), 0, dp(24f), dp(8f))
        })
        column.addView(Sheets.section(this, "Layout"))
        val group = MaterialButtonToggleGroup(this).apply {
            isSingleSelection = true
            isSelectionRequired = true
            setPadding(dp(24f), 0, dp(24f), dp(8f))
        }
        val ids = listOf("One page", "Two pages", "Book").map { label ->
            val b = MaterialButton(this, null, com.google.android.material.R.attr.materialButtonOutlinedStyle).apply {
                text = label
                id = View.generateViewId()
            }
            group.addView(b, LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f))
            b.id
        }
        group.check(ids[spread])
        group.addOnButtonCheckedListener { _, id, checked ->
            if (!checked) return@addOnButtonCheckedListener
            spread = ids.indexOf(id)
            applyView()
        }
        column.addView(group)
        lateinit var rotate: View
        rotate = Sheets.row(this, R.drawable.ic_rotate, "Rotate pages", "${rotation * 90}°") {
            rotation = (rotation + 1) % 4
            applyView()
            rotate.findViewById<TextView>(R.id.subtitle).text = "${rotation * 90}°"
        }
        column.addView(rotate)
        column.addView(Sheets.row(this, R.drawable.ic_goto, "Go to page") {
            sheet.dismiss()
            goToPage()
        })
        column.addView(Sheets.row(this, R.drawable.ic_bookmark_add, "Add a bookmark") {
            sheet.dismiss()
            addBookmark()
        })
        column.addView(Sheets.row(this, R.drawable.ic_info, "Document properties") {
            sheet.dismiss()
            properties()
        })
        column.addView(Sheets.row(this, R.drawable.ic_settings, "Settings") {
            sheet.dismiss()
            startActivity(Intent(this, SettingsActivity::class.java))
        })
        sheet.setContentView(Sheets.scrolling(column))
        sheet.show()
    }

    private fun applyView() {
        clearSelection()
        Native.setView(handle, rotation, spread)
        pages.redraw()
        remember()
    }

    private fun goToPage() {
        Sheets.ask(this, title = "Go to page", hint = "Page, 1 to $count", ok = "Go", number = true) { text ->
            text.toIntOrNull()?.let { go((it - 1).coerceIn(0, count - 1), -1f) }
        }
    }

    private fun properties() {
        val sheet = BottomSheetDialog(this)
        val column = Sheets.column(this, "Document properties")
        column.addView(Sheets.row(this, 0, "File", name) {})
        for (line in Native.info(handle).lines()) {
            val f = line.split('\t', limit = 2)
            if (f.size == 2) column.addView(Sheets.row(this, 0, f[0], f[1]) {})
        }
        sheet.setContentView(Sheets.scrolling(column))
        sheet.show()
    }

    private fun applyLook() {
        if (handle == 0L) return
        Native.setLook(handle, recents.look, LookPicker.around(root, recents.look, isDark))
        pages.redraw()
    }

    // Search

    private fun startSearch() {
        if (handle == 0L) return
        searching = true
        toolbar.visibility = View.GONE
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
        toolbar.visibility = View.VISIBLE
        query.setText("")
        if (handle != 0L) {
            Native.search(handle, "")
            pages.redraw()
        }
    }

    // Back, keys, saving

    private fun goBack() {
        when {
            selecting -> clearSelection()
            searching -> endSearch()
            handle != 0L && Native.back(handle) -> pages.redraw()
            else -> finish()
        }
    }

    /** Volume keys turn pages, as in other readers, unless turned off in the settings. */
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        val h = handle
        if (h != 0L && recents.volumeKeys && (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN || keyCode == KeyEvent.KEYCODE_VOLUME_UP)) {
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
        recents.put(Recent(uri.toString(), name, p[0].toInt(), p[1], p[2], count, System.currentTimeMillis(), rotation, spread))
    }

    override fun onPause() {
        super.onPause()
        remember()
    }

    override fun onResume() {
        super.onResume()
        // the page colors may have changed in the settings
        applyLook()
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

    /** A line in the contents sheet. */
    private sealed class Item {
        class Heading(val title: String) : Item()
        class Row(val icon: Int, val title: String, val subtitle: String?, val depth: Int, val action: (() -> Unit)? = null, val onClick: () -> Unit) : Item()
    }

    private inner class ItemAdapter(private val items: List<Item>) : RecyclerView.Adapter<RecyclerView.ViewHolder>() {
        override fun getItemCount() = items.size
        override fun getItemViewType(position: Int) = if (items[position] is Item.Heading) 0 else 1

        override fun onCreateViewHolder(parent: android.view.ViewGroup, type: Int): RecyclerView.ViewHolder {
            val view = if (type == 0) Sheets.section(this@ReaderActivity, "") else Sheets.row(this@ReaderActivity, 0, "") {}
            view.layoutParams = RecyclerView.LayoutParams(RecyclerView.LayoutParams.MATCH_PARENT, RecyclerView.LayoutParams.WRAP_CONTENT)
            return object : RecyclerView.ViewHolder(view) {}
        }

        override fun onBindViewHolder(h: RecyclerView.ViewHolder, position: Int) {
            when (val item = items[position]) {
                is Item.Heading -> (h.itemView as TextView).text = item.title
                is Item.Row -> {
                    val v = h.itemView
                    val icon = v.findViewById<ImageView>(R.id.icon)
                    if (item.icon != 0) {
                        icon.setImageResource(item.icon)
                        icon.visibility = View.VISIBLE
                    } else icon.visibility = View.GONE
                    v.findViewById<TextView>(R.id.title).text = item.title
                    v.findViewById<TextView>(R.id.subtitle).apply {
                        text = item.subtitle
                        visibility = if (item.subtitle != null) View.VISIBLE else View.GONE
                    }
                    // contents entries step in by their depth
                    v.updatePadding(left = dp(24f) + dp(16f) * item.depth.coerceAtMost(6))
                    val action = v.findViewById<android.widget.ImageButton>(R.id.action)
                    if (item.action != null) {
                        Sheets.action(v, R.drawable.ic_delete, "Remove bookmark") { item.action.invoke() }
                    } else action.visibility = View.GONE
                    v.setOnClickListener { item.onClick() }
                }
            }
        }
    }

    companion object {
        /** The intent extra with the file's name, from the recent list. */
        const val NAME = "name"
    }
}
