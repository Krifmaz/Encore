package app.encore.android.playback

import app.encore.android.playback.CarButtonRules.Group
import app.encore.android.playback.CarButtonRules.Id.ARTIST
import app.encore.android.playback.CarButtonRules.Id.LIKE
import app.encore.android.playback.CarButtonRules.Id.LYRICS
import app.encore.android.playback.CarButtonRules.Id.REPEAT
import app.encore.android.playback.CarButtonRules.Id.SHUFFLE
import app.encore.android.playback.CarButtonRules.Id.SIGNAL
import app.encore.android.playback.CarButtonRules.Layout
import app.encore.android.playback.CarButtonRules.Preview
import app.encore.android.playback.CarButtonRules.Row
import app.encore.android.playback.CarButtonRules.Tag
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Settings → Playback → In the car → Car buttons (CarButtonRules): what Android Auto gets and what a car shows. */
class CarButtonRulesTest {
    private fun layout(screen: List<CarButtonRules.Id>, menu: List<CarButtonRules.Id>, hidden: List<CarButtonRules.Id>) = Layout(screen, menu, hidden)

    /** every button exactly once */
    private fun assertWhole(l: Layout) {
        val all = l.screen + l.menu + l.hidden
        assertEquals(CarButtonRules.Id.entries.size, all.size)
        assertEquals(CarButtonRules.Id.entries.toSet(), all.toSet())
    }

    // ---- the default and Reset -------------------------------------------------------------------------------------

    @Test
    fun theDefaultShowsShuffleRepeatLikeLyricsWithNoMenu() {
        val d = CarButtonRules.DEFAULT
        assertEquals(listOf(SHUFFLE, REPEAT, LIKE, LYRICS), d.screen)
        assertEquals(emptyList<CarButtonRules.Id>(), d.menu)
        assertEquals(listOf(ARTIST, SIGNAL), d.hidden)
        assertWhole(d)
        assertEquals(4, CarButtonRules.DEFAULT_SPOTS)
        assertEquals(listOf(SHUFFLE, REPEAT, LIKE, LYRICS), CarButtonRules.sent(d))
        // the owner's 4-spot car: all four, no ⋯, nothing tagged
        val p = CarButtonRules.preview(d, 4)
        assertEquals(Preview(listOf(SHUFFLE, REPEAT, LIKE, LYRICS), emptyList()), p)
        assertEquals(emptyMap<CarButtonRules.Id, Tag>(), CarButtonRules.tags(d, 4))
        assertEquals("In your car: Shuffle · Repeat · Like · Lyrics", CarButtonRules.previewLine(p))
        assertEquals("No ⋯ – everything fits", CarButtonRules.behindLine(p))
        assertNull(CarButtonRules.likeLine(p))
        assertEquals("Shuffle · Repeat · Like · Lyrics", CarButtonRules.summary(d, 4))
        assertEquals("4 of 4 spots", CarButtonRules.capacityLine(d, 4))
    }

    @Test
    fun resetAndUndoStoreTheirLayoutAsIs() {
        // Reset stores DEFAULT (the spot count is left alone): what decode reads back is the default again
        assertEquals(CarButtonRules.DEFAULT, CarButtonRules.decode(CarButtonRules.encode(CarButtonRules.DEFAULT)))
        assertEquals("v1|screen=shuffle,repeat,like,lyrics|menu=|hidden=artist,signal", CarButtonRules.encode(CarButtonRules.DEFAULT))
        // Undo keeps what Reset replaced in its stored form and puts it back: a changed layout reads back as itself
        val changed = CarButtonRules.move(CarButtonRules.move(CarButtonRules.DEFAULT, LIKE, Group.HIDDEN), SIGNAL, Group.SCREEN, 0)
        assertTrue(changed != CarButtonRules.DEFAULT)
        assertEquals(changed, CarButtonRules.decode(CarButtonRules.encode(changed)))
        val withMenu = layout(listOf(LYRICS), listOf(ARTIST, LIKE, SHUFFLE), listOf(SIGNAL, REPEAT))
        assertEquals(withMenu, CarButtonRules.decode(CarButtonRules.encode(withMenu)))
    }

