package app.encore.android.ui.components

/**
 * One toast on screen. [id] names it (its Close, its place in the list); [seq] goes up each time its message
 * arrives, so a repeat moves it to the newest place and starts its time again. [action] is a button on it ("Undo")
 * that runs [onAction] and closes it; [showMs] how long it stays when it is not an error.
 */
data class ToastItem(
    val id: Long,
    val seq: Long,
    val message: String,
    val kind: String,
    val action: String = "",
    val onAction: (() -> Unit)? = null,
    val showMs: Long = ToastRules.SHOW_MS,
) {
    /** An error that stays until its Close, with Copy. */
    val stays: Boolean get() = ToastRules.stays(message, kind)
}

/** One of the app's windows, in the order they were added, for [ToastRules.covered]. */
data class AppWindow(val type: Int, val shown: Boolean, val toasts: Boolean = false)

/**
 * Which toasts stay and how they stack (Toasts.kt draws them), as the web's `toast` (web-rs/src/ui.rs):
 * - an error ("err") stays until its Close and has Copy; "Copy failed – select and copy manually" goes like any
 *   other toast, as on the web;
 * - every other toast goes after [SHOW_MS];
 * - a new toast goes below the ones on screen (the newest nearest the bottom) and hides none of them: errors pile up
 *   to [MAX_ERRORS] (only a burst of more errors than that takes the oldest away; the stack scrolls), other toasts
 *   to [MAX_OTHERS];
 * - the same message again does not pile up: the toast on screen moves to the newest place and its time starts
 *   again.
 * The owner (Oct 20): "Errors show behind all other screens [...] making it really hard to copy the error before it
 * disappears."
 */
object ToastRules {
    /** The words when the clipboard refuses (that toast says what to do instead, so it has no Copy). */
    const val COPY_FAILED = "Copy failed – select and copy manually"

    /** How long a toast that is not an error stays (the web's 3.8 s plus its fade). */
    const val SHOW_MS = 4_000L

    /**
     * Errors on screen at once. Errors stay until their Close (the owner's ask), so this is only a guard against a
     * runaway burst of different failures: the web's `ERROR_TOASTS` is 3, but here a fourth error would take the
     * first away before it could be copied. The stack scrolls past 60 % of the screen (Toasts.kt).
     */
    const val MAX_ERRORS = 10

    /** Other toasts on screen at once. */
    const val MAX_OTHERS = 3

    /** Room between the keyboard and the toasts, in dp. */
    const val KEYBOARD_GAP = 8f

    // WindowManager.LayoutParams: TYPE_BASE_APPLICATION (an activity's own window) and LAST_APPLICATION_WINDOW;
    // the types between are an activity's dialogs and sheets (TYPE_APPLICATION), stacked in the order they were added
    private const val TYPE_BASE_APPLICATION = 1
    private const val LAST_APPLICATION_WINDOW = 99

    fun stays(message: String, kind: String): Boolean = kind == "err" && message != COPY_FAILED

    /**
     * The toasts after [message] arrived ([id] and [seq] are new numbers; a repeat keeps its id, and takes the new
     * [action], so an "Undo" undoes the latest).
     */
    fun add(
        stack: List<ToastItem>,
        id: Long,
        seq: Long,
        message: String,
        kind: String,
        action: String = "",
        onAction: (() -> Unit)? = null,
        showMs: Long = SHOW_MS,
    ): List<ToastItem> {
        if (message.isBlank()) return stack
        val same = stack.firstOrNull { it.message == message && it.kind == kind }
        val item = ToastItem(same?.id ?: id, seq, message, kind, if (onAction != null) action else "", onAction, showMs)
        val out = stack.filter { it.id != same?.id }.toMutableList()
        out += item
        val max = if (item.stays) MAX_ERRORS else MAX_OTHERS
        while (out.count { it.stays == item.stays } > max) out.remove(out.first { it.stays == item.stays })
        return out
    }

    /** The toasts after a Close. */
    fun close(stack: List<ToastItem>, id: Long): List<ToastItem> = stack.filter { it.id != id }

    /**
     * The toasts when the time of toast [id] as it arrived the [seq]th time is up: it goes unless it is an error or
     * came again since (a new time started then).
     */
    fun expire(stack: List<ToastItem>, id: Long, seq: Long): List<ToastItem> =
        stack.filter { it.id != id || it.seq != seq || it.stays }

    /** The newest arrival on screen (0 when none): when it goes up, the toasts' window is put back on top. */
    fun newest(stack: List<ToastItem>): Long = stack.maxOfOrNull { it.seq } ?: 0L

    /**
     * True when a sheet or dialog that is showing was added after the toasts' window: an activity's windows are
     * stacked in the order they were added, so it covers the toasts, which then have to be added again on top.
     * [windows] are the app's windows, oldest first. Popups and menus are sub-windows that sit with their own window,
     * and another activity's window means this one is going away, so neither counts.
     */
    fun covered(windows: List<AppWindow>): Boolean {
        val i = windows.indexOfFirst { it.toasts }
        if (i < 0) return false
        return windows.drop(i + 1).any { it.shown && it.type > TYPE_BASE_APPLICATION && it.type <= LAST_APPLICATION_WINDOW }
    }

    /**
     * How high the toasts sit, in dp above the navigation bar: [base] (above the mini player and the tabs), or just
     * above the keyboard while it is open. [imeDp] and [navDp] are the keyboard's and the navigation bar's bottom
     * insets (the keyboard's counts from the bottom of the screen, the navigation bar included).
     */
    fun lift(base: Float, imeDp: Float, navDp: Float): Float =
        if (imeDp <= navDp) base else maxOf(base, imeDp - navDp + KEYBOARD_GAP)

    /**
     * How far below the top of the screen the toasts sit when they are at the top (over the full player, whose seek bar
     * and buttons fill the bottom), in dp: below the status bar ([statusDp], its top inset) and the [header] under it
     * (the player's ⌄ / "Now playing" row), so neither is covered.
     */
    fun drop(header: Float, statusDp: Float): Float = maxOf(0f, statusDp) + maxOf(0f, header)

    /**
     * The toasts in the order they are drawn, top to bottom. At the bottom the newest is nearest the bottom (the
     * stack's own order, oldest first); at the [top] the newest is nearest the top, so a new toast always appears at
     * the edge the toasts come from and pushes older ones away from it.
     */
    fun drawOrder(stack: List<ToastItem>, top: Boolean): List<ToastItem> = if (top) stack.asReversed() else stack
}
