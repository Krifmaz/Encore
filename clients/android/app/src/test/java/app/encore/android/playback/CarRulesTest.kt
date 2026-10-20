package app.encore.android.playback

import androidx.media3.common.Player
import androidx.media3.session.CommandButton
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The car's rules (CarRules): the now-playing buttons, the subtitle line, the "More by <artist>" node and how big
 * a lyric line is drawn.
 */
class CarRulesTest {
    // ---- buttons --------------------------------------------------------------------------------------------------

    /**
     * Media3 1.11's placement for the platform session (CommandButton.getCustomLayoutFromMediaButtonPreferences):
     * the first button that may take the back slot, then the first that may take the forward slot, then every
     * overflow button in list order. Each button here carries its slots.
     */
    private fun platformOrder(buttons: List<Pair<String, IntArray>>): List<String> {
        val back = buttons.indexOfFirst { (_, slots) -> slots.firstOrNull { it != CommandButton.SLOT_OVERFLOW } == CommandButton.SLOT_BACK }
        val fwd = buttons.indexOfFirst { (_, slots) -> slots.firstOrNull { it != CommandButton.SLOT_OVERFLOW } == CommandButton.SLOT_FORWARD }
        val out = ArrayList<String>()
        if (back >= 0) out += buttons[back].first
        if (fwd >= 0) out += buttons[fwd].first
        buttons.forEachIndexed { i, (name, slots) -> if (i != back && i != fwd && CommandButton.SLOT_OVERFLOW in slots) out += name }
        return out
    }

    @Test
    fun theCarGetsShuffleRepeatLikeLyricsAndNothingElse() {
        val car = CarRules.buttons(car = true, liked = false, shuffled = false, repeatMode = Player.REPEAT_MODE_OFF, lyricsOn = false)
        assertEquals(
            listOf(CarControls.CMD_SHUFFLE, CarControls.CMD_REPEAT, CarControls.CMD_LIKE, CarControls.CMD_LYRICS),
            car.map { it.command },
        )
        // four: what Android Auto's now-playing row shows beside Play/Pause, Previous and Next without its "⋯"
        assertEquals(4, car.size)
        // overflow only: none of them replaces Previous or Next, and the platform's custom actions keep this order
        assertArrayEquals(intArrayOf(CommandButton.SLOT_OVERFLOW), CarRules.SLOTS)
        assertEquals(car.map { it.command }, platformOrder(car.map { it.command to CarRules.SLOTS }))
        // the phone keeps Like first (the system media controls show the first two)
        val phone = CarRules.buttons(car = false, liked = false, shuffled = false, repeatMode = Player.REPEAT_MODE_OFF, lyricsOn = false)
        assertEquals(
            listOf(CarControls.CMD_LIKE, CarControls.CMD_SHUFFLE, CarControls.CMD_REPEAT, CarControls.CMD_LYRICS),
            phone.map { it.command },
        )
        // the commands the session hands out are exactly these
        assertEquals(car.map { it.command }.toSet(), CarControls.COMMANDS.toSet())
    }