    @Test
    fun sixButtonsInAFourSpotCarPutThreeAndAMenuOnTheScreen() {
        val l = layout(listOf(SHUFFLE, REPEAT, LIKE, LYRICS), listOf(ARTIST, SIGNAL), emptyList())
        assertEquals(listOf(SHUFFLE, REPEAT, LIKE, LYRICS, ARTIST, SIGNAL), CarButtonRules.sent(l))
        val p = CarButtonRules.preview(l, 4)
        assertEquals(listOf(SHUFFLE, REPEAT, LIKE), p.shown)
        assertEquals(listOf(LYRICS, ARTIST, SIGNAL), p.behind)
        assertEquals(mapOf(LYRICS to Tag.BEHIND, ARTIST to Tag.BEHIND, SIGNAL to Tag.BEHIND), CarButtonRules.tags(l, 4))
        assertEquals("In your car: Shuffle · Repeat · Like · ⋯", CarButtonRules.previewLine(p))
        assertEquals("Behind ⋯: Lyrics, Artist, Signal path", CarButtonRules.behindLine(p))
        assertEquals("Shuffle · Repeat · Like · ⋯ (Lyrics, Artist, Signal path)", CarButtonRules.summary(l, 4))
        assertEquals("3 of 3 spots + ⋯", CarButtonRules.capacityLine(l, 4))
        // 6 and 8 spots: everything fits
        for (s in listOf(6, 8)) {
            val q = CarButtonRules.preview(l, s)
            assertEquals(CarButtonRules.sent(l), q.shown)
            assertTrue(q.behind.isEmpty())
            assertEquals(mapOf(ARTIST to Tag.ON_SCREEN, SIGNAL to Tag.ON_SCREEN), CarButtonRules.tags(l, s))
        }
        assertEquals("6 of 8 spots", CarButtonRules.capacityLine(l, 8))
    }

    // ---- the listener's order is kept ----------------------------------------------------------------------------------

    @Test
    fun theOrderIsSentExactlyAsChosen() {
        // Like after Lyrics stays after Lyrics, even where that puts Like behind ⋯
        val l = layout(listOf(SHUFFLE, REPEAT, LYRICS, LIKE), listOf(ARTIST), listOf(SIGNAL))
        assertEquals(listOf(SHUFFLE, REPEAT, LYRICS, LIKE, ARTIST), CarButtonRules.sent(l))
        val p = CarButtonRules.preview(l, 4)
        assertEquals(listOf(SHUFFLE, REPEAT, LYRICS), p.shown)
        assertEquals(listOf(LIKE, ARTIST), p.behind)
        // at 3 spots too: nothing moves up
        assertEquals(listOf(SHUFFLE, REPEAT), CarButtonRules.preview(l, 3).shown)
        // with room for all, nothing moves either
        assertEquals(listOf(SHUFFLE, REPEAT, LYRICS, LIKE, ARTIST), CarButtonRules.preview(l, 5).shown)
        // every arrangement reaches the car in the page's order: screen group, then menu group
        val other = layout(listOf(SIGNAL), listOf(LIKE, SHUFFLE, LYRICS), listOf(REPEAT, ARTIST))
        assertEquals(listOf(SIGNAL, LIKE, SHUFFLE, LYRICS), CarButtonRules.sent(other))
    }

    @Test
    fun likeBehindTheMenuIsSaidPlainly() {
        val l = layout(listOf(SHUFFLE, REPEAT, LYRICS, LIKE), listOf(ARTIST), listOf(SIGNAL))
        val tags = CarButtonRules.tags(l, 4)
        assertEquals(Tag.LIKE_BEHIND, tags[LIKE])
        assertEquals(Tag.BEHIND, tags[ARTIST])
        assertEquals("Behind ⋯ in your car, not on the screen", Tag.LIKE_BEHIND.text)
        assertEquals(
            "Like is behind ⋯ in your car, not on the screen. Move it higher in the list to keep it on the screen.",
            CarButtonRules.likeLine(CarButtonRules.preview(l, 4)),
        )
        // Like put in the menu by the listener and behind ⋯ there: still said
        val inMenu = layout(listOf(SHUFFLE, REPEAT, LYRICS), listOf(ARTIST, LIKE), listOf(SIGNAL))
        assertEquals(Tag.LIKE_BEHIND, CarButtonRules.tags(inMenu, 4)[LIKE])
        // Like on the screen: nothing to say
        val ok = layout(listOf(LIKE, SHUFFLE, REPEAT, LYRICS), listOf(ARTIST), listOf(SIGNAL))
        assertNull(CarButtonRules.tags(ok, 4)[LIKE])
        assertNull(CarButtonRules.likeLine(CarButtonRules.preview(ok, 4)))
        // Like hidden: not in the car, no tag
        val hidden = layout(listOf(SHUFFLE, REPEAT, LYRICS, ARTIST, SIGNAL), emptyList(), listOf(LIKE))
        assertNull(CarButtonRules.tags(hidden, 4)[LIKE])
        assertNull(CarButtonRules.likeLine(CarButtonRules.preview(hidden, 4)))
    }

