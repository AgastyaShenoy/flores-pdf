package io.github.agastyashenoy.flores

import android.animation.ValueAnimator
import android.app.Activity
import android.app.Dialog
import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.graphics.drawable.ColorDrawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.os.Build
import android.text.InputType
import android.text.TextUtils
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.Window
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.view.animation.DecelerateInterpolator
import android.view.animation.PathInterpolator
import android.view.inputmethod.EditorInfo
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.SeekBar
import android.widget.TextView

// The interface's toolkit: its colors, and widgets built in code from the platform's own views, in
// the manner of Flax Reader. No layouts to inflate and no libraries, so screens open at once.

/** One color theme. */
class Pal(
    val dark: Boolean,
    val bg: Int,
    val card: Int,
    val text: Int,
    val muted: Int,
    val border: Int,
    val accent: Int,
    val onAccent: Int,
) {
    /** The current choice in segments and lists: a neutral fill, leaving the accent for the one main
     *  action on a screen. */
    val selected = alpha(text, 0.10f)
    /** Bars over the pages: the card color, nearly opaque. */
    val bar = alpha(card, 0.96f)

    companion object {
        val light = Pal(false, 0xFFF3F4F3.toInt(), 0xFFFFFFFF.toInt(), 0xFF1A1D1D.toInt(), 0xFF5D6767.toInt(), 0xFFE0E4E3.toInt(), 0xFF0E7C7B.toInt(), 0xFFFFFFFF.toInt())
        val dark = Pal(true, 0xFF121414.toInt(), 0xFF1C1F1F.toInt(), 0xFFE5EAE9.toInt(), 0xFF9AA4A3.toInt(), 0xFF2B3030.toInt(), 0xFF5CC3C0.toInt(), 0xFF00302F.toInt())
        val black = Pal(true, 0xFF000000.toInt(), 0xFF0F1111.toInt(), 0xFFE5EAE9.toInt(), 0xFF9AA4A3.toInt(), 0xFF202424.toInt(), 0xFF5CC3C0.toInt(), 0xFF00302F.toInt())

        /** The palette for the theme setting (0 system, 1 light, 2 dark, 3 black). */
        fun of(c: Context): Pal = when (Recents(c).theme) {
            1 -> light
            2 -> dark
            3 -> black
            else -> if (c.isDark) dark else light
        }
    }
}

fun alpha(color: Int, a: Float): Int = (color and 0x00FFFFFF) or ((a * 255).toInt().coerceIn(0, 255) shl 24)

fun Context.dpf(v: Float): Float = v * resources.displayMetrics.density

fun rounded(color: Int, radius: Float, stroke: Int = 0, strokeWidth: Int = 0): GradientDrawable = GradientDrawable().apply {
    setColor(color)
    cornerRadius = radius
    if (strokeWidth > 0) setStroke(strokeWidth, stroke)
}

fun lp(w: Int = ViewGroup.LayoutParams.MATCH_PARENT, h: Int = ViewGroup.LayoutParams.WRAP_CONTENT, top: Int = 0, bottom: Int = 0) =
    LinearLayout.LayoutParams(w, h).apply {
        topMargin = top
        bottomMargin = bottom
    }

fun frame(w: Int = ViewGroup.LayoutParams.MATCH_PARENT, h: Int = ViewGroup.LayoutParams.MATCH_PARENT, gravity: Int = Gravity.NO_GRAVITY) =
    FrameLayout.LayoutParams(w, h, gravity)

fun Context.column(): LinearLayout = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }

fun Context.row(): LinearLayout = LinearLayout(this).apply {
    orientation = LinearLayout.HORIZONTAL
    gravity = Gravity.CENTER_VERTICAL
}

fun Context.label(text: CharSequence, sp: Float, color: Int, bold: Boolean = false, lines: Int = 0): TextView = TextView(this).apply {
    this.text = text
    textSize = sp
    setTextColor(color)
    typeface = if (bold) Typeface.create("sans-serif-medium", Typeface.NORMAL) else Typeface.DEFAULT
    if (lines > 0) {
        maxLines = lines
        ellipsize = TextUtils.TruncateAt.END
    }
}

