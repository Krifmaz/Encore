/**
 * The rules of the TV layout (Android TV / Google TV, e.g. an NVIDIA Shield), as plain values with no Android in them
 * so they are tested without a device (TvRulesTest): which layout the app uses, which side the TV player sits on, what
 * its other half shows, how far the screen is zoomed out, what the side menu lists, which remote buttons move around
 * it and how far a remote press seeks.
 *
 * The stored forms (the `key`s) are what [TvPrefs] writes to this device's SharedPreferences; anything unknown read
 * back falls to the default, so an older or newer app never breaks on a value it doesn't know.
 */
package app.encore.android.ui.tv

import android.view.KeyEvent

/**
 * Whether the app uses the TV layout (the side menu, the TV player, the zoomed-out screen) or the phone one.
 *
 * @property key the stored form.
 * @property label what Settings → TV shows for it.
 */
enum class TvLayoutMode(val key: String, val label: String) {
    /** The TV layout on a TV (a leanback device), the phone layout everywhere else. The default. */
    AUTO("auto", "Automatic"),

    /** The phone layout even on a TV. */
    OFF("off", "Phone layout"),
    ;

    companion object {
        /** The mode stored as [key]; anything unknown is [AUTO]. */
        fun of(key: String?): TvLayoutMode = entries.firstOrNull { it.key == key } ?: AUTO
    }
}

/**
 * Which side of the screen the TV player's controls (cover, song, seek bar, buttons) take; the switchable panel
 * (lyrics or up next) takes the other side.
 *
 * @property key the stored form.
 * @property label what Settings → TV shows for it.
 */
enum class PlayerSide(val key: String, val label: String) {
    /** Controls on the left, the panel on the right. The default. */
    LEFT("left", "Left"),

    /** Controls on the right, the panel on the left (as the phone's sideways player has its lyrics on the left). */
    RIGHT("right", "Right"),
    ;

    companion object {
        /** The side stored as [key]; anything unknown is [LEFT]. */
        fun of(key: String?): PlayerSide = entries.firstOrNull { it.key == key } ?: LEFT
    }
}

/**
 * What the TV player's switchable half shows.
 *
 * @property key the stored form (the half is remembered, so the player opens the way it was left).
 * @property label the tab's word in the panel's switch.
 */
enum class SidePanel(val key: String, val label: String) {
    /** The song's lyrics, time-synced lines following the song. The default. */
    LYRICS("lyrics", "Lyrics"),

    /** What plays after this song. */
    QUEUE("queue", "Up next"),
    ;

    /** The other half: what the switch flips to. */
    fun other(): SidePanel = if (this == LYRICS) QUEUE else LYRICS

    companion object {
        /** The panel stored as [key]; anything unknown is [LYRICS]. */
        fun of(key: String?): SidePanel = entries.firstOrNull { it.key == key } ?: LYRICS
    }
}

/**
 * How far the TV layout zooms the app out: the share of the normal size everything is drawn at. A phone layout at
 * full size on a 1080p TV shows about a phone's worth of content; zoomed out, a row of covers fits across the screen.
 *
 * @property key the stored form.
 * @property label what Settings → TV shows for it.
 * @property scale the factor the screen's density is multiplied by (smaller: more fits, everything smaller).
 */
enum class TvZoom(val key: String, val label: String, val scale: Float) {
    /** The most on screen, for a big TV or sitting close. */
    SMALL("small", "Smaller", 0.70f),

    /** Zoomed out a little: the default. */
    MEDIUM("medium", "Normal", 0.80f),

    /** The phone's own size, for a small TV or sitting far away. */
    LARGE("large", "Larger", 1.0f),
    ;

    companion object {
        /** The zoom stored as [key]; anything unknown is [MEDIUM]. */
        fun of(key: String?): TvZoom = entries.firstOrNull { it.key == key } ?: MEDIUM
    }
}

/**
 * One place the TV side menu goes. The four phone tabs plus the pages the phone keeps behind the Home avatar menu that
 * are worth one press on a TV; Account, Leaderboard, Files and Bug reports stay in that menu, so the side menu keeps to
 * a short list.
 *
 * @property label the word shown beside the icon while the menu is open.
 */
enum class RailItem(val label: String) {
    /** Search and the catalogue (the phone's Discover tab). Listed first: it is what a remote reaches for most. */
    SEARCH("Search"),

    /** The Home tab. */
    HOME("Home"),