    @Test
    fun theMenuCantBeForcedWhileSpotsAreFree() {
        val l = layout(listOf(SHUFFLE, REPEAT), listOf(ARTIST, SIGNAL), listOf(LIKE, LYRICS))
        val p = CarButtonRules.preview(l, 4)
        assertEquals(listOf(SHUFFLE, REPEAT, ARTIST, SIGNAL), p.shown)
        assertTrue(p.behind.isEmpty())
        assertEquals(mapOf(ARTIST to Tag.ON_SCREEN, SIGNAL to Tag.ON_SCREEN), CarButtonRules.tags(l, 4))
        assertEquals("On the screen in your car – there's a free spot", Tag.ON_SCREEN.text)
        // one more menu button: the first one still takes the free spot, the other two go behind ⋯
        val more = layout(listOf(SHUFFLE, REPEAT), listOf(ARTIST, SIGNAL, LYRICS), listOf(LIKE))
        assertEquals(mapOf(ARTIST to Tag.ON_SCREEN, SIGNAL to Tag.BEHIND, LYRICS to Tag.BEHIND), CarButtonRules.tags(more, 4))
    }

    @Test
    fun screenButtonsGoBehindTheMenuWhenTheyDontFit() {
        val l = layout(listOf(SHUFFLE, REPEAT, LIKE, LYRICS), listOf(ARTIST), listOf(SIGNAL))
        assertEquals(Tag.BEHIND, CarButtonRules.tags(l, 4)[LYRICS])
        assertEquals("Behind ⋯ in your car", Tag.BEHIND.text)
        // five on the screen, nothing in the menu, four spots: the 4th and 5th are behind ⋯
        val five = layout(listOf(SHUFFLE, REPEAT, LIKE, LYRICS, ARTIST), emptyList(), listOf(SIGNAL))
        val p = CarButtonRules.preview(five, 4)
        assertEquals(listOf(SHUFFLE, REPEAT, LIKE), p.shown)
        assertEquals(listOf(LYRICS, ARTIST), p.behind)
        assertEquals(mapOf(LYRICS to Tag.BEHIND, ARTIST to Tag.BEHIND), CarButtonRules.tags(five, 4))
    }

    @Test
    fun hiddenButtonsAreNeverSent() {
        val l = layout(listOf(LIKE), listOf(SHUFFLE), listOf(REPEAT, LYRICS, ARTIST, SIGNAL))
        assertEquals(listOf(LIKE, SHUFFLE), CarButtonRules.sent(l))
        for (s in CarButtonRules.SPOTS) {
            val p = CarButtonRules.preview(l, s)
            assertTrue((p.shown + p.behind).none { it in l.hidden })
            assertTrue(CarButtonRules.tags(l, s).keys.none { it in l.hidden })
        }
        // all hidden: only Previous, Play and Next
        val none = layout(emptyList(), emptyList(), CarButtonRules.Id.entries)
        assertEquals(emptyList<CarButtonRules.Id>(), CarButtonRules.sent(none))
        val p = CarButtonRules.preview(none, 4)
        assertEquals("No buttons in the car – only Previous, Play and Next", CarButtonRules.previewLine(p))
        assertEquals("", CarButtonRules.behindLine(p))
        assertEquals("No buttons – only Previous, Play and Next", CarButtonRules.summary(none, 4))
        assertEquals("0 of 4 spots", CarButtonRules.capacityLine(none, 4))
    }