    @Test
    fun buttonsFollowTheirState() {
        val off = CarRules.buttons(true, liked = false, shuffled = false, repeatMode = Player.REPEAT_MODE_OFF, lyricsOn = false).associateBy { it.command }
        val on = CarRules.buttons(true, liked = true, shuffled = true, repeatMode = Player.REPEAT_MODE_ONE, lyricsOn = true).associateBy { it.command }
        assertEquals(CommandButton.ICON_HEART_UNFILLED, off.getValue(CarControls.CMD_LIKE).icon)
        assertEquals(CommandButton.ICON_HEART_FILLED, on.getValue(CarControls.CMD_LIKE).icon)
        assertEquals("Unlike", on.getValue(CarControls.CMD_LIKE).name)
        assertEquals(CommandButton.ICON_SHUFFLE_OFF, off.getValue(CarControls.CMD_SHUFFLE).icon)
        assertEquals(CommandButton.ICON_SHUFFLE_ON, on.getValue(CarControls.CMD_SHUFFLE).icon)
        assertEquals(CommandButton.ICON_REPEAT_OFF, off.getValue(CarControls.CMD_REPEAT).icon)
        assertEquals(CommandButton.ICON_REPEAT_ONE, on.getValue(CarControls.CMD_REPEAT).icon)
        assertEquals(CommandButton.ICON_REPEAT_ALL, CarRules.buttons(true, false, false, Player.REPEAT_MODE_ALL, false)[1].icon)
        assertEquals("Live lyrics on", on.getValue(CarControls.CMD_LYRICS).name)
        assertTrue(on.getValue(CarControls.CMD_LYRICS).customIcon != off.getValue(CarControls.CMD_LYRICS).customIcon)
        // no button has an icon Media3 would place in the back or forward slot on its own
        val moving = setOf(CommandButton.ICON_PREVIOUS, CommandButton.ICON_NEXT, CommandButton.ICON_SKIP_BACK, CommandButton.ICON_SKIP_FORWARD, CommandButton.ICON_REWIND, CommandButton.ICON_FAST_FORWARD)
        assertTrue((off.values + on.values).none { it.icon in moving })
    }

    // ---- the subtitle line -------------------------------------------------------------------------------------------

    @Test
    fun theSubtitleCarriesTheQuality() {
        assertEquals("DROELOE · FLAC 24/96", CarRules.subtitle("DROELOE", "FLAC 24/96", group = false))
        assertEquals("DROELOE", CarRules.subtitle("DROELOE", null, group = false))
        assertEquals("DROELOE", CarRules.subtitle("DROELOE", "", group = false))
        assertEquals("AAC 256", CarRules.subtitle("", "AAC 256", group = false))
        assertEquals("Group listen · DROELOE · FLAC → Cadenza 192 kHz", CarRules.subtitle("DROELOE", "FLAC → Cadenza 192 kHz", group = true))
        assertEquals("Group listen", CarRules.subtitle("", null, group = true))
        // with the lyric line as the title: the song first, the quality last (a short car line loses the quality first)
        assertEquals("Gravity · DROELOE · FLAC 24/96", CarRules.lineSubtitle("Gravity", "DROELOE", "FLAC 24/96", group = false))
        assertEquals("Gravity · FLAC 24/96", CarRules.lineSubtitle("Gravity", "", "FLAC 24/96", group = false))
    }

    @Test
    fun theQualityDetailReadsLikeTheSignalPath() {
        fun d(format: String, bitrate: Int = 0, rate: Int = 0, bits: Int = 0, kind: String = "", phoneRate: Int = 0, local: String = "") =
            CarQuality.of(format, bitrate, rate, bits, kind, phoneRate, local)?.detail
        assertEquals("FLAC 24/96", d("flac", 2800, 96000, 24, "original"))
        assertEquals("AAC 256", d("m4a", 256, 44100))
        assertEquals("MP3 320", d("mp3", 320, 44100))
        assertEquals("FLAC → AAC 96", d("flac", 900, 44100, 16, "aac-96"))
        assertEquals("FLAC → Cadenza 192 kHz", d("flac", 900, 44100, 16, "cadenza-192000"))
        assertEquals("FLAC → Cadenza 192 kHz", d("flac", 900, 44100, 16, "original", phoneRate = 192000))
        assertEquals("AAC 256", d("flac", kind = "local", local = "AAC 256k"))
        assertEquals("Cadenza 96 kHz", d("", kind = "cadenza-96000"))
    }

    // ---- More by <artist> ---------------------------------------------------------------------------------------------

