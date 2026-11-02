package io.github.agastyashenoy.flores

import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Bundle
import android.text.format.DateUtils
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ImageView
import android.widget.TextView
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.view.ViewCompat
import androidx.core.view.updateLayoutParams
import androidx.core.view.updatePadding
import androidx.recyclerview.widget.GridLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.bottomsheet.BottomSheetDialog
import com.google.android.material.floatingactionbutton.ExtendedFloatingActionButton
import java.util.concurrent.Executors

/** The start page: open a file, or pick up one opened before. */
class MainActivity : FloresActivity() {
    private lateinit var recents: Recents
    private lateinit var list: RecyclerView
    private lateinit var empty: View
    private val adapter = Adapter()
    /** Decodes thumbnails off the UI thread. */
    private val loader = Executors.newSingleThreadExecutor()

    private val pick = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@registerForActivityResult
        // so the recent list can open it again after a restart
        try {
            contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (_: SecurityException) {
        }
        read(uri)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        recents = Recents(this)
        setContentView(R.layout.activity_main)
        findViewById<MaterialToolbar>(R.id.toolbar).setOnMenuItemClickListener {
            startActivity(Intent(this, SettingsActivity::class.java))
            true
        }
        empty = findViewById(R.id.empty)
        list = findViewById(R.id.list)
        list.adapter = adapter
        // columns about 170 dp wide: two on a phone, four or five on a tablet
        val columns = (resources.configuration.screenWidthDp / 170).coerceAtLeast(2)
        list.layoutManager = GridLayoutManager(this, columns)
        val open = findViewById<ExtendedFloatingActionButton>(R.id.open)
        open.setOnClickListener { pick.launch(MIME_TYPES) }
        ViewCompat.setOnApplyWindowInsetsListener(list) { v, insets ->
            val b = insets.bars()
            v.updatePadding(left = b.left + dp(10f), right = b.right + dp(10f), bottom = b.bottom + dp(88f))
            open.updateLayoutParams<ViewGroup.MarginLayoutParams> {
                bottomMargin = b.bottom + dp(16f)
                rightMargin = b.right + dp(16f)
            }
            insets
        }
        list.addOnScrollListener(object : RecyclerView.OnScrollListener() {
            override fun onScrolled(rv: RecyclerView, dx: Int, dy: Int) {
                if (dy > 8) open.shrink() else if (dy < -8) open.extend()
            }
        })
    }

    override fun onResume() {
        super.onResume()
        refresh()
    }

    private fun refresh() {
        adapter.items = recents.list()
        adapter.notifyDataSetChanged()
        empty.visibility = if (adapter.items.isEmpty()) View.VISIBLE else View.GONE
    }

    override fun onDestroy() {
        super.onDestroy()
        loader.shutdownNow()
    }

    private fun read(uri: Uri, name: String? = null) {
        // no FLAG_GRANT_READ_URI_PERMISSION: the reader is this app, and passing on a permission the
        // app no longer holds (one a sharing app gave for one use) throws
        startActivity(Intent(this, ReaderActivity::class.java).setData(uri).putExtra(ReaderActivity.NAME, name))
    }

    private fun options(r: Recent) {
        val sheet = BottomSheetDialog(this)
        val column = Sheets.column(this, r.name)
        column.addView(Sheets.row(this, R.drawable.ic_delete, "Remove from recent files") {
            sheet.dismiss()
            recents.remove(r.uri)
            refresh()
        })
        sheet.setContentView(column)
        sheet.show()
    }

    private inner class Holder(view: View) : RecyclerView.ViewHolder(view) {
        val thumb: ImageView = view.findViewById(R.id.thumb)
        val name: TextView = view.findViewById(R.id.name)
        val detail: TextView = view.findViewById(R.id.detail)
    }

    private inner class Adapter : RecyclerView.Adapter<Holder>() {
        var items: List<Recent> = emptyList()

        override fun getItemCount() = items.size

        override fun onCreateViewHolder(parent: ViewGroup, type: Int): Holder =
            Holder(LayoutInflater.from(parent.context).inflate(R.layout.item_recent, parent, false))

        override fun onBindViewHolder(h: Holder, position: Int) {
            val r = items[position]
            h.name.text = r.name
            val where = if (r.pages > 0) "Page ${r.page + 1} of ${r.pages}" else "Page ${r.page + 1}"
            val ago = DateUtils.getRelativeTimeSpanString(r.opened, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)
            h.detail.text = "$where · $ago"
            // a page's shape: a little taller than wide
            val columns = (list.layoutManager as GridLayoutManager).spanCount
            val width = (resources.displayMetrics.widthPixels - list.paddingLeft - list.paddingRight) / columns - dp(12f)
            if (width > 0) h.thumb.updateLayoutParams { height = (width * 1.3f).toInt() }
            h.thumb.setImageDrawable(null)
            h.thumb.tag = r.uri
            val file = Recents.thumbnail(this@MainActivity, r.uri)
            if (file.exists()) {
                loader.execute {
                    val bitmap = BitmapFactory.decodeFile(file.path) ?: return@execute
                    runOnUiThread { if (h.thumb.tag == r.uri) h.thumb.setImageBitmap(bitmap) }
                }
            }
            h.itemView.setOnClickListener { read(Uri.parse(r.uri), r.name) }
            h.itemView.setOnLongClickListener {
                options(r)
                true
            }
        }
    }

    companion object {
        private val MIME_TYPES = arrayOf(
            "application/pdf", "application/epub+zip", "image/vnd.djvu", "image/x-djvu",
            "application/vnd.comicbook+zip", "application/x-cbz", "application/x-cb7", "application/zip",
            "application/x-7z-compressed", "application/octet-stream",
            "image/jpeg", "image/png", "image/gif", "image/webp",
        )
    }
}