    @Test
    fun spotsStayBetweenTwoAndEight() {
        assertEquals(2, CarButtonRules.spots(0))
        assertEquals(2, CarButtonRules.spots(-3))
        assertEquals(8, CarButtonRules.spots(99))
        assertEquals(5, CarButtonRules.spots(5))
        // two spots and three buttons: one shows, ⋯ holds two
        val l = layout(listOf(SHUFFLE, REPEAT, LIKE), emptyList(), listOf(LYRICS, ARTIST, SIGNAL))
        assertEquals(Preview(listOf(SHUFFLE), listOf(REPEAT, LIKE)), CarButtonRules.preview(l, 2))
        assertEquals("1 of 1 spot + ⋯", CarButtonRules.capacityLine(l, 2))
        assertEquals("Shuffle · ⋯ (Repeat, Like)", CarButtonRules.summary(l, 2))
        // out-of-range spots are clamped for the preview too
        assertEquals(CarButtonRules.preview(l, 2), CarButtonRules.preview(l, 0))
    }

    // ---- the stored value ----------------------------------------------------------------------------------------------

    @Test
    fun theLayoutIsStoredAndReadBack() {
        val l = layout(listOf(LIKE, SIGNAL), listOf(LYRICS), listOf(SHUFFLE, REPEAT, ARTIST))
        val s = CarButtonRules.encode(l)
        assertEquals("v1|screen=like,signal|menu=lyrics|hidden=shuffle,repeat,artist", s)
        assertEquals(l, CarButtonRules.decode(s))
        val empty = layout(emptyList(), emptyList(), CarButtonRules.Id.entries)
        assertEquals(empty, CarButtonRules.decode(CarButtonRules.encode(empty)))
    }

    @Test
    fun nothingOrGarbageStoredReadsAsTheDefault() {
        for (s in listOf(null, "", "   ", "junk", "v1", "v1|", "v2|screen=like|menu=|hidden=", "screen=like", "v1|nothing=like")) {
            assertEquals(s.toString(), CarButtonRules.DEFAULT, CarButtonRules.decode(s))
        }
    }

    @Test
    fun unknownIdsAreDroppedAndMissingOnesGoToTheirDefaultGroup() {
        // a button from a later version ("similar") is dropped; Artist and Signal path are missing: hidden, as by default
        val l = CarButtonRules.decode("v1|screen=lyrics,similar,like|menu=shuffle|hidden=repeat")
        assertEquals(listOf(LYRICS, LIKE), l.screen)
        assertEquals(listOf(SHUFFLE), l.menu)
        assertEquals(listOf(REPEAT, ARTIST, SIGNAL), l.hidden)
        assertWhole(l)
        // a missing default-screen button goes to the end of the screen group
        val m = CarButtonRules.decode("v1|screen=like|menu=artist|hidden=signal")
        assertEquals(listOf(LIKE, SHUFFLE, REPEAT, LYRICS), m.screen)
        assertEquals(listOf(ARTIST), m.menu)
        assertEquals(listOf(SIGNAL), m.hidden)
        // a group left out: its buttons come from where they are missing
        val n = CarButtonRules.decode("v1|screen=signal,artist")
        assertEquals(listOf(SIGNAL, ARTIST, SHUFFLE, REPEAT, LIKE, LYRICS), n.screen)
        assertWhole(n)
        // spaces and empty entries are tolerated
        assertEquals(listOf(LIKE, SHUFFLE), CarButtonRules.decode("v1| screen = like, ,shuffle |menu=|hidden=repeat,lyrics,artist,signal").screen)
    }

    @Test
    fun aButtonListedTwiceStaysWhereItCameFirst() {
        val l = CarButtonRules.decode("v1|screen=like,shuffle,like|menu=shuffle,repeat|hidden=lyrics,artist,signal|screen=signal")
        assertEquals(listOf(LIKE, SHUFFLE), l.screen)
        assertEquals(listOf(REPEAT), l.menu)
        assertEquals(listOf(LYRICS, ARTIST, SIGNAL), l.hidden)
        assertWhole(l)
    }

    // ---- moving buttons ------------------------------------------------------------------------------------------------