    @Test
    fun moreByListsShufflePopularAlbumsLiked() {
        val mbid = "4a4ee089-93b1-4470-af9a-6ff575d32704"
        val node = ArtistNode("DROELOE", mbid)
        assertEquals("more-by:$mbid", CarRules.MORE_BY + node.key)
        assertEquals("artist:$mbid", node.id)
        assertEquals("More by DROELOE", CarRules.moreByTitle(node.name))
        val rows = CarRules.moreByRows(node.name, mbid, node.key, popular = 9, albums = 3, liked = 1)
        assertEquals(listOf("Shuffle DROELOE", "Popular songs", "Albums", "Liked from this artist"), rows.map { it.title })
        assertEquals(
            listOf("artist-shuffle:DROELOE", "artist-popular:$mbid", "artist-albums:$mbid", "artist-liked:$mbid"),
            rows.map { it.id },
        )
        assertEquals(listOf(CarRules.Kind.PLAY, CarRules.Kind.FOLDER, CarRules.Kind.FOLDER, CarRules.Kind.FOLDER), rows.map { it.kind })
        assertEquals(listOf(null, "9 songs", "3 albums", "1 song"), rows.map { it.subtitle })
        // without an mbid there are no popular songs; rows with nothing behind them are left out
        val byName = ArtistNode("Some Band", "")
        assertEquals("more-by:Some Band", CarRules.MORE_BY + byName.key)
        assertEquals(
            listOf("Shuffle Some Band", "Albums"),
            CarRules.moreByRows(byName.name, "", byName.key, popular = 5, albums = 2, liked = 0).map { it.title },
        )
        assertEquals(emptyList<CarRules.Row>(), CarRules.moreByRows("", "", "", 1, 1, 1))
    }

    @Test
    fun moreByInEncoreLyricsSaysWhatItDid() {
        assertEquals("Added 25 songs by DROELOE after this song", CarRules.moreByMessage("DROELOE", 25, group = false))
        assertEquals("Added 1 song by DROELOE to the group's Up next", CarRules.moreByMessage("DROELOE", 1, group = true))
        assertEquals("No other songs by DROELOE in your library", CarRules.moreByMessage("DROELOE", 0, group = false))
        // a failed fetch names neither the phone nor the server: either can be the cause
        assertEquals("Couldn't load songs by DROELOE. Try again in a moment.", CarRules.moreByFailed("DROELOE"))
    }

    // ---- text as large as it fits -------------------------------------------------------------------------------------

    /** A monospace stand-in for Paint.measureText: every character half the text size wide. */
    private val mono: (String, Float) -> Float = { s, size -> s.length * size * 0.5f }

    @Test
    fun aShortLineGetsTheLargestSize() {
        val f = CarRules.fit("Hello", 896f, 900f, 56f, 168f, 1.12f, measure = mono)
        assertEquals(168f, f.size)
        assertEquals(listOf("Hello"), f.lines)
    }

    @Test
    fun aLongLineWrapsAndShrinksUntilItFits() {
        val text = "I have been running through the jungle all night long and I still cannot find my way home"
        val w = 896f
        val hgt = 600f
        val f = CarRules.fit(text, w, hgt, 56f, 168f, 1.12f, measure = mono)
        assertTrue("size ${f.size}", f.size in 56f..168f && f.size < 168f)
        assertTrue(f.lines.all { mono(it, f.size) <= w })
        assertTrue(f.height(1.12f) <= hgt)
        // nothing lost, no word broken
        assertEquals(text.split(" "), f.lines.flatMap { it.split(" ") })
        // one pixel larger would not fit any more
        val bigger = CarRules.wrap(text, w, f.size + 1f, mono)
        assertTrue(bigger.size * (f.size + 1f) * 1.12f > hgt)
    }

    @Test
    fun wrappedLinesComeOutBalanced() {
        // greedy wrapping would give "one two three four five six" and a lonely "seven"
        val f = CarRules.fit("one two three four five six seven", 600f, 1000f, 40f, 40f, 1.1f, measure = mono)
        assertEquals(listOf("one two three four", "five six seven"), f.lines)
    }

