package io.github.agastyashenoy.flores

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.util.AttributeSet
import android.view.Gravity
import android.view.View
import android.widget.LinearLayout
import android.widget.TextView
import com.google.android.material.card.MaterialCardView
import com.google.android.material.color.MaterialColors

/**
 * The page colors to choose from, each a small page drawn in its colors: paper, lines of text and
 * a colored picture, which keeps its hue in Dark and Black.
 */
class LookPicker @JvmOverloads constructor(context: Context, attrs: AttributeSet? = null) : LinearLayout(context, attrs) {
    var onPick: ((Int) -> Unit)? = null
    private val cards = mutableListOf<MaterialCardView>()

    var selected: Int = 0
        set(v) {
            field = v
            cards.forEachIndexed { i, c ->
                val on = i == v
                c.strokeWidth = context.dp(if (on) 3f else 1f)
                c.strokeColor = MaterialColors.getColor(c, if (on) com.google.android.material.R.attr.colorPrimary else com.google.android.material.R.attr.colorOutlineVariant)
                c.isChecked = on
            }
        }

    init {
        orientation = HORIZONTAL
        for ((i, name) in NAMES.withIndex()) {
            val column = LinearLayout(context).apply {
                orientation = VERTICAL
                gravity = Gravity.CENTER_HORIZONTAL
            }
            val card = MaterialCardView(context).apply {
                radius = context.dp(14f).toFloat()
                cardElevation = 0f
                isCheckable = true
                checkedIcon = null
                contentDescription = "$name page colors"
                setOnClickListener {
                    selected = i
                    onPick?.invoke(i)
                }
                addView(Preview(context, i))
            }
            cards += card
            column.addView(card, LayoutParams(LayoutParams.MATCH_PARENT, context.dp(92f)))
            column.addView(TextView(context).apply {
                text = name
                setTextAppearance(com.google.android.material.R.style.TextAppearance_Material3_LabelLarge)
                setPadding(0, context.dp(8f), 0, 0)
            })
            addView(column, LayoutParams(0, LayoutParams.WRAP_CONTENT, 1f).apply {
                marginStart = if (i == 0) 0 else context.dp(6f)
                marginEnd = if (i == NAMES.lastIndex) 0 else context.dp(6f)
            })
        }
        selected = 0
    }

    /** A page in look [look]'s colors, on the color around pages. */
    private class Preview(context: Context, private val look: Int) : View(context) {
        private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        private val r = RectF()

        override fun onDraw(canvas: Canvas) {
            val (around, paper, ink) = COLORS[look]
            val d = resources.displayMetrics.density
            canvas.drawColor(if (look == 0) MaterialColors.getColor(this, com.google.android.material.R.attr.colorSurfaceContainerHighest) else around)
            val w = width * 0.62f
            val h = height * 0.78f
            r.set((width - w) / 2, height - h, (width + w) / 2, height.toFloat() + 4 * d)
            paint.color = paper
            canvas.drawRoundRect(r, 3 * d, 3 * d, paint)
            if (look == 3) {
                // a black page on black needs an edge to be seen
                paint.style = Paint.Style.STROKE
                paint.strokeWidth = d
                paint.color = 0xFF3A3A3A.toInt()
                canvas.drawRoundRect(r, 3 * d, 3 * d, paint)
                paint.style = Paint.Style.FILL
            }
            // a heading, a picture, and lines of text
            val left = r.left + 7 * d
            var y = r.top + 8 * d
            paint.color = ink
            canvas.drawRoundRect(left, y, left + w * 0.5f, y + 4 * d, 2 * d, 2 * d, paint)
            y += 9 * d
            paint.color = PICTURE[look]
            canvas.drawRoundRect(left, y, left + w * 0.34f, y + 16 * d, 2 * d, 2 * d, paint)
            paint.color = ink and 0x00FFFFFF or (0xB0 shl 24)
            var line = y
            while (line < r.bottom - 6 * d) {
                val start = if (line < y + 16 * d) left + w * 0.34f + 5 * d else left
                canvas.drawRoundRect(start, line, r.right - 7 * d, line + 2.4f * d, d, d, paint)
                line += 6 * d
            }
        }
    }

    companion object {
        val NAMES = listOf("Normal", "Dark", "Sepia", "Black")

        /** Around the pages, paper and ink, as the engine draws them (render/src/look.rs). */
        private val COLORS = listOf(
            Triple(0, 0xFFFFFFFF.toInt(), 0xFF2B2B2B.toInt()),
            Triple(0xFF121212.toInt(), 0xFF262626.toInt(), 0xFFDCDCDC.toInt()),
            Triple(0xFFD9CDB0.toInt(), 0xFFF7EED6.toInt(), 0xFF2C241C.toInt()),
            Triple(0xFF000000.toInt(), 0xFF000000.toInt(), 0xFFD0D0D0.toInt()),
        )

        /** The little picture: hue kept, lightness flipped in Dark and Black. */
        private val PICTURE = intArrayOf(0xFF2E9C9A.toInt(), 0xFF3FB5B2.toInt(), 0xFF6E8F7A.toInt(), 0xFF3FB5B2.toInt())

        /** The color around pages for [look], as the reader draws it. */
        fun around(view: View, look: Int, dark: Boolean): Int = when (look) {
            1 -> 0x121212
            2 -> 0xD9CDB0
            3 -> 0x000000
            else -> MaterialColors.getColor(
                view,
                if (dark) com.google.android.material.R.attr.colorSurfaceContainerLow else com.google.android.material.R.attr.colorSurfaceContainerHighest,
            ) and 0xFFFFFF
        }
    }
}