    @Test
    fun movesWithinAndAcrossGroups() {
        val d = CarButtonRules.DEFAULT
        // within the screen group
        assertEquals(listOf(SHUFFLE, LIKE, REPEAT, LYRICS), CarButtonRules.up(d, LIKE).screen)
        assertEquals(listOf(SHUFFLE, REPEAT, LYRICS, LIKE), CarButtonRules.down(d, LIKE).screen)
        // the bottom of the screen ▼: the top of the (empty) menu, then the top of Hidden
        val a = CarButtonRules.down(d, LYRICS)
        assertEquals(listOf(SHUFFLE, REPEAT, LIKE), a.screen)
        assertEquals(listOf(LYRICS), a.menu)
        val b = CarButtonRules.down(a, LYRICS)
        assertEquals(emptyList<CarButtonRules.Id>(), b.menu)
        assertEquals(listOf(LYRICS, ARTIST, SIGNAL), b.hidden)
        // the top of Hidden ▲: the end of the menu; the top of the menu ▲: the end of the screen
        val c = CarButtonRules.up(d, ARTIST)
        assertEquals(listOf(ARTIST), c.menu)
        assertEquals(listOf(SHUFFLE, REPEAT, LIKE, LYRICS, ARTIST), CarButtonRules.up(c, ARTIST).screen)
        // nothing above the first screen button, nothing below the last hidden one
        assertEquals(d, CarButtonRules.up(d, SHUFFLE))
        assertEquals(d, CarButtonRules.down(d, SIGNAL))
        assertFalse(CarButtonRules.canUp(d, SHUFFLE))
        assertFalse(CarButtonRules.canDown(d, SIGNAL))
        assertTrue(CarButtonRules.canUp(d, ARTIST))
        assertTrue(CarButtonRules.canDown(d, LYRICS))
        // the ⋮ menu: to the end of a group, or at a place
        val e = CarButtonRules.move(d, SIGNAL, Group.MENU)
        assertEquals(listOf(SIGNAL), e.menu)
        assertEquals(listOf(ARTIST), e.hidden)
        assertEquals(listOf(SIGNAL, SHUFFLE, REPEAT, LIKE, LYRICS), CarButtonRules.move(d, SIGNAL, Group.SCREEN, 0).screen)
        assertEquals(listOf(SHUFFLE, REPEAT, LYRICS), CarButtonRules.move(d, LIKE, Group.HIDDEN).screen)
        assertEquals(listOf(ARTIST, SIGNAL, LIKE), CarButtonRules.move(d, LIKE, Group.HIDDEN).hidden)
        for (l in listOf(a, b, c, e)) assertWhole(l)
        assertEquals(Group.MENU, e.groupOf(SIGNAL))
        assertEquals(Group.HIDDEN, e.groupOf(ARTIST))
        assertEquals(Group.SCREEN, e.groupOf(LIKE))
    }

    @Test
    fun theGroupsHaveTheirNames() {
        assertEquals(listOf("On the car screen", "In the ⋯ menu", "Hidden"), Group.entries.map { it.title })
        assertEquals(listOf("Shuffle", "Repeat", "Like", "Lyrics", "Artist", "Signal path"), CarButtonRules.Id.entries.map { it.label })
    }

    // ---- dragging --------------------------------------------------------------------------------------------------------

    @Test
    fun theListIsHeadersAndButtons() {
        assertEquals(
            listOf(
                Row.Header(Group.SCREEN), Row.Button(SHUFFLE), Row.Button(REPEAT), Row.Button(LIKE), Row.Button(LYRICS),
                Row.Header(Group.MENU),
                Row.Header(Group.HIDDEN), Row.Button(ARTIST), Row.Button(SIGNAL),
            ),
            CarButtonRules.flatten(CarButtonRules.DEFAULT),
        )
    }

