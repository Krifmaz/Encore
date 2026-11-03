package app.encore.android.ui

/**
 * Compose test tags on the nodes the performance journeys look for (`clients/android/journeys`, PERFORMANCE.md).
 * MainActivity turns on `testTagsAsResourceId`, so UiAutomator finds each tag as a resource id. They only name
 * nodes: nothing on screen changes. Keep them in step with `journeys/.../Journeys.kt` `Tags`.
 */
object PerfTags {
    /** The sign-in flow's server address field. */
    const val SERVER_FIELD = "server_field"
    /** The sign-in screen's username field. */
    const val USERNAME_FIELD = "username_field"
    /** The sign-in screen's password field. */
    const val PASSWORD_FIELD = "password_field"
    /** The Library's paged albums grid. */
    const val LIBRARY_ALBUMS = "library_albums"
    /** An album page's list (header, then its songs). */
    const val ALBUM_TRACKS = "album_tracks"
    /** The mini player's row (a tap opens the full player). */
    const val MINI_PLAYER = "mini_player"
    /** The full-screen player. */
    const val FULL_PLAYER = "full_player"
    /** The Up next list. */
    const val UP_NEXT = "up_next"
}
