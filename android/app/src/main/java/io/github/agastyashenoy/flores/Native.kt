package io.github.agastyashenoy.flores

import android.view.Surface

/**
 * The engine, in libflores.so (crates/android). A document is a handle from [open]; call
 * everything on the UI thread, except [open], which may take a while for a long book.
 */
object Native {
    init {
        System.loadLibrary("flores")
    }

    /** What [open] returns when it fails. A handle is an address, which can look negative: Android
     *  tags the top byte of heap pointers. */
    const val UNSUPPORTED = -1L
    const val NEEDS_PASSWORD = -2L
    const val ENCRYPTED = -3L
    const val DAMAGED = -4L
    const val UNREADABLE = -5L

    fun isHandle(h: Long) = h !in UNREADABLE..0L

    /** Opens the file behind [fd], which must stay open until [close]. [gap] is pixels around pages. */
    @JvmStatic external fun open(fd: Int, password: String, gap: Int): Long
    @JvmStatic external fun close(h: Long)
    @JvmStatic external fun pageCount(h: Long): Int
    @JvmStatic external fun title(h: Long): String?
    /** Lines of depth, page (-1 for none), points down it (-1 for its top) and title, tab-separated. */
    @JvmStatic external fun outline(h: Long): String

    @JvmStatic external fun setSurface(h: Long, surface: Surface?, width: Int, height: Int)
    /** [look]: 0 normal, 1 dark, 2 sepia, 3 black. [background]: 0xRRGGBB around the pages. */
    @JvmStatic external fun setLook(h: Long, look: Int, background: Int)
    /** Draws a frame if anything changed; true while more frames are wanted. */
    @JvmStatic external fun draw(h: Long): Boolean

    @JvmStatic external fun scrollBy(h: Long, dx: Float, dy: Float)
    @JvmStatic external fun scrollTo(h: Long, x: Float, y: Float)
    /** Room the bars take at the top and bottom, in pixels: the pages can scroll clear of them. */
    @JvmStatic external fun setInsets(h: Long, top: Float, bottom: Float)
    /** x, y, the largest x and y, and the smallest y. */
    @JvmStatic external fun scrollState(h: Long): FloatArray
    @JvmStatic external fun zoomBy(h: Long, factor: Float, fx: Float, fy: Float)
    /** 1 is the page width fitting the screen. */
    @JvmStatic external fun zoom(h: Long): Float
    @JvmStatic external fun setGesture(h: Long, on: Boolean)

    @JvmStatic external fun currentPage(h: Long): Int
    @JvmStatic external fun goToPage(h: Long, page: Int)
    /** Shows [page] from [y] points down it, or from its top when [y] is negative. */
    @JvmStatic external fun goTo(h: Long, page: Int, y: Float)
    /** Page at the top, points down it, zoom: what [restore] takes. */
    @JvmStatic external fun position(h: Long): FloatArray
    @JvmStatic external fun restore(h: Long, page: Int, offset: Float, zoom: Float)

    /** null: no link there; "": followed a link in the document; else a link's address. */
    @JvmStatic external fun tap(h: Long, x: Float, y: Float): String?
    @JvmStatic external fun back(h: Long): Boolean

    @JvmStatic external fun search(h: Long, query: String)
    /** Matches found, pages searched, pages, current match (-1 for none). */
    @JvmStatic external fun searchStatus(h: Long): IntArray
    @JvmStatic external fun searchStep(h: Long, dir: Int): Boolean
}