    @Test
    fun aDropLandsWhereItWasLetGo() {
        val d = CarButtonRules.DEFAULT
        // rows: 0 [screen] 1 S 2 R 3 L 4 Ly 5 [menu] 6 [hidden] 7 A 8 G
        // down onto a button: after it
        assertEquals(listOf(REPEAT, LIKE, SHUFFLE, LYRICS), CarButtonRules.dropAt(d, SHUFFLE, 3).screen)
        // up onto a button: before it
        assertEquals(listOf(LYRICS, SHUFFLE, REPEAT, LIKE), CarButtonRules.dropAt(d, LYRICS, 1).screen)
        // onto a header, from above or below: first in that group
        val toMenu = CarButtonRules.dropAt(d, LIKE, 5)
        assertEquals(listOf(LIKE), toMenu.menu)
        assertEquals(listOf(SHUFFLE, REPEAT, LYRICS), toMenu.screen)
        val fromHidden = CarButtonRules.dropAt(d, SIGNAL, 5)
        assertEquals(listOf(SIGNAL), fromHidden.menu)
        assertEquals(listOf(ARTIST), fromHidden.hidden)
        assertEquals(listOf(SIGNAL, SHUFFLE, REPEAT, LIKE, LYRICS), CarButtonRules.dropAt(d, SIGNAL, 0).screen)
        assertEquals(listOf(LYRICS, ARTIST, SIGNAL), CarButtonRules.dropAt(d, LYRICS, 6).hidden)
        // up from Hidden onto a screen button: into the screen group before it
        assertEquals(listOf(SHUFFLE, ARTIST, REPEAT, LIKE, LYRICS), CarButtonRules.dropAt(d, ARTIST, 2).screen)
        // past the end: last in Hidden; above the top: first on the screen
        assertEquals(listOf(ARTIST, SIGNAL, SHUFFLE), CarButtonRules.dropAt(d, SHUFFLE, 99).hidden)
        assertEquals(listOf(ARTIST, SHUFFLE, REPEAT, LIKE, LYRICS), CarButtonRules.dropAt(d, ARTIST, -5).screen)
        // let go where it started: nothing changes
        assertEquals(d, CarButtonRules.dropAt(d, LIKE, 3))
        assertEquals(d, CarButtonRules.dropAt(d, SIGNAL, 99))
        for (i in -1..10) for (id in CarButtonRules.Id.entries) assertWhole(CarButtonRules.dropAt(d, id, i))
    }

    // ---- what TalkBack says ---------------------------------------------------------------------------------------------

    @Test
    fun aMoveSaysWhereTheButtonWent() {
        // the last button on the screen, moved down: into the ⋯ menu; a 4-spot car still shows it, a 3-spot car doesn't
        val l = CarButtonRules.down(CarButtonRules.DEFAULT, LYRICS)
        assertEquals(listOf(LYRICS), l.menu)
        assertEquals("Lyrics: in the ⋯ menu, 1 of 1. On the screen in your car – there's a free spot", CarButtonRules.movedLine(l, LYRICS, 4))
        assertEquals("Lyrics: in the ⋯ menu, 1 of 1. Behind ⋯ in your car", CarButtonRules.movedLine(l, LYRICS, 3))
        // with a free spot the menu button shows on the screen, and the line says so
        val a = CarButtonRules.move(CarButtonRules.DEFAULT, ARTIST, Group.MENU)
        assertEquals("Artist: in the ⋯ menu, 1 of 1. On the screen in your car – there's a free spot", CarButtonRules.movedLine(a, ARTIST, 5))
        assertEquals("Artist: in the ⋯ menu, 1 of 1. Behind ⋯ in your car", CarButtonRules.movedLine(a, ARTIST, 4))
        assertEquals("Shuffle: on the car screen, 2 of 4", CarButtonRules.movedLine(CarButtonRules.down(CarButtonRules.DEFAULT, SHUFFLE), SHUFFLE, 4))
        assertEquals("Signal path: hidden, 1 of 2", CarButtonRules.movedLine(CarButtonRules.up(CarButtonRules.DEFAULT, SIGNAL), SIGNAL, 4))
        // the move that pushes Like behind ⋯ (Artist into the menu with Shuffle, Repeat, Lyrics, Like on the screen)
        val r = layout(listOf(SHUFFLE, REPEAT, LYRICS, LIKE), emptyList(), listOf(ARTIST, SIGNAL))
        val pushed = CarButtonRules.move(r, ARTIST, Group.MENU)
        assertEquals("Artist: in the ⋯ menu, 1 of 1. Behind ⋯ in your car", CarButtonRules.movedLine(pushed, ARTIST, 4))
        assertEquals(Tag.LIKE_BEHIND, CarButtonRules.tags(pushed, 4)[LIKE])
        assertEquals(
            "Like: on the car screen, 4 of 4. Behind ⋯ in your car, not on the screen",
            CarButtonRules.movedLine(pushed, LIKE, 4),
        )
    }

