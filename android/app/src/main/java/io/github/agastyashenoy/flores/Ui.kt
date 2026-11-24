package io.github.agastyashenoy.flores

import android.app.Activity
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.util.TypedValue

fun Context.dp(v: Float): Int = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v, resources.displayMetrics).toInt()

/** Whether the app shows dark. */
val Context.isDark: Boolean
    get() = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES

/** The display's fastest refresh rate at its current resolution. */
@Suppress("DEPRECATION")
fun Activity.fastestMode(): android.view.Display.Mode {
    val display = if (Build.VERSION.SDK_INT >= 30) display ?: windowManager.defaultDisplay else windowManager.defaultDisplay
    val now = display.mode
    return display.supportedModes
        .filter { it.physicalWidth == now.physicalWidth && it.physicalHeight == now.physicalHeight }
        .maxByOrNull { it.refreshRate } ?: now
}

/** Asks for the display's fastest refresh rate while this window shows: systems that save power
 *  (HyperOS among them) otherwise keep apps at 60 Hz or less, and scrolling looks it. */
fun Activity.preferFastRefresh() {
    window.attributes = window.attributes.also { it.preferredDisplayModeId = fastestMode().modeId }
}