fun Context.icon(res: Int, color: Int, size: Float = 24f): ImageView = ImageView(this).apply {
    setImageResource(res)
    imageTintList = ColorStateList.valueOf(color)
    layoutParams = LinearLayout.LayoutParams(dp(size), dp(size))
}

/** A round icon button with a ripple; [tip] names it for long presses and screen readers. */
fun Context.iconButton(p: Pal, res: Int, tip: String, onClick: (View) -> Unit): FrameLayout = FrameLayout(this).apply {
    val pad = dp(12f)
    setPadding(pad, pad, pad, pad)
    val radius = dpf(24f)
    background = RippleDrawable(ColorStateList.valueOf(alpha(p.text, 0.14f)), null, rounded(0xFFFFFFFF.toInt(), radius))
    addView(icon(res, p.text).apply { layoutParams = FrameLayout.LayoutParams(dp(24f), dp(24f)) })
    isClickable = true
    setOnClickListener(onClick)
    tooltipText = tip
    contentDescription = tip
}

/** Primary: the one main action on a screen. Tonal: a quiet fill. Ghost: text only. */
enum class Kind { Primary, Tonal, Ghost }

fun Context.button(p: Pal, text: String, kind: Kind = Kind.Tonal, iconRes: Int = 0, onClick: () -> Unit): TextView {
    val (fill, fg) = when (kind) {
        Kind.Primary -> p.accent to p.onAccent
        Kind.Tonal -> alpha(p.text, 0.08f) to p.text
        Kind.Ghost -> 0 to p.accent
    }
    return label(text, 14.5f, fg, bold = true).apply {
        gravity = Gravity.CENTER
        setPadding(dp(if (iconRes != 0) 16f else 20f), dp(11f), dp(20f), dp(11f))
        if (iconRes != 0) {
            val d = getDrawable(iconRes)!!.mutate()
            d.setTint(fg)
            d.setBounds(0, 0, dp(20f), dp(20f))
            setCompoundDrawablesRelative(d, null, null, null)
            compoundDrawablePadding = dp(8f)
        }
        val radius = dpf(22f)
        background = RippleDrawable(ColorStateList.valueOf(alpha(if (kind == Kind.Primary) p.onAccent else p.text, 0.16f)), rounded(fill, radius), rounded(0xFFFFFFFF.toInt(), radius))
        isClickable = true
        setOnClickListener { onClick() }
    }
}

/** A card: the card color, rounded, with a hairline edge. */
fun Context.card(p: Pal): LinearLayout = column().apply {
    background = rounded(p.card, dpf(16f), p.border, if (p.dark) dp(1f) else 0)
}

/** A section heading over a card. */
fun Context.section(p: Pal, title: String): TextView = label(title, 13.5f, p.accent, bold = true).apply {
    setPadding(dp(8f), dp(24f), dp(8f), dp(8f))
}