    @Test
    fun talkBackSaysTheMoreButtonInWords() {
        assertEquals("In the More menu", CarButtonRules.spoken(Group.MENU.title))
        assertEquals("Behind the More button in your car", CarButtonRules.spoken(Tag.BEHIND.text))
        assertEquals("Behind the More button in your car, not on the screen", CarButtonRules.spoken(Tag.LIKE_BEHIND.text))
        assertEquals("No More button – everything fits", CarButtonRules.spoken("No ⋯ – everything fits"))
        assertEquals("3 of 3 spots + the More button", CarButtonRules.spoken("3 of 3 spots + ⋯"))
        assertEquals("Move to the More menu", CarButtonRules.spoken("Move to the ⋯ menu"))
        assertEquals(
            "Use the arrows or the Move to another group button, or hold a button and drag it.",
            CarButtonRules.spoken("Use the arrows or the ⋮ menu, or hold a button and drag it."),
        )
        val l = layout(listOf(SHUFFLE, REPEAT, LYRICS, LIKE), listOf(ARTIST), listOf(SIGNAL))
        val p = CarButtonRules.preview(l, 4)
        assertEquals("In your car: Shuffle · Repeat · Lyrics · the More button", CarButtonRules.spoken(CarButtonRules.previewLine(p)))
        assertEquals("Behind the More button: Like, Artist", CarButtonRules.spoken(CarButtonRules.behindLine(p)))
        // every text of the page that has ⋯ or ⋮ has neither once spoken
        val texts = Tag.entries.map { it.text } + Group.entries.map { it.title } + listOf(
            CarButtonRules.previewLine(p), CarButtonRules.behindLine(p), CarButtonRules.likeLine(p) ?: "",
            CarButtonRules.behindLine(CarButtonRules.preview(CarButtonRules.DEFAULT, 4)),
            CarButtonRules.summary(l, 4), CarButtonRules.capacityLine(l, 4), CarButtonRules.movedLine(l, ARTIST, 4),
        )
        for (t in texts) assertTrue(t, '⋯' !in CarButtonRules.spoken(t) && '⋮' !in CarButtonRules.spoken(t))
        // a text without them is said as it is
        assertEquals("Shuffle · Repeat · Like · Lyrics", CarButtonRules.spoken("Shuffle · Repeat · Like · Lyrics"))
    }

    // ---- notes ----------------------------------------------------------------------------------------------------------

    @Test
    fun aNoteWinsUntilItEnds() {
        assertEquals("Adding songs by DROELOE…", CarButtonRules.subtitleNow("Adding songs by DROELOE…", 5_000L, 4_999L, "DROELOE · FLAC 24/96"))
        assertEquals("DROELOE · FLAC 24/96", CarButtonRules.subtitleNow("Adding songs by DROELOE…", 5_000L, 5_000L, "DROELOE · FLAC 24/96"))
        assertEquals("DROELOE · FLAC 24/96", CarButtonRules.subtitleNow(null, 99_000L, 0L, "DROELOE · FLAC 24/96"))
        assertEquals(3_000L, CarButtonRules.NOTE_MS)
        assertEquals(10_000L, CarButtonRules.SIGNAL_NOTE_MS)
    }

    // ---- Signal path ---------------------------------------------------------------------------------------------------

    @Test
    fun theSignalPathInOneLine() {
        fun line(
            file: String = "FLAC 16/44.1", kind: String = "original", saver: Boolean = false, local: String = "", phoneRate: Int = 0,
            usb: Boolean = false, dac: String = "", effects: Boolean = false, out: Int = 48000, norm: Double = 0.0,
        ) = CarButtonRules.signalLine(CarButtonRules.Signal(file, kind, saver, local, phoneRate, usb, dac, effects, out, norm))
        assertEquals("FLAC 24/96 → as is → Android 48 kHz", line(file = "FLAC 24/96"))
        assertEquals("FLAC 16/44.1 → converted to FLAC → Android 44.1 kHz", line(kind = "flac", out = 44100))
        assertEquals("FLAC 16/44.1 → AAC 96k (Data saver) → Android 48 kHz", line(kind = "aac-96", saver = true))
        assertEquals("FLAC 16/44.1 → AAC 128k (adaptive) → Android 48 kHz", line(kind = "aac-128"))
        assertEquals("FLAC 16/44.1 → Cadenza 24/192 (server) → Android 192 kHz", line(kind = "cadenza-192000", out = 192000))
        assertEquals("FLAC 16/44.1 → as is → Cadenza 96 kHz on this phone", line(phoneRate = 96000, out = 96000))
        assertEquals("Saved FLAC 24/96 → bit-perfect to iFi Zen DAC", line(kind = "local", local = "FLAC 24/96", usb = true, dac = "iFi Zen DAC"))
        assertEquals("Saved FLAC 16/44.1 → Android 48 kHz", line(kind = "local"))
        assertEquals("MP3 320 → as is → bit-perfect to the USB DAC", line(file = "MP3 320", usb = true))
        assertEquals("FLAC 16/44.1 → as is → effects → Android 48 kHz", line(effects = true))
        assertEquals("FLAC 16/44.1 → as is → Android 48 kHz · −3.2 dB", line(norm = -3.2))
        assertEquals("FLAC 16/44.1 → as is → Android 48 kHz · +1.5 dB", line(norm = 1.5))
        // a gain that rounds to 0.0 dB is no gain
        assertEquals("FLAC 16/44.1 → as is → Android 48 kHz", line(norm = 0.04))
        assertEquals("FLAC 16/44.1 → as is → Android", line(out = 0))
        // before the service says how the song arrives
        assertEquals("Quality not known yet", line(kind = ""))
        assertEquals("Quality not known yet", CarButtonRules.signalLine(CarButtonRules.Signal()))
    }

