package io.github.agastyashenoy.flores

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.graphics.Typeface
import android.net.Uri
import android.os.Bundle
import android.text.TextUtils
import android.text.format.DateUtils
import android.view.Gravity
import android.view.View
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.widget.Button
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView

/** The start page: open a file, or pick up one opened before. */
class MainActivity : Activity() {
    private lateinit var recents: Recents
    private lateinit var list: LinearLayout

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        recents = Recents(this)
        edgeToEdge()

        val text = color(R.color.text)
        val dim = color(R.color.text_dim)
        val accent = color(R.color.accent)
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20f), dp(20f), dp(20f), dp(20f))
        }
        val head = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            addView(ImageView(context).apply {
                setImageResource(R.mipmap.ic_launcher)
            }, LinearLayout.LayoutParams(dp(48f), dp(48f)))
            addView(label(26f, text, getString(R.string.app_name)).apply {
                typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
                setPadding(dp(12f), 0, 0, 0)
            })
        }
        column.addView(head)
        val open = Button(this).apply {
            this.text = "Open a file"
            isAllCaps = false
            textSize = 16f
            setTextColor(0xFFFFFFFF.toInt())
            background = rounded(accent, dp(24f).toFloat())
            setCompoundDrawablesRelativeWithIntrinsicBounds(R.drawable.ic_open, 0, 0, 0)
            compoundDrawableTintList = android.content.res.ColorStateList.valueOf(0xFFFFFFFF.toInt())
            compoundDrawablePadding = dp(10f)
            setPadding(dp(22f), 0, dp(26f), 0)
            stateListAnimator = null
            setOnClickListener { pick() }
        }
        column.addView(open, LinearLayout.LayoutParams(WRAP_CONTENT, dp(48f)).apply { topMargin = dp(28f) })
        column.addView(label(14f, dim, "PDF, EPUB, DjVu, CBZ and CB7 comics, and pictures").apply {
            setPadding(dp(4f), dp(10f), 0, 0)
        })
        list = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        column.addView(list, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply { topMargin = dp(28f) })

        val scroll = ScrollView(this).apply {
            clipToPadding = false
            addView(column)
            setOnApplyWindowInsetsListener { v, insets ->
                val b = insets.bars()
                v.setPadding(b[0], b[1], b[2], b[3])
                insets
            }
        }
        setContentView(scroll)
    }

    override fun onResume() {
        super.onResume()
        fill()
    }

    private fun fill() {
        list.removeAllViews()
        val items = recents.list()
        if (items.isEmpty()) return
        list.addView(label(13f, color(R.color.text_dim), "Recent").apply {
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            setPadding(dp(4f), 0, 0, dp(8f))
        })
        val card = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = rounded(color(R.color.surface), dp(16f).toFloat())
            clipToOutline = true
        }
        items.forEachIndexed { i, r ->
            if (i > 0) card.addView(View(this).apply { setBackgroundColor(color(R.color.divider)) }, LinearLayout.LayoutParams(MATCH_PARENT, 1).apply { marginStart = dp(56f) })
            card.addView(row(r))
        }
        list.addView(card)
    }

    private fun row(r: Recent): View {
        val name = label(16f, color(R.color.text), r.name).apply {
            maxLines = 1
            ellipsize = TextUtils.TruncateAt.MIDDLE
        }
        val where = if (r.pages > 0) "Page ${r.page + 1} of ${r.pages}" else "Page ${r.page + 1}"
        val ago = DateUtils.getRelativeTimeSpanString(r.opened, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)
        val detail = label(13f, color(R.color.text_dim), "$where · $ago")
        val words = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(name)
            addView(detail)
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(16f), dp(12f), dp(16f), dp(12f))
            isClickable = true
            background = with(android.util.TypedValue()) {
                theme.resolveAttribute(android.R.attr.selectableItemBackground, this, true)
                getDrawable(resourceId)
            }
            addView(ImageView(context).apply {
                setImageResource(R.drawable.ic_doc)
                imageTintList = android.content.res.ColorStateList.valueOf(color(R.color.accent))
            }, LinearLayout.LayoutParams(dp(24f), dp(24f)).apply { marginEnd = dp(16f) })
            addView(words, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))
            setOnClickListener { read(Uri.parse(r.uri)) }
            setOnLongClickListener {
                AlertDialog.Builder(context)
                    .setTitle(r.name)
                    .setItems(arrayOf("Remove from recent")) { _, _ ->
                        recents.remove(r.uri)
                        releaseAccess(r.uri)
                        fill()
                    }
                    .show()
                true
            }
        }
    }

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
        read(uri)
    }

    private fun read(uri: Uri) {
        startActivity(Intent(this, ReaderActivity::class.java).setData(uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
    }

    private fun releaseAccess(uri: String) {
        try {
            contentResolver.releasePersistableUriPermission(Uri.parse(uri), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (_: SecurityException) {
        }
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