/** A row in a card or panel: an icon, a title, a line under it, and something at its end. */
fun Context.listRow(p: Pal, iconRes: Int, title: String, sub: String? = null, end: View? = null, onClick: (() -> Unit)? = null): LinearLayout = row().apply {
    minimumHeight = dp(56f)
    setPadding(dp(20f), dp(10f), dp(16f), dp(10f))
    if (iconRes != 0) addView(icon(iconRes, p.muted).apply { (layoutParams as LinearLayout.LayoutParams).marginEnd = dp(18f) })
    val words = column()
    words.addView(label(title, 16f, p.text))
    if (sub != null) words.addView(label(sub, 13.5f, p.muted).apply { tag = "sub" })
    addView(words, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
    if (end != null) addView(end)
    if (onClick != null) {
        background = RippleDrawable(ColorStateList.valueOf(alpha(p.text, 0.10f)), null, ColorDrawable(0xFFFFFFFF.toInt()))
        isClickable = true
        setOnClickListener { onClick() }
    }
}

/** Mutually exclusive options in a row; the chosen one gets a neutral fill. */
fun Context.segmented(p: Pal, options: List<String>, selected: Int, onPick: (Int) -> Unit): LinearLayout {
    val box = row().apply {
        background = rounded(alpha(p.text, 0.05f), dpf(22f), p.border, dp(1f))
        setPadding(dp(3f), dp(3f), dp(3f), dp(3f))
    }
    fun paint(sel: Int) {
        for (i in 0 until box.childCount) {
            val t = box.getChildAt(i) as TextView
            val on = i == sel
            t.setTextColor(if (on) p.text else p.muted)
            t.typeface = if (on) Typeface.create("sans-serif-medium", Typeface.NORMAL) else Typeface.DEFAULT
            t.background = if (on) rounded(p.card, dpf(19f), p.border, if (p.dark) dp(1f) else 0).also { if (!p.dark) t.elevation = dpf(1f) } else null
            if (!on) t.elevation = 0f
            t.isSelected = on
        }
    }
    options.forEachIndexed { i, o ->
        val t = label(o, 14f, p.muted, lines = 1).apply {
            gravity = Gravity.CENTER
            setPadding(dp(6f), dp(9f), dp(6f), dp(9f))
            setOnClickListener {
                paint(i)
                onPick(i)
            }
        }
        box.addView(t, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
    }
    paint(selected)
    return box
}

/** An on/off switch: a pill track and a round knob that slides. */
class Toggle(context: Context, private val p: Pal, on: Boolean, private val onChange: (Boolean) -> Unit) : View(context) {
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val r = RectF()
    private var t = if (on) 1f else 0f
    var checked = on
        private set

    init {
        isClickable = true
        setOnClickListener { set(!checked) }
        layoutParams = LinearLayout.LayoutParams(context.dp(52f), context.dp(32f))
    }

    fun set(on: Boolean) {
        if (on == checked) return
        checked = on
        ValueAnimator.ofFloat(t, if (on) 1f else 0f).apply {
            duration = 160
            interpolator = Anim.ease
            addUpdateListener {
                t = it.animatedValue as Float
                invalidate()
            }
            start()
        }
        onChange(on)
    }

    override fun onDraw(canvas: Canvas) {
        val h = height.toFloat()
        r.set(0f, 0f, width.toFloat(), h)
        paint.color = blend(alpha(p.text, 0.16f), p.accent, t)
        canvas.drawRoundRect(r, h / 2, h / 2, paint)
        val knob = h / 2 - context.dpf(4f + 2f * t)
        val cx = h / 2 + (width - h) * t
        paint.color = blend(if (p.dark) p.muted else 0xFFFFFFFF.toInt(), p.onAccent, t)
        canvas.drawCircle(cx, h / 2, knob, paint)
    }

    private fun blend(a: Int, b: Int, f: Float): Int {
        val ch = { s: Int -> (((a shr s) and 0xFF) * (1 - f) + ((b shr s) and 0xFF) * f).toInt() shl s }
        return ch(24) or ch(16) or ch(8) or ch(0)
    }
}

/** A slider in the accent color. */
fun Context.slider(p: Pal, max: Int, value: Int, onChange: (Int, Boolean) -> Unit): SeekBar = SeekBar(this).apply {
    this.max = max
    progress = value
    progressTintList = ColorStateList.valueOf(p.accent)
    thumbTintList = ColorStateList.valueOf(p.accent)
    progressBackgroundTintList = ColorStateList.valueOf(alpha(p.text, 0.25f))
    setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
        override fun onProgressChanged(s: SeekBar, v: Int, user: Boolean) = onChange(v, user)
        override fun onStartTrackingTouch(s: SeekBar) {}
        override fun onStopTrackingTouch(s: SeekBar) = onChange(s.progress, false)
    })
}

/** Short interface animations, skipped when Android's own are turned off. */
object Anim {
    val on: Boolean get() = ValueAnimator.areAnimatorsEnabled()

    /** Material's "emphasized decelerate" curve: quick start, gentle landing. */
    val ease = PathInterpolator(0.05f, 0.7f, 0.1f, 1f)
    private val decel = DecelerateInterpolator(1.6f)

