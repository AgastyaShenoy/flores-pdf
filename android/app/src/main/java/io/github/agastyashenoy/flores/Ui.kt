package io.github.agastyashenoy.flores

import android.app.Activity
import android.content.Context
import android.content.res.ColorStateList
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.os.Build
import android.util.TypedValue
import android.view.View
import android.view.WindowInsets
import android.widget.ImageButton
import android.widget.TextView

// Small helpers for building views in code: the app has no layouts or libraries to load.

fun Context.dp(v: Float): Int = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v, resources.displayMetrics).toInt()

fun Context.color(id: Int): Int = getColor(id)

/** A borderless round button showing [icon], tinted [tint]. */
fun Context.iconButton(icon: Int, label: String, tint: Int, onClick: (View) -> Unit): ImageButton = ImageButton(this).apply {
    setImageResource(icon)
    imageTintList = ColorStateList.valueOf(tint)
    contentDescription = label
    tooltipText = label
    val mask = GradientDrawable().apply { shape = GradientDrawable.OVAL; setColor(0xFFFFFFFF.toInt()) }
    background = RippleDrawable(ColorStateList.valueOf(tint and 0x33FFFFFF), null, mask)
    val p = dp(12f)
    setPadding(p, p, p, p)
    setOnClickListener(onClick)
}

fun Context.label(size: Float, color: Int, text: String = ""): TextView = TextView(this).apply {
    textSize = size
    setTextColor(color)
    this.text = text
}

/** A rounded rectangle of [color], as a background. */
fun rounded(color: Int, radius: Float): GradientDrawable = GradientDrawable().apply {
    setColor(color)
    cornerRadius = radius
}

/** Lets the window draw behind the status and navigation bars; views pad themselves with the insets. */
@Suppress("DEPRECATION")
fun Activity.edgeToEdge() {
    if (Build.VERSION.SDK_INT >= 30) {
        window.setDecorFitsSystemWindows(false)
    } else {
        window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
            View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
    }
}

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

/** The system bars' sizes: left, top, right, bottom. */
@Suppress("DEPRECATION")
fun WindowInsets.bars(): IntArray = if (Build.VERSION.SDK_INT >= 30) {
    val i = getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
    intArrayOf(i.left, i.top, i.right, i.bottom)
} else {
    intArrayOf(systemWindowInsetLeft, systemWindowInsetTop, systemWindowInsetRight, systemWindowInsetBottom)
}
