package app.encore.android.ui.screens

import org.junit.Assert.assertEquals
import org.junit.Test

/** Every credited artist of a song gets its own link, found the way the server matched the credit to the text. */
class CreditSpansTest {
    /** The text each found range covers, in text order, with the credit it belongs to. */
    private fun shown(text: String, names: List<String>): List<Pair<String, String>> =
        CreditSpans.find(text, names).map { (k, r) -> names[k] to text.substring(r.first, r.last + 1) }

    @Test
    fun exactNamesInOrderAreFound() {
        val text = "Snoop Dogg, Dr. Dre, Tom Petty & Jelly Roll"
        val names = listOf("Snoop Dogg", "Dr. Dre", "Tom Petty", "Jelly Roll")
        assertEquals(names.map { it to it }, shown(text, names))
    }

    @Test
    fun aCreditListedOutOfTextOrderIsStillLinked() {
        // the guest comes last in the credit but second in the text: it used to be skipped and stay grey
        val text = "Snoop Dogg, Dr. Dre, Tom Petty & Jelly Roll"
        val names = listOf("Snoop Dogg", "Tom Petty", "Jelly Roll", "Dr. Dre")
        assertEquals(
            listOf("Snoop Dogg" to "Snoop Dogg", "Dr. Dre" to "Dr. Dre", "Tom Petty" to "Tom Petty", "Jelly Roll" to "Jelly Roll"),
            shown(text, names),
        )
    }

    @Test
    fun aCreditSpelledDifferentlyMatchesTheTextsOwnSpelling() {
        // MusicBrainz / the tags say "Dr Dre", the text says "Dr. Dre": the link covers the text's spelling
        assertEquals(listOf("Dr Dre" to "Dr. Dre"), shown("Snoop Dogg · Dr. Dre", listOf("Dr Dre")))
        assertEquals(listOf("beyonce" to "Beyoncé"), shown("Jay-Z & Beyoncé", listOf("beyonce")))
        assertEquals(listOf("Guns and Roses" to "Guns & Roses"), shown("Guns & Roses", listOf("Guns and Roses")))
    }

    @Test
    fun aNameIsMatchedAsWholeWordsOnly() {
        // "Dre" is no part of "Dreams": nothing to link
        assertEquals(emptyList<Pair<String, String>>(), shown("Sweet Dreams", listOf("Dre")))
    }

    @Test
    fun twoCreditsNeverShareText() {
        // the same name twice links its two places, not one place twice
        assertEquals(listOf("A" to "A", "A" to "A"), shown("A & A", listOf("A", "A")))
    }

    @Test
    fun foldIsTheServersFold() {
        assertEquals("dr dre", CreditSpans.fold("Dr. Dre"))
        assertEquals("simon and garfunkel", CreditSpans.fold("Simon & Garfunkel"))
        assertEquals("guns n roses", CreditSpans.fold("Guns N' Roses"))
        assertEquals("motorhead", CreditSpans.fold("  Motörhead!! "))
    }
}
