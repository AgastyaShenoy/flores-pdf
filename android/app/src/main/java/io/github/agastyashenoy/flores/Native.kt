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

    /** Turns pages [rotation] quarter turns clockwise; [spread] 0 one page to a row, 1 two, 2 a book. */
    @JvmStatic external fun setView(h: Long, rotation: Int, spread: Int)

    /** Selects the word at (x, y); false if there's no text there, or it isn't ready yet. */
    @JvmStatic external fun selectWord(h: Long, x: Float, y: Float): Boolean
    /** Moves the selection's start (0) or end (1) to (x, y); returns the handle now being moved. */
    @JvmStatic external fun selectTo(h: Long, handle: Int, x: Float, y: Float): Int
    @JvmStatic external fun clearSelection(h: Long)
    @JvmStatic external fun selectionText(h: Long): String?
    /** Start x, y, end x, y of the selection's handles in the view; null with no selection. */
    @JvmStatic external fun selectionHandles(h: Long): FloatArray?

    /** Lines of label and value, tab-separated. */
    @JvmStatic external fun info(h: Long): String
    /** Saves the first page, [width] pixels wide, as a PNG at [path], in the background. */
    @JvmStatic external fun saveThumbnail(h: Long, path: String, width: Int)

    // Annotations: made here and drawn over the pages, then written into the PDF by PDFium on saving.

    /** Whether annotations can be made: the document is a PDF. */
    @JvmStatic external fun annotatable(h: Long): Boolean
    /** Highlights (0), underlines (1) or strikes out (2) the selected text in [color], 0xRRGGBB. */
    @JvmStatic external fun markSelection(h: Long, kind: Int, color: Int): Boolean
    @JvmStatic external fun penDown(h: Long, x: Float, y: Float, color: Int, width: Float)
    @JvmStatic external fun penMove(h: Long, x: Float, y: Float)
    @JvmStatic external fun penUp(h: Long)
    @JvmStatic external fun penCancel(h: Long)
    @JvmStatic external fun addNote(h: Long, x: Float, y: Float, text: String, color: Int): Boolean
    @JvmStatic external fun addRect(h: Long, x0: Float, y0: Float, x1: Float, y1: Float, color: Int, width: Float): Boolean
    /** 1 a mark made here, 2 an annotation of the PDF, 0 nothing there, -1 failed. */
    @JvmStatic external fun eraseAt(h: Long, x: Float, y: Float): Int
    @JvmStatic external fun undo(h: Long): Boolean
    /** Whether there are changes not saved. */
    @JvmStatic external fun changed(h: Long): Boolean
    /** Makes the document to save, before its file is opened to write; null, or what went wrong. */
    @JvmStatic external fun savePrepare(h: Long): String?
    /** Writes it into [fd]; [same]: the document's own file. Null, or what went wrong. */
    @JvmStatic external fun saveWrite(h: Long, fd: Int, same: Boolean): String?

    // Forms, filled in through PDFium, as on Windows.

    /** Reads the form's fields, once, before the document shows; how many there are. */
    @JvmStatic external fun loadForm(h: Long): Int
    /** The field at (x, y): index, kind (text, multiline, check, radio, choice), value, checked
     *  (0 or 1), name, then the choices, tab-separated; null where there's none. */
    @JvmStatic external fun fieldAt(h: Long, x: Float, y: Float): String?
    /** Fills in field [index] with [value], or checks it; null, or what went wrong. */
    @JvmStatic external fun setField(h: Long, index: Int, value: String, checked: Boolean): String?

    @JvmStatic external fun search(h: Long, query: String)
    /** Matches found, pages searched, pages, current match (-1 for none). */
    @JvmStatic external fun searchStatus(h: Long): IntArray
    @JvmStatic external fun searchStep(h: Long, dir: Int): Boolean
}