    // ---- Artist: More by <artist> --------------------------------------------------------------------------------------

    @Test
    fun moreByPicksNewSongsInOrderUpToTheLimit() {
        val list = listOf(7L, 3L, 9L, 3L, 1L, 4L, 5L)
        // not the one playing, none already coming up, each once, in the list's order
        assertEquals(listOf(7L, 1L, 5L), CarButtonRules.moreByPick(list, playing = 3L, upcoming = setOf(9L, 4L), max = 25))
        assertEquals(listOf(7L, 3L), CarButtonRules.moreByPick(list, playing = null, upcoming = emptySet(), max = 2))
        assertEquals(emptyList<Long>(), CarButtonRules.moreByPick(listOf(3L), playing = 3L, upcoming = emptySet(), max = 25))
        assertEquals(emptyList<Long>(), CarButtonRules.moreByPick(list, playing = null, upcoming = emptySet(), max = 0))
        assertEquals(25, CarButtonRules.moreByPick((1L..500L).toList(), 1L, emptySet(), CarRules.MORE_BY_SONGS).size)
    }

    @Test
    fun aSecondPressForTheSameSongAddsNothing() {
        val song = "track:42|album:7"
        assertTrue(CarButtonRules.alreadyAdded("mbid-1", song, "mbid-1", song))
        assertFalse(CarButtonRules.alreadyAdded("mbid-1", song, "mbid-1", "track:43"))
        assertFalse(CarButtonRules.alreadyAdded("mbid-1", song, "mbid-2", song))
        assertFalse(CarButtonRules.alreadyAdded(null, null, "mbid-1", song))
        assertFalse(CarButtonRules.alreadyAdded(null, null, "mbid-1", null))
    }

    @Test
    fun theArtistButtonSaysWhatItDoes() {
        assertEquals("Adding songs by DROELOE…", CarButtonRules.addingMessage("DROELOE"))
        assertEquals("Already added songs by DROELOE after this song", CarButtonRules.alreadyAddedMessage("DROELOE", group = false))
        assertEquals("Already added songs by DROELOE to the group's Up next", CarButtonRules.alreadyAddedMessage("DROELOE", group = true))
        assertEquals("No artist for this song", CarButtonRules.NO_ARTIST)
        // the result: the same words as Encore Lyrics' "More by <artist>"
        assertEquals("Added 25 songs by DROELOE after this song", CarRules.moreByMessage("DROELOE", 25, group = false))
    }

    @Test
    fun noUiTextHasAnExclamationMark() {
        val l = layout(listOf(SHUFFLE, REPEAT, LYRICS, LIKE), listOf(ARTIST), listOf(SIGNAL))
        val p = CarButtonRules.preview(l, 4)
        val texts = Tag.entries.map { it.text } + Group.entries.map { it.title } + listOf(
            CarButtonRules.previewLine(p), CarButtonRules.behindLine(p), CarButtonRules.likeLine(p) ?: "",
            CarButtonRules.summary(l, 4), CarButtonRules.capacityLine(l, 4), CarButtonRules.addingMessage("X"),
            CarButtonRules.alreadyAddedMessage("X", false), CarButtonRules.NO_ARTIST, CarButtonRules.SIGNAL_UNKNOWN,
            CarButtonRules.movedLine(l, ARTIST, 4), CarButtonRules.spoken(CarButtonRules.summary(l, 4)),
        )
        assertTrue(texts.none { '!' in it })
    }
}
