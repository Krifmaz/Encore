package app.encore.android.ui.components

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Which toasts stay and how they stack (ToastRules), and when the toasts' window has to go back on top of a sheet or
 * dialog. The owner (Oct 20): "Errors show behind all other screens when they need to appear above all other screens,
 * making it really hard to copy the error before it disappears."
 */
class ToastRulesTest {
    private var n = 0L

    private fun add(stack: List<ToastItem>, message: String, kind: String = "info"): List<ToastItem> {
        n++
        return ToastRules.add(stack, n, n, message, kind)
    }

    private fun messages(stack: List<ToastItem>) = stack.map { it.message }

    @Test
    fun errorsStayAndOtherToastsGo() {
        assertTrue(ToastRules.stays("Storage: os error 28 writing /music/a.flac", "err"))
        assertFalse(ToastRules.stays("Added to queue", "ok"))
        assertFalse(ToastRules.stays("Saved", "info"))
        assertFalse(ToastRules.stays("Server is slow", "warn"))
        // the clipboard refused: that toast goes like any other and has no Copy (as on the web)
        assertFalse(ToastRules.stays(ToastRules.COPY_FAILED, "err"))
        assertEquals(ToastRules.COPY_FAILED, Toasts.COPY_FAILED)
    }

    @Test
    fun aNewErrorStacksWithoutHidingTheOldOne() {
        var s = add(emptyList(), "First failed", "err")
        s = add(s, "Second failed", "err")
        // the newest nearest the bottom (last), the first still there
        assertEquals(listOf("First failed", "Second failed"), messages(s))
        assertTrue(s.all { it.stays })
        // an ordinary toast does not hide them either
        s = add(s, "Copied to clipboard", "ok")
        assertEquals(listOf("First failed", "Second failed", "Copied to clipboard"), messages(s))
    }

    @Test
    fun theTimeRunsOutOnlyForToastsThatAreNotErrors() {
        var s = add(emptyList(), "Failed", "err")
        val err = s.single()
        s = add(s, "Added to queue", "ok")
        val ok = s.last()
        s = ToastRules.expire(s, ok.id, ok.seq)
        assertEquals(listOf("Failed"), messages(s))
        // an error stays when its time would be up
        s = ToastRules.expire(s, err.id, err.seq)
        assertEquals(listOf("Failed"), messages(s))
        // until its Close
        s = ToastRules.close(s, err.id)
        assertTrue(s.isEmpty())
    }

    @Test
    fun theSameMessageAgainDoesNotPileUp() {
        var s = add(emptyList(), "Failed", "err")
        val first = s.single()
        s = add(s, "Other", "err")
        s = add(s, "Failed", "err")
        // one "Failed", moved to the newest place, same id (its Close still works), a new seq (on top again)
        assertEquals(listOf("Other", "Failed"), messages(s))
        assertEquals(first.id, s.last().id)
        assertTrue(s.last().seq > first.seq)
        assertEquals(s.last().seq, ToastRules.newest(s))
    }

    @Test
    fun aRepeatedToastStartsItsTimeAgain() {
        var s = add(emptyList(), "Plays next", "ok")
        val old = s.single()
        s = add(s, "Plays next", "ok")
        // the first time's timer ends: the toast stays, it came again since
        s = ToastRules.expire(s, old.id, old.seq)
        assertEquals(listOf("Plays next"), messages(s))
        val again = s.single()
        s = ToastRules.expire(s, again.id, again.seq)
        assertTrue(s.isEmpty())
    }

    @Test
    fun errorsStayInABurstAndOthersAreCapped() {
        var s = emptyList<ToastItem>()
        for (i in 1..4) s = add(s, "Error $i", "err")
        // a fourth error doesn't take the first away: errors stay until their Close
        assertEquals(listOf("Error 1", "Error 2", "Error 3", "Error 4"), messages(s))
        for (i in 1..5) s = add(s, "Note $i")
        // other toasts never take an error's place, and keep to their own cap
        assertEquals(listOf("Error 1", "Error 2", "Error 3", "Error 4", "Note 3", "Note 4", "Note 5"), messages(s))
        assertEquals(ToastRules.MAX_OTHERS, s.count { !it.stays })
    }

    @Test
    fun onlyARunawayBurstOfErrorsTakesTheOldestAway() {
        var s = emptyList<ToastItem>()
        for (i in 1..ToastRules.MAX_ERRORS + 1) s = add(s, "Error $i", "err")
        assertEquals(ToastRules.MAX_ERRORS, s.size)
        assertEquals("Error 2", s.first().message)
        assertEquals("Error ${ToastRules.MAX_ERRORS + 1}", s.last().message)
    }

