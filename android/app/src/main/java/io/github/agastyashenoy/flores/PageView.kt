package io.github.agastyashenoy.flores

import android.animation.ValueAnimator
import android.annotation.SuppressLint
import android.app.Activity
import android.content.Context
import android.graphics.PixelFormat
import android.os.Build
import android.view.Surface
import android.view.Choreographer
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.animation.DecelerateInterpolator
import android.widget.OverScroller

/**
 * The pages: a surface the engine draws into, and the gestures that move them. Frames are drawn
 * on vsync while something is moving or tiles are on their way, and not at all otherwise.
 */
@SuppressLint("ViewConstructor")
class PageView(context: Context, private val listener: Listener) : SurfaceView(context), SurfaceHolder.Callback, Choreographer.FrameCallback {

    interface Listener {
        /** A tap that hit no link. */
        fun onTap()
        fun onLink(uri: String)
        /** After each frame drawn, to update the page number. */
        fun onFrame()
        /** The surface has its size for the first time: a good moment to restore the position. */
        fun onReady()
    }

    private var handle = 0L
    private var surfaceWidth = 0
    private var surfaceHeight = 0
    private var ready = false
    private var looping = false
    private val scroller = OverScroller(context)
    private var zooming: ValueAnimator? = null

    init {
        holder.addCallback(this)
        holder.setFormat(PixelFormat.RGBX_8888)
    }

    /** Shows the document [h], or nothing for 0. */
    fun attach(h: Long) {
        handle = h
        ready = false
        if (h != 0L && surfaceWidth > 0 && holder.surface.isValid) {
            Native.setSurface(h, holder.surface, surfaceWidth, surfaceHeight)
            becomeReady()
        }
    }

    fun detach() {
        scroller.forceFinished(true)
        zooming?.cancel()
        if (handle != 0L) Native.setSurface(handle, null, 0, 0)
        handle = 0L
    }

    /** Draws again soon: after the view changed or tiles were asked for. */
    fun redraw() {
        if (!looping && handle != 0L) {
            looping = true
            Choreographer.getInstance().postFrameCallback(this)
        }
    }

    fun stopFling() = scroller.forceFinished(true)

    private fun becomeReady() {
        if (!ready) {
            ready = true
            listener.onReady()
        }
        redraw()
    }

    override fun doFrame(frameTimeNanos: Long) {
        looping = false
        val h = handle
        if (h == 0L) return
        var more = false
        if (!scroller.isFinished && scroller.computeScrollOffset()) {
            Native.scrollTo(h, scroller.currX.toFloat(), scroller.currY.toFloat())
            more = true
        }
        if (Native.draw(h)) more = true
        listener.onFrame()
        if (more) redraw()
    }

    override fun surfaceCreated(holder: SurfaceHolder) {}

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, w: Int, h: Int) {
        surfaceWidth = w
        surfaceHeight = h
        // the pages move with the finger: as many frames as the display shows
        if (Build.VERSION.SDK_INT >= 30) {
            val fastest = (context as? Activity)?.fastestMode()?.refreshRate
            if (fastest != null) holder.surface.setFrameRate(fastest, Surface.FRAME_RATE_COMPATIBILITY_DEFAULT)
        }
        if (handle != 0L) {
            Native.setSurface(handle, holder.surface, w, h)
            becomeReady()
        }
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        surfaceWidth = 0
        if (handle != 0L) Native.setSurface(handle, null, 0, 0)
    }

    // Gestures

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent): Boolean {
            scroller.forceFinished(true)
            return true
        }

        override fun onScroll(e1: MotionEvent?, e2: MotionEvent, dx: Float, dy: Float): Boolean {
            if (handle == 0L || scale.isInProgress) return false
            Native.scrollBy(handle, dx, dy)
            redraw()
            return true
        }

        override fun onFling(e1: MotionEvent?, e2: MotionEvent, vx: Float, vy: Float): Boolean {
            if (handle == 0L || scale.isInProgress) return false
            val s = Native.scrollState(handle)
            scroller.fling(s[0].toInt(), s[1].toInt(), -vx.toInt(), -vy.toInt(), 0, s[2].toInt(), s[4].toInt(), s[3].toInt())
            redraw()
            return true
        }

        override fun onSingleTapConfirmed(e: MotionEvent): Boolean {
            if (handle == 0L) return false
            when (val uri = Native.tap(handle, e.x, e.y)) {
                null -> listener.onTap()
                "" -> redraw()
                else -> listener.onLink(uri)
            }
            return true
        }

        override fun onDoubleTap(e: MotionEvent): Boolean {
            if (handle == 0L) return false
            // in to read closer, or back out to the page's width
            val target = if (Native.zoom(handle) < 1.6f) 2.5f else 1f
            animateZoom(target, e.x, e.y)
            return true
        }
    })

    private val scale = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        private var fx = 0f
        private var fy = 0f

        override fun onScaleBegin(d: ScaleGestureDetector): Boolean {
            if (handle == 0L) return false
            zooming?.cancel()
            scroller.forceFinished(true)
            fx = d.focusX
            fy = d.focusY
            Native.setGesture(handle, true)
            return true
        }

        override fun onScale(d: ScaleGestureDetector): Boolean {
            if (handle == 0L) return false
            // the pages follow the fingers as they move apart and along
            Native.scrollBy(handle, fx - d.focusX, fy - d.focusY)
            Native.zoomBy(handle, d.scaleFactor, d.focusX, d.focusY)
            fx = d.focusX
            fy = d.focusY
            redraw()
            return true
        }

        override fun onScaleEnd(d: ScaleGestureDetector) {
            if (handle == 0L) return
            Native.setGesture(handle, false)
            redraw()
        }
    })

    private fun animateZoom(target: Float, x: Float, y: Float) {
        val h = handle
        zooming?.cancel()
        scroller.forceFinished(true)
        val from = Native.zoom(h)
        Native.setGesture(h, true)
        zooming = ValueAnimator.ofFloat(from, target).apply {
            duration = 220
            interpolator = DecelerateInterpolator()
            addUpdateListener {
                if (handle != h) return@addUpdateListener
                Native.zoomBy(h, it.animatedValue as Float / Native.zoom(h), x, y)
                redraw()
            }
            addListener(object : android.animation.AnimatorListenerAdapter() {
                override fun onAnimationEnd(animation: android.animation.Animator) {
                    if (handle != h) return
                    Native.setGesture(h, false)
                    redraw()
                }
            })
            start()
        }
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        scale.onTouchEvent(event)
        gestures.onTouchEvent(event)
        return true
    }
}
