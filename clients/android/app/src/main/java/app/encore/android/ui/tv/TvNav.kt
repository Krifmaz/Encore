/**
 * Which TV side menu entry stands for what is on screen: kept apart from the menu's drawing so it is tested without a
 * device (TvNavTest).
 */
package app.encore.android.ui.tv

import app.encore.android.ui.Screen
import app.encore.android.ui.Tab

/** The link between the app's places ([Screen], [Tab]) and the TV side menu's entries ([RailItem]). */
object TvNav {
    /**
     * The side menu entry marked for what is on screen.
     *
     * @param playerOpen whether the TV player is open (it covers everything: Now playing).
     * @param top the page on top of the stack (null: the tab's root page).
     * @param tab the tab underneath.
     * @return the entry, or null for a page the menu has no entry for (an album, an artist, a playlist…): nothing is
     *   marked then, as the phone's tabs mark none above their root.
     */
    fun selected(playerOpen: Boolean, top: Screen?, tab: Tab): RailItem? = when {
        playerOpen -> RailItem.NOW_PLAYING
        top == null -> when (tab) {
            Tab.HOME -> RailItem.HOME
            Tab.DISCOVER -> RailItem.SEARCH
            Tab.LIBRARY -> RailItem.LIBRARY
            Tab.PLAYLISTS -> RailItem.PLAYLISTS
        }
        top is Screen.ForYou -> RailItem.FOR_YOU
        top is Screen.Settings || top is Screen.SettingsAt -> RailItem.SETTINGS
        else -> null
    }
}
