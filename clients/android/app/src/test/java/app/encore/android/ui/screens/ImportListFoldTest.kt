package app.encore.android.ui.screens

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Import playlist → Paste a list folds a long list to ImportListRows lines with Show all / Collapse. The owner
 * (Oct 23): "we need a button to collapse and expand this cause this is a lot to scroll through".
 */
class ImportListFoldTest {
    @Test
    fun countsLinesWithSomethingOnThem() {
        assertEquals(2, listLines("A - B\n\n  \nC - D\n"))
        assertEquals(0, listLines(""))
    }

    @Test
    fun aLongPasteFoldsTheList() {
        assertTrue(isLongPaste("", "x\n".repeat(20)))
        assertFalse(isLongPaste("A - B", "A - B\nC - D"))
        assertFalse("typing one more line keeps it open", isLongPaste("x\n".repeat(20), "x\n".repeat(21)))
    }
}
