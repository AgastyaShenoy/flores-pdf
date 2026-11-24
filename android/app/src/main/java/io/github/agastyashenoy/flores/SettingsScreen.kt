package io.github.agastyashenoy.flores

import android.content.Intent
import android.net.Uri
import android.view.View
import android.widget.LinearLayout
import android.widget.ScrollView

/** Settings, in cards: appearance, reading, recent files, and about. */
class SettingsScreen(private val a: MainActivity, insets: IntArray) {
    private val p = a.pal
    private val recents = a.recents
    val view: View

    init {
        val col = a.column().apply { setPadding(a.dp(16f) + insets[0], insets[1], a.dp(16f) + insets[2], insets[3] + a.dp(24f)) }
        val head = a.row().apply { setPadding(0, a.dp(4f), 0, a.dp(4f)) }
        head.addView(a.iconButton(p, R.drawable.ic_back, "Back") { a.onBackPressedDispatcherCompat() })
        head.addView(a.label("Settings", 22f, p.text).apply { setPadding(a.dp(8f), 0, 0, 0) })
        col.addView(head)

        col.addView(a.section(p, "Appearance"))
        val look = a.card(p).apply { setPadding(a.dp(16f), a.dp(16f), a.dp(16f), a.dp(16f)) }
        look.addView(a.label("Theme", 16f, p.text, bold = true))
        look.addView(a.label("Black is dark with pure black for OLED screens.", 13.5f, p.muted), lp(bottom = a.dp(12f)))
        look.addView(a.segmented(p, listOf("System", "Light", "Dark", "Black"), recents.theme) {
            recents.theme = it
            a.restyle()
        })
        look.addView(a.label("Page colors", 16f, p.text, bold = true), lp(top = a.dp(24f)))
        look.addView(a.label("How documents look. Pictures keep their colors in Dark and Black.", 13.5f, p.muted), lp(bottom = a.dp(12f)))
        look.addView(Looks.picker(a, p, recents.look) { recents.look = it })
        col.addView(look)

        col.addView(a.section(p, "Reading"))
        val reading = a.card(p).apply { setPadding(0, a.dp(4f), 0, a.dp(4f)) }
        reading.addView(a.listRow(p, 0, "Keep the screen on while reading", end = Toggle(a, p, recents.keepScreenOn) { recents.keepScreenOn = it }))
        reading.addView(a.listRow(p, 0, "Volume keys turn pages", end = Toggle(a, p, recents.volumeKeys) { recents.volumeKeys = it }))
        col.addView(reading)

        col.addView(a.section(p, "Recent files"))
        val files = a.card(p).apply { setPadding(a.dp(16f), a.dp(16f), a.dp(16f), a.dp(16f)) }
        val top = a.row()
        top.addView(a.label("Remember", 16f, p.text, bold = true), LinearLayout.LayoutParams(0, -2, 1f))
        val value = a.label("${recents.limit} files", 16f, p.accent, bold = true)
        top.addView(value)
        files.addView(top)
        val steps = (Recents.MAX_LIMIT - Recents.MIN_LIMIT) / 5
        files.addView(a.slider(p, steps, (recents.limit - Recents.MIN_LIMIT) / 5) { v, moving ->
            val n = Recents.MIN_LIMIT + v * 5
            value.text = "$n files"
            if (!moving) setLimit(n)
        }.apply { contentDescription = "Recent files to remember" }, lp(top = a.dp(8f), bottom = a.dp(8f)))
        files.addView(a.label(
            "Each file opens again where you left it. Files shared from other apps, such as WhatsApp or mail, are kept as private copies so they open again too; a copy is deleted when its file leaves the list.",
            13.5f, p.muted,
        ).apply { setLineSpacing(0f, 1.15f) })
        files.addView(a.button(p, "Clear recent files", Kind.Tonal) {
            a.ask(p, "Clear recent files?", "The list is emptied, with the places you left each file, their bookmarks, and private copies of shared files. The files themselves aren't touched.", ok = "Clear") {
                recents.clear()
            }
        }, LinearLayout.LayoutParams(-2, -2).apply { topMargin = a.dp(16f) })
        col.addView(files)

        col.addView(a.section(p, "About"))
        val about = a.card(p)
        val version = a.packageManager.getPackageInfo(a.packageName, 0).versionName
        about.addView(a.listRow(p, R.drawable.ic_info, a.getString(R.string.app_name), "Version $version"))
        about.addView(a.listRow(p, R.drawable.ic_open_in_new, "Source code and releases", "github.com/AgastyaShenoy/flores-pdf") {
            a.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://github.com/AgastyaShenoy/flores-pdf")))
        })
        col.addView(about)

        view = ScrollView(a).apply {
            isVerticalScrollBarEnabled = false
            addView(col)
        }
    }

    /** Fewer than the list holds: say what goes before it goes. */
    private fun setLimit(n: Int) {
        if (n == recents.limit) return
        val dropped = recents.list().size - n
        if (dropped <= 0) {
            recents.limit = n
            return
        }
        a.ask(
            p, "Remember $n files?",
            "The $dropped oldest ${if (dropped == 1) "file leaves" else "files leave"} the recent list, with any private copies and bookmarks.",
            ok = "Remember $n",
            onCancel = { a.openSettings() },
        ) { recents.limit = n }
    }
}

/** The back button's action, from a screen's own back arrow. */
@Suppress("DEPRECATION")
fun MainActivity.onBackPressedDispatcherCompat() = onBackPressed()
