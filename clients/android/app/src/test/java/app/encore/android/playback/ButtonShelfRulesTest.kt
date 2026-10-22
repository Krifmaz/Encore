package app.encore.android.playback

import org.junit.Assert.assertEquals
import org.junit.Test

/** Settings → Android Auto (ButtonShelfRules): the Shown and Hidden rows of car buttons. */
class ButtonShelfRulesTest {
    private val r = ButtonShelfRules
    private val all = listOf("a", "b", "c", "d", "e")

    @Test fun hideTakesItOff() {
        assertEquals(listOf("a", "c"), r.hide(listOf("a", "b", "c"), "b"))
        assertEquals(listOf("a"), r.hide(listOf("a"), "z"))
    }

    @Test fun showAddsToTheEnd() {
        assertEquals(listOf("a", "c"), r.show(listOf("a"), "c"))
        assertEquals(listOf("a", "c"), r.show(listOf("a", "c"), "a"))
    }

    @Test fun showKeepsTheLimit() {
        assertEquals(listOf("a", "b"), r.show(listOf("a", "b"), "c", max = 2))
        assertEquals(listOf("a", "c"), r.show(listOf("a"), "c", max = 2))
    }

    @Test fun leftAndRight() {
        assertEquals(listOf("b", "a", "c"), r.left(listOf("a", "b", "c"), "b"))
        assertEquals(listOf("a", "b", "c"), r.left(listOf("a", "b", "c"), "a"))
        assertEquals(listOf("a", "c", "b"), r.right(listOf("a", "b", "c"), "b"))
        assertEquals(listOf("a", "b", "c"), r.right(listOf("a", "b", "c"), "c"))
        assertEquals(listOf("a", "b"), r.right(listOf("a", "b"), "z"))
    }

    @Test fun hiddenKeepsTheFullOrder() {
        assertEquals(listOf("a", "c", "e"), r.hidden(all, listOf("d", "b")))
        assertEquals(all, r.hidden(all, emptyList()))
    }

    @Test fun titles() {
        assertEquals("Shown · 4 of 4", r.shownTitle(4, 4))
        assertEquals("Shown · 5", r.shownTitle(5))
    }
}
