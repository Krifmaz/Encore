package app.encore.android.ui.components

import org.junit.Assert.assertEquals
import org.junit.Test

/** [ListKeys]: unique keys for lazy lists, and the play queue's per-entry keys across edits. */
class ListKeysTest {
    /** Repeats are numbered in list order; the first keeps the plain key. */
    @Test
    fun uniqueNumbersRepeats() {
        assertEquals(listOf("a", "b", "a#2", "a#3"), ListKeys.unique(listOf("a", "b", "a", "a")) { it })
    }

    /** A counter for fresh keys, starting at 100 so they can't be mistaken for old ones. */
    private fun counter(): () -> Long {
        var n = 100L
        return { n++ }
    }

    /** The first list: every entry gets a fresh key. */
    @Test
    fun aNewQueueGetsNewKeys() {
        assertEquals(listOf(100L, 101L, 102L), ListKeys.carry(emptyList(), emptyList(), listOf(7, 8, 7), counter()))
    }

    /** Removing the first of a song queued twice keeps the second one's key (not the removed one's). */
    @Test
    fun removingADuplicateKeepsTheOtherKey() {
        val keys = ListKeys.carry(listOf(5, 7, 8, 7), listOf(1, 2, 3, 4), listOf(5, 8, 7), counter())
        assertEquals(listOf(1L, 3L, 4L), keys)
    }

    /** A song moved to another place keeps its key; the others keep theirs. */
    @Test
    fun aMoveKeepsEveryKey() {
        val keys = ListKeys.carry(listOf(1, 2, 3, 4), listOf(10, 20, 30, 40), listOf(1, 3, 4, 2), counter())
        assertEquals(listOf(10L, 30L, 40L, 20L), keys)
    }

    /** Songs added at the end get new keys; the ones before keep theirs. */
    @Test
    fun addedAtTheEnd() {
        val keys = ListKeys.carry(listOf(1, 2), listOf(10, 20), listOf(1, 2, 2), counter())
        assertEquals(listOf(10L, 20L, 100L), keys)
    }

    /** Keys stay unique whatever the edit. */
    @Test
    fun keysAreUnique() {
        val keys = ListKeys.carry(listOf(1, 1, 1), listOf(10, 20, 30), listOf(1, 1, 1, 1, 1), counter())
        assertEquals(keys.size, keys.toSet().size)
    }
}