    @Test
    fun aWordWiderThanTheLineBreaksBetweenCharacters() {
        val text = "あいうえおかきくけこさしすせそたちつてとなにぬねの"
        val f = CarRules.fit(text, 400f, 2000f, 40f, 40f, 1.1f, measure = mono)
        assertEquals(text, f.lines.joinToString(""))
        assertTrue(f.lines.size > 1)
        assertTrue(f.lines.all { mono(it, 40f) <= 400f })
    }

    @Test
    fun textThatNeverFitsEndsInAnEllipsis() {
        val text = (1..60).joinToString(" ") { "word$it" }
        val f = CarRules.fit(text, 300f, 200f, 40f, 80f, 1.1f, maxLines = 3, measure = mono)
        assertEquals(40f, f.size)
        assertTrue(f.lines.size <= 3)
        assertTrue(f.lines.last().endsWith("…"))
        assertTrue(f.lines.all { mono(it, 40f) <= 300f })
        assertEquals("♪", CarRules.shown("  "))
        assertEquals("♪", CarRules.shown(null))
    }

    @Test
    fun theCardShowsTheCurrentLineLargeAndTheNextBelow() {
        val c = CarRules.card("Feel the gravity pull me down", "Into the light again", mono, mono)
        val size = CarRules.CARD_SIZE.toFloat()
        val next = c.next!!
        assertTrue(c.current.size > next.size)
        assertTrue(next.lines.size <= 2)
        assertTrue(c.current.lines.all { mono(it, c.current.size) <= size - 2 * CarRules.CARD_MARGIN })
        // the next line below the current one, the pair centred and inside the card
        assertEquals(c.currentTop + c.current.height(CarRules.CARD_LEADING) + CarRules.CARD_GAP, c.nextTop)
        val bottom = c.nextTop + next.height(CarRules.CARD_LEADING)
        assertTrue(c.currentTop >= CarRules.CARD_MARGIN * 0.5f && bottom <= size - CarRules.CARD_MARGIN * 0.5f)
        assertEquals(c.currentTop, size - bottom, 1f)
        // a short last line: as large as allowed
        val last = CarRules.card("Home", null, mono, mono)
        assertEquals(CarRules.CARD_CURRENT_MAX, last.current.size)
        assertEquals(null, last.next)
        // before the first line: a note, the first line below it
        assertEquals(listOf("♪"), CarRules.card("", "First words", mono, mono).current.lines)
    }

    @Test
    fun theSurfaceGivesTheCurrentLineMostOfTheRoom() {
        val lines = listOf("One", "Two lines of lyrics", "The line that plays now", "The next line", "After that")
        val w = 1250f
        val hgt = 620f
        val blocks = CarRules.surface(lines, 2, w, hgt, 1.5f, mono, mono)
        val cur = blocks.single { it.current }
        val next = blocks.single { it.index == 3 }
        assertEquals(2, cur.index)
        assertEquals(1f, cur.alpha)
        assertTrue(cur.fit.size <= hgt * 0.2f && cur.fit.size >= hgt * 0.15f)
        assertTrue(next.fit.size < cur.fit.size)
        assertEquals(cur.top + cur.fit.height(CarRules.SURFACE_LEADING) + next.fit.size * 0.6f, next.top, next.fit.size * 0.3f)
        // the lines before: above, dimmer the further back
        val past = blocks.filter { it.index < 2 }
        assertTrue(past.isNotEmpty())
        assertTrue(past.all { it.top + it.fit.height(CarRules.SURFACE_LEADING) <= cur.top && it.alpha < next.alpha })
        // everything inside the area
        assertTrue(blocks.all { it.top >= 0f && it.top + it.fit.height(CarRules.SURFACE_LEADING) <= hgt })
        // before the first line: a note, the first line below it
        val before = CarRules.surface(lines, -1, w, hgt, 1.5f, mono, mono)
        assertEquals(listOf("♪"), before.single { it.current }.fit.lines)
        assertEquals(0, before.single { !it.current }.index)
        // the old surface's size was the area's height / 9, at most 40 dp; now at least twice that here
        assertTrue(cur.fit.size >= 2 * minOf(hgt / 9f, 40f * 1.5f))
    }
}
