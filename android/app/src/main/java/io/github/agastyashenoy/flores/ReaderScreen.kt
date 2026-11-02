package io.github.agastyashenoy.flores

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.view.Gravity
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.WindowInsets
import android.view.WindowManager
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.TextView
import android.widget.Toast

/** One document: the pages, a bar of tools above them and a page slider below, shown by a tap. */
class ReaderScreen(private val a: MainActivity, private var uri: Uri, givenName: String?, private val shared: Boolean) : PageView.Listener {
    private val p = a.pal
    private val recents = a.recents
    /** The file's app no longer lets Flores read it. */
    private var denied = false
    private val name = givenName ?: displayName(uri)

    val view = FrameLayout(a)
    private val pages = PageView(a, this)
    private val note = a.label("", 16f, p.text).apply {
        gravity = Gravity.CENTER
        setPadding(a.dp(32f), 0, a.dp(32f), 0)
    }
    private val top = a.column().apply { setBackgroundColor(p.bar) }
    private val tools = a.row().apply { setPadding(a.dp(4f), 0, a.dp(4f), 0) }
    private val titleView = a.label(name, 18f, p.text, lines = 1)
    private val find = a.row().apply {
        visibility = View.GONE
        setPadding(a.dp(4f), 0, a.dp(4f), 0)
    }
    private val query = EditText(a)
    private val found = a.label("", 13.5f, p.muted)
    private val bottom = a.row().apply {
        setBackgroundColor(p.bar)
        visibility = View.GONE
    }
    private val pageLabel = a.label("", 14f, p.text, bold = true).apply { minWidth = a.dp(64f) }
    private lateinit var slider: SeekBar
    private val handleStart = ImageView(a)
    private val handleEnd = ImageView(a)
    private val selectionBar = a.row()

    private var handle = 0L
    private var file: ParcelFileDescriptor? = null
    private var count = 0
    private var shownPage = -1
    private var barsShown = true
    private var searching = false
    private var lastStatus = intArrayOf()
    private var pendingJump = false
    private var rotation = 0
    private var spread = 0
    private var selecting = false
    private var dragging = 1

    // Annotating: a bar of tools in place of the page slider; one finger draws, two move the pages.
    private var annotating = false
    private var tool = HIGHLIGHT
    private var color = COLORS[0]
    private var lineWidth = WIDTHS[1]
    /** A markup tool's drag has text selected, and where it started. */
    private var marking = false
    private var markFrom = FloatArray(2)
    private var boxFrom: FloatArray? = null
    private val annotBar = a.column().apply {
        setBackgroundColor(p.bar)
        visibility = View.GONE
    }
    private val boxPreview = View(a).apply { visibility = View.GONE }