    @Test
    fun blankMessagesAreNotShown() {
        assertTrue(add(emptyList(), "  ", "err").isEmpty())
    }

    @Test
    fun newestIsZeroWithNoToasts() {
        assertEquals(0L, ToastRules.newest(emptyList()))
    }

    // WindowManager.LayoutParams types: an activity's own window, a dialog or sheet, a popup or menu
    private val activity = 1
    private val dialog = 2
    private val popup = 1000

    @Test
    fun aSheetOrDialogOpenedAfterTheToastsCoversThem() {
        // a sheet open before the toasts' window was added: the toasts are on top
        assertFalse(ToastRules.covered(listOf(AppWindow(activity, true), AppWindow(dialog, true), AppWindow(dialog, true, toasts = true))))
        // a dialog opened from the sheet afterwards covers them
        assertTrue(ToastRules.covered(listOf(AppWindow(activity, true), AppWindow(dialog, true, toasts = true), AppWindow(dialog, true))))
        // a hidden dialog, a menu (a sub-window of its own window) or another activity's window does not
        assertFalse(ToastRules.covered(listOf(AppWindow(activity, true), AppWindow(dialog, true, toasts = true), AppWindow(dialog, false))))
        assertFalse(ToastRules.covered(listOf(AppWindow(activity, true), AppWindow(dialog, true, toasts = true), AppWindow(popup, true))))
        assertFalse(ToastRules.covered(listOf(AppWindow(activity, true), AppWindow(dialog, true, toasts = true), AppWindow(activity, true))))
        // no toasts' window: nothing to put back
        assertFalse(ToastRules.covered(listOf(AppWindow(activity, true), AppWindow(dialog, true))))
    }

    @Test
    fun theToastsSitAboveTheKeyboardWhileItIsOpen() {
        // no keyboard: above the mini player and the tabs
        assertEquals(150f, ToastRules.lift(150f, 0f, 48f), 0.01f)
        assertEquals(150f, ToastRules.lift(150f, 48f, 48f), 0.01f)
        // a 300 dp keyboard over a 48 dp navigation bar: just above it
        assertEquals(300f - 48f + ToastRules.KEYBOARD_GAP, ToastRules.lift(150f, 300f, 48f), 0.01f)
        // a small keyboard below the usual place does not bring the toasts down
        assertEquals(150f, ToastRules.lift(150f, 120f, 48f), 0.01f)
    }

    /**
     * Over the full player the toasts sit at the top, below the status bar and the player's header, so they cover
     * neither the seek bar and buttons at the bottom nor the ⌄ and Up next at the top. The owner (Oct 26): a
     * "Got it – you won't hear this song…" toast sat over the seek bar and the times.
     */
    @Test
    fun overThePlayerTheToastsSitBelowTheStatusBarAndHeader() {
        assertEquals(24f + 56f, ToastRules.drop(56f, 24f), 0.01f)
        // no status bar (a TV, a full-screen window): just below the header
        assertEquals(68f, ToastRules.drop(68f, 0f), 0.01f)
        // a nonsense negative inset never pulls them up over the status bar
        assertEquals(56f, ToastRules.drop(56f, -10f), 0.01f)
    }

    /** At the bottom the newest is nearest the bottom; at the top it is nearest the top. */
    @Test
    fun theNewestToastIsNearestTheEdgeTheToastsComeFrom() {
        var stack = add(emptyList(), "Added to queue")
        stack = add(stack, "Saved")
        assertEquals(listOf("Added to queue", "Saved"), messages(ToastRules.drawOrder(stack, top = false)))
        assertEquals(listOf("Saved", "Added to queue"), messages(ToastRules.drawOrder(stack, top = true)))
    }

    @Test
    fun anUndoToastHasItsButtonAndItsOwnTime() {
        var undone = 0
        var s = ToastRules.add(emptyList(), 1, 1, "Up next cleared", "ok", action = "Undo", onAction = { undone++ }, showMs = 6000L)
        val t = s.single()
        assertEquals("Undo", t.action)
        assertEquals(6000L, t.showMs)
        assertFalse(t.stays)
        t.onAction!!()
        assertEquals(1, undone)
        // cleared again before it went: one toast, whose Undo undoes the latest
        s = ToastRules.add(s, 2, 2, "Up next cleared", "ok", action = "Undo", onAction = { undone += 10 }, showMs = 6000L)
        assertEquals(1, s.size)
        assertEquals(1L, s.single().id)
        s.single().onAction!!()
        assertEquals(11, undone)
        // a toast without one: no button, the usual time
        val plain = ToastRules.add(emptyList(), 3, 3, "Saved", "info", action = "Undo").single()
        assertEquals("", plain.action)
        assertEquals(ToastRules.SHOW_MS, plain.showMs)
    }
}