    fun enter(v: View, dx: Float = 0f, dy: Float = 0f, ms: Long = 180) {
        v.animate().cancel()
        if (!on) {
            v.alpha = 1f
            v.translationX = 0f
            v.translationY = 0f
            return
        }
        v.alpha = 0f
        v.translationX = dx
        v.translationY = dy
        v.animate().alpha(1f).translationX(0f).translationY(0f).setDuration(ms).setInterpolator(ease).withEndAction(null).start()
    }

    fun exit(v: View, dx: Float = 0f, dy: Float = 0f, ms: Long = 140, end: () -> Unit) {
        v.animate().cancel()
        if (!on) return end()
        v.animate().alpha(0f).translationX(dx).translationY(dy).setDuration(ms).setInterpolator(decel).withEndAction(end).start()
    }
}

/**
 * Panels that slide up from the bottom of the screen, in the window itself rather than a dialog's
 * own: a dimmed backdrop, and a rounded sheet with a handle. One at a time.
 */
class Panels(private val host: FrameLayout, private val p: Pal) {
    private var shown: Pair<View, View>? = null
    val open: Boolean get() = shown != null

    /** Shows [content] in a panel; [title] goes at its top. */
    fun show(content: View, title: String? = null, scroll: Boolean = true) {
        close(animate = false)
        val c = host.context
        val scrim = View(c).apply {
            setBackgroundColor(alpha(0xFF000000.toInt(), if (p.dark) 0.55f else 0.32f))
            isClickable = true
            setOnClickListener { close() }
        }
        val sheet = c.column().apply {
            background = GradientDrawable().apply {
                setColor(p.card)
                val r = c.dpf(28f)
                cornerRadii = floatArrayOf(r, r, r, r, 0f, 0f, 0f, 0f)
            }
            elevation = c.dpf(8f)
            isClickable = true
        }
        sheet.addView(View(c).apply { background = rounded(alpha(p.text, 0.25f), c.dpf(2f)) }, LinearLayout.LayoutParams(c.dp(32f), c.dp(4f)).apply {
            gravity = Gravity.CENTER_HORIZONTAL
            topMargin = c.dp(12f)
            bottomMargin = c.dp(if (title != null) 12f else 8f)
        })
        if (title != null) sheet.addView(c.label(title, 22f, p.text).apply { setPadding(c.dp(24f), 0, c.dp(24f), c.dp(8f)) })
        val body = if (scroll) ScrollView(c).apply {
            isVerticalScrollBarEnabled = false
            addView(content)
        } else content
        sheet.addView(body, lp())
        // the system's navigation bar below, and no wider than reads well on a tablet
        val bottomInset = host.rootWindowInsets?.let { insetsBottom(it) } ?: 0
        sheet.setPadding(0, 0, 0, bottomInset + c.dp(12f))
        val width = minOf(c.resources.displayMetrics.widthPixels, c.dp(640f))
        host.addView(scrim, frame())
        host.addView(sheet, FrameLayout.LayoutParams(width, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM or Gravity.CENTER_HORIZONTAL).apply {
            topMargin = c.dp(64f)
        })
        shown = scrim to sheet
        Anim.enter(scrim, ms = 160)
        Anim.enter(sheet, dy = c.dpf(160f), ms = 240)
    }

    fun close(animate: Boolean = true) {
        val (scrim, sheet) = shown ?: return
        shown = null
        if (!animate) {
            host.removeView(scrim)
            host.removeView(sheet)
            return
        }
        Anim.exit(scrim, ms = 140) { host.removeView(scrim) }
        Anim.exit(sheet, dy = sheet.height * 0.6f, ms = 160) { host.removeView(sheet) }
    }

    @Suppress("DEPRECATION")
    private fun insetsBottom(i: WindowInsets): Int =
        if (Build.VERSION.SDK_INT >= 30) i.getInsets(WindowInsets.Type.systemBars()).bottom else i.systemWindowInsetBottom
}

/** A dialog in the app's colors (the platform's own are dark grey whatever the theme): a title, a
 *  message or a field, Cancel and the one action. */