    init {
        view.setBackgroundColor(Looks.around(p, recents.look) or (0xFF shl 24))
        // an empty surface is black: shown once there's a document to draw
        pages.visibility = View.INVISIBLE
        view.addView(pages, frame())
        view.addView(note, frame(h = -2, gravity = Gravity.CENTER))

        tools.addView(a.iconButton(p, R.drawable.ic_back, "Back") { a.requestCloseReader() })
        tools.addView(titleView, LinearLayout.LayoutParams(0, -2, 1f).apply { marginStart = a.dp(4f) })
        tools.addView(a.iconButton(p, R.drawable.ic_search, "Search") { startSearch() })
        tools.addView(a.iconButton(p, R.drawable.ic_contents, "Contents and bookmarks") { contents() })
        tools.addView(a.iconButton(p, R.drawable.ic_more, "More") { more() })
        top.addView(tools, lp(h = a.dp(60f)))

        query.apply {
            hint = "Search in document"
            setHintTextColor(p.muted)
            setTextColor(p.text)
            textSize = 16f
            isSingleLine = true
            imeOptions = EditorInfo.IME_ACTION_SEARCH
            background = null
            setOnEditorActionListener { _, action, _ ->
                if (action == EditorInfo.IME_ACTION_SEARCH) {
                    search()
                    true
                } else false
            }
        }
        find.addView(a.iconButton(p, R.drawable.ic_close, "Close search") { endSearch() })
        find.addView(query, LinearLayout.LayoutParams(0, -2, 1f))
        find.addView(found)
        find.addView(a.iconButton(p, R.drawable.ic_up, "Previous match") { step(-1) })
        find.addView(a.iconButton(p, R.drawable.ic_down, "Next match") { step(1) })
        top.addView(find, lp(h = a.dp(60f)))
        view.addView(top, frame(h = -2, gravity = Gravity.TOP))

        slider = a.slider(p, 1, 0) { v, user ->
            if (!user || handle == 0L) return@slider
            pages.stopFling()
            Native.goToPage(handle, v)
            pages.redraw()
        }.apply { contentDescription = "Page" }
        bottom.addView(pageLabel)
        bottom.addView(slider, LinearLayout.LayoutParams(0, a.dp(48f), 1f))
        view.addView(bottom, frame(h = -2, gravity = Gravity.BOTTOM))

        for ((h, res) in listOf(handleStart to R.drawable.handle_start, handleEnd to R.drawable.handle_end)) {
            h.setImageResource(res)
            h.imageTintList = android.content.res.ColorStateList.valueOf(p.accent)
            h.visibility = View.GONE
            view.addView(h, FrameLayout.LayoutParams(a.dp(22f), a.dp(22f)))
        }
        handleStart.setOnTouchListener { _, e -> dragHandle(0, e) }
        handleEnd.setOnTouchListener { _, e -> dragHandle(1, e) }
        selectionBar.apply {
            background = rounded(p.card, a.dpf(24f), p.border, if (p.dark) a.dp(1f) else 0)
            elevation = a.dpf(6f)
            setPadding(a.dp(4f), a.dp(4f), a.dp(4f), a.dp(4f))
            visibility = View.GONE
            addView(a.button(p, "Copy", Kind.Ghost) { copySelection() })
            addView(a.button(p, "Share", Kind.Ghost) { shareSelection() })
            addView(a.button(p, "Find", Kind.Ghost) { findSelection() })
            addView(a.button(p, "Highlight", Kind.Ghost) { highlightSelection() })
        }
        view.addView(selectionBar, FrameLayout.LayoutParams(-2, -2))
        view.addView(boxPreview, FrameLayout.LayoutParams(0, 0))
        view.addView(annotBar, frame(h = -2, gravity = Gravity.BOTTOM))
    }

    /** Pads the bars clear of the system's. */
    fun insets(b: IntArray) {
        // keep the bars' room while the system's are hidden, so they don't jump
        if (b[1] > 0) top.setPadding(b[0], b[1], b[2], 0)
        if (b[3] > 0 || b[0] > 0 || b[2] > 0) {
            bottom.setPadding(b[0] + a.dp(16f), a.dp(4f), b[2] + a.dp(16f), b[3] + a.dp(4f))
            annotBar.setPadding(b[0] + a.dp(8f), a.dp(4f), b[2] + a.dp(8f), b[3] + a.dp(4f))
        }
    }