    /** The Library tab. */
    LIBRARY("Library"),

    /** The Playlists tab. */
    PLAYLISTS("Playlists"),

    /** For you (ListenBrainz picks), from the phone's Home menu. */
    FOR_YOU("For you"),

    /** The full-screen TV player; listed only while a song is loaded. */
    NOW_PLAYING("Now playing"),

    /** Settings, kept apart at the bottom of the menu. */
    SETTINGS("Settings"),
}

/** A remote button the TV side menu cares about ([TvRules.railKey]). */
enum class RailKey {
    /** D-pad Left: on the page's left edge it opens the menu. */
    LEFT,

    /** D-pad Right: in the open menu it goes back to the page. */
    RIGHT,

    /** The remote's Menu button: opens the menu from anywhere on the page. */
    MENU,
}

/** Plain decisions of the TV layout. */
object TvRules {
    /**
     * Which side menu button a key is, if any.
     *
     * @param keyCode the button's Android key code (`KeyEvent.KEYCODE_…`).
     */
    fun railKey(keyCode: Int): RailKey? = when (keyCode) {
        KeyEvent.KEYCODE_DPAD_LEFT -> RailKey.LEFT
        KeyEvent.KEYCODE_DPAD_RIGHT -> RailKey.RIGHT
        KeyEvent.KEYCODE_MENU -> RailKey.MENU
        else -> null
    }

    /**
     * Whether a key moves the focus up or down on a TV page: true for Down, false for Up, null for any other key.
     * The TV frame moves the focus itself for these before a control sees them, so a text field can't keep them.
     *
     * @param keyCode the button's Android key code (`KeyEvent.KEYCODE_…`).
     */
    fun verticalMove(keyCode: Int): Boolean? = when (keyCode) {
        KeyEvent.KEYCODE_DPAD_DOWN -> true
        KeyEvent.KEYCODE_DPAD_UP -> false
        else -> null
    }

    /** How far one Left / Right press on the TV seek bar moves the song, in ms. */
    const val SEEK_STEP_MS = 10_000L

    /** The side menu's width while closed (icons only), in dp. */
    const val RAIL_CLOSED = 76f

    /** The side menu's width while one of its entries has the focus (icons and words), in dp. */
    const val RAIL_OPEN = 220f

    /** The share of the TV player's width its controls half takes; the switchable panel gets the rest. */
    const val PLAYER_SHARE = 0.42f

    /**
     * Whether the app uses the TV layout.
     *
     * @param mode the setting (Settings → TV → Layout).
     * @param isTv whether this device is a TV (Android's television UI mode, or a leanback device).
     */
    fun useTvLayout(mode: TvLayoutMode, isTv: Boolean): Boolean = isTv && mode == TvLayoutMode.AUTO

    /**
     * The side menu's entries, top to bottom, before [RailItem.SETTINGS] (which always sits apart at the bottom).
     *
     * @param playing whether a song is loaded (adds [RailItem.NOW_PLAYING] right after the tabs).
     */
    fun railItems(playing: Boolean): List<RailItem> = buildList {
        add(RailItem.SEARCH)
        add(RailItem.HOME)
        add(RailItem.LIBRARY)
        add(RailItem.PLAYLISTS)
        add(RailItem.FOR_YOU)
        if (playing) add(RailItem.NOW_PLAYING)
    }

    /**
     * Where one Left / Right press on the TV seek bar lands: [SEEK_STEP_MS] back or forward, kept inside the song.
     * A song without a known length (0) doesn't move.
     *
     * @param positionMs where the song is.
     * @param durationMs the song's length (0 or less: not known yet).
     * @param forward true for Right (forward), false for Left (back).
     */
    fun seekTarget(positionMs: Long, durationMs: Long, forward: Boolean): Long {
        if (durationMs <= 0) return positionMs.coerceAtLeast(0)
        val to = positionMs + if (forward) SEEK_STEP_MS else -SEEK_STEP_MS
        return to.coerceIn(0L, durationMs)
    }

    /**
     * Whether the TV player's controls half comes first (left) in its row.
     *
     * @param side the setting.
     * @param rtl whether the layout runs right to left (Arabic, Hebrew): a Row's start is then on the right, so the
     *   order is flipped to keep "Left" meaning the left of the screen.
     */
    fun controlsFirst(side: PlayerSide, rtl: Boolean): Boolean = (side == PlayerSide.LEFT) != rtl
}