fun Activity.ask(
    p: Pal,
    title: String,
    message: String? = null,
    hint: String? = null,
    initial: String = "",
    number: Boolean = false,
    password: Boolean = false,
    ok: String,
    other: String? = null,
    onOther: (() -> Unit)? = null,
    onCancel: (() -> Unit)? = null,
    onOk: (String) -> Unit,
) {
    val d = Dialog(this)
    d.requestWindowFeature(Window.FEATURE_NO_TITLE)
    val box = column().apply {
        background = rounded(p.card, dpf(28f))
        setPadding(dp(24f), dp(24f), dp(24f), dp(16f))
    }
    box.addView(label(title, 22f, p.text))
    if (message != null) box.addView(label(message, 14.5f, p.muted).apply { setLineSpacing(0f, 1.15f) }, lp(top = dp(12f)))
    var field: EditText? = null
    if (hint != null) {
        field = EditText(this).apply {
            this.hint = hint
            setHintTextColor(p.muted)
            setTextColor(p.text)
            textSize = 16f
            isSingleLine = true
            setText(initial)
            setSelectAllOnFocus(true)
            inputType = when {
                number -> InputType.TYPE_CLASS_NUMBER
                password -> InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
                else -> InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
            }
            imeOptions = EditorInfo.IME_ACTION_DONE
            background = rounded(alpha(p.text, 0.05f), dpf(12f), p.border, dp(1f))
            setPadding(dp(16f), dp(14f), dp(16f), dp(14f))
        }
        box.addView(field, lp(top = dp(20f)))
    }
    val buttons = row().apply { gravity = Gravity.END or Gravity.CENTER_VERTICAL }
    var done = false
    val confirm = {
        done = true
        d.dismiss()
        onOk(field?.text?.toString().orEmpty())
    }
    if (other != null) {
        // a second way out, such as Discard, on its own at the start
        buttons.addView(button(p, other, Kind.Ghost) {
            done = true
            d.dismiss()
            onOther?.invoke()
        })
        buttons.addView(View(this), LinearLayout.LayoutParams(0, 1, 1f))
    }
    buttons.addView(button(p, "Cancel", Kind.Ghost) { d.cancel() })
    buttons.addView(button(p, ok, Kind.Primary) { confirm() }, LinearLayout.LayoutParams(-2, -2).apply { marginStart = dp(8f) })
    box.addView(buttons, lp(top = dp(24f)))
    field?.setOnEditorActionListener { _, _, _ ->
        confirm()
        true
    }
    d.setOnCancelListener { if (!done) onCancel?.invoke() }
    d.setContentView(box)
    d.window?.apply {
        setBackgroundDrawable(ColorDrawable(0))
        setDimAmount(if (p.dark) 0.6f else 0.4f)
        if (field != null) setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE or WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
    }
    d.show()
    d.window?.setLayout(minOf(resources.displayMetrics.widthPixels - dp(48f), dp(420f)), ViewGroup.LayoutParams.WRAP_CONTENT)
    field?.requestFocus()
}

/** Draws behind the system bars, with their icons dark on a light palette. */
@Suppress("DEPRECATION")
fun Activity.edgeToEdge(p: Pal) {
    window.statusBarColor = 0
    window.navigationBarColor = 0
    if (Build.VERSION.SDK_INT >= 30) {
        window.setDecorFitsSystemWindows(false)
        val light = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS
        window.insetsController?.setSystemBarsAppearance(if (p.dark) 0 else light, light)
    } else {
        var flags = View.SYSTEM_UI_FLAG_LAYOUT_STABLE or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
        if (!p.dark) flags = flags or View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR or (if (Build.VERSION.SDK_INT >= 26) View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR else 0)
        window.decorView.systemUiVisibility = flags
    }
}

/** The system bars' sizes: left, top, right, bottom. */
@Suppress("DEPRECATION")
fun WindowInsets.bars(): IntArray = if (Build.VERSION.SDK_INT >= 30) {
    val i = getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
    intArrayOf(i.left, i.top, i.right, i.bottom)
} else {
    intArrayOf(systemWindowInsetLeft, systemWindowInsetTop, systemWindowInsetRight, systemWindowInsetBottom)
}