    private fun displayName(uri: Uri): String {
        try {
            a.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) it.getString(0)?.let { n -> return n }
            }
        } catch (_: Exception) {
        }
        return uri.lastPathSegment?.substringAfterLast('/') ?: "Document"
    }

    // Opening and closing

    fun open(password: String? = null) {
        if (recents.keepScreenOn) a.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        note.text = "Opening…"
        note.visibility = View.VISIBLE
        val gap = a.dp(6f)
        val copy = shared && password == null
        Thread {
            // an app that lets Flores read the file just this once: open a copy, which the recent
            // list can open again
            if (copy && !Received.lasting(a, uri)) {
                Received.copy(a, uri, name)?.let { uri = it }
            }
            val fd = try {
                a.contentResolver.openFileDescriptor(uri, "r")
            } catch (e: SecurityException) {
                denied = true
                null
            } catch (e: Exception) {
                null
            }
            val h = if (fd == null) Native.UNREADABLE else Native.open(fd.fd, password.orEmpty(), gap)
            a.runOnUiThread {
                if (a.isDestroyed || !view.isAttachedToWindow) {
                    if (Native.isHandle(h)) Native.close(h)
                    fd?.close()
                    return@runOnUiThread
                }
                when {
                    Native.isHandle(h) -> opened(h, fd!!)
                    h == Native.NEEDS_PASSWORD -> {
                        fd?.close()
                        note.text = "This document needs a password."
                        a.ask(p, if (password != null) "That password didn't work" else "Password", "$name is protected by a password.",
                            hint = "Password", password = true, ok = "Open", onCancel = { a.closeReader() }) { open(it) }
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
        if (count > 1) {
            slider.max = count - 1
            bottom.visibility = View.VISIBLE
        }
        note.visibility = View.GONE
        applyLook()
        pages.visibility = View.VISIBLE
        pages.attach(h)
    }

    private fun failed(code: Long) {
        if (denied) recents.remove(uri.toString())
        note.text = when {
            denied -> "The app this file came from let Flores PDF open it only once. Open it from that app again."
            code == Native.UNSUPPORTED -> "Flores PDF can't read this kind of file."
            code == Native.ENCRYPTED -> "This document is encrypted in a way Flores PDF can't open."
            code == Native.DAMAGED -> "This file is damaged."
            else -> "Flores PDF can't get at this file. It may have been moved or deleted."
        }
        note.visibility = View.VISIBLE
    }

    fun close() {
        remember()
        a.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        showSystemBars(true)
        pages.detach()
        if (handle != 0L) Native.close(handle)
        handle = 0L
        file?.close()
        file = null
    }

    fun remember() {
        val h = handle
        if (h == 0L) return
        val pos = Native.position(h)
        recents.put(Recent(uri.toString(), name, pos[0].toInt(), pos[1], pos[2], count, System.currentTimeMillis(), rotation, spread))
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
        val thumb = Recents.thumbnail(a, uri.toString())
        if (!thumb.exists()) Native.saveThumbnail(h, thumb.path, 360)
    }

    override fun onFrame() {
        val h = handle
        if (h == 0L) return
        val page = Native.currentPage(h)
        if (page != shownPage) {
            shownPage = page
            pageLabel.text = "${page + 1} / $count"
            slider.progress = page
        }
        if (searching) showStatus()
        if (selecting) placeSelection(selectionBar.visibility == View.VISIBLE)
    }

    override fun onTap() {
        if (selecting) clearSelection() else showBars(!barsShown)
    }

    override fun onLink(uri: String) {
        a.ask(p, "Open link?", uri, ok = "Open") {
            try {
                a.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(uri)))
            } catch (_: Exception) {
                Toast.makeText(a, "No app opens this link.", Toast.LENGTH_SHORT).show()
            }
        }
    }

    // Selecting text

    override fun onLongPress(x: Float, y: Float): Boolean {
        val h = handle
        if (h == 0L) return false
        if (Native.selectWord(h, x, y)) {
            startSelection(false)
            return true
        }
        // the page's text may still be on its way: try again in a moment
        pages.postDelayed({ if (handle == h && Native.selectWord(h, x, y)) startSelection(true) }, 200)
        return false
    }

    override fun onSelectDrag(x: Float, y: Float) {
        dragging = Native.selectTo(handle, dragging, x, y)
        pages.redraw()
        placeSelection(false)
    }

    override fun onSelectEnd() = placeSelection(true)

    // Annotating

    override fun onDraw(action: Int, x: Float, y: Float) {
        val h = handle
        if (h == 0L) return
        when (tool) {
            HIGHLIGHT, UNDERLINE, STRIKEOUT -> when (action) {
                // the drag selects text from where it starts, and letting go marks it
                0 -> {
                    markFrom = floatArrayOf(x, y)
                    marking = Native.selectWord(h, x, y)
                }
                1 -> if (marking) {
                    Native.selectTo(h, 1, x, y)
                } else if (Native.selectWord(h, x, y)) {
                    // the drag began off the text: the marking begins where it did
                    marking = true
                    Native.selectTo(h, 0, markFrom[0], markFrom[1])
                }
                2 -> {
                    if (marking) Native.markSelection(h, tool, color)
                    marking = false
                }
            }
            PEN -> when (action) {
                0 -> Native.penDown(h, x, y, color, lineWidth)
                1 -> Native.penMove(h, x, y)
                2 -> Native.penUp(h)
            }
            BOX -> when (action) {
                0 -> boxFrom = floatArrayOf(x, y)
                1 -> boxFrom?.let { showBox(it[0], it[1], x, y) }
                2 -> {
                    boxFrom?.let { Native.addRect(h, it[0], it[1], x, y, color, lineWidth) }
                    boxFrom = null
                    boxPreview.visibility = View.GONE
                }
            }
            NOTE -> if (action == 2) {
                a.ask(p, "Add a note", hint = "Note", ok = "Add") { text ->
                    if (text.isNotBlank() && Native.addNote(h, x, y, text.trim(), color)) changed()
                }
            }
            ERASER -> if (action == 2) {
                when (Native.eraseAt(h, x, y)) {
                    -1 -> Toast.makeText(a, "Flores PDF couldn't remove that annotation.", Toast.LENGTH_SHORT).show()
                    0 -> {}
                    else -> changed()
                }
            }
        }
        pages.redraw()
        if (action == 2) changed()
    }

    override fun onDrawCancel() {
        val h = handle
        if (h == 0L) return
        Native.penCancel(h)
        if (marking) Native.clearSelection(h)
        marking = false
        boxFrom = null
        boxPreview.visibility = View.GONE
        pages.redraw()
    }

    /** The rectangle being dragged out, before it's made. */
    private fun showBox(x0: Float, y0: Float, x1: Float, y1: Float) {
        val stroke = (lineWidth * 2).coerceAtLeast(2f).toInt()
        boxPreview.background = rounded(0, 0f, color or (0xFF shl 24), stroke)
        boxPreview.x = minOf(x0, x1)
        boxPreview.y = minOf(y0, y1)
        boxPreview.layoutParams = boxPreview.layoutParams.apply {
            this.width = kotlin.math.abs(x1 - x0).toInt()
            height = kotlin.math.abs(y1 - y0).toInt()
        }
        boxPreview.visibility = View.VISIBLE
    }

    private fun startAnnotating() {
        if (handle == 0L) return
        clearSelection()
        if (searching) endSearch()
        annotating = true
        pages.drawing = true
        if (!barsShown) showBars(true)
        bottom.visibility = View.GONE
        buildAnnotBar()
        annotBar.visibility = View.VISIBLE
        Anim.enter(annotBar, dy = a.dpf(12f), ms = 180)
        annotBar.post { Native.setInsets(handle, top.height.toFloat(), annotBar.height.toFloat()) }
    }

    private fun stopAnnotating() {
        annotating = false
        pages.drawing = false
        onDrawCancel()
        annotBar.visibility = View.GONE
        if (count > 1) bottom.visibility = View.VISIBLE
        Native.setInsets(handle, top.height.toFloat(), if (count > 1) bottom.height.toFloat() else 0f)
        pages.redraw()
    }

    /** After a change: the bar's Undo and Save follow it. */
    private fun changed() {
        if (annotating) buildAnnotBar()
    }

    /** Tools, then colors and widths, undo, save and done. */
    private fun buildAnnotBar() {
        annotBar.removeAllViews()
        val tools = a.row().apply { gravity = Gravity.CENTER }
        for ((i, pair) in TOOLS.withIndex()) {
            val (icon, label) = pair
            val b = a.iconButton(p, icon, label) {
                tool = i
                onDrawCancel()
                buildAnnotBar()
            }
            if (i == tool) b.background = rounded(p.selected, a.dpf(24f))
            tools.addView(b)
        }
        annotBar.addView(android.widget.HorizontalScrollView(a).apply {
            isHorizontalScrollBarEnabled = false
            addView(tools)
        }, lp().apply { gravity = Gravity.CENTER_HORIZONTAL })
        val row = a.row().apply { setPadding(a.dp(8f), 0, a.dp(4f), 0) }
        for (c in COLORS) {
            row.addView(View(a).apply {
                val on = c == color
                background = android.graphics.drawable.LayerDrawable(arrayOf(
                    android.graphics.drawable.GradientDrawable().apply {
                        shape = android.graphics.drawable.GradientDrawable.OVAL
                        setColor(c or (0xFF shl 24))
                        setStroke(a.dp(if (on) 3f else 1f), if (on) p.text else p.border)
                    },
                ))
                contentDescription = "Color"
                setOnClickListener {
                    color = c
                    buildAnnotBar()
                }
            }, LinearLayout.LayoutParams(a.dp(28f), a.dp(28f)).apply { setMargins(a.dp(4f), a.dp(8f), a.dp(4f), a.dp(8f)) })
        }
        if (tool == PEN || tool == BOX) {
            row.addView(View(a), LinearLayout.LayoutParams(a.dp(8f), 1))
            for (w in WIDTHS) {
                val dot = a.dp(4f + w * 2.4f)
                val box = FrameLayout(a).apply {
                    if (w == lineWidth) background = rounded(p.selected, a.dpf(18f))
                    addView(View(a).apply {
                        background = android.graphics.drawable.GradientDrawable().apply {
                            shape = android.graphics.drawable.GradientDrawable.OVAL
                            setColor(p.text)
                        }
                    }, FrameLayout.LayoutParams(dot, dot, Gravity.CENTER))
                    contentDescription = "Line width"
                    setOnClickListener {
                        lineWidth = w
                        buildAnnotBar()
                    }
                }
                row.addView(box, LinearLayout.LayoutParams(a.dp(36f), a.dp(36f)))
            }
        }
        row.addView(View(a), LinearLayout.LayoutParams(0, 1, 1f))
        row.addView(a.iconButton(p, R.drawable.ic_undo, "Undo") {
            if (Native.undo(handle)) {
                pages.redraw()
                changed()
            }
        }.apply { alpha = if (Native.changed(handle)) 1f else 0.4f })
        row.addView(a.iconButton(p, R.drawable.ic_save, "Save") { save() }.apply { alpha = if (Native.changed(handle)) 1f else 0.4f })
        row.addView(a.iconButton(p, R.drawable.ic_check, "Done") { stopAnnotating() })
        annotBar.addView(row)
    }

    private fun highlightSelection() {
        if (Native.markSelection(handle, HIGHLIGHT, COLORS[0])) {
            clearSelection()
            Toast.makeText(a, "Highlighted. Save it from the ⋮ menu.", Toast.LENGTH_SHORT).show()
        }
    }

    // Saving

    /** Writes the changes into the file, or, when Android won't let Flores write it, into a copy
     *  that then opens in its place. [after] runs once they're saved into the file. */
    fun save(after: (() -> Unit)? = null) {
        val h = handle
        if (h == 0L || !Native.changed(h)) {
            after?.invoke()
            return
        }
        Native.savePrepare(h)?.let {
            Toast.makeText(a, "Flores PDF couldn't save: $it", Toast.LENGTH_LONG).show()
            return
        }
        val out = try {
            a.contentResolver.openFileDescriptor(uri, "rwt")
        } catch (_: Exception) {
            null
        }
        if (out == null) {
            saveCopy(prepared = true)
            return
        }
        val error = out.use { Native.saveWrite(h, it.fd, true) }
        if (error != null) {
            Toast.makeText(a, "Flores PDF couldn't save: $error", Toast.LENGTH_LONG).show()
            return
        }
        Toast.makeText(a, "Saved", Toast.LENGTH_SHORT).show()
        // the start page's picture shows the saved first page
        Native.saveThumbnail(h, Recents.thumbnail(a, uri.toString()).path, 360)
        changed()
        pages.redraw()
        after?.invoke()
    }

    /** Writes the changes into a new file the user picks, then opens that. */
    fun saveCopy(prepared: Boolean = false) {
        val h = handle
        if (h == 0L) return
        if (!prepared) Native.savePrepare(h)?.let {
            Toast.makeText(a, "Flores PDF couldn't save: $it", Toast.LENGTH_LONG).show()
            return
        }
        val suggested = name.substringBeforeLast('.') + " (annotated).pdf"
        a.createDocument(suggested) { target ->
            val error = try {
                a.contentResolver.openFileDescriptor(target, "rwt")?.use { Native.saveWrite(handle, it.fd, false) } ?: "the new file can't be written"
            } catch (e: Exception) {
                e.message ?: "the new file can't be written"
            }
            if (error != null) {
                Toast.makeText(a, "Flores PDF couldn't save: $error", Toast.LENGTH_LONG).show()
            } else {
                Toast.makeText(a, "Saved as a copy", Toast.LENGTH_SHORT).show()
                a.openReader(target, null, shared = false, discard = true)
            }
        }
    }

    /** Runs [then] once the reader can close: changes saved or let go, after asking. */
    fun confirmClose(then: () -> Unit) {
        val h = handle
        if (h == 0L || !Native.changed(h)) return then()
        a.ask(p, "Save your annotations?", "$name has changes that aren't saved yet.", ok = "Save", other = "Discard", onOther = then) {
            save(then)
        }
    }

    private fun startSelection(showBar: Boolean) {
        selecting = true
        dragging = 1
        pages.redraw()
        placeSelection(showBar)
    }

    private fun placeSelection(showBar: Boolean) {
        val q = Native.selectionHandles(handle) ?: return
        handleStart.x = q[0] - a.dp(22f)
        handleStart.y = q[1]
        handleEnd.x = q[2]
        handleEnd.y = q[3]
        handleStart.visibility = View.VISIBLE
        handleEnd.visibility = View.VISIBLE
        if (!showBar) {
            selectionBar.visibility = View.GONE
            return
        }
        if (selectionBar.visibility != View.VISIBLE) {
            selectionBar.visibility = View.VISIBLE
            Anim.enter(selectionBar, dy = a.dpf(6f), ms = 140)
        }
        selectionBar.post {
            val w = selectionBar.width
            val h = selectionBar.height
            val margin = a.dp(8f)
            selectionBar.x = ((q[0] + q[2]) / 2 - w / 2).coerceIn(margin.toFloat(), (view.width - w - margin).toFloat().coerceAtLeast(0f))
            val above = q[1] - a.dp(40f) - h
            val ceiling = if (barsShown) top.height else 0
            selectionBar.y = if (above > ceiling) above else q[3] + a.dp(22f) + margin
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
                view.getLocationOnScreen(at)
                // the text line is just above the handle the finger holds
                dragging = Native.selectTo(handle, dragging, e.rawX - at[0], e.rawY - at[1] - a.dp(22f) * 1.2f)
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
        a.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText(name, text))
        // Android 13 and later show their own note of what was copied
        if (Build.VERSION.SDK_INT < 33) Toast.makeText(a, "Copied", Toast.LENGTH_SHORT).show()
        clearSelection()
    }

    private fun shareSelection() {
        val text = Native.selectionText(handle) ?: return
        a.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text), null))
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
            if (show) {
                v.visibility = View.VISIBLE
                Anim.enter(v, dy = if (v === top) -a.dpf(8f) else a.dpf(8f), ms = 160)
            } else {
                Anim.exit(v, dy = if (v === top) -a.dpf(8f) else a.dpf(8f), ms = 120) {
                    v.visibility = View.GONE
                    v.translationY = 0f
                    v.alpha = 1f
                }
            }
        }
        showSystemBars(show)
    }

    @Suppress("DEPRECATION")
    private fun showSystemBars(show: Boolean) {
        if (Build.VERSION.SDK_INT >= 30) {
            val c = a.window.insetsController ?: return
            c.systemBarsBehavior = android.view.WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            if (show) c.show(WindowInsets.Type.systemBars()) else c.hide(WindowInsets.Type.systemBars())
        } else {
            a.edgeToEdge(p)
            if (!show) a.window.decorView.systemUiVisibility = a.window.decorView.systemUiVisibility or
                View.SYSTEM_UI_FLAG_FULLSCREEN or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
        }
    }

    // Panels

    /** Bookmarks and contents, in one panel. */
    private fun contents() {
        val h = handle
        if (h == 0L) return
        val body = a.column()
        body.addView(a.section(p, "Bookmarks").apply { setPadding(a.dp(24f), a.dp(8f), a.dp(24f), a.dp(4f)) })
        val key = uri.toString()
        for (b in recents.bookmarks(key)) {
            val remove = a.iconButton(p, R.drawable.ic_delete, "Remove bookmark") {
                recents.setBookmarks(key, recents.bookmarks(key) - b)
                contents()
            }
            body.addView(a.listRow(p, R.drawable.ic_bookmark, b.name, "Page ${b.page + 1}", remove) {
                a.panels.close()
                go(b.page, -1f)
            })
        }
        body.addView(a.listRow(p, R.drawable.ic_bookmark_add, "Add a bookmark here") {
            a.panels.close()
            addBookmark()
        })
        val outline = Native.outline(h).lineSequence().mapNotNull { line ->
            val f = line.split('\t', limit = 4)
            if (f.size < 4) null else listOf(f[0], f[1], f[2], f[3])
        }.toList()
        if (outline.isNotEmpty()) {
            body.addView(a.section(p, "Contents").apply { setPadding(a.dp(24f), a.dp(16f), a.dp(24f), a.dp(4f)) })
            // a very long outline is cut, so the panel opens at once
            for ((depth, page, y, title) in outline.take(400)) {
                val n = page.toIntOrNull() ?: -1
                val row = a.listRow(p, 0, title, if (n >= 0) "${n + 1}" else null) {
                    if (n >= 0) {
                        a.panels.close()
                        go(n, y.toFloatOrNull() ?: -1f)
                    }
                }
                row.setPadding(a.dp(24f) + a.dp(16f) * (depth.toIntOrNull() ?: 0).coerceAtMost(6), row.paddingTop, row.paddingRight, row.paddingBottom)
                body.addView(row)
            }
        }
        a.panels.show(body)
    }

    private fun go(page: Int, y: Float) {
        pages.stopFling()
        Native.goTo(handle, page, y)
        pages.redraw()
    }

    private fun addBookmark() {
        val page = Native.currentPage(handle)
        a.ask(p, "Add a bookmark", hint = "Name", initial = "Page ${page + 1}", ok = "Add") { text ->
            val label = text.trim().ifEmpty { "Page ${page + 1}" }
            val key = uri.toString()
            recents.setBookmarks(key, recents.bookmarks(key).filter { it.page != page } + Bookmark(page, label))
            Toast.makeText(a, "Bookmarked page ${page + 1}", Toast.LENGTH_SHORT).show()
        }
    }

    /** Page colors, layout, rotation, and the rest. */
    private fun more() {
        if (handle == 0L) return
        val body = a.column()
        val pad = { v: View -> v.apply { setPadding(a.dp(24f), paddingTop, a.dp(24f), paddingBottom) } }
        body.addView(pad(a.section(p, "Page colors")))
        body.addView(pad(Looks.picker(a, p, recents.look) {
            recents.look = it
            applyLook()
        }))
        body.addView(pad(a.section(p, "Layout").apply { setPadding(0, a.dp(20f), 0, a.dp(8f)) }))
        body.addView(a.segmented(p, listOf("One page", "Two pages", "Book"), spread) {
            spread = it
            applyView()
        }, lp().apply {
            marginStart = a.dp(24f)
            marginEnd = a.dp(24f)
            bottomMargin = a.dp(8f)
        })
        lateinit var rotate: LinearLayout
        rotate = a.listRow(p, R.drawable.ic_rotate, "Rotate pages", "${rotation * 90}°") {
            rotation = (rotation + 1) % 4
            applyView()
            rotate.findViewWithTag<TextView>("sub")?.text = "${rotation * 90}°"
        }
        body.addView(rotate)
        if (Native.annotatable(handle)) {
            body.addView(a.listRow(p, R.drawable.ic_pen, "Annotate", "Highlight, draw, add notes") {
                a.panels.close()
                startAnnotating()
            })
            if (Native.changed(handle)) {
                body.addView(a.listRow(p, R.drawable.ic_save, "Save annotations") {
                    a.panels.close()
                    save()
                })
            }
            body.addView(a.listRow(p, R.drawable.ic_save, "Save a copy") {
                a.panels.close()
                saveCopy()
            })
        }
        body.addView(a.listRow(p, R.drawable.ic_goto, "Go to page") {
            a.panels.close()
            a.ask(p, "Go to page", hint = "Page, 1 to $count", number = true, ok = "Go") { text ->
                text.toIntOrNull()?.let { go((it - 1).coerceIn(0, count - 1), -1f) }
            }
        })
        body.addView(a.listRow(p, R.drawable.ic_bookmark_add, "Add a bookmark") {
            a.panels.close()
            addBookmark()
        })
        body.addView(a.listRow(p, R.drawable.ic_info, "Document properties") {
            a.panels.close()
            properties()
        })
        body.addView(a.listRow(p, R.drawable.ic_settings, "Settings") {
            a.panels.close()
            a.openSettings()
        })
        a.panels.show(body)
    }

    private fun applyView() {
        clearSelection()
        Native.setView(handle, rotation, spread)
        pages.redraw()
        remember()
    }

    private fun properties() {
        val body = a.column()
        body.addView(a.listRow(p, 0, "File", name))
        for (line in Native.info(handle).lines()) {
            val f = line.split('\t', limit = 2)
            if (f.size == 2) body.addView(a.listRow(p, 0, f[0], f[1]))
        }
        a.panels.show(body, "Document properties")
    }

    private fun applyLook() {
        val around = Looks.around(p, recents.look)
        view.setBackgroundColor(around or (0xFF shl 24))
        if (handle == 0L) return
        Native.setLook(handle, recents.look, around)
        pages.redraw()
    }

    // Search

    private fun startSearch() {
        if (handle == 0L) return
        searching = true
        tools.visibility = View.GONE
        find.visibility = View.VISIBLE
        Anim.enter(find, ms = 140)
        found.text = ""
        query.requestFocus()
        a.getSystemService(InputMethodManager::class.java).showSoftInput(query, 0)
    }

    private fun search() {
        val q = query.text.toString().trim()
        if (q.isEmpty()) return
        a.getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(query.windowToken, 0)
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
        a.getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(query.windowToken, 0)
        find.visibility = View.GONE
        tools.visibility = View.VISIBLE
        query.setText("")
        if (handle != 0L) {
            Native.search(handle, "")
            pages.redraw()
        }
    }

    /** Back: ends a selection or a search, or goes back from a link; false when the reader closes. */
    fun back(): Boolean = when {
        selecting -> {
            clearSelection()
            true
        }
        annotating -> {
            stopAnnotating()
            true
        }
        searching -> {
            endSearch()
            true
        }
        handle != 0L && Native.back(handle) -> {
            pages.redraw()
            true
        }
        else -> false
    }

    /** Volume keys turn pages, unless turned off in the settings. */
    fun key(keyCode: Int): Boolean {
        val h = handle
        if (h == 0L || !recents.volumeKeys || (keyCode != KeyEvent.KEYCODE_VOLUME_DOWN && keyCode != KeyEvent.KEYCODE_VOLUME_UP)) return false
        val screen = pages.height * 0.9f
        pages.stopFling()
        Native.scrollBy(h, 0f, if (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN) screen else -screen)
        pages.redraw()
        return true
    }

    companion object {
        const val HIGHLIGHT = 0
        const val UNDERLINE = 1
        const val STRIKEOUT = 2
        const val PEN = 3
        const val BOX = 4
        const val NOTE = 5
        const val ERASER = 6
        private val TOOLS = listOf(
            R.drawable.ic_highlight to "Highlight text",
            R.drawable.ic_underline to "Underline text",
            R.drawable.ic_strike to "Strike out text",
            R.drawable.ic_pen to "Draw",
            R.drawable.ic_box to "Rectangle",
            R.drawable.ic_note to "Add a note",
            R.drawable.ic_eraser to "Erase annotations",
        )
        /** The Windows app's: four highlight colors, red and black. */
        private val COLORS = intArrayOf(0xFFE14D, 0x7CDB6E, 0x6EC6FF, 0xFF8AC8, 0xE53935, 0x1B1B1B)
        /** Line widths, in points. */
        private val WIDTHS = floatArrayOf(1f, 2.5f, 5f)
    }
}
