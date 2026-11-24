package io.github.agastyashenoy.flores

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.view.Gravity
import android.view.View
import android.widget.FrameLayout
import android.widget.LinearLayout

/** Page colors to choose from: each a small page drawn in its colors, with lines of text and a
 *  colored picture, which keeps its hue in Dark and Black. */
object Looks {
    val NAMES = listOf("Normal", "Dark", "Sepia", "Black")

    /** Around the pages, paper and ink, as the engine draws them (render/src/look.rs). */
    private val PAPER = intArrayOf(0xFFFFFFFF.toInt(), 0xFF262626.toInt(), 0xFFF7EED6.toInt(), 0xFF000000.toInt())
    private val INK = intArrayOf(0xFF2B2B2B.toInt(), 0xFFDCDCDC.toInt(), 0xFF2C241C.toInt(), 0xFFD0D0D0.toInt())
    private val PICTURE = intArrayOf(0xFF2E9C9A.toInt(), 0xFF3FB5B2.toInt(), 0xFF6E8F7A.toInt(), 0xFF3FB5B2.toInt())

    /** The color around pages for [look] in palette [p], as the reader draws it. */
    fun around(p: Pal, look: Int): Int = when (look) {
        1 -> 0x121212
        2 -> 0xD9CDB0
        3 -> 0x000000
        else -> (if (p.dark) 0xFF0B0C0C.toInt() else 0xFFE3E6E5.toInt()) and 0xFFFFFF
    }

    /** A row of the four, [selected] ringed in the accent; [onPick] is told the one tapped. */
    fun picker(c: Context, p: Pal, selected: Int, onPick: (Int) -> Unit): LinearLayout {
        val row = LinearLayout(c).apply { orientation = LinearLayout.HORIZONTAL }
        val tiles = mutableListOf<FrameLayout>()
        fun mark(sel: Int) = tiles.forEachIndexed { i, t ->
            t.background = rounded(0, c.dpf(18f), if (i == sel) p.accent else p.border, c.dp(if (i == sel) 3f else 1f))
        }
        for ((i, name) in NAMES.withIndex()) {
            val column = c.column().apply { gravity = Gravity.CENTER_HORIZONTAL }
            val tile = FrameLayout(c).apply {
                setPadding(c.dp(4f), c.dp(4f), c.dp(4f), c.dp(4f))
                addView(Preview(c, p, i), frame())
                contentDescription = "$name page colors"
                isClickable = true
                setOnClickListener {
                    mark(i)
                    onPick(i)
                }
            }
            tiles += tile
            column.addView(tile, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, c.dp(96f)))
            column.addView(c.label(name, 13.5f, p.text, bold = true).apply { setPadding(0, c.dp(8f), 0, 0) })
            row.addView(column, LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f).apply {
                marginStart = if (i == 0) 0 else c.dp(5f)
                marginEnd = if (i == NAMES.lastIndex) 0 else c.dp(5f)
            })
        }
        mark(selected)
        return row
    }

    private class Preview(context: Context, private val p: Pal, private val look: Int) : View(context) {
        private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        private val r = RectF()

        override fun onDraw(canvas: Canvas) {
            val d = resources.displayMetrics.density
            val outer = 14 * d
            r.set(0f, 0f, width.toFloat(), height.toFloat())
            paint.color = around(p, look) or (0xFF shl 24)
            canvas.drawRoundRect(r, outer, outer, paint)
            canvas.save()
            canvas.clipRect(r)
            val w = width * 0.62f
            r.set((width - w) / 2, height * 0.2f, (width + w) / 2, height.toFloat() + 6 * d)
            paint.color = PAPER[look]
            canvas.drawRoundRect(r, 3 * d, 3 * d, paint)
            if (look == 3) {
                // a black page on black needs an edge to be seen
                paint.style = Paint.Style.STROKE
                paint.strokeWidth = d
                paint.color = 0xFF3A3A3A.toInt()
                canvas.drawRoundRect(r, 3 * d, 3 * d, paint)
                paint.style = Paint.Style.FILL
            }
            val left = r.left + 7 * d
            var y = r.top + 8 * d
            paint.color = INK[look]
            canvas.drawRoundRect(left, y, left + w * 0.5f, y + 4 * d, 2 * d, 2 * d, paint)
            y += 9 * d
            paint.color = PICTURE[look]
            canvas.drawRoundRect(left, y, left + w * 0.34f, y + 16 * d, 2 * d, 2 * d, paint)
            paint.color = INK[look] and 0x00FFFFFF or (0xB0 shl 24)
            var line = y
            while (line < height - 8 * d) {
                val start = if (line < y + 16 * d) left + w * 0.34f + 5 * d else left
                canvas.drawRoundRect(start, line, r.right - 7 * d, line + 2.4f * d, d, d, paint)
                line += 6 * d
            }
            canvas.restore()
        }
    }
}
