package io.github.agastyashenoy.flores

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.LinearLayout
import android.widget.TextView
import androidx.core.view.ViewCompat
import androidx.core.view.updatePadding
import androidx.core.widget.NestedScrollView
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.button.MaterialButtonToggleGroup
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.materialswitch.MaterialSwitch
import com.google.android.material.slider.Slider

class SettingsActivity : FloresActivity() {
    private lateinit var recents: Recents

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        recents = Recents(this)
        setContentView(R.layout.activity_settings)
        findViewById<MaterialToolbar>(R.id.toolbar).setNavigationOnClickListener { finish() }
        val scroll = findViewById<NestedScrollView>(R.id.scroll)
        ViewCompat.setOnApplyWindowInsetsListener(scroll) { v, insets ->
            val b = insets.bars()
            v.updatePadding(left = b.left, right = b.right, bottom = b.bottom)
            insets
        }

        val ids = listOf(R.id.theme_system, R.id.theme_light, R.id.theme_dark, R.id.theme_black)
        val theme = findViewById<MaterialButtonToggleGroup>(R.id.theme)
        theme.check(ids[recents.theme.coerceIn(0, 3)])
        theme.addOnButtonCheckedListener { _, id, checked ->
            if (!checked) return@addOnButtonCheckedListener
            val chosen = ids.indexOf(id)
            if (chosen == recents.theme) return@addOnButtonCheckedListener
            recents.theme = chosen
            FloresApp.applyTheme(chosen)
            // light and dark recreate the screen themselves; Dark and Black differ only in surfaces
            recreate()
        }

        val looks = findViewById<LookPicker>(R.id.looks)
        looks.selected = recents.look
        looks.onPick = { recents.look = it }

        findViewById<MaterialSwitch>(R.id.keep_screen_on).apply {
            isChecked = recents.keepScreenOn
            setOnCheckedChangeListener { _, on -> recents.keepScreenOn = on }
        }
        findViewById<MaterialSwitch>(R.id.volume_keys).apply {
            isChecked = recents.volumeKeys
            setOnCheckedChangeListener { _, on -> recents.volumeKeys = on }
        }

        val value = findViewById<TextView>(R.id.limit_value)
        val limit = findViewById<Slider>(R.id.limit)
        limit.value = recents.limit.toFloat()
        value.text = "${recents.limit} files"
        limit.setLabelFormatter { "${it.toInt()} files" }
        limit.addOnChangeListener { _, v, _ -> value.text = "${v.toInt()} files" }
        limit.addOnSliderTouchListener(object : Slider.OnSliderTouchListener {
            override fun onStartTrackingTouch(slider: Slider) {}
            override fun onStopTrackingTouch(slider: Slider) {
                val n = slider.value.toInt()
                val dropped = recents.list().size - n
                if (dropped <= 0) {
                    recents.limit = n
                    return
                }
                // fewer than the list holds: say what goes before it goes
                MaterialAlertDialogBuilder(this@SettingsActivity)
                    .setTitle("Remember $n files?")
                    .setMessage("The $dropped oldest ${if (dropped == 1) "file leaves" else "files leave"} the recent list, with any private copies and bookmarks.")
                    .setPositiveButton("Remember $n") { _, _ -> recents.limit = n }
                    .setNegativeButton("Cancel") { _, _ -> slider.value = recents.limit.toFloat() }
                    .setOnCancelListener { slider.value = recents.limit.toFloat() }
                    .show()
            }
        })

        findViewById<android.widget.Button>(R.id.clear).setOnClickListener {
            MaterialAlertDialogBuilder(this)
                .setTitle("Clear recent files?")
                .setMessage("The list is emptied, with the places you left each file, their bookmarks, and private copies of shared files. The files themselves aren't touched.")
                .setPositiveButton("Clear") { _, _ -> recents.clear() }
                .setNegativeButton("Cancel", null)
                .show()
        }

        val about = findViewById<LinearLayout>(R.id.about)
        val version = packageManager.getPackageInfo(packageName, 0).versionName
        about.addView(Sheets.row(this, R.drawable.ic_info, getString(R.string.app_name), "Version $version") {})
        about.addView(Sheets.row(this, R.drawable.ic_open_in_new, "Source code and releases", "github.com/AgastyaShenoy/flores-pdf") {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://github.com/AgastyaShenoy/flores-pdf")))
        })
    }
}
