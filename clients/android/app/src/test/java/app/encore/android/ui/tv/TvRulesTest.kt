package app.encore.android.ui.tv

import android.view.KeyEvent
import app.encore.android.ui.Screen
import app.encore.android.ui.Tab
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The TV layout's rules (TvRules.kt) and which side menu entry is marked (TvNav.kt). */
class TvRulesTest {
    /** The TV layout is on for a TV unless its setting says "Phone layout", and never on a phone. */
    @Test
    fun tvLayoutOnlyOnATv() {
        assertTrue(TvRules.useTvLayout(TvLayoutMode.AUTO, isTv = true))
        assertFalse(TvRules.useTvLayout(TvLayoutMode.OFF, isTv = true))
        assertFalse(TvRules.useTvLayout(TvLayoutMode.AUTO, isTv = false))
    }

    /** Stored values read back as they were written; unknown or missing ones fall to the defaults. */
    @Test
    fun storedValuesRoundTrip() {
        for (v in PlayerSide.entries) assertEquals(v, PlayerSide.of(v.key))
        for (v in SidePanel.entries) assertEquals(v, SidePanel.of(v.key))
        for (v in TvZoom.entries) assertEquals(v, TvZoom.of(v.key))
        for (v in TvLayoutMode.entries) assertEquals(v, TvLayoutMode.of(v.key))
        assertEquals(PlayerSide.LEFT, PlayerSide.of(null))
        assertEquals(SidePanel.LYRICS, SidePanel.of("nonsense"))
        assertEquals(TvZoom.MEDIUM, TvZoom.of(""))
        assertEquals(TvLayoutMode.AUTO, TvLayoutMode.of("tablet"))
    }

    /** The panel's switch flips between its two halves. */
    @Test
    fun panelSwitchFlips() {
        assertEquals(SidePanel.QUEUE, SidePanel.LYRICS.other())
        assertEquals(SidePanel.LYRICS, SidePanel.QUEUE.other())
    }

    /** "Left" and "Right" mean the screen's sides, also when the layout runs right to left. */
    @Test
    fun controlsSideFollowsTheScreen() {
        assertTrue(TvRules.controlsFirst(PlayerSide.LEFT, rtl = false))
        assertFalse(TvRules.controlsFirst(PlayerSide.RIGHT, rtl = false))
        assertFalse(TvRules.controlsFirst(PlayerSide.LEFT, rtl = true))
        assertTrue(TvRules.controlsFirst(PlayerSide.RIGHT, rtl = true))
    }

    /** Zooming only ever makes things smaller or keeps the phone's size. */
    @Test
    fun zoomScalesDown() {
        for (z in TvZoom.entries) assertTrue(z.scale in 0.5f..1.0f)
        assertTrue(TvZoom.SMALL.scale < TvZoom.MEDIUM.scale && TvZoom.MEDIUM.scale < TvZoom.LARGE.scale)
    }

    /** A remote press seeks 10 s, never before the start or past the end, and not at all without a length. */
    @Test
    fun remoteSeekStaysInTheSong() {
        assertEquals(40_000L, TvRules.seekTarget(30_000L, 200_000L, forward = true))
        assertEquals(20_000L, TvRules.seekTarget(30_000L, 200_000L, forward = false))
        assertEquals(0L, TvRules.seekTarget(4_000L, 200_000L, forward = false))
        assertEquals(200_000L, TvRules.seekTarget(195_000L, 200_000L, forward = true))
        assertEquals(5_000L, TvRules.seekTarget(5_000L, 0L, forward = true))
    }

    /** The side menu lists Now playing only while a song is loaded, and keeps Settings for its bottom. */
    @Test
    fun railListsNowPlayingOnlyWhilePlaying() {
        assertFalse(RailItem.NOW_PLAYING in TvRules.railItems(playing = false))
        assertTrue(RailItem.NOW_PLAYING in TvRules.railItems(playing = true))
        assertFalse(RailItem.SETTINGS in TvRules.railItems(playing = true))
        assertEquals(RailItem.SEARCH, TvRules.railItems(playing = false).first())
    }

    /** The entry marked follows the player, the page on top and the tab underneath. */
    @Test
    fun markedEntryFollowsTheScreen() {
        assertEquals(RailItem.NOW_PLAYING, TvNav.selected(true, Screen.Album("a"), Tab.HOME))
        assertEquals(RailItem.HOME, TvNav.selected(false, null, Tab.HOME))
        assertEquals(RailItem.SEARCH, TvNav.selected(false, null, Tab.DISCOVER))
        assertEquals(RailItem.LIBRARY, TvNav.selected(false, null, Tab.LIBRARY))
        assertEquals(RailItem.PLAYLISTS, TvNav.selected(false, null, Tab.PLAYLISTS))
        assertEquals(RailItem.FOR_YOU, TvNav.selected(false, Screen.ForYou, Tab.HOME))
        assertEquals(RailItem.SETTINGS, TvNav.selected(false, Screen.Settings, Tab.LIBRARY))
        assertEquals(RailItem.SETTINGS, TvNav.selected(false, Screen.SettingsAt("soulseek"), Tab.HOME))
        assertNull(TvNav.selected(false, Screen.Album("a"), Tab.HOME))
    }

    /** With the screen off, playback buttons act on the music, the TV keeps its own buttons, the rest wake it. */
    @Test
    fun screenOffButtons() {
        assertEquals(ScreenOffAction.TOGGLE_PLAY, ScreenOffRules.actionOf(KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE))
        assertEquals(ScreenOffAction.PLAY, ScreenOffRules.actionOf(KeyEvent.KEYCODE_MEDIA_PLAY))
        assertEquals(ScreenOffAction.PAUSE, ScreenOffRules.actionOf(KeyEvent.KEYCODE_MEDIA_PAUSE))
        assertEquals(ScreenOffAction.NEXT, ScreenOffRules.actionOf(KeyEvent.KEYCODE_MEDIA_NEXT))
        assertEquals(ScreenOffAction.PREVIOUS, ScreenOffRules.actionOf(KeyEvent.KEYCODE_MEDIA_PREVIOUS))
        assertEquals(ScreenOffAction.SYSTEM, ScreenOffRules.actionOf(KeyEvent.KEYCODE_VOLUME_UP))
        assertEquals(ScreenOffAction.SYSTEM, ScreenOffRules.actionOf(KeyEvent.KEYCODE_POWER))
        assertEquals(ScreenOffAction.WAKE, ScreenOffRules.actionOf(KeyEvent.KEYCODE_DPAD_CENTER))
        assertEquals(ScreenOffAction.WAKE, ScreenOffRules.actionOf(KeyEvent.KEYCODE_BACK))
        assertEquals(ScreenOffAction.WAKE, ScreenOffRules.actionOf(KeyEvent.KEYCODE_DPAD_LEFT))
    }
}
